//! MCP integration tests.
//!
//! These tests spawn the real `rtk mcp-serve` binary and exercise the full
//! JSON-RPC handshake over stdin/stdout pipes.
//!
//! Run with: cargo test --test mcp_integration -- --ignored
//!
//! Requirements:
//!   - RTK binary compiled: `cargo build --release`
//!   - The binary is located at `target/release/rtk` (or debug)

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn rtk_binary() -> std::path::PathBuf {
    // Prefer release build for speed; fall back to debug
    let release = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/release/rtk");
    let debug = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/rtk");
    if release.exists() {
        release
    } else {
        debug
    }
}

struct McpProcess {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
}

impl McpProcess {
    fn spawn() -> Self {
        let mut child = Command::new(rtk_binary())
            .arg("mcp-serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("Failed to spawn rtk mcp-serve");

        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        Self {
            child,
            stdin,
            reader: BufReader::new(stdout),
        }
    }

    fn send(&mut self, line: &str) {
        self.stdin
            .write_all(line.as_bytes())
            .expect("write to mcp-serve stdin");
        self.stdin.write_all(b"\n").expect("write newline");
        self.stdin.flush().expect("flush stdin");
    }

    fn recv(&mut self) -> serde_json::Value {
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .expect("read from mcp-serve stdout");
        serde_json::from_str(line.trim()).expect("parse JSON response")
    }

    fn send_recv(&mut self, line: &str) -> serde_json::Value {
        self.send(line);
        self.recv()
    }
}

impl Drop for McpProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[test]
#[ignore]
fn test_initialize_handshake() {
    let mut proc = McpProcess::spawn();

    let resp = proc.send_recv(
        r#"{"jsonrpc":"2.0","method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}},"id":1}"#,
    );

    assert_eq!(resp["jsonrpc"].as_str().unwrap(), "2.0");
    assert_eq!(resp["id"].as_i64().unwrap(), 1);
    assert!(resp["result"].is_object(), "expected result object");
    assert_eq!(
        resp["result"]["protocolVersion"].as_str().unwrap(),
        "2024-11-05"
    );
    assert_eq!(
        resp["result"]["serverInfo"]["name"].as_str().unwrap(),
        "rtk"
    );
}

#[test]
#[ignore]
fn test_tools_list_returns_bash() {
    let mut proc = McpProcess::spawn();

    // Initialize first
    proc.send_recv(r#"{"jsonrpc":"2.0","method":"initialize","params":{},"id":1}"#);

    let resp = proc.send_recv(r#"{"jsonrpc":"2.0","method":"tools/list","params":{},"id":2}"#);

    assert!(resp["result"].is_object());
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    assert!(!tools.is_empty(), "expected at least one tool");
    assert_eq!(tools[0]["name"].as_str().unwrap(), "bash");

    let schema = &tools[0]["inputSchema"];
    assert_eq!(schema["type"].as_str().unwrap(), "object");
    assert!(
        schema["properties"]["command"].is_object(),
        "command property missing"
    );
}

#[test]
#[ignore]
fn test_tools_call_bash_echo() {
    let mut proc = McpProcess::spawn();
    proc.send_recv(r#"{"jsonrpc":"2.0","method":"initialize","params":{},"id":1}"#);

    let resp = proc.send_recv(
        r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"bash","arguments":{"command":"echo rtk_mcp_ok"}},"id":3}"#,
    );

    assert!(
        resp.get("result").is_some(),
        "expected result, got: {}",
        resp
    );
    assert_eq!(resp["result"]["isError"].as_bool(), Some(false));
    let text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        text.contains("rtk_mcp_ok"),
        "expected echo output in: {:?}",
        text
    );
}

#[test]
#[ignore]
fn test_tools_call_bash_git_status() {
    let mut proc = McpProcess::spawn();
    proc.send_recv(r#"{"jsonrpc":"2.0","method":"initialize","params":{},"id":1}"#);

    let resp = proc.send_recv(
        r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"bash","arguments":{"command":"git status"}},"id":4}"#,
    );

    assert!(
        resp.get("result").is_some(),
        "expected result, got: {}",
        resp
    );
    // git status in the rtk repo should succeed
    assert_eq!(
        resp["result"]["isError"].as_bool(),
        Some(false),
        "git status failed unexpectedly: {}",
        resp
    );
    let text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(!text.is_empty(), "expected non-empty git status output");
}

#[test]
#[ignore]
fn test_tools_call_bash_filters_git_output() {
    let mut proc = McpProcess::spawn();
    proc.send_recv(r#"{"jsonrpc":"2.0","method":"initialize","params":{},"id":1}"#);

    let resp = proc.send_recv(
        r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"bash","arguments":{"command":"git log -5"}},"id":5}"#,
    );

    assert!(resp.get("result").is_some());
    let text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
    // RTK git log filter produces compact output — verify it's shorter than raw git log
    // (raw git log -5 would be at minimum 5 * ~6 lines = 30 lines; RTK compacts to 5 lines)
    let line_count = text.lines().count();
    assert!(line_count > 0, "expected some output from git log");
    assert!(
        line_count <= 20,
        "expected RTK to compact git log (got {} lines, expected ≤20)",
        line_count
    );
}

#[test]
#[ignore]
fn test_tools_call_failing_command_is_error() {
    let mut proc = McpProcess::spawn();
    proc.send_recv(r#"{"jsonrpc":"2.0","method":"initialize","params":{},"id":1}"#);

    // A command that always fails
    let resp = proc.send_recv(
        r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"bash","arguments":{"command":"sh -c 'exit 42'"}},"id":6}"#,
    );

    assert!(resp.get("result").is_some());
    assert_eq!(
        resp["result"]["isError"].as_bool(),
        Some(true),
        "expected isError=true for failing command"
    );
}

#[test]
#[ignore]
fn test_unknown_method_returns_error() {
    let mut proc = McpProcess::spawn();
    proc.send_recv(r#"{"jsonrpc":"2.0","method":"initialize","params":{},"id":1}"#);

    let resp =
        proc.send_recv(r#"{"jsonrpc":"2.0","method":"nonexistent/method","params":{},"id":7}"#);

    assert!(resp["error"].is_object(), "expected error object");
    assert_eq!(resp["error"]["code"].as_i64().unwrap(), -32601);
}

#[test]
#[ignore]
fn test_parse_error_returns_error() {
    let mut proc = McpProcess::spawn();

    let resp = proc.send_recv("this is not json {{{");
    assert!(resp["error"].is_object(), "expected parse error");
    assert_eq!(resp["error"]["code"].as_i64().unwrap(), -32700);
}

#[test]
#[ignore]
fn test_notification_receives_no_response() {
    let mut proc = McpProcess::spawn();

    // Send initialize to get a response
    proc.send_recv(r#"{"jsonrpc":"2.0","method":"initialize","params":{},"id":1}"#);

    // Send initialized notification (no id) — must get NO response
    proc.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);

    // Send ping — should get a response (proves server is still alive)
    let resp = proc.send_recv(r#"{"jsonrpc":"2.0","method":"ping","params":{},"id":2}"#);
    assert!(resp["result"].is_object(), "ping should return result");
}

#[test]
#[ignore]
fn test_multiple_sequential_calls() {
    let mut proc = McpProcess::spawn();
    proc.send_recv(r#"{"jsonrpc":"2.0","method":"initialize","params":{},"id":1}"#);

    // Run 3 commands sequentially, verify ids are preserved
    for i in 2..=4 {
        let req = format!(
            r#"{{"jsonrpc":"2.0","method":"tools/call","params":{{"name":"bash","arguments":{{"command":"echo seq_{}"}}}},"id":{}}}"#,
            i, i
        );
        let resp = proc.send_recv(&req);
        assert_eq!(resp["id"].as_i64().unwrap(), i as i64);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains(&format!("seq_{}", i)));
    }
}

#[test]
#[ignore]
fn test_shutdown_cleanly_closes_server() {
    let mut proc = McpProcess::spawn();
    proc.send_recv(r#"{"jsonrpc":"2.0","method":"initialize","params":{},"id":1}"#);

    let resp = proc.send_recv(r#"{"jsonrpc":"2.0","method":"shutdown","params":{},"id":99}"#);
    // shutdown returns null result (not an object) — just verify no error
    assert!(resp.get("error").is_none(), "shutdown returned error: {}", resp);

    // After shutdown, reading from stdout should yield EOF
    let mut line = String::new();
    let n = proc.reader.read_line(&mut line).unwrap_or(0);
    assert_eq!(n, 0, "expected EOF after shutdown");
}
