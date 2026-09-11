//! Content-addressed recall store backing `rtk recall`.

use super::constants::RECALL_DB;
use crate::core::config::Config;
use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
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
    pub compression: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database_path: Option<PathBuf>,
    pub tee_max_files: usize,
    pub tee_max_file_size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tee_directory: Option<PathBuf>,
    /// Legacy `[tee] mode = "always"`: archive successful runs too. Tee mode only.
    pub tee_on_success: bool,
}

impl Default for RetrieverConfig {
    fn default() -> Self {
        Self {
            mode: RecoveryMode::Sqlite,
            max_entry_bytes: DEFAULT_MAX_ENTRY_BYTES,
            max_entries: DEFAULT_MAX_ENTRIES,
            retention_days: DEFAULT_RETENTION_DAYS,
            compression: true,
            database_path: None,
            tee_max_files: DEFAULT_TEE_MAX_FILES,
            tee_max_file_size: DEFAULT_TEE_MAX_FILE_SIZE,
            tee_directory: None,
            tee_on_success: false,
        }
    }
}

pub struct StoredRef {
    pub hash: String,
    pub hidden_lines: usize,
}

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
    use regex::bytes::Regex;
    let re = Regex::new(pattern)
        .or_else(|_| Regex::new(&regex::escape(pattern)))
        .ok();
    let Some(re) = re else {
        return input.to_vec();
    };
    let mut out = Vec::new();
    for line in input.split_inclusive(|&b| b == b'\n') {
        let body = line.strip_suffix(b"\n").unwrap_or(line);
        if re.is_match(body) {
            out.extend_from_slice(line);
        }
    }
    out
}

fn gzip(data: &[u8]) -> Result<Vec<u8>> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data).context("gzip write")?;
    enc.finish().context("gzip finish")
}

fn gunzip(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    GzDecoder::new(data)
        .read_to_end(&mut out)
        .context("gunzip")?;
    Ok(out)
}

fn db_path(cfg: &RetrieverConfig) -> Result<PathBuf> {
    if let Ok(p) = std::env::var("RTK_RECALL_DB") {
        return Ok(PathBuf::from(p));
    }
    if let Some(ref p) = cfg.database_path {
        return Ok(p.clone());
    }
    // A test that names no store must never reach the developer's own: its rows
    // would ship as real usage through the telemetry ping.
    #[cfg(test)]
    let base = std::env::temp_dir().join(format!("rtk-test-store-{}", std::process::id()));
    #[cfg(not(test))]
    let base = dirs::data_local_dir()
        .ok_or_else(|| anyhow::anyhow!("no local data directory available"))?
        .join(super::constants::RTK_DATA_DIR);
    Ok(base.join(RECALL_DB))
}

fn open(cfg: &RetrieverConfig) -> Result<Connection> {
    let path = db_path(cfg)?;
    if let Some(parent) = path.parent() {
        let _ = crate::core::utils::create_private_dir(parent);
    }
    crate::core::utils::open_private(std::fs::OpenOptions::new().write(true).create(true), &path)
        .with_context(|| format!("pre-create private recall DB: {}", path.display()))?;
    let conn = Connection::open(&path).with_context(|| format!("open {}", path.display()))?;
    // best-effort: NFS / read-only filesystems may reject WAL
    let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;");
    init_schema(&conn)?;
    Ok(conn)
}

thread_local! {
    static CONN_CACHE: std::cell::RefCell<Option<(PathBuf, Connection)>> =
        const { std::cell::RefCell::new(None) };
}

fn with_open<T>(cfg: &RetrieverConfig, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    let path = db_path(cfg)?;
    CONN_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let reuse = matches!(&*cache, Some((p, _)) if *p == path && path.exists());
        if !reuse {
            *cache = Some((path.clone(), open(cfg)?));
        }
        let (_, conn) = cache.as_ref().expect("cache populated above");
        f(conn)
    })
}

fn open_existing(cfg: &RetrieverConfig) -> Result<Option<Connection>> {
    let path = db_path(cfg)?;
    if !path.exists() {
        return Ok(None);
    }
    let conn = Connection::open(&path).with_context(|| format!("open {}", path.display()))?;
    let _ = conn.execute_batch("PRAGMA busy_timeout=5000;");
    init_schema(&conn)?;
    Ok(Some(conn))
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS recall (
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
        );",
    )
    .context("init recall schema")?;
    let _ = conn.execute(
        "ALTER TABLE recall ADD COLUMN recalled INTEGER NOT NULL DEFAULT 0",
        [],
    );
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

pub struct RecallStat {
    pub slug: String,
    pub mode: String,
    pub elisions: i64,
    pub recalls: i64,
}

fn stats_snapshot_with(cfg: &RetrieverConfig) -> Result<Vec<RecallStat>> {
    let Some(conn) = open_existing(cfg)? else {
        return Ok(Vec::new());
    };
    let mut stmt = conn.prepare("SELECT slug, mode, elisions, recalls FROM recall_stats")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })?;
    let mut agg: std::collections::BTreeMap<(String, String), (i64, i64)> =
        std::collections::BTreeMap::new();
    for row in rows.filter_map(|r| r.ok()) {
        let (slug, mode, elisions, recalls) = row;
        let entry = agg.entry((mode, stat_key(&slug))).or_insert((0, 0));
        entry.0 += elisions;
        entry.1 += recalls;
    }
    let mut stats: Vec<RecallStat> = agg
        .into_iter()
        .map(|((mode, slug), (elisions, recalls))| RecallStat {
            slug,
            mode,
            elisions,
            recalls,
        })
        .collect();
    stats.sort_by(|a, b| {
        a.mode
            .cmp(&b.mode)
            .then(b.elisions.cmp(&a.elisions))
            .then(a.slug.cmp(&b.slug))
    });
    Ok(stats)
}

pub fn stats_snapshot() -> Result<Vec<RecallStat>> {
    let cfg = Config::load().unwrap_or_default().retriever;
    stats_snapshot_with(&cfg)
}

pub fn reset_stats() -> Result<()> {
    let cfg = Config::load().unwrap_or_default().retriever;
    reset_stats_with(&cfg)
}

fn reset_stats_with(cfg: &RetrieverConfig) -> Result<()> {
    let Some(conn) = open_existing(cfg)? else {
        return Ok(());
    };
    conn.execute_batch(
        "BEGIN;
         DELETE FROM recall_stats;
         DELETE FROM tee_reads;
         UPDATE recall SET recalled = 0;
         COMMIT;",
    )
    .context("reset recall stats")?;
    Ok(())
}

pub fn record_tee_elision(cfg: &RetrieverConfig, slug: &str) {
    if cfg.mode == RecoveryMode::Disabled {
        return;
    }
    // Never create the store from the tee path: choosing tee must leave no
    // sqlite artifact behind. Stats are recorded only into an existing store.
    if let Ok(Some(conn)) = open_existing(cfg) {
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

pub(crate) fn recovery_disabled_by_env() -> bool {
    matches!(std::env::var("RTK_RECALL").ok().as_deref(), Some("0"))
        || matches!(std::env::var("RTK_TEE").ok().as_deref(), Some("0"))
}

fn record_tee_recall_with(cfg: &RetrieverConfig, slug: &str, path: &str) {
    if recovery_disabled_by_env() || cfg.mode == RecoveryMode::Disabled {
        return;
    }
    if let Ok(Some(conn)) = open_existing(cfg) {
        record_tee_recall_on(&conn, slug, path);
    }
}

pub fn record_tee_recall(slug: &str, path: &str) {
    let cfg = Config::load().unwrap_or_default().retriever;
    record_tee_recall_with(&cfg, slug, path);
}

fn evict(conn: &Connection, cfg: &RetrieverConfig, keep_hash: &str) {
    if cfg.retention_days > 0 {
        let cutoff = now_secs() - (cfg.retention_days as i64) * 86_400;
        let _ = conn.execute(
            "DELETE FROM recall WHERE created_at < ?1 AND hash != ?2",
            params![cutoff, keep_hash],
        );
    }
    if cfg.max_entries > 0 {
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM recall", [], |r| r.get(0))
            .unwrap_or(0);
        let excess = count - cfg.max_entries as i64;
        if excess > 0 {
            let _ = conn.execute(
                "DELETE FROM recall WHERE rowid IN (
                    SELECT rowid FROM recall WHERE hash != ?2
                    ORDER BY created_at ASC, rowid ASC LIMIT ?1
                )",
                params![excess, keep_hash],
            );
        }
    }
}

pub fn store(
    cfg: &RetrieverConfig,
    content: &[u8],
    command: &str,
    exit_code: Option<i32>,
    shown_upto: usize,
) -> Stored {
    if content.is_empty() {
        return Stored::Empty;
    }
    match store_inner(cfg, content, command, exit_code, shown_upto.max(1)) {
        Ok(r) => Stored::Saved(r),
        Err(_) => Stored::Unavailable,
    }
}

fn store_inner(
    cfg: &RetrieverConfig,
    content: &[u8],
    command: &str,
    exit_code: Option<i32>,
    shown_upto: usize,
) -> Result<StoredRef> {
    let cap = cfg.max_entry_bytes;
    let cut_at_line = |slice: &[u8]| {
        slice[..cap]
            .iter()
            .rposition(|&b| b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(cap)
    };
    // Over the cap the store keeps the hidden tail rather than the prefix the
    // agent already saw, so the hint can never promise more than recall returns.
    let (payload, stored_shown_upto, truncated) = if content.len() <= cap {
        (content, shown_upto, false)
    } else if shown_upto > 1 {
        let hidden = slice_from_line(content, shown_upto);
        if hidden.len() > cap {
            (&hidden[..cut_at_line(hidden)], 1, true)
        } else {
            (hidden, 1, true)
        }
    } else {
        (&content[..cut_at_line(content)], 1, true)
    };
    let total_lines = count_lines(payload);
    let shown_upto = stored_shown_upto;
    let hash = content_hash(command, content);
    let (blob, codec): (Vec<u8>, &str) = if cfg.compression {
        match gzip(payload) {
            Ok(z) => (z, "gzip"),
            Err(_) => (payload.to_vec(), "raw"),
        }
    } else {
        (payload.to_vec(), "raw")
    };
    let cwd = std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().into_owned());

    with_open(cfg, |conn| {
        conn.execute(
        "INSERT INTO recall
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
             blob = excluded.blob",
        params![
            hash,
            command,
            cwd,
            exit_code,
            now_secs(),
            total_lines as i64,
            shown_upto as i64,
            content.len() as i64,
            truncated as i64,
            codec,
            blob
        ],
    )
    .context("insert recall row")?;
        bump_stat(conn, command, "sqlite", "elisions");
        evict(conn, cfg, &hash);

        Ok(StoredRef {
            hash: hash.clone(),
            hidden_lines: total_lines.saturating_sub(shown_upto.saturating_sub(1)),
        })
    })
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
    let exact = format!("SELECT {SELECT_COLS} FROM recall WHERE hash = ?1");
    if let Some(row) = conn.query_row(&exact, params![hash], map_row).optional()? {
        return Ok(Some(row));
    }
    let mut stmt = conn.prepare(
        "SELECT hash FROM recall WHERE substr(hash, 1, length(?1)) = ?1 ORDER BY hash ASC",
    )?;
    let candidates: Vec<String> = stmt
        .query_map(params![hash], |r| r.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    match candidates.as_slice() {
        [] => Ok(None),
        [only] => {
            let sql = format!("SELECT {SELECT_COLS} FROM recall WHERE hash = ?1");
            Ok(conn.query_row(&sql, params![only], map_row).optional()?)
        }
        many => anyhow::bail!(
            "ambiguous hash prefix '{hash}': matches {}",
            many.join(", ")
        ),
    }
}

fn decode(row: &Row) -> Result<Vec<u8>> {
    match row.codec.as_str() {
        "gzip" => gunzip(&row.blob),
        _ => Ok(row.blob.clone()),
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

pub fn run_recall(args: RecallArgs) -> Result<i32> {
    if args.hash.is_none() && !args.list {
        eprintln!("rtk recall: provide a <hash> (from a recovery hint) or --list");
        return Ok(2);
    }
    let cfg = Config::load().unwrap_or_default().retriever;
    let conn = match open_existing(&cfg) {
        Ok(Some(c)) => c,
        Ok(None) => {
            if args.list {
                println!("(no recall entries)");
                return Ok(0);
            }
            eprintln!("rtk recall: no matching entry (try `rtk recall --list`)");
            return Ok(1);
        }
        Err(e) => {
            eprintln!("rtk recall: store unavailable: {e}");
            return Ok(1);
        }
    };

    if args.list {
        return list_entries(&conn);
    }

    // `args.list` returned above and the usage guard rejected a missing hash,
    // so a hash is present here.
    let Some(hash) = args.hash.as_ref() else {
        return Ok(2);
    };
    let row = match load_by_hash(&conn, hash) {
        Ok(row) => row,
        Err(e) => {
            eprintln!("rtk recall: {e}");
            return Ok(1);
        }
    };

    let Some(row) = row else {
        eprintln!("rtk recall: no matching entry (try `rtk recall --list`)");
        return Ok(1);
    };

    let full = decode(&row)?;
    let sliced: Vec<u8> = if args.full {
        full.clone()
    } else if let Some(n) = args.from {
        slice_from_line(&full, n).to_vec()
    } else if let Some(n) = args.lines {
        slice_first_lines(&full, n).to_vec()
    } else {
        slice_from_line(&full, row.shown_upto).to_vec()
    };
    let out = match args.grep {
        Some(pat) => {
            if regex::bytes::Regex::new(pat).is_err() {
                eprintln!(
                    "rtk recall: note: --grep pattern is not a valid regex, matching it literally"
                );
            }
            grep_bytes(&sliced, pat)
        }
        None => sliced,
    };

    if out.is_empty() && row.truncated && args.grep.is_none() {
        eprintln!(
            "rtk recall: the requested lines were lost — this entry was stored truncated at the cap in force at the time"
        );
        return Ok(1);
    }
    let stdout = std::io::stdout();
    let _ = stdout.lock().write_all(&out);
    mark_recalled(&conn, &row.hash, &row.command);

    if row.truncated {
        eprintln!(
            "rtk recall: note: this entry was stored truncated (output exceeded the cap in force at the time)"
        );
    }
    Ok(0)
}

fn list_entries(conn: &Connection) -> Result<i32> {
    let mut stmt = conn.prepare(
        "SELECT hash, command, total_lines, shown_upto, exit_code, truncated, byte_size \
         FROM recall ORDER BY created_at DESC LIMIT 50",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, Option<i64>>(4)?,
            r.get::<_, i64>(5)?,
            r.get::<_, i64>(6)?,
        ))
    })?;

    println!(
        "{:<14} {:<26} {:>7} {:>7} {:>9} {:>5} TRUNC",
        "HASH", "COMMAND", "LINES", "HIDDEN", "ORIG", "EXIT"
    );
    let mut n = 0;
    for row in rows {
        let (hash, command, total, shown, exit, truncated, byte_size) = row?;
        let hidden = total.saturating_sub(shown.saturating_sub(1)).max(0);
        let cmd = if command.chars().count() > 26 {
            let head: String = command.chars().take(25).collect();
            format!("{head}…")
        } else {
            command
        };
        println!(
            "{:<14} {:<26} {:>7} {:>7} {:>9} {:>5} {}",
            hash,
            cmd,
            total,
            hidden,
            crate::core::utils::format_tokens(byte_size.max(0) as usize),
            exit.map(|e| e.to_string()).unwrap_or_else(|| "-".into()),
            if truncated != 0 { "yes" } else { "" }
        );
        n += 1;
    }
    if n == 0 {
        println!("(no recall entries)");
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_cfg(dir: &std::path::Path) -> RetrieverConfig {
        RetrieverConfig {
            database_path: Some(dir.join("recall_test.db")),
            ..RetrieverConfig::default()
        }
    }

    #[test]
    fn test_reset_stats_clears_counters_and_lets_reads_count_again() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let stored = store_inner(&cfg, b"a\nb\nc\n", "vitest", Some(1), 1).unwrap();
        let conn = open(&cfg).unwrap();
        mark_recalled(&conn, &stored.hash, "vitest");
        record_tee_recall_on(&conn, "vitest", "/tmp/vitest.log");
        assert!(!stats_snapshot_with(&cfg).unwrap().is_empty());

        drop(conn);
        reset_stats_with(&cfg).unwrap();
        assert!(
            stats_snapshot_with(&cfg).unwrap().is_empty(),
            "reset must zero every counter"
        );

        let conn = open(&cfg).unwrap();
        mark_recalled(&conn, &stored.hash, "vitest");
        let stats = stats_snapshot_with(&cfg).unwrap();
        let s = stats
            .iter()
            .find(|s| s.slug == "vitest" && s.mode == "sqlite")
            .expect("a read after the reset counts again");
        assert_eq!(s.recalls, 1);
    }

    #[test]
    fn test_reset_stats_keeps_the_stored_output_recoverable() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let stored = store_inner(&cfg, b"a\nb\nc\n", "vitest", Some(1), 1).unwrap();
        reset_stats_with(&cfg).unwrap();
        let conn = open(&cfg).unwrap();
        assert!(
            load_by_hash(&conn, &stored.hash).unwrap().is_some(),
            "reset clears counters, never the recoverable output"
        );
    }

    #[test]
    fn test_reset_stats_never_creates_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        reset_stats_with(&cfg).unwrap();
        assert!(!cfg.database_path.as_ref().unwrap().exists());
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
    fn test_grep_match_all_roundtrips_byte_exact() {
        let input = b"a\nb\nc\n".to_vec();
        assert_eq!(grep_bytes(&input, "^"), input);
        let no_trailing = b"a\nb\nno-eol".to_vec();
        assert_eq!(grep_bytes(&no_trailing, "^"), no_trailing);
    }

    #[test]
    fn test_grep_bytes() {
        let input = b"alpha\nbeta\ngamma\n";
        assert_eq!(grep_bytes(input, "et"), b"beta\n");
        assert_eq!(grep_bytes(input, "^g"), b"gamma\n");
    }

    #[test]
    fn test_gzip_roundtrip_arbitrary_bytes() {
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
            let z = gzip(&c).expect("gzip");
            assert_eq!(gunzip(&z).expect("gunzip"), c, "gzip must be byte-exact");
        }
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

        let stored = store_inner(&cfg, &nasty, "nasty-cmd", Some(0), 1).expect("store");
        let conn = open(&cfg).unwrap();
        let row = load_by_hash(&conn, &stored.hash).unwrap().expect("row");
        assert_eq!(
            decode(&row).unwrap(),
            nasty,
            "stored bytes must round-trip exactly"
        );
    }

    #[test]
    fn test_store_fetch_raw_codec() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            compression: false,
            ..temp_cfg(dir.path())
        };
        let data = vec![0u8, 1, 2, 255, b'\n', b'x'];
        let stored = store_inner(&cfg, &data, "c", Some(0), 1).unwrap();
        let conn = open(&cfg).unwrap();
        let row = load_by_hash(&conn, &stored.hash).unwrap().unwrap();
        assert_eq!(row.codec, "raw");
        assert_eq!(decode(&row).unwrap(), data);
    }

    #[test]
    fn test_delta_recall_returns_only_missed() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let content = b"i1\ni2\ni3\ni4\ni5\n";
        let stored = store_inner(&cfg, content, "list", Some(0), 3).unwrap();
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
        let stored = store_inner(&cfg, &big, "big", Some(0), 1).unwrap();
        let conn = open(&cfg).unwrap();
        let row = load_by_hash(&conn, &stored.hash).unwrap().unwrap();
        assert!(row.truncated);
        assert_eq!(decode(&row).unwrap().len(), 10);
    }

    #[test]
    fn test_hidden_lines_equal_recallable_lines_under_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            max_entry_bytes: 200,
            ..temp_cfg(dir.path())
        };
        let content: Vec<u8> = (0..100)
            .flat_map(|i| format!("line {i:03} padding\n").into_bytes())
            .collect();
        let stored = store_inner(&cfg, &content, "cmd", None, 5).unwrap();
        let conn = open(&cfg).unwrap();
        let row = load_by_hash(&conn, &stored.hash).unwrap().unwrap();
        let recallable = count_lines(slice_from_line(&decode(&row).unwrap(), row.shown_upto));
        assert_eq!(
            stored.hidden_lines, recallable,
            "the hint must promise exactly what recall can return"
        );
    }

    #[test]
    fn test_truncation_prefers_hidden_over_already_shown() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            max_entry_bytes: 400,
            ..temp_cfg(dir.path())
        };
        let content: Vec<u8> = (0..200)
            .flat_map(|i| format!("line {i:03} padding padding\n").into_bytes())
            .collect();
        let stored = store_inner(&cfg, &content, "cmd", None, 150).unwrap();
        assert!(
            stored.hidden_lines > 0,
            "a cap that cannot hold the shown prefix must still store the hidden tail"
        );
        let conn = open(&cfg).unwrap();
        let row = load_by_hash(&conn, &stored.hash).unwrap().unwrap();
        let delta = slice_from_line(&decode(&row).unwrap(), row.shown_upto).to_vec();
        assert_eq!(
            count_lines(&delta),
            stored.hidden_lines,
            "promise must equal what recall returns"
        );
        assert!(
            String::from_utf8_lossy(&delta).starts_with("line 149"),
            "stored window must begin at the first hidden line, got: {}",
            String::from_utf8_lossy(&delta[..30.min(delta.len())])
        );
    }

    #[test]
    fn test_list_reports_true_size_not_just_stored() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            max_entry_bytes: 400,
            ..temp_cfg(dir.path())
        };
        let content: Vec<u8> = (0..300)
            .flat_map(|i| format!("line {i:03} padding padding\n").into_bytes())
            .collect();
        let stored = store_inner(&cfg, &content, "cmd", None, 20).unwrap();
        let conn = open(&cfg).unwrap();
        let byte_size: i64 = conn
            .query_row(
                "SELECT byte_size FROM recall WHERE hash = ?1",
                params![stored.hash],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            byte_size,
            content.len() as i64,
            "byte_size must keep the true original size for --list to surface"
        );
    }

    #[test]
    fn test_tiny_cap_still_yields_a_recallable_hidden_line() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            max_entry_bytes: 40,
            ..temp_cfg(dir.path())
        };
        let content: Vec<u8> = (0..100)
            .flat_map(|i| format!("line {i:03} padding\n").into_bytes())
            .collect();
        let stored = store_inner(&cfg, &content, "cmd", None, 50).unwrap();
        assert!(
            stored.hidden_lines > 0,
            "even a tiny cap keeps hidden content, so callers never see a None hint"
        );
        let conn = open(&cfg).unwrap();
        let row = load_by_hash(&conn, &stored.hash).unwrap().unwrap();
        let delta = slice_from_line(&decode(&row).unwrap(), row.shown_upto).to_vec();
        assert_eq!(count_lines(&delta), stored.hidden_lines);
    }

    #[test]
    fn test_truncation_cuts_at_line_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            max_entry_bytes: 12,
            ..temp_cfg(dir.path())
        };
        let stored = store_inner(&cfg, b"aaaa\nbbbb\ncccc\n", "cmd", Some(0), 1).unwrap();
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
        let stored = store_inner(&cfg, &big, "big", Some(0), 1).unwrap();
        let conn = open(&cfg).unwrap();
        let row = load_by_hash(&conn, &stored.hash).unwrap().unwrap();
        assert!(row.truncated);
        assert_eq!(decode(&row).unwrap().len(), 10);
    }

    #[test]
    fn test_read_paths_never_create_the_db() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let db = dir.path().join("recall_test.db");
        let stats = stats_snapshot_with(&cfg).expect("snapshot on missing db");
        assert!(stats.is_empty());
        assert!(
            !db.exists(),
            "reading stats must never create the database file"
        );
    }

    #[test]
    fn test_stored_hash_always_resolves_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = RetrieverConfig {
            max_entries: 10,
            retention_days: 0,
            ..temp_cfg(dir.path())
        };
        for i in 0..6 {
            store_inner(&cfg, format!("o{i}\n").as_bytes(), "cmd", Some(1), 1).unwrap();
        }
        cfg.max_entries = 3;
        let stored = store_inner(&cfg, b"o0\n", "cmd", Some(1), 1).expect("re-store");
        let conn = open(&cfg).unwrap();
        assert!(
            load_by_hash(&conn, &stored.hash).unwrap().is_some(),
            "a hash returned by store() must resolve immediately"
        );
    }

    #[test]
    fn test_refreshed_entry_outlives_stale_ones() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            max_entries: 5,
            retention_days: 0,
            ..temp_cfg(dir.path())
        };
        let old = store_inner(&cfg, b"first\n", "cmd", Some(1), 1).unwrap();
        for i in 1..5 {
            store_inner(&cfg, format!("mid{i}\n").as_bytes(), "cmd", Some(1), 1).unwrap();
        }
        let conn = open(&cfg).unwrap();
        conn.execute(
            "UPDATE recall SET created_at = created_at - 100 WHERE hash != ?1",
            params![old.hash],
        )
        .unwrap();
        conn.execute(
            "UPDATE recall SET created_at = created_at + 50 WHERE hash = ?1",
            params![old.hash],
        )
        .unwrap();
        drop(conn);
        store_inner(&cfg, b"newest\n", "cmd", Some(1), 1).unwrap();
        let conn = open(&cfg).unwrap();
        assert!(
            load_by_hash(&conn, &old.hash).unwrap().is_some(),
            "a refreshed (recent created_at) entry must not be evicted before stale ones"
        );
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
        evict(&conn, &cfg, "aaa111111111");
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
            store_inner(&cfg, content.as_bytes(), &format!("cmd{i}"), Some(0), 1).unwrap();
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
        let a = store_inner(&cfg, b"same output\n", "cmd", Some(0), 1).unwrap();
        let b = store_inner(&cfg, b"same output\n", "cmd", Some(0), 1).unwrap();
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
        store_inner(&cfg, b"out1\n", "cargo-test", Some(1), 1).unwrap();
        store_inner(&cfg, b"out2\n", "cargo-test", Some(1), 1).unwrap();
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
        let stored = store_inner(&cfg, b"x\ny\n", "vitest", Some(1), 1).unwrap();
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

    #[test]
    fn test_record_tee_recall_respects_kill_switch() {
        let _guard = crate::core::utils::TEST_ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        std::env::set_var("RTK_RECALL", "0");
        record_tee_recall_with(&cfg, "grep", "/tee/1_grep.log");
        std::env::remove_var("RTK_RECALL");
        assert!(
            !dir.path().join("recall_test.db").exists(),
            "RTK_RECALL=0 must prevent any recall.db write from the hook path"
        );
        let disabled = RetrieverConfig {
            mode: RecoveryMode::Disabled,
            ..temp_cfg(dir.path())
        };
        record_tee_recall_with(&disabled, "grep", "/tee/1_grep.log");
        assert!(!dir.path().join("recall_test.db").exists());
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

    #[test]
    fn test_force_path_stores_null_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let stored = store_inner(&cfg, b"trimmed list\n", "docker-images", None, 2).unwrap();
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
        let stored = store_inner(&cfg, b"same fail\n", "cargo_test", Some(1), 1).unwrap();
        let conn = open(&cfg).unwrap();
        mark_recalled(&conn, &stored.hash, "cargo_test");
        store_inner(&cfg, b"same fail\n", "cargo_test", Some(1), 1).unwrap();
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
        let stored = store_inner(&cfg, b"a\nb\nc\n", "gh-prs", Some(0), 2).unwrap();
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
            store_inner(&cfg, format!("o{i}\n").as_bytes(), "find", Some(1), 1).unwrap();
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

        let stored =
            store_inner(&cfg, b"x\ny\n", "vitest", Some(1), 1).expect("store on old schema");
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
        let stored = store_inner(&cfg, b"hello world\n", "cmd", Some(0), 1).unwrap();
        let conn = open(&cfg).unwrap();
        let prefix = &stored.hash[..6];
        assert!(load_by_hash(&conn, prefix).unwrap().is_some());
    }
}
