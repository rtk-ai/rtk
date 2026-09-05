//! Deterministic output-only host-adapter contract checks.

use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

fn run_adapter(payload: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["hook", "claude", "--event", "post-tool-use"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn output adapter");
    child
        .stdin
        .take()
        .expect("adapter stdin")
        .write_all(payload.as_bytes())
        .expect("write adapter payload");
    child.wait_with_output().expect("wait for output adapter")
}

#[test]
fn native_read_is_filtered_as_supplemental_context_without_reexecution() {
    let output = run_adapter(
        r#"{"hook_event_name":"PostToolUse","tool_name":"Read","tool_input":{"file_path":"src/main.rs","command":"exit 91"},"tool_response":"// noise\nfn main() {}\n"}"#,
    );
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "adapter JSON: {error}; stdout={:?}; stderr={:?}",
            output.stdout, output.stderr
        )
    });
    let context = &response["hookSpecificOutput"]["additionalContext"];
    assert_eq!(context["replacement_supported"], false);
    assert!(context["output"]
        .as_str()
        .unwrap()
        .contains("2: fn main() {}"));
    assert!(
        output.stderr.is_empty(),
        "adapter stderr: {:?}",
        output.stderr
    );
}

#[test]
fn native_errors_images_and_already_filtered_results_are_not_replaced() {
    for payload in [
        r#"{"hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{"command":"exit 91"},"tool_response":{"stdout":"ok","stderr":"failure","exit_code":91}}"#,
        r#"{"hook_event_name":"PostToolUse","tool_name":"Read","tool_response":{"type":"image","data":"base64"}}"#,
        r#"{"hook_event_name":"PostToolUse","tool_name":"Read","tool_input":{"rtk_execution_id":"exec-1"},"tool_response":"// noise\nfn main() {}\n"}"#,
    ] {
        let output = run_adapter(payload);
        assert!(output.status.success());
        assert!(output.stdout.is_empty());
    }
}
