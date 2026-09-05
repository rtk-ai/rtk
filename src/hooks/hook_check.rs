//! Detects whether RTK hooks are installed and warns if they are outdated.

use super::constants::{HOOKS_SUBDIR, PRE_TOOL_USE_KEY, REWRITE_HOOK_FILE, SETTINGS_JSON};
use super::init::resolve_claude_dir;
use super::is_claude_hook_command;
use crate::core::constants::RTK_DATA_DIR;
use crate::core::utils::from_json_str;
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

/// Native Codex hook installation status.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum CodexHookStatus {
    /// A single usable RTK Bash PreToolUse hook is configured.
    Ready,
    /// Codex's config.toml does not exist.
    MissingConfig,
    /// config.toml exists but has no usable RTK Bash hook.
    MissingHook,
    /// Codex config.toml cannot be parsed or has an invalid hook shape.
    InvalidConfig,
    /// More than one RTK Codex hook is registered.
    ConflictingConfig,
    /// The configured absolute RTK executable no longer exists.
    MissingBinary,
}

impl CodexHookStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::MissingConfig => "missing-config",
            Self::MissingHook => "missing-hook",
            Self::InvalidConfig => "invalid-config",
            Self::ConflictingConfig => "conflicting-config",
            Self::MissingBinary => "missing-binary",
        }
    }
}

/// Return true when any supported RTK integration is present for this user.
///
/// `rtk gain` must not report a missing Claude hook when the active agent uses
/// a plugin or a different native hook integration.
pub fn any_integration_installed() -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    integration_installed_in(&home)
}

fn integration_installed_in(home: &std::path::Path) -> bool {
    let paths = [
        home.join(super::constants::CONFIG_DIR)
            .join(super::constants::OPENCODE_SUBDIR)
            .join(super::constants::PLUGIN_SUBDIR)
            .join(super::constants::OPENCODE_PLUGIN_FILE),
        home.join(super::constants::CURSOR_DIR)
            .join(HOOKS_SUBDIR)
            .join(REWRITE_HOOK_FILE),
        home.join(super::constants::GEMINI_DIR)
            .join(HOOKS_SUBDIR)
            .join(super::constants::GEMINI_HOOK_FILE),
        home.join(super::constants::HERMES_DIR)
            .join(super::constants::HERMES_PLUGINS_SUBDIR)
            .join(super::constants::HERMES_PLUGIN_NAME)
            .join(super::constants::HERMES_PLUGIN_MANIFEST_FILE),
    ];
    paths.iter().any(|path| path.is_file()) || codex_integration_installed(home)
}

fn codex_integration_installed(home: &std::path::Path) -> bool {
    let codex_dir = home.join(super::constants::CODEX_DIR);
    let agents_has_rtk = std::fs::read_to_string(codex_dir.join("AGENTS.md"))
        .ok()
        .is_some_and(|content| {
            content.contains("RTK.md") || content.contains("<!-- rtk-instructions")
        });
    let instructions_are_rtk = std::fs::read_to_string(codex_dir.join("RTK.md"))
        .ok()
        .is_some_and(|content| content.contains("<!-- rtk-instructions"));

    if crate::service::debug_enabled() {
        eprintln!(
            "[rtk-debug] hook_check.codex decision={} agents_reference={} instructions_marker={}",
            if agents_has_rtk || instructions_are_rtk {
                "installed"
            } else {
                "not-installed"
            },
            agents_has_rtk,
            instructions_are_rtk
        );
    }
    agents_has_rtk || instructions_are_rtk || codex_status_in(&codex_dir) == CodexHookStatus::Ready
}

/// Inspect the effective Codex config without modifying it or starting Codex.
/// The hook is considered ready only when exactly one RTK Bash handler is
/// registered and an absolute executable path, if supplied, still exists.
pub fn codex_status() -> CodexHookStatus {
    let Some(home) = dirs::home_dir() else {
        return CodexHookStatus::MissingConfig;
    };
    let codex_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| home.join(super::constants::CODEX_DIR));
    codex_status_in(&codex_home)
}

/// Testable Codex hook status inspection for one config directory.
pub fn codex_status_in(codex_home: &Path) -> CodexHookStatus {
    let config_path = codex_home.join("config.toml");
    if !config_path.is_file() {
        return CodexHookStatus::MissingConfig;
    }
    let Ok(content) = std::fs::read_to_string(&config_path) else {
        return CodexHookStatus::InvalidConfig;
    };
    let Ok(root) = content.parse::<toml::Value>() else {
        return CodexHookStatus::InvalidConfig;
    };
    let Some(hooks) = root.get("hooks").and_then(toml::Value::as_table) else {
        return CodexHookStatus::MissingHook;
    };
    let Some(groups) = hooks.get("PreToolUse").and_then(toml::Value::as_array) else {
        return CodexHookStatus::MissingHook;
    };

    let mut rtk_hooks = Vec::new();
    for group in groups {
        let Some(group) = group.as_table() else {
            return CodexHookStatus::InvalidConfig;
        };
        if group.get("matcher").and_then(toml::Value::as_str) != Some("Bash") {
            continue;
        }
        let Some(handlers) = group.get("hooks").and_then(toml::Value::as_array) else {
            return CodexHookStatus::InvalidConfig;
        };
        for handler in handlers {
            let Some(handler) = handler.as_table() else {
                return CodexHookStatus::InvalidConfig;
            };
            let Some(command) = handler.get("command").and_then(toml::Value::as_str) else {
                continue;
            };
            if let Some(executable) = codex_hook_executable(command) {
                rtk_hooks.push(executable.to_string());
            }
        }
    }

    match rtk_hooks.as_slice() {
        [] => CodexHookStatus::MissingHook,
        [_first, _second, ..] => CodexHookStatus::ConflictingConfig,
        [executable] if codex_executable_missing(executable) => CodexHookStatus::MissingBinary,
        [_] => CodexHookStatus::Ready,
    }
}

fn codex_hook_executable(command: &str) -> Option<&str> {
    let executable = command.strip_suffix(" hook codex")?.trim();
    let executable = executable
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            executable
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(executable)
        .trim();
    let binary = executable.rsplit(['/', '\\']).next()?;
    if binary.eq_ignore_ascii_case("rtk") || binary.eq_ignore_ascii_case("rtk.exe") {
        Some(executable)
    } else {
        None
    }
}

fn codex_executable_missing(executable: &str) -> bool {
    if !executable.contains('/') && !executable.contains('\\') {
        // A bare `rtk`/`rtk.exe` is resolved by Codex through PATH; checking
        // it here would make the status depend on the checker's own PATH.
        return false;
    }
    !Path::new(executable).is_file()
}

pub(crate) fn claude_hook_installed_in(claude_dir: &std::path::Path) -> bool {
    binary_hook_registered(claude_dir)
        || claude_dir
            .join(HOOKS_SUBDIR)
            .join(REWRITE_HOOK_FILE)
            .is_file()
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
        return if any_integration_installed() {
            HookStatus::Ok
        } else {
            HookStatus::Missing
        };
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
pub(crate) fn binary_hook_registered(claude_dir: &std::path::Path) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::constants::{
        CODEX_DIR, CONFIG_DIR, CURSOR_DIR, GEMINI_DIR, GEMINI_HOOK_FILE, HERMES_DIR,
        HERMES_PLUGINS_SUBDIR, HERMES_PLUGIN_MANIFEST_FILE, HERMES_PLUGIN_NAME,
        OPENCODE_PLUGIN_FILE, OPENCODE_SUBDIR, PLUGIN_SUBDIR,
    };

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
        assert!(!integration_installed_in(tmp.path()));
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
        assert!(integration_installed_in(tmp.path()));
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
        assert!(integration_installed_in(tmp.path()));
    }

    #[test]
    fn test_other_integration_codex() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join(CODEX_DIR).join("AGENTS.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"unrelated agent instructions").unwrap();
        assert!(!integration_installed_in(tmp.path()));

        std::fs::write(&path, b"@RTK.md").unwrap();
        assert!(integration_installed_in(tmp.path()));
    }

    #[test]
    fn test_other_integration_codex_rtk_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join(CODEX_DIR).join("RTK.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"<!-- rtk-instructions v2 -->").unwrap();
        assert!(integration_installed_in(tmp.path()));
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
        assert!(integration_installed_in(tmp.path()));
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
        assert!(integration_installed_in(tmp.path()));
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
        assert!(!integration_installed_in(tmp.path()));
    }

    #[test]
    fn test_codex_status_requires_a_valid_bash_hook_and_binary() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let rtk = tmp
            .path()
            .join(if cfg!(windows) { "rtk.exe" } else { "rtk" });
        std::fs::write(&rtk, b"rtk").expect("write fake rtk");
        let rtk_toml_path = rtk.to_string_lossy().replace('\\', "/");
        let config = format!(
            "[mcp_servers.rtk]\ncommand = \"{}\"\nargs = [\"mcp\"]\n\n[hooks]\n[[hooks.PreToolUse]]\nmatcher = \"Bash\"\n[[hooks.PreToolUse.hooks]]\ntype = \"command\"\ncommand = \"{} hook codex\"\n",
            rtk_toml_path,
            rtk_toml_path
        );
        std::fs::write(tmp.path().join("config.toml"), config).expect("write config");

        assert_eq!(codex_status_in(tmp.path()), CodexHookStatus::Ready);
    }

    #[test]
    fn test_codex_status_reports_missing_binary_and_duplicate_hooks() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("config.toml"),
            "[hooks]\n[[hooks.PreToolUse]]\nmatcher = \"Bash\"\n[[hooks.PreToolUse.hooks]]\ntype = \"command\"\ncommand = \"C:\\\\missing\\\\rtk.exe hook codex\"\n[[hooks.PreToolUse.hooks]]\ntype = \"command\"\ncommand = \"rtk hook codex\"\n",
        )
        .expect("write config");

        assert_eq!(
            codex_status_in(tmp.path()),
            CodexHookStatus::ConflictingConfig
        );
    }

    #[test]
    fn test_codex_status_reports_missing_config_and_hook() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(codex_status_in(tmp.path()), CodexHookStatus::MissingConfig);

        std::fs::write(tmp.path().join("config.toml"), "model = \"gpt-test\"\n")
            .expect("write config");
        assert_eq!(codex_status_in(tmp.path()), CodexHookStatus::MissingHook);
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
