//! Tracking migration and execution-identity checks at process boundaries.

use rusqlite::Connection;
use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

fn run_rtk(db: &std::path::Path, args: &[&str], input: Option<&str>) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rtk"));
    command
        .args(args)
        .env("RTK_DB_PATH", db)
        .env("RTK_TEE", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command.spawn().expect("spawn RTK");
    if let Some(input) = input {
        child
            .stdin
            .take()
            .expect("RTK stdin")
            .write_all(input.as_bytes())
            .expect("write RTK stdin");
    }
    child.wait_with_output().expect("wait for RTK")
}

fn call_mcp(db: &std::path::Path) -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("mcp")
        .env("RTK_DB_PATH", db)
        .env("RTK_TEE", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn MCP");
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "run_filtered",
            "arguments": {
                "rtk_args": ["read", "tests/fixtures/agent_git_status_clean.txt", "-l", "none"],
                "response_mode": "legacy"
            }
        }
    });
    child
        .stdin
        .take()
        .expect("MCP stdin")
        .write_all(format!("{request}\n").as_bytes())
        .expect("write MCP request");
    let output = child.wait_with_output().expect("wait for MCP");
    assert!(output.status.success(), "{:?}", output.stderr);
    serde_json::from_slice(&output.stdout).expect("MCP JSON")
}

#[test]
fn old_schema_is_migrated_without_losing_history_and_new_runs_are_linked() {
    let temp = tempfile::tempdir().expect("tracking directory");
    let db = temp.path().join("history.db");
    let conn = Connection::open(&db).expect("old tracking database");
    conn.execute_batch(
        "CREATE TABLE commands (
            id INTEGER PRIMARY KEY,
            timestamp TEXT NOT NULL,
            original_cmd TEXT NOT NULL,
            rtk_cmd TEXT NOT NULL,
            input_tokens INTEGER NOT NULL,
            output_tokens INTEGER NOT NULL,
            saved_tokens INTEGER NOT NULL,
            savings_pct REAL NOT NULL,
            exec_time_ms INTEGER DEFAULT 0,
            project_path TEXT DEFAULT ''
        );
        CREATE TABLE hook_decisions (
            id INTEGER PRIMARY KEY,
            timestamp TEXT NOT NULL,
            session_id TEXT NOT NULL,
            tool_use_id TEXT NOT NULL,
            project_path TEXT DEFAULT '',
            raw_cmd TEXT NOT NULL,
            decision TEXT NOT NULL,
            rewritten_cmd TEXT,
            rtk_version TEXT NOT NULL
        );
        INSERT INTO commands (
            timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens,
            saved_tokens, savings_pct, exec_time_ms, project_path
        ) VALUES (
            '2026-09-04T00:00:00Z', 'old command', 'rtk old command',
            100, 25, 75, 75.0, 1, 'D:\\src\\rtk'
        );
        INSERT INTO hook_decisions (
            timestamp, session_id, tool_use_id, raw_cmd, decision, rtk_version
        ) VALUES ('2026-09-04T00:00:00Z', 'session-old', 'tool-old',
                  'git status', 'ask', '0.46.1');",
    )
    .expect("create old schema");
    drop(conn);

    let output = run_rtk(
        &db,
        &[
            "read",
            "-l",
            "none",
            "tests/fixtures/agent_git_status_clean.txt",
        ],
        None,
    );
    assert!(output.status.success(), "{:?}", output.stderr);

    let conn = Connection::open(&db).expect("open migrated database");
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("schema version");
    assert!(version >= 4);
    let old_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM commands WHERE original_cmd = 'old command'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old_count, 1);
    let event_table: String = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'execution_events'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(event_table, "execution_events");

    let response = call_mcp(&db);
    let result = &response["result"]["structuredContent"];
    let execution_id = result["execution_id"]
        .as_str()
        .expect("MCP exposes stable execution id");
    assert!(result["metrics_available"].as_bool().unwrap());
    assert_eq!(result["input_tokens"], result["output_tokens"]);
    let command_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM commands WHERE execution_id = ?1",
            [execution_id],
            |row| row.get(0),
        )
        .unwrap();
    let event_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM execution_events WHERE execution_id = ?1",
            [execution_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(command_count, 1);
    assert_eq!(event_count, 1);
}

#[test]
fn repeated_hook_notifications_are_idempotent_by_explicit_identity() {
    let temp = tempfile::tempdir().expect("tracking directory");
    let db = temp.path().join("history.db");
    let payload = r#"{"session_id":"session-1","tool_use_id":"tool-1","cwd":"D:\\src\\rtk","tool_name":"Bash","tool_input":{"command":"git status"}}"#;
    for _ in 0..2 {
        let output = run_rtk(&db, &["hook", "claude"], Some(payload));
        assert!(output.status.success(), "{:?}", output.stderr);
    }
    let conn = Connection::open(&db).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM hook_decisions WHERE session_id = 'session-1'
             AND tool_use_id = 'tool-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}
