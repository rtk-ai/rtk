//! Detects whether RTK hooks are installed and warns if they are outdated.

use super::constants::{
    CLAUDE_HOOK_COMMAND, HOOKS_SUBDIR, PRE_TOOL_USE_KEY, REWRITE_HOOK_FILE, SETTINGS_JSON,
    SETTINGS_LOCAL_JSON,
};
use super::init::resolve_claude_dir;
use super::is_claude_hook_invocation;
use crate::core::constants::RTK_DATA_DIR;
use crate::discover::lexer::shell_split;
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

/// Wrapper scripts larger than this are not inspected (avoid reading arbitrary large files).
const MAX_WRAPPER_SCRIPT_BYTES: u64 = 64 * 1024;

/// Check if the native binary command is registered in settings.json or
/// settings.local.json, directly or via a wrapper script (e.g.
/// `bash ~/.claude/hooks/rtk-pipe-guard.sh`) that pipes into it.
fn binary_hook_registered(claude_dir: &std::path::Path) -> bool {
    [SETTINGS_JSON, SETTINGS_LOCAL_JSON]
        .iter()
        .any(|file_name| settings_file_registers_hook(claude_dir, file_name))
}

fn settings_file_registers_hook(claude_dir: &std::path::Path, file_name: &str) -> bool {
    let settings_path = claude_dir.join(file_name);
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
    let commands: Vec<&str> = pre_tool_use
        .iter()
        .filter_map(|entry| entry.get("hooks")?.as_array())
        .flatten()
        .filter_map(|hook| hook.get("command")?.as_str())
        .collect();

    commands.iter().any(|cmd| is_claude_hook_invocation(cmd))
        || commands
            .iter()
            .any(|cmd| wrapper_script_references_hook(cmd, claude_dir))
}

/// True if `cmd` invokes a wrapper script (directly, or via an interpreter/
/// `env` prefix — see [`super::skip_wrapper_prefix`]) whose contents
/// reference the rtk hook command.
fn wrapper_script_references_hook(cmd: &str, claude_dir: &std::path::Path) -> bool {
    let tokens = shell_split(cmd);
    let Some(token) = super::skip_wrapper_prefix(&tokens).first() else {
        return false;
    };
    if token.starts_with('$') {
        return false; // unresolvable env-var path — can't be probed on disk
    }

    let home = dirs::home_dir();
    let script_path = if let Some(rest) = token.strip_prefix("~/") {
        match home {
            Some(h) => h.join(rest),
            None => return false,
        }
    } else if token.as_str() == "~" {
        match home {
            Some(h) => h.to_path_buf(),
            None => return false,
        }
    } else {
        let candidate = PathBuf::from(token);
        if candidate.is_absolute() {
            candidate
        } else {
            claude_dir.join(candidate)
        }
    };

    let Ok(meta) = std::fs::metadata(&script_path) else {
        return false;
    };
    if !meta.is_file() || meta.len() > MAX_WRAPPER_SCRIPT_BYTES {
        return false;
    }
    let Ok(content) = std::fs::read_to_string(&script_path) else {
        return false;
    };
    content
        .lines()
        .any(|line| !line.trim_start().starts_with('#') && line.contains(CLAUDE_HOOK_COMMAND))
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

    fn write_settings(claude_dir: &std::path::Path, file_name: &str, command: &str) {
        std::fs::create_dir_all(claude_dir).unwrap();
        let json = format!(
            r#"{{"hooks":{{"{}":[{{"hooks":[{{"command":"{}"}}]}}]}}}}"#,
            PRE_TOOL_USE_KEY,
            command.replace('\\', "\\\\").replace('"', "\\\"")
        );
        std::fs::write(claude_dir.join(file_name), json).unwrap();
    }

    #[test]
    fn test_binary_hook_registered_missing_settings() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path()).unwrap();
        assert!(!binary_hook_registered(tmp.path()));
    }

    #[test]
    fn test_binary_hook_registered_settings_local_only() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_settings(tmp.path(), SETTINGS_LOCAL_JSON, CLAUDE_HOOK_COMMAND);
        assert!(binary_hook_registered(tmp.path()));
    }

    #[test]
    fn test_binary_hook_registered_inline_wrapper_string() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_settings(tmp.path(), SETTINGS_JSON, r#"bash -c "rtk hook claude""#);
        assert!(binary_hook_registered(tmp.path()));
    }

    #[test]
    fn test_binary_hook_registered_wrapper_script_with_hook_string() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let claude_dir = tmp.path();
        let script_path = claude_dir.join(HOOKS_SUBDIR).join("rtk-pipe-guard.sh");
        std::fs::create_dir_all(script_path.parent().unwrap()).unwrap();
        std::fs::write(
            &script_path,
            format!("#!/bin/bash\ncat | {}\n", CLAUDE_HOOK_COMMAND),
        )
        .unwrap();
        let command = format!("bash {}", script_path.display());
        write_settings(claude_dir, SETTINGS_JSON, &command);
        assert!(binary_hook_registered(claude_dir));
    }

    #[test]
    fn test_binary_hook_registered_wrapper_script_without_hook_string() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let claude_dir = tmp.path();
        let script_path = claude_dir.join(HOOKS_SUBDIR).join("unrelated.sh");
        std::fs::create_dir_all(script_path.parent().unwrap()).unwrap();
        std::fs::write(&script_path, "#!/bin/bash\necho hi\n").unwrap();
        let command = format!("bash {}", script_path.display());
        write_settings(claude_dir, SETTINGS_JSON, &command);
        assert!(!binary_hook_registered(claude_dir));
    }

    #[test]
    fn test_binary_hook_registered_wrapper_script_hook_only_in_comment() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let claude_dir = tmp.path();
        let script_path = claude_dir.join(HOOKS_SUBDIR).join("rtk-pipe-guard.sh");
        std::fs::create_dir_all(script_path.parent().unwrap()).unwrap();
        std::fs::write(
            &script_path,
            format!("#!/bin/bash\n# {}\necho hi\n", CLAUDE_HOOK_COMMAND),
        )
        .unwrap();
        let command = format!("bash {}", script_path.display());
        write_settings(claude_dir, SETTINGS_JSON, &command);
        assert!(!binary_hook_registered(claude_dir));
    }

    #[test]
    fn test_binary_hook_registered_wrapper_script_hook_on_piped_line() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let claude_dir = tmp.path();
        let script_path = claude_dir.join(HOOKS_SUBDIR).join("rtk-pipe-guard.sh");
        std::fs::create_dir_all(script_path.parent().unwrap()).unwrap();
        std::fs::write(
            &script_path,
            format!(
                "#!/bin/bash\nprintf '%s' \"$INPUT\" | {}\n",
                CLAUDE_HOOK_COMMAND
            ),
        )
        .unwrap();
        let command = format!("bash {}", script_path.display());
        write_settings(claude_dir, SETTINGS_JSON, &command);
        assert!(binary_hook_registered(claude_dir));
    }

    #[test]
    fn test_binary_hook_registered_wrapper_quoted_path_with_space() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let claude_dir = tmp.path();
        let script_path = claude_dir.join(HOOKS_SUBDIR).join("rtk pipe guard.sh");
        std::fs::create_dir_all(script_path.parent().unwrap()).unwrap();
        std::fs::write(
            &script_path,
            format!("#!/bin/bash\n{}\n", CLAUDE_HOOK_COMMAND),
        )
        .unwrap();
        let command = format!("bash \"{}\"", script_path.display());
        write_settings(claude_dir, SETTINGS_JSON, &command);
        assert!(binary_hook_registered(claude_dir));
    }

    #[test]
    fn test_binary_hook_registered_wrapper_bin_bash_prefix() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let claude_dir = tmp.path();
        let script_path = claude_dir.join(HOOKS_SUBDIR).join("rtk-pipe-guard.sh");
        std::fs::create_dir_all(script_path.parent().unwrap()).unwrap();
        std::fs::write(
            &script_path,
            format!("#!/bin/bash\n{}\n", CLAUDE_HOOK_COMMAND),
        )
        .unwrap();
        let command = format!("/bin/bash {}", script_path.display());
        write_settings(claude_dir, SETTINGS_JSON, &command);
        assert!(binary_hook_registered(claude_dir));
    }

    #[test]
    fn test_binary_hook_registered_wrapper_env_prefix() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let claude_dir = tmp.path();
        let script_path = claude_dir.join(HOOKS_SUBDIR).join("rtk-pipe-guard.sh");
        std::fs::create_dir_all(script_path.parent().unwrap()).unwrap();
        std::fs::write(
            &script_path,
            format!("#!/bin/bash\n{}\n", CLAUDE_HOOK_COMMAND),
        )
        .unwrap();
        let command = format!("env RTK_LOG=1 bash {}", script_path.display());
        write_settings(claude_dir, SETTINGS_JSON, &command);
        assert!(binary_hook_registered(claude_dir));
    }

    /// Serialises tests that mutate the process-wide `HOME` env var.
    static HOME_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_wrapper_script_tilde_resolves_via_home_not_claude_dir_parent() {
        let _guard = HOME_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let fake_home = tempfile::tempdir().expect("tempdir");
        let script_path = fake_home
            .path()
            .join(HOOKS_SUBDIR)
            .join("rtk-pipe-guard.sh");
        std::fs::create_dir_all(script_path.parent().unwrap()).unwrap();
        std::fs::write(
            &script_path,
            format!("#!/bin/bash\n{}\n", CLAUDE_HOOK_COMMAND),
        )
        .unwrap();

        // claude_dir sits far away from fake_home, so claude_dir.parent()
        // would resolve "~/..." to the wrong directory — only dirs::home_dir()
        // (backed by $HOME) resolves it to fake_home.
        let unrelated = tempfile::tempdir().expect("tempdir");
        let claude_dir = unrelated.path().join("nested").join("claude_config");
        std::fs::create_dir_all(&claude_dir).unwrap();

        let orig_home = std::env::var_os("HOME");
        std::env::set_var("HOME", fake_home.path());
        let command = "bash ~/hooks/rtk-pipe-guard.sh".to_string();
        write_settings(&claude_dir, SETTINGS_JSON, &command);
        let result = binary_hook_registered(&claude_dir);
        match orig_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }

        assert!(result);
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
