//! Reversible compression (CCR): a content-addressed store of original inputs.
//!
//! `rtkx ccr store <file>` saves the file's contents keyed by a content hash and
//! prints the handle; `rtkx ccr restore <handle>` prints the original back. AXON
//! uses this to offer on-demand retrieval of pre-compression context over MCP --
//! the compressed text stays in the LLM window, the full original a tool-call away.

use super::constants::RTK_DATA_DIR;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// Keep at most this many stored originals (oldest pruned by mtime).
const CCR_MAX_FILES: usize = 500;

/// Resolve the CCR store directory (env override, else the shared data dir).
pub fn ccr_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("RTKX_CCR_DIR") {
        return Some(PathBuf::from(dir));
    }
    dirs::data_local_dir().map(|d| d.join(RTK_DATA_DIR).join("ccr"))
}

/// A short (16 hex char) content handle derived from the SHA-256 of `content`.
pub fn handle_for(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn no_dir() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "CCR data directory unavailable")
}

/// Store `content` under `dir`, returning its handle. Idempotent by content hash.
pub fn store_in(dir: &Path, content: &str) -> io::Result<String> {
    let handle = handle_for(content);
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{handle}.gz"));
    if !path.exists() {
        let file = std::fs::File::create(&path)?;
        let mut encoder = GzEncoder::new(file, Compression::default());
        encoder.write_all(content.as_bytes())?;
        encoder.finish()?;
    }
    cleanup(dir, CCR_MAX_FILES);
    Ok(handle)
}

/// Restore the original content for `handle` from `dir`.
pub fn restore_from(dir: &Path, handle: &str) -> io::Result<String> {
    let path = dir.join(format!("{handle}.gz"));
    let file = std::fs::File::open(&path)?;
    let mut decoder = GzDecoder::new(file);
    let mut content = String::new();
    decoder.read_to_string(&mut content)?;
    Ok(content)
}

/// Store using the resolved data directory.
pub fn store(content: &str) -> io::Result<String> {
    let dir = ccr_dir().ok_or_else(no_dir)?;
    store_in(&dir, content)
}

/// Restore using the resolved data directory.
pub fn restore(handle: &str) -> io::Result<String> {
    let dir = ccr_dir().ok_or_else(no_dir)?;
    restore_from(&dir, handle)
}

/// Prune oldest `.gz` originals, keeping at most `max` (by mtime).
fn cleanup(dir: &Path, max: usize) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "gz"))
        .collect();

    if entries.len() <= max {
        return;
    }

    entries.sort_by_key(|e| {
        e.metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });

    let to_remove = entries.len() - max;
    for entry in entries.iter().take(to_remove) {
        let _ = std::fs::remove_file(entry.path());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handle_is_stable_and_16_hex() {
        let h1 = handle_for("hello world");
        let h2 = handle_for("hello world");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 16);
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(handle_for("a"), handle_for("b"));
    }

    #[test]
    fn test_store_and_restore_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let original = "fn main() {\n    println!(\"hi\");\n}\n".repeat(20);
        let handle = store_in(tmp.path(), &original).unwrap();
        let restored = restore_from(tmp.path(), &handle).unwrap();
        assert_eq!(restored, original);
    }

    #[test]
    fn test_store_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let h1 = store_in(tmp.path(), "same content").unwrap();
        let h2 = store_in(tmp.path(), "same content").unwrap();
        assert_eq!(h1, h2);
        let count = std::fs::read_dir(tmp.path()).unwrap().count();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_restore_missing_handle_errors() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(restore_from(tmp.path(), "0000000000000000").is_err());
    }

    #[test]
    fn test_cleanup_prunes_oldest() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..5 {
            store_in(tmp.path(), &format!("content-{i}")).unwrap();
        }
        cleanup(tmp.path(), 3);
        let count = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "gz"))
            .count();
        assert_eq!(count, 3);
    }
}
