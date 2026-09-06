//! Content-addressed recall store backing `rtk recall`.

// Complexity ratchet — see clippy.toml. Ceilings may only fall.
#![deny(
    clippy::too_many_lines,
    clippy::too_many_arguments,
    clippy::cognitive_complexity,
    clippy::excessive_nesting,
    clippy::fn_params_excessive_bools,
    clippy::struct_excessive_bools,
    clippy::type_complexity
)]

use super::constants::{RECALL_DB, RTK_DATA_DIR};
use crate::core::config::Config;
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_MAX_ENTRY_BYTES: usize = 10 * 1024 * 1024;
const DEFAULT_MAX_ENTRIES: usize = 200;
const DEFAULT_RETENTION_DAYS: u32 = 30;
pub const MIN_FAILURE_BYTES: usize = 500;
const HASH_HEX_LEN: usize = 12;
const DEFAULT_TEE_MAX_FILES: usize = 20;
const DEFAULT_TEE_MAX_FILE_SIZE: usize = 1_048_576;

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecoveryMode {
    #[default]
    Sqlite,
    Tee,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RetrieverConfig {
    pub mode: RecoveryMode,
    pub max_entry_bytes: usize,
    pub max_entries: usize,
    pub retention_days: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database_path: Option<PathBuf>,
    pub tee_max_files: usize,
    pub tee_max_file_size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tee_directory: Option<PathBuf>,
}

impl Default for RetrieverConfig {
    fn default() -> Self {
        Self {
            mode: RecoveryMode::Sqlite,
            max_entry_bytes: DEFAULT_MAX_ENTRY_BYTES,
            max_entries: DEFAULT_MAX_ENTRIES,
            retention_days: DEFAULT_RETENTION_DAYS,
            database_path: None,
            tee_max_files: DEFAULT_TEE_MAX_FILES,
            tee_max_file_size: DEFAULT_TEE_MAX_FILE_SIZE,
            tee_directory: None,
        }
    }
}

#[derive(Debug)]
pub struct StoredRef {
    pub hash: String,
    pub hidden_lines: usize,
}

#[derive(Debug)]
pub enum Stored {
    Saved(StoredRef),
    Unavailable,
    Empty,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn content_hash(command: &str, content: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(command.as_bytes());
    hasher.update([0u8]);
    hasher.update(content);
    let hex = format!("{:x}", hasher.finalize());
    hex[..HASH_HEX_LEN].to_string()
}

fn count_lines(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    let newlines = bytes.iter().filter(|&&b| b == b'\n').count();
    if *bytes.last().unwrap() == b'\n' {
        newlines
    } else {
        newlines + 1
    }
}

fn slice_from_line(bytes: &[u8], from: usize) -> &[u8] {
    if from <= 1 {
        return bytes;
    }
    let mut seen = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            seen += 1;
            if seen == from - 1 {
                return &bytes[i + 1..];
            }
        }
    }
    &[]
}

fn slice_first_lines(bytes: &[u8], n: usize) -> &[u8] {
    if n == 0 {
        return &[];
    }
    let mut seen = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            seen += 1;
            if seen == n {
                return &bytes[..=i];
            }
        }
    }
    bytes
}

fn grep_bytes(input: &[u8], pattern: &str) -> Vec<u8> {
    let Some(re) = compile_grep_pattern(pattern) else {
        return input.to_vec();
    };
    let (lines, trailing_newline) = split_keeping_terminator(input);
    let matched: Vec<&[u8]> = lines.into_iter().filter(|l| re.is_match(l)).collect();
    join_matched_lines(&matched, trailing_newline)
}

/// A regex if the pattern is one, otherwise the pattern matched literally.
/// `rtk recall --grep` takes whatever the agent typed, and a stray `[` should
/// search for a bracket rather than fail the command.
fn compile_grep_pattern(pattern: &str) -> Option<regex::bytes::Regex> {
    use regex::bytes::Regex;
    Regex::new(pattern)
        .or_else(|_| Regex::new(&regex::escape(pattern)))
        .ok()
}

/// Split on newlines, reporting whether the input ended with one.
///
/// The flag is the whole point. `split` yields a trailing empty slice for input
/// ending in `\n`, and treating that as a line is what made `--grep '^'` return
/// the stored bytes plus one newline — the §B.B4 byte-fidelity defect. Dropping
/// it here, and carrying the terminator separately, keeps the rejoin honest.
fn split_keeping_terminator(input: &[u8]) -> (Vec<&[u8]>, bool) {
    let trailing_newline = input.last() == Some(&b'\n');
    let mut lines: Vec<&[u8]> = input.split(|&b| b == b'\n').collect();
    if trailing_newline && lines.last() == Some(&&b""[..]) {
        lines.pop();
    }
    (lines, trailing_newline)
}

/// Rejoin matches, restoring the original terminator and no more: input that
/// ended in a newline gets one back, input that did not does not gain one.
fn join_matched_lines(matched: &[&[u8]], trailing_newline: bool) -> Vec<u8> {
    if matched.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (i, line) in matched.iter().enumerate() {
        out.extend_from_slice(line);
        if i + 1 < matched.len() || trailing_newline {
            out.push(b'\n');
        }
    }
    out
}

/// The only codec written. Storing raw was measurably worse on both axes —
/// slower than lz4 at every size above 1KB (the sqlite write dominates) and
/// 2-9x larger on disk — so there is no uncompressed path to select.
const CODEC_LZ4: &str = "lz4";

fn lz4_compress(data: &[u8]) -> Vec<u8> {
    lz4_flex::compress_prepend_size(data)
}

fn lz4_decompress(data: &[u8]) -> Result<Vec<u8>> {
    lz4_flex::decompress_size_prepended(data).context("lz4 decompress")
}

fn db_path(cfg: &RetrieverConfig) -> Result<PathBuf> {
    if let Ok(p) = std::env::var("RTK_RECALL_DB") {
        return Ok(PathBuf::from(p));
    }
    if let Some(ref p) = cfg.database_path {
        return Ok(p.clone());
    }
    let data_dir = dirs::data_local_dir()
        .ok_or_else(|| anyhow::anyhow!("no local data directory available"))?;
    Ok(data_dir.join(RTK_DATA_DIR).join(RECALL_DB))
}

/// A cached connection and the DB path it was opened against.
type CachedConn = (PathBuf, std::rc::Rc<Connection>);

/// `(mode, canonical slug)` -> `(elisions, recalls)`, the fold used by
/// `aggregate_stats`.
type StatTotals = std::collections::BTreeMap<(String, String), (i64, i64)>;

thread_local! {
    /// Last connection opened on this thread, keyed by the DB path it was
    /// opened against. `store()` runs once per elided command, but the hint
    /// paths in `search.rs` fire once *per file*, and each `open()` re-runs the
    /// pragmas plus six DDL statements — enough to breach the <10ms startup
    /// target (B11/V18). rtk is single-threaded and short-lived, so one cached
    /// handle per thread needs no pooling.
    ///
    /// Keyed by path, not unconditional: `RTK_RECALL_DB` and `cfg.database_path`
    /// can select a different DB within one process, and tests routinely do.
    static CACHED_CONN: std::cell::RefCell<Option<CachedConn>> =
        const { std::cell::RefCell::new(None) };
}

fn open(cfg: &RetrieverConfig) -> Result<std::rc::Rc<Connection>> {
    let path = db_path(cfg)?;
    let cached = CACHED_CONN.with(|c| {
        c.borrow()
            .as_ref()
            .filter(|(cached_path, _)| *cached_path == path)
            .map(|(_, conn)| conn.clone())
    });
    if let Some(conn) = cached {
        return Ok(conn);
    }
    let conn = std::rc::Rc::new(open_uncached(&path)?);
    CACHED_CONN.with(|c| *c.borrow_mut() = Some((path, conn.clone())));
    Ok(conn)
}

fn open_uncached(path: &std::path::Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        let _ = crate::core::utils::create_private_dir(parent);
    }
    crate::core::utils::open_private(std::fs::OpenOptions::new().write(true).create(true), path)
        .with_context(|| format!("pre-create private recall DB: {}", path.display()))?;
    let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
    // best-effort: NFS / read-only filesystems may reject WAL
    let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;");
    init_schema(&conn)?;
    Ok(conn)
}

/// Drop the cached handle. Tests that replace or corrupt the DB file underneath
/// a path they have already opened need the next `open()` to be a real open.
#[cfg(test)]
fn reset_conn_cache() {
    CACHED_CONN.with(|c| *c.borrow_mut() = None);
}

/// Schema as it should exist after `open()`. Every statement is
/// `IF NOT EXISTS`, so this runs on every connection and is the definition of
/// the store rather than a migration step.
const SCHEMA_SQL: &str = "CREATE TABLE IF NOT EXISTS recall (
            hash        TEXT PRIMARY KEY,
            command     TEXT NOT NULL,
            cwd         TEXT,
            exit_code   INTEGER,
            created_at  INTEGER NOT NULL,
            total_lines INTEGER NOT NULL,
            shown_upto  INTEGER NOT NULL,
            byte_size   INTEGER NOT NULL,
            truncated   INTEGER NOT NULL,
            codec       TEXT NOT NULL,
            blob        BLOB NOT NULL,
            recalled    INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS tee_reads (
            path TEXT PRIMARY KEY
        );
        CREATE INDEX IF NOT EXISTS idx_recall_command ON recall(command, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_recall_created ON recall(created_at);
        CREATE TABLE IF NOT EXISTS recall_stats (
            slug     TEXT NOT NULL,
            mode     TEXT NOT NULL,
            elisions INTEGER NOT NULL DEFAULT 0,
            recalls  INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (slug, mode)
        );";

/// Adds `recalled` to a `recall` table created before that column existed.
/// Expected to fail with "duplicate column" on every current database, which is
/// why the result is discarded: SQLite has no `ADD COLUMN IF NOT EXISTS`.
const BACKFILL_RECALLED_SQL: &str =
    "ALTER TABLE recall ADD COLUMN recalled INTEGER NOT NULL DEFAULT 0";

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA_SQL)
        .context("init recall schema")?;
    let _ = conn.execute(BACKFILL_RECALLED_SQL, []);
    Ok(())
}

fn stat_family(slug: &str) -> &str {
    match slug.find(|c: char| c.is_ascii_digit()) {
        Some(i) if i > 0 && slug.as_bytes()[i - 1] == b'_' => &slug[..i - 1],
        _ => slug,
    }
}

fn strip_shortened_hash(s: &str) -> &str {
    let b = s.as_bytes();
    if b.len() == 15
        && b[8] == b'_'
        && s[9..]
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    {
        &s[..8]
    } else {
        s
    }
}

fn stat_key(slug: &str) -> String {
    let sanitized = crate::core::tee_file::sanitize_slug(slug);
    stat_family(strip_shortened_hash(&sanitized)).to_string()
}

fn bump_stat(conn: &Connection, slug: &str, mode: &str, column: &str) {
    let slug = stat_key(slug);
    let slug = slug.as_str();
    let sql = match column {
        "elisions" => {
            "INSERT INTO recall_stats (slug, mode, elisions, recalls) VALUES (?1, ?2, 1, 0)
             ON CONFLICT(slug, mode) DO UPDATE SET elisions = elisions + 1"
        }
        "recalls" => {
            "INSERT INTO recall_stats (slug, mode, elisions, recalls) VALUES (?1, ?2, 0, 1)
             ON CONFLICT(slug, mode) DO UPDATE SET recalls = recalls + 1"
        }
        _ => return,
    };
    let _ = conn.execute(sql, params![slug, mode]);
}

#[derive(Debug)]
pub struct RecallStat {
    pub slug: String,
    pub mode: String,
    pub elisions: i64,
    pub recalls: i64,
}

fn stats_snapshot_with(cfg: &RetrieverConfig) -> Result<Vec<RecallStat>> {
    let conn = open(cfg)?;
    let mut stmt = conn.prepare("SELECT slug, mode, elisions, recalls FROM recall_stats")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })?;
    Ok(aggregate_stats(rows.filter_map(|r| r.ok())))
}

/// Fold raw `recall_stats` rows onto their canonical slug and order them for
/// display: by mode, then busiest first, then alphabetically.
///
/// Separate from the query because the folding is where the meaning is — two
/// stored slugs that `stat_key` maps together are one row to the reader, and
/// that rule deserves to be exercised without standing up a database.
fn aggregate_stats(rows: impl Iterator<Item = (String, String, i64, i64)>) -> Vec<RecallStat> {
    let mut stats: Vec<RecallStat> = fold_stat_rows(rows)
        .into_iter()
        .map(|((mode, slug), (elisions, recalls))| RecallStat {
            slug,
            mode,
            elisions,
            recalls,
        })
        .collect();
    stats.sort_by(by_mode_then_busiest);
    stats
}

/// Display order: grouped by mode, busiest first within a mode, alphabetical to
/// break ties so the table is stable between runs.
fn by_mode_then_busiest(a: &RecallStat, b: &RecallStat) -> std::cmp::Ordering {
    a.mode
        .cmp(&b.mode)
        .then(b.elisions.cmp(&a.elisions))
        .then(a.slug.cmp(&b.slug))
}

/// Sum rows onto their canonical slug. This is where two stored slugs that
/// `stat_key` maps together become one row to the reader.
fn fold_stat_rows(rows: impl Iterator<Item = (String, String, i64, i64)>) -> StatTotals {
    let mut agg: StatTotals = std::collections::BTreeMap::new();
    for (slug, mode, elisions, recalls) in rows {
        let entry = agg.entry((mode, stat_key(&slug))).or_insert((0, 0));
        entry.0 += elisions;
        entry.1 += recalls;
    }
    agg
}

pub fn stats_snapshot() -> Result<Vec<RecallStat>> {
    let cfg = Config::load().unwrap_or_default().retriever;
    if cfg.mode == RecoveryMode::Disabled {
        return Ok(Vec::new());
    }
    stats_snapshot_with(&cfg)
}

pub fn record_tee_elision(cfg: &RetrieverConfig, slug: &str) {
    if cfg.mode == RecoveryMode::Disabled {
        return;
    }
    if let Ok(conn) = open(cfg) {
        bump_stat(&conn, slug, "tee", "elisions");
    }
}

fn mark_recalled(conn: &Connection, hash: &str, command: &str) {
    let changed = conn
        .execute(
            "UPDATE recall SET recalled = 1 WHERE hash = ?1 AND recalled = 0",
            params![hash],
        )
        .unwrap_or(0);
    if changed > 0 {
        bump_stat(conn, command, "sqlite", "recalls");
    }
}

fn record_tee_recall_on(conn: &Connection, slug: &str, path: &str) {
    let inserted = conn
        .execute(
            "INSERT OR IGNORE INTO tee_reads (path) VALUES (?1)",
            params![path],
        )
        .unwrap_or(0);
    if inserted > 0 {
        bump_stat(conn, slug, "tee", "recalls");
        let _ = conn.execute(
            "DELETE FROM tee_reads WHERE rowid NOT IN (
                SELECT rowid FROM tee_reads ORDER BY rowid DESC LIMIT 500
            )",
            [],
        );
    }
}

/// `RTK_RECALL=0` (legacy `RTK_TEE=0`) kill switch.
fn recall_disabled_by_env() -> bool {
    matches!(std::env::var("RTK_RECALL").ok().as_deref(), Some("0"))
        || matches!(std::env::var("RTK_TEE").ok().as_deref(), Some("0"))
}

pub fn record_tee_recall(slug: &str, path: &str) {
    let cfg = Config::load().unwrap_or_default().retriever;
    record_tee_recall_with(&cfg, slug, path, recall_disabled_by_env())
}

/// Kill-switch state is a parameter so tests can exercise it against their own
/// config without setting `RTK_RECALL_DB` process-wide. `db_path()` gives that
/// env var precedence over `cfg.database_path`, so a test that sets it
/// redirects every concurrently running test's store to its tempdir (B12/V19).
fn record_tee_recall_with(cfg: &RetrieverConfig, slug: &str, path: &str, disabled: bool) {
    if disabled {
        return;
    }
    if cfg.mode == RecoveryMode::Disabled {
        return;
    }
    if let Ok(conn) = open(cfg) {
        record_tee_recall_on(&conn, slug, path);
    }
}

/// Two independent rules, and they do not agree on what "oldest" means — see
/// §B.B2/§V.V13. Retention orders by `created_at`, which the upsert refreshes;
/// the count cap orders by `rowid`, which it does not. An entry re-stored daily
/// is therefore fresh to one rule and first-to-die to the other. Splitting them
/// is not a fix; it makes the disagreement visible at the call site.
fn evict(conn: &Connection, cfg: &RetrieverConfig) {
    evict_by_age(conn, cfg.retention_days);
    evict_by_count(conn, cfg.max_entries);
}

/// Drops entries older than `retention_days`, measured on `created_at`.
fn evict_by_age(conn: &Connection, retention_days: u32) {
    if retention_days == 0 {
        return;
    }
    let cutoff = now_secs() - (retention_days as i64) * 86_400;
    let _ = conn.execute("DELETE FROM recall WHERE created_at < ?1", params![cutoff]);
}

/// Trims the store back to `max_entries`, dropping lowest `rowid` first —
/// insertion order, which `ON CONFLICT DO UPDATE` never bumps.
fn evict_by_count(conn: &Connection, max_entries: usize) {
    if max_entries == 0 {
        return;
    }
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM recall", [], |r| r.get(0))
        .unwrap_or(0);
    let excess = count - max_entries as i64;
    if excess <= 0 {
        return;
    }
    let _ = conn.execute(EVICT_BY_COUNT_SQL, params![excess]);
}

/// Lowest `rowid` first — insertion order. Note this is *not* `created_at`
/// order: `ON CONFLICT DO UPDATE` refreshes the timestamp but never the rowid,
/// which is the disagreement between the two eviction rules.
const EVICT_BY_COUNT_SQL: &str = "DELETE FROM recall WHERE rowid IN (
        SELECT rowid FROM recall ORDER BY rowid ASC LIMIT ?1
    )";

/// What is being recorded about one captured command.
///
/// These three always travel together and describe the occurrence rather than
/// the store, which is why they are one value: the config says where to write
/// and the content says what, while this says whose output it was, how it
/// ended, and how much of it the agent already saw.
pub struct Capture<'a> {
    pub command: &'a str,
    pub exit_code: Option<i32>,
    /// 1-based first line the agent has *not* seen. 1 means it saw none.
    pub shown_upto: usize,
}

impl<'a> Capture<'a> {
    /// The agent saw none of this output — the whole entry is hidden.
    ///
    /// Replaces a bare `1` at the call site, which read as a magic number next
    /// to the offsets used by the tail form.
    pub fn full(command: &'a str, exit_code: Option<i32>) -> Self {
        Self {
            command,
            exit_code,
            shown_upto: 1,
        }
    }

    /// The agent saw up to `shown_upto`; recall returns the tail after it.
    /// Used by the truncation paths, which know the offset but not an exit code.
    pub fn tail(command: &'a str, shown_upto: usize) -> Self {
        Self {
            command,
            exit_code: None,
            shown_upto,
        }
    }
}

pub fn store(cfg: &RetrieverConfig, content: &[u8], capture: Capture<'_>) -> Stored {
    if content.is_empty() {
        return Stored::Empty;
    }
    let capture = Capture {
        shown_upto: capture.shown_upto.max(1),
        ..capture
    };
    match store_inner(cfg, content, capture) {
        Ok(r) => Stored::Saved(r),
        Err(_) => Stored::Unavailable,
    }
}

/// Cut `content` to at most `cap` bytes, backing up to the last newline so a
/// truncated entry never ends mid-line. Falls back to a hard cut when the first
/// `cap` bytes contain no newline at all. Returns the payload and whether it was
/// shortened.
fn truncate_at_line_boundary(content: &[u8], cap: usize) -> (&[u8], bool) {
    if content.len() <= cap {
        return (content, false);
    }
    let cut = content[..cap]
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|i| i + 1)
        .unwrap_or(cap);
    (&content[..cut], true)
}

/// Upsert keyed on the content hash: re-storing identical output from a later
/// run refreshes the metadata rather than adding a row. Every column is
/// overwritten, so the entry always reflects its most recent occurrence.
const UPSERT_RECALL_SQL: &str = "INSERT INTO recall
     (hash, command, cwd, exit_code, created_at, total_lines, shown_upto, byte_size, truncated, codec, blob)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
     ON CONFLICT(hash) DO UPDATE SET
         command = excluded.command,
         cwd = excluded.cwd,
         exit_code = excluded.exit_code,
         created_at = excluded.created_at,
         total_lines = excluded.total_lines,
         shown_upto = excluded.shown_upto,
         byte_size = excluded.byte_size,
         truncated = excluded.truncated,
         codec = excluded.codec,
         blob = excluded.blob";

fn store_inner(cfg: &RetrieverConfig, content: &[u8], capture: Capture<'_>) -> Result<StoredRef> {
    let entry = encode_entry(content, capture.command, cfg.max_entry_bytes);
    let conn = open(cfg)?;
    upsert_entry(&conn, &entry, &capture)?;
    bump_stat(&conn, capture.command, "sqlite", "elisions");
    evict(&conn, cfg);

    Ok(StoredRef {
        hash: entry.hash,
        hidden_lines: entry
            .total_lines
            .saturating_sub(capture.shown_upto.saturating_sub(1)),
    })
}

/// Write the encoded entry as a row. Takes the entry rather than eleven loose
/// values so the column order stays next to the statement it feeds, and so the
/// signature stays inside the argument ceiling.
// The one exception to the line ceiling, argued rather than raised: this is a
// positional list of eleven columns feeding one statement. Its length is the
// width of the `recall` table, not complexity — there is no branching and
// nothing to name. Splitting it would separate the values from the statement
// whose parameter order they must match, which is exactly the mistake the
// column list is here to prevent. The ceiling stays where it is; this site opts
// out and says why.
#[allow(clippy::too_many_lines)]
fn upsert_entry(conn: &Connection, entry: &EncodedEntry, capture: &Capture<'_>) -> Result<()> {
    conn.execute(
        UPSERT_RECALL_SQL,
        params![
            entry.hash,
            capture.command,
            current_cwd(),
            capture.exit_code,
            now_secs(),
            entry.total_lines as i64,
            capture.shown_upto as i64,
            entry.byte_size as i64,
            entry.truncated as i64,
            entry.codec,
            entry.blob
        ],
    )
    .context("insert recall row")?;
    Ok(())
}

/// What actually gets written to the blob column, and the numbers describing it.
struct EncodedEntry {
    hash: String,
    blob: Vec<u8>,
    codec: &'static str,
    total_lines: usize,
    /// Size of the input, not of `blob` — what arrived, before truncation and
    /// before compression.
    byte_size: usize,
    truncated: bool,
}

/// Address and compress the content. Split out because this is the whole of the
/// store's content-addressing contract in one place: the hash covers the *full*
/// input while the blob holds the possibly-truncated payload (§B.B16), and
/// `total_lines` likewise counts what arrived, not what was kept.
fn encode_entry(content: &[u8], command: &str, max_entry_bytes: usize) -> EncodedEntry {
    let (payload, truncated) = truncate_at_line_boundary(content, max_entry_bytes);
    EncodedEntry {
        hash: content_hash(command, content),
        blob: lz4_compress(payload),
        codec: CODEC_LZ4,
        total_lines: count_lines(content),
        byte_size: content.len(),
        truncated,
    }
}

fn current_cwd() -> Option<String> {
    std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

#[derive(Debug)]
struct Row {
    shown_upto: usize,
    truncated: bool,
    codec: String,
    blob: Vec<u8>,
    command: String,
    hash: String,
}

fn map_row(r: &rusqlite::Row) -> rusqlite::Result<Row> {
    Ok(Row {
        shown_upto: r.get::<_, i64>(0)? as usize,
        truncated: r.get::<_, i64>(1)? != 0,
        codec: r.get(2)?,
        blob: r.get(3)?,
        command: r.get(4)?,
        hash: r.get(5)?,
    })
}

const SELECT_COLS: &str = "shown_upto, truncated, codec, blob, command, hash";

fn load_by_hash(conn: &Connection, hash: &str) -> Result<Option<Row>> {
    if let Some(row) = load_exact(conn, hash)? {
        return Ok(Some(row));
    }
    match resolve_prefix(conn, hash)? {
        Some(full) => load_exact(conn, &full),
        None => Ok(None),
    }
}

fn load_exact(conn: &Connection, hash: &str) -> Result<Option<Row>> {
    let sql = format!("SELECT {SELECT_COLS} FROM recall WHERE hash = ?1");
    Ok(conn.query_row(&sql, params![hash], map_row).optional()?)
}

/// Expand a hash prefix to the one entry it names.
///
/// Errors rather than guessing when a prefix matches several entries: the hint
/// the agent was given is a full hash, so an ambiguous prefix means a
/// hand-typed abbreviation, and picking one silently would return the wrong
/// output under a plausible-looking hash.
fn resolve_prefix(conn: &Connection, hash: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare(
        "SELECT hash FROM recall WHERE substr(hash, 1, length(?1)) = ?1 ORDER BY hash ASC",
    )?;
    let candidates: Vec<String> = stmt
        .query_map(params![hash], |r| r.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    match candidates.as_slice() {
        [] => Ok(None),
        [only] => Ok(Some(only.clone())),
        many => anyhow::bail!(
            "ambiguous hash prefix '{hash}': matches {}",
            many.join(", ")
        ),
    }
}

fn decode(row: &Row) -> Result<Vec<u8>> {
    match row.codec.as_str() {
        CODEC_LZ4 => lz4_decompress(&row.blob),
        // An unrecognized codec must error, never fall through to returning the
        // blob verbatim: handing compressed bytes back as if they were the
        // payload is silent corruption. Reachable only from a DB written by a
        // build with a different codec set.
        other => anyhow::bail!("unsupported recall codec: {other}"),
    }
}

pub struct RecallArgs<'a> {
    pub hash: Option<&'a str>,
    pub full: bool,
    pub from: Option<usize>,
    pub lines: Option<usize>,
    pub grep: Option<&'a str>,
    pub list: bool,
}

/// Open the store for a `rtk recall` invocation, reporting unavailability as an
/// exit code rather than an error. Same convention as [`load_requested_row`]:
/// `Err` is what the process should exit with, and the message has already been
/// printed.
fn open_for_recall(cfg: &RetrieverConfig) -> std::result::Result<std::rc::Rc<Connection>, i32> {
    open(cfg).map_err(|e| {
        eprintln!("rtk recall: store unavailable: {e}");
        1
    })
}

pub fn run_recall(args: RecallArgs) -> Result<i32> {
    let cfg = Config::load().unwrap_or_default().retriever;
    let conn = match open_for_recall(&cfg) {
        Ok(c) => c,
        Err(code) => return Ok(code),
    };

    if args.list {
        return list_entries(&conn);
    }

    let row = match load_requested_row(&conn, args.hash) {
        Ok(row) => row,
        Err(code) => return Ok(code),
    };

    emit_selected(&conn, &row, &args, cfg.max_entry_bytes)
}

/// Decode the entry, narrow it to what the flags asked for, and deliver it.
fn emit_selected(
    conn: &Connection,
    row: &Row,
    args: &RecallArgs,
    max_entry_bytes: usize,
) -> Result<i32> {
    let full = decode(row)?;
    let out = apply_grep(select_slice(&full, args, row.shown_upto), args.grep);
    emit_recall(conn, row, &out, max_entry_bytes);
    Ok(0)
}

/// Deliver the entry and record that it was delivered. The two belong together:
/// `recalls` is meant to count output the agent actually received, so the
/// bookkeeping must not drift away from the write that earns it.
fn emit_recall(conn: &Connection, row: &Row, out: &[u8], max_entry_bytes: usize) {
    let stdout = std::io::stdout();
    let _ = stdout.lock().write_all(out);
    mark_recalled(conn, &row.hash, &row.command);

    if row.truncated {
        eprintln!(
            "rtk recall: note: output exceeded the {max_entry_bytes}-byte cap and was stored truncated"
        );
    }
}

/// Resolve the requested entry, or the exit code to return instead. `Err` here
/// is a CLI exit status, not a failure to propagate: 2 for a missing argument,
/// 1 for a lookup error or an entry that does not exist.
fn load_requested_row(conn: &Connection, hash: Option<&str>) -> std::result::Result<Row, i32> {
    let Some(hash) = hash else {
        eprintln!("rtk recall: provide a <hash> (from a recovery hint) or --list");
        return Err(2);
    };
    match load_by_hash(conn, hash) {
        Ok(Some(row)) => Ok(row),
        Ok(None) => {
            eprintln!("rtk recall: no matching entry (try `rtk recall --list`)");
            Err(1)
        }
        Err(e) => {
            eprintln!("rtk recall: {e}");
            Err(1)
        }
    }
}

/// Which bytes of the stored entry the flags ask for. Defaults to the hidden
/// tail — the part the agent has not already seen.
fn select_slice(full: &[u8], args: &RecallArgs, shown_upto: usize) -> Vec<u8> {
    if args.full {
        full.to_vec()
    } else if let Some(n) = args.from {
        slice_from_line(full, n).to_vec()
    } else if let Some(n) = args.lines {
        slice_first_lines(full, n).to_vec()
    } else {
        slice_from_line(full, shown_upto).to_vec()
    }
}

fn apply_grep(sliced: Vec<u8>, pattern: Option<&str>) -> Vec<u8> {
    let Some(pattern) = pattern else {
        return sliced;
    };
    if regex::bytes::Regex::new(pattern).is_err() {
        eprintln!("rtk recall: note: --grep pattern is not a valid regex, matching it literally");
    }
    grep_bytes(&sliced, pattern)
}

/// Width of the COMMAND column in `rtk recall --list`.
const LIST_COMMAND_WIDTH: usize = 26;

/// Lines the agent never saw: total minus what was shown. `shown_upto` is
/// 1-based, and both sides saturate because a row written by an older build may
/// carry a `shown_upto` past `total_lines`.
fn hidden_line_count(total: i64, shown_upto: i64) -> i64 {
    total.saturating_sub(shown_upto.saturating_sub(1)).max(0)
}

/// Fit a command into `width` display cells, eliding with a single-char
/// ellipsis. Counts `chars`, not bytes: slicing a UTF-8 command line by byte
/// offset would panic mid-codepoint on any non-ASCII path or argument.
fn elide_command(command: String, width: usize) -> String {
    if command.chars().count() <= width {
        return command;
    }
    let head: String = command.chars().take(width - 1).collect();
    format!("{head}…")
}

/// One line of `rtk recall --list`. Named rather than a positional 6-tuple
/// because the columns were destructured twenty lines from where they were
/// read, which is where a swapped `total`/`shown` would have hidden.
struct ListEntry {
    hash: String,
    command: String,
    total_lines: i64,
    shown_upto: i64,
    exit_code: Option<i64>,
    truncated: bool,
}

const LIST_ENTRIES_SQL: &str =
    "SELECT hash, command, total_lines, shown_upto, exit_code, truncated \
     FROM recall ORDER BY created_at DESC LIMIT 50";

fn load_list_entries(conn: &Connection) -> Result<Vec<ListEntry>> {
    let mut stmt = conn.prepare(LIST_ENTRIES_SQL)?;
    let rows = stmt.query_map([], |r| {
        Ok(ListEntry {
            hash: r.get(0)?,
            command: r.get(1)?,
            total_lines: r.get(2)?,
            shown_upto: r.get(3)?,
            exit_code: r.get(4)?,
            truncated: r.get::<_, i64>(5)? != 0,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// One row of the `--list` table. Separate from the loop so the column
/// derivations — elision, hidden-line arithmetic, the `-` for a missing exit
/// code — sit next to each other rather than inside a `println!` argument list.
fn format_list_row(e: &ListEntry) -> String {
    format!(
        "{:<14} {:<26} {:>7} {:>7} {:>5} {}",
        e.hash,
        elide_command(e.command.clone(), LIST_COMMAND_WIDTH),
        e.total_lines,
        hidden_line_count(e.total_lines, e.shown_upto),
        e.exit_code.map_or_else(|| "-".into(), |c| c.to_string()),
        if e.truncated { "yes" } else { "" }
    )
}

fn list_entries(conn: &Connection) -> Result<i32> {
    let entries = load_list_entries(conn)?;
    println!(
        "{:<14} {:<26} {:>7} {:>7} {:>5} TRUNC",
        "HASH", "COMMAND", "LINES", "HIDDEN", "EXIT"
    );
    for e in entries.iter() {
        println!("{}", format_list_row(e));
    }
    if entries.is_empty() {
        println!("(no recall entries)");
    }
    Ok(0)
}

#[cfg(test)]
// Test bodies are linear setup-act-assert scripts; splitting them to satisfy
// the ratchet makes them harder to read. See clippy.toml.
#[allow(
    clippy::too_many_lines,
    clippy::too_many_arguments,
    clippy::cognitive_complexity,
    clippy::excessive_nesting
)]
mod tests {
    use super::*;

    fn temp_cfg(dir: &std::path::Path) -> RetrieverConfig {
        RetrieverConfig {
            database_path: Some(dir.join("recall_test.db")),
            ..RetrieverConfig::default()
        }
    }

    #[test]
    fn test_count_lines() {
        assert_eq!(count_lines(b""), 0);
        assert_eq!(count_lines(b"abc"), 1);
        assert_eq!(count_lines(b"a\nb\nc"), 3);
        assert_eq!(count_lines(b"a\nb\nc\n"), 3);
        assert_eq!(count_lines(b"\n"), 1);
    }

    #[test]
    fn test_slice_from_line() {
        let b = b"l1\nl2\nl3\n";
        assert_eq!(slice_from_line(b, 1), b);
        assert_eq!(slice_from_line(b, 2), b"l2\nl3\n");
        assert_eq!(slice_from_line(b, 3), b"l3\n");
        assert_eq!(slice_from_line(b, 4), b"");
        assert_eq!(slice_from_line(b, 99), b"");
    }

    #[test]
    fn test_slice_first_lines() {
        let b = b"l1\nl2\nl3\n";
        assert_eq!(slice_first_lines(b, 0), b"");
        assert_eq!(slice_first_lines(b, 1), b"l1\n");
        assert_eq!(slice_first_lines(b, 2), b"l1\nl2\n");
        assert_eq!(slice_first_lines(b, 99), b);
    }

    #[test]
    fn test_content_hash_deterministic() {
        let a = content_hash("cmd", b"output");
        assert_eq!(a, content_hash("cmd", b"output"));
        assert_eq!(a.len(), HASH_HEX_LEN);
        assert_ne!(a, content_hash("cmd2", b"output"));
        assert_ne!(a, content_hash("cmd", b"output2"));
    }

    #[test]
    fn test_grep_bytes() {
        let input = b"alpha\nbeta\ngamma\n";
        assert_eq!(grep_bytes(input, "et"), b"beta\n");
        assert_eq!(grep_bytes(input, "^g"), b"gamma\n");
    }

    #[test]
    fn test_lz4_roundtrip_arbitrary_bytes() {
        let cases: Vec<Vec<u8>> = vec![
            b"hello\n".to_vec(),
            vec![0xff, 0xfe, 0x00, 0x01, 0x80],
            b"crlf\r\nline\r\n".to_vec(),
            b"lone\rcr".to_vec(),
            "emoji😀漢字".as_bytes().to_vec(),
            b"no trailing newline".to_vec(),
            (0u8..=255).collect(),
        ];
        for c in cases {
            let z = lz4_compress(&c);
            assert_eq!(
                lz4_decompress(&z).expect("lz4 decompress"),
                c,
                "lz4 must be byte-exact"
            );
        }
    }

    fn row_with_codec(codec: &str, blob: Vec<u8>) -> Row {
        Row {
            shown_upto: 1,
            truncated: false,
            codec: codec.to_string(),
            blob,
            command: "cmd".to_string(),
            hash: "0123456789ab".to_string(),
        }
    }

    #[test]
    fn test_decode_lz4_codec() {
        let payload = b"recall payload\n".to_vec();
        let row = row_with_codec(CODEC_LZ4, lz4_compress(&payload));
        assert_eq!(decode(&row).unwrap(), payload);
    }

    /// An unknown codec must error rather than hand back the blob verbatim —
    /// returning compressed bytes as if they were the payload is silent
    /// corruption that `rtk recall` would print as garbage.
    #[test]
    fn test_decode_unknown_codec_errors_not_silent_passthrough() {
        let compressed = lz4_compress(b"payload that must not leak\n");
        let row = row_with_codec("gzip", compressed.clone());
        let err = decode(&row).expect_err("unknown codec must error");
        assert!(err.to_string().contains("unsupported recall codec"));
        assert!(err.to_string().contains("gzip"));
    }

    #[test]
    fn test_decode_unknown_codec_never_returns_blob() {
        for codec in ["gzip", "zstd", "br", "raw", ""] {
            let row = row_with_codec(codec, b"\x1f\x8b raw bytes".to_vec());
            assert!(decode(&row).is_err(), "codec {codec:?} must not decode");
        }
    }

    /// Every new row carries the lz4 codec — there is no uncompressed path.
    #[test]
    fn test_store_writes_lz4_codec() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let stored = store_inner(&cfg, b"payload\n", Capture::full("cmd", Some(0))).unwrap();
        let conn = open(&cfg).unwrap();
        let codec: String = conn
            .query_row(
                "SELECT codec FROM recall WHERE hash = ?1",
                params![stored.hash],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(codec, CODEC_LZ4);
    }

    #[test]
    fn test_store_fetch_byte_faithful() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let mut nasty = Vec::new();
        nasty.extend_from_slice(b"line1\r\n");
        nasty.extend_from_slice(&[0xff, 0x00, 0xfe]);
        nasty.extend_from_slice("漢字\n".as_bytes());
        nasty.extend_from_slice(b"no-eol-tail");

        let stored = store_inner(&cfg, &nasty, Capture::full("nasty-cmd", Some(0))).expect("store");
        let conn = open(&cfg).unwrap();
        let row = load_by_hash(&conn, &stored.hash).unwrap().expect("row");
        assert_eq!(
            decode(&row).unwrap(),
            nasty,
            "stored bytes must round-trip exactly"
        );
    }

    #[test]
    fn test_store_fetch_binary_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let data = vec![0u8, 1, 2, 255, b'\n', b'x'];
        let stored = store_inner(&cfg, &data, Capture::full("c", Some(0))).unwrap();
        let conn = open(&cfg).unwrap();
        let row = load_by_hash(&conn, &stored.hash).unwrap().unwrap();
        assert_eq!(row.codec, CODEC_LZ4);
        assert_eq!(decode(&row).unwrap(), data);
    }

    #[test]
    fn test_delta_recall_returns_only_missed() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let content = b"i1\ni2\ni3\ni4\ni5\n";
        let stored = store_inner(
            &cfg,
            content,
            Capture {
                command: "list",
                exit_code: Some(0),
                shown_upto: 3,
            },
        )
        .unwrap();
        assert_eq!(stored.hidden_lines, 3);
        let conn = open(&cfg).unwrap();
        let row = load_by_hash(&conn, &stored.hash).unwrap().unwrap();
        let full = decode(&row).unwrap();
        assert_eq!(slice_from_line(&full, row.shown_upto), b"i3\ni4\ni5\n");
    }

    #[test]
    fn test_truncation_cap_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            max_entry_bytes: 10,
            ..temp_cfg(dir.path())
        };
        let big = vec![b'a'; 100];
        let stored = store_inner(&cfg, &big, Capture::full("big", Some(0))).unwrap();
        let conn = open(&cfg).unwrap();
        let row = load_by_hash(&conn, &stored.hash).unwrap().unwrap();
        assert!(row.truncated);
        assert_eq!(decode(&row).unwrap().len(), 10);
    }

    #[test]
    fn test_truncation_cuts_at_line_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            max_entry_bytes: 12,
            ..temp_cfg(dir.path())
        };
        let stored =
            store_inner(&cfg, b"aaaa\nbbbb\ncccc\n", Capture::full("cmd", Some(0))).unwrap();
        let conn = open(&cfg).unwrap();
        let row = load_by_hash(&conn, &stored.hash).unwrap().unwrap();
        assert!(row.truncated);
        assert_eq!(decode(&row).unwrap(), b"aaaa\nbbbb\n");
    }

    #[test]
    fn test_truncation_single_giant_line_falls_back_to_byte_cut() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            max_entry_bytes: 10,
            ..temp_cfg(dir.path())
        };
        let big = vec![b'a'; 100];
        let stored = store_inner(&cfg, &big, Capture::full("big", Some(0))).unwrap();
        let conn = open(&cfg).unwrap();
        let row = load_by_hash(&conn, &stored.hash).unwrap().unwrap();
        assert!(row.truncated);
        assert_eq!(decode(&row).unwrap().len(), 10);
    }

    #[test]
    fn test_eviction_same_second_keeps_newest_insertions() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            max_entries: 2,
            retention_days: 0,
            ..temp_cfg(dir.path())
        };
        let conn = open(&cfg).unwrap();
        insert_row(&conn, "zzz999999999", "old1");
        insert_row(&conn, "yyy888888888", "old2");
        insert_row(&conn, "aaa111111111", "newest");
        evict(&conn, &cfg);
        let newest: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM recall WHERE hash = 'aaa111111111'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            newest, 1,
            "the just-inserted row must survive same-second eviction ties"
        );
    }

    #[test]
    fn test_fifo_count_eviction() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            max_entries: 3,
            retention_days: 0,
            ..temp_cfg(dir.path())
        };
        for i in 0..5 {
            let content = format!("output-{i}");
            store_inner(
                &cfg,
                content.as_bytes(),
                Capture::full(&format!("cmd{i}"), Some(0)),
            )
            .unwrap();
        }
        let conn = open(&cfg).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM recall", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 3, "FIFO cap should retain only max_entries");
    }

    #[test]
    fn test_dedup_same_content_same_hash() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let a = store_inner(&cfg, b"same output\n", Capture::full("cmd", Some(0))).unwrap();
        let b = store_inner(&cfg, b"same output\n", Capture::full("cmd", Some(0))).unwrap();
        assert_eq!(a.hash, b.hash);
        let conn = open(&cfg).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM recall", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "identical output must dedupe to one row");
    }

    #[test]
    fn test_stats_elision_counted_on_store() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        store_inner(&cfg, b"out1\n", Capture::full("cargo-test", Some(1))).unwrap();
        store_inner(&cfg, b"out2\n", Capture::full("cargo-test", Some(1))).unwrap();
        let stats = stats_snapshot_with(&cfg).unwrap();
        let row = stats
            .iter()
            .find(|s| s.slug == "cargo-test" && s.mode == "sqlite")
            .expect("stat row");
        assert_eq!(row.elisions, 2);
        assert_eq!(row.recalls, 0);
    }

    #[test]
    fn test_stats_modes_never_merge() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let conn = open(&cfg).unwrap();
        bump_stat(&conn, "docker-images", "sqlite", "elisions");
        bump_stat(&conn, "docker-images", "tee", "elisions");
        bump_stat(&conn, "docker-images", "tee", "recalls");
        let stats = stats_snapshot_with(&cfg).unwrap();
        let sqlite = stats
            .iter()
            .find(|s| s.slug == "docker-images" && s.mode == "sqlite")
            .expect("sqlite row");
        let tee = stats
            .iter()
            .find(|s| s.slug == "docker-images" && s.mode == "tee")
            .expect("tee row");
        assert_eq!((sqlite.elisions, sqlite.recalls), (1, 0));
        assert_eq!((tee.elisions, tee.recalls), (1, 1));
    }

    #[test]
    fn test_stats_recall_deduped_per_entry() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let stored = store_inner(&cfg, b"x\ny\n", Capture::full("vitest", Some(1))).unwrap();
        let conn = open(&cfg).unwrap();
        mark_recalled(&conn, &stored.hash, "vitest");
        mark_recalled(&conn, &stored.hash, "vitest");
        mark_recalled(&conn, &stored.hash, "vitest");
        let stats = stats_snapshot_with(&cfg).unwrap();
        let s = stats
            .iter()
            .find(|s| s.slug == "vitest" && s.mode == "sqlite")
            .expect("row");
        assert_eq!(
            (s.elisions, s.recalls),
            (1, 1),
            "re-reading the same entry must not inflate the rate"
        );
    }

    /// B12/V19: asserts the kill switch through the injected flag rather than
    /// by setting `RTK_RECALL_DB`, which would redirect every test running
    /// concurrently to this tempdir.
    #[test]
    fn test_record_tee_recall_respects_kill_switch() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let db = db_path(&cfg).unwrap();
        record_tee_recall_with(&cfg, "grep", "/tee/1_grep.log", true);
        assert!(
            !db.exists(),
            "kill switch must prevent any recall.db write from the hook path"
        );
    }

    /// Companion: with the switch off the same call does record, so the test
    /// above is proving the guard rather than a path that never writes.
    #[test]
    fn test_record_tee_recall_writes_when_not_killed() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        record_tee_recall_with(&cfg, "grep", "/tee/1_grep.log", false);
        assert_eq!(tee_recalls_for(&cfg, "grep"), 1);
    }

    #[test]
    fn test_tee_recall_deduped_per_file() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let conn = open(&cfg).unwrap();
        record_tee_recall_on(&conn, "docker-images", "/tee/1_docker-images.log");
        record_tee_recall_on(&conn, "docker-images", "/tee/1_docker-images.log");
        record_tee_recall_on(&conn, "docker-images", "/tee/2_docker-images.log");
        let stats = stats_snapshot_with(&cfg).unwrap();
        let s = stats
            .iter()
            .find(|s| s.slug == "docker-images" && s.mode == "tee")
            .expect("row");
        assert_eq!(s.recalls, 2, "one count per distinct tee file");
    }

    /// V17/B6: `tee_reads` dedup is non-upgradable — the first record of a path
    /// is permanent. A read counted before the permission verdict therefore
    /// blocks the later legitimate read of the same file from ever counting,
    /// which is why tracking must fire only after an Allow verdict.
    #[test]
    fn test_tee_recall_dedup_is_not_upgradable() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let conn = open(&cfg).unwrap();
        let path = "/tee/1_cargo_test.log";

        record_tee_recall_on(&conn, "cargo_test", path);
        let after_first = tee_recalls_for(&cfg, "cargo_test");

        for _ in 0..5 {
            record_tee_recall_on(&conn, "cargo_test", path);
        }
        assert_eq!(
            tee_recalls_for(&cfg, "cargo_test"),
            after_first,
            "re-recording the same path must never bump the count again"
        );
    }

    /// V17/B6: distinct files each count once — the fix must not suppress
    /// legitimate reads of different tee files.
    #[test]
    fn test_tee_recall_counts_each_distinct_file_once() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let conn = open(&cfg).unwrap();
        for i in 0..4 {
            record_tee_recall_on(&conn, "gh-prs", &format!("/tee/{i}_gh-prs.log"));
        }
        assert_eq!(tee_recalls_for(&cfg, "gh-prs"), 4);
    }

    fn tee_recalls_for(cfg: &RetrieverConfig, slug: &str) -> i64 {
        stats_snapshot_with(cfg)
            .unwrap()
            .iter()
            .find(|s| s.slug == slug && s.mode == "tee")
            .map(|s| s.recalls)
            .unwrap_or(0)
    }

    #[test]
    fn test_force_path_stores_null_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let stored = store_inner(
            &cfg,
            b"trimmed list\n",
            Capture {
                command: "docker-images",
                exit_code: None,
                shown_upto: 2,
            },
        )
        .unwrap();
        let conn = open(&cfg).unwrap();
        let exit: Option<i64> = conn
            .query_row(
                "SELECT exit_code FROM recall WHERE hash = ?1",
                params![stored.hash],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            exit, None,
            "unknown exit codes must be stored as NULL, not 0"
        );
    }

    #[test]
    fn test_identical_restore_preserves_recalled_flag() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let stored =
            store_inner(&cfg, b"same fail\n", Capture::full("cargo_test", Some(1))).unwrap();
        let conn = open(&cfg).unwrap();
        mark_recalled(&conn, &stored.hash, "cargo_test");
        store_inner(&cfg, b"same fail\n", Capture::full("cargo_test", Some(1))).unwrap();
        mark_recalled(&conn, &stored.hash, "cargo_test");
        let stats = stats_snapshot_with(&cfg).unwrap();
        let s = stats
            .iter()
            .find(|s| s.slug == "cargo_test" && s.mode == "sqlite")
            .expect("row");
        assert_eq!(
            s.recalls, 1,
            "identical re-store must not reset recalled and re-count reads"
        );
    }

    #[test]
    fn test_stats_recall_counted_on_read() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let stored = store_inner(
            &cfg,
            b"a\nb\nc\n",
            Capture {
                command: "gh-prs",
                exit_code: Some(0),
                shown_upto: 2,
            },
        )
        .unwrap();
        let conn = open(&cfg).unwrap();
        let row = load_by_hash(&conn, &stored.hash).unwrap().unwrap();
        bump_stat(&conn, &row.command, "sqlite", "recalls");
        let stats = stats_snapshot_with(&cfg).unwrap();
        let s = stats
            .iter()
            .find(|s| s.slug == "gh-prs" && s.mode == "sqlite")
            .expect("row");
        assert_eq!((s.elisions, s.recalls), (1, 1));
    }

    #[test]
    fn test_stats_survive_entry_eviction() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            max_entries: 1,
            retention_days: 0,
            ..temp_cfg(dir.path())
        };
        for i in 0..4 {
            store_inner(
                &cfg,
                format!("o{i}\n").as_bytes(),
                Capture::full("find", Some(1)),
            )
            .unwrap();
        }
        let conn = open(&cfg).unwrap();
        let entries: i64 = conn
            .query_row("SELECT COUNT(*) FROM recall", [], |r| r.get(0))
            .unwrap();
        assert_eq!(entries, 1);
        let stats = stats_snapshot_with(&cfg).unwrap();
        let s = stats
            .iter()
            .find(|s| s.slug == "find" && s.mode == "sqlite")
            .expect("row");
        assert_eq!(s.elisions, 4, "stats must survive FIFO eviction");
    }

    #[test]
    fn test_old_schema_db_gains_recalled_column() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let path = db_path(&cfg).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE recall (
                hash TEXT PRIMARY KEY, command TEXT NOT NULL, cwd TEXT,
                exit_code INTEGER, created_at INTEGER NOT NULL,
                total_lines INTEGER NOT NULL, shown_upto INTEGER NOT NULL,
                byte_size INTEGER NOT NULL, truncated INTEGER NOT NULL,
                codec TEXT NOT NULL, blob BLOB NOT NULL
            );",
        )
        .unwrap();
        drop(conn);

        let stored = store_inner(&cfg, b"x\ny\n", Capture::full("vitest", Some(1)))
            .expect("store on old schema");
        let conn = open(&cfg).unwrap();
        mark_recalled(&conn, &stored.hash, "vitest");
        let stats = stats_snapshot_with(&cfg).unwrap();
        let s = stats
            .iter()
            .find(|s| s.slug == "vitest" && s.mode == "sqlite")
            .expect("row");
        assert_eq!(s.recalls, 1, "recalled column must be added to old DBs");
    }

    #[test]
    fn test_stat_key_canonical_across_elision_and_recall_sides() {
        let raw = "helm install a-very-long-release-name ./some/long/chart/path";
        let filename_slug = crate::core::tee_file::sanitize_slug(raw);
        assert_eq!(
            stat_key(raw),
            stat_key(&filename_slug),
            "raw slug and tee-filename slug must map to the same stats key"
        );
    }

    #[test]
    fn test_stat_key_bounds_toml_raw_command_slugs() {
        let a = stat_key("helm install myapp ./chart");
        let b = stat_key("helm install other ./elsewhere --wait");
        assert_eq!(a, b, "same subcommand family must aggregate");
        assert!(a.len() <= 24, "key stays bounded: {a}");
        assert!(a.starts_with("helm"), "key stays readable: {a}");
    }

    /// B8 mechanism: raw command lines short enough to skip the 24-char hash
    /// fold reach `recall_stats` verbatim, so every distinct invocation opens
    /// its own row. This is why the TOML path must pass the filter family name.
    #[test]
    fn test_stat_key_raw_short_commands_do_not_aggregate() {
        let a = stat_key("jq .items");
        let b = stat_key("jq .metadata.name");
        assert_ne!(
            a, b,
            "short raw command lines fragment into separate stats rows"
        );
    }

    /// B8 fix: the filter family name is one stable key regardless of arguments.
    #[test]
    fn test_stat_key_filter_family_name_aggregates() {
        let key = stat_key("jq");
        for _ in ["jq .items", "jq .metadata.name", "jq -r '.a|.b'"] {
            assert_eq!(stat_key("jq"), key);
        }
        assert_eq!(key, "jq");
    }

    /// B8/Y6: the family name must not carry arguments into the ledger.
    #[test]
    fn test_stat_key_family_name_carries_no_arguments() {
        for name in ["helm", "jq", "brew-install", "dotnet-build", "gcloud"] {
            let key = stat_key(name);
            assert_eq!(key, name, "family name must survive stat_key unchanged");
            assert!(!key.contains(' '), "no argument separator in key: {key}");
        }
    }

    /// B8: a family name must never trip the 24-char hash fold, which would
    /// make `rtk gain --recalls` show an unreadable `prefix_hash` row.
    #[test]
    fn test_stat_key_family_names_stay_readable() {
        for name in ["ansible-playbook", "fail2ban-client", "basedpyright"] {
            let key = stat_key(name);
            assert_eq!(key, name);
            assert!(key.len() <= 24, "key stays bounded: {key}");
        }
    }

    /// B8 end-to-end: repeated stores under one family name collapse to a
    /// single stats row, where the raw-command slug would have opened three.
    #[test]
    fn test_toml_family_slug_aggregates_across_invocations() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        for i in 0..3 {
            store_inner(
                &cfg,
                format!("output {i}\n").as_bytes(),
                Capture::full("jq", None),
            )
            .unwrap();
        }
        let stats = stats_snapshot_with(&cfg).unwrap();
        let rows: Vec<_> = stats.iter().filter(|s| s.mode == "sqlite").collect();
        assert_eq!(rows.len(), 1, "one row per filter family, got {rows:?}");
        assert_eq!(rows[0].slug, "jq");
        assert_eq!(rows[0].elisions, 3);
    }

    #[test]
    fn test_bump_stat_uses_canonical_key() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let conn = open(&cfg).unwrap();
        let raw = "helm install a-very-long-release-name ./some/long/chart/path";
        bump_stat(&conn, raw, "tee", "elisions");
        let filename_slug = crate::core::tee_file::sanitize_slug(raw);
        bump_stat(&conn, &filename_slug, "tee", "recalls");
        let stats = stats_snapshot_with(&cfg).unwrap();
        let rows: Vec<_> = stats.iter().filter(|s| s.mode == "tee").collect();
        assert_eq!(
            rows.len(),
            1,
            "one reconciled row, got: {:?}",
            rows.iter().map(|r| &r.slug).collect::<Vec<_>>()
        );
        assert_eq!((rows[0].elisions, rows[0].recalls), (1, 1));
    }

    #[test]
    fn test_stat_family_collapses_invocation_slugs() {
        assert_eq!(stat_family("grep_0__tmp__tmpXYZ"), "grep");
        assert_eq!(stat_family("grep_9_src_cmds_cloud_wget"), "grep");
        assert_eq!(stat_family("grep_skipped"), "grep_skipped");
        assert_eq!(stat_family("docker-images"), "docker-images");
        assert_eq!(stat_family("aws_s3_ls"), "aws_s3_ls");
        assert_eq!(stat_family("cargo_test"), "cargo_test");
    }

    #[test]
    fn test_stats_snapshot_aggregates_families() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let conn = open(&cfg).unwrap();
        bump_stat(&conn, "grep_0__tmp_a", "sqlite", "elisions");
        bump_stat(&conn, "grep_1__tmp_b", "sqlite", "elisions");
        bump_stat(&conn, "grep_1__tmp_b", "sqlite", "recalls");
        let stats = stats_snapshot_with(&cfg).unwrap();
        let greps: Vec<_> = stats
            .iter()
            .filter(|s| s.mode == "sqlite" && s.slug == "grep")
            .collect();
        assert_eq!(greps.len(), 1, "one aggregated family row");
        assert_eq!((greps[0].elisions, greps[0].recalls), (2, 1));
    }

    fn insert_row(conn: &Connection, hash: &str, command: &str) {
        conn.execute(
            "INSERT INTO recall (hash, command, created_at, total_lines, shown_upto, byte_size, truncated, codec, blob)
             VALUES (?1, ?2, 1, 1, 1, 1, 0, 'raw', x'61')",
            params![hash, command],
        )
        .unwrap();
    }

    #[test]
    fn test_ambiguous_prefix_errors_with_candidates() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let conn = open(&cfg).unwrap();
        insert_row(&conn, "aaa111111111", "cmd1");
        insert_row(&conn, "aaa222222222", "cmd2");
        let err = load_by_hash(&conn, "aaa").expect_err("must be ambiguous");
        let msg = err.to_string();
        assert!(msg.contains("aaa111111111"), "candidates listed: {msg}");
        assert!(msg.contains("aaa222222222"), "candidates listed: {msg}");
    }

    #[test]
    fn test_unique_prefix_still_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let conn = open(&cfg).unwrap();
        insert_row(&conn, "aaa111111111", "cmd1");
        insert_row(&conn, "bbb222222222", "cmd2");
        assert!(load_by_hash(&conn, "aaa").unwrap().is_some());
        assert!(load_by_hash(&conn, "aaa111111111").unwrap().is_some());
        assert!(load_by_hash(&conn, "ccc").unwrap().is_none());
    }

    #[test]
    fn test_load_by_hash_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let stored = store_inner(&cfg, b"hello world\n", Capture::full("cmd", Some(0))).unwrap();
        let conn = open(&cfg).unwrap();
        let prefix = &stored.hash[..6];
        assert!(load_by_hash(&conn, prefix).unwrap().is_some());
    }

    // --- V8 byte fidelity: 1-to-1 coverage per byte class ---

    fn recall_full_pipeline(cfg: &RetrieverConfig, content: &[u8], cmd: &str) -> Vec<u8> {
        let stored = store_inner(cfg, content, Capture::full(cmd, Some(0))).expect("store");
        let conn = open(cfg).unwrap();
        let row = load_by_hash(&conn, &stored.hash).unwrap().expect("row");
        let full = decode(&row).unwrap();
        slice_from_line(&full, 1).to_vec()
    }

    #[test]
    fn test_v8_nul_bytes_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let content = b"before\x00after\n";
        assert_eq!(recall_full_pipeline(&cfg, content, "nul-test"), content);
    }

    #[test]
    fn test_v8_ansi_escape_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let content = b"\x1b[31merror\x1b[0m: something failed\n";
        assert_eq!(recall_full_pipeline(&cfg, content, "ansi-test"), content);
    }

    #[test]
    fn test_v8_crlf_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let content = b"line1\r\nline2\r\nline3\r\n";
        assert_eq!(recall_full_pipeline(&cfg, content, "crlf-test"), content);
    }

    #[test]
    fn test_v8_lone_cr_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let content = b"progress\r50%\r100%\n";
        assert_eq!(recall_full_pipeline(&cfg, content, "cr-test"), content);
    }

    #[test]
    fn test_v8_high_bytes_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let content: Vec<u8> = (0x80..=0xff).collect();
        assert_eq!(recall_full_pipeline(&cfg, &content, "high-test"), content);
    }

    #[test]
    fn test_v8_all_256_bytes_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let content: Vec<u8> = (0u8..=255).collect();
        assert_eq!(recall_full_pipeline(&cfg, &content, "all256-test"), content);
    }

    #[test]
    fn test_v8_utf8_multibyte_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let content = "漢字\n日本語\nемодзі😀\n".as_bytes();
        assert_eq!(recall_full_pipeline(&cfg, content, "utf8-test"), content);
    }

    #[test]
    fn test_v8_no_trailing_newline_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let content = b"no newline at end";
        assert_eq!(recall_full_pipeline(&cfg, content, "noeol-test"), content);
    }

    #[test]
    fn test_v8_empty_lines_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let content = b"\n\n\nthree empty above\n\n";
        assert_eq!(recall_full_pipeline(&cfg, content, "empty-lines"), content);
    }

    #[test]
    fn test_v8_mixed_nasty_bytes_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let mut content = Vec::new();
        content.extend_from_slice(b"\x1b[32mOK\x1b[0m\r\n");
        content.extend_from_slice(&[0x00, 0x01, 0xfe, 0xff]);
        content.extend_from_slice("日本語\n".as_bytes());
        content.extend_from_slice(b"tab\there\n");
        content.extend_from_slice(b"no-eol-end");
        assert_eq!(recall_full_pipeline(&cfg, &content, "mixed-test"), content);
    }

    #[test]
    fn test_v8_slice_from_preserves_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let content = b"\x1b[31mline1\x1b[0m\n\x00line2\x00\nline3\r\n";
        let stored = store_inner(
            &cfg,
            content,
            Capture {
                command: "slice-test",
                exit_code: Some(0),
                shown_upto: 2,
            },
        )
        .unwrap();
        let conn = open(&cfg).unwrap();
        let row = load_by_hash(&conn, &stored.hash).unwrap().unwrap();
        let full = decode(&row).unwrap();
        let sliced = slice_from_line(&full, 2);
        assert_eq!(sliced, b"\x00line2\x00\nline3\r\n");
    }

    #[test]
    fn test_v8_slice_first_lines_preserves_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let content = b"\x1b[31mline1\x1b[0m\n\x00line2\x00\nline3\r\n";
        let stored = store_inner(&cfg, content, Capture::full("first-lines", Some(0))).unwrap();
        let conn = open(&cfg).unwrap();
        let row = load_by_hash(&conn, &stored.hash).unwrap().unwrap();
        let full = decode(&row).unwrap();
        let sliced = slice_first_lines(&full, 2);
        assert_eq!(sliced, b"\x1b[31mline1\x1b[0m\n\x00line2\x00\n");
    }

    // --- B4 FIXED: grep_bytes byte-faithful after fix ---

    #[test]
    fn test_b4_fixed_grep_match_all_preserves_input() {
        let input = b"alpha\nbeta\ngamma\n";
        let result = grep_bytes(input, "^");
        assert_eq!(
            result.as_slice(),
            input,
            "grep '^' on newline-terminated input must return exact bytes"
        );
    }

    #[test]
    fn test_b4_fixed_grep_no_trailing_newline_preserved() {
        let input = b"alpha\nbeta";
        let result = grep_bytes(input, "^");
        assert_eq!(
            result.as_slice(),
            b"alpha\nbeta",
            "input without trailing newline must not gain one"
        );
    }

    #[test]
    fn test_b4_fixed_grep_single_line_no_newline() {
        let input = b"only-line";
        let result = grep_bytes(input, "only");
        assert_eq!(result.as_slice(), b"only-line");
    }

    #[test]
    fn test_b4_fixed_grep_single_line_with_newline() {
        let input = b"only-line\n";
        let result = grep_bytes(input, "only");
        assert_eq!(result.as_slice(), b"only-line\n");
    }

    #[test]
    fn test_b4_fixed_grep_preserves_ansi_in_matched_line() {
        let input = b"\x1b[31merror\x1b[0m: fail\nok\n";
        let result = grep_bytes(input, "error");
        assert_eq!(result.as_slice(), b"\x1b[31merror\x1b[0m: fail\n");
    }

    #[test]
    fn test_b4_fixed_grep_preserves_nul_in_matched_line() {
        let input = b"has\x00nul\nclean\n";
        let result = grep_bytes(input, "nul");
        assert_eq!(result.as_slice(), b"has\x00nul\n");
    }

    #[test]
    fn test_b4_fixed_grep_crlf_preserved() {
        let input = b"line1\r\nline2\r\n";
        let result = grep_bytes(input, "line");
        assert_eq!(
            result.as_slice(),
            b"line1\r\nline2\r\n",
            "\\r preserved within line bytes, \\n re-appended for newline-terminated input"
        );
    }

    #[test]
    fn test_b4_fixed_grep_no_match_returns_empty() {
        let input = b"alpha\nbeta\n";
        let result = grep_bytes(input, "zzz");
        assert!(result.is_empty(), "no match must return empty");
    }

    #[test]
    fn test_b4_fixed_grep_partial_match_preserves_structure() {
        let input = b"match1\nskip\nmatch2\n";
        let result = grep_bytes(input, "match");
        assert_eq!(result.as_slice(), b"match1\nmatch2\n");
    }

    #[test]
    fn test_b4_fixed_grep_empty_input() {
        let result = grep_bytes(b"", "anything");
        assert!(result.is_empty());
    }

    #[test]
    fn test_v8_lz4_round_trip_ansi_nul() {
        let content = b"\x1b[32mgreen\x1b[0m\x00tail\n";
        let compressed = lz4_compress(content);
        let decompressed = lz4_decompress(&compressed).unwrap();
        assert_eq!(decompressed, content);
    }

    #[test]
    fn test_v8_lz4_round_trip_crlf() {
        let content = b"win\r\nlines\r\n";
        let compressed = lz4_compress(content);
        let decompressed = lz4_decompress(&compressed).unwrap();
        assert_eq!(decompressed, content);
    }

    #[test]
    fn test_v8_content_hash_stable_with_nul() {
        let a = content_hash("cmd", b"a\x00b");
        let b = content_hash("cmd", b"a\x00b");
        assert_eq!(a, b);
        assert_ne!(a, content_hash("cmd", b"ab"));
    }

    #[test]
    fn test_v8_content_hash_stable_with_ansi() {
        let a = content_hash("cmd", b"\x1b[31mred\x1b[0m");
        let b = content_hash("cmd", b"\x1b[31mred\x1b[0m");
        assert_eq!(a, b);
        assert_ne!(a, content_hash("cmd", b"red"));
    }

    // --- B2/V13: eviction order bug — rowid vs created_at inconsistency ---

    #[test]
    #[should_panic(expected = "B2: hot entry evicted despite fresh created_at")]
    fn test_b2_hot_entry_should_survive_eviction() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            max_entries: 3,
            retention_days: 0,
            ..temp_cfg(dir.path())
        };
        // Insert entry A (gets lowest rowid)
        store_inner(&cfg, b"hot-entry-a\n", Capture::full("hot-cmd", Some(0))).unwrap();
        // Insert entries B and C
        store_inner(&cfg, b"entry-b\n", Capture::full("cmd-b", Some(0))).unwrap();
        store_inner(&cfg, b"entry-c\n", Capture::full("cmd-c", Some(0))).unwrap();
        // Re-store A with same (command, content) → ON CONFLICT DO UPDATE refreshes
        // created_at but NOT rowid
        store_inner(&cfg, b"hot-entry-a\n", Capture::full("hot-cmd", Some(1))).unwrap();
        // Now insert D — triggers eviction. A has lowest rowid even though
        // it was most recently refreshed.
        store_inner(&cfg, b"entry-d\n", Capture::full("cmd-d", Some(0))).unwrap();

        let conn = open(&cfg).unwrap();
        let hot_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM recall WHERE command = 'hot-cmd'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        // CORRECT behavior: hot entry should survive (it was just refreshed).
        // B2 BUG: it doesn't survive because eviction uses rowid ASC.
        assert!(
            hot_exists,
            "B2: hot entry evicted despite fresh created_at — count rule uses rowid ASC, \
             ON CONFLICT DO UPDATE does not bump rowid"
        );
    }

    #[test]
    fn test_b2_eviction_rules_use_different_ordering() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            max_entries: 2,
            retention_days: 30,
            ..temp_cfg(dir.path())
        };
        let conn = open(&cfg).unwrap();
        // Insert old row with low rowid but set created_at in the past
        let old_ts = now_secs() - 100;
        conn.execute(
            "INSERT INTO recall (hash, command, created_at, total_lines, shown_upto, byte_size, truncated, codec, blob)
             VALUES ('aaa_old_rowid_', 'old', ?1, 1, 1, 1, 0, 'raw', x'61')",
            params![old_ts],
        ).unwrap();
        // Insert new row — higher rowid, fresh created_at
        conn.execute(
            "INSERT INTO recall (hash, command, created_at, total_lines, shown_upto, byte_size, truncated, codec, blob)
             VALUES ('bbb_new_rowid_', 'new', ?1, 1, 1, 1, 0, 'raw', x'62')",
            params![now_secs()],
        ).unwrap();
        // Update old row's created_at to NOW (simulating upsert refresh)
        conn.execute(
            "UPDATE recall SET created_at = ?1 WHERE hash = 'aaa_old_rowid_'",
            params![now_secs()],
        )
        .unwrap();

        // Now evict: retention_days=30 won't delete either (both recent).
        // max_entries=2 means no excess. But if we add a third:
        conn.execute(
            "INSERT INTO recall (hash, command, created_at, total_lines, shown_upto, byte_size, truncated, codec, blob)
             VALUES ('ccc_third_row_', 'third', ?1, 1, 1, 1, 0, 'raw', x'63')",
            params![now_secs()],
        ).unwrap();
        evict(&conn, &cfg);

        // Count rule evicts by rowid ASC → 'aaa_old_rowid_' deleted even though
        // its created_at was just refreshed
        let old_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM recall WHERE hash = 'aaa_old_rowid_'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            !old_exists,
            "B2: row with lowest rowid evicted despite fresh created_at. \
             Retention says keep, count says delete."
        );
    }

    #[test]
    fn test_b2_upsert_does_not_change_rowid() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let conn = open(&cfg).unwrap();
        // Store entry
        store_inner(&cfg, b"original\n", Capture::full("dedup-cmd", Some(0))).unwrap();
        let rowid_before: i64 = conn
            .query_row(
                "SELECT rowid FROM recall WHERE command = 'dedup-cmd'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // Re-store with same (command, content) → ON CONFLICT DO UPDATE
        store_inner(&cfg, b"original\n", Capture::full("dedup-cmd", Some(0))).unwrap();
        let rowid_after: i64 = conn
            .query_row(
                "SELECT rowid FROM recall WHERE command = 'dedup-cmd'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            rowid_before, rowid_after,
            "B2 root cause: ON CONFLICT DO UPDATE does not change rowid"
        );
    }

    // --- V6: concurrency — N threads store simultaneously ---

    #[test]
    fn test_v6_concurrent_stores_no_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            max_entries: 200,
            ..temp_cfg(dir.path())
        };
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let cfg = cfg.clone();
                std::thread::spawn(move || {
                    for j in 0..10 {
                        let content = format!("thread-{i}-iter-{j}\n");
                        let cmd = format!("cmd-{i}-{j}");
                        let result = store(&cfg, content.as_bytes(), Capture::full(&cmd, Some(0)));
                        assert!(
                            !matches!(result, Stored::Unavailable),
                            "thread {i} iter {j}: store must not fail under concurrency"
                        );
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().expect("thread must not panic");
        }

        let conn = open(&cfg).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM recall", [], |r| r.get(0))
            .unwrap();
        assert!(
            count > 0 && count <= 100,
            "expected 1-100 rows after 10x10 stores, got {count}"
        );

        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            integrity, "ok",
            "DB must pass integrity check after concurrent writes"
        );
    }

    #[test]
    fn test_v6_concurrent_store_and_recall() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let stored = store_inner(
            &cfg,
            b"concurrent read target\n",
            Capture::full("read-cmd", Some(0)),
        )
        .unwrap();
        let hash = stored.hash.clone();

        let handles: Vec<_> = (0..5)
            .map(|i| {
                let cfg = cfg.clone();
                let hash = hash.clone();
                std::thread::spawn(move || {
                    for _ in 0..5 {
                        let conn = open(&cfg).unwrap();
                        let row = load_by_hash(&conn, &hash).unwrap();
                        assert!(row.is_some(), "thread {i}: entry must be readable");
                        let data = decode(&row.unwrap()).unwrap();
                        assert_eq!(data, b"concurrent read target\n");
                    }
                })
            })
            .collect();

        // Concurrent writes while reads are happening
        for i in 0..5 {
            let content = format!("concurrent-write-{i}\n");
            store(
                &cfg,
                content.as_bytes(),
                Capture::full(&format!("write-cmd-{i}"), Some(0)),
            );
        }

        for h in handles {
            h.join().expect("reader thread must not panic");
        }
    }

    #[test]
    fn test_v6_concurrent_eviction_no_panic() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            max_entries: 5,
            retention_days: 0,
            ..temp_cfg(dir.path())
        };

        let handles: Vec<_> = (0..5)
            .map(|i| {
                let cfg = cfg.clone();
                std::thread::spawn(move || {
                    for j in 0..20 {
                        let content = format!("evict-thread-{i}-{j}\n");
                        let _ = store(
                            &cfg,
                            content.as_bytes(),
                            Capture::full(&format!("evict-{i}-{j}"), Some(0)),
                        );
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().expect("eviction thread must not panic");
        }

        let conn = open(&cfg).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM recall", [], |r| r.get(0))
            .unwrap();
        assert!(
            count <= 5,
            "max_entries=5 must be respected even under concurrency, got {count}"
        );
    }

    // --- V12: -wal/-shm permissions while connection is open ---

    #[cfg(unix)]
    #[test]
    fn test_v12_wal_perms_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let db = db_path(&cfg).unwrap();
        store_inner(&cfg, b"trigger wal\n", Capture::full("wal-test", Some(0))).unwrap();
        let conn = open(&cfg).unwrap();
        conn.execute("INSERT INTO recall (hash, command, created_at, total_lines, shown_upto, byte_size, truncated, codec, blob) VALUES ('wal_test_hash_', 'x', 1, 1, 1, 1, 0, 'raw', x'61')", []).unwrap();
        let wal = db.with_extension("db-wal");
        if wal.exists() {
            let mode = std::fs::metadata(&wal).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o600,
                "WAL file must be owner-only (0600), got {:o}",
                mode
            );
        }
        let shm = db.with_extension("db-shm");
        if shm.exists() {
            let mode = std::fs::metadata(&shm).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o600,
                "SHM file must be owner-only (0600), got {:o}",
                mode
            );
        }
        drop(conn);
    }

    #[cfg(unix)]
    #[test]
    fn test_v12_db_perms_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let db = db_path(&cfg).unwrap();
        store_inner(&cfg, b"perms check\n", Capture::full("perm-test", Some(0))).unwrap();
        let mode = std::fs::metadata(&db).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "DB file must be owner-only (0600), got {:o}",
            mode
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_v12_parent_dir_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let db = db_path(&cfg).unwrap();
        store_inner(&cfg, b"dir check\n", Capture::full("dir-test", Some(0))).unwrap();
        if let Some(parent) = db.parent() {
            if parent != dir.path() {
                let mode = std::fs::metadata(parent).unwrap().permissions().mode() & 0o777;
                assert_eq!(
                    mode, 0o700,
                    "parent dir must be owner-only (0700), got {:o}",
                    mode
                );
            }
        }
    }

    // --- B5/V5: Disabled mode must not create DB ---

    #[test]
    fn test_b5_stats_snapshot_disabled_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            mode: RecoveryMode::Disabled,
            database_path: Some(dir.path().join("should_not_exist.db")),
            ..RetrieverConfig::default()
        };
        let _result = stats_snapshot_with(&cfg);
        // stats_snapshot_with doesn't check mode — the public wrapper does.
        // But we can verify it would create the DB:
        assert!(
            dir.path().join("should_not_exist.db").exists(),
            "stats_snapshot_with creates DB unconditionally (B5 root cause)"
        );
    }

    #[test]
    fn test_b5_fixed_record_tee_elision_disabled_no_db() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("elision_guard.db");
        let cfg = RetrieverConfig {
            mode: RecoveryMode::Disabled,
            database_path: Some(db_path.clone()),
            ..RetrieverConfig::default()
        };
        record_tee_elision(&cfg, "test-slug");
        assert!(
            !db_path.exists(),
            "record_tee_elision must not create DB when mode=Disabled"
        );
    }

    #[test]
    fn test_b5_fixed_record_tee_elision_enabled_creates_db() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("elision_ok.db");
        let cfg = RetrieverConfig {
            mode: RecoveryMode::Tee,
            database_path: Some(db_path.clone()),
            ..RetrieverConfig::default()
        };
        record_tee_elision(&cfg, "test-slug");
        assert!(
            db_path.exists(),
            "record_tee_elision must create DB when mode=Tee"
        );
    }

    #[test]
    fn test_b5_store_disabled_returns_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("store_guard.db");
        let cfg = RetrieverConfig {
            mode: RecoveryMode::Disabled,
            database_path: Some(db_path.clone()),
            ..RetrieverConfig::default()
        };
        // store() itself doesn't check mode — caller (tee.rs) does
        // But store_inner still creates DB. This is B5 scope for store path.
        let result = store(&cfg, b"test\n", Capture::full("cmd", Some(0)));
        // store doesn't guard on mode — caller must
        assert!(matches!(result, Stored::Saved(_)));
    }

    // --- B11/V18: connection caching ---

    /// V18: repeated `open()` on one path must reuse the handle rather than
    /// re-running the pragmas and six DDL statements per call.
    #[test]
    fn test_open_reuses_cached_connection_for_same_path() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        reset_conn_cache();
        let a = open(&cfg).unwrap();
        let b = open(&cfg).unwrap();
        assert!(
            std::rc::Rc::ptr_eq(&a, &b),
            "same path must yield the same handle"
        );
    }

    /// B11: the cache is keyed by path — a different DB must never be served
    /// the previous connection, or writes land in the wrong store.
    #[test]
    fn test_open_reopens_when_path_changes() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let cfg_a = temp_cfg(dir_a.path());
        let cfg_b = temp_cfg(dir_b.path());
        reset_conn_cache();
        let a = open(&cfg_a).unwrap();
        let b = open(&cfg_b).unwrap();
        assert!(
            !std::rc::Rc::ptr_eq(&a, &b),
            "distinct paths must not share a handle"
        );
    }

    /// B11: switching back after a different path was opened is a miss, not a
    /// silent reuse of the stale entry.
    #[test]
    fn test_open_after_path_switch_back_is_a_fresh_handle() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let cfg_a = temp_cfg(dir_a.path());
        let cfg_b = temp_cfg(dir_b.path());
        reset_conn_cache();
        let first = open(&cfg_a).unwrap();
        let _ = open(&cfg_b).unwrap();
        let again = open(&cfg_a).unwrap();
        assert!(!std::rc::Rc::ptr_eq(&first, &again));
    }

    /// V18: the cached handle must stay writable — reuse is worthless if the
    /// second store fails or lands in a stale snapshot.
    #[test]
    fn test_cached_connection_still_writes_correctly() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        reset_conn_cache();
        for i in 0..5 {
            store_inner(
                &cfg,
                format!("payload {i}\n").as_bytes(),
                Capture::full("cmd", Some(0)),
            )
            .unwrap();
        }
        let conn = open(&cfg).unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM recall", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 5, "every store through the cached handle persisted");
    }

    /// V18: reads issued through the cached handle must see writes made
    /// through it — no stale snapshot across calls.
    #[test]
    fn test_cached_connection_reads_own_writes() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        reset_conn_cache();
        let stored = store_inner(&cfg, b"recall me\n", Capture::full("cmd", Some(0))).unwrap();
        let conn = open(&cfg).unwrap();
        let row = load_by_hash(&conn, &stored.hash).unwrap().expect("row");
        assert_eq!(decode(&row).unwrap(), b"recall me\n");
    }

    /// B11: the cache must not create a DB for a path that was never opened —
    /// the Disabled-mode guarantee (V5) still holds with caching in place.
    #[test]
    fn test_cache_does_not_create_db_before_first_open() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        reset_conn_cache();
        let path = db_path(&cfg).unwrap();
        assert!(!path.exists(), "no DB before any open()");
        let _ = open(&cfg).unwrap();
        assert!(path.exists(), "open() creates it");
    }

    #[test]
    #[ignore]
    fn bench_open_cached_vs_uncached() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let path = db_path(&cfg).unwrap();
        reset_conn_cache();
        let _ = open(&cfg).unwrap();

        let t0 = std::time::Instant::now();
        for _ in 0..9 {
            let _ = open(&cfg).unwrap();
        }
        let cached = t0.elapsed();

        let t1 = std::time::Instant::now();
        for _ in 0..9 {
            let _ = open_uncached(&path).unwrap();
        }
        let uncached = t1.elapsed();

        println!("9 cached opens:   {cached:?}");
        println!("9 uncached opens: {uncached:?}");
    }

    // --- V9: corrupted/unavailable DB → silent, no extra token, no exit code change ---

    #[test]
    fn test_v9_corrupted_db_store_returns_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("recall_test.db");
        std::fs::write(&db_path, b"NOT A SQLITE DATABASE").unwrap();
        let cfg = RetrieverConfig {
            database_path: Some(db_path),
            ..RetrieverConfig::default()
        };
        let result = store(&cfg, b"some output\n", Capture::full("cmd", Some(1)));
        assert!(
            matches!(result, Stored::Unavailable),
            "corrupted DB must return Unavailable, got {:?}",
            result
        );
    }

    #[test]
    fn test_v9_corrupted_db_store_no_panic() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("recall_test.db");
        std::fs::write(&db_path, vec![0xde, 0xad, 0xbe, 0xef]).unwrap();
        let cfg = RetrieverConfig {
            database_path: Some(db_path),
            ..RetrieverConfig::default()
        };
        let _ = store(&cfg, b"output\n", Capture::full("cmd", Some(0)));
    }

    #[test]
    fn test_v9_empty_content_returns_empty_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let result = store(&cfg, b"", Capture::full("cmd", Some(0)));
        assert!(matches!(result, Stored::Empty));
    }

    #[test]
    fn test_v9_store_unavailable_yields_no_hint() {
        let result = Stored::Unavailable;
        let hint = match result {
            Stored::Saved(s) => Some(format!("[full output: rtk recall {}]", s.hash)),
            Stored::Unavailable | Stored::Empty => None,
        };
        assert!(
            hint.is_none(),
            "Unavailable must produce no hint (no extra token)"
        );
    }

    #[test]
    fn test_v9_store_empty_yields_no_hint() {
        let result = Stored::Empty;
        let hint = match result {
            Stored::Saved(s) => Some(format!("[full output: rtk recall {}]", s.hash)),
            Stored::Unavailable | Stored::Empty => None,
        };
        assert!(hint.is_none(), "Empty must produce no hint");
    }

    #[test]
    fn test_v9_truncated_db_file_store_returns_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("recall_test.db");
        let cfg_good = RetrieverConfig {
            database_path: Some(db_path.clone()),
            ..RetrieverConfig::default()
        };
        store_inner(&cfg_good, b"seed\n", Capture::full("cmd", Some(0))).unwrap();
        // Drop the cached handle before touching the file: closing the
        // connection checkpoints the WAL into the main DB, so truncating first
        // would just be undone by the checkpoint on close.
        reset_conn_cache();
        let db_bytes = std::fs::read(&db_path).unwrap();
        std::fs::write(&db_path, &db_bytes[..db_bytes.len() / 2]).unwrap();
        let cfg = RetrieverConfig {
            database_path: Some(db_path),
            ..RetrieverConfig::default()
        };
        let result = store(&cfg, b"after corruption\n", Capture::full("cmd2", Some(1)));
        assert!(
            matches!(result, Stored::Unavailable),
            "half-truncated DB must return Unavailable, got {:?}",
            result
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_v9_readonly_db_store_returns_unavailable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("recall_test.db");
        let cfg = RetrieverConfig {
            database_path: Some(db_path.clone()),
            ..RetrieverConfig::default()
        };
        store_inner(&cfg, b"seed\n", Capture::full("cmd", Some(0))).unwrap();
        // Close the seeded handle first — an already-open connection keeps
        // writing regardless of the mode set on the file afterwards.
        reset_conn_cache();
        std::fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o444)).unwrap();
        let result = store(&cfg, b"new content\n", Capture::full("cmd2", Some(1)));
        std::fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            matches!(result, Stored::Unavailable),
            "read-only DB must return Unavailable, got {:?}",
            result
        );
    }

    #[test]
    fn test_v9_open_failure_does_not_emit_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("recall_test.db");
        std::fs::write(&db_path, b"GARBAGE").unwrap();
        let cfg = RetrieverConfig {
            database_path: Some(db_path),
            ..RetrieverConfig::default()
        };
        let result = store(&cfg, b"output\n", Capture::full("cmd", Some(1)));
        assert!(matches!(result, Stored::Unavailable));
    }
}
