mod common;

use rusqlite::Connection;
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn command(dir: &Path) -> Command {
    let mut cmd = common::rtk_command();
    cmd.current_dir(dir)
        .env("RTK_DB_PATH", dir.join("history.db"))
        .env("RTK_RECALL_DB", dir.join("recall.db"))
        .env("CLAUDE_CONFIG_DIR", dir.join("claude"));
    cmd
}

fn exported(dir: &Path) -> Value {
    let output = command(dir)
        .args(["gain", "--history", "--format", "json"])
        .output()
        .expect("gain");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON export")
}

#[test]
fn hook_and_command_share_explicit_ids() {
    let dir = tempfile::tempdir().expect("directory");
    std::fs::write(dir.path().join("input.txt"), "first\nsecond\n").expect("fixture");
    let payload = json!({
        "tool_name": "Bash", "session_id": "session-a", "tool_use_id": "tool-a",
        "cwd": dir.path(), "tool_input": {"command": "head -n 1 input.txt", "timeout": 1200}
    });
    let mut hook = command(dir.path())
        .args(["hook", "claude"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hook");
    hook.stdin
        .take()
        .expect("stdin")
        .write_all(payload.to_string().as_bytes())
        .expect("payload");
    let output = hook.wait_with_output().expect("hook output");
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).expect("hook JSON");
    let rewritten = response["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .expect("rewrite");
    let words: Vec<&str> = rewritten.split_whitespace().collect();
    assert_eq!(&words[..2], &["rtk", "--hook-event-id"]);
    assert_eq!(words[2].len(), 32);
    assert_eq!(
        response["hookSpecificOutput"]["updatedInput"]["timeout"],
        1200
    );
    assert!(response["hookSpecificOutput"]["permissionDecision"].is_null());
    let result = command(dir.path())
        .args(&words[1..])
        .output()
        .expect("rewritten command");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let export = exported(dir.path());
    let row = &export["commands"][0];
    assert_eq!(row["hook_event_id"], words[2]);
    assert_eq!(row["tool_use_id"], "tool-a");
    assert_eq!(row["session_id"], "session-a");
}

#[test]
fn repeated_filtered_reads_count_actual_stdout_and_stay_unattributed() {
    let dir = tempfile::tempdir().expect("directory");
    let conn = Connection::open(dir.path().join("recall.db")).expect("db");
    conn.execute_batch(
        "CREATE TABLE recall (
        hash TEXT PRIMARY KEY, command TEXT NOT NULL, cwd TEXT, exit_code INTEGER,
        created_at INTEGER NOT NULL, total_lines INTEGER NOT NULL, shown_upto INTEGER NOT NULL,
        byte_size INTEGER NOT NULL, truncated INTEGER NOT NULL, codec TEXT NOT NULL,
        blob BLOB NOT NULL, recalled INTEGER NOT NULL DEFAULT 0);",
    )
    .expect("legacy schema");
    let raw = b"first\nsecond\nthird\n";
    conn.execute(
        "INSERT INTO recall VALUES ('abc123','test','',0,0,3,2,19,0,'raw',?1,0)",
        [raw.as_slice()],
    )
    .expect("fixture");
    drop(conn);
    let mut expected = 0;
    for args in [
        vec!["recall", "abc123"],
        vec!["recall", "abc123"],
        vec!["recall", "abc123", "--lines", "1"],
        vec!["recall", "abc123", "--full", "--grep", "second"],
        vec!["recall", "abc123", "--full", "--grep", "absent"],
    ] {
        let output = command(dir.path()).args(args).output().expect("recall");
        assert!(output.status.success());
        expected += output.stdout.len().div_ceil(4);
    }
    let export = exported(dir.path());
    let accounting = &export["output_accounting"];
    assert_eq!(accounting["recall_events"], 5);
    assert_eq!(accounting["unattributed_recall_events"], 5);
    assert_eq!(accounting["retrieved_output_tokens"], expected as i64);
    assert_eq!(accounting["observed_net_saved_tokens"], -(expected as i64));
    assert_eq!(accounting["measurement_complete"], false);
    let rows = export["recall_events"].as_array().expect("events");
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0]["emitted_bytes"], 0);
    assert!(rows.iter().all(|r| r["source_capture_id"].is_null()));
    let conn = Connection::open(dir.path().join("recall.db")).expect("db");
    let unique_recalls: i64 = conn
        .query_row("SELECT recalls FROM recall_stats", [], |r| r.get(0))
        .expect("legacy stats");
    assert_eq!(
        unique_recalls, 1,
        "legacy unique-content metric keeps its meaning"
    );
}

#[test]
fn command_without_hook_keeps_hook_fields_null() {
    let dir = common::temp_git_repo();
    let output = command(dir.path())
        .args(["git", "log", "--oneline", "-1"])
        .output()
        .expect("command");
    assert!(output.status.success());
    let export = exported(dir.path());
    assert!(export["commands"][0]["invocation_id"].is_string());
    assert!(export["commands"][0]["hook_event_id"].is_null());
    assert!(export["commands"][0]["session_id"].is_null());
}
