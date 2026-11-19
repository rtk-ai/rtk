//! The hook config checked in at `.github/hooks/rtk-rewrite.json` is what the
//! Copilot cloud agent runs on every tool call of this repository, in a sandbox
//! where `rtk` is not installed. It must call a subcommand that exists and must
//! pass the call through untouched when `rtk` is missing.

#![cfg(unix)]

use std::io::Write;
use std::process::{Command, Stdio};

mod common;

const PAYLOAD: &str = r#"{"tool_name":"run_in_terminal","tool_input":{"command":"git status"}}"#;

fn bash_hook() -> String {
    let raw = include_str!("../.github/hooks/rtk-rewrite.json");
    let config: serde_json::Value = serde_json::from_str(raw).expect("hook config is JSON");
    let entries = config["hooks"]["PreToolUse"]
        .as_array()
        .expect("PreToolUse entries");
    assert_eq!(entries.len(), 1, "one entry: each one is a process spawn");
    entries[0]["bash"]
        .as_str()
        .expect("bash command")
        .to_string()
}

fn run_hook(path: &std::path::Path) -> std::process::Output {
    let mut cmd = Command::new("/bin/sh");
    // Isolation first: it clears the environment and picks the working directory.
    common::isolate_rtk(&mut cmd);
    let mut child = cmd
        .args(["-c", &bash_hook()])
        .env("PATH", path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hook");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(PAYLOAD.as_bytes())
        .expect("write payload");
    child.wait_with_output().expect("hook output")
}

#[test]
fn hook_is_silent_and_succeeds_without_rtk() {
    let empty = common::tempdir();
    let out = run_hook(empty.path());
    assert!(out.status.success(), "exit {:?}", out.status.code());
    assert!(out.stdout.is_empty());
    assert!(out.stderr.is_empty());
}

#[test]
fn hook_reaches_rtk_when_present() {
    let bin = common::tempdir();
    // Only the path is read; no child is spawned from it.
    std::os::unix::fs::symlink(common::rtk_command().get_program(), bin.path().join("rtk"))
        .expect("link rtk");
    let out = run_hook(bin.path());
    assert!(out.status.success(), "exit {:?}", out.status.code());
    let reply: serde_json::Value = serde_json::from_slice(&out.stdout).expect("hook reply is JSON");
    let hook_output = &reply["hookSpecificOutput"];
    assert_eq!(
        hook_output["updatedInput"]["command"], "rtk git status",
        "not a rewrite: {reply}"
    );
    assert!(
        hook_output.get("permissionDecision").is_none(),
        "deny: {reply}"
    );
}
