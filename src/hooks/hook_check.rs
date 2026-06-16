//! Detects whether RTK hooks are installed and warns if they are outdated.

use super::constants::{
    CLAUDE_DIR, CLAUDE_HOOK_COMMAND, CODEX_DIR, CODEX_HOOK_COMMAND, HOOKS_JSON, HOOKS_SUBDIR,
    PRE_TOOL_USE_KEY, REWRITE_HOOK_FILE, SETTINGS_JSON, SETTINGS_LOCAL_JSON,
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
    let Some(home) = dirs::home_dir() else {
        return HookStatus::Ok;
    };

    status_for_home(&home)
}

fn status_for_home(home: &Path) -> HookStatus {
    let claude_dir = home.join(CLAUDE_DIR);
    if !claude_dir.exists() {
        return HookStatus::Ok;
    }

    // Check for new binary command in Claude settings first. A Codex hook
    // must not hide missing Claude hook coverage when Claude is installed.
    if binary_hook_registered(&claude_dir) {
        // If old script file still exists alongside new command, report Outdated
        // (migration not complete — user should run `rtk init -g` to clean up)
        let old_hook = claude_dir.join(HOOKS_SUBDIR).join(REWRITE_HOOK_FILE);
        if old_hook.exists() {
            return HookStatus::Outdated;
        }
        return HookStatus::Ok;
    }
    if binary_hook_partially_registered(&claude_dir) {
        return HookStatus::Outdated;
    }

    // Fall back to legacy script file check
    let Some(hook_path) = hook_installed_path_for_home(home) else {
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

/// Check if the native binary command is registered in both Claude settings files.
///
/// Claude print/SDK paths can ignore the durable `settings.local.json` fallback.
/// Treat one-file coverage as incomplete so `rtk gain` and diagnostics do not
/// claim auto-rewrite is active when a host mode can silently skip PreToolUse.
fn binary_hook_registered(claude_dir: &std::path::Path) -> bool {
    [SETTINGS_JSON, SETTINGS_LOCAL_JSON]
        .iter()
        .all(|file_name| {
            hook_command_registered_in_json(&claude_dir.join(file_name), CLAUDE_HOOK_COMMAND)
        })
}

fn binary_hook_partially_registered(claude_dir: &std::path::Path) -> bool {
    [SETTINGS_JSON, SETTINGS_LOCAL_JSON]
        .iter()
        .any(|file_name| {
            hook_command_registered_in_json(&claude_dir.join(file_name), CLAUDE_HOOK_COMMAND)
        })
}

#[allow(dead_code)]
fn codex_hook_registered(home: &Path) -> bool {
    hook_command_registered_in_json(&home.join(CODEX_DIR).join(HOOKS_JSON), CODEX_HOOK_COMMAND)
}

fn hook_command_registered_in_json(path: &Path, command: &str) -> bool {
    let content = match std::fs::read_to_string(path) {
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
    ["Bash", "Shell", "PowerShell"].iter().all(|matcher| {
        pre_tool_use.iter().any(|entry| {
            entry
                .get("matcher")
                .and_then(|m| m.as_str())
                .is_some_and(|m| m.eq_ignore_ascii_case(matcher))
                && entry
                    .get("hooks")
                    .and_then(|h| h.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|hook| hook.get("command")?.as_str())
                    .any(|cmd| cmd == command)
        })
    })
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
    hook_installed_path_for_home(&home)
}

fn hook_installed_path_for_home(home: &Path) -> Option<PathBuf> {
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

    fn complete_hook_json(command: &str) -> String {
        format!(
            r#"{{
              "hooks": {{
                "PreToolUse": [
                  {{"matcher": "Bash", "hooks": [{{"type": "command", "command": "{command}"}}]}},
                  {{"matcher": "Shell", "hooks": [{{"type": "command", "command": "{command}"}}]}},
                  {{"matcher": "PowerShell", "hooks": [{{"type": "command", "command": "{command}"}}]}}
                ]
              }}
            }}"#
        )
    }

    #[test]
    fn test_binary_hook_registered_requires_both_settings_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let claude_dir = tmp.path().join(CLAUDE_DIR);
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(
            claude_dir.join(SETTINGS_LOCAL_JSON),
            complete_hook_json(CLAUDE_HOOK_COMMAND),
        )
        .unwrap();

        assert!(!binary_hook_registered(&claude_dir));
        assert!(binary_hook_partially_registered(&claude_dir));

        std::fs::write(
            claude_dir.join(SETTINGS_JSON),
            complete_hook_json(CLAUDE_HOOK_COMMAND),
        )
        .unwrap();

        assert!(binary_hook_registered(&claude_dir));
    }

    #[test]
    fn test_binary_hook_registered_rejects_missing_matchers() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let claude_dir = tmp.path().join(CLAUDE_DIR);
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(
            claude_dir.join(SETTINGS_LOCAL_JSON),
            r#"{
              "hooks": {
                "PreToolUse": [
                  {"matcher": "Bash", "hooks": [{"type": "command", "command": "rtk hook claude"}]}
                ]
              }
            }"#,
        )
        .unwrap();

        assert!(!binary_hook_registered(&claude_dir));
    }

    #[test]
    fn test_codex_hook_does_not_mask_missing_claude_hook() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let claude_dir = tmp.path().join(CLAUDE_DIR);
        std::fs::create_dir_all(&claude_dir).unwrap();
        let codex_dir = tmp.path().join(CODEX_DIR);
        std::fs::create_dir_all(&codex_dir).unwrap();
        std::fs::write(
            codex_dir.join(HOOKS_JSON),
            complete_hook_json(CODEX_HOOK_COMMAND),
        )
        .unwrap();

        assert!(codex_hook_registered(tmp.path()));
        assert_eq!(status_for_home(tmp.path()), HookStatus::Missing);
    }

    #[test]
    fn test_status_for_home_marks_local_only_claude_hook_outdated() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let claude_dir = tmp.path().join(CLAUDE_DIR);
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(
            claude_dir.join(SETTINGS_LOCAL_JSON),
            complete_hook_json(CLAUDE_HOOK_COMMAND),
        )
        .unwrap();

        assert_eq!(status_for_home(tmp.path()), HookStatus::Outdated);
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
