//! Deterministic subprocess-boundary checks for agent integrations.
//!
//! These tests use a fake Codex home and the locally built RTK binary.  They
//! never start a real model or consume an agent/API request.

use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

fn run_rtk(
    args: &[&str],
    input: Option<&str>,
    codex_home: Option<&std::path::Path>,
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rtk"));
    command.args(args).env("LC_ALL", "C");
    if let Some(home) = codex_home {
        command.env("CODEX_HOME", home);
    }
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn RTK subprocess");
    if let Some(input) = input {
        child
            .stdin
            .take()
            .expect("subprocess stdin")
            .write_all(input.as_bytes())
            .expect("write subprocess input");
    }
    child.wait_with_output().expect("wait for RTK subprocess")
}

#[test]
fn fake_codex_home_reports_each_integration_dimension_independently() {
    let home = tempfile::tempdir().expect("fake Codex home");
    std::fs::write(home.path().join("AGENTS.md"), "@RTK.md\n").expect("write AGENTS.md");
    std::fs::write(home.path().join("RTK.md"), "# RTK policy\n").expect("write RTK.md");
    std::fs::write(
        home.path().join("config.toml"),
        "[mcp_servers.rtk]\ncommand = \"rtk\"\nargs = [\"mcp\"]\n\n[hooks]\n[[hooks.PreToolUse]]\nmatcher = \"Bash\"\n[[hooks.PreToolUse.hooks]]\ntype = \"command\"\ncommand = \"rtk hook codex\"\n",
    )
    .expect("write Codex config");

    let output = run_rtk(
        &["doctor", "--agent", "codex", "--format", "json"],
        None,
        Some(home.path()),
    );
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).expect("doctor JSON");
    assert_eq!(report["instructions"], "present");
    assert_eq!(report["mcp"], "present");
    assert_eq!(report["hook"], "ready");
    assert_eq!(report["hook_trust"], "host-managed");
    assert_eq!(report["profile"], "default");
    assert_eq!(report["live_verification"], "unverified");
}

#[test]
fn fake_codex_host_boundary_preserves_tool_input_and_approval_surface() {
    let event = r#"{"hook_event_name":"PreToolUse","permission_mode":"bypassPermissions","tool_name":"Bash","cwd":"C:\\work","tool_input":{"command":"git status","description":"keep","timeout_ms":5000}}"#;
    let output = run_rtk(&["hook", "codex"], Some(event), None);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let response: Value = serde_json::from_slice(&output.stdout).expect("hook JSON");
    let updated = &response["hookSpecificOutput"]["updatedInput"];
    assert_eq!(updated["command"], "rtk git status");
    assert_eq!(updated["description"], "keep");
    assert_eq!(updated["timeout_ms"], 5000);
    assert!(response["hookSpecificOutput"]
        .get("permissionDecision")
        .is_none());
}
