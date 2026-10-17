use serde_json::{Value, json};
use std::io::Write;
use std::process::{Output, Stdio};

mod common;

fn run_minimax_hook(payload: &str) -> Output {
    let home = tempfile::tempdir().unwrap();
    let mut child = common::rtk_command()
        .args(["hook", "minimax"])
        .env("HOME", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn rtk hook minimax");
    child
        .stdin
        .take()
        .expect("missing hook stdin")
        .write_all(payload.as_bytes())
        .expect("failed to write hook payload");
    child.wait_with_output().expect("hook process failed")
}

#[test]
fn minimax_hook_rewrites_bash_and_preserves_timeout() {
    let payload = json!({
        "tool_name": "bash",
        "tool_input": {"command": "git status", "timeout": 30}
    })
    .to_string();
    let out = run_minimax_hook(&payload);
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).expect("hook stdout should be JSON");
    let updated = &v["hookSpecificOutput"]["updatedInput"];
    assert_eq!(updated["command"], "rtk git status");
    assert_eq!(updated["timeout"], 30);
}

#[test]
fn minimax_hook_bad_json_exits_zero_with_empty_stdout() {
    let out = run_minimax_hook("{not json");
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}
