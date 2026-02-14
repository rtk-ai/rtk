//! Built-in trash implementation using the `trash` crate.
//! Cross-platform: Windows Recycle Bin, macOS Trash, Linux FreeDesktop trash.
//! Silent on success (like rm), only outputs on error.

use anyhow::Result;
use std::path::Path;

/// Move files/directories to system trash.
/// Returns true on success, false on failure.
pub fn execute(paths: &[String]) -> Result<bool> {
    // Filter out empty paths and expand ~
    let expanded_paths: Vec<String> = paths.iter()
        .filter(|p| !p.is_empty())
        .map(|p| super::predicates::expand_tilde(p))
        .collect();

    // Filter to existing paths only
    let existing: Vec<&str> = expanded_paths.iter()
        .filter(|p| Path::new(p).exists())
        .map(|s| s.as_str())
        .collect();

    if existing.is_empty() {
        return Ok(false);
    }

    match trash::delete_all(&existing) {
        Ok(_) => Ok(true),
        Err(e) => {
            eprintln!("trash: ✗ {} (RTK_SAFE_COMMANDS=0 to bypass)", e);
            Ok(false)
        }
    }
}

/// Check if trash is available
pub fn is_available() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn create_temp_file(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("rtk_test_{}", name));
        fs::write(&path, "test").unwrap();
        path
    }

    fn cleanup(path: &PathBuf) {
        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_trash_empty() {
        // Empty paths = nothing to trash = returns false
        assert!(!execute(&[]).unwrap());
    }

    #[test]
    fn test_trash_single() {
        let path = create_temp_file("single");
        let path_str = path.to_string_lossy().to_string();
        assert!(execute(&[path_str]).unwrap());
        cleanup(&path);
    }

    #[test]
    fn test_trash_multiple() {
        let p1 = create_temp_file("m1");
        let p2 = create_temp_file("m2");
        assert!(execute(&[p1.to_string_lossy().to_string(), p2.to_string_lossy().to_string()]).unwrap());
        cleanup(&p1);
        cleanup(&p2);
    }

    #[test]
    fn test_trash_nonexistent() {
        assert!(!execute(&["/nonexistent/file".to_string()]).unwrap());
    }

    #[test]
    fn test_is_available() {
        assert!(is_available());
    }
}
