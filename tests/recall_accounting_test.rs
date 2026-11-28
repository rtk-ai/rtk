mod common;

use rusqlite::Connection;
use serde_json::Value;
use std::path::Path;
use std::process::Command;

fn command(dir: &Path, db: &Path) -> Command {
    let mut cmd = common::rtk_command();
    cmd.current_dir(dir)
        .env("RTK_DB_PATH", db.join("history.db"))
        .env("RTK_RECALL_DB", db.join("recall.db"))
        .env("CLAUDE_CONFIG_DIR", db.join("claude"));
    cmd
}

fn exported(dir: &Path, db: &Path, args: &[&str]) -> Value {
    let output = command(dir, db)
        .args(["gain", "--format", "json"])
        .args(args)
        .output()
        .expect("gain");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON export")
}

fn seed_recall(dir: &Path) {
    let conn = Connection::open(dir.join("recall.db")).expect("db");
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
}

#[test]
fn repeated_filtered_reads_count_actual_stdout_and_stay_unattributed() {
    let dir = common::tempdir();
    seed_recall(dir.path());
    let mut expected = 0;
    for args in [
        vec!["recall", "abc123"],
        vec!["recall", "abc123"],
        vec!["recall", "abc123", "--lines", "1"],
        vec!["recall", "abc123", "--full", "--grep", "second"],
        vec!["recall", "abc123", "--full", "--grep", "absent"],
    ] {
        let output = command(dir.path(), dir.path())
            .args(args)
            .output()
            .expect("recall");
        assert!(output.status.success());
        expected += output.stdout.len().div_ceil(4);
    }
    let export = exported(dir.path(), dir.path(), &["--history"]);
    let accounting = &export["output_accounting"];
    assert_eq!(accounting["recall_events"], 5);
    assert_eq!(accounting["unattributed_recall_events"], 5);
    assert_eq!(accounting["retrieved_output_tokens"], expected as i64);
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
fn project_export_scopes_commands_and_recalls_by_their_own_directories() {
    let db = common::tempdir();
    let project_a = common::temp_git_repo();
    let project_b = common::temp_git_repo();
    seed_recall(db.path());
    for project in [project_a.path(), project_b.path()] {
        assert!(
            command(project, db.path())
                .args(["git", "log", "--oneline", "-1"])
                .output()
                .expect("command")
                .status
                .success()
        );
    }
    for (project, count) in [(project_a.path(), "1"), (project_b.path(), "2")] {
        assert!(
            command(project, db.path())
                .args(["recall", "abc123", "--lines", count])
                .output()
                .expect("recall")
                .status
                .success()
        );
    }
    let a = exported(project_a.path(), db.path(), &["--project", "--history"]);
    let b = exported(project_b.path(), db.path(), &["--project", "--history"]);
    let all = exported(project_a.path(), db.path(), &["--history"]);
    for (export, bytes) in [(&a, 6), (&b, 13)] {
        let commands = export["commands"].as_array().expect("commands");
        assert_eq!(commands.len(), 1);
        assert!(commands[0]["invocation_id"].is_string());
        let events = export["recall_events"].as_array().expect("recall events");
        assert_eq!(events.len(), 1);
        assert!(events[0]["invoking_id"].is_string());
        assert_eq!(events[0]["emitted_bytes"], bytes);
        assert_eq!(export["output_accounting"]["recall_events"], 1);
        assert_eq!(
            export["output_accounting"]["retrieved_output_tokens"],
            (bytes as usize).div_ceil(4)
        );
        assert_eq!(
            export["output_accounting"]["initial_input_tokens"],
            export["summary"]["total_input"]
        );
    }
    assert_ne!(
        a["commands"][0]["invocation_id"],
        b["commands"][0]["invocation_id"]
    );
    assert_eq!(all["commands"].as_array().expect("all commands").len(), 2);
    assert_eq!(
        all["recall_events"]
            .as_array()
            .expect("all recall events")
            .len(),
        2
    );
    assert_eq!(
        all["output_accounting"]["initial_input_tokens"].as_i64(),
        Some(
            a["summary"]["total_input"]
                .as_i64()
                .expect("project A input")
                + b["summary"]["total_input"]
                    .as_i64()
                    .expect("project B input")
        )
    );
}

#[test]
fn project_export_includes_rows_recorded_in_subdirectories() {
    let db = common::tempdir();
    let project = common::temp_git_repo();
    let sub = project.path().join("sub");
    std::fs::create_dir(&sub).expect("subdirectory");
    seed_recall(db.path());
    assert!(
        command(&sub, db.path())
            .args(["git", "log", "--oneline", "-1"])
            .output()
            .expect("command")
            .status
            .success()
    );
    assert!(
        command(&sub, db.path())
            .args(["recall", "abc123", "--lines", "1"])
            .output()
            .expect("recall")
            .status
            .success()
    );
    let export = exported(project.path(), db.path(), &["--project", "--history"]);
    assert_eq!(export["commands"].as_array().expect("commands").len(), 1);
    assert_eq!(
        export["recall_events"]
            .as_array()
            .expect("recall events")
            .len(),
        1
    );
    let accounting = &export["output_accounting"];
    assert_eq!(accounting["recall_events"], 1);
    assert_eq!(accounting["retrieved_output_tokens"], 2);
    assert_eq!(
        accounting["commands_without_invocation_id"], 0,
        "rows rtk records itself carry an invocation ID"
    );
    // The project is the only one recorded, so scoping must keep every row.
    let unscoped = exported(project.path(), db.path(), &["--history"]);
    let scoped_input = accounting["initial_input_tokens"]
        .as_i64()
        .expect("scoped initial input");
    assert!(scoped_input > 0);
    assert_eq!(
        Some(scoped_input),
        unscoped["output_accounting"]["initial_input_tokens"].as_i64()
    );
}

#[test]
fn json_history_caps_pages_and_initial_accounting_matches_headline() {
    let dir = common::tempdir();
    let plain = exported(dir.path(), dir.path(), &[]);
    assert!(plain.get("output_accounting").is_none());
    assert!(plain.get("commands").is_none());
    assert!(plain.get("recall_events").is_none());
    let conn = Connection::open(dir.path().join("history.db")).expect("initialized tracking");
    conn.execute_batch(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i < 101)
         INSERT INTO commands (id, timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens, saved_tokens, savings_pct)
         SELECT i, '2026-11-20', 'cmd', 'rtk cmd', CASE WHEN i=101 THEN 7501 ELSE 7500 END,
                CASE WHEN i=101 THEN 200 ELSE 2 END, 7000, 80 FROM n ORDER BY i;
         WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i < 101)
         INSERT INTO recall_events (event_id, timestamp, emitted_bytes, output_tokens)
         SELECT CAST(i AS TEXT), '2026-11-20', 8, 2 FROM n ORDER BY i;"
    ).expect("seed both histories");
    drop(conn);
    let export = exported(dir.path(), dir.path(), &["--history"]);
    let commands = export["commands"].as_array().expect("commands");
    let recalls = export["recall_events"].as_array().expect("recalls");
    assert_eq!((commands.len(), recalls.len()), (100, 100));
    assert_eq!(commands[0]["id"], 101);
    assert_eq!(recalls[0]["event_id"], "101");
    let accounting = &export["output_accounting"];
    assert_eq!(accounting["initial_input_tokens"], 750_000);
    assert_eq!(
        accounting["initial_input_tokens"],
        export["summary"]["total_input"]
    );
    assert_eq!(accounting["initial_output_tokens"], 200);
    assert_eq!(
        accounting["initial_output_tokens"],
        export["summary"]["total_output"]
    );
    assert_eq!(accounting["initial_oversized_excluded"], 1);
    assert_eq!(
        accounting["initial_oversized_excluded"],
        export["summary"]["oversized_excluded"]
    );
    assert_eq!(accounting["recall_events"], 101);
    assert_eq!(accounting["retrieved_output_tokens"], 202);
    assert_eq!(
        accounting["commands_without_invocation_id"], 100,
        "only headline-eligible rows are counted, like the totals next to it"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn recall_to_full_device_returns_failure_without_recording_an_emission() {
    let dir = common::tempdir();
    seed_recall(dir.path());
    let stdout = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .expect("full device");
    let output = command(dir.path(), dir.path())
        .args(["recall", "abc123", "--full"])
        .stdout(std::process::Stdio::from(stdout))
        .output()
        .expect("recall");
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        exported(dir.path(), dir.path(), &["--history"])["output_accounting"]["recall_events"],
        0
    );
}
