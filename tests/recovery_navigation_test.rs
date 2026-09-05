//! Bounded, ID-based source and recovery navigation checks.

use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

fn artifact() -> (tempfile::TempDir, String) {
    let temp = tempfile::tempdir().expect("recovery directory");
    let id = "1234567890_navigation.lossless.log".to_string();
    let mut content = String::new();
    for line in 1..=200 {
        content.push_str(&format!("L{line:03}\n"));
    }
    content.push_str("token=super-secret\n");
    content.push_str("Authorization: Bearer\nFAKE_TEST_TOKEN\n");
    content.replace_range(
        content.find("L180").expect("line 180")..content.find("L180").unwrap() + 4,
        "ERROR",
    );
    std::fs::write(temp.path().join(&id), content).expect("write recovery fixture");
    (temp, id)
}

fn call_mcp(tee_dir: &std::path::Path, name: &str, arguments: Value) -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("mcp")
        .env("RTK_TEE_DIR", tee_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn MCP server");
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    });
    child
        .stdin
        .take()
        .expect("MCP stdin")
        .write_all(format!("{request}\n").as_bytes())
        .expect("write MCP request");
    let output = child.wait_with_output().expect("wait for MCP server");
    assert!(output.status.success(), "MCP stderr: {:?}", output.stderr);
    serde_json::from_slice(&output.stdout).expect("MCP response JSON")
}

#[test]
fn cli_range_reads_original_lines_from_a_recovery_id() {
    let (temp, id) = artifact();
    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args([
            "read",
            "-l",
            "none",
            "--recovery",
            &id,
            "--lines",
            "120:122",
        ])
        .env("RTK_TEE_DIR", temp.path())
        .output()
        .expect("run recovery range");
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "L120\nL121\nL122\n"
    );
}

#[test]
fn mcp_pages_a_recovery_artifact_without_rerunning_a_producer() {
    let (temp, id) = artifact();
    let first = call_mcp(
        temp.path(),
        "read_recovery",
        serde_json::json!({
            "recovery_id": id,
            "lines": "120:122",
            "max_lines": 2
        }),
    );
    let first_result = &first["result"]["structuredContent"];
    assert_eq!(first_result["content"], "L120\nL121\n");
    assert_eq!(first_result["start_line"], 120);
    assert_eq!(first_result["end_line"], 121);
    assert_eq!(first_result["has_more"], true);
    assert_eq!(first_result["next_cursor"], 121);

    let second = call_mcp(
        temp.path(),
        "read_recovery",
        serde_json::json!({
            "recovery_id": "1234567890_navigation.lossless.log",
            "lines": "120:122",
            "cursor": 121,
            "max_lines": 2
        }),
    );
    let second_result = &second["result"]["structuredContent"];
    assert_eq!(second_result["content"], "L122\n");
    assert_eq!(second_result["has_more"], false);
    assert!(second_result.get("next_cursor").is_none());
}

#[test]
fn mcp_search_returns_original_location_and_context() {
    let (temp, id) = artifact();
    let response = call_mcp(
        temp.path(),
        "search_recovery",
        serde_json::json!({
            "recovery_id": id,
            "pattern": "ERROR",
            "context": 1
        }),
    );
    let result = &response["result"]["structuredContent"];
    assert_eq!(result["match_count"], 1);
    let found = &result["matches"][0];
    assert_eq!(found["line"], 180);
    assert_eq!(found["context"][0]["line"], 179);
    assert_eq!(found["context"][1]["line"], 180);
    assert_eq!(found["context"][2]["line"], 181);
}

#[test]
fn mcp_recovery_navigation_redacts_sensitive_content() {
    let (temp, id) = artifact();
    let page = call_mcp(
        temp.path(),
        "read_recovery",
        serde_json::json!({
            "recovery_id": id,
            "lines": "201:201"
        }),
    );
    let page_content = page["result"]["structuredContent"]["content"]
        .as_str()
        .expect("recovery page content");
    assert!(page_content.contains("[REDACTED]"));
    assert!(!page_content.contains("super-secret"));

    let search = call_mcp(
        temp.path(),
        "search_recovery",
        serde_json::json!({
            "recovery_id": id,
            "pattern": "super-secret",
            "context": 1
        }),
    );
    let result = &search["result"]["structuredContent"];
    let found = &result["matches"][0];
    assert_eq!(found["text"], "[REDACTED]");
    assert!(!found.to_string().contains("super-secret"));

    let multiline = call_mcp(
        temp.path(),
        "search_recovery",
        serde_json::json!({
            "recovery_id": id,
            "pattern": "Authorization",
            "context": 1
        }),
    );
    assert!(!multiline["result"]["structuredContent"]["matches"]
        .to_string()
        .contains("FAKE_TEST_TOKEN"));
}

#[test]
fn mcp_recovery_page_bounds_oversized_lines_and_advances() {
    let temp = tempfile::tempdir().expect("recovery directory");
    let id = "1234567890_oversized.lossless.log";
    let content = format!("before\n{}\nafter\n", "x".repeat(1_200_000));
    std::fs::write(temp.path().join(id), content).expect("write oversized recovery fixture");

    let page = call_mcp(
        temp.path(),
        "read_recovery",
        serde_json::json!({
            "recovery_id": id,
            "lines": "2:2",
            "max_lines": 1
        }),
    );
    let result = &page["result"]["structuredContent"];
    let content = result["content"].as_str().expect("page content");
    assert!(content.contains("[rtk: line truncated"));
    assert!(content.len() <= 1_048_576);
    assert_eq!(result["next_cursor"], 2);

    let next = call_mcp(
        temp.path(),
        "read_recovery",
        serde_json::json!({
            "recovery_id": id,
            "lines": "2:3",
            "cursor": 2,
            "max_lines": 1
        }),
    );
    assert_eq!(next["result"]["structuredContent"]["content"], "after\n");

    let search = call_mcp(
        temp.path(),
        "search_recovery",
        serde_json::json!({
            "recovery_id": id,
            "pattern": "xxx",
            "context": 0
        }),
    );
    let search_result = &search["result"]["structuredContent"];
    assert_eq!(search_result["match_count"], 1);
    assert_eq!(search_result["truncated"], true);
    assert!(search_result["matches"][0]["text"]
        .as_str()
        .expect("bounded match text")
        .contains("[rtk: line truncated"));

    let marker_search = call_mcp(
        temp.path(),
        "search_recovery",
        serde_json::json!({
            "recovery_id": id,
            "pattern": "rtk: line truncated",
            "context": 0
        }),
    );
    assert_eq!(
        marker_search["result"]["structuredContent"]["match_count"],
        0
    );
}

#[test]
fn mcp_recovery_reader_keeps_buffer_boundary_newlines() {
    let temp = tempfile::tempdir().expect("recovery directory");
    let id = "1234567890_boundary.lossless.log";
    let content = format!("{}\nsecond\nthird\n", "x".repeat(8_191));
    std::fs::write(temp.path().join(id), content).expect("write boundary fixture");

    let page = call_mcp(
        temp.path(),
        "read_recovery",
        serde_json::json!({
            "recovery_id": id,
            "lines": "2:2"
        }),
    );
    assert_eq!(page["result"]["structuredContent"]["content"], "second\n");

    let search = call_mcp(
        temp.path(),
        "search_recovery",
        serde_json::json!({
            "recovery_id": id,
            "pattern": "second",
            "context": 0
        }),
    );
    assert_eq!(
        search["result"]["structuredContent"]["matches"][0]["line"],
        2
    );
}

#[test]
fn invalid_ranges_fail_without_falling_back_to_a_full_read() {
    let (temp, id) = artifact();
    for range in ["0:5", "10:3", "120", "120:"] {
        let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args(["read", "-l", "none", "--recovery", &id, "--lines", range])
            .env("RTK_TEE_DIR", temp.path())
            .output()
            .expect("run invalid recovery range");
        assert!(!output.status.success(), "accepted invalid range {range}");
    }
}
