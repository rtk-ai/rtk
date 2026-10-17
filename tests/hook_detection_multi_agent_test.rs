//! Integration tests for multi-agent hook detection and warning suppression.
//!
//! Unix only: the check resolves agent directories through the home
//! directory, and only Unix takes that from `$HOME`.
#![cfg(unix)]

use std::fs;
use std::path::Path;
use tempfile::TempDir;

mod common;

const NO_HOOK_WARNING: &str = "No hook installed";
const OUTDATED_WARNING: &str = "Hook outdated";

fn seed_tracking(home: &Path) {
    let _ = common::rtk_command()
        .args(["ls"])
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("RTK_DB_PATH", home.join("rtk.db"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("RTK_TELEMETRY_DISABLED", "1")
        .output();
}

fn run_rtk_gain(cwd: &Path, home: &Path) -> (bool, String, String) {
    let output = common::rtk_command()
        .args(["gain"])
        .current_dir(cwd)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("RTK_DB_PATH", home.join("rtk.db"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("RTK_TELEMETRY_DISABLED", "1")
        .output()
        .expect("run rtk gain");

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    (output.status.success(), stdout, stderr)
}

fn fresh_env() -> (TempDir, TempDir) {
    let ws = TempDir::new().expect("workspace tempdir");
    let home = TempDir::new().expect("home tempdir");
    // Create ~/.claude with no hook to simulate the condition where status() would return Missing
    fs::create_dir_all(home.path().join(".claude")).expect("create .claude");
    seed_tracking(home.path());
    (ws, home)
}

#[test]
fn test_empty_claude_without_agent_warns() {
    let (ws, home) = fresh_env();
    let (_, _, stderr) = run_rtk_gain(ws.path(), home.path());
    assert!(
        stderr.contains(NO_HOOK_WARNING),
        "Expected missing hook warning on unconfigured setup. Got:\n{stderr}"
    );
}

#[test]
fn test_antigravity_workspace_plugin_silences_warning() {
    let (ws, home) = fresh_env();
    let plugin_dir = ws.path().join(".agents/plugins/rtk");
    fs::create_dir_all(&plugin_dir).expect("create plugin dir");
    fs::write(
        plugin_dir.join("plugin.json"),
        r#"{"name":"rtk","version":"1.0.0"}"#,
    )
    .expect("write plugin.json");

    let (_, _, stderr) = run_rtk_gain(ws.path(), home.path());
    assert!(
        !stderr.contains(NO_HOOK_WARNING),
        "Antigravity workspace plugin should silence warning. Got:\n{stderr}"
    );
}

#[test]
fn test_antigravity_global_plugin_silences_warning() {
    let (ws, home) = fresh_env();
    let plugin_dir = home.path().join(".gemini/config/plugins/rtk");
    fs::create_dir_all(&plugin_dir).expect("create global plugin dir");
    fs::write(
        plugin_dir.join("plugin.json"),
        r#"{"name":"rtk","version":"1.0.0"}"#,
    )
    .expect("write plugin.json");

    let (_, _, stderr) = run_rtk_gain(ws.path(), home.path());
    assert!(
        !stderr.contains(NO_HOOK_WARNING),
        "Antigravity global plugin should silence warning. Got:\n{stderr}"
    );
}

#[test]
fn test_cursor_hook_silences_warning() {
    let (ws, home) = fresh_env();
    let cursor_dir = home.path().join(".cursor");
    fs::create_dir_all(&cursor_dir).expect("create .cursor dir");
    fs::write(
        cursor_dir.join("hooks.json"),
        r#"{
            "version": 1,
            "hooks": {
                "preToolUse": [
                    { "matcher": "Shell", "command": "rtk hook cursor" }
                ]
            }
        }"#,
    )
    .expect("write cursor hooks.json");

    let (_, _, stderr) = run_rtk_gain(ws.path(), home.path());
    assert!(
        !stderr.contains(NO_HOOK_WARNING),
        "Cursor hook should silence warning. Got:\n{stderr}"
    );
}

#[test]
fn test_opencode_plugin_silences_warning() {
    let (ws, home) = fresh_env();
    let plugin_dir = home.path().join(".config/opencode/plugins");
    fs::create_dir_all(&plugin_dir).expect("create opencode plugins dir");
    fs::write(plugin_dir.join("rtk.ts"), b"export default {}").expect("write rtk.ts");

    let (_, _, stderr) = run_rtk_gain(ws.path(), home.path());
    assert!(
        !stderr.contains(NO_HOOK_WARNING),
        "OpenCode plugin should silence warning. Got:\n{stderr}"
    );
}

#[test]
fn test_gemini_hook_silences_warning() {
    let (ws, home) = fresh_env();
    let gemini_dir = home.path().join(".gemini");
    fs::create_dir_all(&gemini_dir).expect("create .gemini dir");
    fs::write(
        gemini_dir.join("settings.json"),
        r#"{
            "hooks": {
                "BeforeTool": [
                    {
                        "matcher": "run_shell_command",
                        "hooks": [{ "command": "rtk hook gemini" }]
                    }
                ]
            }
        }"#,
    )
    .expect("write gemini settings.json");

    let (_, _, stderr) = run_rtk_gain(ws.path(), home.path());
    assert!(
        !stderr.contains(NO_HOOK_WARNING),
        "Gemini hook should silence warning. Got:\n{stderr}"
    );
}

#[test]
fn test_outdated_claude_hook_still_warns_even_with_other_agent() {
    let (ws, home) = fresh_env();
    // Configure Antigravity in workspace
    let plugin_dir = ws.path().join(".agents/plugins/rtk");
    fs::create_dir_all(&plugin_dir).expect("create plugin dir");
    fs::write(
        plugin_dir.join("plugin.json"),
        r#"{"name":"rtk","version":"1.0.0"}"#,
    )
    .expect("write plugin.json");

    // Configure outdated Claude script hook (version 2 < 4)
    let hooks_dir = home.path().join(".claude/hooks");
    fs::create_dir_all(&hooks_dir).expect("create .claude/hooks");
    fs::write(
        hooks_dir.join("rtk-rewrite.sh"),
        "#!/usr/bin/env bash\n# rtk-hook-version: 2\n",
    )
    .expect("write old hook");

    let (_, _, stderr) = run_rtk_gain(ws.path(), home.path());
    assert!(
        stderr.contains(OUTDATED_WARNING),
        "Outdated hook must warn even when another agent is installed. Got:\n{stderr}"
    );
}
