//! Detects whether RTK hooks are installed and warns if they are outdated.

use super::constants::{
    CODEX_DIR, CONFIG_DIR, CURSOR_DIR, DROID_DIR, GEMINI_DIR, GEMINI_HOOK_FILE, HERMES_DIR,
    HERMES_PLUGINS_SUBDIR, HERMES_PLUGIN_MANIFEST_FILE, HERMES_PLUGIN_NAME, HOOKS_JSON,
    HOOKS_SUBDIR, OPENCODE_PLUGIN_FILE, OPENCODE_SUBDIR, PLUGIN_SUBDIR, PRE_TOOL_USE_KEY,
    REWRITE_HOOK_FILE, SETTINGS_JSON, VIBE_DIR, VIBE_HOOKS_FILE,
};
use super::init::{resolve_claude_dir, AGENTS_MD};
use super::is_claude_hook_command;
use crate::core::constants::RTK_DATA_DIR;
use crate::core::utils::from_json_str;
use std::path::PathBuf;

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
    // Don't warn users who don't have Claude Code installed
    let claude_dir = match resolve_claude_dir() {
        Ok(d) => d,
        Err(_) => return HookStatus::Ok,
    };
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
        // No Claude hook — but a `~/.claude` directory only means Claude Code
        // has been run on this machine, not that it is the agent in use. A user
        // whose agent is Codex, Antigravity, Gemini, Cursor, Droid, Hermes or
        // Vibe would otherwise be told "No hook installed" forever, with a fix
        // (`rtk init -g`) that targets an agent they do not use.
        if any_other_integration_installed().unwrap_or(false) {
            return HookStatus::Ok;
        }
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
    let root: serde_json::Value = match from_json_str(&content) {
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

fn hook_installed_path() -> Option<PathBuf> {
    let claude_dir = resolve_claude_dir().ok()?;
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

/// Artifacts RTK writes when it integrates with an agent other than Claude
/// Code, relative to the user's home directory.
///
/// Deliberately a list of *installed artifacts* rather than of *agents*: an
/// agent being present on the machine says nothing about whether RTK is wired
/// into it, and that is the only question `status` needs answered.
fn other_integration_paths(home: &std::path::Path) -> [PathBuf; 8] {
    [
        // OpenCode / Pi-style TypeScript plugin
        home.join(CONFIG_DIR)
            .join(OPENCODE_SUBDIR)
            .join(PLUGIN_SUBDIR)
            .join(OPENCODE_PLUGIN_FILE),
        // Cursor
        home.join(CURSOR_DIR)
            .join(HOOKS_SUBDIR)
            .join(REWRITE_HOOK_FILE),
        // Codex CLI
        home.join(CODEX_DIR).join(AGENTS_MD),
        home.join(CODEX_DIR).join(HOOKS_JSON),
        // Gemini CLI
        home.join(GEMINI_DIR)
            .join(HOOKS_SUBDIR)
            .join(GEMINI_HOOK_FILE),
        // Hermes
        home.join(HERMES_DIR)
            .join(HERMES_PLUGINS_SUBDIR)
            .join(HERMES_PLUGIN_NAME)
            .join(HERMES_PLUGIN_MANIFEST_FILE),
        // Factory Droid
        home.join(DROID_DIR).join(HOOKS_JSON),
        // Mistral Vibe
        home.join(VIBE_DIR).join(VIBE_HOOKS_FILE),
    ]
}

/// True when RTK is integrated with some agent other than Claude Code.
fn other_integration_installed(home: &std::path::Path) -> bool {
    other_integration_paths(home).iter().any(|p| p.exists())
}

/// Same question against the real home directory. `None` when the home
/// directory cannot be resolved — the caller then has no evidence either way
/// and must not treat that as "no other integration".
fn any_other_integration_installed() -> Option<bool> {
    Some(other_integration_installed(&dirs::home_dir()?))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_other_integration_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(!other_integration_installed(tmp.path()));
    }

    /// Covers every artifact `status` consults, so an integration added later
    /// that forgets this list fails here rather than in a user's terminal.
    #[test]
    fn test_other_integration_detects_every_listed_artifact() {
        for probe in other_integration_paths(std::path::Path::new("/probe")) {
            let tmp = tempfile::tempdir().expect("tempdir");
            let relative = probe
                .strip_prefix("/probe")
                .expect("paths are built from the home directory");
            let path = tmp.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"installed").unwrap();
            assert!(
                other_integration_installed(tmp.path()),
                "{} must count as an installed integration",
                relative.display()
            );
        }
    }

    #[test]
    fn test_other_integration_droid_vibe_and_codex_hook() {
        // Droid, Vibe and the Codex hook post-date the original list, so their
        // users were told "No hook installed" indefinitely.
        for relative in [
            std::path::Path::new(DROID_DIR).join(HOOKS_JSON),
            std::path::Path::new(VIBE_DIR).join(VIBE_HOOKS_FILE),
            std::path::Path::new(CODEX_DIR).join(HOOKS_JSON),
        ] {
            let tmp = tempfile::tempdir().expect("tempdir");
            let path = tmp.path().join(&relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"hook").unwrap();
            assert!(
                other_integration_installed(tmp.path()),
                "{} must count as an installed integration",
                relative.display()
            );
        }
    }

    #[test]
    fn test_other_integration_ignores_unrelated_files() {
        // Detection keys on RTK's own artifacts. An agent's config directory
        // existing says nothing about whether RTK is wired into it.
        let tmp = tempfile::tempdir().expect("tempdir");
        for relative in [
            std::path::Path::new(CODEX_DIR).join("config.toml"),
            std::path::Path::new(CURSOR_DIR).join("mcp.json"),
            std::path::Path::new(GEMINI_DIR).join("settings.json"),
        ] {
            let path = tmp.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"unrelated").unwrap();
        }
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
