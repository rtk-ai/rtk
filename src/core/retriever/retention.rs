//! Retention policy and explicit maintenance for the configured recall store.

use super::{RetrieverConfig, db_path, now_secs, open_existing_path, query, writer_gate};
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use std::collections::HashSet;

pub(super) fn init_storage_accounting(conn: &Connection) -> Result<()> {
    // db's migration engine owns the transaction and version stamp. Keep this
    // step idempotent so `rtk init` also restores a dropped accounting trigger.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS recall_storage (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            entries INTEGER NOT NULL,
            blob_bytes INTEGER NOT NULL
        );
        INSERT INTO recall_storage (id, entries, blob_bytes)
        SELECT 1, (SELECT COUNT(*) FROM recall),
                  (SELECT COALESCE(SUM(length(blob)), 0) FROM recall)
        ON CONFLICT(id) DO UPDATE SET entries = excluded.entries,
            blob_bytes = excluded.blob_bytes;
        CREATE TRIGGER IF NOT EXISTS recall_storage_insert AFTER INSERT ON recall BEGIN
            UPDATE recall_storage SET entries = entries + 1,
                blob_bytes = blob_bytes + length(NEW.blob) WHERE id = 1;
        END;
        CREATE TRIGGER IF NOT EXISTS recall_storage_update AFTER UPDATE OF blob ON recall BEGIN
            UPDATE recall_storage SET blob_bytes = blob_bytes + length(NEW.blob) - length(OLD.blob)
            WHERE id = 1;
        END;
        CREATE TRIGGER IF NOT EXISTS recall_storage_delete AFTER DELETE ON recall BEGIN
            UPDATE recall_storage SET entries = entries - 1,
                blob_bytes = blob_bytes - length(OLD.blob) WHERE id = 1;
        END;",
    )?;
    Ok(())
}

fn totals(conn: &Connection) -> Result<(i64, i64)> {
    Ok(conn.query_row(
        "SELECT entries, blob_bytes FROM recall_storage WHERE id = 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

/// Call inside the same write transaction as insertion. Protected entries must
/// remain recoverable; if they alone exceed a cap, the caller must roll back.
pub(super) fn evict(conn: &Connection, cfg: &RetrieverConfig, keep_hashes: &[&str]) -> Result<()> {
    let protected: HashSet<&str> = keep_hashes.iter().copied().collect();
    if cfg.retention_days > 0 {
        let cutoff = now_secs() - i64::from(cfg.retention_days) * 86_400;
        let mut stmt = conn.prepare("SELECT hash FROM recall WHERE created_at < ?1")?;
        let expired = stmt
            .query_map([cutoff], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        delete_hashes(
            conn,
            expired
                .into_iter()
                .filter(|hash| !protected.contains(hash.as_str())),
        )?;
    }

    let (mut entries, mut bytes) = totals(conn)?;
    let over_limit = |entries: i64, bytes: i64| {
        (cfg.max_entries > 0 && entries as u64 > cfg.max_entries as u64)
            || (cfg.max_total_bytes > 0 && bytes as u64 > cfg.max_total_bytes as u64)
    };
    if !over_limit(entries, bytes) {
        return Ok(());
    }

    // Read only hash/length metadata, and only under pressure. Never load the
    // payloads or rescan their total on the normal write path.
    let mut stmt =
        conn.prepare("SELECT hash, length(blob) FROM recall ORDER BY created_at ASC, rowid ASC")?;
    let mut rows = stmt.query([])?;
    let mut remove = Vec::new();
    while over_limit(entries, bytes) {
        let Some(row) = rows.next()? else {
            anyhow::bail!("protected recall entries exceed the storage limits");
        };
        let hash: String = row.get(0)?;
        if protected.contains(hash.as_str()) {
            continue;
        }
        entries -= 1;
        bytes -= row.get::<_, i64>(1)?;
        remove.push(hash);
    }
    drop(rows);
    drop(stmt);
    delete_hashes(conn, remove.into_iter())
}

fn delete_hashes(conn: &Connection, hashes: impl Iterator<Item = String>) -> Result<()> {
    let mut delete = conn.prepare_cached("DELETE FROM recall WHERE hash = ?1")?;
    for hash in hashes {
        delete.execute([hash])?;
    }
    Ok(())
}

/// Never create a database from a maintenance request, and refuse unrelated
/// SQLite databases before schema initialization or compaction changes them.
fn open_for_maintenance(cfg: &RetrieverConfig) -> Result<Option<Connection>> {
    let path = db_path(cfg)?;
    if !path.try_exists()? {
        return Ok(None);
    }
    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .with_context(|| format!("open recall store {}", path.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.prepare(
        "SELECT hash, command, cwd, exit_code, created_at, total_lines, shown_upto,
                byte_size, truncated, codec, blob FROM recall LIMIT 0",
    )
    .context("configured database is not a recall store")?;
    drop(conn);
    let Some(opened) = open_existing_path(&path)? else {
        return Ok(None);
    };
    query(&opened, |conn| {
        totals(conn)?;
        Ok(())
    })?;
    Ok(Some(opened.conn))
}

pub(super) fn maintain_store(cfg: &RetrieverConfig, purge: bool) -> Result<usize> {
    let writer = writer_gate(&db_path(cfg)?)?;
    let _guard = writer.lock().unwrap_or_else(|e| e.into_inner());
    let Some(conn) = open_for_maintenance(cfg)? else {
        return Ok(0);
    };
    let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
    let before = totals(&tx)?.0;
    if purge {
        tx.execute("DELETE FROM recall", [])?;
    } else {
        evict(&tx, cfg, &[])?;
    }
    let removed = before - totals(&tx)?.0;
    tx.commit().context("apply recall retention")?;

    // Logical byte caps exclude SQLite pages, indexes and journals. Reclaim
    // those explicitly, without unlinking a file held by another RTK process.
    conn.execute_batch("VACUUM;")
        .context("recall entries removed, but database compaction failed")?;
    let busy: i64 = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))?;
    anyhow::ensure!(
        busy == 0,
        "recall entries removed, but WAL compaction is busy; retry --gc when readers finish"
    );
    Ok(removed as usize)
}
