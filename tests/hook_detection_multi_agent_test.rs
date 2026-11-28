//! Integration tests for multi-agent hook detection and warning suppression.
//!
//! Unix only: the check resolves agent directories through the home
//! directory, and only Unix takes that from `$HOME`.
#![cfg(unix)]

use std::fs;
use std::path::Path;
use std::process::Stdio;
use tempfile::TempDir;

mod common;

const NO_HOOK_WARNING: &str = "No hook installed";
const OUTDATED_WARNING: &str = "Hook outdated";

fn clear_warn_marker(home: &Path) {
    let _ = fs::remove_file(home.join(".local/share/rtk/.hook_warn_last"));
    let _ = fs::remove_file(home.join("Library/Application Support/rtk/.hook_warn_last"));
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

fn run_rtk_ls(cwd: &Path, home: &Path) -> (bool, String, String) {
    clear_warn_marker(home);
    let output = common::rtk_command()
        .args(["ls", "."])
        .current_dir(cwd)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("RTK_DB_PATH", home.join("rtk.db"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("RTK_TELEMETRY_DISABLED", "1")
        .output()
        .expect("run rtk ls .");

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    (output.status.success(), stdout, stderr)
}

fn run_rtk_init(cwd: &Path, home: &Path, args: &[&str]) {
    let output = common::rtk_command()
        .arg("init")
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("RTK_DB_PATH", home.join("rtk.db"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("RTK_TELEMETRY_DISABLED", "1")
        .stdin(Stdio::null())
        .output()
        .expect("run rtk init");

    assert!(
        output.status.success(),
        "rtk init {args:?} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fresh_env() -> (TempDir, TempDir) {
    let ws = TempDir::new().expect("workspace tempdir");
    let home = TempDir::new().expect("home tempdir");
    // Create ~/.claude with no hook to simulate the condition where status() returns Missing
    fs::create_dir_all(home.path().join(".claude")).expect("create .claude");
    (ws, home)
}

fn assert_silent_in_gain_and_daily_check(ws: &Path, home: &Path, label: &str) {
    let (_, _, gain_stderr) = run_rtk_gain(ws, home);
    assert!(
        !gain_stderr.contains(NO_HOOK_WARNING),
        "{label}: expected rtk gain to be silent, got:\n{gain_stderr}"
    );
    let (_, _, ls_stderr) = run_rtk_ls(ws, home);
    assert!(
        !ls_stderr.contains(NO_HOOK_WARNING),
        "{label}: expected rtk ls . daily check to be silent, got:\n{ls_stderr}"
    );
}

fn assert_warns_in_gain_and_daily_check(ws: &Path, home: &Path, label: &str) {
    let (_, _, gain_stderr) = run_rtk_gain(ws, home);
    assert!(
        gain_stderr.contains(NO_HOOK_WARNING),
        "{label}: expected rtk gain to warn '{NO_HOOK_WARNING}', got:\n{gain_stderr}"
    );
    let (_, _, ls_stderr) = run_rtk_ls(ws, home);
    assert!(
        ls_stderr.contains(NO_HOOK_WARNING),
        "{label}: expected rtk ls . daily check to warn '{NO_HOOK_WARNING}', got:\n{ls_stderr}"
    );
}

#[test]
fn test_empty_claude_without_agent_warns_in_gain_and_daily_check() {
    let (ws, home) = fresh_env();
    assert_warns_in_gain_and_daily_check(ws.path(), home.path(), "unconfigured setup");
}

#[test]
fn test_daily_check_opencode_plugin_silences_and_removal_warns() {
    let (ws, home) = fresh_env();

    // Without plugin: daily check (rtk ls .) must warn
    let (_, _, ls_stderr_before) = run_rtk_ls(ws.path(), home.path());
    assert!(
        ls_stderr_before.contains(NO_HOOK_WARNING),
        "Expected daily check to warn without OpenCode plugin. Got:\n{ls_stderr_before}"
    );

    // Install the OpenCode plugin where `rtk init -g --opencode` writes it
    let plugin_dir = home.path().join(".config/opencode/plugins");
    fs::create_dir_all(&plugin_dir).expect("create opencode plugins dir");
    let plugin_file = plugin_dir.join("rtk.ts");
    fs::write(&plugin_file, b"export default {}").expect("write rtk.ts");

    assert_silent_in_gain_and_daily_check(ws.path(), home.path(), "OpenCode plugin installed");

    fs::remove_file(&plugin_file).expect("remove rtk.ts");
    assert_warns_in_gain_and_daily_check(ws.path(), home.path(), "OpenCode plugin removed");
}

#[test]
fn test_init_and_uninstall_cycle_for_all_global_agents() {
    let cases: &[(&str, &[&str], &[&str])] = &[
        (
            "cursor-global",
            &["-g", "--agent", "cursor"],
            &["-g", "--agent", "cursor", "--uninstall"],
        ),
        (
            "gemini-global",
            &["-g", "--gemini", "--auto-patch"],
            &["-g", "--gemini", "--uninstall"],
        ),
        (
            "codex-global",
            &["-g", "--codex"],
            &["-g", "--codex", "--uninstall"],
        ),
        (
            "droid-global",
            &["-g", "--agent", "droid"],
            &["-g", "--agent", "droid", "--uninstall"],
        ),
        (
            "trae-global",
            &["-g", "--agent", "trae"],
            &["-g", "--agent", "trae", "--uninstall"],
        ),
        (
            "hermes-global",
            &["-g", "--agent", "hermes"],
            &["-g", "--agent", "hermes", "--uninstall"],
        ),
        (
            "pi-global",
            &["-g", "--agent", "pi", "--auto-patch"],
            &["-g", "--agent", "pi", "--uninstall"],
        ),
        (
            "omp-global",
            &["-g", "--agent", "omp", "--auto-patch"],
            &["-g", "--agent", "omp", "--uninstall"],
        ),
        (
            "vibe-global",
            &["-g", "--agent", "vibe", "--auto-patch"],
            &["-g", "--agent", "vibe", "--uninstall"],
        ),
        (
            "antigravity-global",
            &["-g", "--agent", "antigravity"],
            &["-g", "--agent", "antigravity", "--uninstall"],
        ),
    ];

    for (label, install_args, uninstall_args) in cases {
        let (ws, home) = fresh_env();
        run_rtk_init(ws.path(), home.path(), install_args);
        assert_silent_in_gain_and_daily_check(
            ws.path(),
            home.path(),
            &format!("{label} after install"),
        );

        run_rtk_init(ws.path(), home.path(), uninstall_args);
        assert_warns_in_gain_and_daily_check(
            ws.path(),
            home.path(),
            &format!("{label} after uninstall"),
        );
    }
}

#[test]
fn test_init_and_uninstall_cycle_for_all_project_scope_agents() {
    let cases: &[(&str, &[&str], &[&str])] = &[
        (
            "antigravity-project",
            &["--agent", "antigravity"],
            &["--agent", "antigravity", "--uninstall"],
        ),
        (
            "trae-project",
            &["--agent", "trae"],
            &["--agent", "trae", "--uninstall"],
        ),
        (
            "pi-project",
            &["--agent", "pi", "--auto-patch"],
            &["--agent", "pi", "--uninstall"],
        ),
        (
            "omp-project",
            &["--agent", "omp", "--auto-patch"],
            &["--agent", "omp", "--uninstall"],
        ),
    ];

    for (label, install_args, uninstall_args) in cases {
        let (ws, home) = fresh_env();
        run_rtk_init(ws.path(), home.path(), install_args);
        assert_silent_in_gain_and_daily_check(
            ws.path(),
            home.path(),
            &format!("{label} after install"),
        );

        run_rtk_init(ws.path(), home.path(), uninstall_args);
        assert_warns_in_gain_and_daily_check(
            ws.path(),
            home.path(),
            &format!("{label} after uninstall"),
        );
    }
}

#[test]
fn test_hermes_individual_arms_and_negative_config() {
    // Arm 1: plugin __init__.py alone (without config.yaml) silences warning
    {
        let (ws, home) = fresh_env();
        let plugin_dir = home.path().join(".hermes/plugins/rtk-rewrite");
        fs::create_dir_all(&plugin_dir).expect("create hermes plugin dir");
        fs::write(plugin_dir.join("__init__.py"), "# rtk plugin\n").expect("write __init__.py");
        assert_silent_in_gain_and_daily_check(
            ws.path(),
            home.path(),
            "Hermes plugin __init__.py alone",
        );
    }

    // Arm 2: config.yaml with rtk-rewrite alone (without __init__.py) silences warning
    {
        let (ws, home) = fresh_env();
        let hermes_dir = home.path().join(".hermes");
        fs::create_dir_all(&hermes_dir).expect("create .hermes dir");
        fs::write(
            hermes_dir.join("config.yaml"),
            "plugins:\n  enabled:\n    - rtk-rewrite\n",
        )
        .expect("write hermes config.yaml");
        assert_silent_in_gain_and_daily_check(ws.path(), home.path(), "Hermes config.yaml alone");
    }

    // Negative: config.yaml with unrelated plugin still warns
    {
        let (ws, home) = fresh_env();
        let hermes_dir = home.path().join(".hermes");
        fs::create_dir_all(&hermes_dir).expect("create .hermes dir");
        fs::write(
            hermes_dir.join("config.yaml"),
            "plugins:\n  enabled:\n    - other-plugin\n",
        )
        .expect("write hermes config.yaml");
        assert_warns_in_gain_and_daily_check(
            ws.path(),
            home.path(),
            "Hermes config.yaml with unrelated plugin",
        );
    }
}

#[test]
fn test_gemini_matcher_variants_and_unregistered_script_rejected() {
    let (ws, home) = fresh_env();
    let gemini_dir = home.path().join(".gemini");
    fs::create_dir_all(&gemini_dir).expect("create .gemini dir");

    // 1. Wildcard "*" matcher silences warning
    fs::write(
        gemini_dir.join("settings.json"),
        r#"{
            "hooks": {
                "BeforeTool": [
                    {
                        "matcher": "*",
                        "hooks": [{ "command": "rtk hook gemini" }]
                    }
                ]
            }
        }"#,
    )
    .expect("write gemini wildcard settings.json");
    assert_silent_in_gain_and_daily_check(ws.path(), home.path(), "Gemini wildcard '*' matcher");

    // 2. Non-shell matcher ("ReadFile") must NOT count as configured
    fs::write(
        gemini_dir.join("settings.json"),
        r#"{
            "hooks": {
                "BeforeTool": [
                    {
                        "matcher": "ReadFile",
                        "hooks": [{ "command": "rtk hook gemini" }]
                    }
                ]
            }
        }"#,
    )
    .expect("write gemini non-shell settings.json");
    assert_warns_in_gain_and_daily_check(ws.path(), home.path(), "Gemini non-shell matcher");

    // 3. Unregistered script file without settings.json entry must NOT silence warning
    fs::remove_file(gemini_dir.join("settings.json")).expect("remove settings.json");
    let gemini_hooks = gemini_dir.join("hooks");
    fs::create_dir_all(&gemini_hooks).expect("create .gemini/hooks");
    fs::write(
        gemini_hooks.join("rtk-hook-gemini.sh"),
        "#!/usr/bin/env bash\n",
    )
    .expect("write unregistered gemini script");
    assert_warns_in_gain_and_daily_check(
        ws.path(),
        home.path(),
        "Gemini unregistered script file alone",
    );
}

#[test]
fn test_cursor_unregistered_legacy_script_warns() {
    let (ws, home) = fresh_env();
    let cursor_hooks = home.path().join(".cursor/hooks");
    fs::create_dir_all(&cursor_hooks).expect("create .cursor/hooks");
    fs::write(cursor_hooks.join("rtk-rewrite.sh"), "#!/usr/bin/env bash\n")
        .expect("write stray cursor script");

    assert_warns_in_gain_and_daily_check(
        ws.path(),
        home.path(),
        "Cursor unregistered legacy script alone",
    );
}

#[test]
fn test_outdated_claude_hook_still_warns_even_with_other_agent() {
    let (ws, home) = fresh_env();
    // Configure Antigravity in workspace
    run_rtk_init(ws.path(), home.path(), &["--agent", "antigravity"]);

    // Configure outdated Claude script hook (version 2 < 4)
    let hooks_dir = home.path().join(".claude/hooks");
    fs::create_dir_all(&hooks_dir).expect("create .claude/hooks");
    fs::write(
        hooks_dir.join("rtk-rewrite.sh"),
        "#!/usr/bin/env bash\n# rtk-hook-version: 2\n",
    )
    .expect("write old hook");

    let (_, _, gain_stderr) = run_rtk_gain(ws.path(), home.path());
    assert!(
        gain_stderr.contains(OUTDATED_WARNING),
        "Outdated hook must warn in rtk gain even when another agent is installed. Got:\n{gain_stderr}"
    );

    let (_, _, ls_stderr) = run_rtk_ls(ws.path(), home.path());
    assert!(
        ls_stderr.contains(OUTDATED_WARNING),
        "Outdated hook must warn in daily check even when another agent is installed. Got:\n{ls_stderr}"
    );
}
