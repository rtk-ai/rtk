//! Detects whether RTK hooks are installed and warns if they are outdated.

use super::constants::{
    CLAUDE_DIR, CLAUDE_HOOK_COMMAND, CURSOR_DIR, CURSOR_HOOK_COMMAND, HOOKS_JSON, HOOKS_SUBDIR,
    PRE_TOOL_USE_KEY, REWRITE_HOOK_FILE, SETTINGS_JSON,
};
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
/// Returns `Ok` if no Claude Code is detected (not applicable).
pub fn status() -> HookStatus {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return HookStatus::Ok,
    };

    // Cursor 3.x users register the hook in `~/.cursor/hooks.json` directly
    // (no `.claude/` directory required). If that file references `rtk hook
    // cursor` (directly or via a wrapper script that invokes rtk), treat the
    // hook as installed and skip the legacy Claude detection path.
    if cursor_3x_hook_registered(&home) {
        return HookStatus::Ok;
    }

    // Don't warn users who don't have Claude Code installed
    let claude_dir = home.join(CLAUDE_DIR);
    if !claude_dir.exists() {
        return HookStatus::Ok;
    }

    // Check for new binary command in settings.json first
    if binary_hook_registered(&claude_dir) {
        // If old script file still exists alongside new command, report Outdated
        // (migration not complete — user should run `rtk init -g` to clean up)
        let old_hook = claude_dir.join(HOOKS_SUBDIR).join(REWRITE_HOOK_FILE);
        if old_hook.exists() {
            return HookStatus::Outdated;
        }
        return HookStatus::Ok;
    }

    // Fall back to legacy script file check
    let Some(hook_path) = hook_installed_path() else {
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

/// Check whether `~/.cursor/hooks.json` registers an rtk-aware hook (Cursor 3.x format).
///
/// Cursor 3.x adopts a project-style hook config at `~/.cursor/hooks.json` with
/// a top-level `version: 1` and a `hooks` map keyed by hook event names
/// (`preToolUse`, `beforeShellExecution`, etc.). Each entry has a `command`
/// string that Cursor invokes with the tool input piped on stdin. This is a
/// distinct surface from `~/.claude/settings.json`, so users who configure
/// Cursor 3.x correctly were still seeing the "No hook installed" warning.
///
/// Returns true if the file exists, is valid JSON, and any hook entry under
/// `preToolUse` or `beforeShellExecution` has a `command` that either invokes
/// `rtk hook cursor` directly or references the `rtk` binary through a
/// platform-specific wrapper (PowerShell/bash/batch). The wrapper case covers
/// users on Windows + Cursor where the `permission: "ask"` quirk requires a
/// thin shim returning `permission: "allow"` (see rtk-ai/rtk#1718).
fn cursor_3x_hook_registered(home: &Path) -> bool {
    let cursor_hooks = home.join(CURSOR_DIR).join(HOOKS_JSON);
    let Ok(content) = std::fs::read_to_string(&cursor_hooks) else {
        return false;
    };
    let root: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let hooks = match root.get("hooks") {
        Some(h) => h,
        None => return false,
    };
    // Inspect the hook arrays Cursor uses for command interception.
    for key in &["preToolUse", "beforeShellExecution"] {
        if let Some(arr) = hooks.get(key).and_then(|v| v.as_array()) {
            let any_rtk = arr
                .iter()
                .filter_map(|entry| entry.get("command")?.as_str())
                .any(|cmd| {
                    cmd == CURSOR_HOOK_COMMAND
                        || cmd.contains(CURSOR_HOOK_COMMAND)
                        || cmd.contains("rtk")
                });
            if any_rtk {
                return true;
            }
        }
    }
    false
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
        .any(|cmd| cmd == CLAUDE_HOOK_COMMAND)
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
    let _ = std::fs::create_dir_all(marker.parent()?);
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

fn hook_installed_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let path = home
        .join(CLAUDE_DIR)
        .join(HOOKS_SUBDIR)
        .join(REWRITE_HOOK_FILE);
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

    // --- Cursor 3.x hook detection ---

    fn write_cursor_hooks_json(home: &std::path::Path, contents: &str) {
        let dir = home.join(CURSOR_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(HOOKS_JSON), contents).unwrap();
    }

    #[test]
    fn test_cursor_3x_hook_registered_native_command() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_cursor_hooks_json(
            tmp.path(),
            r#"{
                "version": 1,
                "hooks": {
                    "preToolUse": [
                        { "command": "rtk hook cursor", "matcher": "Shell" }
                    ]
                }
            }"#,
        );
        assert!(cursor_3x_hook_registered(tmp.path()));
    }

    #[test]
    fn test_cursor_3x_hook_registered_wrapper_script() {
        // Users on Windows + Cursor sometimes need a thin wrapper around
        // `rtk hook cursor` until rtk-ai/rtk#1718 (BOM strip + permission:"allow"
        // + continue:true) is merged. Detection should still recognize that
        // wrapper as an rtk-aware hook so the warning is suppressed.
        let tmp = tempfile::tempdir().expect("tempdir");
        write_cursor_hooks_json(
            tmp.path(),
            r#"{
                "version": 1,
                "hooks": {
                    "preToolUse": [
                        {
                            "command": "powershell.exe -File C:/Users/foo/.cursor/hooks/rtk-cursor-allow.ps1",
                            "matcher": "Shell"
                        }
                    ]
                }
            }"#,
        );
        assert!(cursor_3x_hook_registered(tmp.path()));
    }

    #[test]
    fn test_cursor_3x_hook_registered_before_shell_execution() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_cursor_hooks_json(
            tmp.path(),
            r#"{
                "version": 1,
                "hooks": {
                    "beforeShellExecution": [
                        { "command": "rtk hook cursor" }
                    ]
                }
            }"#,
        );
        assert!(cursor_3x_hook_registered(tmp.path()));
    }

    #[test]
    fn test_cursor_3x_hook_not_registered_no_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(!cursor_3x_hook_registered(tmp.path()));
    }

    #[test]
    fn test_cursor_3x_hook_not_registered_unrelated_command() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_cursor_hooks_json(
            tmp.path(),
            r#"{
                "version": 1,
                "hooks": {
                    "preToolUse": [
                        { "command": "./scripts/audit.sh" }
                    ]
                }
            }"#,
        );
        assert!(!cursor_3x_hook_registered(tmp.path()));
    }

    #[test]
    fn test_cursor_3x_hook_not_registered_invalid_json() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_cursor_hooks_json(tmp.path(), "{ this is not json");
        assert!(!cursor_3x_hook_registered(tmp.path()));
    }

    #[test]
    fn test_cursor_3x_hook_not_registered_empty_hooks() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_cursor_hooks_json(tmp.path(), r#"{ "version": 1, "hooks": {} }"#);
        assert!(!cursor_3x_hook_registered(tmp.path()));
    }
}
