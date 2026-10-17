//! `rtk llm-proxy`: lifecycle for the `rtk-proxy` daemon (crate `proxy/`).
//!
//! The daemon is a **separate binary**, not a library linked into `rtk`. Two
//! reasons, and both matter:
//!
//!   * `rtk` is a synchronous CLI with a <10ms startup budget; the proxy is a
//!     tokio/axum server. Linking them would drag an async runtime and a TLS
//!     stack into every `rtk ls`.
//!   * They are licensed differently — `rtk` is Apache-2.0, `proxy/` is
//!     BUSL-1.1 (see `proxy/LICENSE`). Both link the BUSL-1.1 `engine/`
//!     crate, but the daemon itself stays a separate artifact.
//!
//! So this module locates the sibling binary, spawns it detached, and
//! otherwise forwards straight through to its own CLI: `stop`/`status`/
//! `stats`/`config`/`service` are already thin HTTP clients over there, so
//! there is nothing to re-implement here.
//!
//! The one thing `rtk` reads directly is the proxy's SQLite usage DB —
//! [`read_gain_summary`], for `rtk gain`. Read-only and best-effort: `rtk
//! gain` must never fail or block because the proxy is mid-write.

use crate::core::utils::tool_exists;
use anyhow::{Context, Result};
use clap::Subcommand;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const DEFAULT_BIND: &str = "127.0.0.1:8080";

/// The daemon binary's name, as installed next to `rtk`.
const PROXY_BIN: &str = if cfg!(windows) {
    "rtk-proxy.exe"
} else {
    "rtk-proxy"
};

#[derive(Debug, Subcommand)]
pub enum LlmProxySubcommand {
    /// Check the daemon is available and report which AI agent(s) on this
    /// machine can be pointed at it
    Setup,
    /// Launch the proxy as a background daemon (no-op if already running)
    Start {
        /// Address to bind (default 127.0.0.1:8080)
        #[arg(long)]
        bind: Option<String>,
        /// Print `export ...` lines instead of a human status report (for
        /// `eval "$(rtk llm-proxy start --env)"`)
        #[arg(long)]
        env: bool,
    },
    /// Ask a running proxy to shut down
    Stop {
        #[arg(long)]
        bind: Option<String>,
    },
    /// Check whether the proxy is running and reachable
    Status {
        #[arg(long)]
        bind: Option<String>,
    },
    /// Print usage stats from a running proxy
    Stats {
        #[arg(long)]
        bind: Option<String>,
    },
    /// Get/set runtime settings on a running proxy (forwarded to `rtk-proxy config`)
    Config {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Manage a per-user background service (forwarded to `rtk-proxy service`)
    Service {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

pub fn run(cmd: &LlmProxySubcommand) -> Result<()> {
    match cmd {
        LlmProxySubcommand::Setup => run_setup(),
        LlmProxySubcommand::Start { bind, env } => run_start(bind.as_deref(), *env),
        LlmProxySubcommand::Stop { bind } => forward("stop", bind.as_deref(), &[]),
        LlmProxySubcommand::Status { bind } => forward("status", bind.as_deref(), &[]),
        LlmProxySubcommand::Stats { bind } => forward("stats", bind.as_deref(), &[]),
        LlmProxySubcommand::Config { args } => forward_raw("config", args),
        LlmProxySubcommand::Service { args } => forward_raw("service", args),
    }
}

/// Where `rtk` keeps the daemon's log and databases. One fixed location, so
/// the daemon's state doesn't depend on the shell's cwd when it was started.
fn state_dir() -> Result<PathBuf> {
    let dir = crate::core::user_dirs::config()
        .context("could not resolve a user config directory")?
        .join("llm-proxy");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

/// The proxy's usage DB — the file [`read_gain_summary`] reads and the one
/// [`ensure_running`] points the daemon at.
fn db_path() -> Option<PathBuf> {
    Some(
        crate::core::user_dirs::config()?
            .join("llm-proxy")
            .join("rtk-proxy.db"),
    )
}

/// Aggregate usage totals from the proxy's own SQLite DB (`usage_records`),
/// for `rtk gain` to show alongside RTK's command-filtering savings — these
/// are two different kinds of savings (LLM request/response token compression
/// vs. dev-command output filtering) tracked in two separate databases, so
/// this stays its own section rather than one merged number.
///
/// Deliberately excludes `cost_usd`: that column is a static per-model price
/// table lookup, not a real billed amount, and showing a specific dollar
/// figure without heavy caveats reads as ground truth to anyone glancing at
/// the dashboard. `rtk llm-proxy stats` still prints it, already labeled
/// "estimated cost" there.
#[derive(Debug, Clone, Default)]
pub struct ProxyGainSummary {
    pub total_requests: i64,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub tokens_saved: i64,
    /// How many requests the transforms actually changed. There is no honest
    /// stored denominator for a trim percentage: `tokens_in` is dominated by
    /// cached prefix, and the composition columns re-count the whole history
    /// on every request while `tokens_saved` counts only fresh removals — so
    /// the display shows absolutes here and leaves ratios to the pipeline
    /// ledger, which measures per mechanism on the same trajectory.
    pub trimmed_requests: i64,
    /// Context-composition breakdown across every recorded request (see the
    /// proxy's `context_analysis::analyze`) — estimated tokens by what's
    /// actually filling the context window. Zero on a DB predating this
    /// feature (the columns default to 0), which `ctx_total == 0` signals to
    /// the caller as "no composition data yet".
    pub ctx_total: i64,
    pub ctx_tool_result: i64,
    pub ctx_tool_use: i64,
    pub ctx_duplicate: i64,
    pub ctx_image: i64,
    /// `tokens_saved`'s per-method breakdown — which proxy-side transform
    /// removed how much. The OSS daemon ships CCR and category-A normalize;
    /// the other columns exist so a build with more stages reports through
    /// the same schema (see the proxy's `server::record_usage`).
    pub saved_ingest_filter: i64,
    pub saved_pixel_compress: i64,
    pub saved_ccr: i64,
    pub saved_normalize: i64,
    /// Provider prompt-cache totals. The proxy rewrites request bodies, so a
    /// replacement that is not byte-stable across turns invalidates the
    /// upstream prefix — which costs far more than compression saves and is
    /// otherwise invisible. `cache_read == 0 && cache_created == 0` means a DB
    /// predating these columns, not a cold cache.
    pub cache_read: i64,
    pub cache_created: i64,
    /// Mid-session prefix invalidations over the most recent requests: the
    /// previous request in the session read a large cached prefix, this one
    /// read nothing and rewrote one, close enough in time to rule out the
    /// entry simply expiring. Bounded — see [`FRACTURE_SCAN_ROWS`].
    pub cache_fractures: i64,
    /// `rtk_retrieve` round trips the proxy served — how often the model had
    /// to go back for content CCR removed. Savings that only reappear as extra
    /// round trips are not savings, so this is read next to `tokens_saved`.
    pub retrieve_calls: i64,
    /// Requests the provider answered with an error (status ≥ 400). They
    /// billed nothing and are kept out of every cost figure; the count is
    /// shown so a failing session cannot hide inside quiet totals.
    pub failed_requests: i64,
    /// Observed spend (provider usage × registry price) across billed
    /// requests, and the same requests priced as if the pipeline had not
    /// acted (removed tokens added back, pre-route model). Both zero on a DB
    /// predating the `cost_without` column.
    pub cost_actual_usd: f64,
    pub cost_without_usd: f64,
    /// Requests a model-route transform redirected, and their billed input
    /// tokens — the volume behind the routing share of `cost_without`.
    pub routed_requests: i64,
    pub routed_tokens_in: i64,
    /// Dollars the routed requests saved: their `cost_without` (priced at the
    /// model each one displaced) minus what they actually billed. The routing
    /// share of actual-vs-without, split out so the proxy panel can show it —
    /// routing moves tokens to a cheaper biller rather than removing them, so
    /// it is invisible in `tokens_saved` by construction.
    pub routed_saved_usd: f64,
    /// What the routed requests actually billed — the measured half of the
    /// comparison, shown exactly; `routed_saved_usd` is the modeled half.
    pub routed_cost_usd: f64,
}

impl ProxyGainSummary {
    /// Cached share of prefix tokens, `None` when nothing has been recorded.
    /// A healthy cache stays high; sustained decline means the prefix is
    /// churning.
    pub fn cache_hit_ratio(&self) -> Option<f64> {
        let denom = self.cache_read + self.cache_created;
        (denom > 0).then(|| self.cache_read as f64 / denom as f64)
    }

    /// How much the prompt cache cut effective input cost, as a fraction of
    /// what the same input would bill uncached: raw input is `tokens_in +
    /// reads + writes`, billed-equivalent weighs reads at 0.1x and writes at
    /// 1.25x (Anthropic's published multipliers). A logical comparison of
    /// provider-reported counts — no price registry involved, so it carries
    /// across users — but the multipliers are Anthropic's, so a mixed-provider
    /// store makes this an estimate, and it is labeled as one.
    pub fn cache_effect_pct(&self) -> Option<f64> {
        let raw = self.tokens_in + self.cache_read + self.cache_created;
        if raw <= 0 || self.cache_read + self.cache_created <= 0 {
            return None;
        }
        let billed =
            self.tokens_in as f64 + self.cache_read as f64 * 0.1 + self.cache_created as f64 * 1.25;
        Some((1.0 - billed / raw as f64) * 100.0)
    }

    /// The per-method breakdown, in display order, with never-used methods
    /// dropped — so a CCR-only daemon shows one line, not three with two
    /// permanent zeroes.
    pub fn savings_by_method(&self) -> Vec<(&'static str, i64)> {
        [
            ("ccr", self.saved_ccr),
            ("normalize", self.saved_normalize),
            ("ingest-filter", self.saved_ingest_filter),
            ("pixel-compress", self.saved_pixel_compress),
        ]
        .into_iter()
        .filter(|(_, v)| *v > 0)
        .collect()
    }
}

/// Best-effort, read-only: `None` whenever the proxy has never run (no DB
/// yet) or the DB can't be read (e.g. briefly locked by a live writer) —
/// `rtk gain` should never fail or block because of this.
pub fn read_gain_summary() -> Option<ProxyGainSummary> {
    let path = db_path()?;
    if !path.is_file() {
        return None;
    }
    read_gain_summary_at(&path)
}

/// Requests scanned for the fracture count. `rtk gain` has a <10ms budget and
/// the partitioned window sort is the expensive part: unbounded, it runs far
/// past that budget on a large DB, even with a covering index. A recent window is also the better signal — the
/// point is catching a change that starts churning the prefix, not totalling
/// history.
const FRACTURE_SCAN_ROWS: i64 = 2000;

/// Mid-session prefix invalidations over the last [`FRACTURE_SCAN_ROWS`]
/// requests. An entry that merely aged out over an idle gap is
/// token-for-token identical to a rewritten prefix, so anything past the
/// provider's 1h TTL is excluded; most unguarded candidates are expiries.
fn count_cache_fractures(conn: &rusqlite::Connection) -> Option<i64> {
    conn.query_row(
        "WITH recent AS ( \
             SELECT ts, id, session, cache_read_tokens, cache_creation_tokens \
             FROM usage_records WHERE session IS NOT NULL AND status < 400 \
             ORDER BY id DESC LIMIT ?1 \
         ), seq AS ( \
             SELECT cache_read_tokens AS rd, cache_creation_tokens AS wr, \
                    LAG(cache_read_tokens) OVER w AS prev_rd, \
                    (julianday(ts) - julianday(LAG(ts) OVER w)) * 86400.0 AS idle_s \
             FROM recent \
             WINDOW w AS (PARTITION BY session ORDER BY ts, id) \
         ) \
         SELECT COALESCE(SUM(CASE WHEN prev_rd > 4096 AND rd = 0 AND wr > 4096 \
                                       AND idle_s IS NOT NULL AND idle_s < 3600.0 \
                                  THEN 1 ELSE 0 END), 0) FROM seq",
        [FRACTURE_SCAN_ROWS],
        |r| r.get(0),
    )
    .ok()
}

/// [`read_gain_summary`] against an explicit path, so the query is testable
/// without depending on the caller's config directory.
fn read_gain_summary_at(db: &std::path::Path) -> Option<ProxyGainSummary> {
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()?;
    conn.busy_timeout(Duration::from_millis(500)).ok()?;
    // One pass over the table, newest columns included. A DB written by an
    // older proxy lacks them and this connection is read-only, so it cannot be
    // migrated here -- fall back to the older column list rather than losing
    // the whole summary over a few missing sums.
    let totals = |cache: bool| {
        let cache_cols = if cache {
            ", COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_creation_tokens),0), \
               COALESCE(SUM(ccr_retrieve_calls),0), COALESCE(SUM(saved_normalize),0)"
        } else {
            ", 0, 0, 0, 0"
        };
        conn.query_row(
            &format!(
                "SELECT COUNT(*), COALESCE(SUM(tokens_in),0), COALESCE(SUM(tokens_out),0), \
                 COALESCE(SUM(tokens_saved),0), COALESCE(SUM(ctx_total),0), \
                 COALESCE(SUM(ctx_tool_result),0), COALESCE(SUM(ctx_tool_use),0), \
                 COALESCE(SUM(ctx_duplicate),0), COALESCE(SUM(ctx_image),0), \
                 COALESCE(SUM(saved_ingest_filter),0), COALESCE(SUM(saved_pixel_compress),0), \
                 COALESCE(SUM(saved_ccr),0), \
                 COALESCE(SUM(CASE WHEN status >= 400 THEN 1 ELSE 0 END),0){cache_cols} \
                 FROM usage_records"
            ),
            [],
            |r| {
                Ok(ProxyGainSummary {
                    total_requests: r.get(0)?,
                    tokens_in: r.get(1)?,
                    tokens_out: r.get(2)?,
                    tokens_saved: r.get(3)?,
                    ctx_total: r.get(4)?,
                    ctx_tool_result: r.get(5)?,
                    ctx_tool_use: r.get(6)?,
                    ctx_duplicate: r.get(7)?,
                    ctx_image: r.get(8)?,
                    saved_ingest_filter: r.get(9)?,
                    saved_pixel_compress: r.get(10)?,
                    saved_ccr: r.get(11)?,
                    failed_requests: r.get(12)?,
                    cache_read: r.get(13)?,
                    cache_created: r.get(14)?,
                    cache_fractures: 0,
                    retrieve_calls: r.get(15)?,
                    saved_normalize: r.get(16)?,
                    cost_actual_usd: 0.0,
                    cost_without_usd: 0.0,
                    trimmed_requests: 0,
                    routed_requests: 0,
                    routed_tokens_in: 0,
                    routed_saved_usd: 0.0,
                    routed_cost_usd: 0.0,
                })
            },
        )
        .ok()
    };

    let mut s = totals(true).or_else(|| totals(false))?;
    if s.cache_read + s.cache_created > 0 {
        s.cache_fractures = count_cache_fractures(&conn).unwrap_or(0);
    }
    if let Ok((actual, without, routed_n, routed_in, routed_saved, routed_cost)) = conn.query_row(
        "SELECT COALESCE(SUM(CASE WHEN status < 400 THEN cost_usd ELSE 0 END),0),                 COALESCE(SUM(CASE WHEN status < 400 THEN cost_without ELSE 0 END),0),                 COALESCE(SUM(CASE WHEN routed_from IS NOT NULL THEN 1 ELSE 0 END),0),                 COALESCE(SUM(CASE WHEN routed_from IS NOT NULL THEN tokens_in ELSE 0 END),0),                 COALESCE(SUM(CASE WHEN routed_from IS NOT NULL AND status < 400 THEN cost_without - cost_usd ELSE 0 END),0),                 COALESCE(SUM(CASE WHEN routed_from IS NOT NULL AND status < 400 THEN cost_usd ELSE 0 END),0)          FROM usage_records",
        [],
        |r| {
            Ok((
                r.get::<_, f64>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, f64>(4)?,
                r.get::<_, f64>(5)?,
            ))
        },
    ) {
        s.cost_actual_usd = actual;
        s.cost_without_usd = without;
        s.routed_requests = routed_n;
        s.routed_tokens_in = routed_in;
        s.routed_saved_usd = routed_saved;
        s.routed_cost_usd = routed_cost;
    }
    if let Ok(n) = conn.query_row(
        "SELECT COUNT(*) FROM usage_records WHERE tokens_saved > 0",
        [],
        |r| r.get::<_, i64>(0),
    ) {
        s.trimmed_requests = n;
    }
    Some(s)
}

/// Proxy-side savings grouped by the pipeline active when each request was
/// handled. Rows written before the `pipeline` column existed (or by a boot
/// that resolved none) group under the empty name, rendered as "(unknown)".
#[derive(Debug, Clone)]
pub struct ProxyPipelineRow {
    pub name: String,
    pub requests: i64,
    pub tokens_in: i64,
    pub tokens_saved: i64,
    pub saved_ccr: i64,
    pub saved_normalize: i64,
    pub saved_ingest_filter: i64,
    pub saved_pixel_compress: i64,
    pub cache_read: i64,
    pub cache_created: i64,
}

#[cfg(feature = "engine")]
/// The single daemon summary as a one-row pipeline list, for a project
/// pipeline whose name the proxy never stamps (the daemon runs the user-scope
/// pipeline). Named for the daemon's own pipeline so the card labels it
/// honestly rather than claiming the project pipeline's name.
pub fn read_gain_summary_as_pipeline() -> Option<ProxyPipelineRow> {
    let s = read_gain_summary()?;
    if s.total_requests == 0 {
        return None;
    }
    Some(ProxyPipelineRow {
        name: "llm proxy (daemon)".to_string(),
        requests: s.total_requests,
        tokens_in: s.tokens_in,
        tokens_saved: s.tokens_saved,
        saved_ccr: s.saved_ccr,
        saved_normalize: s.saved_normalize,
        saved_ingest_filter: s.saved_ingest_filter,
        saved_pixel_compress: s.saved_pixel_compress,
        cache_read: s.cache_read,
        cache_created: s.cache_created,
    })
}

impl ProxyPipelineRow {
    /// Per-method breakdown of this pipeline's `tokens_saved`, display
    /// order, zero methods dropped — same contract as
    /// [`ProxyGainSummary::savings_by_method`].
    pub fn savings_by_method(&self) -> Vec<(&'static str, i64)> {
        [
            ("ccr", self.saved_ccr),
            ("normalize", self.saved_normalize),
            ("ingest-filter", self.saved_ingest_filter),
            ("pixel-compress", self.saved_pixel_compress),
        ]
        .into_iter()
        .filter(|(_, v)| *v > 0)
        .collect()
    }
}

/// Best-effort like [`read_gain_summary`]: `None` when the proxy has never
/// run, the DB is unreadable, or it predates the `pipeline` column.
pub fn savings_by_pipeline() -> Option<Vec<ProxyPipelineRow>> {
    let path = db_path()?;
    if !path.is_file() {
        return None;
    }
    savings_by_pipeline_at(&path)
}

fn savings_by_pipeline_at(db: &std::path::Path) -> Option<Vec<ProxyPipelineRow>> {
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()?;
    conn.busy_timeout(Duration::from_millis(500)).ok()?;
    let mut stmt = conn
        .prepare(
            "SELECT COALESCE(pipeline, ''), COUNT(*), COALESCE(SUM(tokens_in),0), \
             COALESCE(SUM(tokens_saved),0), COALESCE(SUM(saved_ccr),0), \
             COALESCE(SUM(saved_normalize),0), COALESCE(SUM(saved_ingest_filter),0), \
             COALESCE(SUM(saved_pixel_compress),0), COALESCE(SUM(cache_read_tokens),0), \
             COALESCE(SUM(cache_creation_tokens),0) FROM usage_records \
             GROUP BY 1 ORDER BY 4 DESC",
        )
        .ok()?;
    let rows = stmt
        .query_map([], |r| {
            Ok(ProxyPipelineRow {
                name: r.get(0)?,
                requests: r.get(1)?,
                tokens_in: r.get(2)?,
                tokens_saved: r.get(3)?,
                saved_ccr: r.get(4)?,
                saved_normalize: r.get(5)?,
                saved_ingest_filter: r.get(6)?,
                saved_pixel_compress: r.get(7)?,
                cache_read: r.get(8)?,
                cache_created: r.get(9)?,
            })
        })
        .ok()?
        .filter_map(|r| r.ok())
        .collect();
    Some(rows)
}

/// Candidate paths for the daemon binary, in preference order: next to the
/// running `rtk`, then whatever is on `PATH`.
///
/// The sibling lookup covers both real cases at once — a release installs the
/// two binaries into the same directory, and `cargo build` drops them in the
/// same `target/<profile>/`. Deliberately no `CARGO_MANIFEST_DIR` fallback:
/// that would bake the build machine's absolute path into every shipped
/// binary to serve one marginal case (rtk installed, proxy only built).
fn binary_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        out.push(dir.join(PROXY_BIN));
    }
    if let Ok(found) = which::which(PROXY_BIN) {
        out.push(found);
    }
    out
}

/// Locate the daemon binary, or explain how to get one.
fn locate_binary() -> Result<PathBuf> {
    let candidates = binary_candidates();
    if let Some(found) = candidates.iter().find(|p| p.is_file()) {
        return Ok(found.clone());
    }
    anyhow::bail!(
        "{PROXY_BIN} not found.\n\
         The daemon ships as its own release archive (rtk-proxy-<target>), not \
         inside the rtk one -- it is licensed separately. Download it and put it \
         next to `rtk`, or in a source checkout build it with\n  \
         cargo build --release --manifest-path proxy/Cargo.toml\nLooked in:\n{}",
        candidates
            .iter()
            .map(|p| format!("  {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn health_check(bind: &str) -> bool {
    health_check_with_timeout(bind, Duration::from_millis(400))
}

/// Health check an arbitrary loopback address: GET /health and verify the
/// response identifies as rtk-proxy. Public so other hook sites (e.g.
/// SessionStart) confirm it is *our* daemon rather than any server that
/// happens to answer TCP on the port.
#[cfg(feature = "engine")]
pub fn is_rtk_proxy_at(addr: &str, timeout: Duration) -> bool {
    health_check_with_timeout(addr, timeout)
}

fn health_check_with_timeout(bind: &str, timeout: Duration) -> bool {
    ureq::get(&format!("http://{bind}/health"))
        .timeout(timeout)
        .call()
        .ok()
        .and_then(|resp| resp.into_string().ok())
        .is_some_and(|body| is_rtk_proxy_health(&body))
}

/// Whether a `/health` body identifies the rtk daemon. Anything else on the
/// port — a dev server, a different proxy — answers 200 too, and treating it
/// as ours would skip the real daemon and route an agent at a server that
/// cannot serve it.
fn is_rtk_proxy_health(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("service").and_then(|s| s.as_str()).map(String::from))
        .is_some_and(|service| service == "rtk-proxy")
}

/// How to spell this CLI in a hint, taken from the running executable's file
/// name — a binary installed under another name would otherwise be told, by a
/// hardcoded "rtk", to run a command the reader does not have. Display only:
/// never use this for anything persisted or executed.
pub fn cli_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "rtk".to_string())
}

/// Whether the proxy is running right now, on the default bind — for
/// `rtk gain` to decide whether to auto-launch `--watch`. Bounded to a short
/// timeout (localhost only) so a bare `rtk gain` stays snappy even when the
/// proxy isn't running.
/// Where the daemon listens: `RTK_PROXY_BIND` if the user moved it (the same
/// variable the daemon itself reads), else the default port. Without this the
/// CLI probes 8080 while the daemon answers elsewhere, and every
/// is-it-running check silently says no.
pub fn daemon_bind() -> String {
    crate::core::user_env::var("RTK_PROXY_BIND")
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_BIND.to_string())
}

pub fn is_reachable() -> bool {
    health_check_with_timeout(&daemon_bind(), Duration::from_millis(150))
}

/// What to say when the proxy has no usage rows. Started-but-unused is the
/// common case and needs a different instruction from not-started: telling
/// someone to start a daemon they have just started sends them in a circle,
/// and the step they are actually missing is pointing an agent at it.
pub fn no_data_hint() -> String {
    let cli = cli_name();
    if !is_reachable() {
        return format!("no data yet — run `{cli} llm-proxy start`");
    }
    let launchers = PROXY_AGENTS
        .iter()
        .map(|a| format!("`{cli} {}`", a.name))
        .collect::<Vec<_>>()
        .join(" or ");
    format!(
        "running on {} · no agent routed through it — launch one with {launchers}",
        daemon_bind()
    )
}

/// An agent `rtk <agent>` can launch through the proxy.
///
/// Membership is the contract: a subcommand exists only where redirection is
/// verified, so `rtk <agent>` resolving is itself the signal that the traffic
/// really goes through the daemon. Agents whose redirection is unimplemented,
/// or impossible because they offer no base-URL override, are absent and fall
/// through to plain passthrough. See `proxy/CONTRIBUTING.md`.
pub struct ProxyAgent {
    /// The `rtk <name>` subcommand, and the binary it execs.
    pub name: &'static str,
    /// The base-URL variable this agent honours.
    pub base_url_var: &'static str,
}

pub const PROXY_AGENTS: &[ProxyAgent] = &[ProxyAgent {
    name: "claude",
    base_url_var: "ANTHROPIC_BASE_URL",
}];

pub fn resolve_agent(name: &str) -> Option<&'static ProxyAgent> {
    PROXY_AGENTS.iter().find(|a| a.name == name)
}

/// How `run_agent` treats the agent's base-URL variable. Pure — decided from
/// the existing environment value and whether the daemon came up, so the
/// fail-open rules stay testable without spawning anything.
#[derive(Debug, PartialEq)]
enum LaunchPlan {
    /// Daemon healthy, no competing route: point the agent at it.
    Proxied,
    /// The variable already routes somewhere else (Bedrock, LiteLLM, a
    /// gateway): leave it alone and say so — never clobber a working setup.
    KeepExisting,
    /// Daemon unavailable and the variable is unset: launch untouched.
    Unproxied,
    /// Daemon unavailable but the variable already points at it: clear it,
    /// or the agent would talk to a dead port.
    UnproxiedClear,
}

fn launch_plan(existing: Option<&str>, proxy_url: &str, daemon_up: bool) -> LaunchPlan {
    match existing {
        Some(url) if !url.is_empty() && url != proxy_url => LaunchPlan::KeepExisting,
        Some(url) if url == proxy_url && !daemon_up => LaunchPlan::UnproxiedClear,
        _ if daemon_up => LaunchPlan::Proxied,
        _ => LaunchPlan::Unproxied,
    }
}

/// Start the daemon if possible, point this agent at it, and hand over the
/// process. On Unix this `exec`s, so signals, stdio and the exit code behave
/// exactly as if the agent had been launched directly. Never blocks the
/// agent: when the daemon cannot run, the agent launches unproxied with a
/// loud warning instead of failing.
pub fn run_agent(agent: &ProxyAgent, args: &[String]) -> Result<i32> {
    if !tool_exists(agent.name) {
        eprintln!("rtk: `{}` is not on PATH", agent.name);
        return Ok(127);
    }
    let bind = daemon_bind();
    let base_url = format!("http://{bind}");
    let existing = std::env::var(agent.base_url_var).ok();
    let daemon_up = match ensure_running(&bind) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("rtk: proxy unavailable ({e:#})");
            false
        }
    };
    let mut cmd = Command::new(agent.name);
    cmd.args(args);
    match launch_plan(existing.as_deref(), &base_url, daemon_up) {
        LaunchPlan::Proxied => {
            cmd.env(agent.base_url_var, &base_url);
        }
        LaunchPlan::KeepExisting => {
            eprintln!(
                "rtk: {} is already set — leaving it; unset it to route through the rtk proxy",
                agent.base_url_var
            );
        }
        LaunchPlan::Unproxied => {
            eprintln!(
                "rtk: launching `{}` unproxied — API traffic will not be compressed. \
                 Install the daemon (rtk llm-proxy setup) to enable it.",
                agent.name
            );
        }
        LaunchPlan::UnproxiedClear => {
            eprintln!(
                "rtk: {} pointed at the rtk proxy, which is not running — launching `{}` \
                 unproxied instead.",
                agent.base_url_var, agent.name
            );
            cmd.env_remove(agent.base_url_var);
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        Err(anyhow::Error::new(err).context(format!("exec {}", agent.name)))
    }
    #[cfg(not(unix))]
    {
        let status = cmd
            .status()
            .with_context(|| format!("running {}", agent.name))?;
        Ok(status.code().unwrap_or(1))
    }
}

fn detect_claude_code() -> bool {
    tool_exists("claude")
}

fn run_setup() -> Result<()> {
    let bin = locate_binary()?;
    println!("rtk-proxy binary ready: {}", bin.display());
    if detect_claude_code() {
        println!(
            "detected: claude (Claude Code) -- run `rtk init` to route it through the proxy, or `eval \"$(rtk llm-proxy start --env)\"` for one shell"
        );
    } else {
        println!(
            "no known AI CLI detected on PATH -- `rtk llm-proxy start --env` will still print the export line to set manually"
        );
    }
    let names: Vec<&str> = PROXY_AGENTS.iter().map(|a| a.name).collect();
    println!(
        "launch through the proxy: {}",
        names
            .iter()
            .map(|n| format!("rtk {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("next: rtk llm-proxy start");
    Ok(())
}

fn run_start(bind: Option<&str>, env_only: bool) -> Result<()> {
    let bind = bind.map(str::to_string).unwrap_or_else(daemon_bind);
    let started = ensure_running(&bind)?;

    if env_only {
        emit_env(&bind);
    } else if let Some(paths) = started {
        println!("rtk-proxy up on {bind}");
        println!("  logs: {}", paths.log.display());
        println!("  db:   {}", paths.db.display());
        if detect_claude_code() {
            println!();
            println!("point Claude Code at it:");
            if let Some(export) = bind_export_hint(&bind) {
                println!("  {export}");
            }
            println!("  eval \"$(rtk llm-proxy start --env)\"");
            println!("  then: claude [claude args...]");
        }
    } else {
        println!("already running on {bind}");
    }
    Ok(())
}

fn bind_export_hint(bind: &str) -> Option<String> {
    (bind != daemon_bind()).then(|| format!("export RTK_PROXY_BIND={bind}"))
}

/// Where a freshly-spawned daemon writes.
pub(crate) struct DaemonPaths {
    log: PathBuf,
    db: PathBuf,
}

/// Idempotent: no-op if already reachable. Otherwise spawns the proxy as a
/// detached daemon and blocks until it's healthy. Returns `Some(paths)` if it
/// actually started the daemon this call, `None` if it was already running.
pub(crate) fn ensure_running(bind: &str) -> Result<Option<DaemonPaths>> {
    if health_check(bind) {
        return Ok(None);
    }

    let bin = locate_binary()?;
    let dir = state_dir()?;
    let log_path = dir.join("rtk-proxy.log");
    let db_path = dir.join("rtk-proxy.db");
    // CCR's cache. Passed explicitly for the same reason as the usage DB: the
    // daemon's default is cwd-relative, and it inherits the cwd of whatever
    // shell ran `rtk llm-proxy start`.
    let icm_db_path = dir.join("rtk-proxy-memory.db");
    let log_file = std::fs::File::create(&log_path)
        .with_context(|| format!("creating {}", log_path.display()))?;
    let log_file_err = log_file.try_clone().context("cloning log file handle")?;

    Command::new(&bin)
        .arg("start")
        .arg("--bind")
        .arg(bind)
        .env("RTK_PROXY_DB", &db_path)
        .env("RTK_PROXY_ICM_DB", &icm_db_path)
        .stdin(Stdio::null())
        .stdout(log_file)
        .stderr(log_file_err)
        .spawn()
        .with_context(|| format!("failed to spawn {}", bin.display()))?;

    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if health_check(bind) {
            return Ok(Some(DaemonPaths {
                log: log_path,
                db: db_path,
            }));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    anyhow::bail!(
        "rtk-proxy did not become reachable on {bind} within 15s -- see {}",
        log_path.display()
    );
}

fn env_line(bind: &str) -> String {
    format!("export ANTHROPIC_BASE_URL=http://{bind}")
}

fn emit_env(bind: &str) {
    println!("{}", env_line(bind));
}

/// Build the argument vector for a forwarded subcommand: `<subcmd> [--bind
/// <bind>] [extra...]`. Pure so the flag placement can be unit-tested without
/// spawning a process.
fn build_forward_args(subcmd: &str, bind: Option<&str>, extra: &[String]) -> Vec<String> {
    let mut args: Vec<String> = vec![subcmd.to_string()];
    if let Some(b) = bind {
        args.push("--bind".to_string());
        args.push(b.to_string());
    }
    args.extend(extra.iter().cloned());
    args
}

fn forward(subcmd: &str, bind: Option<&str>, extra: &[String]) -> Result<()> {
    exec(&build_forward_args(subcmd, bind, extra))
}

fn forward_raw(subcmd: &str, extra: &[String]) -> Result<()> {
    exec(&build_forward_args(subcmd, None, extra))
}

fn exec(args: &[String]) -> Result<()> {
    let bin = locate_binary()?;
    let status = Command::new(&bin)
        .args(args)
        .status()
        .with_context(|| format!("failed to run {} {}", bin.display(), args.join(" ")))?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Never empty and never a path — it is interpolated straight into a
    /// "run this" hint.
    #[test]
    fn health_identity_rejects_a_stranger_on_the_port() {
        assert!(is_rtk_proxy_health(
            r#"{"status":"ok","service":"rtk-proxy","edition":"local","version":"1.0.0"}"#
        ));
        assert!(!is_rtk_proxy_health(r#"{"status":"ok"}"#));
        assert!(!is_rtk_proxy_health("<html>It works!</html>"));
        assert!(!is_rtk_proxy_health(""));
    }

    #[test]
    fn launch_plan_never_blocks_and_never_clobbers() {
        let ours = "http://127.0.0.1:8080";
        assert_eq!(launch_plan(None, ours, true), LaunchPlan::Proxied);
        assert_eq!(launch_plan(None, ours, false), LaunchPlan::Unproxied);
        assert_eq!(
            launch_plan(Some("https://bedrock.corp"), ours, true),
            LaunchPlan::KeepExisting,
            "an existing route is the user's routing decision"
        );
        assert_eq!(
            launch_plan(Some("https://bedrock.corp"), ours, false),
            LaunchPlan::KeepExisting
        );
        assert_eq!(launch_plan(Some(ours), ours, true), LaunchPlan::Proxied);
        assert_eq!(
            launch_plan(Some(ours), ours, false),
            LaunchPlan::UnproxiedClear,
            "never launch the agent pointed at a dead proxy"
        );
        assert_eq!(launch_plan(Some(""), ours, false), LaunchPlan::Unproxied);
    }

    #[test]
    fn cli_name_is_a_bare_command_name() {
        let n = cli_name();
        assert!(!n.is_empty());
        assert!(!n.contains(std::path::MAIN_SEPARATOR));
    }

    #[test]
    fn only_verified_agents_get_a_subcommand() {
        assert!(resolve_agent("claude").is_some());
        // No base-URL override exists for these, so `rtk cursor` must stay
        // plain passthrough rather than pretending to route.
        for absent in ["cursor", "windsurf", "cline", "codex", "opencode"] {
            assert!(
                resolve_agent(absent).is_none(),
                "{absent} has no verified redirection; it must not resolve"
            );
        }
        for agent in PROXY_AGENTS {
            assert!(!agent.name.is_empty());
            assert!(
                agent.base_url_var.ends_with("_BASE_URL"),
                "{} must name a base-URL variable",
                agent.name
            );
        }
    }

    #[test]
    fn env_line_points_at_the_bound_address() {
        assert_eq!(
            env_line("127.0.0.1:8080"),
            "export ANTHROPIC_BASE_URL=http://127.0.0.1:8080"
        );
    }

    #[test]
    fn forward_args_omit_bind_when_none() {
        assert_eq!(
            build_forward_args("status", None, &[]),
            vec!["status".to_string()]
        );
    }

    #[test]
    fn forward_args_place_bind_right_after_the_subcommand() {
        assert_eq!(
            build_forward_args("stats", Some("127.0.0.1:9090"), &[]),
            vec!["stats", "--bind", "127.0.0.1:9090"]
        );
    }

    #[test]
    fn forward_args_append_extra_args_after_bind() {
        let extra = vec!["get".to_string(), "ccr".to_string()];
        assert_eq!(
            build_forward_args("config", Some("127.0.0.1:8080"), &extra),
            vec!["config", "--bind", "127.0.0.1:8080", "get", "ccr"]
        );
    }

    /// The sibling of the running binary must be tried before `PATH`: a
    /// release installs `rtk` and `rtk-proxy` together, and an unrelated
    /// `rtk-proxy` earlier on `PATH` must not win over the one that shipped
    /// with this `rtk`.
    #[test]
    fn binary_search_prefers_the_sibling_of_the_running_exe() {
        let candidates = binary_candidates();
        let expected = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|d| d.join(PROXY_BIN)));
        assert_eq!(candidates.first().cloned(), expected);
    }

    /// The error message is the only guidance a user gets when the daemon
    /// isn't installed, so it must list real, absolute places it looked --
    /// never an empty list or a relative path.
    #[test]
    fn missing_binary_error_names_every_place_it_looked() {
        let candidates = binary_candidates();
        assert!(!candidates.is_empty());
        assert!(candidates.iter().all(|p| p.is_absolute()));
        assert!(
            candidates
                .iter()
                .all(|p| p.file_name().is_some_and(|n| n == PROXY_BIN))
        );
    }

    #[test]
    fn read_gain_summary_aggregates_usage_records() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("rtk-proxy.db");
        let conn = rusqlite::Connection::open(&db_path).expect("open db");
        conn.execute_batch(
            "CREATE TABLE usage_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts TEXT NOT NULL,
                tokens_in INTEGER NOT NULL DEFAULT 0,
                tokens_out INTEGER NOT NULL DEFAULT 0,
                tokens_saved INTEGER NOT NULL DEFAULT 0,
                cost_usd REAL NOT NULL DEFAULT 0,
                ctx_total INTEGER NOT NULL DEFAULT 0,
                ctx_tool_result INTEGER NOT NULL DEFAULT 0,
                ctx_tool_use INTEGER NOT NULL DEFAULT 0,
                ctx_duplicate INTEGER NOT NULL DEFAULT 0,
                ctx_image INTEGER NOT NULL DEFAULT 0,
                saved_ingest_filter INTEGER NOT NULL DEFAULT 0,
                saved_pixel_compress INTEGER NOT NULL DEFAULT 0,
                saved_ccr INTEGER NOT NULL DEFAULT 0,
                status INTEGER NOT NULL DEFAULT 0
            );
            INSERT INTO usage_records
                (ts, tokens_in, tokens_out, tokens_saved, cost_usd,
                 ctx_total, ctx_tool_result, ctx_tool_use, ctx_duplicate, ctx_image,
                 saved_ingest_filter, saved_pixel_compress, saved_ccr, status)
            VALUES ('2026-01-01', 100, 50, 30, 0.01, 1000, 400, 100, 50, 0, 0, 0, 30, 200),
                   ('2026-01-02', 200, 80, 60, 0.02, 2000, 900, 300, 150, 1300, 0, 0, 60, 200),
                   ('2026-01-03', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 429);",
        )
        .expect("seed db");
        drop(conn);

        let s = read_gain_summary_at(&db_path).expect("summary");
        assert_eq!(
            (s.total_requests, s.tokens_in, s.tokens_out, s.tokens_saved),
            (3, 300, 130, 90)
        );
        assert_eq!(
            s.failed_requests, 1,
            "the 429 row is counted apart, not hidden in the totals"
        );
        assert_eq!(
            (
                s.ctx_total,
                s.ctx_tool_result,
                s.ctx_tool_use,
                s.ctx_duplicate,
                s.ctx_image
            ),
            (3000, 1300, 400, 200, 1300)
        );
        assert_eq!(s.saved_ccr, 90);
        // A CCR-only daemon reports one method, not three with two zeroes.
        assert_eq!(s.savings_by_method(), vec![("ccr", 90)]);
    }

    #[test]
    fn read_gain_summary_is_none_for_a_missing_db() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(read_gain_summary_at(&dir.path().join("nope.db")).is_none());
    }

    #[test]
    fn savings_by_pipeline_groups_and_labels_unstamped_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("rtk-proxy.db");
        let conn = rusqlite::Connection::open(&db_path).expect("open db");
        conn.execute_batch(
            "CREATE TABLE usage_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts TEXT NOT NULL,
                tokens_in INTEGER NOT NULL DEFAULT 0,
                tokens_saved INTEGER NOT NULL DEFAULT 0,
                saved_ccr INTEGER NOT NULL DEFAULT 0,
                saved_normalize INTEGER NOT NULL DEFAULT 0,
                saved_ingest_filter INTEGER NOT NULL DEFAULT 0,
                saved_pixel_compress INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                pipeline TEXT
            );
            INSERT INTO usage_records
                (ts, tokens_in, tokens_saved, saved_ccr, saved_normalize,
                 cache_read_tokens, cache_creation_tokens, pipeline)
            VALUES ('2026-01-01', 100, 900, 600, 300, 4000, 1000, 'my-pipe'),
                   ('2026-01-02', 200, 300, 200, 100, 2000, 0, 'my-pipe'),
                   ('2026-01-03', 500, 10, 0, 10, 0, 0, NULL);",
        )
        .expect("seed db");
        drop(conn);

        let rows = savings_by_pipeline_at(&db_path).expect("rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (
                rows[0].name.as_str(),
                rows[0].requests,
                rows[0].tokens_saved
            ),
            ("my-pipe", 2, 1200)
        );
        assert_eq!((rows[1].name.as_str(), rows[1].tokens_saved), ("", 10));
    }

    #[test]
    fn savings_by_pipeline_is_none_when_the_column_predates_the_feature() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("rtk-proxy.db");
        let conn = rusqlite::Connection::open(&db_path).expect("open db");
        conn.execute_batch(
            "CREATE TABLE usage_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts TEXT NOT NULL,
                tokens_in INTEGER NOT NULL DEFAULT 0,
                tokens_saved INTEGER NOT NULL DEFAULT 0
            );",
        )
        .expect("seed db");
        drop(conn);
        assert!(savings_by_pipeline_at(&db_path).is_none());
    }

    #[test]
    fn savings_by_method_lists_every_non_zero_method_ccr_first() {
        let s = ProxyGainSummary {
            total_requests: 1,
            tokens_in: 0,
            tokens_out: 0,
            tokens_saved: 0,
            ctx_total: 0,
            ctx_tool_result: 0,
            ctx_tool_use: 0,
            ctx_duplicate: 0,
            ctx_image: 0,
            saved_ingest_filter: 5,
            saved_pixel_compress: 0,
            saved_ccr: 7,
            saved_normalize: 3,
            cache_read: 0,
            cache_created: 0,
            cache_fractures: 0,
            retrieve_calls: 0,
            failed_requests: 0,
            ..Default::default()
        };
        assert_eq!(
            s.savings_by_method(),
            vec![("ccr", 7), ("normalize", 3), ("ingest-filter", 5)]
        );
    }

    #[test]
    fn cache_health_reads_from_a_current_schema() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("p.db");
        let conn = rusqlite::Connection::open(&db_path).expect("open");
        conn.execute_batch(
            "CREATE TABLE usage_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts TEXT NOT NULL,
                tokens_in INTEGER NOT NULL DEFAULT 0,
                tokens_out INTEGER NOT NULL DEFAULT 0,
                tokens_saved INTEGER NOT NULL DEFAULT 0,
                cost_usd REAL NOT NULL DEFAULT 0,
                status INTEGER NOT NULL DEFAULT 0,
                session TEXT,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                ccr_retrieve_calls INTEGER NOT NULL DEFAULT 0,
                ctx_total INTEGER NOT NULL DEFAULT 0,
                ctx_tool_result INTEGER NOT NULL DEFAULT 0,
                ctx_tool_use INTEGER NOT NULL DEFAULT 0,
                ctx_duplicate INTEGER NOT NULL DEFAULT 0,
                ctx_image INTEGER NOT NULL DEFAULT 0,
                saved_ingest_filter INTEGER NOT NULL DEFAULT 0,
                saved_pixel_compress INTEGER NOT NULL DEFAULT 0,
                saved_ccr INTEGER NOT NULL DEFAULT 0,
                saved_normalize INTEGER NOT NULL DEFAULT 0
            );
            INSERT INTO usage_records
                (ts, status, session, cache_read_tokens, cache_creation_tokens, ccr_retrieve_calls, saved_normalize)
            VALUES ('2026-01-01T00:00:00Z', 200, 's1', 0, 20000, 2, 40),
                   ('2026-01-01T00:00:30Z', 200, 's1', 18232, 627, 0, 0),
                   ('2026-01-01T00:01:00Z', 200, 's1', 0, 20783, 3, 71);",
        )
        .expect("seed db");
        drop(conn);

        let s = read_gain_summary_at(&db_path).expect("summary");
        assert_eq!(s.cache_read, 18232);
        assert_eq!(s.cache_created, 41410);
        assert_eq!(s.cache_fractures, 1);
        assert_eq!(s.retrieve_calls, 5);
        assert_eq!(s.saved_normalize, 111);
        assert!(s.cache_hit_ratio().is_some());
    }

    #[test]
    fn cache_hit_ratio_is_none_before_any_cache_data() {
        let s = ProxyGainSummary {
            cache_read: 0,
            cache_created: 0,
            ..Default::default()
        };
        assert!(s.cache_hit_ratio().is_none());
    }

    #[test]
    fn cache_hit_ratio_reports_the_cached_share() {
        let s = ProxyGainSummary {
            cache_read: 97,
            cache_created: 3,
            ..Default::default()
        };
        assert!((s.cache_hit_ratio().unwrap() - 0.97).abs() < 1e-9);
    }
}

#[cfg(test)]
mod no_data_hint_tests {
    use super::*;

    #[test]
    fn rtk_proxy_bind_moves_the_default_bind() {
        crate::core::user_env::with_vars(&[("RTK_PROXY_BIND", Some("127.0.0.1:8580"))], || {
            assert_eq!(daemon_bind(), "127.0.0.1:8580");
        });
        crate::core::user_env::with_vars(&[("RTK_PROXY_BIND", None)], || {
            assert_eq!(daemon_bind(), DEFAULT_BIND);
        });
    }

    #[test]
    fn a_one_off_bind_is_told_to_export_its_port() {
        crate::core::user_env::with_vars(&[("RTK_PROXY_BIND", Some("127.0.0.1:8580"))], || {
            assert_eq!(bind_export_hint("127.0.0.1:8580"), None);
            assert_eq!(
                bind_export_hint("127.0.0.1:9000").as_deref(),
                Some("export RTK_PROXY_BIND=127.0.0.1:9000")
            );
        });
        crate::core::user_env::with_vars(&[("RTK_PROXY_BIND", None)], || {
            assert_eq!(bind_export_hint(DEFAULT_BIND), None);
        });
    }

    /// The daemon's reachability decides the instruction. The unreachable arm is
    /// the only one assertable without a listener; the reachable arm is covered
    /// by the shape assertions below, which hold for whichever arm runs.
    #[test]
    fn the_hint_never_tells_you_to_start_a_running_daemon() {
        let hint = no_data_hint();
        if is_reachable() {
            assert!(
                !hint.contains("llm-proxy start"),
                "a running daemon must not be told to start: {hint}"
            );
            assert!(hint.contains(&daemon_bind()), "{hint}");
            assert!(hint.contains("claude"), "{hint}");
        } else {
            assert!(hint.contains("llm-proxy start"), "{hint}");
        }
    }

    #[test]
    fn the_hint_names_every_launchable_agent_when_reachable() {
        if !is_reachable() {
            return;
        }
        let hint = no_data_hint();
        for agent in PROXY_AGENTS {
            assert!(
                hint.contains(agent.name),
                "{} missing from: {hint}",
                agent.name
            );
        }
    }
}

/// A counterfactual dollar amount, displayed honestly: `~` prefix, coarse
/// precision, never four decimals. Measured spend prints exactly; anything
/// that exists only by comparison with a request that never ran goes through
/// here. One rule, used by every gain surface, so the display's confidence
/// cannot drift from the number's epistemics.
pub fn approx_usd(v: f64) -> String {
    let v = v.max(0.0);
    if v < 0.0005 {
        "<$0.001".to_string()
    } else if v < 0.10 {
        format!("~${v:.3}")
    } else {
        format!("~${v:.2}")
    }
}
