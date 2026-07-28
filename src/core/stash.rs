//! Content-addressed output stash — park full command output behind a short
//! handle so the agent reads a compact preview and pulls the full bytes only
//! when it actually needs them.
//!
//! # Architecture
//!
//! - Blobs: gzip-compressed, content-addressed (SHA-256), sharded on disk at
//!   `~/.local/share/rtk/stash/<hash[:2]>/<hash>.gz`. Identical content hashes
//!   to the same blob, so re-stashing the same output is free (dedup).
//! - Index: a `stash` table in the shared `history.db` (same DB as tracking).
//! - Retrieval: `rtk retrieve <handle>` resolves a hash prefix (git-style),
//!   decompresses, and prints — optionally sliced/grepped so the agent can
//!   re-interrogate a parked blob without pulling the whole thing.
//! - Lifecycle: retention-day + LRU byte-cap garbage collection, plus pruning
//!   of rows whose blob has gone missing.
//!
//! The write path is split in two:
//! - [`auto_stash`] — config-gated, error-swallowing helper wired into the tee
//!   hint functions so every existing recovery hint gains a recall handle.
//! - [`StashStore::put`] — the explicit sink behind `rtk stash`.

use super::constants::{RTK_DATA_DIR, STASH_DIR};
use crate::core::tracking::{estimate_tokens, get_db_path};
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Default short-handle length (hex chars of the SHA-256).
const DEFAULT_HANDLE_LEN: usize = 8;
/// Default minimum content size (bytes) for the automatic path to bother.
const DEFAULT_MIN_BYTES: usize = 2048;
/// Default retention window in days.
const DEFAULT_RETENTION_DAYS: i64 = 7;
/// Default blob-store byte cap (uncompressed accounting): 512 MiB.
const DEFAULT_MAX_BYTES: u64 = 536_870_912;

/// Configuration for the stash feature (mirrors [`TeeConfig`](super::tee::TeeConfig)).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StashConfig {
    /// Master switch for the whole feature (`rtk stash`, `rtk retrieve`, auto).
    pub enabled: bool,
    /// Whether the tee hint functions record an auto-stash + recall handle.
    pub auto: bool,
    /// Automatic path skips content smaller than this (bytes).
    pub min_bytes: usize,
    /// Delete blobs older than this many days.
    pub retention_days: i64,
    /// LRU-evict blobs once the store exceeds this many (uncompressed) bytes.
    pub max_bytes: u64,
    /// Short-handle length in hex chars.
    pub handle_len: usize,
    /// Override the blob directory (defaults to `~/.local/share/rtk/stash`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory: Option<PathBuf>,
}

impl Default for StashConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto: true,
            min_bytes: DEFAULT_MIN_BYTES,
            retention_days: DEFAULT_RETENTION_DAYS,
            max_bytes: DEFAULT_MAX_BYTES,
            handle_len: DEFAULT_HANDLE_LEN,
            directory: None,
        }
    }
}

/// Result of a successful [`StashStore::put`].
#[derive(Debug, Clone)]
pub struct StashEntry {
    /// Short handle (hash prefix) the agent uses with `rtk retrieve`.
    pub handle: String,
    /// Uncompressed byte length.
    pub bytes: usize,
    /// Estimated token count of the parked content.
    pub tokens: usize,
    /// True when identical content was already stored (no new blob written).
    pub deduped: bool,
}

/// One indexed stash row.
#[derive(Debug, Clone)]
pub struct StashRow {
    pub hash: String,
    pub created: String,
    pub command: String,
    pub content_type: String,
    pub bytes: usize,
    pub tokens: usize,
    pub path: PathBuf,
    pub last_accessed: String,
    pub access_count: i64,
}

impl StashRow {
    /// Short handle for display (hash prefix).
    pub fn handle(&self, handle_len: usize) -> String {
        let n = handle_len.min(self.hash.len());
        self.hash[..n].to_string()
    }
}

/// Outcome of a [`StashStore::gc`] run.
#[derive(Debug, Default, Clone)]
pub struct GcStats {
    /// Rows removed because they aged past the retention window.
    pub expired: usize,
    /// Rows removed by the LRU byte-cap.
    pub evicted: usize,
    /// Rows removed because their blob file had vanished.
    pub pruned_missing: usize,
    /// Total bytes (uncompressed accounting) remaining after GC.
    pub remaining_bytes: u64,
}

/// Handle to the content-addressed stash (index + blob store).
pub struct StashStore {
    conn: Connection,
    blob_dir: PathBuf,
    cfg: StashConfig,
}

impl StashStore {
    /// Open (or create) the stash store, honoring config + env overrides.
    pub fn open() -> Result<Self> {
        let cfg = super::config::Config::load()
            .map(|c| c.stash)
            .unwrap_or_default();
        Self::open_with(cfg)
    }

    /// Open with an explicit config (used by tests and the auto path).
    pub fn open_with(cfg: StashConfig) -> Result<Self> {
        let db_path = get_db_path()?;
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(&db_path).context("open stash DB")?;
        // Match tracking: WAL + busy_timeout for concurrent Claude Code instances.
        let _ = conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA busy_timeout=5000;",
        );
        let blob_dir = resolve_blob_dir(&cfg);
        let store = Self {
            conn,
            blob_dir,
            cfg,
        };
        store.ensure_schema()?;
        Ok(store)
    }

    #[cfg(test)]
    fn open_in(dir: &Path, cfg: StashConfig) -> Result<Self> {
        let conn = Connection::open(dir.join("history.db"))?;
        let store = Self {
            conn,
            blob_dir: dir.join("stash"),
            cfg,
        };
        store.ensure_schema()?;
        Ok(store)
    }

    fn ensure_schema(&self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS stash (
                hash          TEXT PRIMARY KEY,
                created       TEXT NOT NULL,
                command       TEXT NOT NULL DEFAULT '',
                content_type  TEXT NOT NULL DEFAULT '',
                bytes         INTEGER NOT NULL,
                tokens        INTEGER NOT NULL,
                path          TEXT NOT NULL,
                last_accessed TEXT NOT NULL,
                access_count  INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_stash_created ON stash(created)",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_stash_last_accessed ON stash(last_accessed)",
            [],
        )?;
        Ok(())
    }

    /// The configured short-handle length.
    pub fn handle_len(&self) -> usize {
        self.cfg.handle_len
    }

    fn blob_path(&self, hash: &str) -> PathBuf {
        // git-style two-char shard to keep any single dir small.
        let shard = &hash[..2.min(hash.len())];
        self.blob_dir.join(shard).join(format!("{hash}.gz"))
    }

    /// Park `content`, returning its handle. Deduplicates by content hash.
    pub fn put(&self, content: &str, command: &str, content_type: &str) -> Result<StashEntry> {
        let bytes = content.len();
        let tokens = estimate_tokens(content);
        let hash = crate::hooks::integrity::compute_hash_bytes(content.as_bytes());
        let handle = hash[..self.cfg.handle_len.min(hash.len())].to_string();
        let now = Utc::now().to_rfc3339();
        let blob = self.blob_path(&hash);

        let already: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM stash WHERE hash = ?1)",
                params![hash],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if already {
            // Refresh recency; heal a blob that was deleted out from under us.
            if !blob.exists() {
                write_blob(&blob, content)?;
            }
            self.conn.execute(
                "UPDATE stash SET last_accessed = ?1, access_count = access_count + 1 WHERE hash = ?2",
                params![now, hash],
            )?;
            return Ok(StashEntry {
                handle,
                bytes,
                tokens,
                deduped: true,
            });
        }

        write_blob(&blob, content)?;
        let ct = if content_type.is_empty() {
            detect_content_type(content)
        } else {
            content_type
        };
        self.conn.execute(
            "INSERT INTO stash
                (hash, created, command, content_type, bytes, tokens, path, last_accessed, access_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0)",
            params![
                hash,
                now,
                command,
                ct,
                bytes as i64,
                tokens as i64,
                blob.to_string_lossy(),
                now,
            ],
        )?;

        // Opportunistic housekeeping — never fatal.
        let _ = self.gc();

        Ok(StashEntry {
            handle,
            bytes,
            tokens,
            deduped: false,
        })
    }

    /// Resolve a handle prefix to exactly one row (git-style ambiguity rules).
    pub fn resolve(&self, prefix: &str) -> Result<StashRow> {
        let prefix = prefix.trim();
        if prefix.is_empty() {
            return Err(anyhow!("empty handle"));
        }
        if !prefix.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(anyhow!("'{prefix}' is not a valid handle (hex expected)"));
        }
        let like = format!("{prefix}%");
        let mut stmt = self.conn.prepare(
            "SELECT hash, created, command, content_type, bytes, tokens, path, last_accessed, access_count
             FROM stash WHERE hash LIKE ?1 ORDER BY hash LIMIT 8",
        )?;
        let rows: Vec<StashRow> = stmt
            .query_map(params![like], row_to_stash)?
            .collect::<Result<Vec<_>, _>>()?;

        match rows.len() {
            0 => Err(anyhow!(
                "no stash entry matches handle '{prefix}' (it may have been evicted — try `rtk stash --list`)"
            )),
            1 => Ok(rows.into_iter().next().unwrap()),
            n => {
                let opts: Vec<String> = rows
                    .iter()
                    .map(|r| format!("  {} ({})", r.handle(self.cfg.handle_len.max(12)), r.command))
                    .collect();
                Err(anyhow!(
                    "handle '{prefix}' is ambiguous ({n} matches):\n{}",
                    opts.join("\n")
                ))
            }
        }
    }

    /// Resolve, read, decompress, and bump access counters. Returns (row, content).
    pub fn retrieve(&self, prefix: &str) -> Result<(StashRow, String)> {
        let mut row = self.resolve(prefix)?;
        let content = read_blob(&row.path).with_context(|| {
            format!(
                "blob for {} is unreadable (evicted?) — `rtk stash --gc` to clean the index",
                row.handle(self.cfg.handle_len)
            )
        })?;
        let now = Utc::now().to_rfc3339();
        let _ = self.conn.execute(
            "UPDATE stash SET last_accessed = ?1, access_count = access_count + 1 WHERE hash = ?2",
            params![now, row.hash],
        );
        row.last_accessed = now;
        row.access_count += 1;
        Ok((row, content))
    }

    /// Newest-first listing, capped at `limit`.
    pub fn list(&self, limit: usize) -> Result<Vec<StashRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT hash, created, command, content_type, bytes, tokens, path, last_accessed, access_count
             FROM stash ORDER BY created DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], row_to_stash)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Retention + LRU + missing-blob garbage collection.
    pub fn gc(&self) -> Result<GcStats> {
        let mut stats = GcStats::default();

        // 1. Prune rows whose blob has gone missing (and thus can't be retrieved).
        {
            let mut stmt = self.conn.prepare("SELECT hash, path FROM stash")?;
            let missing: Vec<String> = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .filter_map(|r| r.ok())
                .filter(|(_, p)| !Path::new(p).exists())
                .map(|(h, _)| h)
                .collect();
            for hash in &missing {
                self.conn
                    .execute("DELETE FROM stash WHERE hash = ?1", params![hash])?;
            }
            stats.pruned_missing = missing.len();
        }

        // 2. Retention window.
        let cutoff = (Utc::now() - chrono::Duration::days(self.cfg.retention_days)).to_rfc3339();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT hash, path FROM stash WHERE created < ?1")?;
            let expired: Vec<(String, String)> = stmt
                .query_map(params![cutoff], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .filter_map(|r| r.ok())
                .collect();
            for (hash, path) in &expired {
                remove_blob(path);
                self.conn
                    .execute("DELETE FROM stash WHERE hash = ?1", params![hash])?;
            }
            stats.expired = expired.len();
        }

        // 3. LRU byte-cap: evict least-recently-accessed until under max_bytes.
        let mut total: u64 = self
            .conn
            .query_row("SELECT COALESCE(SUM(bytes), 0) FROM stash", [], |r| {
                r.get(0)
            })
            .unwrap_or(0i64) as u64;
        if total > self.cfg.max_bytes {
            let mut stmt = self
                .conn
                .prepare("SELECT hash, path, bytes FROM stash ORDER BY last_accessed ASC")?;
            let candidates: Vec<(String, String, u64)> = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)? as u64,
                    ))
                })?
                .filter_map(|r| r.ok())
                .collect();
            for (hash, path, bytes) in candidates {
                if total <= self.cfg.max_bytes {
                    break;
                }
                remove_blob(&path);
                self.conn
                    .execute("DELETE FROM stash WHERE hash = ?1", params![hash])?;
                total = total.saturating_sub(bytes);
                stats.evicted += 1;
            }
        }

        stats.remaining_bytes = total;
        Ok(stats)
    }
}

fn row_to_stash(row: &rusqlite::Row) -> rusqlite::Result<StashRow> {
    Ok(StashRow {
        hash: row.get(0)?,
        created: row.get(1)?,
        command: row.get(2)?,
        content_type: row.get(3)?,
        bytes: row.get::<_, i64>(4)? as usize,
        tokens: row.get::<_, i64>(5)? as usize,
        path: PathBuf::from(row.get::<_, String>(6)?),
        last_accessed: row.get(7)?,
        access_count: row.get(8)?,
    })
}

fn resolve_blob_dir(cfg: &StashConfig) -> PathBuf {
    if let Ok(dir) = std::env::var("RTK_STASH_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(ref dir) = cfg.directory {
        return dir.clone();
    }
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(RTK_DATA_DIR)
        .join(STASH_DIR)
}

fn write_blob(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    }
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(content.as_bytes())?;
    let compressed = encoder.finish()?;
    // Write to a temp sibling then rename for atomicity.
    let tmp = path.with_extension("gz.tmp");
    std::fs::write(&tmp, &compressed).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename into {}", path.display()))?;
    Ok(())
}

fn read_blob(path: &Path) -> Result<String> {
    let compressed = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let mut decoder = flate2::read::GzDecoder::new(compressed.as_slice());
    let mut out = String::new();
    decoder.read_to_string(&mut out)?;
    Ok(out)
}

fn remove_blob(path: &str) {
    let _ = std::fs::remove_file(path);
}

/// Coarse content-type label — best-effort, for display only.
fn detect_content_type(content: &str) -> &'static str {
    let head = &content[..content.len().min(2048)];
    let trimmed = head.trim_start();
    if trimmed.starts_with("diff --git") || (head.contains("\n@@ ") && head.contains("\n+++ ")) {
        return "diff";
    }
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return "json";
    }
    if head.contains("test result:")
        || head.contains("=== test session starts")
        || head.contains("PASS")
        || head.contains("FAIL")
    {
        return "test";
    }
    // grep-style file:line:content on the first few lines.
    if head
        .lines()
        .take(4)
        .filter(|l| !l.trim().is_empty())
        .any(|l| {
            let parts: Vec<&str> = l.splitn(3, ':').collect();
            parts.len() == 3 && parts[1].parse::<usize>().is_ok()
        })
    {
        return "grep";
    }
    if head.contains("error[")
        || head.contains(": error:")
        || head.contains(" ERROR ")
        || head.contains(" WARN ")
    {
        return "log";
    }
    "text"
}

/// Config-gated, error-swallowing auto-stash used by the tee hint functions.
///
/// Returns the short recall handle when content was parked, or `None` when the
/// feature is off, the content is too small, or anything went wrong (the tee
/// hint must never fail because of stash).
pub fn auto_stash(content: &str, command_slug: &str) -> Option<String> {
    // Keep `cargo test` runs from writing real blobs via unrelated command tests
    // that exercise the tee hint functions.
    if cfg!(test) {
        return None;
    }
    if std::env::var("RTK_STASH").ok().as_deref() == Some("0") {
        return None;
    }
    let cfg = super::config::Config::load()
        .map(|c| c.stash)
        .unwrap_or_default();
    if !cfg.enabled || !cfg.auto {
        return None;
    }
    if content.len() < cfg.min_bytes {
        return None;
    }
    let store = StashStore::open_with(cfg).ok()?;
    store.put(content, command_slug, "").ok().map(|e| e.handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cfg() -> StashConfig {
        StashConfig {
            min_bytes: 1,
            ..StashConfig::default()
        }
    }

    #[test]
    fn put_and_retrieve_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = StashStore::open_in(dir.path(), test_cfg()).unwrap();
        let content = "hello world\n".repeat(100);
        let entry = store.put(&content, "grep foo", "").unwrap();
        assert!(!entry.deduped);
        assert_eq!(entry.handle.len(), DEFAULT_HANDLE_LEN);

        let (row, got) = store.retrieve(&entry.handle).unwrap();
        assert_eq!(got, content);
        assert_eq!(row.command, "grep foo");
        assert_eq!(row.access_count, 1);
    }

    #[test]
    fn dedup_same_content() {
        let dir = tempfile::tempdir().unwrap();
        let store = StashStore::open_in(dir.path(), test_cfg()).unwrap();
        let content = "x".repeat(500);
        let a = store.put(&content, "cmd-a", "").unwrap();
        let b = store.put(&content, "cmd-b", "").unwrap();
        assert_eq!(a.handle, b.handle);
        assert!(!a.deduped);
        assert!(b.deduped);
        // Only one row.
        assert_eq!(store.list(10).unwrap().len(), 1);
    }

    #[test]
    fn prefix_resolution_and_ambiguity() {
        let dir = tempfile::tempdir().unwrap();
        let store = StashStore::open_in(dir.path(), test_cfg()).unwrap();
        let e = store.put("some distinct content here", "c", "").unwrap();
        // Full handle resolves.
        assert!(store.resolve(&e.handle).is_ok());
        // Nonexistent prefix errors.
        assert!(store.resolve("ffffffff").is_err());
        // Non-hex errors.
        assert!(store.resolve("zzzz").is_err());
    }

    #[test]
    fn missing_blob_is_pruned_by_gc() {
        let dir = tempfile::tempdir().unwrap();
        let store = StashStore::open_in(dir.path(), test_cfg()).unwrap();
        let e = store.put("content to delete", "c", "").unwrap();
        let row = store.resolve(&e.handle).unwrap();
        std::fs::remove_file(&row.path).unwrap();
        let stats = store.gc().unwrap();
        assert_eq!(stats.pruned_missing, 1);
        assert!(store.resolve(&e.handle).is_err());
    }

    #[test]
    fn lru_eviction_respects_max_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = StashConfig {
            min_bytes: 1,
            max_bytes: 100,
            ..StashConfig::default()
        };
        let store = StashStore::open_in(dir.path(), cfg).unwrap();
        // Each ~60 bytes; two of them exceed the 100-byte cap.
        store.put(&"a".repeat(60), "first", "").unwrap();
        store.put(&"b".repeat(60), "second", "").unwrap();
        // gc runs opportunistically on put; at most one row should survive.
        let remaining = store.list(10).unwrap();
        assert!(remaining.len() <= 1, "expected LRU eviction under cap");
    }

    #[test]
    fn detect_content_type_variants() {
        assert_eq!(detect_content_type("diff --git a/x b/x\n"), "diff");
        assert_eq!(detect_content_type("{\"a\":1}"), "json");
        assert_eq!(detect_content_type("test result: ok. 1 passed"), "test");
        assert_eq!(detect_content_type("src/x.rs:42:boom"), "grep");
        assert_eq!(detect_content_type("just some prose"), "text");
    }
}
