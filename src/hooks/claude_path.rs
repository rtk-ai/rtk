//! Shared Claude Code config directory resolution.
//!
//! Honors the `CLAUDE_CONFIG_DIR` environment variable, falling back to
//! `$HOME/.claude` when unset or empty.  This mirrors the existing
//! `CODEX_HOME` support for Codex CLI.

use super::constants::CLAUDE_DIR;
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Resolve the Claude Code config directory.
///
/// Priority: `$CLAUDE_CONFIG_DIR` env var → `$HOME/.claude`.
pub(crate) fn resolve_claude_dir() -> Result<PathBuf> {
    resolve_claude_dir_from(
        std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from),
        dirs::home_dir(),
    )
}

/// Testable inner resolver (no env var side-effects).
pub(crate) fn resolve_claude_dir_from(
    claude_config_dir: Option<PathBuf>,
    home_dir: Option<PathBuf>,
) -> Result<PathBuf> {
    if let Some(path) = claude_config_dir.filter(|p| !p.as_os_str().is_empty()) {
        return Ok(path);
    }

    home_dir
        .map(|home| home.join(CLAUDE_DIR))
        .context("Cannot determine Claude config directory. Set $CLAUDE_CONFIG_DIR or $HOME.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_prefers_env_var_over_home() {
        let custom = PathBuf::from("/custom/claude");
        let home = PathBuf::from("/home/user");
        let result = resolve_claude_dir_from(Some(custom.clone()), Some(home)).unwrap();
        assert_eq!(result, custom);
    }

    #[test]
    fn test_empty_env_var_falls_back_to_home() {
        let home = PathBuf::from("/home/user");
        let result = resolve_claude_dir_from(Some(PathBuf::new()), Some(home.clone())).unwrap();
        assert_eq!(result, home.join(".claude"));
    }

    #[test]
    fn test_none_env_var_falls_back_to_home() {
        let home = PathBuf::from("/home/user");
        let result = resolve_claude_dir_from(None, Some(home.clone())).unwrap();
        assert_eq!(result, home.join(".claude"));
    }

    #[test]
    fn test_both_none_returns_error() {
        assert!(resolve_claude_dir_from(None, None).is_err());
    }

    #[test]
    fn test_path_with_spaces() {
        let custom = PathBuf::from("/Users/John Doe/.my claude");
        let home = PathBuf::from("/home/user");
        let result = resolve_claude_dir_from(Some(custom.clone()), Some(home)).unwrap();
        assert_eq!(result, custom);
    }

    #[test]
    fn test_relative_path_preserved_as_is() {
        let relative = PathBuf::from("./configs/.claude");
        let home = PathBuf::from("/home/user");
        let result = resolve_claude_dir_from(Some(relative.clone()), Some(home)).unwrap();
        assert_eq!(result, relative);
    }

    #[test]
    fn test_env_var_takes_priority_even_when_home_missing() {
        let custom = PathBuf::from("/custom/claude");
        let result = resolve_claude_dir_from(Some(custom.clone()), None).unwrap();
        assert_eq!(result, custom);
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_resolve_with_real_tempdir() {
        let temp = TempDir::new().unwrap();
        let claude_dir = temp.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();
        let result = resolve_claude_dir_from(Some(claude_dir.clone()), None).unwrap();
        assert_eq!(result, claude_dir);
        assert!(result.exists());
    }

    #[cfg(unix)]
    #[test]
    fn test_resolve_with_symlink() {
        let temp = TempDir::new().unwrap();
        let real_dir = temp.path().join("real-claude");
        let symlink = temp.path().join("symlink-claude");
        fs::create_dir_all(&real_dir).unwrap();
        std::os::unix::fs::symlink(&real_dir, &symlink).unwrap();
        let result = resolve_claude_dir_from(Some(symlink.clone()), None).unwrap();
        assert_eq!(result, symlink);
    }
}
