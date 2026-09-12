//! Detects whether RTK hooks are installed and warns if they are outdated.

use super::constants::{
    CODEX_DIR, CONFIG_DIR, COPILOT_HOOK_FILE, CURSOR_DIR, CURSOR_HOOK_COMMAND, DROID_DIR,
    DROID_HOOKS_FILE, DROID_HOOKS_SUBDIR, DROID_HOOK_COMMAND, DROID_SETTINGS_FILE, GEMINI_DIR,
    GEMINI_HOOK_FILE, GITHUB_DIR, HERMES_DIR, HERMES_PLUGINS_SUBDIR, HERMES_PLUGIN_INIT_FILE,
    HERMES_PLUGIN_MANIFEST_FILE, HERMES_PLUGIN_NAME, HOOKS_JSON, HOOKS_SUBDIR, OMP_LOCAL_DIR,
    OPENCODE_PLUGIN_FILE, OPENCODE_SUBDIR, PI_EXTENSIONS_SUBDIR, PI_LOCAL_DIR, PI_PLUGIN_FILE,
    PLUGIN_SUBDIR, PRE_TOOL_USE_KEY, REWRITE_HOOK_FILE, SETTINGS_JSON, VIBE_DIR, VIBE_HOOKS_FILE,
    VIBE_HOOK_COMMAND, VIBE_HOOK_NAME,
};
use super::init::{
    copilot_user_dir, omp_extension_path_for_scope, pi_plugin_path_for_scope, resolve_claude_dir,
    resolve_droid_dir,
};
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
        return status_with_other_integration(HookStatus::Missing, other_integration_installed());
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
    let Some(warning) = warning_for_status(status()) else {
        return Some(());
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

fn warning_for_status(status: HookStatus) -> Option<&'static str> {
    match status {
        HookStatus::Ok => None,
        HookStatus::Missing => {
            Some("[rtk] /!\\ No hook installed — run `rtk init -g` for automatic token savings")
        }
        HookStatus::Outdated => Some("[rtk] /!\\ Hook outdated — run `rtk init -g` to update"),
    }
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

fn status_with_other_integration(status: HookStatus, has_other_integration: bool) -> HookStatus {
    if status == HookStatus::Missing && has_other_integration {
        HookStatus::Ok
    } else {
        status
    }
}

/// Return whether an executable non-Claude integration is configured with RTK.
/// Codex is retained as the established instruction-only exception; the other
/// instruction-only install targets are deliberately not inferred here.
///
/// This only suppresses a missing Claude warning; an outdated Claude hook is
/// still reported so users can complete its migration.
fn other_integration_installed() -> bool {
    dirs::home_dir().is_some_and(|home| other_integration_installed_at(&home))
        || pi_or_omp_global_integration_installed()
        || droid_global_integration_installed()
        || copilot_global_integration_installed()
        || std::env::current_dir().is_ok_and(|cwd| project_integration_installed_at(&cwd))
}

fn other_integration_installed_at(home: &Path) -> bool {
    opencode_plugin_registered(
        &home
            .join(CONFIG_DIR)
            .join(OPENCODE_SUBDIR)
            .join(PLUGIN_SUBDIR)
            .join(OPENCODE_PLUGIN_FILE),
    ) || cursor_hook_registered(&home.join(CURSOR_DIR).join(HOOKS_JSON))
        || codex_instructions_registered(&home.join(CODEX_DIR))
        || gemini_hook_registered(&home.join(GEMINI_DIR))
        || hermes_plugin_registered(&home.join(HERMES_DIR))
        || vibe_hook_registered(&home.join(VIBE_DIR).join(VIBE_HOOKS_FILE))
}

fn pi_or_omp_global_integration_installed() -> bool {
    [
        pi_plugin_path_for_scope(true),
        omp_extension_path_for_scope(true),
    ]
    .into_iter()
    .filter_map(Result::ok)
    .any(|path| pi_extension_registered(&path))
}

fn droid_global_integration_installed() -> bool {
    resolve_droid_dir().is_ok_and(|dir| droid_hook_registered_in(&dir))
}

fn copilot_global_integration_installed() -> bool {
    copilot_user_dir().is_ok_and(|dir| copilot_hook_registered(&dir.join(HOOKS_SUBDIR)))
}

fn project_integration_installed_at(project_dir: &Path) -> bool {
    pi_extension_registered(
        &project_dir
            .join(PI_LOCAL_DIR)
            .join(PI_EXTENSIONS_SUBDIR)
            .join(PI_PLUGIN_FILE),
    ) || pi_extension_registered(
        &project_dir
            .join(OMP_LOCAL_DIR)
            .join(PI_EXTENSIONS_SUBDIR)
            .join(PI_PLUGIN_FILE),
    ) || droid_hook_registered_in(&project_dir.join(DROID_DIR))
        || copilot_hook_registered(&project_dir.join(GITHUB_DIR).join(HOOKS_SUBDIR))
}

fn opencode_plugin_registered(path: &Path) -> bool {
    read_file(path).is_some_and(|content| {
        content.contains("export const RtkOpenCodePlugin") && content.contains("rtk rewrite")
    })
}

fn cursor_hook_registered(path: &Path) -> bool {
    read_json(path).is_some_and(|root| {
        root.get("hooks")
            .and_then(|hooks| hooks.get("preToolUse"))
            .and_then(serde_json::Value::as_array)
            .is_some_and(|entries| {
                entries.iter().any(|entry| {
                    entry
                        .get("command")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|command| command == CURSOR_HOOK_COMMAND)
                })
            })
    })
}

fn codex_instructions_registered(codex_dir: &Path) -> bool {
    let agents_path = codex_dir.join("AGENTS.md");
    let rtk_md_path = codex_dir.join("RTK.md");
    let rtk_md_ref = format!("@{}", rtk_md_path.display());
    rtk_md_path.exists()
        && read_file(&agents_path).is_some_and(|content| {
            content
                .lines()
                .map(str::trim)
                .any(|line| line == "@RTK.md" || line == rtk_md_ref)
        })
}

fn gemini_hook_registered(gemini_dir: &Path) -> bool {
    let hook_path = gemini_dir.join(HOOKS_SUBDIR).join(GEMINI_HOOK_FILE);
    let hook_command = hook_path.to_string_lossy();
    read_file(&hook_path).is_some_and(|content| content.contains("rtk hook gemini"))
        && read_json(&gemini_dir.join(SETTINGS_JSON)).is_some_and(|root| {
            root.get("hooks")
                .and_then(|hooks| hooks.get("BeforeTool"))
                .and_then(serde_json::Value::as_array)
                .is_some_and(|entries| {
                    entries.iter().any(|entry| {
                        entry
                            .get("hooks")
                            .and_then(serde_json::Value::as_array)
                            .is_some_and(|hooks| {
                                hooks.iter().any(|hook| {
                                    hook.get("command").and_then(serde_json::Value::as_str)
                                        == Some(hook_command.as_ref())
                                })
                            })
                    })
                })
        })
}

fn hermes_plugin_registered(hermes_dir: &Path) -> bool {
    let plugin_dir = hermes_dir
        .join(HERMES_PLUGINS_SUBDIR)
        .join(HERMES_PLUGIN_NAME);
    read_file(&plugin_dir.join(HERMES_PLUGIN_MANIFEST_FILE)).is_some_and(|content| {
        content
            .lines()
            .any(|line| line.trim() == "name: rtk-rewrite")
    }) && read_file(&plugin_dir.join(HERMES_PLUGIN_INIT_FILE))
        .is_some_and(|content| content.contains("rtk rewrite"))
        && read_file(&hermes_dir.join("config.yaml")).is_some_and(|content| {
            content
                .lines()
                .any(|line| line.trim().trim_matches(['\'', '"']) == "- rtk-rewrite")
        })
}

fn pi_extension_registered(path: &Path) -> bool {
    read_file(path).is_some_and(|content| super::init::looks_like_rtk_pi_plugin(&content))
}

fn droid_hook_registered_in(droid_dir: &Path) -> bool {
    [
        droid_dir.join(DROID_HOOKS_FILE),
        droid_dir.join(DROID_HOOKS_SUBDIR).join(DROID_HOOKS_FILE),
        droid_dir.join(DROID_SETTINGS_FILE),
    ]
    .iter()
    .any(|path| droid_hook_registered(path))
}

fn droid_hook_registered(path: &Path) -> bool {
    read_json(path).is_some_and(|root| {
        [
            root.get(PRE_TOOL_USE_KEY),
            root.get("hooks")
                .and_then(|hooks| hooks.get(PRE_TOOL_USE_KEY)),
        ]
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_array)
        .flatten()
        .filter_map(|entry| entry.get("hooks").and_then(serde_json::Value::as_array))
        .flatten()
        .any(|hook| {
            hook.get("command").and_then(serde_json::Value::as_str) == Some(DROID_HOOK_COMMAND)
        })
    })
}

fn vibe_hook_registered(path: &Path) -> bool {
    read_file(path).is_some_and(|content| {
        content.contains(&format!(r#"name = "{VIBE_HOOK_NAME}""#))
            && content.contains(&format!(r#"command = "{VIBE_HOOK_COMMAND}""#))
    })
}

fn copilot_hook_registered(hooks_dir: &Path) -> bool {
    read_json(&hooks_dir.join(COPILOT_HOOK_FILE)).is_some_and(|root| {
        root.get("hooks")
            .and_then(|hooks| hooks.get(PRE_TOOL_USE_KEY))
            .and_then(serde_json::Value::as_array)
            .is_some_and(|entries| {
                entries.iter().any(|entry| {
                    entry.get("command").and_then(serde_json::Value::as_str)
                        == Some("rtk hook copilot")
                })
            })
    })
}

fn read_file(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    let content = read_file(path)?;
    (!content.trim().is_empty())
        .then(|| from_json_str(&content).ok())
        .flatten()
}

fn warn_marker_path() -> Option<PathBuf> {
    let data_dir = dirs::data_local_dir()?.join(RTK_DATA_DIR);
    Some(data_dir.join(".hook_warn_last"))
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
        assert_eq!(
            status_with_other_integration(
                HookStatus::Missing,
                other_integration_installed_at(tmp.path())
            ),
            HookStatus::Missing
        );
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
        std::fs::write(&path, "export const RtkOpenCodePlugin = () => rtk rewrite").unwrap();
        assert_eq!(
            status_with_other_integration(
                HookStatus::Missing,
                other_integration_installed_at(tmp.path())
            ),
            HookStatus::Ok
        );
    }

    #[test]
    fn test_other_integration_does_not_hide_outdated_claude_hook() {
        assert_eq!(
            status_with_other_integration(HookStatus::Outdated, true),
            HookStatus::Outdated
        );
    }

    #[test]
    fn test_warning_for_status_is_hermetic() {
        assert_eq!(warning_for_status(HookStatus::Ok), None);
        assert_eq!(
            warning_for_status(HookStatus::Missing),
            Some("[rtk] /!\\ No hook installed — run `rtk init -g` for automatic token savings")
        );
        assert_eq!(
            warning_for_status(HookStatus::Outdated),
            Some("[rtk] /!\\ Hook outdated — run `rtk init -g` to update")
        );
    }

    #[test]
    fn test_other_integration_rejects_unrecognized_artifacts() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = [
            tmp.path()
                .join(CONFIG_DIR)
                .join(OPENCODE_SUBDIR)
                .join(PLUGIN_SUBDIR)
                .join(OPENCODE_PLUGIN_FILE),
            tmp.path().join(CURSOR_DIR).join(HOOKS_JSON),
            tmp.path().join(CODEX_DIR).join("AGENTS.md"),
            tmp.path()
                .join(GEMINI_DIR)
                .join(HOOKS_SUBDIR)
                .join(GEMINI_HOOK_FILE),
            tmp.path()
                .join(HERMES_DIR)
                .join(HERMES_PLUGINS_SUBDIR)
                .join(HERMES_PLUGIN_NAME)
                .join(HERMES_PLUGIN_MANIFEST_FILE),
        ];
        for path in paths {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"not an RTK integration").unwrap();
        }

        assert!(!other_integration_installed_at(tmp.path()));
    }

    #[test]
    fn test_other_integration_cursor() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join(CURSOR_DIR).join(HOOKS_JSON);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!(r#"{{"hooks":{{"preToolUse":[{{"command":"{CURSOR_HOOK_COMMAND}"}}]}}}}"#),
        )
        .unwrap();
        assert!(other_integration_installed_at(tmp.path()));
    }

    #[test]
    fn test_other_integration_rejects_legacy_cursor_script_reference() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join(CURSOR_DIR).join(HOOKS_JSON);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"hooks":{"preToolUse":[{"command":"/tmp/rtk-rewrite.sh"}]}}"#,
        )
        .unwrap();

        assert!(!other_integration_installed_at(tmp.path()));
    }

    #[test]
    fn test_other_integration_codex() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join(CODEX_DIR).join("AGENTS.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "@RTK.md").unwrap();
        std::fs::write(
            tmp.path().join(CODEX_DIR).join("RTK.md"),
            "RTK instructions",
        )
        .unwrap();
        assert!(other_integration_installed_at(tmp.path()));
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
        std::fs::write(&path, "exec rtk hook gemini").unwrap();
        std::fs::write(
            tmp.path().join(GEMINI_DIR).join(SETTINGS_JSON),
            serde_json::json!({
                "hooks": { "BeforeTool": [{ "hooks": [{ "command": path }] }] }
            })
            .to_string(),
        )
        .unwrap();
        assert!(other_integration_installed_at(tmp.path()));
    }

    #[test]
    fn test_other_integration_hermes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let plugin_dir = tmp
            .path()
            .join(HERMES_DIR)
            .join(HERMES_PLUGINS_SUBDIR)
            .join(HERMES_PLUGIN_NAME);
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join(HERMES_PLUGIN_MANIFEST_FILE),
            "name: rtk-rewrite",
        )
        .unwrap();
        std::fs::write(plugin_dir.join(HERMES_PLUGIN_INIT_FILE), "rtk rewrite").unwrap();
        std::fs::write(
            tmp.path().join(HERMES_DIR).join("config.yaml"),
            "enabled:\n  - rtk-rewrite",
        )
        .unwrap();
        assert!(other_integration_installed_at(tmp.path()));
    }

    #[test]
    fn test_project_pi_and_omp_integrations() {
        let tmp = tempfile::tempdir().expect("tempdir");
        for agent_dir in [PI_LOCAL_DIR, OMP_LOCAL_DIR] {
            let path = tmp
                .path()
                .join(agent_dir)
                .join(PI_EXTENSIONS_SUBDIR)
                .join(PI_PLUGIN_FILE);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "pi.exec(\"rtk\", [\"rewrite\", command])").unwrap();
            assert!(project_integration_installed_at(tmp.path()));
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn test_droid_integrations_include_root_and_nested_config() {
        let tmp = tempfile::tempdir().expect("tempdir");
        for path in [
            tmp.path().join(DROID_HOOKS_FILE),
            tmp.path().join(DROID_SETTINGS_FILE),
        ] {
            let root = if path.file_name().and_then(|name| name.to_str())
                == Some(DROID_SETTINGS_FILE)
            {
                serde_json::json!({
                    "hooks": { PRE_TOOL_USE_KEY: [{ "hooks": [{ "command": DROID_HOOK_COMMAND }] }] }
                })
            } else {
                serde_json::json!({
                    PRE_TOOL_USE_KEY: [{ "hooks": [{ "command": DROID_HOOK_COMMAND }] }]
                })
            };
            std::fs::write(&path, root.to_string()).unwrap();
            assert!(droid_hook_registered_in(tmp.path()));
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn test_project_droid_and_copilot_integrations() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let droid = tmp.path().join(DROID_DIR).join(DROID_HOOKS_FILE);
        std::fs::create_dir_all(droid.parent().unwrap()).unwrap();
        std::fs::write(
            &droid,
            serde_json::json!({
                PRE_TOOL_USE_KEY: [{ "hooks": [{ "command": DROID_HOOK_COMMAND }] }]
            })
            .to_string(),
        )
        .unwrap();
        assert!(project_integration_installed_at(tmp.path()));
        std::fs::remove_file(droid).unwrap();

        let copilot = tmp
            .path()
            .join(GITHUB_DIR)
            .join(HOOKS_SUBDIR)
            .join(COPILOT_HOOK_FILE);
        std::fs::create_dir_all(copilot.parent().unwrap()).unwrap();
        std::fs::write(
            &copilot,
            serde_json::json!({
                "hooks": { PRE_TOOL_USE_KEY: [{ "command": "rtk hook copilot" }] }
            })
            .to_string(),
        )
        .unwrap();
        assert!(project_integration_installed_at(tmp.path()));
    }

    #[test]
    fn test_vibe_and_copilot_global_hook_artifacts() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let vibe = tmp.path().join(VIBE_HOOKS_FILE);
        std::fs::write(
            &vibe,
            format!(r#"name = "{VIBE_HOOK_NAME}"\ncommand = "{VIBE_HOOK_COMMAND}""#),
        )
        .unwrap();
        assert!(vibe_hook_registered(&vibe));

        let hooks_dir = tmp.path().join(HOOKS_SUBDIR);
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(
            hooks_dir.join(COPILOT_HOOK_FILE),
            serde_json::json!({
                "hooks": { PRE_TOOL_USE_KEY: [{ "command": "rtk hook copilot" }] }
            })
            .to_string(),
        )
        .unwrap();
        assert!(copilot_hook_registered(&hooks_dir));
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
        assert!(!other_integration_installed_at(tmp.path()));
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
