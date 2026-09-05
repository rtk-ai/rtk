//! MCP response-envelope contract checks at the real stdio boundary.

use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

fn call_mcp(arguments: Value) -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn MCP server");
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": "run_filtered", "arguments": arguments }
    });
    child
        .stdin
        .take()
        .expect("MCP stdin")
        .write_all(format!("{request}\n").as_bytes())
        .expect("write MCP request");
    let output = child.wait_with_output().expect("wait for MCP server");
    assert!(output.status.success(), "MCP stderr: {:?}", output.stderr);
    serde_json::from_slice::<Value>(&output.stdout).expect("MCP response JSON")
}

#[test]
fn compact_execution_response_omits_internal_duplicate_metadata() {
    let response = call_mcp(serde_json::json!({
        "rtk_args": ["read", "tests/fixtures/agent_git_status_clean.txt", "-l", "none"],
        "response_mode": "compact",
        "max_output_tokens": 64
    }));
    let result = &response["result"];
    let structured = result["structuredContent"]
        .as_object()
        .expect("structured object");
    assert!(structured.contains_key("exit_code"));
    assert!(structured.contains_key("stdout"));
    assert!(structured.contains_key("stderr"));
    assert!(!structured.contains_key("rtk_args"));
    assert!(!structured.contains_key("tee_path"));
    assert_eq!(
        result["content"][0]["text"],
        serde_json::to_string(structured).expect("compact JSON")
    );
}

#[test]
fn legacy_execution_response_preserves_full_result_shape() {
    let response = call_mcp(serde_json::json!({
        "rtk_args": ["read", "tests/fixtures/agent_git_status_clean.txt", "-l", "none"],
        "response_mode": "legacy"
    }));
    let structured = response["result"]["structuredContent"]
        .as_object()
        .expect("structured object");
    assert!(structured.contains_key("rtk_args"));
    assert!(structured.contains_key("filtered"));
    assert!(structured.contains_key("metrics_available"));
}
