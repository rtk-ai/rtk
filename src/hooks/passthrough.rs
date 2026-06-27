//! Signal-vs-bulk classifier for hook command passthrough.
//!
//! Some commands emit a small *control signal* — `git push`'s `To <repo>`
//! marker, `git status`'s porcelain lines, `gh pr` state — that downstream
//! agents grep against canonical strings to gate next-step decisions. Routing
//! those through rtk's reduction pipeline reshapes or inflates the marker, so
//! the agent misclassifies a successful operation as hung or failed.
//!
//! This module ships a default allowlist of substring patterns whose command
//! bypasses rewriting (raw stdout reaches the caller). Operators extend or
//! replace it via `~/.config/rtk/passthrough.toml`:
//!
//! ```toml
//! [signal]
//! # Replaces the built-in default set.
//! patterns = ["git push", "git status", "gh pr"]
//!
//! [signal.extend]
//! # Appends to whatever the effective `[signal]` set is.
//! patterns = ["glab mr", "kubectl get pods"]
//! ```
//!
//! Matching is a case-sensitive substring test against the reconstructed
//! command line.

use crate::core::constants::RTK_DATA_DIR;
use serde::Deserialize;
use std::path::PathBuf;

/// Operator override file, alongside `config.toml` in the rtk config dir.
const PASSTHROUGH_TOML: &str = "passthrough.toml";

/// Built-in signal patterns: SCM client status/markers and forge clients whose
/// stdout is a control signal, not bulk output. Small and stable by design.
const DEFAULT_SIGNAL_PATTERNS: &[&str] = &[
    "git push",
    "git pull",
    "git fetch",
    "git merge",
    "git status",
    "git remote",
    "git rev-parse",
    "git branch",
    "gh pr",
    "gh issue",
    "gh release",
    "gh api",
    "gh run",
    "glab mr",
    "glab issue",
];

#[derive(Debug, Default, Deserialize)]
struct PassthroughFile {
    #[serde(default)]
    signal: SignalConfig,
}

#[derive(Debug, Default, Deserialize)]
struct SignalConfig {
    /// When present, replaces the built-in default patterns entirely.
    #[serde(default)]
    patterns: Option<Vec<String>>,
    /// Appended to the effective pattern set (defaults or replacement).
    #[serde(default)]
    extend: SignalExtend,
}

#[derive(Debug, Default, Deserialize)]
struct SignalExtend {
    #[serde(default)]
    patterns: Vec<String>,
}

fn passthrough_path() -> Option<PathBuf> {
    let config_dir = dirs::config_dir()?;
    Some(config_dir.join(RTK_DATA_DIR).join(PASSTHROUGH_TOML))
}

/// Load the operator override file. Missing or malformed files fall back to
/// defaults — a bad override must never block the hook pipeline.
fn load_passthrough_file() -> PassthroughFile {
    let path = match passthrough_path() {
        Some(p) => p,
        None => return PassthroughFile::default(),
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return PassthroughFile::default(),
    };
    toml::from_str(&content).unwrap_or_default()
}

/// Resolve the effective pattern set: operator `[signal] patterns` replace the
/// built-ins, then `[signal.extend] patterns` are appended.
fn resolve_patterns(file: &PassthroughFile) -> Vec<String> {
    let mut patterns: Vec<String> = match &file.signal.patterns {
        Some(p) => p.clone(),
        None => DEFAULT_SIGNAL_PATTERNS
            .iter()
            .map(|s| s.to_string())
            .collect(),
    };
    patterns.extend(file.signal.extend.patterns.iter().cloned());
    patterns
}

/// Case-sensitive substring match of any pattern against the command line.
fn matches(cmd: &str, patterns: &[String]) -> bool {
    patterns
        .iter()
        .any(|p| !p.is_empty() && cmd.contains(p.as_str()))
}

/// Whether `cmd` is a signal command whose stdout must bypass rtk rewriting.
pub fn is_signal_command(cmd: &str) -> bool {
    matches(cmd, &resolve_patterns(&load_passthrough_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> PassthroughFile {
        PassthroughFile::default()
    }

    #[test]
    fn test_default_patterns_match_git_signal_commands() {
        let patterns = resolve_patterns(&defaults());
        assert!(matches("git push origin main", &patterns));
        assert!(matches("git status", &patterns));
        assert!(matches("git rev-parse HEAD", &patterns));
        assert!(matches("GIT_PAGER=cat git status", &patterns));
    }

    #[test]
    fn test_default_patterns_match_forge_clients() {
        let patterns = resolve_patterns(&defaults());
        assert!(matches("gh pr view 123", &patterns));
        assert!(matches("gh run list", &patterns));
        assert!(matches("glab mr list", &patterns));
    }

    #[test]
    fn test_default_patterns_skip_bulk_commands() {
        let patterns = resolve_patterns(&defaults());
        assert!(!matches("git log -p", &patterns));
        assert!(!matches("ls -la", &patterns));
        assert!(!matches("find . -name '*.rs'", &patterns));
        assert!(!matches("docker logs web", &patterns));
        // `git add`/`git commit` are bulk-safe and not in the signal set.
        assert!(!matches("git add .", &patterns));
    }

    #[test]
    fn test_match_is_case_sensitive() {
        let patterns = resolve_patterns(&defaults());
        assert!(!matches("GIT PUSH origin main", &patterns));
    }

    #[test]
    fn test_signal_patterns_replace_defaults() {
        let toml = r#"
[signal]
patterns = ["git push"]
"#;
        let file: PassthroughFile = toml::from_str(toml).expect("valid toml");
        let patterns = resolve_patterns(&file);
        assert!(matches("git push origin main", &patterns));
        // `git status` was in the defaults but is no longer present.
        assert!(!matches("git status", &patterns));
    }

    #[test]
    fn test_signal_extend_appends_to_defaults() {
        let toml = r#"
[signal.extend]
patterns = ["kubectl get pods"]
"#;
        let file: PassthroughFile = toml::from_str(toml).expect("valid toml");
        let patterns = resolve_patterns(&file);
        // Defaults remain in effect.
        assert!(matches("git push origin main", &patterns));
        // Extension is appended.
        assert!(matches("kubectl get pods -n default", &patterns));
    }

    #[test]
    fn test_signal_extend_appends_to_replacement() {
        let toml = r#"
[signal]
patterns = ["git push"]

[signal.extend]
patterns = ["glab mr"]
"#;
        let file: PassthroughFile = toml::from_str(toml).expect("valid toml");
        let patterns = resolve_patterns(&file);
        assert!(matches("git push origin main", &patterns));
        assert!(matches("glab mr create", &patterns));
        assert!(!matches("git status", &patterns));
    }

    #[test]
    fn test_empty_pattern_never_matches_everything() {
        let patterns = vec![String::new()];
        assert!(!matches("git status", &patterns));
        assert!(!matches("anything", &patterns));
    }

    #[test]
    fn test_missing_signal_section_uses_defaults() {
        // An override file that omits [signal] keeps the built-in set.
        let file: PassthroughFile = toml::from_str("").expect("valid toml");
        let patterns = resolve_patterns(&file);
        assert!(matches("git push origin main", &patterns));
    }
}
