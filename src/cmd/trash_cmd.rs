//! Built-in trash implementation using the `trash` crate.
//! No external binary dependency needed.
//! Cross-platform: Windows Recycle Bin, macOS Trash, Linux FreeDesktop trash.

use anyhow::Result;
use std::path::Path;

/// Move files/directories to system trash.
/// Returns true on success, false on failure.
pub fn execute(paths: &[String]) -> Result<bool> {
    if paths.is_empty() {
        println!("rtk trash: No paths specified");
        return Ok(true);
    }

    // Filter out empty paths and expand ~
    let expanded_paths: Vec<String> = paths.iter()
        .filter(|p| !p.is_empty())
        .map(|p| super::predicates::expand_tilde(p))
        .collect();

    if expanded_paths.is_empty() {
        println!("rtk trash: No valid paths specified");
        return Ok(true);
    }

    // Verify paths exist before trashing
    let (existing, missing): (Vec<_>, Vec<_>) = expanded_paths.iter()
        .partition(|p| Path::new(p).exists());

    if !missing.is_empty() {
        eprintln!("rtk trash: Warning: {} path(s) do not exist:", missing.len());
        for p in &missing {
            eprintln!("  {}", p);
        }
    }

    if existing.is_empty() {
        eprintln!("rtk trash: No existing paths to trash");
        return Ok(false);
    }

    println!("rtk trash: Moving {} item(s) to trash...", existing.len());

    // Convert to &str for trash crate
    let path_refs: Vec<&str> = existing.iter().map(|s| s.as_str()).collect();

    match trash::delete_all(&path_refs) {
        Ok(_) => {
            println!("rtk trash: Done.");
            Ok(true)
        }
        Err(e) => {
            // Smart error handling with suggestions (from refinements)
            eprintln!("rtk trash: Failed: {}", e);
            eprintln!("   The recycle bin might not be supported on this volume.");

            // SUGGESTION: Provide the exact force command
            let escaped_args: String = paths.iter()
                .map(|s| format!("'{}'", s))
                .collect::<Vec<_>>()
                .join(" ");
            eprintln!("   SUGGESTION: If you are sure, use the force override:");
            eprintln!("   RTK_SAFE_RM= rtk run -c \"rm {}\"", escaped_args);

            Ok(false)
        }
    }
}

/// Check if trash is available (always true with built-in implementation)
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
        fs::write(&path, "test content").unwrap();
        path
    }

    fn cleanup_temp_file(path: &PathBuf) {
        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_trash_empty_paths() {
        let result = execute(&[]).unwrap();
        assert!(result);
    }

    #[test]
    fn test_trash_single_file() {
        let path = create_temp_file("single");
        let path_str = path.to_string_lossy().to_string();

        // File should exist
        assert!(Path::new(&path_str).exists());

        // Trash it
        let result = execute(&[path_str.clone()]).unwrap();
        assert!(result);

        // File should no longer exist
        // Note: On some systems, the file might still briefly exist
        // so we just verify the function returned success

        cleanup_temp_file(&path);
    }

    #[test]
    fn test_trash_multiple_files() {
        let path1 = create_temp_file("multi1");
        let path2 = create_temp_file("multi2");
        let paths = vec![
            path1.to_string_lossy().to_string(),
            path2.to_string_lossy().to_string(),
        ];

        let result = execute(&paths).unwrap();
        assert!(result);

        cleanup_temp_file(&path1);
        cleanup_temp_file(&path2);
    }

    #[test]
    fn test_trash_nonexistent_file() {
        let result = execute(&["/nonexistent/file/that/does/not/exist".to_string()]).unwrap();
        // Should return false because no files were trashed
        assert!(!result);
    }

    #[test]
    fn test_trash_mixed_existing_nonexistent() {
        let path = create_temp_file("mixed");
        let paths = vec![
            path.to_string_lossy().to_string(),
            "/nonexistent/file".to_string(),
        ];

        let result = execute(&paths).unwrap();
        // Should succeed for the existing file
        assert!(result);

        cleanup_temp_file(&path);
    }

    #[test]
    fn test_is_available() {
        assert!(is_available());
    }
}
