//! Context-aware predicates for conditional safety rules.
//! These give RTK "situational awareness" - checking git state, file existence, etc.

use std::path::Path;
use std::process::Command;

/// Check if there are unstaged changes in the current git repo
pub fn has_unstaged_changes() -> bool {
    Command::new("git")
        .args(["diff", "--quiet"])
        .status()
        .map(|s| !s.success())  // git diff --quiet returns 1 if changes exist
        .unwrap_or(false)
}

/// Check if there are staged but uncommitted changes
pub fn has_staged_changes() -> bool {
    Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .status()
        .map(|s| !s.success())
        .unwrap_or(false)
}

/// Check if any stash entries exist
pub fn stash_exists() -> bool {
    Command::new("git")
        .args(["stash", "list"])
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false)
}

/// Check if path is a file
pub fn is_file(path: &str) -> bool {
    Path::new(path).is_file()
}

/// Check if path is a directory
pub fn is_dir(path: &str) -> bool {
    Path::new(path).is_dir()
}

/// Check if path exists (file or directory)
pub fn path_exists(path: &str) -> bool {
    Path::new(path).exists()
}

/// Critical for token reduction: detect if output goes to human or agent
pub fn is_interactive() -> bool {
    atty::is(atty::Stream::Stderr)
}

/// Check if we're inside a git repository
pub fn in_git_repo() -> bool {
    Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Expand ~ to $HOME, with fallback
pub fn expand_tilde(path: &str) -> String {
    if path.starts_with("~") {
        // Try HOME first, then USERPROFILE (Windows)
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| "/".to_string());
        path.replacen("~", &home, 1)
    } else {
        path.to_string()
    }
}

/// Get HOME directory with fallback
pub fn get_home() -> String {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/".to_string())
}

/// Check if a binary exists in PATH
pub fn binary_exists(name: &str) -> bool {
    which::which(name).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    // === PATH EXPANSION TESTS ===

    #[test]
    fn test_expand_tilde_simple() {
        let home = env::var("HOME").unwrap_or("/".to_string());
        assert_eq!(expand_tilde("~/src"), format!("{}/src", home));
    }

    #[test]
    fn test_expand_tilde_no_tilde() {
        assert_eq!(expand_tilde("/absolute/path"), "/absolute/path");
    }

    #[test]
    fn test_expand_tilde_only_tilde() {
        let home = env::var("HOME").unwrap_or("/".to_string());
        assert_eq!(expand_tilde("~"), home);
    }

    #[test]
    fn test_expand_tilde_relative() {
        assert_eq!(expand_tilde("relative/path"), "relative/path");
    }

    // === FILE SYSTEM TESTS ===

    #[test]
    fn test_is_file_exists() {
        // Cargo.toml should exist in any Rust project
        assert!(is_file("Cargo.toml") || !Path::new("Cargo.toml").exists());
    }

    #[test]
    fn test_is_file_directory() {
        // src should be a directory, not a file
        assert!(!is_file("src"));
    }

    #[test]
    fn test_is_dir_exists() {
        assert!(is_dir("src") || !Path::new("src").exists());
    }

    #[test]
    fn test_is_dir_file() {
        assert!(!is_dir("Cargo.toml"));
    }

    #[test]
    fn test_path_exists_file() {
        assert!(path_exists("Cargo.toml") || !Path::new("Cargo.toml").exists());
    }

    #[test]
    fn test_path_exists_dir() {
        assert!(path_exists("src") || !Path::new("src").exists());
    }

    #[test]
    fn test_path_exists_nonexistent() {
        assert!(!path_exists("/nonexistent/path/that/does/not/exist"));
    }

    // === HOME DIRECTORY TESTS ===

    #[test]
    fn test_get_home_returns_something() {
        let home = get_home();
        assert!(!home.is_empty());
    }

    // === INTERACTIVE TESTS ===

    #[test]
    fn test_is_interactive() {
        // This will be false when running tests
        // Just ensure it doesn't panic
        let _ = is_interactive();
    }

    // === GIT PREDICATE TESTS ===
    // Note: These tests depend on git being installed and the CWD being a git repo

    #[test]
    fn test_in_git_repo() {
        // This test should pass when run in the rtk repo
        // Just ensure it doesn't panic
        let _ = in_git_repo();
    }

    #[test]
    fn test_has_unstaged_changes() {
        // Just ensure it doesn't panic
        let _ = has_unstaged_changes();
    }

    #[test]
    fn test_stash_exists() {
        // Just ensure it doesn't panic
        let _ = stash_exists();
    }
}
