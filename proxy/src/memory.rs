//! Permanent memory backed by ICM (`icm-core` / `icm-store`): CCR's cache
//! needs real recall, not just an internal key/value store.
//!
//! FTS only (no embeddings/fastembed) — keeps the binary lean. SQLite backend
//! only: this is a single-node local daemon, so there is no postgres/multi-pod
//! story here.
//!
//! No multi-tenant isolation: this proxy is a single local process for one
//! operator, so there is no tenant to isolate — `record`/`recall` are
//! unscoped, and CCR's cache topic below carries no `{tenant}/` segment
//! (it keeps `{session}/` — several concurrent Claude Code sessions against
//! the same local proxy is a real scenario).

use crate::error::{AppError, AppResult};
use icm_core::{Importance, Memory, MemoryStore};
use icm_store::Store as IcmStore;
use std::sync::{Arc, Mutex};

/// A recall hit returned to callers.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecallHit {
    pub topic: String,
    pub summary: String,
}

/// Handle to the ICM permanent-memory store.
#[derive(Clone)]
pub struct IcmMemory {
    inner: Arc<Mutex<IcmStore>>,
}

impl IcmMemory {
    /// Open (or create) the ICM store at `path` (SQLite file).
    ///
    /// The file holds raw tool output verbatim, so on Unix it is created --
    /// and kept -- owner-only. SQLite copies the database file's mode onto
    /// its `-wal`/`-shm` siblings, so tightening the one file covers all
    /// three; an already-existing world-readable cache is tightened here too
    /// rather than left as it was found.
    pub fn open(path: &str) -> AppResult<IcmMemory> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let p = std::path::Path::new(path);
            if !p.exists() {
                if let Some(parent) = p.parent() {
                    if !parent.as_os_str().is_empty() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                }
                let _ = std::fs::OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .write(true)
                    .mode(0o600)
                    .open(p);
            } else if let Ok(meta) = std::fs::metadata(p) {
                let mut perms = meta.permissions();
                if perms.mode() & 0o077 != 0 {
                    perms.set_mode(0o600);
                    let _ = std::fs::set_permissions(p, perms);
                }
            }
        }
        let store = IcmStore::new(std::path::Path::new(path))
            .map_err(|e| AppError::Memory(format!("open icm store: {e}")))?;
        Ok(IcmMemory {
            inner: Arc::new(Mutex::new(store)),
        })
    }

    /// In-memory store, for tests.
    pub fn in_memory() -> AppResult<IcmMemory> {
        let store = IcmStore::in_memory()
            .map_err(|e| AppError::Memory(format!("open in-memory icm store: {e}")))?;
        Ok(IcmMemory {
            inner: Arc::new(Mutex::new(store)),
        })
    }

    /// Record an exchange as a permanent memory under `topic`.
    pub async fn record(
        &self,
        topic: String,
        summary: String,
        raw: Option<String>,
    ) -> AppResult<String> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut mem = Memory::new(topic, summary, Importance::Medium);
            mem.raw_excerpt = raw;
            let store = inner
                .lock()
                .map_err(|_| "memory store poisoned".to_string())?;
            store.store(mem).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| AppError::Memory(format!("join: {e}")))?
        .map_err(AppError::Memory)
    }

    /// Recall up to `limit` memories matching `query` via full-text search.
    pub async fn recall(&self, query: String, limit: usize) -> AppResult<Vec<RecallHit>> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| "memory store poisoned".to_string())?;
            let hits = store.search_fts(&query, limit).map_err(|e| e.to_string())?;
            Ok::<_, String>(
                hits.into_iter()
                    .map(|m| RecallHit {
                        topic: m.topic,
                        summary: m.summary,
                    })
                    .collect(),
            )
        })
        .await
        .map_err(|e| AppError::Memory(format!("join: {e}")))?
        .map_err(AppError::Memory)
    }

    /// Recall memories relevant to a request, for transparent injection.
    /// Combines keyword OR search with a same-topic fallback; deduplicated,
    /// capped at `limit`.
    pub async fn recall_relevant(
        &self,
        keywords: Vec<String>,
        topic: Option<String>,
        limit: usize,
    ) -> AppResult<Vec<RecallHit>> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| "memory store poisoned".to_string())?;
            let mut seen = std::collections::HashSet::new();
            let mut out: Vec<RecallHit> = Vec::new();

            if !keywords.is_empty() {
                let refs: Vec<&str> = keywords.iter().map(|s| s.as_str()).collect();
                let hits = store
                    .search_by_keywords(&refs, limit)
                    .map_err(|e| e.to_string())?;
                for m in hits {
                    if out.len() >= limit {
                        break;
                    }
                    if seen.insert(m.id.clone()) {
                        out.push(RecallHit {
                            topic: m.topic,
                            summary: m.summary,
                        });
                    }
                }
            }

            if out.len() < limit {
                if let Some(t) = topic {
                    let topic_hits = store.get_by_topic(&t).map_err(|e| e.to_string())?;
                    for m in topic_hits {
                        if out.len() >= limit {
                            break;
                        }
                        if seen.insert(m.id.clone()) {
                            out.push(RecallHit {
                                topic: m.topic,
                                summary: m.summary,
                            });
                        }
                    }
                }
            }

            out.truncate(limit);
            Ok::<_, String>(out)
        })
        .await
        .map_err(|e| AppError::Memory(format!("join: {e}")))?
        .map_err(AppError::Memory)
    }

    /// Store a single value verbatim under an exact `topic`, returning the id.
    /// Used by CCR's cache: `topic` is the lookup key, `value` is the raw
    /// original content. A fixed non-matchable summary ("rtk-ccr") keeps
    /// these entries out of keyword recall (see [`Self::recall_relevant`]) so
    /// they can't crowd out real memories.
    pub async fn put_exact(&self, topic: String, value: String) -> AppResult<String> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut mem = Memory::new(topic, "rtk-ccr".to_string(), Importance::Low);
            mem.raw_excerpt = Some(value);
            let store = inner
                .lock()
                .map_err(|_| "memory store poisoned".to_string())?;
            store.store(mem).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| AppError::Memory(format!("join: {e}")))?
        .map_err(AppError::Memory)
    }

    /// Fetch the verbatim value previously stored under an exact `topic`, or
    /// `None` if nothing was stored there.
    pub async fn get_exact(&self, topic: String) -> AppResult<Option<String>> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| "memory store poisoned".to_string())?;
            let hits = store.get_by_topic(&topic).map_err(|e| e.to_string())?;
            Ok::<_, String>(hits.into_iter().find_map(|m| m.raw_excerpt))
        })
        .await
        .map_err(|e| AppError::Memory(format!("join: {e}")))?
        .map_err(AppError::Memory)
    }

    /// Total number of stored memories.
    pub async fn count(&self) -> AppResult<usize> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| "memory store poisoned".to_string())?;
            store.count().map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| AppError::Memory(format!("join: {e}")))?
        .map_err(AppError::Memory)
    }
}

/// Expire CCR cache entries older than `ttl_secs`.
///
/// icm-store exposes no delete API for memories, so this goes to the SQLite
/// file directly -- acceptable only because the dependency is pinned by rev
/// (the schema cannot drift under us) and the sweep is scoped to CCR's own
/// `ccr/%` topics, never a user's real memories. The store's `AFTER DELETE`
/// trigger keeps the FTS index in sync with a plain DELETE. Everything is
/// best-effort: a locked or missing table prunes nothing and says so, it
/// never takes the daemon down.
pub fn prune_ccr_cache(path: &str, ttl_secs: u64) -> Result<usize, String> {
    if ttl_secs == 0 || matches!(path.trim(), "" | "off" | ":memory:") {
        return Ok(0);
    }
    let conn = rusqlite::Connection::open(path).map_err(|e| format!("open for prune: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| format!("busy_timeout: {e}"))?;
    let has_table: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='memories')",
            [],
            |r| r.get(0),
        )
        .map_err(|e| format!("schema probe: {e}"))?;
    if !has_table {
        return Ok(0);
    }
    conn.execute(
        "DELETE FROM memories WHERE topic LIKE 'ccr/%' \
         AND datetime(created_at) < datetime('now', ?1)",
        [format!("-{ttl_secs} seconds")],
    )
    .map_err(|e| format!("prune: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn put_get_exact_round_trips() {
        let mem = IcmMemory::in_memory().unwrap();
        let topic = "ccr/default/abc123".to_string();
        let original = "line one\nline two\nsecret-line-1500\nlast line".to_string();
        mem.put_exact(topic.clone(), original.clone())
            .await
            .unwrap();
        let got = mem.get_exact(topic).await.unwrap();
        assert_eq!(
            got.as_deref(),
            Some(original.as_str()),
            "CCR cache round-trip lost the value"
        );
    }

    #[tokio::test]
    async fn get_exact_for_unknown_topic_is_none() {
        let mem = IcmMemory::in_memory().unwrap();
        assert!(
            mem.get_exact("ccr/default/nope".into())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn ccr_cache_entries_do_not_match_keyword_recall() {
        let mem = IcmMemory::in_memory().unwrap();
        mem.put_exact(
            "ccr/default/h1".into(),
            "database connection pool config error output".into(),
        )
        .await
        .unwrap();
        assert!(
            mem.get_exact("ccr/default/h1".into())
                .await
                .unwrap()
                .is_some()
        );
        let hits = mem
            .recall_relevant(vec!["config".to_string(), "database".to_string()], None, 10)
            .await
            .unwrap();
        assert!(
            hits.iter().all(|h| !h.topic.starts_with("ccr/")),
            "a ccr/ cache entry matched keyword recall: {hits:?}"
        );
    }

    #[tokio::test]
    async fn record_then_recall_and_count() {
        let mem = IcmMemory::in_memory().unwrap();
        assert_eq!(mem.count().await.unwrap(), 0);

        mem.record(
            "proxy/myproject".to_string(),
            "fixed the database connection pool".to_string(),
            Some("full excerpt text".to_string()),
        )
        .await
        .unwrap();

        assert_eq!(mem.count().await.unwrap(), 1);

        let hits = mem
            .recall("database connection".to_string(), 5)
            .await
            .unwrap();
        assert!(!hits.is_empty(), "expected at least one recall hit");
        assert_eq!(hits[0].topic, "proxy/myproject");
        assert!(hits[0].summary.contains("database"));
    }

    #[tokio::test]
    async fn recall_no_match_is_empty() {
        let mem = IcmMemory::in_memory().unwrap();
        mem.record("proxy/x".to_string(), "alpha beta gamma".to_string(), None)
            .await
            .unwrap();
        let hits = mem
            .recall("nonexistentwordxyz".to_string(), 5)
            .await
            .unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn recall_respects_limit() {
        let mem = IcmMemory::in_memory().unwrap();
        for i in 0..5 {
            mem.record(
                format!("proxy/t{i}"),
                format!("shared keyword entry number {i}"),
                None,
            )
            .await
            .unwrap();
        }
        let hits = mem.recall("keyword".to_string(), 2).await.unwrap();
        assert!(hits.len() <= 2);
    }
}
