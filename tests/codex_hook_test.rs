//! End-to-end checks for the native Codex hook protocol.

use std::io::Write;
use std::process::{Command, Output, Stdio};

fn run_hook(args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk Codex hook");
    child
        .stdin
        .take()
        .expect("hook stdin")
        .write_all(input.as_bytes())
        .expect("write hook input");
    child.wait_with_output().expect("wait for rtk Codex hook")
}

#[test]
fn codex_subagent_start_event_is_a_silent_noop() {
    let output = run_hook(
        &["hook", "codex", "--event", "subagent-start"],
        r#"{"hook_event_name":"SubagentStart","session_id":"s1"}"#,
    );

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn codex_pre_tool_use_returns_only_a_safe_input_update() {
    let output = run_hook(
        &["hook", "codex"],
        r#"{"hook_event_name":"PreToolUse","permission_mode":"bypassPermissions","tool_name":"Bash","tool_input":{"command":"git status","description":"Inspect repository"}}"#,
    );

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let response: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON response");
    assert_eq!(
        response["hookSpecificOutput"]["hookEventName"],
        "PreToolUse"
    );
    assert_eq!(
        response["hookSpecificOutput"]["updatedInput"]["command"],
        "rtk git status"
    );
    assert_eq!(
        response["hookSpecificOutput"]["updatedInput"]["description"],
        "Inspect repository"
    );
    assert!(response["hookSpecificOutput"]
        .get("permissionDecision")
        .is_none());
}

#[test]
fn codex_does_not_rewrite_when_original_approval_cannot_be_preserved() {
    let output = run_hook(
        &["hook", "codex"],
        r#"{"hook_event_name":"PreToolUse","permission_mode":"default","tool_name":"Bash","tool_input":{"command":"git status"}}"#,
    );

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn codex_does_not_rewrite_unattestable_shell_expression() {
    let output = run_hook(
        &["hook", "codex"],
        r#"{"hook_event_name":"PreToolUse","permission_mode":"bypassPermissions","tool_name":"Bash","tool_input":{"command":"git status > result.txt"}}"#,
    );

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}
