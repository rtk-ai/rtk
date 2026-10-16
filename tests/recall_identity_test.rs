#![cfg(unix)]
mod common;

use rusqlite::Connection;
use std::path::Path;
use std::process::Command;

fn command(dir: &Path) -> Command {
    let mut cmd = common::rtk_command();
    cmd.current_dir(dir)
        .env("RTK_DB_PATH", dir.join("history.db"))
        .env("RTK_RECALL_DB", dir.join("recall.db"));
    cmd
}

fn capture(dir: &Path, max: &str) -> String {
    let output = command(dir)
        .args(["grep", "--max", max, "needle", "input.txt"])
        .output()
        .expect("capture command");
    assert!(output.status.success(), "{:?}", output);
    let text = String::from_utf8(output.stdout).expect("output");
    text.split("rtk recall ")
        .nth(1)
        .and_then(|hint| hint.split(']').next())
        .unwrap_or_else(|| panic!("missing recall hint: {text}"))
        .to_owned()
}

#[test]
fn printed_capture_hint_keeps_its_output_and_attribution_after_recapture() {
    let dir = tempfile::tempdir().expect("directory");
    let raw = (1..=30)
        .map(|i| {
            format!(
                "needle line {i} with enough synthetic content to exceed recovery hint overhead\n"
            )
        })
        .collect::<String>();
    std::fs::write(dir.path().join("input.txt"), raw).expect("fixture");
    let first = capture(dir.path(), "1");
    assert!(first.starts_with("c_"));
    assert_eq!(first.len(), 34);
    let before = command(dir.path())
        .args(["recall", &first])
        .output()
        .expect("recall before");
    assert!(before.status.success());
    let second = capture(dir.path(), "3");
    assert_ne!(first, second);
    let after = command(dir.path())
        .args(["recall", &first])
        .output()
        .expect("recall after");
    assert!(after.status.success());
    assert_eq!(before.stdout, after.stdout);
    assert_eq!(String::from_utf8_lossy(&after.stdout).lines().count(), 29);

    let listed = command(dir.path())
        .args(["recall", "--list"])
        .output()
        .expect("list");
    let listed = String::from_utf8(listed.stdout).expect("list text");
    assert!(listed.contains(&first));
    assert!(listed.contains(&second));
    let source = Connection::open(dir.path().join("recall.db")).expect("recall db");
    let invocation: String = source
        .query_row(
            "SELECT invocation_id FROM recall_captures WHERE capture_id = ?1",
            [&first[2..]],
            |r| r.get(0),
        )
        .expect("source invocation");
    let history = Connection::open(dir.path().join("history.db")).expect("history");
    let mut stmt = history
        .prepare("SELECT source_invocation_id, source_capture_id, emitted_bytes FROM recall_events")
        .expect("events");
    let events: Vec<(String, String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("query")
        .collect::<rusqlite::Result<_>>()
        .expect("events");
    assert_eq!(events.len(), 2);
    for (source_invocation, capture, bytes) in events {
        assert_eq!(source_invocation, invocation);
        assert_eq!(capture, first[2..]);
        assert_eq!(bytes as usize, before.stdout.len());
    }
    source
        .execute("DELETE FROM recall", [])
        .expect("evict payload");
    let missing = command(dir.path())
        .args(["recall", &first])
        .output()
        .expect("expired");
    assert!(!missing.status.success());
    assert!(missing.stdout.is_empty());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("capture unavailable or expired"));
    let third = capture(dir.path(), "1");
    assert_ne!(first, third);
    let still_missing = command(dir.path())
        .args(["recall", &first])
        .output()
        .expect("old capture");
    assert!(!still_missing.status.success());
    assert!(still_missing.stdout.is_empty());
}
