//! Session-level dedup cache: SHA-256 persistent cross-command deduplication.
//! Repeat reads of the same content return a compact `§ref:HASH§` token (~13 tokens)
//! instead of recompressing the full output.

use anyhow::{Context, Result};
use rusqlite::{self, OptionalExtension};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub enum CacheResult {
    Ref { inline_ref: String },
    Fresh { compressed: String },
}

pub struct DedupCache {
    db_path: PathBuf,
    ttl_days: u64,
}

impl DedupCache {
    pub fn new(db_path: PathBuf) -> Result<Self> {
        let conn = rusqlite::Connection::open(&db_path)
            .with_context(|| format!("Failed to open dedup cache: {}", db_path.display()))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS dedup_cache (
                hash         TEXT PRIMARY KEY,
                compressed   TEXT NOT NULL,
                cmd          TEXT,
                created_at   INTEGER NOT NULL,
                accessed_at  INTEGER NOT NULL,
                access_count INTEGER DEFAULT 1
            );
            CREATE INDEX IF NOT EXISTS idx_dedup_accessed ON dedup_cache(accessed_at);",
        )
        .context("Failed to initialize dedup_cache table")?;
        Ok(Self {
            db_path,
            ttl_days: 7,
        })
    }

    /// Check the cache for `raw`. On hit return a compact ref token; on miss
    /// insert `compressed` and return it as Fresh.
    pub fn get_or_insert(&self, raw: &str, cmd: &str, compressed: &str) -> Result<CacheResult> {
        let hash = sha256_hex(raw);
        let short = &hash[..8];
        let conn =
            rusqlite::Connection::open(&self.db_path).context("Failed to open dedup cache")?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let existing: Option<String> = conn
            .query_row(
                "SELECT hash FROM dedup_cache WHERE hash = ?1",
                rusqlite::params![hash],
                |row| row.get(0),
            )
            .ok();

        if existing.is_some() {
            conn.execute(
                "UPDATE dedup_cache SET accessed_at=?1, access_count=access_count+1 WHERE hash=?2",
                rusqlite::params![now, hash],
            )
            .ok();
            return Ok(CacheResult::Ref {
                inline_ref: format!("§ref:{}§", short),
            });
        }

        conn.execute(
            "INSERT OR IGNORE INTO dedup_cache (hash, compressed, cmd, created_at, accessed_at)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            rusqlite::params![hash, compressed, cmd, now],
        )
        .context("Failed to insert into dedup cache")?;

        Ok(CacheResult::Fresh {
            compressed: compressed.to_string(),
        })
    }

    /// Remove entries not accessed in the last `ttl_days` days.
    pub fn evict_stale(&self) -> Result<usize> {
        let conn = rusqlite::Connection::open(&self.db_path)
            .context("Failed to open dedup cache for eviction")?;
        let cutoff = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
            - (self.ttl_days as i64 * 86400);
        let n = conn
            .execute(
                "DELETE FROM dedup_cache WHERE accessed_at < ?1",
                rusqlite::params![cutoff],
            )
            .context("Failed to evict stale cache entries")?;
        Ok(n)
    }

    /// Lookup by hash prefix (first 8 chars). Returns the stored compressed content or None.
    pub fn expand_prefix(&self, prefix: &str) -> Result<Option<String>> {
        let conn =
            rusqlite::Connection::open(&self.db_path).context("Failed to open dedup cache")?;
        let pattern = format!("{}%", prefix);
        conn.query_row(
            "SELECT compressed FROM dedup_cache WHERE hash LIKE ?1 LIMIT 1",
            rusqlite::params![pattern],
            |row| row.get(0),
        )
        .optional()
        .context("Failed to query dedup cache")
    }

    /// Return basic cache statistics.
    pub fn stats(&self) -> Result<CacheStats> {
        let conn =
            rusqlite::Connection::open(&self.db_path).context("Failed to open dedup cache")?;
        let count: usize = conn
            .query_row("SELECT COUNT(*) FROM dedup_cache", [], |r| r.get(0))
            .unwrap_or(0);
        let size_bytes: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(LENGTH(compressed)), 0) FROM dedup_cache",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        Ok(CacheStats {
            count,
            size_kb: size_bytes as f64 / 1024.0,
        })
    }
}

pub struct CacheStats {
    pub count: usize,
    pub size_kb: f64,
}

/// Compute the SHA-256 hex digest of a string.
pub fn sha256_hex(content: &str) -> String {
    let mut h = Sha256::new();
    h.update(content.as_bytes());
    format!("{:x}", h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_first_call_returns_fresh() {
        let dir = tempdir().unwrap();
        let cache = DedupCache::new(dir.path().join("test.db")).unwrap();
        let result = cache
            .get_or_insert("hello world content", "cat f.rs", "compressed")
            .unwrap();
        assert!(matches!(result, CacheResult::Fresh { .. }));
    }

    #[test]
    fn test_second_identical_call_returns_ref() {
        let dir = tempdir().unwrap();
        let cache = DedupCache::new(dir.path().join("test.db")).unwrap();
        cache
            .get_or_insert("same content here", "cat f.rs", "compressed")
            .unwrap();
        let result = cache
            .get_or_insert("same content here", "cat f.rs", "compressed")
            .unwrap();
        assert!(matches!(result, CacheResult::Ref { .. }));
    }

    #[test]
    fn test_ref_token_format() {
        let dir = tempdir().unwrap();
        let cache = DedupCache::new(dir.path().join("test.db")).unwrap();
        cache.get_or_insert("content abc", "cat", "comp").unwrap();
        if let CacheResult::Ref { inline_ref } =
            cache.get_or_insert("content abc", "cat", "comp").unwrap()
        {
            assert!(inline_ref.starts_with("§ref:"), "got: {inline_ref}");
            assert!(inline_ref.ends_with("§"), "got: {inline_ref}");
        } else {
            panic!("expected Ref, got Fresh");
        }
    }

    #[test]
    fn test_different_content_no_dedup() {
        let dir = tempdir().unwrap();
        let cache = DedupCache::new(dir.path().join("test.db")).unwrap();
        cache.get_or_insert("content A", "cat", "comp A").unwrap();
        let result = cache.get_or_insert("content B", "cat", "comp B").unwrap();
        assert!(matches!(result, CacheResult::Fresh { .. }));
    }

    #[test]
    fn test_sha256_hex_deterministic() {
        let h1 = sha256_hex("hello world");
        let h2 = sha256_hex("hello world");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn test_evict_stale_no_panic_on_empty_db() {
        let dir = tempdir().unwrap();
        let cache = DedupCache::new(dir.path().join("empty.db")).unwrap();
        let evicted = cache.evict_stale().unwrap();
        assert_eq!(evicted, 0);
    }
}
