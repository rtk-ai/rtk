//! Minimal usage store: a single SQLite table recording one
//! row per forwarded request, for the basic stats endpoints. Deliberately small
//! — no users, groups, budgets or PostgreSQL. Open
//! with `Store::open`, record off the response path with `record`.
//!
//! rusqlite is blocking, so every DB touch runs on `spawn_blocking`; a single
//! `Mutex<Connection>` is plenty for a single-node local daemon.

use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// One recorded request.
#[derive(Debug, Clone, Default)]
pub struct UsageRow {
    /// RFC3339 UTC timestamp.
    pub ts: String,
    pub provider: String,
    pub model: String,
    /// Estimated input tokens (request body).
    pub tokens_in: i64,
    /// Output tokens, when known (0 until response parsing is wired).
    pub tokens_out: i64,
    /// Tokens removed by the compression pipeline this request.
    pub tokens_saved: i64,
    /// USD cost, when known (0.0 until output tokens are recorded).
    pub cost_usd: f64,
    /// Upstream HTTP status.
    pub status: i64,
    /// Detected client (`claude-code`, `codex`, …), best-effort.
    pub client: Option<String>,
    /// Project the request declared via `x-rtk-project`, for attribution.
    pub project: Option<String>,
    /// Session the request declared via `x-rtk-session`. Also CCR's cache scope.
    pub session: Option<String>,
    /// Name of the pipeline whose `proxy.request` rules were active when this
    /// request was handled (resolved once at boot). `rtk gain --by-pipeline`
    /// groups on it.
    pub pipeline: Option<String>,
    /// The model the client asked for before a route transform swapped it;
    /// `None` when no route fired. Prices the routing counterfactual.
    pub routed_from: Option<String>,
    /// The request priced as if the pipeline had not acted (removed tokens
    /// added back, the pre-route model's price); cache fields carried over,
    /// so this side is modeled. 0 when nothing was billed.
    pub cost_without: f64,
    /// Upstream latency (ms) for this request.
    pub latency_ms: i64,
    /// Tokens served from the provider prompt cache (when reported).
    pub cache_read_tokens: i64,
    /// Tokens written to the provider prompt cache (when reported).
    pub cache_creation_tokens: i64,
    /// `rtk_retrieve` calls the proxy resolved for this request -- how often
    /// the model had to go back for content CCR removed. The recovery-cost
    /// side of compression: savings that only show up as extra round trips
    /// are not savings.
    pub ccr_retrieve_calls: i64,
    /// Upstream path (e.g. `/v1/messages`).
    pub path: String,
    /// Latest user prompt text (truncated), when body logging is enabled.
    /// Assistant completion text (truncated), when body logging is enabled.
    /// The compression level (`off`/`light`/`balanced`/`aggressive`) active
    /// when this request was handled -- lets a real-traffic compression
    /// bench correlate `tokens_saved` with the level in effect at the time,
    /// since the level can change live via `PUT /admin/settings`.
    pub compression_level: Option<String>,
    /// Estimated context-composition breakdown of the (post-pipeline)
    /// request body — see `context_analysis::analyze`.
    pub ctx_total: i64,
    pub ctx_tool_result: i64,
    pub ctx_tool_use: i64,
    pub ctx_duplicate: i64,
    pub ctx_image: i64,
    /// `tokens_saved`'s per-method breakdown — which compression transform
    /// removed how much (see `forward::Applied`'s matching fields).
    pub saved_ingest_filter: i64,
    pub saved_pixel_compress: i64,
    pub saved_ccr: i64,
    pub saved_normalize: i64,
}

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS usage_records (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    ts           TEXT    NOT NULL,
    provider     TEXT    NOT NULL,
    model        TEXT    NOT NULL,
    tokens_in    INTEGER NOT NULL DEFAULT 0,
    tokens_out   INTEGER NOT NULL DEFAULT 0,
    tokens_saved INTEGER NOT NULL DEFAULT 0,
    cost_usd     REAL    NOT NULL DEFAULT 0,
    status       INTEGER NOT NULL DEFAULT 0,
    client       TEXT,
    latency_ms            INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens     INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
    ccr_retrieve_calls    INTEGER NOT NULL DEFAULT 0,
    path            TEXT,
    compression_level TEXT,
    ctx_total       INTEGER NOT NULL DEFAULT 0,
    ctx_tool_result INTEGER NOT NULL DEFAULT 0,
    ctx_tool_use    INTEGER NOT NULL DEFAULT 0,
    ctx_duplicate   INTEGER NOT NULL DEFAULT 0,
    ctx_image       INTEGER NOT NULL DEFAULT 0,
    saved_ingest_filter  INTEGER NOT NULL DEFAULT 0,
    saved_pixel_compress INTEGER NOT NULL DEFAULT 0,
    saved_ccr            INTEGER NOT NULL DEFAULT 0,
    saved_normalize      INTEGER NOT NULL DEFAULT 0,
    project TEXT,
    session TEXT,
    pipeline TEXT,
    routed_from TEXT,
    cost_without REAL NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_usage_ts    ON usage_records(ts);
CREATE INDEX IF NOT EXISTS idx_usage_model ON usage_records(model);

CREATE TABLE IF NOT EXISTS ccr_decisions (
    session TEXT NOT NULL,
    block_id TEXT NOT NULL,
    ts TEXT NOT NULL,
    agent TEXT,
    tool TEXT,
    field TEXT,
    normalisers TEXT,
    shape TEXT,
    decided_by TEXT,
    strategy TEXT,
    elided_ranges TEXT,
    tokens_before INTEGER NOT NULL DEFAULT 0,
    tokens_after  INTEGER NOT NULL DEFAULT 0,
    f_indent_sd   INTEGER NOT NULL DEFAULT 0,
    f_prefix_rep  INTEGER NOT NULL DEFAULT 0,
    f_unique_ratio INTEGER NOT NULL DEFAULT 0,
    retrieve_count INTEGER NOT NULL DEFAULT 0,
    pinned INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (session, block_id)
);

CREATE TABLE IF NOT EXISTS app_settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

/// Columns added after the initial release — applied best-effort to an existing
/// DB (a fresh DB already has them from `SCHEMA`; `ADD COLUMN` then no-ops).
const MIGRATIONS: &[&str] = &[
    "ALTER TABLE usage_records ADD COLUMN latency_ms INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE usage_records ADD COLUMN cache_read_tokens INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE usage_records ADD COLUMN cache_creation_tokens INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE usage_records ADD COLUMN ccr_retrieve_calls INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE usage_records ADD COLUMN path TEXT",
    "ALTER TABLE usage_records ADD COLUMN compression_level TEXT",
    "ALTER TABLE usage_records ADD COLUMN ctx_total INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE usage_records ADD COLUMN ctx_tool_result INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE usage_records ADD COLUMN ctx_tool_use INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE usage_records ADD COLUMN ctx_duplicate INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE usage_records ADD COLUMN ctx_image INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE usage_records ADD COLUMN saved_ingest_filter INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE usage_records ADD COLUMN saved_pixel_compress INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE usage_records ADD COLUMN saved_ccr INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE usage_records ADD COLUMN project TEXT",
    "ALTER TABLE usage_records ADD COLUMN session TEXT",
    "ALTER TABLE usage_records ADD COLUMN pipeline TEXT",
    "ALTER TABLE usage_records ADD COLUMN saved_normalize INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE usage_records ADD COLUMN routed_from TEXT",
    "ALTER TABLE usage_records ADD COLUMN cost_without REAL NOT NULL DEFAULT 0",
];

impl Store {
    /// Open (or create) the SQLite DB at `path` (`:memory:` for an ephemeral one)
    /// and ensure the schema.
    pub fn open(path: &str) -> anyhow::Result<Self> {
        let conn = if path == ":memory:" {
            Connection::open_in_memory()?
        } else {
            Connection::open(path)?
        };
        conn.execute_batch(SCHEMA)?;
        for stmt in MIGRATIONS {
            let _ = conn.execute(stmt, []);
        }
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// All persisted runtime settings (`app_settings`), read synchronously — used
    /// once at boot to overlay the config seed. Empty on any error.
    pub fn all_settings_sync(&self) -> HashMap<String, String> {
        let c = self.conn.lock().expect("store mutex");
        let mut out = HashMap::new();
        if let Ok(mut stmt) = c.prepare("SELECT key, value FROM app_settings") {
            if let Ok(rows) =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            {
                for kv in rows.flatten() {
                    out.insert(kv.0, kv.1);
                }
            }
        }
        out
    }

    /// Upsert one runtime setting (persisted across restarts).
    pub async fn put_setting(&self, key: &str, value: &str) {
        let conn = self.conn.clone();
        let (key, value) = (key.to_string(), value.to_string());
        let res = tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
            let c = conn.lock().expect("store mutex");
            c.execute(
                "INSERT INTO app_settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
            Ok(())
        })
        .await;
        if let Ok(Err(e)) = res {
            tracing::warn!(error = %e, "setting upsert failed");
        }
    }

    /// Insert one usage row. Fire-and-forget off the response path: errors are
    /// logged, never propagated (recording must not fail a request).
    pub async fn record(&self, row: UsageRow) {
        let conn = self.conn.clone();
        let res = tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
            let c = conn.lock().expect("store mutex");
            c.execute(
                "INSERT INTO usage_records
                   (ts, provider, model, tokens_in, tokens_out, tokens_saved, cost_usd, status, client,
                    latency_ms, cache_read_tokens, cache_creation_tokens, ccr_retrieve_calls,
                    path,
                    compression_level, ctx_total, ctx_tool_result, ctx_tool_use, ctx_duplicate, ctx_image,
                    saved_ingest_filter, saved_pixel_compress, saved_ccr, saved_normalize, project, session, pipeline,
                    routed_from, cost_without)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29)",
                params![
                    row.ts, row.provider, row.model, row.tokens_in, row.tokens_out,
                    row.tokens_saved, row.cost_usd, row.status, row.client,
                    row.latency_ms, row.cache_read_tokens, row.cache_creation_tokens,
                    row.ccr_retrieve_calls,
                    row.path, row.compression_level,
                    row.ctx_total, row.ctx_tool_result, row.ctx_tool_use, row.ctx_duplicate, row.ctx_image,
                    row.saved_ingest_filter, row.saved_pixel_compress, row.saved_ccr,
                    row.saved_normalize, row.project, row.session, row.pipeline,
                    row.routed_from, row.cost_without
                ],
            )?;
            Ok(())
        })
        .await;
        if let Ok(Err(e)) = res {
            tracing::warn!(error = %e, "usage record insert failed");
        }
    }

    /// Run a read query on the blocking pool, mapping the connection to a JSON
    /// value. Returns `null` on error (stats must never fail a caller's read).
    async fn query<F>(&self, f: F) -> serde_json::Value
    where
        F: FnOnce(&Connection) -> rusqlite::Result<serde_json::Value> + Send + 'static,
    {
        let conn = self.conn.clone();
        match tokio::task::spawn_blocking(move || f(&conn.lock().expect("store mutex"))).await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "stats query failed");
                serde_json::Value::Null
            }
            Err(_) => serde_json::Value::Null,
        }
    }

    /// Totals across all recorded requests.
    pub async fn summary(&self) -> serde_json::Value {
        self.query(|c| {
            c.query_row(
                "SELECT count(*), COALESCE(sum(tokens_in),0), COALESCE(sum(tokens_out),0),
                        COALESCE(sum(tokens_saved),0),
                        COALESCE(sum(CASE WHEN status < 400 THEN cost_usd ELSE 0 END),0),
                        COALESCE(sum(CASE WHEN status >= 400 THEN 1 ELSE 0 END),0)
                 FROM usage_records",
                [],
                |r| {
                    Ok(serde_json::json!({
                        "total_requests": r.get::<_, i64>(0)?,
                        "tokens_in":      r.get::<_, i64>(1)?,
                        "tokens_out":     r.get::<_, i64>(2)?,
                        "tokens_saved":   r.get::<_, i64>(3)?,
                        "cost_usd":       r.get::<_, f64>(4)?,
                        "failed_requests": r.get::<_, i64>(5)?,
                    }))
                },
            )
        })
        .await
    }

    /// Aggregate context-composition breakdown across every recorded
    /// request (see `context_analysis::analyze`) -- what the context window
    /// is actually made of: tool output, tool-call args, images, or plain
    /// text/system/tool-defs (the `other` residual, computed by the caller
    /// as `total - tool_result - tool_use - image`). `duplicate` is a
    /// cross-cutting overlay (bytes that also live inside one of the other
    /// slices), not part of that partition.
    /// `saved_*` is the same `tokens_saved` total, broken down by which
    /// compression transform removed it (see `forward::Applied`'s matching
    /// fields) -- lets the composition panel show "removed before reaching
    /// the model" per method, not just one aggregate figure.
    pub async fn context_summary(&self) -> serde_json::Value {
        self.query(|c| {
            c.query_row(
                "SELECT COALESCE(sum(ctx_total),0), COALESCE(sum(ctx_tool_result),0),
                        COALESCE(sum(ctx_tool_use),0), COALESCE(sum(ctx_duplicate),0),
                        COALESCE(sum(ctx_image),0), COALESCE(sum(saved_ingest_filter),0),
                        COALESCE(sum(saved_pixel_compress),0), COALESCE(sum(saved_ccr),0),
                        COALESCE(sum(saved_normalize),0)
                 FROM usage_records",
                [],
                |r| {
                    Ok(serde_json::json!({
                        "ctx_total":            r.get::<_, i64>(0)?,
                        "ctx_tool_result":      r.get::<_, i64>(1)?,
                        "ctx_tool_use":         r.get::<_, i64>(2)?,
                        "ctx_duplicate":        r.get::<_, i64>(3)?,
                        "ctx_image":            r.get::<_, i64>(4)?,
                        "saved_ingest_filter":  r.get::<_, i64>(5)?,
                        "saved_pixel_compress": r.get::<_, i64>(6)?,
                        "saved_ccr":            r.get::<_, i64>(7)?,
                        "saved_normalize":      r.get::<_, i64>(8)?,
                    }))
                },
            )
        })
        .await
    }

    /// Per-model breakdown (requests + tokens saved), busiest first.
    pub async fn by_model(&self) -> serde_json::Value {
        self.query(|c| {
            let mut stmt = c.prepare(
                "SELECT model, count(*) n, COALESCE(sum(tokens_saved),0) saved
                 FROM usage_records GROUP BY model ORDER BY n DESC",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(serde_json::json!({
                        "model": r.get::<_, String>(0)?,
                        "requests": r.get::<_, i64>(1)?,
                        "tokens_saved": r.get::<_, i64>(2)?,
                    }))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(serde_json::Value::Array(rows))
        })
        .await
    }

    /// Per-compression-level breakdown -- for a real-traffic compression
    /// bench: compare `avg_tokens_saved`/`avg_pct` across `off`/`light`/
    /// `balanced`/`aggressive` as the operator changes the live setting over
    /// time (each row already remembers which level was active, see
    /// `UsageRow::compression_level`). Rows recorded before this column
    /// existed group under `"unknown"`.
    pub async fn by_compression_level(&self) -> serde_json::Value {
        self.query(|c| {
            let mut stmt = c.prepare(
                "SELECT COALESCE(compression_level, 'unknown') AS level,
                        count(*) n,
                        COALESCE(sum(tokens_in),0) tin,
                        COALESCE(sum(tokens_out),0) tout,
                        COALESCE(sum(tokens_saved),0) saved
                 FROM usage_records GROUP BY level ORDER BY n DESC",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    let n: i64 = r.get(1)?;
                    let tin: i64 = r.get(2)?;
                    let saved: i64 = r.get(4)?;
                    let avg_pct = if tin + saved > 0 {
                        saved as f64 / (tin + saved) as f64 * 100.0
                    } else {
                        0.0
                    };
                    Ok(serde_json::json!({
                        "compression_level": r.get::<_, String>(0)?,
                        "requests": n,
                        "tokens_in": tin,
                        "tokens_out": r.get::<_, i64>(3)?,
                        "tokens_saved": saved,
                        "avg_savings_pct": avg_pct,
                    }))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(serde_json::Value::Array(rows))
        })
        .await
    }

    /// Per-day request counts + tokens saved (UTC date), last 30 days.
    pub async fn by_day(&self) -> serde_json::Value {
        self.query(|c| {
            let mut stmt = c.prepare(
                "SELECT substr(ts,1,10) day, count(*) n, COALESCE(sum(tokens_saved),0) saved
                 FROM usage_records GROUP BY day ORDER BY day DESC LIMIT 30",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(serde_json::json!({
                        "day": r.get::<_, String>(0)?,
                        "requests": r.get::<_, i64>(1)?,
                        "tokens_saved": r.get::<_, i64>(2)?,
                    }))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(serde_json::Value::Array(rows))
        })
        .await
    }
}

/// Aggregate totals for the `/metrics` endpoint.
/// One decision-log row, decoupled from the ccr module's internal type.
#[derive(Debug, Clone, Default)]
pub struct DecisionRow {
    pub session: String,
    pub block_id: String,
    pub tool: Option<String>,
    pub field: Option<String>,
    pub normalisers: String,
    pub shape: String,
    pub decided_by: String,
    pub strategy: String,
    pub elided_ranges: String,
    pub tokens_before: i64,
    pub tokens_after: i64,
    pub f_indent_sd: i64,
    pub f_prefix_rep: i64,
    pub f_unique_ratio: i64,
}

#[derive(Debug, Clone, Default)]
pub struct MetricTotals {
    pub requests: i64,
    pub errors: i64,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub cache_read: i64,
    pub cache_creation: i64,
    pub tokens_saved: i64,
    pub cost_usd: f64,
    pub latency_ms_sum: i64,
}

/// Per-model counts for the `/metrics` endpoint.
#[derive(Debug, Clone, Default)]
pub struct ModelMetric {
    pub model: String,
    pub requests: i64,
    pub tokens_in: i64,
    pub tokens_out: i64,
}

/// Per-client counts for the `/metrics` endpoint.
#[derive(Debug, Clone, Default)]
pub struct ClientMetric {
    pub client: String,
    pub requests: i64,
}

/// Everything the Prometheus exposition needs, in typed form.
#[derive(Debug, Clone, Default)]
pub struct MetricsSnapshot {
    pub totals: MetricTotals,
    pub by_model: Vec<ModelMetric>,
    pub by_client: Vec<ClientMetric>,
    pub cache: CacheHealth,
}

/// Prompt-cache health. The proxy rewrites request bodies, so a marker that
/// is not byte-stable across turns silently invalidates the upstream prefix
/// and costs far more than compression saves. These are the numbers that make
/// that visible instead of invisible.
#[derive(Debug, Clone, Default)]
pub struct CacheHealth {
    /// `read / (read + created)` over all recorded requests. A healthy corpus
    /// sits near 1; sustained decline means the prefix is churning.
    pub hit_ratio: f64,
    /// Mid-session prefix invalidations: the previous request in the session
    /// read a large cached prefix, this one read nothing and rewrote one.
    pub fractures: i64,
    /// Sessions with at least two recorded requests (the fracture denominator).
    pub sessions: i64,
}

/// A request reading fewer cached tokens than this is not treated as having
/// had a prefix worth losing, so trivial openers cannot register as fractures.
const FRACTURE_MIN_PREFIX_TOKENS: i64 = 4096;

/// An entry that simply aged out over an idle gap looks exactly like a
/// fracture from the token counts alone, so anything past the provider TTL is
/// excluded.
const FRACTURE_MAX_IDLE_SECONDS: f64 = 3600.0;

impl Store {
    /// Persist a batch of CCR classifier decisions, one row per unique
    /// `(session, block_id)`. `INSERT OR IGNORE` so a recurring block is
    /// logged once. Best-effort and fail-open: a logging failure must never
    /// affect the request or the replacement.
    pub async fn record_decisions(&self, agent: &str, ts: &str, rows: Vec<DecisionRow>) {
        if rows.is_empty() {
            return;
        }
        let conn = self.conn.clone();
        let agent = agent.to_string();
        let ts = ts.to_string();
        let _ = tokio::task::spawn_blocking(move || {
            let c = conn.lock().expect("store mutex");
            for r in rows {
                let _ = c.execute(
                    "INSERT OR IGNORE INTO ccr_decisions
                       (session, block_id, ts, agent, tool, field, normalisers, shape,
                        decided_by, strategy, elided_ranges, tokens_before, tokens_after,
                        f_indent_sd, f_prefix_rep, f_unique_ratio)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
                    params![
                        r.session,
                        r.block_id,
                        ts,
                        agent,
                        r.tool,
                        r.field,
                        r.normalisers,
                        r.shape,
                        r.decided_by,
                        r.strategy,
                        r.elided_ranges,
                        r.tokens_before,
                        r.tokens_after,
                        r.f_indent_sd,
                        r.f_prefix_rep,
                        r.f_unique_ratio
                    ],
                );
            }
        })
        .await;
    }

    /// Record that a compressed block was retrieved: the classifier's
    /// consequence signal. Fail-open.
    pub async fn bump_retrieve(&self, session: &str, block_id: &str, pinned: bool) {
        let conn = self.conn.clone();
        let (session, block_id) = (session.to_string(), block_id.to_string());
        let _ = tokio::task::spawn_blocking(move || {
            let c = conn.lock().expect("store mutex");
            let _ = c.execute(
                "UPDATE ccr_decisions SET retrieve_count = retrieve_count + 1,                  pinned = MAX(pinned, ?3) WHERE session = ?1 AND block_id = ?2",
                params![session, block_id, pinned as i64],
            );
        })
        .await;
    }

    /// Typed aggregate snapshot for `/metrics` (totals + per-model + per-client).
    /// One blocking hop, three queries; returns an empty snapshot on any error.
    pub async fn metrics(&self) -> MetricsSnapshot {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().expect("store mutex");
            let mut snap = MetricsSnapshot::default();

            if let Ok(t) = c.query_row(
                "SELECT count(*),
                        COALESCE(sum(CASE WHEN status >= 400 THEN 1 ELSE 0 END),0),
                        COALESCE(sum(tokens_in),0), COALESCE(sum(tokens_out),0),
                        COALESCE(sum(cache_read_tokens),0), COALESCE(sum(cache_creation_tokens),0),
                        COALESCE(sum(tokens_saved),0),
                        COALESCE(sum(CASE WHEN status < 400 THEN cost_usd ELSE 0 END),0),
                        COALESCE(sum(latency_ms),0)
                 FROM usage_records",
                [],
                |r| {
                    Ok(MetricTotals {
                        requests: r.get(0)?,
                        errors: r.get(1)?,
                        tokens_in: r.get(2)?,
                        tokens_out: r.get(3)?,
                        cache_read: r.get(4)?,
                        cache_creation: r.get(5)?,
                        tokens_saved: r.get(6)?,
                        cost_usd: r.get(7)?,
                        latency_ms_sum: r.get(8)?,
                    })
                },
            ) {
                snap.totals = t;
            }

            if let Ok(mut stmt) = c.prepare(
                "SELECT model, count(*), COALESCE(sum(tokens_in),0), COALESCE(sum(tokens_out),0)
                 FROM usage_records GROUP BY model ORDER BY count(*) DESC",
            ) {
                if let Ok(rows) = stmt.query_map([], |r| {
                    Ok(ModelMetric {
                        model: r.get(0)?,
                        requests: r.get(1)?,
                        tokens_in: r.get(2)?,
                        tokens_out: r.get(3)?,
                    })
                }) {
                    snap.by_model = rows.flatten().collect();
                }
            }

            if let Ok(mut stmt) = c.prepare(
                "SELECT COALESCE(client,'unknown'), count(*)
                 FROM usage_records GROUP BY client ORDER BY count(*) DESC",
            ) {
                if let Ok(rows) = stmt.query_map([], |r| {
                    Ok(ClientMetric {
                        client: r.get(0)?,
                        requests: r.get(1)?,
                    })
                }) {
                    snap.by_client = rows.flatten().collect();
                }
            }

            let denom = snap.totals.cache_read + snap.totals.cache_creation;
            if denom > 0 {
                snap.cache.hit_ratio = snap.totals.cache_read as f64 / denom as f64;
            }

            if let Ok((fractures, sessions)) = c.query_row(
                "WITH seq AS (
                     SELECT session,
                            cache_read_tokens AS rd,
                            cache_creation_tokens AS wr,
                            LAG(cache_read_tokens) OVER (
                                PARTITION BY session ORDER BY ts, id
                            ) AS prev_rd,
                            (julianday(ts) - julianday(LAG(ts) OVER (
                                PARTITION BY session ORDER BY ts, id
                            ))) * 86400.0 AS idle_s
                     FROM usage_records
                     WHERE session IS NOT NULL AND status < 400
                 )
                 SELECT
                     COALESCE(sum(CASE WHEN prev_rd > ?1 AND rd = 0 AND wr > ?1
                                            AND idle_s IS NOT NULL AND idle_s < ?2
                                       THEN 1 ELSE 0 END), 0),
                     COUNT(DISTINCT CASE WHEN prev_rd IS NOT NULL THEN session END)
                 FROM seq",
                rusqlite::params![FRACTURE_MIN_PREFIX_TOKENS, FRACTURE_MAX_IDLE_SECONDS],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            ) {
                snap.cache.fractures = fractures;
                snap.cache.sessions = sessions;
            }

            snap
        })
        .await
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn rec(s: &Store, session: &str, ts: &str, read: i64, created: i64) {
        s.record(UsageRow {
            ts: ts.into(),
            provider: "anthropic".into(),
            model: "claude-opus-5".into(),
            status: 200,
            session: Some(session.into()),
            cache_read_tokens: read,
            cache_creation_tokens: created,
            ..Default::default()
        })
        .await;
    }

    #[tokio::test]
    async fn context_summary_breakdown_includes_normalize_and_reconciles() {
        let s = Store::open(":memory:").unwrap();
        s.record(UsageRow {
            ts: "2026-01-01T00:00:00Z".into(),
            provider: "anthropic".into(),
            model: "claude-opus-5".into(),
            status: 200,
            tokens_saved: 100,
            saved_ingest_filter: 10,
            saved_pixel_compress: 20,
            saved_ccr: 30,
            saved_normalize: 40,
            ..Default::default()
        })
        .await;

        let ctx = s.context_summary().await;
        assert_eq!(ctx["saved_normalize"], 40);
        let breakdown = ctx["saved_ingest_filter"].as_i64().unwrap()
            + ctx["saved_pixel_compress"].as_i64().unwrap()
            + ctx["saved_ccr"].as_i64().unwrap()
            + ctx["saved_normalize"].as_i64().unwrap();
        assert_eq!(
            breakdown,
            s.summary().await["tokens_saved"].as_i64().unwrap(),
            "per-method breakdown must reconcile with tokens_saved"
        );
    }

    #[tokio::test]
    async fn cache_health_spots_a_mid_session_fracture() {
        let s = Store::open(":memory:").unwrap();
        // The fracture signature: a warm prefix, then a turn that reads
        // nothing and rewrites the whole thing.
        rec(&s, "s1", "2026-01-01T00:00:01Z", 0, 20_000).await;
        rec(&s, "s1", "2026-01-01T00:00:02Z", 18_232, 627).await;
        rec(&s, "s1", "2026-01-01T00:00:03Z", 0, 20_783).await;

        let snap = s.metrics().await;
        assert_eq!(snap.cache.fractures, 1);
        assert_eq!(snap.cache.sessions, 1);
        assert!(snap.cache.hit_ratio > 0.0 && snap.cache.hit_ratio < 1.0);
    }

    #[tokio::test]
    async fn healthy_session_reports_no_fracture() {
        let s = Store::open(":memory:").unwrap();
        rec(&s, "s1", "2026-01-01T00:00:01Z", 0, 20_000).await;
        rec(&s, "s1", "2026-01-01T00:00:02Z", 20_000, 800).await;
        rec(&s, "s1", "2026-01-01T00:00:03Z", 20_800, 900).await;

        let snap = s.metrics().await;
        assert_eq!(snap.cache.fractures, 0);
        assert!(snap.cache.hit_ratio > 0.65, "got {}", snap.cache.hit_ratio);
    }

    #[tokio::test]
    async fn ttl_expiry_is_not_counted_as_a_fracture() {
        let s = Store::open(":memory:").unwrap();
        // Same token signature as a fracture, but two hours apart: the entry
        // aged out, nothing rewrote the prefix.
        rec(&s, "s1", "2026-01-01T00:00:00Z", 0, 20_000).await;
        rec(&s, "s1", "2026-01-01T00:00:30Z", 18_232, 627).await;
        rec(&s, "s1", "2026-01-01T02:30:00Z", 0, 20_783).await;

        let snap = s.metrics().await;
        assert_eq!(snap.cache.fractures, 0);
    }

    #[tokio::test]
    async fn cold_open_alone_is_not_a_fracture() {
        let s = Store::open(":memory:").unwrap();
        rec(&s, "s1", "2026-01-01T00:00:01Z", 0, 50_000).await;
        rec(&s, "s2", "2026-01-01T00:00:02Z", 0, 50_000).await;

        let snap = s.metrics().await;
        assert_eq!(snap.cache.fractures, 0);
        assert_eq!(snap.cache.sessions, 0);
    }

    #[tokio::test]
    async fn open_and_record() {
        let s = Store::open(":memory:").unwrap();
        s.record(UsageRow {
            ts: "2026-01-01T00:00:00Z".into(),
            provider: "anthropic".into(),
            model: "claude-haiku-4-5".into(),
            tokens_in: 1200,
            tokens_saved: 300,
            status: 200,
            pipeline: Some("my-pipe".into()),
            ..Default::default()
        })
        .await;
        // Count back the row (blocking, but fine in a test).
        let n: i64 = s
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM usage_records", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        let pipe: Option<String> = s
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT pipeline FROM usage_records", [], |r| r.get(0))
            .unwrap();
        assert_eq!(pipe.as_deref(), Some("my-pipe"));
    }
}
