//! Content-addressed recall store backing `rtk recall`.

use super::constants::RECALL_DB;
use crate::core::config::Config;
use crate::core::user_dirs;
use crate::core::user_env;
use anyhow::{Context, Result};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

mod retention;
use retention::{evict, init_storage_accounting, maintain_store};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_MAX_ENTRY_BYTES: usize = 10 * 1024 * 1024;
const DEFAULT_MAX_ENTRIES: usize = 200;
const DEFAULT_RETENTION_DAYS: u32 = 30;
pub const MIN_FAILURE_BYTES: usize = 500;
#[cfg(test)]
const HASH_HEX_LEN: usize = 12;
const DEFAULT_TEE_MAX_FILES: usize = 20;
// Capture metadata is independent of deduplicated payloads and always bounded.
const MAX_CAPTURE_RECORDS: i64 = 1000;
const DEFAULT_TEE_MAX_FILE_SIZE: usize = 1_048_576;
// Fixed independently of retained output: dynamic command names must not grow
// the statistics indefinitely. This leaves room for both modes' usual filters.
const MAX_STATS_ROWS: usize = 500;

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
    /// Combined encoded blob bytes; zero preserves the legacy unbounded budget.
    pub max_total_bytes: usize,
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
            max_total_bytes: 0,
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
    /// Exact immutable capture reference printed in recovery hints.
    pub reference: String,
    #[cfg(test)]
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

#[cfg(test)]
fn content_hash(command: &str, content: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(command.as_bytes());
    hasher.update([0u8]);
    hasher.update(content);
    let hex = format!("{:x}", hasher.finalize());
    hex[..HASH_HEX_LEN].to_string()
}

/// A separate namespace from legacy source hashes: only retained bytes belong
/// to this identity. Compression is an encoding detail, not part of the key.
fn payload_hash(command: &str, payload: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(command.as_bytes());
    hasher.update([0u8]);
    hasher.update(payload);
    format!("p_{:x}", hasher.finalize())
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
    if let Some(p) = user_env::var("RTK_RECALL_DB") {
        return Ok(PathBuf::from(p));
    }
    if let Some(ref p) = cfg.database_path {
        return Ok(p.clone());
    }
    // A test that names no store must never reach the developer's own: its rows
    // would ship as real usage through the telemetry ping. `user_dirs::data` answers
    // with a scratch directory in a test build.
    let base = super::user_dirs::data()
        .ok_or_else(|| anyhow::anyhow!("no local data directory available"))?;
    Ok(base.join(RECALL_DB))
}

/// Erase the configured recall store without creating a missing database.
pub(crate) fn forget(cfg: &RetrieverConfig) -> Result<()> {
    let path = db_path(cfg)?;
    anyhow::ensure!(
        !path.as_os_str().is_empty(),
        "Refusing an empty recall database path"
    );
    CONN_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if matches!(&*cache, Some((cached, _)) if *cached == path) {
            // Release our WAL handle before unlinking, including on Windows.
            cache.take();
        }
    });
    let paths: Vec<_> = ["", "-wal", "-shm", "-journal"]
        .iter()
        .map(|suffix| {
            let mut name = path.as_os_str().to_os_string();
            name.push(suffix);
            PathBuf::from(name)
        })
        .collect();
    // A configured path may be shared or accidentally name a directory. Never
    // recurse, and do not claim erasure after merely unlinking a symlink.
    for path in &paths {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.is_file() => {}
            Ok(_) => anyhow::bail!(
                "Refusing to delete non-regular recall file: {}",
                path.display()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("Inspect {}", path.display())),
        }
    }
    if path.try_exists()? {
        let conn = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
            .with_context(|| format!("Open recall store for erasure: {}", path.display()))?;
        conn.busy_timeout(std::time::Duration::from_millis(250))?;
        conn.execute_batch("PRAGMA locking_mode=EXCLUSIVE; BEGIN EXCLUSIVE; ROLLBACK;")
            .with_context(|| {
                format!(
                    "Cannot erase recall store {}; stop other RTK commands and retry",
                    path.display()
                )
            })?;
        // Windows needs this handle closed before unlink. This is a quiescence
        // check, not an interprocess erasure barrier: callers must stop writers.
        drop(conn);
    }
    for path in paths {
        // nosemgrep: filesystem-deletion
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("Delete {}", path.display())),
        }
    }
    Ok(())
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
        CREATE TABLE IF NOT EXISTS recall_captures (
            capture_id TEXT PRIMARY KEY,
            invocation_id TEXT,
            hash TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            command TEXT,
            cwd TEXT,
            exit_code INTEGER,
            total_lines INTEGER,
            shown_upto INTEGER,
            byte_size INTEGER,
            truncated INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_recall_captures_hash ON recall_captures(hash);
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
    // Serialize first-open migrations before checking columns. Concurrent RTK
    // processes can otherwise both observe a missing column and race ALTER.
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
        .context("begin recall capture migration")?;
    // Accounting-only captures predate stable hints. Do not reconstruct their
    // snapshot from a mutable legacy row: missing metadata stays unknown.
    let columns: Vec<String> = tx
        .prepare("PRAGMA table_info(recall_captures)")?
        .query_map([], |row| row.get(1))?
        .collect::<rusqlite::Result<_>>()?;
    for (name, kind) in [
        ("command", "TEXT"),
        ("cwd", "TEXT"),
        ("exit_code", "INTEGER"),
        ("total_lines", "INTEGER"),
        ("shown_upto", "INTEGER"),
        ("byte_size", "INTEGER"),
        ("truncated", "INTEGER"),
    ] {
        if !columns.iter().any(|column| column == name) {
            tx.execute(
                &format!("ALTER TABLE recall_captures ADD COLUMN {name} {kind}"),
                [],
            )
            .with_context(|| format!("add recall capture {name}"))?;
        }
    }
    tx.commit().context("commit recall capture migration")?;
    init_stats_retention(conn)?;
    init_storage_accounting(conn)
}

fn init_stats_retention(conn: &Connection) -> Result<()> {
    // The trigger runs only for a new (slug, mode), not an upsert's UPDATE arm.
    // Keeping eviction in the INSERT makes the bound atomic across connections
    // and composes with callers that already own a transaction.
    conn.execute_batch(&format!(
        "CREATE TRIGGER IF NOT EXISTS recall_stats_cap
         AFTER INSERT ON recall_stats
         WHEN (SELECT COUNT(*) FROM recall_stats) > {MAX_STATS_ROWS}
         BEGIN
             DELETE FROM recall_stats WHERE rowid IN (
                 SELECT rowid FROM recall_stats WHERE rowid != NEW.rowid
                 ORDER BY elisions + recalls ASC, rowid ASC
                 LIMIT (SELECT COUNT(*) FROM recall_stats) - {MAX_STATS_ROWS}
             );
         END;
         DELETE FROM recall_stats WHERE rowid IN (
             SELECT rowid FROM recall_stats
             ORDER BY elisions + recalls ASC, rowid ASC
             LIMIT MAX((SELECT COUNT(*) FROM recall_stats) - {MAX_STATS_ROWS}, 0)
         );"
    ))
    .context("bound recall statistics")
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
    matches!(user_env::var("RTK_RECALL").as_deref(), Some("0"))
        || matches!(user_env::var("RTK_TEE").as_deref(), Some("0"))
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

/// One independently recoverable output in a batch. Offsets remain per entry;
/// batching never combines files into an opaque, run-wide recovery blob.
pub struct StoreInput<'a> {
    pub content: &'a [u8],
    pub command: &'a str,
    pub exit_code: Option<i32>,
    pub shown_upto: usize,
}

struct PreparedEntry<'a> {
    input: StoreInput<'a>,
    hash: String,
    capture_id: String,
    total_lines: usize,
    shown_upto: usize,
    truncated: bool,
    codec: &'static str,
    blob: Vec<u8>,
}

fn prepare_entry<'a>(cfg: &RetrieverConfig, input: StoreInput<'a>) -> Result<PreparedEntry<'a>> {
    let content = input.content;
    let shown_upto = input.shown_upto.max(1);
    let cap = cfg.max_entry_bytes;
    let cut_at_line = |slice: &[u8]| {
        slice[..cap]
            .iter()
            .rposition(|&b| b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(cap)
    };
    // Keep the hidden tail when the cap cannot hold the already-shown prefix.
    let (payload, shown_upto, truncated) = if content.len() <= cap {
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
    let (blob, codec) = if cfg.compression {
        match gzip(payload) {
            Ok(z) => (z, "gzip"),
            Err(_) => (payload.to_vec(), "raw"),
        }
    } else {
        (payload.to_vec(), "raw")
    };
    Ok(PreparedEntry {
        hash: payload_hash(input.command, payload),
        capture_id: super::invocation::new_id().context("generate immutable recall capture ID")?,
        total_lines: count_lines(payload),
        input,
        shown_upto,
        truncated,
        codec,
        blob,
    })
}

enum PreparedSlot<'a> {
    Empty,
    Unavailable,
    Entry(PreparedEntry<'a>),
}

impl PreparedSlot<'_> {
    fn entry(&self) -> Option<&PreparedEntry<'_>> {
        match self {
            Self::Entry(entry) => Some(entry),
            Self::Empty | Self::Unavailable => None,
        }
    }

    fn unavailable_result(&self) -> Stored {
        match self {
            Self::Empty => Stored::Empty,
            Self::Unavailable | Self::Entry(_) => Stored::Unavailable,
        }
    }
}

const INSERT_RECALL: &str =
    "INSERT INTO recall
     (hash, command, cwd, exit_code, created_at, total_lines, shown_upto, byte_size, truncated, codec, blob)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
     ON CONFLICT(hash) DO UPDATE SET created_at = excluded.created_at";

/// Store a run's outputs in one transaction, with one retention pass. Only
/// committed rows surviving that pass are eligible for recovery hints.
pub fn store_batch(cfg: &RetrieverConfig, inputs: &[StoreInput<'_>]) -> Vec<Stored> {
    store_batch_inner(cfg, inputs).unwrap_or_else(|_| {
        inputs
            .iter()
            .map(|input| {
                if input.content.is_empty() {
                    Stored::Empty
                } else {
                    Stored::Unavailable
                }
            })
            .collect()
    })
}

fn store_batch_inner(cfg: &RetrieverConfig, inputs: &[StoreInput<'_>]) -> Result<Vec<Stored>> {
    // Hashing/compression and eligibility run before BEGIN so oversized entries
    // do not block other processes or suppress eligible per-file recovery.
    let entries: Vec<_> = inputs
        .iter()
        .map(|input| {
            if input.content.is_empty() {
                return PreparedSlot::Empty;
            }
            match prepare_entry(
                cfg,
                StoreInput {
                    content: input.content,
                    command: input.command,
                    exit_code: input.exit_code,
                    shown_upto: input.shown_upto,
                },
            ) {
                Ok(entry)
                    if cfg.max_total_bytes == 0 || entry.blob.len() <= cfg.max_total_bytes =>
                {
                    PreparedSlot::Entry(entry)
                }
                Ok(_) | Err(_) => PreparedSlot::Unavailable,
            }
        })
        .collect();
    write_batch(cfg, &entries)
}

fn write_batch(cfg: &RetrieverConfig, entries: &[PreparedSlot<'_>]) -> Result<Vec<Stored>> {
    if !entries.iter().any(|slot| slot.entry().is_some()) {
        return Ok(entries
            .iter()
            .map(PreparedSlot::unavailable_result)
            .collect());
    }
    let cwd = user_dirs::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    let invocation = super::invocation::current();
    with_open(cfg, |conn| {
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
            .context("begin recall batch")?;
        // Deduplicated payloads retain their first encoding. A smaller newly
        // prepared blob must not make an oversized existing payload eligible.
        let mut existing_size =
            tx.prepare_cached("SELECT length(blob) FROM recall WHERE hash = ?1")?;
        let eligible = entries
            .iter()
            .filter_map(PreparedSlot::entry)
            .map(|entry| {
                let fits = if cfg.max_total_bytes == 0 {
                    true
                } else {
                    let retained: Option<i64> = existing_size
                        .query_row([&entry.hash], |r| r.get(0))
                        .optional()?;
                    retained.map_or(entry.blob.len() <= cfg.max_total_bytes, |bytes| {
                        bytes as u64 <= cfg.max_total_bytes as u64
                    })
                };
                Ok(fits.then_some(entry))
            })
            .collect::<Result<Vec<_>>>()?;
        drop(existing_size);
        let eligible: Vec<_> = eligible.into_iter().flatten().collect();
        let Some(last) = eligible.last() else {
            return Ok(entries
                .iter()
                .map(PreparedSlot::unavailable_result)
                .collect());
        };
        // Remove old references while their payloads are still absent, before an
        // identical payload can be recaptured during this transaction.
        prune_captures(&tx)?;
        {
            let mut insert = tx
                .prepare_cached(INSERT_RECALL)
                .context("prepare recall insert")?;
            let mut capture = tx
                .prepare_cached(
                    "INSERT INTO recall_captures
                 (capture_id, invocation_id, hash, created_at, command, cwd, exit_code,
                  total_lines, shown_upto, byte_size, truncated)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                )
                .context("prepare recall capture")?;
            let created_at = now_secs();
            for entry in &eligible {
                insert
                    .execute(params![
                        entry.hash,
                        entry.input.command,
                        cwd,
                        entry.input.exit_code,
                        created_at,
                        entry.total_lines as i64,
                        entry.shown_upto as i64,
                        entry.input.content.len() as i64,
                        entry.truncated as i64,
                        entry.codec,
                        entry.blob,
                    ])
                    .context("insert recall payload")?;
                capture
                    .execute(params![
                        entry.capture_id,
                        invocation.id,
                        entry.hash,
                        created_at,
                        entry.input.command,
                        cwd,
                        entry.input.exit_code,
                        entry.total_lines as i64,
                        entry.shown_upto as i64,
                        entry.input.content.len() as i64,
                        entry.truncated as i64,
                    ])
                    .context("record immutable recall capture")?;
                bump_stat(&tx, entry.input.command, "sqlite", "elisions");
            }
        }
        evict(&tx, cfg, &[&last.hash])?;
        prune_captures(&tx)?;
        let results = {
            // Both caps matter: a payload can survive while this particular
            // capture expires. Each duplicate also keeps its own exact offset.
            let mut retained = tx
                .prepare_cached(
                    "SELECT c.total_lines, c.shown_upto FROM recall_captures c
                 JOIN recall p ON p.hash = c.hash WHERE c.capture_id = ?1",
                )
                .context("prepare retained capture lookup")?;
            entries
                .iter()
                .map(|slot| {
                    let Some(entry) = slot.entry() else {
                        return Ok(slot.unavailable_result());
                    };
                    let counts: Option<(i64, i64)> = retained
                        .query_row([&entry.capture_id], |r| Ok((r.get(0)?, r.get(1)?)))
                        .optional()
                        .context("verify retained recall capture")?;
                    Ok(match counts {
                        Some((total, shown)) => Stored::Saved(StoredRef {
                            reference: format!("c_{}", entry.capture_id),
                            #[cfg(test)]
                            hash: entry.hash.clone(),
                            hidden_lines: total.saturating_sub(shown.saturating_sub(1)).max(0)
                                as usize,
                        }),
                        None => Stored::Unavailable,
                    })
                })
                .collect::<Result<Vec<_>>>()?
        };
        tx.commit().context("commit recall batch")?;
        Ok(results)
    })
}

#[cfg(feature = "engine")]
pub fn trim_family(cfg: &RetrieverConfig, prefix: &str, keep: usize) {
    let _ = with_open(cfg, |conn| {
        let tx =
            rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM recall WHERE rowid IN (
                SELECT rowid FROM recall WHERE command LIKE ?1 || '%'
                ORDER BY created_at DESC, rowid DESC LIMIT -1 OFFSET ?2
            )",
            params![prefix, keep as i64],
        )?;
        prune_captures(&tx)?;
        tx.commit().context("trim recall family")
    });
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
    match store_inner(cfg, content, command, exit_code, shown_upto) {
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
    let input = StoreInput {
        content,
        command,
        exit_code,
        shown_upto,
    };
    match store_batch_inner(cfg, &[input])?.pop() {
        Some(Stored::Saved(stored)) => Ok(stored),
        _ => anyhow::bail!("recall entry was not retained"),
    }
}

fn prune_captures(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM recall_captures WHERE hash NOT IN (SELECT hash FROM recall)",
        [],
    )?;
    conn.execute(
        "DELETE FROM recall_captures WHERE rowid NOT IN
         (SELECT rowid FROM recall_captures ORDER BY rowid DESC LIMIT ?1)",
        [MAX_CAPTURE_RECORDS],
    )?;
    Ok(())
}

/// Count bytes actually accepted by stdout, including a partial write before an
/// error. Token estimates use exactly those bytes, never the full stored blob.
fn emit_recall(writer: &mut impl Write, output: &[u8]) -> (usize, std::io::Result<()>) {
    let mut written = 0;
    while written < output.len() {
        match writer.write(&output[written..]) {
            Ok(0) => return (written, Err(std::io::ErrorKind::WriteZero.into())),
            Ok(n) => written += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return (written, Err(e)),
        }
    }
    (written, writer.flush())
}

#[derive(Debug)]
struct Row {
    shown_upto: usize,
    truncated: bool,
    codec: String,
    blob: Vec<u8>,
    command: String,
    hash: String,
    capture_id: Option<String>,
    invocation_id: Option<String>,
}

fn map_row(r: &rusqlite::Row) -> rusqlite::Result<Row> {
    Ok(Row {
        shown_upto: r.get::<_, i64>(0)? as usize,
        truncated: r.get::<_, i64>(1)? != 0,
        codec: r.get(2)?,
        blob: r.get(3)?,
        command: r.get(4)?,
        hash: r.get(5)?,
        capture_id: None,
        invocation_id: None,
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

fn load_reference(conn: &Connection, reference: &str) -> Result<Option<Row>> {
    let Some(capture_id) = reference.strip_prefix("c_") else {
        // Legacy hashes remain unassigned, including after migration. Never
        // guess which source invocation owns an ambiguous content identity.
        return load_by_hash(conn, reference);
    };
    if !super::invocation::valid_id(capture_id) {
        anyhow::bail!("use the complete c_<capture-id> from the recovery hint");
    }
    conn.query_row(
        "SELECT c.shown_upto, c.truncated, p.codec, p.blob, c.command, p.hash,
                c.capture_id, c.invocation_id
         FROM recall_captures c JOIN recall p ON p.hash = c.hash
         WHERE c.capture_id = ?1 AND c.shown_upto IS NOT NULL
           AND c.truncated IS NOT NULL AND c.command IS NOT NULL",
        [capture_id],
        |r| {
            let mut row = map_row(r)?;
            row.capture_id = r.get(6)?;
            row.invocation_id = r.get(7)?;
            Ok(row)
        },
    )
    .optional()
    .context("load recall capture")
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

pub fn run_maintenance(purge: bool) -> Result<i32> {
    // A broken config must not fall back to purging a different default store.
    let cfg = Config::load()?.retriever;
    let removed = maintain_store(&cfg, purge)?;
    println!("rtk recall: removed {removed} stored output(s); recall statistics preserved");
    Ok(0)
}

pub fn run_recall(args: RecallArgs) -> Result<i32> {
    if args.hash.is_none() && !args.list {
        eprintln!("rtk recall: provide a <reference> (from a recovery hint) or --list");
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
    let row = match load_reference(&conn, hash) {
        Ok(row) => row,
        Err(e) => {
            eprintln!("rtk recall: {e}");
            return Ok(1);
        }
    };

    let Some(row) = row else {
        if hash.starts_with("c_") {
            eprintln!("rtk recall: capture unavailable or expired (try `rtk recall --list`)");
        } else {
            eprintln!("rtk recall: no matching entry (try `rtk recall --list`)");
        }
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
    let (emitted, result) = emit_recall(&mut stdout.lock(), &out);
    if (result.is_ok() || emitted > 0)
        && let Ok(tracker) = super::tracking::Tracker::new()
    {
        let _ = tracker.record_recall_event(
            emitted,
            row.invocation_id.as_deref(),
            row.capture_id.as_deref(),
        );
    }
    result.context("write recalled output")?;
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
        "SELECT reference, command, total_lines, shown_upto, exit_code, truncated, byte_size
         FROM (
             SELECT 'c_' || c.capture_id AS reference, c.command, c.total_lines,
                    c.shown_upto, c.exit_code, c.truncated, c.byte_size, c.created_at
             FROM recall_captures c JOIN recall p ON p.hash = c.hash
             WHERE c.command IS NOT NULL
             UNION ALL
             SELECT hash, command, total_lines, shown_upto, exit_code, truncated,
                    byte_size, created_at FROM recall WHERE substr(hash, 1, 2) != 'p_'
         ) ORDER BY created_at DESC, reference DESC LIMIT 50",
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
        "{:<34} {:<26} {:>7} {:>7} {:>9} {:>5} TRUNC",
        "REFERENCE", "COMMAND", "LINES", "HIDDEN", "ORIG", "EXIT"
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
            "{:<34} {:<26} {:>7} {:>7} {:>9} {:>5} {}",
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

    #[cfg(feature = "engine")]
    #[test]
    fn test_posthook_family_trim_updates_payload_budget() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            compression: false,
            max_total_bytes: 16,
            ..temp_cfg(dir.path())
        };
        let failure = store_inner(&cfg, b"err\n", "cargo test", Some(1), 1).unwrap();
        store_inner(&cfg, b"old\n", "posthook:bash_ls", None, 1).unwrap();
        let newest = store_inner(&cfg, b"new\n", "posthook:bash_ls", None, 1).unwrap();
        trim_family(&cfg, "posthook:", 1);
        let conn = open(&cfg).unwrap();
        assert_eq!(storage_totals(&conn), (2, 8));
        assert!(load_by_hash(&conn, &failure.hash).unwrap().is_some());
        assert!(load_by_hash(&conn, &newest.hash).unwrap().is_some());
        store_inner(&cfg, b"next log", "cargo build", Some(1), 1).unwrap();
        assert_eq!(storage_totals(&conn), (3, 16));
        assert!(load_by_hash(&conn, &failure.hash).unwrap().is_some());
    }

    #[test]
    fn test_forget_rejects_an_empty_database_path() {
        let cfg = RetrieverConfig::default();
        let error =
            user_env::with_vars(&[("RTK_RECALL_DB", Some(""))], || forget(&cfg)).unwrap_err();
        assert!(error.to_string().contains("empty recall database path"));
    }

    #[test]
    fn test_forget_closes_cached_store_without_recreating_it() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        store_inner(&cfg, b"private output\n", "cmd", Some(1), 1).unwrap();
        forget(&cfg).unwrap();
        assert!(!db_path(&cfg).unwrap().exists());
        assert!(stats_snapshot_with(&cfg).unwrap().is_empty());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        forget(&cfg).unwrap();
    }

    #[test]
    fn test_forget_removes_orphan_sidecars_and_preserves_neighbors() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let path = db_path(&cfg).unwrap();
        for suffix in ["-wal", "-shm", "-journal", ".backup"] {
            std::fs::write(
                path.with_file_name(format!("recall_test.db{suffix}")),
                "secret",
            )
            .unwrap();
        }
        forget(&cfg).unwrap();
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["recall_test.db.backup"]);
    }

    #[test]
    fn test_forget_respects_environment_override() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let configured = db_path(&cfg).unwrap();
        let overridden = dir.path().join("override.sqlite");
        drop(open(&cfg).unwrap());
        user_env::with_path("RTK_RECALL_DB", Some(&overridden), || {
            drop(open(&cfg).unwrap());
            forget(&cfg)
        })
        .unwrap();
        assert!(!overridden.exists());
        assert!(configured.exists());
    }

    #[test]
    fn test_forget_refuses_busy_wal_store_without_unlinking_it() {
        for transaction in ["BEGIN IMMEDIATE", "BEGIN; SELECT * FROM recall"] {
            let dir = tempfile::tempdir().unwrap();
            let cfg = temp_cfg(dir.path());
            store_inner(&cfg, b"private output", "cmd", Some(1), 1).unwrap();
            let other = open(&cfg).unwrap();
            other.execute_batch(transaction).unwrap();
            let error = forget(&cfg).unwrap_err();
            assert!(error.to_string().contains("stop other RTK commands"));
            assert!(db_path(&cfg).unwrap().exists());
            assert!(dir.path().join("recall_test.db-wal").exists());
            drop(other);
            forget(&cfg).unwrap();
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        }
    }

    #[test]
    fn test_forget_never_removes_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let path = db_path(&cfg).unwrap();
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("keep"), "unrelated").unwrap();
        assert!(forget(&cfg).is_err());
        assert!(path.join("keep").exists());
    }

    #[cfg(unix)]
    #[test]
    fn test_forget_rejects_symlinks_without_touching_the_target() {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let dir = tempfile::tempdir().unwrap();
            let cfg = temp_cfg(dir.path());
            if !suffix.is_empty() {
                drop(open(&cfg).unwrap());
            }
            let target = dir.path().join("unrelated");
            std::fs::write(&target, "keep").unwrap();
            let link = dir.path().join(format!("recall_test.db{suffix}"));
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert!(forget(&cfg).is_err());
            assert!(db_path(&cfg).unwrap().exists());
            assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
            assert_eq!(std::fs::read_to_string(target).unwrap(), "keep");
        }
    }

    fn temp_cfg(dir: &std::path::Path) -> RetrieverConfig {
        RetrieverConfig {
            database_path: Some(dir.join("recall_test.db")),
            ..RetrieverConfig::default()
        }
    }

    fn storage_totals(conn: &Connection) -> (i64, i64) {
        conn.query_row(
            "SELECT entries, blob_bytes FROM recall_storage WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    }

    #[test]
    fn test_total_budget_defaults_to_legacy_unlimited() {
        let cfg: RetrieverConfig = toml::from_str("max_entries = 12").unwrap();
        assert_eq!(cfg.max_total_bytes, 0);
        assert_eq!(cfg.max_entries, 12);
    }

    #[test]
    fn test_total_budget_evicts_oldest_and_keeps_returned_hash() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            max_total_bytes: 8,
            compression: false,
            ..temp_cfg(dir.path())
        };
        let first = store_inner(&cfg, b"aaa\n", "cmd", None, 1).unwrap();
        let second = store_inner(&cfg, b"bbb\n", "cmd", None, 1).unwrap();
        let newest = store_inner(&cfg, b"ccc\n", "cmd", None, 1).unwrap();
        let conn = open(&cfg).unwrap();
        assert!(load_by_hash(&conn, &first.hash).unwrap().is_none());
        assert!(load_by_hash(&conn, &second.hash).unwrap().is_some());
        assert_eq!(
            decode(&load_by_hash(&conn, &newest.hash).unwrap().unwrap()).unwrap(),
            b"ccc\n"
        );
        assert_eq!(storage_totals(&conn), (2, 8));
    }

    #[test]
    fn test_total_budget_measures_encoded_blob_and_rejects_oversize() {
        let dir = tempfile::tempdir().unwrap();
        let content = vec![b'x'; 4096];
        let bytes = gzip(&content).unwrap().len();
        let cfg = RetrieverConfig {
            max_total_bytes: bytes,
            ..temp_cfg(dir.path())
        };
        let saved = store_inner(&cfg, &content, "cmd", None, 1).unwrap();
        let too_small = RetrieverConfig {
            max_total_bytes: bytes - 1,
            ..cfg.clone()
        };
        assert!(matches!(
            store(&too_small, &content, "new", None, 1),
            Stored::Unavailable
        ));
        let conn = open(&cfg).unwrap();
        assert_eq!(storage_totals(&conn), (1, bytes as i64));
        assert!(load_by_hash(&conn, &saved.hash).unwrap().is_some());
        assert_eq!(stats_snapshot_with(&cfg).unwrap()[0].elisions, 1);
    }

    #[test]
    fn test_storage_totals_follow_upsert_delete_and_legacy_migration() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let content = vec![b'a'; 100];
        let stored = store_inner(&cfg, &content, "cmd", None, 1).unwrap();
        let raw = RetrieverConfig {
            compression: false,
            ..cfg.clone()
        };
        store_inner(&raw, &content, "cmd", None, 1).unwrap();
        let conn = open(&cfg).unwrap();
        assert_eq!(
            storage_totals(&conn),
            (1, gzip(&content).unwrap().len() as i64)
        );
        conn.execute_batch("DROP TRIGGER recall_storage_insert; DROP TRIGGER recall_storage_update; DROP TRIGGER recall_storage_delete; DROP TABLE recall_storage;").unwrap();
        init_schema(&conn).unwrap();
        init_schema(&conn).unwrap();
        assert_eq!(
            storage_totals(&conn),
            (1, gzip(&content).unwrap().len() as i64)
        );
        conn.execute("DELETE FROM recall WHERE hash = ?1", [&stored.hash])
            .unwrap();
        assert_eq!(storage_totals(&conn), (0, 0));
    }

    #[test]
    fn test_eviction_failure_rolls_back_store_and_stats() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            max_total_bytes: 4,
            compression: false,
            ..temp_cfg(dir.path())
        };
        let old = store_inner(&cfg, b"old\n", "cmd", None, 1).unwrap();
        let conn = open(&cfg).unwrap();
        conn.execute_batch("CREATE TRIGGER block_eviction BEFORE DELETE ON recall BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;").unwrap();
        assert!(matches!(
            store(&cfg, b"new\n", "cmd", None, 1),
            Stored::Unavailable
        ));
        assert!(load_by_hash(&conn, &old.hash).unwrap().is_some());
        assert_eq!(storage_totals(&conn), (1, 4));
        assert_eq!(stats_snapshot_with(&cfg).unwrap()[0].elisions, 1);
    }

    #[test]
    fn test_gc_applies_retention_and_budget_without_new_store() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            compression: false,
            ..temp_cfg(dir.path())
        };
        let old = store_inner(&cfg, b"old\n", "cmd", None, 1).unwrap();
        let keep = store_inner(&cfg, b"new\n", "cmd", None, 1).unwrap();
        let conn = open(&cfg).unwrap();
        conn.execute(
            "UPDATE recall SET created_at = 0 WHERE hash = ?1",
            [&old.hash],
        )
        .unwrap();
        let bounded = RetrieverConfig {
            max_total_bytes: 4,
            ..cfg.clone()
        };
        assert_eq!(maintain_store(&bounded, false).unwrap(), 1);
        assert!(load_by_hash(&conn, &keep.hash).unwrap().is_some());
        assert_eq!(storage_totals(&conn), (1, 4));
    }

    #[test]
    fn test_purge_preserves_stats_and_unrelated_files_even_when_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        store_inner(&cfg, b"stored\n", "cmd", None, 1).unwrap();
        let unrelated = dir.path().join("tracking.db");
        std::fs::write(&unrelated, b"unrelated fixture").unwrap();
        let disabled = RetrieverConfig {
            mode: RecoveryMode::Disabled,
            ..cfg.clone()
        };
        assert_eq!(maintain_store(&disabled, true).unwrap(), 1);
        let conn = open(&cfg).unwrap();
        assert_eq!(storage_totals(&conn), (0, 0));
        assert_eq!(stats_snapshot_with(&cfg).unwrap()[0].elisions, 1);
        assert_eq!(std::fs::read(unrelated).unwrap(), b"unrelated fixture");
        assert_eq!(maintain_store(&cfg, true).unwrap(), 0);
    }

    #[test]
    fn test_maintenance_never_creates_store_or_initializes_unrelated_database() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        assert_eq!(maintain_store(&cfg, false).unwrap(), 0);
        assert_eq!(maintain_store(&cfg, true).unwrap(), 0);
        let path = cfg.database_path.as_ref().unwrap();
        assert!(!path.exists());
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE unrelated (value TEXT); INSERT INTO unrelated VALUES ('fixture');",
        )
        .unwrap();
        assert!(maintain_store(&cfg, true).is_err());
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_concurrent_stores_obey_total_budget() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            max_total_bytes: 40,
            compression: false,
            ..temp_cfg(dir.path())
        };
        drop(open(&cfg).unwrap());
        let workers: Vec<_> = (0..8)
            .map(|i| {
                let cfg = cfg.clone();
                std::thread::spawn(move || {
                    for j in 0..10 {
                        store_inner(&cfg, format!("{i}:{j}\n").as_bytes(), "cmd", None, 1).unwrap();
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        let conn = open(&cfg).unwrap();
        let (entries, bytes) = storage_totals(&conn);
        assert!(entries <= 10);
        assert!(bytes <= 40);
        let actual: i64 = conn
            .query_row("SELECT SUM(length(blob)) FROM recall", [], |r| r.get(0))
            .unwrap();
        assert_eq!(bytes, actual);
        assert_eq!(stats_snapshot_with(&cfg).unwrap()[0].elisions, 80);
    }

    #[test]
    fn test_gc_enforces_lowered_budget_and_compacts_pages() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            compression: false,
            retention_days: 0,
            ..temp_cfg(dir.path())
        };
        store_inner(&cfg, &vec![b'x'; 256 * 1024], "large", None, 1).unwrap();
        store_inner(&cfg, b"new\n", "small", None, 1).unwrap();
        let conn = open(&cfg).unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        let path = cfg.database_path.as_ref().unwrap();
        let before = std::fs::metadata(path).unwrap().len();
        let bounded = RetrieverConfig {
            max_total_bytes: 4,
            ..cfg.clone()
        };
        assert_eq!(maintain_store(&bounded, false).unwrap(), 1);
        assert_eq!(storage_totals(&conn), (1, 4));
        assert!(std::fs::metadata(path).unwrap().len() < before);
        // The blob budget is deliberately not a physical SQLite-file budget.
        assert!(std::fs::metadata(path).unwrap().len() > 4);
    }

    #[test]
    fn test_purge_uses_environment_override_only() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        store_inner(&cfg, b"config\n", "cmd", None, 1).unwrap();
        let other = dir.path().join("override.db");
        user_env::with_path("RTK_RECALL_DB", Some(&other), || {
            store_inner(&cfg, b"override\n", "cmd", None, 1).unwrap();
            assert_eq!(maintain_store(&cfg, true).unwrap(), 1);
            assert_eq!(storage_totals(&open(&cfg).unwrap()), (0, 0));
        });
        assert_eq!(storage_totals(&open(&cfg).unwrap()).0, 1);
    }

    #[test]
    fn test_budget_evict_protects_all_promised_hashes_or_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RetrieverConfig {
            compression: false,
            ..temp_cfg(dir.path())
        };
        let first = store_inner(&cfg, b"one\n", "cmd", None, 1).unwrap();
        let second = store_inner(&cfg, b"two\n", "cmd", None, 1).unwrap();
        let conn = open(&cfg).unwrap();
        let bounded = RetrieverConfig {
            max_total_bytes: 4,
            ..cfg
        };
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).unwrap();
        assert!(evict(&tx, &bounded, &[&first.hash, &second.hash]).is_err());
        drop(tx);
        assert_eq!(storage_totals(&conn), (2, 8));
    }

    #[test]
    fn test_maintenance_accepts_legacy_store_before_recalled_migration() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        store_inner(&cfg, b"legacy\n", "cmd", None, 1).unwrap();
        let conn = open(&cfg).unwrap();
        conn.execute_batch("ALTER TABLE recall DROP COLUMN recalled;")
            .unwrap();
        assert_eq!(maintain_store(&cfg, true).unwrap(), 1);
        assert_eq!(storage_totals(&conn), (0, 0));
    }

    #[test]
    fn test_eviction_handles_large_protected_hash_sets() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        let saved = store_inner(&cfg, b"keep\n", "cmd", None, 1).unwrap();
        let conn = open(&cfg).unwrap();
        conn.execute("UPDATE recall SET created_at = 0", [])
            .unwrap();
        let protected = vec![saved.hash.as_str(); 40_000];
        evict(&conn, &cfg, &protected).unwrap();
        assert!(load_by_hash(&conn, &saved.hash).unwrap().is_some());
    }

    #[test]
    fn test_batch_budget_rejects_only_oversized_slots_without_opening_empty_store() {
        let dir = tempfile::tempdir().expect("directory");
        let cfg = RetrieverConfig {
            max_total_bytes: 8,
            compression: false,
            ..temp_cfg(dir.path())
        };
        let rejected = store_batch(
            &cfg,
            &[
                batch_input(b"oversized", "large", 1),
                batch_input(b"", "empty", 1),
            ],
        );
        assert!(matches!(rejected[0], Stored::Unavailable));
        assert!(matches!(rejected[1], Stored::Empty));
        assert!(!db_path(&cfg).expect("path").exists());
        let results = store_batch(
            &cfg,
            &[
                batch_input(b"oversized", "first", 1),
                batch_input(b"one\n", "accepted", 1),
                batch_input(b"", "empty", 1),
                batch_input(b"oversized", "last", 1),
            ],
        );
        assert!(matches!(results[0], Stored::Unavailable));
        assert!(matches!(results[2], Stored::Empty));
        assert!(matches!(results[3], Stored::Unavailable));
        let Stored::Saved(ref saved) = results[1] else {
            panic!("eligible capture");
        };
        let conn = open(&cfg).expect("db");
        let row = load_reference(&conn, &saved.reference)
            .expect("lookup")
            .expect("capture");
        assert_eq!(decode(&row).expect("decode"), b"one\n");
        assert_eq!(storage_totals(&conn), (1, 4));
        assert_eq!(
            stats_snapshot_with(&cfg)
                .expect("stats")
                .iter()
                .map(|s| s.elisions)
                .sum::<i64>(),
            1
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM recall_captures", [], |r| r
                .get::<_, i64>(0))
                .expect("captures"),
            1
        );
        let rejected = store_batch(&cfg, &[batch_input(b"oversized", "large", 1)]);
        assert!(matches!(rejected[0], Stored::Unavailable));
        assert_eq!(storage_totals(&conn), (1, 4));
        assert!(
            load_reference(&conn, &saved.reference)
                .expect("lookup")
                .is_some()
        );
    }

    #[test]
    fn test_batch_total_budget_evicts_payloads_captures_and_keeps_exact_totals() {
        let dir = tempfile::tempdir().expect("directory");
        let cfg = RetrieverConfig {
            max_entries: 0,
            max_total_bytes: 8,
            compression: false,
            ..temp_cfg(dir.path())
        };
        let results = store_batch(
            &cfg,
            &[
                batch_input(b"one\n", "one", 1),
                batch_input(b"two\n", "two", 1),
                batch_input(b"tri\n", "three", 1),
            ],
        );
        assert!(matches!(results[0], Stored::Unavailable));
        let conn = open(&cfg).expect("db");
        assert_eq!(storage_totals(&conn), (2, 8));
        for result in &results[1..] {
            let Stored::Saved(saved) = result else {
                panic!("retained capture");
            };
            assert!(
                load_reference(&conn, &saved.reference)
                    .expect("lookup")
                    .is_some()
            );
        }
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM recall_captures", [], |r| r
                .get::<_, i64>(0))
                .expect("captures"),
            2
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM recall_captures WHERE hash NOT IN (SELECT hash FROM recall)",
                [],
                |r| r.get::<_, i64>(0)
            )
            .expect("orphans"),
            0
        );
    }

    #[test]
    fn test_batch_capture_cap_does_not_advertise_expired_duplicate_references() {
        let dir = tempfile::tempdir().expect("directory");
        let cfg = RetrieverConfig {
            max_entries: 0,
            ..temp_cfg(dir.path())
        };
        let inputs: Vec<_> = (0..MAX_CAPTURE_RECORDS + 5)
            .map(|_| batch_input(b"same\npayload\n", "same", 2))
            .collect();
        let results = store_batch(&cfg, &inputs);
        assert!(
            results[..5]
                .iter()
                .all(|s| matches!(s, Stored::Unavailable))
        );
        let conn = open(&cfg).expect("db");
        for result in &results[5..] {
            let Stored::Saved(saved) = result else {
                panic!("retained capture");
            };
            let row = load_reference(&conn, &saved.reference)
                .expect("lookup")
                .expect("capture");
            assert_eq!(saved.hidden_lines, 1);
            assert_eq!(
                slice_from_line(&decode(&row).expect("decode"), row.shown_upto),
                b"payload\n"
            );
        }
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM recall_captures", [], |r| r
                .get::<_, i64>(0))
                .expect("captures"),
            MAX_CAPTURE_RECORDS
        );
        assert_eq!(storage_totals(&conn).0, 1);
    }

    #[test]
    fn test_batch_capture_insert_failure_rolls_back_payload_stats_and_totals() {
        let dir = tempfile::tempdir().expect("directory");
        let cfg = temp_cfg(dir.path());
        let conn = open(&cfg).expect("db");
        conn.execute_batch("CREATE TRIGGER reject_capture BEFORE INSERT ON recall_captures WHEN NEW.command = 'second' BEGIN SELECT RAISE(ABORT, 'test failure'); END;").expect("trigger");
        let results = store_batch(
            &cfg,
            &[
                batch_input(b"first\n", "first", 1),
                batch_input(b"second\n", "second", 1),
            ],
        );
        assert!(results.iter().all(|s| matches!(s, Stored::Unavailable)));
        assert_eq!(storage_totals(&conn), (0, 0));
        assert!(stats_snapshot_with(&cfg).expect("stats").is_empty());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM recall_captures", [], |r| r
                .get::<_, i64>(0))
                .expect("captures"),
            0
        );
    }

    #[test]
    fn test_maintenance_prunes_capture_metadata_without_resurrecting_old_hints() {
        for purge in [false, true] {
            let dir = tempfile::tempdir().expect("directory");
            let cfg = temp_cfg(dir.path());
            let old = store_inner(&cfg, b"private\n", "private", None, 1).expect("capture");
            let conn = open(&cfg).expect("db");
            if !purge {
                conn.execute("UPDATE recall SET created_at = 0", [])
                    .expect("age");
            }
            assert_eq!(maintain_store(&cfg, purge).expect("maintenance"), 1);
            assert_eq!(storage_totals(&conn), (0, 0));
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM recall_captures", [], |r| r
                    .get::<_, i64>(0))
                    .expect("captures"),
                0
            );
            let new = store_inner(&cfg, b"private\n", "private", None, 1).expect("recapture");
            assert!(
                load_reference(&conn, &old.reference)
                    .expect("old lookup")
                    .is_none()
            );
            assert!(
                load_reference(&conn, &new.reference)
                    .expect("new lookup")
                    .is_some()
            );
        }
    }

    #[test]
    fn test_batched_stats_cap_composes_with_payload_and_capture_accounting() {
        let dir = tempfile::tempdir().expect("directory");
        let cfg = RetrieverConfig {
            max_entries: 0,
            ..temp_cfg(dir.path())
        };
        let commands: Vec<_> = (0..MAX_STATS_ROWS + 5)
            .map(|i| format!("dynamic{i}"))
            .collect();
        let inputs: Vec<_> = commands
            .iter()
            .map(|command| batch_input(b"row\n", command, 1))
            .collect();
        let results = store_batch(&cfg, &inputs);
        assert!(results.iter().all(|s| matches!(s, Stored::Saved(_))));
        let conn = open(&cfg).expect("db");
        assert_eq!(
            stats_snapshot_with(&cfg).expect("stats").len(),
            MAX_STATS_ROWS
        );
        assert_eq!(storage_totals(&conn).0, (MAX_STATS_ROWS + 5) as i64);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM recall_captures", [], |r| r
                .get::<_, i64>(0))
                .expect("captures"),
            (MAX_STATS_ROWS + 5) as i64
        );
    }

    #[test]
    fn test_batch_budget_checks_immutable_retained_encoding_and_keeps_valid_peers() {
        let dir = tempfile::tempdir().expect("directory");
        let original = RetrieverConfig {
            compression: false,
            ..temp_cfg(dir.path())
        };
        let large = vec![b'x'; 4096];
        let old = store_inner(&original, &large, "same", None, 1).expect("raw capture");
        let bounded = RetrieverConfig {
            compression: true,
            max_total_bytes: 100,
            ..original
        };
        assert!(gzip(&large).expect("gzip").len() < bounded.max_total_bytes);
        let results = store_batch(
            &bounded,
            &[
                batch_input(b"valid\n", "valid", 1),
                batch_input(&large, "same", 1),
            ],
        );
        let Stored::Saved(ref valid) = results[0] else {
            panic!("valid peer must survive rejected duplicate");
        };
        assert!(matches!(results[1], Stored::Unavailable));
        let conn = open(&bounded).expect("db");
        assert!(
            load_reference(&conn, &valid.reference)
                .expect("valid lookup")
                .is_some()
        );
        assert!(
            load_reference(&conn, &old.reference)
                .expect("old lookup")
                .is_none()
        );
        assert!(storage_totals(&conn).1 <= bounded.max_total_bytes as i64);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM recall_captures", [], |r| r
                .get::<_, i64>(0))
                .expect("captures"),
            1
        );
    }

    fn batch_input<'a>(content: &'a [u8], command: &'a str, shown_upto: usize) -> StoreInput<'a> {
        StoreInput {
            content,
            command,
            exit_code: None,
            shown_upto,
        }
    }

    #[test]
    fn test_batch_empty_never_creates_database() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = temp_cfg(dir.path());
        assert!(store_batch(&cfg, &[]).is_empty());
        assert!(matches!(
            store_batch(&cfg, &[batch_input(b"", "empty", 1)])[0],
            Stored::Empty
        ));
        assert!(!cfg.database_path.expect("path").exists());
    }

    #[test]
    fn test_batch_rolls_back_every_row_on_insert_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = temp_cfg(dir.path());
        let conn = open(&cfg).expect("open");
        conn.execute_batch(
            "CREATE TRIGGER reject_second BEFORE INSERT ON recall
            WHEN NEW.command = 'second' BEGIN SELECT RAISE(ABORT, 'test failure'); END;",
        )
        .expect("trigger");
        let stored = store_batch(
            &cfg,
            &[
                batch_input(b"first\n", "first", 1),
                batch_input(b"second\n", "second", 1),
            ],
        );
        assert!(stored.iter().all(|s| matches!(s, Stored::Unavailable)));
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM recall", [], |r| r.get::<_, i64>(0))
                .expect("count"),
            0
        );
        assert!(stats_snapshot_with(&cfg).expect("stats").is_empty());
        conn.execute_batch("DROP TRIGGER reject_second;")
            .expect("drop trigger");
        assert!(
            matches!(store(&cfg, b"next\n", "next", None, 1), Stored::Saved(_)),
            "failed batch must release its transaction"
        );
    }

    #[test]
    fn test_batch_rolls_back_when_retention_cannot_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = RetrieverConfig {
            max_entries: 1,
            ..temp_cfg(dir.path())
        };
        let conn = open(&cfg).expect("open");
        conn.execute_batch(
            "CREATE TRIGGER reject_delete BEFORE DELETE ON recall
            BEGIN SELECT RAISE(ABORT, 'test failure'); END;",
        )
        .expect("trigger");
        let stored = store_batch(
            &cfg,
            &[
                batch_input(b"first\n", "first", 1),
                batch_input(b"second\n", "second", 1),
            ],
        );
        assert!(stored.iter().all(|s| matches!(s, Stored::Unavailable)));
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM recall", [], |r| r.get::<_, i64>(0))
                .expect("count"),
            0
        );
        assert!(stats_snapshot_with(&cfg).expect("stats").is_empty());
    }

    #[test]
    fn test_batch_returns_only_retained_captures_with_original_offsets() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = RetrieverConfig {
            max_entries: 2,
            ..temp_cfg(dir.path())
        };
        let stored = store_batch(
            &cfg,
            &[
                batch_input(b"old\n", "old", 1),
                batch_input(b"shown\nhidden\ntail\n", "duplicate", 1),
                batch_input(b"new\n", "new", 1),
                batch_input(b"shown\nhidden\ntail\n", "duplicate", 3),
            ],
        );
        assert!(matches!(stored[0], Stored::Unavailable));
        let conn = open(&cfg).expect("open");
        for (index, expected) in [(1, 3), (2, 1), (3, 1)] {
            let Stored::Saved(ref saved) = stored[index] else {
                panic!("retained entry");
            };
            let row = load_reference(&conn, &saved.reference)
                .expect("read")
                .expect("row");
            assert_eq!(saved.hidden_lines, expected);
            assert_eq!(
                count_lines(slice_from_line(
                    &decode(&row).expect("decode"),
                    row.shown_upto
                )),
                expected
            );
        }
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM recall", [], |r| r.get::<_, i64>(0))
                .expect("count"),
            2
        );
    }

    #[test]
    fn test_batch_preserves_caps_codecs_offsets_and_bytes() {
        for compression in [false, true] {
            for cap in [0, 5, 12, 1024] {
                let dir = tempfile::tempdir().expect("tempdir");
                let cfg = RetrieverConfig {
                    compression,
                    max_entry_bytes: cap,
                    ..temp_cfg(dir.path())
                };
                let inputs = [
                    batch_input(b"seen\nhidden\n\xff\x00tail\n", "one", 2),
                    batch_input(b"hello\r\nno-eol", "two", 0),
                    batch_input(b"", "empty", 1),
                ];
                let stored = store_batch(&cfg, &inputs);
                assert!(matches!(stored[2], Stored::Empty));
                let conn = open(&cfg).expect("open");
                for (input, result) in inputs.iter().zip(stored).take(2) {
                    let Stored::Saved(saved) = result else {
                        panic!("stored");
                    };
                    let row = load_reference(&conn, &saved.reference)
                        .expect("read")
                        .expect("row");
                    let bytes = decode(&row).expect("decode");
                    assert!(bytes.len() <= cap);
                    assert_eq!(
                        saved.hidden_lines,
                        count_lines(slice_from_line(&bytes, row.shown_upto))
                    );
                    assert_eq!(row.codec, if compression { "gzip" } else { "raw" });
                    if cap >= input.content.len() {
                        assert_eq!(bytes, input.content);
                    }
                }
            }
        }
    }

    #[test]
    fn test_batch_concurrent_writers_commit_complete_batches() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = temp_cfg(dir.path());
        // Initialize before racing writers so this exercises writer transactions
        // independently of schema creation.
        drop(open(&cfg).expect("initialize"));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let handles: Vec<_> = (0..4)
            .map(|writer| {
                let cfg = cfg.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let commands: Vec<_> = (0..20).map(|i| format!("grep_{writer}_{i}")).collect();
                    let inputs: Vec<_> = commands
                        .iter()
                        .map(|c| batch_input(b"seen\nhidden\n", c, 2))
                        .collect();
                    barrier.wait();
                    let results = store_batch(&cfg, &inputs);
                    assert!(results.iter().all(|s| matches!(s, Stored::Saved(_))));
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("writer");
        }
        let conn = open(&cfg).expect("open");
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM recall", [], |r| r.get::<_, i64>(0))
                .expect("count"),
            80
        );
        assert_eq!(
            stats_snapshot_with(&cfg)
                .expect("stats")
                .iter()
                .map(|s| s.elisions)
                .sum::<i64>(),
            80
        );
    }

    #[test]
    #[ignore = "manual timing: cargo test --release --lib test_benchmark_recall_batch_phases -- --ignored --nocapture"]
    fn test_benchmark_recall_batch_phases() {
        let payloads: Vec<_> = (0..100).map(|file| (0..80)
            .map(|line| format!("corpus/file_{file:04}.txt:{line}:needle file={file:04} line={line:04} deterministic search benchmark payload\n"))
            .collect::<String>()).collect();
        let commands: Vec<_> = (0..100).map(|i| format!("grep_{i}_file")).collect();
        for compression in [false, true] {
            let dir = tempfile::tempdir().expect("tempdir");
            let cfg = RetrieverConfig {
                compression,
                ..temp_cfg(dir.path())
            };
            let mut prepare_us = Vec::new();
            let mut write_us = Vec::new();
            for iteration in 0..23 {
                let start = std::time::Instant::now();
                let entries: Vec<_> = payloads
                    .iter()
                    .zip(&commands)
                    .map(|(body, command)| {
                        PreparedSlot::Entry(
                            prepare_entry(&cfg, batch_input(body.as_bytes(), command, 26))
                                .expect("prepare"),
                        )
                    })
                    .collect();
                let prepared = start.elapsed().as_micros();
                let start = std::time::Instant::now();
                assert!(
                    write_batch(&cfg, &entries)
                        .expect("batch")
                        .iter()
                        .all(|s| matches!(s, Stored::Saved(_)))
                );
                if iteration >= 3 {
                    prepare_us.push(prepared);
                    write_us.push(start.elapsed().as_micros());
                }
            }
            prepare_us.sort_unstable();
            write_us.sort_unstable();
            eprintln!(
                "100-file batch compression={compression}: preparation median={} us; transaction median={} us",
                prepare_us[10], write_us[10]
            );
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

    #[cfg(feature = "engine")]
    #[test]
    fn family_rotation_prunes_capture_metadata_without_resurrection() {
        let dir = tempfile::tempdir().expect("directory");
        let cfg = temp_cfg(dir.path());
        store_inner(&cfg, b"first\noutput\n", "posthook:read", None, 1).expect("first");
        let conn = open(&cfg).expect("db");
        let first_capture: String = conn
            .query_row("SELECT capture_id FROM recall_captures LIMIT 1", [], |r| {
                r.get(0)
            })
            .expect("capture");
        drop(conn);
        store_inner(&cfg, b"second\noutput\n", "posthook:read", None, 1).expect("second");
        store_inner(&cfg, b"failure\n", "cargo", Some(1), 1).expect("unrelated");
        trim_family(&cfg, "posthook:", 1);
        let conn = open(&cfg).expect("rotated db");
        let captures: i64 = conn
            .query_row("SELECT COUNT(*) FROM recall_captures", [], |r| r.get(0))
            .expect("remaining captures");
        assert_eq!(captures, 2, "rotation prunes only evicted captures");
        let present = |conn: &Connection| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM recall_captures WHERE capture_id = ?1",
                [&first_capture],
                |r| r.get(0),
            )
            .expect("old capture count")
        };
        assert_eq!(present(&conn), 0);
        drop(conn);
        store_inner(&cfg, b"first\noutput\n", "posthook:read", None, 1).expect("recapture");
        let conn = open(&cfg).expect("db after recapture");
        assert_eq!(present(&conn), 0, "an expired capture must not return");
    }

    #[test]
    fn equal_payloads_have_distinct_immutable_capture_ids() {
        let dir = tempfile::tempdir().expect("directory");
        let cfg = temp_cfg(dir.path());
        let first = store_inner(&cfg, b"same\noutput\n", "cmd", None, 1).expect("store");
        let second = store_inner(&cfg, b"same\noutput\n", "cmd", None, 2).expect("store");
        assert_eq!(first.hash, second.hash);
        let conn = open(&cfg).expect("db");
        let distinct: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT capture_id) FROM recall_captures",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(distinct, 2);
        let source: Option<String> = conn
            .query_row(
                "SELECT invocation_id FROM recall_captures LIMIT 1",
                [],
                |r| r.get(0),
            )
            .expect("capture source");
        assert_eq!(source, super::super::invocation::current().id);
    }

    #[test]
    fn capture_metadata_is_bounded_and_prunes_payload_orphans() {
        let conn = Connection::open_in_memory().expect("db");
        init_schema(&conn).expect("schema");
        conn.execute(
            "INSERT INTO recall VALUES ('hash','cmd','',0,0,1,1,1,0,'raw',X'61',0)",
            [],
        )
        .expect("payload");
        for i in 0..MAX_CAPTURE_RECORDS + 5 {
            conn.execute(
                "INSERT INTO recall_captures (capture_id, invocation_id, hash, created_at) VALUES (?1,NULL,'hash',0)",
                [i.to_string()],
            )
            .expect("capture");
        }
        prune_captures(&conn).expect("prune");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM recall_captures", [], |r| r.get(0))
            .expect("count");
        assert_eq!(count, MAX_CAPTURE_RECORDS);
        conn.execute("DELETE FROM recall", [])
            .expect("delete payload");
        prune_captures(&conn).expect("orphans");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM recall_captures", [], |r| r.get(0))
            .expect("count");
        assert_eq!(count, 0);
    }

    #[test]
    fn emission_counts_partial_and_non_utf8_bytes() {
        struct FailsAfterThree {
            written: usize,
        }
        impl Write for FailsAfterThree {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.written == 3 {
                    return Err(std::io::ErrorKind::BrokenPipe.into());
                }
                let n = bytes.len().min(3 - self.written);
                self.written += n;
                Ok(n)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (bytes, result) = emit_recall(&mut FailsAfterThree { written: 0 }, b"abcdef");
        assert_eq!(bytes, 3);
        assert!(result.is_err());
        let mut out = Vec::new();
        let (bytes, result) = emit_recall(&mut out, b"a\xff\n");
        assert_eq!(bytes, 3);
        assert!(result.is_ok());
        assert_eq!(out, b"a\xff\n");
    }

    #[test]
    fn regression_same_output_keeps_original_offset() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cfg = temp_cfg(dir.path());
        let data = b"first\nsecond\nthird\nfourth\n";
        let original = store_inner(&cfg, data, "same", Some(1), 2).expect("first capture");
        let _later = store_inner(&cfg, data, "same", Some(2), 4).expect("later capture");
        let conn = open(&cfg).expect("open");
        let row = load_reference(&conn, &original.reference)
            .expect("lookup")
            .expect("row");
        assert_eq!(
            slice_from_line(&decode(&row).expect("decode"), row.shown_upto),
            b"second\nthird\nfourth\n"
        );
    }

    #[test]
    fn regression_same_output_keeps_original_capped_bytes() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut cfg = temp_cfg(dir.path());
        cfg.max_entry_bytes = 13;
        let data = b"first\nsecond\nthird\nfourth\n";
        let original = store_inner(&cfg, data, "same", Some(1), 2).expect("first capture");
        cfg.max_entry_bytes = 7;
        let _later = store_inner(&cfg, data, "same", Some(2), 4).expect("later capture");
        let conn = open(&cfg).expect("open");
        let row = load_reference(&conn, &original.reference)
            .expect("lookup")
            .expect("row");
        assert_eq!(decode(&row).expect("decode"), b"second\nthird\n");
    }

    #[test]
    fn capture_snapshots_metadata_and_invocation_across_deduplicated_payloads() {
        let dir = tempfile::tempdir().expect("directory");
        let cfg = temp_cfg(dir.path());
        let cwd_a = tempfile::tempdir_in(super::super::test_isolation::root()).expect("cwd a");
        let cwd_b = tempfile::tempdir_in(super::super::test_isolation::root()).expect("cwd b");
        let first = {
            let _cwd = super::super::test_isolation::enter(cwd_a.path());
            super::super::invocation::initialize(None);
            store_inner(&cfg, b"a\nb\nc\n", "same", Some(1), 2).expect("first")
        };
        let first_invocation = super::super::invocation::current().id;
        let second = {
            let _cwd = super::super::test_isolation::enter(cwd_b.path());
            super::super::invocation::initialize(None);
            store_inner(&cfg, b"a\nb\nc\n", "same", Some(2), 3).expect("second")
        };
        assert_eq!(first.hash, second.hash);
        assert_ne!(first.reference, second.reference);
        let conn = open(&cfg).expect("db");
        let row = load_reference(&conn, &first.reference)
            .expect("lookup")
            .expect("first");
        assert_eq!(row.invocation_id, first_invocation);
        assert_eq!(row.shown_upto, 2);
        let metadata: (String, i32, i64, i64, bool) = conn
            .query_row(
                "SELECT cwd, exit_code, total_lines, byte_size, truncated FROM recall_captures
             WHERE capture_id = ?1",
                [&first.reference[2..]],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .expect("snapshot");
        assert_eq!(
            metadata,
            (cwd_a.path().to_string_lossy().into_owned(), 1, 3, 6, false)
        );
    }

    #[test]
    fn capped_and_uncapped_captures_can_share_bytes_without_sharing_metadata() {
        let dir = tempfile::tempdir().expect("directory");
        let mut cfg = temp_cfg(dir.path());
        cfg.max_entry_bytes = 4;
        let first = store_inner(&cfg, b"x\na\nb\n", "same", None, 2).expect("capped");
        cfg.compression = false;
        let second = store_inner(&cfg, b"a\nb\n", "same", None, 2).expect("uncapped");
        assert_eq!(
            first.hash, second.hash,
            "keys describe saved bytes, not the source"
        );
        let conn = open(&cfg).expect("db");
        let a = load_reference(&conn, &first.reference)
            .expect("lookup")
            .expect("first");
        let b = load_reference(&conn, &second.reference)
            .expect("lookup")
            .expect("second");
        assert!(a.truncated);
        assert!(!b.truncated);
        assert_eq!((a.shown_upto, b.shown_upto), (1, 2));
        assert_eq!(decode(&a).expect("decode"), b"a\nb\n");
        assert_eq!(decode(&a).expect("decode"), decode(&b).expect("decode"));
    }

    #[test]
    fn evicted_capture_never_revives_when_identical_bytes_return() {
        let dir = tempfile::tempdir().expect("directory");
        let cfg = RetrieverConfig {
            max_entries: 1,
            ..temp_cfg(dir.path())
        };
        let first = store_inner(&cfg, b"same\n", "cmd", None, 1).expect("first");
        store_inner(&cfg, b"different\n", "cmd", None, 1).expect("evict first");
        let conn = open(&cfg).expect("db");
        assert!(
            load_reference(&conn, &first.reference)
                .expect("lookup")
                .is_none()
        );
        let later = store_inner(&cfg, b"same\n", "cmd", None, 1).expect("recapture");
        assert_eq!(first.hash, later.hash);
        assert!(
            load_reference(&conn, &first.reference)
                .expect("lookup")
                .is_none()
        );
        assert!(
            load_reference(&conn, &later.reference)
                .expect("lookup")
                .is_some()
        );
    }

    #[test]
    fn maintenance_eviction_does_not_reattach_orphaned_captures() {
        let dir = tempfile::tempdir().expect("directory");
        let cfg = temp_cfg(dir.path());
        let first = store_inner(&cfg, b"same\n", "cmd", None, 1).expect("first");
        let conn = open(&cfg).expect("db");
        conn.execute("DELETE FROM recall", [])
            .expect("maintenance eviction");
        assert!(
            load_reference(&conn, &first.reference)
                .expect("lookup")
                .is_none()
        );
        store_inner(&cfg, b"same\n", "cmd", None, 1).expect("recapture");
        assert!(
            load_reference(&conn, &first.reference)
                .expect("lookup")
                .is_none()
        );
    }

    #[test]
    fn captures_require_exact_namespaced_ids() {
        let dir = tempfile::tempdir().expect("directory");
        let cfg = temp_cfg(dir.path());
        let stored = store_inner(&cfg, b"x\n", "cmd", None, 1).expect("capture");
        let conn = open(&cfg).expect("db");
        assert!(load_reference(&conn, &stored.reference[..10]).is_err());
        assert!(
            load_reference(&conn, &stored.reference[2..])
                .expect("lookup")
                .is_none()
        );
        assert!(
            load_reference(&conn, &stored.reference)
                .expect("lookup")
                .is_some()
        );
    }

    #[test]
    fn capture_failure_rolls_back_payload_and_stats() {
        let dir = tempfile::tempdir().expect("directory");
        let cfg = temp_cfg(dir.path());
        let conn = open(&cfg).expect("db");
        conn.execute_batch(
            "CREATE TRIGGER reject_capture BEFORE INSERT ON recall_captures
                            BEGIN SELECT RAISE(FAIL, 'capture failure'); END;",
        )
        .expect("trigger");
        assert!(matches!(
            store(&cfg, b"data\n", "cmd", None, 1),
            Stored::Unavailable
        ));
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM recall", [], |r| r.get(0))
            .expect("count");
        assert_eq!(count, 0);
        assert!(stats_snapshot_with(&cfg).expect("stats").is_empty());
    }

    #[test]
    fn migration_preserves_legacy_rows_and_does_not_invent_capture_snapshots() {
        let dir = tempfile::tempdir().expect("directory");
        let cfg = RetrieverConfig {
            retention_days: 0,
            ..temp_cfg(dir.path())
        };
        let conn = open(&cfg).expect("db");
        insert_row(&conn, "abc123def456", "legacy");
        conn.execute_batch(
            "DROP TABLE recall_captures;
            CREATE TABLE recall_captures (capture_id TEXT PRIMARY KEY, invocation_id TEXT,
                                         hash TEXT NOT NULL, created_at INTEGER NOT NULL);",
        )
        .expect("old schema");
        let id = "0123456789abcdef0123456789abcdef";
        conn.execute(
            "INSERT INTO recall_captures VALUES (?1, NULL, 'abc123def456', 0)",
            [id],
        )
        .expect("old capture");
        init_schema(&conn).expect("migrate");
        init_schema(&conn).expect("idempotent migration");
        assert!(
            load_reference(&conn, &format!("c_{id}"))
                .expect("lookup")
                .is_none()
        );
        let legacy = load_reference(&conn, "abc123")
            .expect("lookup")
            .expect("legacy");
        assert_eq!(decode(&legacy).expect("decode"), b"a");
        assert!(legacy.capture_id.is_none());
        assert!(legacy.invocation_id.is_none());
        store_inner(&cfg, b"a", "legacy", None, 1).expect("new capture");
        let legacy = load_reference(&conn, "abc123def456")
            .expect("lookup")
            .expect("legacy");
        assert_eq!(decode(&legacy).expect("decode"), b"a");
        let old_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM recall_captures WHERE capture_id = ?1 AND command IS NULL",
                [id],
                |r| r.get(0),
            )
            .expect("old capture");
        assert_eq!(old_count, 1);
    }

    #[test]
    fn migration_serializes_concurrent_accounting_schema_openers() {
        let dir = tempfile::tempdir().expect("directory");
        let cfg = temp_cfg(dir.path());
        let conn = open(&cfg).expect("db");
        conn.execute_batch(
            "DROP TABLE recall_captures;
            CREATE TABLE recall_captures (capture_id TEXT PRIMARY KEY, invocation_id TEXT,
                                         hash TEXT NOT NULL, created_at INTEGER NOT NULL);",
        )
        .expect("old schema");
        drop(conn);
        let path = cfg.database_path.expect("path");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(12));
        let threads: Vec<_> = (0..12)
            .map(|_| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let conn = Connection::open(path).expect("open thread db");
                    conn.busy_timeout(std::time::Duration::from_secs(5))
                        .expect("busy timeout");
                    barrier.wait();
                    init_schema(&conn).map_err(|error| format!("{error:#}"))
                })
            })
            .collect();
        for thread in threads {
            thread
                .join()
                .expect("thread")
                .unwrap_or_else(|error| panic!("simultaneous migration must succeed: {error}"));
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
        evict(&conn, &cfg, &["aaa111111111"]).unwrap();
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

    fn stats_count(conn: &Connection) -> usize {
        conn.query_row("SELECT COUNT(*) FROM recall_stats", [], |r| r.get(0))
            .expect("count statistics")
    }

    fn stats_counts(conn: &Connection, slug: &str, mode: &str) -> Option<(i64, i64)> {
        conn.query_row(
            "SELECT elisions, recalls FROM recall_stats WHERE slug = ?1 AND mode = ?2",
            params![slug, mode],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .expect("read statistics")
    }

    #[test]
    fn test_stats_cap_bounds_dynamic_command_names() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let cfg = RetrieverConfig {
            compression: false,
            ..temp_cfg(dir.path())
        };
        let conn = open(&cfg).expect("open store");
        for i in 0..MAX_STATS_ROWS * 2 {
            // Digits after a letter bypass stat_family's invocation suffix rule.
            let slug = format!("cargo_task{i}");
            assert_eq!(stat_key(&slug), slug);
            store_inner(&cfg, b"output\n", &slug, Some(0), 1).expect("store output");
            assert_eq!(stats_count(&conn), (i + 1).min(MAX_STATS_ROWS));
        }
        assert!(stats_counts(&conn, "cargo_task0", "sqlite").is_none());
        assert_eq!(
            stats_counts(&conn, &format!("cargo_task{}", MAX_STATS_ROWS), "sqlite"),
            Some((1, 0)),
            "equal-use ties discard the oldest insertion first"
        );
    }

    #[test]
    fn test_stats_cap_preserves_frequent_filters_and_separate_modes() {
        let conn = Connection::open_in_memory().expect("open store");
        init_schema(&conn).expect("initialize schema");
        for mode in ["sqlite", "tee"] {
            for _ in 0..5 {
                bump_stat(&conn, "cargo_test", mode, "elisions");
            }
            bump_stat(&conn, "cargo_test", mode, "recalls");
        }
        for i in 0..MAX_STATS_ROWS * 2 {
            let mode = if i % 2 == 0 { "sqlite" } else { "tee" };
            bump_stat(&conn, &format!("bun_task{i}"), mode, "elisions");
        }
        for mode in ["sqlite", "tee"] {
            assert_eq!(stats_counts(&conn, "cargo_test", mode), Some((5, 1)));
            for _ in 0..10 {
                bump_stat(&conn, "cargo_test", mode, "elisions");
                bump_stat(&conn, "cargo_test", mode, "recalls");
            }
            assert_eq!(stats_counts(&conn, "cargo_test", mode), Some((15, 11)));
        }
        assert_eq!(stats_count(&conn), MAX_STATS_ROWS, "one cap across modes");
    }

    #[test]
    fn test_stats_cap_keeps_new_filter_even_when_existing_counts_are_higher() {
        let conn = Connection::open_in_memory().expect("open store");
        init_schema(&conn).expect("initialize schema");
        for i in 0..MAX_STATS_ROWS {
            let slug = format!("deno_task{i}");
            bump_stat(&conn, &slug, "sqlite", "elisions");
            bump_stat(&conn, &slug, "sqlite", "elisions");
        }
        bump_stat(&conn, "new_filter", "tee", "recalls");
        assert_eq!(stats_count(&conn), MAX_STATS_ROWS);
        assert_eq!(stats_counts(&conn, "new_filter", "tee"), Some((0, 1)));
        assert!(stats_counts(&conn, "deno_task0", "sqlite").is_none());
        assert_eq!(stats_counts(&conn, "deno_task1", "sqlite"), Some((2, 0)));
    }

    #[test]
    fn test_stats_cap_trims_existing_oversized_store_on_open() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let cfg = temp_cfg(dir.path());
        let conn = open(&cfg).expect("open store");
        insert_row(&conn, "aaa111111111", "cargo_test");
        conn.execute_batch("DROP TRIGGER recall_stats_cap")
            .expect("simulate schema before stats cap");
        for i in 0..MAX_STATS_ROWS + 20 {
            bump_stat(&conn, &format!("cargo_task{i}"), "sqlite", "elisions");
        }
        bump_stat(&conn, "cargo_task0", "sqlite", "recalls");
        drop(conn);

        let stats = stats_snapshot_with(&cfg).expect("open existing store for snapshot");
        assert_eq!(stats.len(), MAX_STATS_ROWS);
        let conn = open(&cfg).expect("reopen store");
        assert_eq!(stats_counts(&conn, "cargo_task0", "sqlite"), Some((1, 1)));
        assert!(stats_counts(&conn, "cargo_task20", "sqlite").is_none());
        assert_eq!(stats_counts(&conn, "cargo_task21", "sqlite"), Some((1, 0)));
        assert!(
            load_by_hash(&conn, "aaa111111111")
                .expect("recall row")
                .is_some()
        );
        bump_stat(&conn, "new_filter", "tee", "elisions");
        assert_eq!(
            stats_count(&conn),
            MAX_STATS_ROWS,
            "trigger restored on open"
        );
    }

    #[test]
    fn test_stats_cap_rolls_back_with_callers_transaction() {
        let conn = Connection::open_in_memory().expect("open store");
        init_schema(&conn).expect("initialize schema");
        for i in 0..MAX_STATS_ROWS {
            bump_stat(&conn, &format!("cargo_task{i}"), "sqlite", "elisions");
        }
        let tx = conn
            .unchecked_transaction()
            .expect("begin caller transaction");
        bump_stat(&tx, "new_filter", "tee", "elisions");
        assert_eq!(stats_count(&tx), MAX_STATS_ROWS);
        assert!(stats_counts(&tx, "cargo_task0", "sqlite").is_none());
        tx.rollback().expect("roll back caller transaction");
        assert_eq!(stats_count(&conn), MAX_STATS_ROWS);
        assert_eq!(stats_counts(&conn, "cargo_task0", "sqlite"), Some((1, 0)));
        assert!(stats_counts(&conn, "new_filter", "tee").is_none());
    }

    #[test]
    fn test_stats_cap_concurrent_connections_preserve_updates_and_bound() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let cfg = temp_cfg(dir.path());
        let conn = open(&cfg).expect("open store");
        bump_stat(&conn, "cargo_test", "sqlite", "elisions");
        bump_stat(&conn, "cargo_test", "tee", "elisions");
        let barrier = std::sync::Barrier::new(4);
        std::thread::scope(|scope| {
            for worker in 0..4 {
                let cfg = &cfg;
                let barrier = &barrier;
                scope.spawn(move || {
                    let mut conn = open(cfg).expect("open worker connection");
                    // Exercise competing writers without thousands of durable
                    // autocommits exhausting the production best-effort timeout
                    // on slower Windows runners. Retention must compose with a
                    // caller-owned transaction and preserve its committed counts.
                    conn.busy_timeout(std::time::Duration::from_secs(60))
                        .expect("set test contention timeout");
                    barrier.wait();
                    let tx = conn
                        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                        .expect("serialize worker transaction");
                    for i in 0..MAX_STATS_ROWS / 2 {
                        for mode in ["sqlite", "tee"] {
                            bump_stat(&tx, "cargo_test", mode, "elisions");
                            bump_stat(&tx, "cargo_test", mode, "recalls");
                        }
                        let slug = format!("cargo_taskw{worker}x{i}");
                        bump_stat(&tx, &slug, "sqlite", "elisions");
                        assert!(stats_count(&tx) <= MAX_STATS_ROWS);
                    }
                    tx.commit().expect("commit worker statistics");
                });
            }
        });
        assert_eq!(stats_count(&conn), MAX_STATS_ROWS);
        for mode in ["sqlite", "tee"] {
            assert_eq!(
                stats_counts(&conn, "cargo_test", mode),
                Some(((MAX_STATS_ROWS * 2 + 1) as i64, (MAX_STATS_ROWS * 2) as i64)),
                "concurrent upserts must retain both counters"
            );
        }
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
        let dir = tempfile::tempdir().unwrap();
        let cfg = temp_cfg(dir.path());
        user_env::with_vars(&[("RTK_RECALL", Some("0"))], || {
            record_tee_recall_with(&cfg, "grep", "/tee/1_grep.log");
        });
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
