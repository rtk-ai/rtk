//! Context-aware predicates for conditional safety rules.
//! These give RTK "situational awareness" - checking git state, file existence, etc.

use std::process::Command;

/// Check if there are unstaged changes in the current git repo
pub(crate) fn has_unstaged_changes() -> bool {
    Command::new("git")
        .args(["diff", "--quiet"])
        .status()
        .map(|s| !s.success()) // git diff --quiet returns 1 if changes exist
        .unwrap_or(false)
}

/// Critical for token reduction: detect if output goes to human or agent
pub(crate) fn is_interactive() -> bool {
    use std::io::IsTerminal;
    std::io::stderr().is_terminal()
}

/// Expand ~ to $HOME, with fallback
pub(crate) fn expand_tilde(path: &str) -> String {
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
pub(crate) fn get_home() -> String {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/".to_string())
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

    #[test]
    fn test_has_unstaged_changes() {
        // Just ensure it doesn't panic
        let _ = has_unstaged_changes();
    }
}
