//! Detects whether RTK hooks are installed and warns if they are outdated.

use super::constants::{
    COPILOT_HOOK_FILE, HOOKS_SUBDIR, PRE_TOOL_USE_KEY, REWRITE_HOOK_FILE, SETTINGS_JSON,
};
use super::init::{copilot_user_dir, resolve_claude_dir};
use super::{is_claude_hook_command, is_copilot_hook_command};
use crate::core::constants::RTK_DATA_DIR;
use std::path::{Path, PathBuf};

const CURRENT_HOOK_VERSION: u8 = 3;
const WARN_INTERVAL_SECS: u64 = 24 * 3600;

/// Hook status for diagnostics and `rtk gain`.
#[derive(Debug, PartialEq, Clone)]
pub enum HookStatus {
    /// Hook is installed and up to date.
    Ok,
    /// Hook exists but is outdated or unreadable.
    Outdated,
    /// No hook file found (but Claude Code is installed).
    Missing,
}

/// Return the current hook status without printing anything.
///
/// Aggregates the supported automatic-rewrite integrations: a missing Claude
/// hook is not reported as `Missing` when another automatic integration
/// (currently the user-global GitHub Copilot hook) is installed and valid.
/// Returns `Ok` if no Claude Code is detected (not applicable).
pub fn status() -> HookStatus {
    let claude_dir = resolve_claude_dir().ok();
    let copilot_dir = copilot_user_dir().ok();
    status_at(claude_dir.as_deref(), copilot_dir.as_deref())
}

/// Path-parameterized core of [`status`], testable without env mutation.
fn status_at(claude_dir: Option<&Path>, copilot_dir: Option<&Path>) -> HookStatus {
    let claude = claude_status_at(claude_dir);
    if claude == HookStatus::Missing && copilot_dir.is_some_and(copilot_hook_registered) {
        return HookStatus::Ok;
    }
    claude
}

/// Claude Code hook status. Returns `Ok` if Claude Code is not installed.
fn claude_status_at(claude_dir: Option<&Path>) -> HookStatus {
    // Don't warn users who don't have Claude Code installed
    let Some(claude_dir) = claude_dir else {
        return HookStatus::Ok;
    };
    if !claude_dir.exists() {
        return HookStatus::Ok;
    }

    // Check for new binary command in settings.json first
    if binary_hook_registered(claude_dir) {
        // If old script file still exists alongside new command, report Outdated
        // (migration not complete — user should run `rtk init -g` to clean up)
        let old_hook = claude_dir.join(HOOKS_SUBDIR).join(REWRITE_HOOK_FILE);
        if old_hook.exists() {
            return HookStatus::Outdated;
        }
        return HookStatus::Ok;
    }

    // Fall back to legacy script file check
    let Some(hook_path) = hook_installed_path(claude_dir) else {
        return HookStatus::Missing;
    };
    let Ok(content) = std::fs::read_to_string(&hook_path) else {
        return HookStatus::Outdated; // exists but unreadable — treat as needs-update
    };
    if parse_hook_version(&content) >= CURRENT_HOOK_VERSION {
        HookStatus::Ok
    } else {
        HookStatus::Outdated
    }
}

/// Check if the native binary command is registered in settings.json
fn binary_hook_registered(claude_dir: &std::path::Path) -> bool {
    let settings_path = claude_dir.join(SETTINGS_JSON);
    let content = match std::fs::read_to_string(&settings_path) {
        Ok(c) if !c.trim().is_empty() => c,
        _ => return false,
    };
    let root: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let pre_tool_use = match root
        .get("hooks")
        .and_then(|h| h.get(PRE_TOOL_USE_KEY))
        .and_then(|p| p.as_array())
    {
        Some(arr) => arr,
        None => return false,
    };
    pre_tool_use
        .iter()
        .filter_map(|entry| entry.get("hooks")?.as_array())
        .flatten()
        .filter_map(|hook| hook.get("command")?.as_str())
        .any(is_claude_hook_command)
}

/// Check whether a valid RTK GitHub Copilot hook is installed under
/// `copilot_dir` (`$COPILOT_HOME` or `~/.copilot`).
///
/// Valid means `hooks/rtk-rewrite.json` parses as JSON and contains a
/// `PreToolUse` command entry invoking `rtk hook copilot` (bare or via an
/// absolute path to the rtk binary).
pub(crate) fn copilot_hook_registered(copilot_dir: &Path) -> bool {
    let hook_path = copilot_dir.join(HOOKS_SUBDIR).join(COPILOT_HOOK_FILE);
    let content = match std::fs::read_to_string(&hook_path) {
        Ok(c) if !c.trim().is_empty() => c,
        _ => return false,
    };
    let root: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let entries = match root
        .get("hooks")
        .and_then(|h| h.get(PRE_TOOL_USE_KEY))
        .and_then(|p| p.as_array())
    {
        Some(arr) => arr,
        None => return false,
    };
    entries
        .iter()
        .filter_map(|entry| entry.get("command")?.as_str())
        .any(is_copilot_hook_command)
}

/// Check if the installed hook is missing or outdated, warn once per day.
pub fn maybe_warn() {
    // Don't block startup — fail silently on any error
    let _ = check_and_warn();
}

/// Single source of truth: delegates to `status()` then rate-limits the warning.
fn check_and_warn() -> Option<()> {
    let warning = match status() {
        HookStatus::Ok => return Some(()),
        HookStatus::Missing => {
            "[rtk] /!\\ No hook installed — run `rtk init -g` for automatic token savings"
        }
        HookStatus::Outdated => "[rtk] /!\\ Hook outdated — run `rtk init -g` to update",
    };

    // Rate limit: warn once per day
    let marker = warn_marker_path()?;
    if let Ok(meta) = std::fs::metadata(&marker) {
        if let Ok(modified) = meta.modified() {
            if modified.elapsed().map(|e| e.as_secs()).unwrap_or(u64::MAX) < WARN_INTERVAL_SECS {
                return Some(());
            }
        }
    }

    eprintln!("{}", warning);

    // Touch marker after warning is printed
    let _ = crate::core::utils::create_private_dir(marker.parent()?);
    let _ = std::fs::write(&marker, b"");

    Some(())
}

pub fn parse_hook_version(content: &str) -> u8 {
    // Version tag must be in the first 5 lines (shebang + header convention)
    for line in content.lines().take(5) {
        if let Some(rest) = line.strip_prefix("# rtk-hook-version:") {
            if let Ok(v) = rest.trim().parse::<u8>() {
                return v;
            }
        }
    }
    0 // No version tag = version 0 (outdated)
}

fn hook_installed_path(claude_dir: &Path) -> Option<PathBuf> {
    let path = claude_dir.join(HOOKS_SUBDIR).join(REWRITE_HOOK_FILE);
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

fn warn_marker_path() -> Option<PathBuf> {
    let data_dir = dirs::data_local_dir()?.join(RTK_DATA_DIR);
    Some(data_dir.join(".hook_warn_last"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::constants::{
        CODEX_DIR, CONFIG_DIR, CURSOR_DIR, GEMINI_DIR, GEMINI_HOOK_FILE, HERMES_DIR,
        HERMES_PLUGINS_SUBDIR, HERMES_PLUGIN_MANIFEST_FILE, HERMES_PLUGIN_NAME,
        OPENCODE_PLUGIN_FILE, OPENCODE_SUBDIR, PLUGIN_SUBDIR,
    };

    fn other_integration_installed(home: &std::path::Path) -> bool {
        let paths = [
            home.join(CONFIG_DIR)
                .join(OPENCODE_SUBDIR)
                .join(PLUGIN_SUBDIR)
                .join(OPENCODE_PLUGIN_FILE),
            home.join(CURSOR_DIR)
                .join(HOOKS_SUBDIR)
                .join(REWRITE_HOOK_FILE),
            home.join(CODEX_DIR).join("AGENTS.md"),
            home.join(GEMINI_DIR)
                .join(HOOKS_SUBDIR)
                .join(GEMINI_HOOK_FILE),
            home.join(HERMES_DIR)
                .join(HERMES_PLUGINS_SUBDIR)
                .join(HERMES_PLUGIN_NAME)
                .join(HERMES_PLUGIN_MANIFEST_FILE),
        ];
        paths.iter().any(|p| p.exists())
    }

    #[test]
    fn test_parse_hook_version_present() {
        let content = "#!/usr/bin/env bash\n# rtk-hook-version: 2\n# some comment\n";
        assert_eq!(parse_hook_version(content), 2);
    }

    #[test]
    fn test_parse_hook_version_missing() {
        let content = "#!/usr/bin/env bash\n# old hook without version\n";
        assert_eq!(parse_hook_version(content), 0);
    }

    #[test]
    fn test_parse_hook_version_future() {
        let content = "#!/usr/bin/env bash\n# rtk-hook-version: 5\n";
        assert_eq!(parse_hook_version(content), 5);
    }

    #[test]
    fn test_parse_hook_version_no_tag() {
        assert_eq!(parse_hook_version("no version here"), 0);
        assert_eq!(parse_hook_version(""), 0);
    }

    #[test]
    fn test_hook_status_enum() {
        assert_ne!(HookStatus::Ok, HookStatus::Missing);
        assert_ne!(HookStatus::Outdated, HookStatus::Missing);
        assert_eq!(HookStatus::Ok, HookStatus::Ok);
        // Clone works
        let s = HookStatus::Missing;
        assert_eq!(s.clone(), HookStatus::Missing);
    }

    #[test]
    fn test_binary_hook_registered_accepts_absolute_rtk_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join(SETTINGS_JSON),
            r#"{
                "hooks": {
                    "PreToolUse": [{
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/opt/homebrew/bin/rtk hook claude",
                            "timeout": 5
                        }]
                    }]
                }
            }"#,
        )
        .expect("write settings");

        assert!(binary_hook_registered(tmp.path()));
    }

    // ── Copilot hook detection ───────────────────────────────

    const COPILOT_STOCK: &str = r#"{
  "version": 1,
  "hooks": {
    "PreToolUse": [
      { "type": "command", "command": "rtk hook copilot", "cwd": ".", "timeout": 5 }
    ]
  }
}
"#;

    fn write_copilot_hook(copilot_dir: &std::path::Path, content: &str) {
        let hooks_dir = copilot_dir.join(HOOKS_SUBDIR);
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(hooks_dir.join(COPILOT_HOOK_FILE), content).unwrap();
    }

    #[test]
    fn test_copilot_hook_registered_stock_config() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_copilot_hook(tmp.path(), COPILOT_STOCK);
        assert!(copilot_hook_registered(tmp.path()));
    }

    #[test]
    fn test_copilot_hook_registered_matches_installed_stock_json() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_copilot_hook(tmp.path(), crate::hooks::init::COPILOT_HOOK_JSON);
        assert!(copilot_hook_registered(tmp.path()));
    }

    #[test]
    fn test_copilot_hook_registered_accepts_absolute_rtk_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_copilot_hook(
            tmp.path(),
            &COPILOT_STOCK.replace("rtk hook copilot", "/opt/homebrew/bin/rtk hook copilot"),
        );
        assert!(copilot_hook_registered(tmp.path()));
    }

    #[test]
    fn test_copilot_hook_missing_file_not_registered() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(!copilot_hook_registered(tmp.path()));
        // Hooks dir without the file is not enough either.
        std::fs::create_dir_all(tmp.path().join(HOOKS_SUBDIR)).unwrap();
        assert!(!copilot_hook_registered(tmp.path()));
    }

    #[test]
    fn test_copilot_hook_malformed_json_not_registered() {
        let tmp = tempfile::tempdir().expect("tempdir");
        for content in ["{ not json", "", "   ", "[1, 2]", "\"rtk hook copilot\""] {
            write_copilot_hook(tmp.path(), content);
            assert!(
                !copilot_hook_registered(tmp.path()),
                "content {content:?} must not count as installed"
            );
        }
    }

    #[test]
    fn test_copilot_hook_empty_pre_tool_use_not_registered() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_copilot_hook(
            tmp.path(),
            r#"{ "version": 1, "hooks": { "PreToolUse": [] } }"#,
        );
        assert!(!copilot_hook_registered(tmp.path()));
    }

    #[test]
    fn test_copilot_hook_wrong_command_not_registered() {
        let tmp = tempfile::tempdir().expect("tempdir");
        for cmd in [
            "other-tool --hook",
            "rtk hook claude",
            "echo rtk hook copilot",
        ] {
            write_copilot_hook(tmp.path(), &COPILOT_STOCK.replace("rtk hook copilot", cmd));
            assert!(
                !copilot_hook_registered(tmp.path()),
                "command {cmd:?} must not count as installed"
            );
        }
    }

    // ── Aggregate status ─────────────────────────────────────

    fn write_claude_binary_hook(claude_dir: &std::path::Path) {
        std::fs::create_dir_all(claude_dir).unwrap();
        std::fs::write(
            claude_dir.join(SETTINGS_JSON),
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"rtk hook claude","timeout":5}]}]}}"#,
        )
        .unwrap();
    }

    #[test]
    fn test_status_copilot_only_is_ok_without_claude_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let claude_dir = tmp.path().join(".claude"); // never created
        let copilot_dir = tmp.path().join(".copilot");
        write_copilot_hook(&copilot_dir, COPILOT_STOCK);
        assert_eq!(
            status_at(Some(&claude_dir), Some(&copilot_dir)),
            HookStatus::Ok
        );
    }

    #[test]
    fn test_status_copilot_only_is_ok_with_unconfigured_claude_dir() {
        // Regression: `.claude` exists but has no RTK hook; a valid Copilot
        // hook must suppress the "No hook installed" warning.
        let tmp = tempfile::tempdir().expect("tempdir");
        let claude_dir = tmp.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        let copilot_dir = tmp.path().join(".copilot");
        write_copilot_hook(&copilot_dir, COPILOT_STOCK);
        assert_eq!(
            status_at(Some(&claude_dir), Some(&copilot_dir)),
            HookStatus::Ok
        );
    }

    #[test]
    fn test_status_missing_without_any_integration() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let claude_dir = tmp.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        let copilot_dir = tmp.path().join(".copilot");
        assert_eq!(
            status_at(Some(&claude_dir), Some(&copilot_dir)),
            HookStatus::Missing
        );
        assert_eq!(status_at(Some(&claude_dir), None), HookStatus::Missing);
    }

    #[test]
    fn test_status_invalid_copilot_hook_does_not_suppress_warning() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let claude_dir = tmp.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        let copilot_dir = tmp.path().join(".copilot");
        write_copilot_hook(&copilot_dir, "{ not json");
        assert_eq!(
            status_at(Some(&claude_dir), Some(&copilot_dir)),
            HookStatus::Missing
        );
    }

    #[test]
    fn test_status_valid_claude_hook_still_ok() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let claude_dir = tmp.path().join(".claude");
        write_claude_binary_hook(&claude_dir);
        assert_eq!(status_at(Some(&claude_dir), None), HookStatus::Ok);
    }

    #[test]
    fn test_status_outdated_claude_hook_not_masked_by_copilot() {
        // A real "hook outdated" condition must keep warning even when a
        // valid Copilot hook exists.
        let tmp = tempfile::tempdir().expect("tempdir");
        let claude_dir = tmp.path().join(".claude");
        write_claude_binary_hook(&claude_dir);
        let old_script = claude_dir.join(HOOKS_SUBDIR).join(REWRITE_HOOK_FILE);
        std::fs::create_dir_all(old_script.parent().unwrap()).unwrap();
        std::fs::write(&old_script, "#!/usr/bin/env bash\n").unwrap();
        let copilot_dir = tmp.path().join(".copilot");
        write_copilot_hook(&copilot_dir, COPILOT_STOCK);
        assert_eq!(
            status_at(Some(&claude_dir), Some(&copilot_dir)),
            HookStatus::Outdated
        );
    }

    #[test]
    fn test_other_integration_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(!other_integration_installed(tmp.path()));
    }

    #[test]
    fn test_other_integration_opencode() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp
            .path()
            .join(CONFIG_DIR)
            .join(OPENCODE_SUBDIR)
            .join(PLUGIN_SUBDIR)
            .join(OPENCODE_PLUGIN_FILE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"plugin").unwrap();
        assert!(other_integration_installed(tmp.path()));
    }

    #[test]
    fn test_other_integration_cursor() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp
            .path()
            .join(CURSOR_DIR)
            .join(HOOKS_SUBDIR)
            .join(REWRITE_HOOK_FILE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"hook").unwrap();
        assert!(other_integration_installed(tmp.path()));
    }

    #[test]
    fn test_other_integration_codex() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join(CODEX_DIR).join("AGENTS.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"agents").unwrap();
        assert!(other_integration_installed(tmp.path()));
    }

    #[test]
    fn test_other_integration_gemini() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp
            .path()
            .join(GEMINI_DIR)
            .join(HOOKS_SUBDIR)
            .join(GEMINI_HOOK_FILE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"hook").unwrap();
        assert!(other_integration_installed(tmp.path()));
    }

    #[test]
    fn test_other_integration_hermes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp
            .path()
            .join(HERMES_DIR)
            .join(HERMES_PLUGINS_SUBDIR)
            .join(HERMES_PLUGIN_NAME)
            .join(HERMES_PLUGIN_MANIFEST_FILE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"plugin").unwrap();
        assert!(other_integration_installed(tmp.path()));
    }

    #[test]
    fn test_other_integration_empty_dirs_not_enough() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join(CURSOR_DIR).join(HOOKS_SUBDIR)).unwrap();
        std::fs::create_dir_all(tmp.path().join(CODEX_DIR)).unwrap();
        std::fs::create_dir_all(tmp.path().join(GEMINI_DIR)).unwrap();
        std::fs::create_dir_all(
            tmp.path()
                .join(HERMES_DIR)
                .join(HERMES_PLUGINS_SUBDIR)
                .join(HERMES_PLUGIN_NAME),
        )
        .unwrap();
        assert!(!other_integration_installed(tmp.path()));
    }

    #[test]
    fn test_status_returns_valid_variant() {
        // Skip on machines without Claude Code
        let home = match dirs::home_dir() {
            Some(h) => h,
            None => return,
        };
        let claude_dir = home.join(".claude");
        if !claude_dir.exists() {
            assert_eq!(status(), HookStatus::Ok);
            return;
        }
        // With .claude dir present, status must be one of the valid variants
        let s = status();
        assert!(
            s == HookStatus::Ok || s == HookStatus::Outdated || s == HookStatus::Missing,
            "Expected valid HookStatus variant, got {:?}",
            s
        );
    }
}
