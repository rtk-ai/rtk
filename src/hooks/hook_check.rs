//! Detects whether RTK hooks are installed and warns if they are outdated.

use super::constants::{
    CLAUDE_DIR, CLAUDE_HOOK_COMMAND, CODEX_DIR, CODEX_HOOK_COMMAND, HOOKS_JSON, HOOKS_SUBDIR,
    PRE_TOOL_USE_KEY, REWRITE_HOOK_FILE, SETTINGS_JSON, SETTINGS_LOCAL_JSON,
};
use crate::core::user_dirs;
use crate::core::utils::from_json_str;
use std::path::{Path, PathBuf};

pub const CURRENT_HOOK_VERSION: u8 = 4;
const WARN_INTERVAL_SECS: u64 = 24 * 3600;

/// Hook status for diagnostics and `rtk gain`.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum HookStatus {
    /// Hook is installed and up to date.
    Ok,
    /// Hook exists but is outdated or unreadable.
    Outdated,
    /// No hook file found (but Claude Code is installed).
    Missing,
}

fn status_for_home(home: &Path) -> HookStatus {
    let claude_dir = home.join(CLAUDE_DIR);
    status_for_claude_dir(&claude_dir)
}

pub(super) fn status_for_claude_dir(claude_dir: &Path) -> HookStatus {
    if !claude_dir.exists() {
        return HookStatus::Ok;
    }

    // Check for new binary command in Claude settings first. A Codex hook
    // must not hide missing Claude hook coverage when Claude is installed.
    if binary_hook_registered(claude_dir) {
        // If old script file still exists alongside new command, report Outdated
        // (migration not complete — user should run `rtk init -g` to clean up)
        let old_hook = claude_dir.join(HOOKS_SUBDIR).join(REWRITE_HOOK_FILE);
        if old_hook.exists() {
            return HookStatus::Outdated;
        }
        return HookStatus::Ok;
    }
    if binary_hook_partially_registered(claude_dir) {
        return HookStatus::Outdated;
    }

    // Fall back to legacy script file check
    let hook_path = claude_dir.join(HOOKS_SUBDIR).join(REWRITE_HOOK_FILE);
    if !hook_path.exists() {
        return HookStatus::Missing;
    }
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
            hook_command_present_in_json(&claude_dir.join(file_name), CLAUDE_HOOK_COMMAND)
        })
}

#[allow(dead_code)]
fn codex_hook_registered(home: &Path) -> bool {
    hook_command_registered_in_json(&home.join(CODEX_DIR).join(HOOKS_JSON), CODEX_HOOK_COMMAND)
}

fn split_hook_command(command: &str) -> Option<(&str, &str)> {
    let command = command.trim();
    if command.is_empty() {
        return None;
    }

    let bytes = command.as_bytes();
    if matches!(bytes.first(), Some(b'"' | b'\'')) {
        let quote = bytes[0];
        let end = bytes
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(idx, byte)| (*byte == quote).then_some(idx))?;
        let program = &command[1..end];
        let rest = command[end + 1..].trim();
        return Some((program, rest));
    }

    let split_at = command.find(char::is_whitespace).unwrap_or(command.len());
    let program = &command[..split_at];
    let rest = command[split_at..].trim();
    Some((program, rest))
}

fn hook_program_is_rtk(program: &str) -> bool {
    let file_name = program
        .trim_matches(|c| c == '"' || c == '\'')
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();

    matches!(file_name.as_str(), "rtk" | "rtk.exe")
}

fn hook_command_equivalent(actual: &str, expected: &str) -> bool {
    if actual == expected {
        return true;
    }

    let Some((expected_program, expected_args)) = split_hook_command(expected) else {
        return false;
    };
    if !hook_program_is_rtk(expected_program) {
        return false;
    }

    let Some((actual_program, actual_args)) = split_hook_command(actual) else {
        return false;
    };
    hook_program_is_rtk(actual_program) && actual_args.eq_ignore_ascii_case(expected_args)
}

fn hook_command_registered_in_json(path: &Path, command: &str) -> bool {
    let content = match std::fs::read_to_string(path) {
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
                    .any(|cmd| hook_command_equivalent(cmd, command))
        })
    })
}

fn hook_command_present_in_json(path: &Path, command: &str) -> bool {
    let content = match std::fs::read_to_string(path) {
        Ok(content) if !content.trim().is_empty() => content,
        _ => return false,
    };
    let root: serde_json::Value = match serde_json::from_str(&content) {
        Ok(root) => root,
        Err(_) => return false,
    };
    root.get("hooks")
        .and_then(|hooks| hooks.get(PRE_TOOL_USE_KEY))
        .and_then(|pre_tool_use| pre_tool_use.as_array())
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("hooks")?.as_array())
        .flatten()
        .filter_map(|hook| hook.get("command")?.as_str())
        .any(|actual| hook_command_equivalent(actual, command))
}

/// All non-Claude agent probes for active RTK hook/plugin installation.
/// Every agent owns its own probe inside `src/hooks/init/<agent>.rs`.
const AGENT_PROBES: &[fn() -> bool] = &[
    super::init::antigravity::is_configured,
    super::init::cursor::is_configured,
    super::init::gemini::is_configured,
    super::init::opencode::is_configured,
    super::init::codex::is_configured,
    super::init::droid::is_configured,
    super::init::trae::is_configured,
    super::init::hermes::is_configured,
    super::init::pi::is_configured,
    super::init::vibe::is_configured,
];

/// Returns true if at least one supported non-Claude agent has an active hook/plugin configured.
pub fn is_any_agent_configured() -> bool {
    AGENT_PROBES.iter().any(|probe| probe())
}

/// Returns the hook status for Claude Code without printing anything.
/// Returns `HookStatus::Ok` if Claude Code is not installed on this system.
///
/// Note: This probe is intentionally specific to Claude Code because `rtk discover`
/// and legacy transcript analysis rely specifically on Claude's hook status.
/// General warning checks also consult [`is_any_agent_configured`].
pub fn status() -> HookStatus {
    super::init::claude::hook_status()
}

/// Check if the installed hook is missing or outdated, warn once per day.
pub fn maybe_warn() {
    // Don't block startup — fail silently on any error
    let _ = check_and_warn();
}

/// Message to print for `status`, if any.
/// `suppress_missing` only hides [`HookStatus::Missing`]; outdated stays visible.
fn warning_text(status: HookStatus, suppress_missing: bool) -> Option<&'static str> {
    match status {
        HookStatus::Ok => None,
        HookStatus::Missing if suppress_missing => None,
        HookStatus::Missing => {
            Some("[rtk] /!\\ No hook installed — run `rtk init -g` for automatic token savings")
        }
        HookStatus::Outdated => Some("[rtk] /!\\ Hook outdated — run `rtk init -g` to update"),
    }
}

/// Single source of truth: delegates to `status()` then rate-limits the warning.
fn check_and_warn() -> Option<()> {
    // Probe first so the common HookStatus::Ok path never reads config.toml.
    // Suppression is consulted only when a missing-hook warning would print.
    let status = status();
    if status == HookStatus::Ok {
        return Some(());
    }
    let suppress_missing = status == HookStatus::Missing
        && (crate::core::config::hook_warning_suppressed() || is_any_agent_configured());
    let warning = warning_text(status, suppress_missing)?;

    // Rate limit: warn once per day
    let marker = warn_marker_path()?;
    if warn_marker_is_fresh(&marker) {
        return Some(());
    }

    eprintln!("{}", warning);

    // Write non-empty content so Windows reliably refreshes an existing marker's mtime.
    let _ = refresh_warn_marker(&marker);

    Some(())
}

pub fn parse_hook_version(content: &str) -> u8 {
    // Version tag must be in the first 5 lines (shebang + header convention)
    for line in content.lines().take(5) {
        if let Some(rest) = line.strip_prefix("# rtk-hook-version:")
            && let Ok(v) = rest.trim().parse::<u8>()
        {
            return v;
        }
    }
    0 // No version tag = version 0 (outdated)
}

fn warn_marker_path() -> Option<PathBuf> {
    let data_dir = user_dirs::data()?;
    Some(data_dir.join(".hook_warn_last"))
}

fn warn_marker_is_fresh(marker: &std::path::Path) -> bool {
    std::fs::metadata(marker)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed.as_secs() < WARN_INTERVAL_SECS)
}

fn refresh_warn_marker(marker: &std::path::Path) -> std::io::Result<()> {
    if let Some(parent) = marker.parent() {
        crate::core::utils::create_private_dir(parent)?;
    }
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();
    std::fs::write(marker, timestamp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::test_isolation;
    use crate::core::user_env;
    use crate::hooks::constants::{
        CODEX_DIR, CONFIG_DIR, CURSOR_DIR, GEMINI_DIR, GEMINI_HOOK_FILE, HERMES_DIR,
        HERMES_PLUGIN_MANIFEST_FILE, HERMES_PLUGIN_NAME, HERMES_PLUGINS_SUBDIR,
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

    /// The shipped Claude hook script must carry the current version. `rtk init`
    /// no longer installs it, so the version grades copies already deployed:
    /// raising it reports older copies as outdated, which sends their owners to
    /// `rtk init -g` and from there to the in-process hook. The constant and the
    /// script move together.
    #[test]
    fn test_shipped_claude_hook_carries_the_current_version() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let content = std::fs::read_to_string(root.join("hooks/claude/rtk-rewrite.sh"))
            .expect("read hooks/claude/rtk-rewrite.sh");
        assert_eq!(
            parse_hook_version(&content),
            CURRENT_HOOK_VERSION,
            "hooks/claude/rtk-rewrite.sh and CURRENT_HOOK_VERSION disagree"
        );
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
        let content = complete_hook_json("/opt/homebrew/bin/rtk hook claude");
        std::fs::write(tmp.path().join(SETTINGS_JSON), &content).expect("write settings");
        std::fs::write(tmp.path().join(SETTINGS_LOCAL_JSON), &content)
            .expect("write local settings");

        assert!(binary_hook_registered(tmp.path()));
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
        serde_json::json!({
            "hooks": {
                PRE_TOOL_USE_KEY: [
                    {"matcher": "Bash", "hooks": [{"type": "command", "command": command}]},
                    {"matcher": "Shell", "hooks": [{"type": "command", "command": command}]},
                    {"matcher": "PowerShell", "hooks": [{"type": "command", "command": command}]}
                ]
            }
        })
        .to_string()
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
    fn test_binary_hook_registered_accepts_absolute_rtk_exe() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let claude_dir = tmp.path().join(CLAUDE_DIR);
        std::fs::create_dir_all(&claude_dir).unwrap();
        let absolute = r#"C:\Users\Administrator\.local\bin\rtk.exe hook claude"#;
        std::fs::write(claude_dir.join(SETTINGS_JSON), complete_hook_json(absolute)).unwrap();
        std::fs::write(
            claude_dir.join(SETTINGS_LOCAL_JSON),
            complete_hook_json(absolute),
        )
        .unwrap();

        assert!(binary_hook_registered(&claude_dir));
    }

    #[test]
    fn test_codex_hook_registered_accepts_absolute_rtk_exe() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let codex_dir = tmp.path().join(CODEX_DIR);
        std::fs::create_dir_all(&codex_dir).unwrap();
        std::fs::write(
            codex_dir.join(HOOKS_JSON),
            complete_hook_json(r#"C:\Users\Administrator\.local\bin\rtk.exe hook codex"#),
        )
        .unwrap();

        assert!(codex_hook_registered(tmp.path()));
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
        assert!(binary_hook_partially_registered(&claude_dir));
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
    fn test_status_for_claude_dir_accepts_resolved_override() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let claude_dir = tmp.path().join("custom-claude-config");
        std::fs::create_dir_all(&claude_dir).unwrap();
        let content = complete_hook_json(CLAUDE_HOOK_COMMAND);
        std::fs::write(claude_dir.join(SETTINGS_JSON), &content).unwrap();
        std::fs::write(claude_dir.join(SETTINGS_LOCAL_JSON), &content).unwrap();

        assert_eq!(status_for_claude_dir(&claude_dir), HookStatus::Ok);
    }

    #[test]
    fn test_refresh_warn_marker_rewrites_existing_empty_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let marker = tmp.path().join(".hook_warn_last");
        std::fs::write(&marker, b"").expect("seed empty marker");

        refresh_warn_marker(&marker).expect("refresh marker");

        let contents = std::fs::read_to_string(&marker).expect("read marker");
        assert!(
            !contents.trim().is_empty(),
            "refresh must change an existing zero-byte file on Windows"
        );
        assert!(
            warn_marker_is_fresh(&marker),
            "a refreshed marker must suppress the next warning"
        );
    }

    #[test]
    fn test_warning_text_scopes_suppression_to_missing() {
        assert_eq!(warning_text(HookStatus::Ok, false), None);
        assert_eq!(warning_text(HookStatus::Ok, true), None);
        assert!(
            warning_text(HookStatus::Missing, false).is_some(),
            "missing hook must warn when the flag is off"
        );
        assert_eq!(
            warning_text(HookStatus::Missing, true),
            None,
            "suppress_hook_warning must hide HookStatus::Missing"
        );
        assert!(
            warning_text(HookStatus::Outdated, false).is_some(),
            "outdated hook must warn when the flag is off"
        );
        assert!(
            warning_text(HookStatus::Outdated, true).is_some(),
            "suppress_hook_warning must not hide the outdated-hook upgrade prompt"
        );
    }

    #[test]
    fn test_status_returns_valid_variant() {
        // `status()` resolves through `CLAUDE_CONFIG_DIR`; pinned so both
        // states can be asserted.
        let tmp = test_isolation::tempdir();
        let claude_dir = tmp.path().join(".claude");
        user_env::with_path("CLAUDE_CONFIG_DIR", Some(&claude_dir), || {
            assert_eq!(
                status(),
                HookStatus::Ok,
                "no Claude dir: nothing to warn about"
            );
            std::fs::create_dir_all(&claude_dir).expect("create Claude dir");
            assert_eq!(
                status(),
                HookStatus::Missing,
                "a Claude dir with no rtk hook"
            );
        });
    }

    #[test]
    fn test_probes_ignore_foreign_hooks_in_agent_files() {
        let tmp = test_isolation::tempdir();
        let home = tmp.path().join("home");
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(home.join(".gemini")).expect("create .gemini");
        std::fs::create_dir_all(home.join(".vibe")).expect("create .vibe");
        std::fs::create_dir_all(&ws).expect("create ws");

        test_isolation::with_root(&home, || {
            let _entered = test_isolation::enter(&ws);
            std::fs::write(
                home.join(".gemini/settings.json"),
                r#"{"hooks":{"BeforeTool":[{"matcher":"run_shell_command","hooks":[{"type":"command","command":"/usr/local/bin/audit-shell"}]}]}}"#,
            )
            .expect("write gemini settings");
            std::fs::write(
                home.join(".vibe/hooks.toml"),
                "[[hooks]]\nname = \"audit\"\ntype = \"pre_tool\"\nmatch = \"bash\"\ncommand = \"rtk-audit\"\n",
            )
            .expect("write vibe hooks");

            assert!(
                !super::super::init::gemini::is_configured(),
                "a non-rtk BeforeTool hook is not an rtk install"
            );
            assert!(
                !super::super::init::vibe::is_configured(),
                "a Vibe hook not named rtk-rewrite is not an rtk install"
            );
            assert!(!is_any_agent_configured(), "no agent is configured");
        });
    }

    #[test]
    fn test_check_and_warn_suppressed_when_agent_configured() {
        let tmp = test_isolation::tempdir();
        let home = tmp.path().join("home");
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(home.join(".claude")).expect("create .claude");
        std::fs::create_dir_all(&ws).expect("create ws");

        test_isolation::with_root(&home, || {
            let _entered = test_isolation::enter(&ws);
            let marker = warn_marker_path().expect("marker path");

            // With OpenCode plugin present, check_and_warn must not warn or touch marker
            let opencode_plugin = home.join(".config/opencode/plugins/rtk.ts");
            std::fs::create_dir_all(opencode_plugin.parent().unwrap()).unwrap();
            std::fs::write(&opencode_plugin, b"export default {}").unwrap();

            check_and_warn();
            assert!(
                !marker.exists(),
                "check_and_warn must not warn when an agent is configured"
            );

            // Once removed, check_and_warn warns and touches the daily marker
            std::fs::remove_file(&opencode_plugin).unwrap();
            check_and_warn();
            assert!(
                marker.exists(),
                "check_and_warn must warn and write marker when no agent is configured"
            );
        });
    }

    #[test]
    fn test_all_agent_probes_and_arms_in_process() {
        use crate::hooks::init::{InitContext, PatchMode};

        // Helper that runs `f(home, ws)` inside an isolated home + workspace
        let run_isolated = |f: &dyn Fn(&std::path::Path, &std::path::Path)| {
            let tmp = test_isolation::tempdir();
            let home = tmp.path().join("home");
            let ws = tmp.path().join("ws");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::create_dir_all(&ws).unwrap();
            test_isolation::with_root(&home, || {
                let _entered = test_isolation::enter(&ws);
                assert!(
                    !is_any_agent_configured(),
                    "precondition: fresh isolated env has no configured agent"
                );
                f(&home, &ws);
            });
        };

        // 1. Antigravity (local & global)
        run_isolated(&|_home, _ws| {
            super::super::init::antigravity::run_antigravity_mode(false, InitContext::default())
                .unwrap();
            assert!(is_any_agent_configured(), "antigravity local");
            super::super::init::antigravity::uninstall_antigravity_mode(
                false,
                InitContext::default(),
            )
            .unwrap();
            assert!(!is_any_agent_configured(), "antigravity local uninstalled");

            super::super::init::antigravity::run_antigravity_mode(true, InitContext::default())
                .unwrap();
            assert!(is_any_agent_configured(), "antigravity global");
            super::super::init::antigravity::uninstall_antigravity_mode(
                true,
                InitContext::default(),
            )
            .unwrap();
            assert!(!is_any_agent_configured(), "antigravity global uninstalled");
        });

        // 2. Cursor (configured vs unregistered script)
        run_isolated(&|home, _ws| {
            let cursor_hooks = home.join(".cursor/hooks");
            std::fs::create_dir_all(&cursor_hooks).unwrap();
            std::fs::write(cursor_hooks.join("rtk-rewrite.sh"), "#!/bin/sh\n").unwrap();
            assert!(
                !is_any_agent_configured(),
                "unregistered cursor script must not count as configured"
            );
            std::fs::remove_file(cursor_hooks.join("rtk-rewrite.sh")).unwrap();

            std::fs::write(
                home.join(".cursor/hooks.json"),
                r#"{"version":1,"hooks":{"preToolUse":[{"matcher":"Shell","command":"rtk hook cursor"}]}}"#,
            )
            .unwrap();
            assert!(is_any_agent_configured(), "cursor hooks.json");
            std::fs::remove_file(home.join(".cursor/hooks.json")).unwrap();
            assert!(!is_any_agent_configured(), "cursor removed");
        });

        // 3. Gemini (run_shell_command, wildcard *, non-shell matcher, unregistered script)
        run_isolated(&|home, _ws| {
            super::super::init::gemini::run_gemini(
                true,
                false,
                PatchMode::Auto,
                InitContext::default(),
            )
            .unwrap();
            assert!(is_any_agent_configured(), "gemini init");

            // Wildcard matcher
            std::fs::write(
                home.join(".gemini/settings.json"),
                r#"{"hooks":{"BeforeTool":[{"matcher":"*","hooks":[{"command":"rtk hook gemini"}]}]}}"#,
            )
            .unwrap();
            assert!(is_any_agent_configured(), "gemini wildcard matcher");

            // Non-shell matcher must be rejected
            std::fs::write(
                home.join(".gemini/settings.json"),
                r#"{"hooks":{"BeforeTool":[{"matcher":"ReadFile","hooks":[{"command":"rtk hook gemini"}]}]}}"#,
            )
            .unwrap();
            assert!(!is_any_agent_configured(), "gemini non-shell matcher");

            // Unregistered script file alone must be rejected
            std::fs::remove_file(home.join(".gemini/settings.json")).unwrap();
            let gemini_hooks = home.join(".gemini/hooks");
            std::fs::create_dir_all(&gemini_hooks).unwrap();
            std::fs::write(gemini_hooks.join("rtk-hook-gemini.sh"), "#!/bin/sh\n").unwrap();
            assert!(!is_any_agent_configured(), "gemini unregistered script");
        });

        // 4. Codex
        run_isolated(&|home, _ws| {
            std::fs::create_dir_all(home.join(".codex")).unwrap();
            std::fs::write(
                home.join(".codex/hooks.json"),
                r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"rtk hook codex"}]}]}}"#,
            )
            .unwrap();
            assert!(is_any_agent_configured(), "codex configured");
            std::fs::remove_file(home.join(".codex/hooks.json")).unwrap();
            assert!(!is_any_agent_configured(), "codex removed");
        });

        // 5. Droid
        run_isolated(&|_home, _ws| {
            super::super::init::droid::run_droid_mode(true, InitContext::default()).unwrap();
            assert!(is_any_agent_configured(), "droid global");
            super::super::init::droid::uninstall_droid(true, InitContext::default()).unwrap();
            assert!(!is_any_agent_configured(), "droid uninstalled");
        });

        // 6. Trae (local & global)
        run_isolated(&|_home, _ws| {
            super::super::init::trae::run_trae_mode(false, InitContext::default()).unwrap();
            assert!(is_any_agent_configured(), "trae local");
            super::super::init::trae::uninstall_trae_mode(false, InitContext::default()).unwrap();
            assert!(!is_any_agent_configured(), "trae local uninstalled");

            super::super::init::trae::run_trae_mode(true, InitContext::default()).unwrap();
            assert!(is_any_agent_configured(), "trae global");
            super::super::init::trae::uninstall_trae_mode(true, InitContext::default()).unwrap();
            assert!(!is_any_agent_configured(), "trae global uninstalled");
        });

        // 7. Hermes (full init/uninstall + individual arms)
        run_isolated(&|home, _ws| {
            super::super::init::hermes::run_hermes_mode(InitContext::default()).unwrap();
            assert!(is_any_agent_configured(), "hermes init");
            super::super::init::hermes::uninstall_hermes(InitContext::default()).unwrap();
            assert!(!is_any_agent_configured(), "hermes uninstalled");

            // Plugin file arm alone
            let plugin_init = home.join(".hermes/plugins/rtk-rewrite/__init__.py");
            std::fs::create_dir_all(plugin_init.parent().unwrap()).unwrap();
            std::fs::write(&plugin_init, "# plugin\n").unwrap();
            assert!(is_any_agent_configured(), "hermes plugin file arm");
            std::fs::remove_file(&plugin_init).unwrap();
            assert!(!is_any_agent_configured(), "hermes plugin file removed");

            // config.yaml arm alone
            std::fs::write(
                home.join(".hermes/config.yaml"),
                "plugins:\n  enabled:\n    - rtk-rewrite\n",
            )
            .unwrap();
            assert!(is_any_agent_configured(), "hermes config.yaml arm");
            std::fs::write(
                home.join(".hermes/config.yaml"),
                "plugins:\n  enabled:\n    - other\n",
            )
            .unwrap();
            assert!(!is_any_agent_configured(), "hermes unrelated config.yaml");
        });

        // 8. Pi & OMP (local & global)
        run_isolated(&|_home, _ws| {
            super::super::init::pi::run_pi_mode_with_patch_mode(
                false,
                PatchMode::Auto,
                InitContext::default(),
            )
            .unwrap();
            assert!(is_any_agent_configured(), "pi local");
            std::fs::remove_file(user_dirs::in_working_dir(".pi/extensions/rtk.ts")).unwrap();
            assert!(!is_any_agent_configured(), "pi local removed");

            super::super::init::pi::run_pi_mode_with_patch_mode(
                true,
                PatchMode::Auto,
                InitContext::default(),
            )
            .unwrap();
            assert!(is_any_agent_configured(), "pi global");
            std::fs::remove_file(
                user_dirs::home()
                    .unwrap()
                    .join(".pi/agent/extensions/rtk.ts"),
            )
            .unwrap();
            assert!(!is_any_agent_configured(), "pi global removed");

            super::super::init::pi::run_omp_mode_with_patch_mode(
                false,
                PatchMode::Auto,
                InitContext::default(),
            )
            .unwrap();
            assert!(is_any_agent_configured(), "omp local");
            std::fs::remove_file(user_dirs::in_working_dir(".omp/extensions/rtk.ts")).unwrap();
            assert!(!is_any_agent_configured(), "omp local removed");

            super::super::init::pi::run_omp_mode_with_patch_mode(
                true,
                PatchMode::Auto,
                InitContext::default(),
            )
            .unwrap();
            assert!(is_any_agent_configured(), "omp global");
            std::fs::remove_file(
                user_dirs::home()
                    .unwrap()
                    .join(".omp/agent/extensions/rtk.ts"),
            )
            .unwrap();
            assert!(!is_any_agent_configured(), "omp global removed");
        });

        // 9. Vibe
        run_isolated(&|_home, _ws| {
            super::super::init::vibe::run_vibe_mode(
                true,
                false,
                PatchMode::Auto,
                InitContext::default(),
            )
            .unwrap();
            assert!(is_any_agent_configured(), "vibe global");
            super::super::init::vibe::uninstall_vibe(InitContext::default()).unwrap();
            assert!(!is_any_agent_configured(), "vibe uninstalled");
        });
    }
}
