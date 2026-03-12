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
