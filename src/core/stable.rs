//! Deterministic output normalization for prompt-cache friendliness.
//!
//! `--stable` (or `[cache] stable`, or `RTK_STABLE=1`) makes rtk output
//! byte-identical for the same logical input across machines and working
//! directories, by rewriting absolute cwd/home prefixes to `.`/`~`. Identical
//! bytes across runs keep an LLM provider's prompt-cache prefix longer, so more
//! input tokens are served from cache instead of recomputed.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

static STABLE: AtomicBool = AtomicBool::new(false);

/// Enable or disable stable mode for the process (set once at startup).
pub fn set_enabled(enabled: bool) {
    STABLE.store(enabled, Ordering::Relaxed);
}

/// Whether stable mode is active.
pub fn enabled() -> bool {
    STABLE.load(Ordering::Relaxed)
}

/// Apply normalization only when stable mode is enabled.
pub fn apply(input: &str) -> String {
    if enabled() {
        normalize_paths(input)
    } else {
        input.to_string()
    }
}

/// Rewrite absolute cwd (`.`) and home (`~`) prefixes for byte-stable output.
pub fn normalize_paths(input: &str) -> String {
    normalize_with(input, std::env::current_dir().ok(), dirs::home_dir())
}

/// Testable core: cwd is replaced before home so that a cwd nested inside home
/// becomes `.` (not `~/...`). Idempotent.
fn normalize_with(input: &str, cwd: Option<PathBuf>, home: Option<PathBuf>) -> String {
    let mut out = input.to_string();
    if let Some(cwd) = cwd.as_deref().and_then(|p| p.to_str()) {
        if !cwd.is_empty() {
            out = out.replace(cwd, ".");
        }
    }
    if let Some(home) = home.as_deref().and_then(|p| p.to_str()) {
        if !home.is_empty() {
            out = out.replace(home, "~");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Option<PathBuf> {
        Some(PathBuf::from(s))
    }

    #[test]
    fn test_home_rewritten_to_tilde() {
        let out = normalize_with(
            "see /home/sam/.cache/file",
            p("/home/sam/proj"),
            p("/home/sam"),
        );
        assert_eq!(out, "see ~/.cache/file");
    }

    #[test]
    fn test_cwd_rewritten_to_dot() {
        let out = normalize_with(
            "edited /home/sam/proj/src/main.rs",
            p("/home/sam/proj"),
            p("/home/sam"),
        );
        assert_eq!(out, "edited ./src/main.rs");
    }

    #[test]
    fn test_cwd_inside_home_takes_precedence() {
        // The cwd prefix must win, otherwise we'd emit `~/proj/src` not `./src`.
        let out = normalize_with(
            "/home/sam/proj/src and /home/sam/other",
            p("/home/sam/proj"),
            p("/home/sam"),
        );
        assert_eq!(out, "./src and ~/other");
    }

    #[test]
    fn test_noop_without_matches() {
        let out = normalize_with("nothing to rewrite", p("/home/sam/proj"), p("/home/sam"));
        assert_eq!(out, "nothing to rewrite");
    }

    #[test]
    fn test_idempotent() {
        let once = normalize_with("/home/sam/proj/x", p("/home/sam/proj"), p("/home/sam"));
        let twice = normalize_with(&once, p("/home/sam/proj"), p("/home/sam"));
        assert_eq!(once, twice);
        assert_eq!(once, "./x");
    }

    #[test]
    fn test_apply_is_noop_when_disabled() {
        set_enabled(false);
        assert_eq!(apply("/home/sam/anything"), "/home/sam/anything");
    }
}
