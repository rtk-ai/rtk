//! A migration that cannot run does not take `rtk gain` down: the history is
//! shown from the database as it is, and stderr says it was not upgraded.

mod common;

use rusqlite::Connection;
use std::process::Output;

fn gain(tracking: &std::path::Path, args: &[&str]) -> Output {
    common::rtk_command()
        .arg("gain")
        .args(args)
        .env("RTK_DB_PATH", tracking)
        .env("RTK_RECALL_DB", tracking.with_file_name("recall.db"))
        .output()
        .expect("spawn rtk")
}

#[test]
fn gain_reports_a_failed_migration_and_shows_the_history() {
    let dir = common::tempdir();
    let tracking = stuck_database(dir.path(), ONE_COMMAND);

    let output = gain(&tracking, &["--format", "json"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains("rtk: tracking database was not upgraded: "),
        "{stderr}"
    );
    assert!(stdout.contains("\"total_commands\": 1"), "{stdout}");
    assert!(
        !stdout.contains("not upgraded"),
        "the report stays out of the data: {stdout}"
    );

    let stamp: i64 = Connection::open(&tracking)
        .expect("open should succeed")
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("the stamp should be readable");
    assert_eq!(stamp, 2, "the failed step is not stamped");
}

/// `rtk init` would fail the same step, so a write is not told to run it.
#[test]
fn a_write_to_a_table_a_failed_step_would_create_does_not_blame_corruption() {
    let dir = common::tempdir();
    let tracking = stuck_database(dir.path(), "DROP TABLE commands;");

    let output = common::rtk_command()
        .args(["ls", "."])
        .current_dir(dir.path())
        .env("RTK_DB_PATH", &tracking)
        .output()
        .expect("spawn rtk");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(!stderr.contains("looks corrupted"), "{stderr}");
}

/// A tracking database stamped by a release, whose next step cannot run because a
/// table holds the name of an index it creates, set up further by `sql`.
fn stuck_database(dir: &std::path::Path, sql: &str) -> std::path::PathBuf {
    let tracking = dir.join("history.db");
    assert!(gain(&tracking, &[]).status.success());
    Connection::open(&tracking)
        .expect("open should succeed")
        .execute_batch(&format!(
            "DROP INDEX idx_project_path_timestamp;
             CREATE TABLE idx_project_path_timestamp (x);
             {sql}
             PRAGMA user_version = 2;"
        ))
        .expect("the database should be set up");
    tracking
}

fn hook(dir: &std::path::Path, tracking: &std::path::Path) -> Output {
    use std::io::Write;
    use std::process::Stdio;

    let payload = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "git status" },
        "session_id": "s",
        "tool_use_id": "t",
    })
    .to_string();
    let mut child = common::rtk_command()
        .args(["hook", "claude"])
        .current_dir(dir)
        .env("RTK_DB_PATH", tracking)
        .env("RTK_RECALL_DB", tracking.with_file_name("recall.db"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk hook claude");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(payload.as_bytes())
        .expect("the payload should be written");
    child.wait_with_output().expect("rtk should finish")
}

/// A hook call must not print on every command what `rtk gain` already reports.
#[test]
fn the_hook_stays_quiet_when_a_failed_step_left_out_its_table() {
    let dir = common::tempdir();
    let tracking = stuck_database(dir.path(), "DROP TABLE hook_decisions;");

    let output = hook(dir.path(), &tracking);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(!stderr.contains("logging failed"), "{stderr}");
    assert!(!stderr.contains("looks corrupted"), "{stderr}");
}

/// Only what the failed step explains is quiet.
#[test]
fn the_hook_still_reports_another_logging_failure_on_such_a_database() {
    let dir = common::tempdir();
    let tracking = stuck_database(
        dir.path(),
        "CREATE TRIGGER refuse BEFORE INSERT ON hook_decisions
         BEGIN SELECT RAISE(ABORT, 'the insert is refused'); END;",
    );

    let output = hook(dir.path(), &tracking);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains("hook_decisions logging failed: the insert is refused"),
        "{stderr}"
    );
}

#[test]
fn every_gain_report_says_the_database_was_not_upgraded() {
    let dir = common::tempdir();
    let tracking = stuck_database(dir.path(), "");
    for args in [
        &[][..],
        &["--failures"],
        &["--history"],
        &["--format", "json"],
        &["--by-method"],
        &["--by-rule"],
        &["--shadow"],
        &["--pipeline", "x"],
    ] {
        let output = gain(&tracking, args);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{args:?}: {stderr}");
        assert_eq!(
            stderr.matches("tracking database was not upgraded").count(),
            1,
            "{args:?}: {stderr}"
        );
    }
}

#[test]
fn the_recall_report_says_the_store_was_not_upgraded() {
    let dir = common::tempdir();
    let tracking = dir.path().join("history.db");
    common::recall_store(
        &tracking.with_file_name("recall.db"),
        1,
        "CREATE VIEW recall AS SELECT 'abc' AS hash;
         CREATE TABLE recall_stats (slug TEXT, mode TEXT, elisions INTEGER, recalls INTEGER);",
    );

    let output = gain(&tracking, &["--recalls"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains("rtk: recall database was not upgraded: "),
        "{stderr}"
    );
}

fn count(tracking: &std::path::Path, table: &str) -> rusqlite::Result<i64> {
    Connection::open(tracking)
        .expect("open should succeed")
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
}

const ONE_COMMAND: &str = "INSERT INTO commands (timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens, saved_tokens, savings_pct)
     VALUES ('2026-01-01T00:00:00Z', 'git log', 'rtk git log', 100, 20, 80, 80.0);";

/// The step that still fails leaves out no table, so the reset can run.
#[test]
fn a_reset_clears_a_database_whose_step_still_fails() {
    let dir = common::tempdir();
    let tracking = stuck_database(dir.path(), ONE_COMMAND);

    let output = gain(&tracking, &["--reset", "--yes"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert_eq!(count(&tracking, "commands").ok(), Some(0));
    assert_eq!(
        stderr.matches("tracking database was not upgraded").count(),
        1,
        "{stderr}"
    );
}

/// A reset that cannot run says why, with the step's cause.
#[test]
fn a_reset_the_replay_cannot_help_says_why() {
    let dir = common::tempdir();
    let tracking = dir.path().join("history.db");
    assert!(gain(&tracking, &[]).status.success());
    // An index holding the name makes the step that creates the table fail.
    Connection::open(&tracking)
        .expect("open should succeed")
        .execute_batch(&format!(
            "{ONE_COMMAND}
             DROP TABLE engine_attribution;
             CREATE TABLE blocker (x INTEGER);
             CREATE INDEX engine_attribution ON blocker(x);
             PRAGMA user_version = 1;"
        ))
        .expect("the database should be set up");

    let output = gain(&tracking, &["--reset", "--yes"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr.contains("tracking database was not upgraded: "),
        "{stderr}"
    );
    assert_eq!(
        count(&tracking, "commands").ok(),
        Some(1),
        "nothing is lost"
    );
}

/// `rtk init` would fail the same step, so `rtk discover` does not suggest it.
#[test]
fn discover_does_not_suggest_rtk_init_for_a_failed_step() {
    let dir = common::tempdir();
    let tracking = stuck_database(dir.path(), "DROP TABLE hook_decisions;");
    let discover = |args: &[&str]| {
        common::rtk_command()
            .args(args)
            .arg("discover")
            .current_dir(dir.path())
            .env("RTK_DB_PATH", &tracking)
            .env("RTK_RECALL_DB", tracking.with_file_name("recall.db"))
            .output()
            .expect("spawn rtk")
    };

    let output = discover(&[]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(!stderr.contains("hook_decisions"), "{stderr}");

    let output = discover(&["-v"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("tracking database was not upgraded"),
        "{stderr}"
    );
    assert!(!stderr.contains("Run `rtk init`"), "{stderr}");
}

/// With nothing in the way of the step, the reset has nothing to report.
#[test]
fn a_reset_on_a_database_that_upgrades_reports_nothing() {
    let dir = common::tempdir();
    let tracking = stuck_database(dir.path(), "DROP TABLE idx_project_path_timestamp;");

    let output = gain(&tracking, &["--reset", "--yes"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(!stderr.contains("was not upgraded"), "{stderr}");
}

#[test]
fn a_reset_reports_a_recall_store_that_is_still_stuck() {
    let dir = common::tempdir();
    let tracking = dir.path().join("history.db");
    let store = tracking.with_file_name("recall.db");
    assert!(gain(&tracking, &[]).status.success());
    // Every table the reset clears is there, under a name that fails the step's index.
    Connection::open(&store)
        .expect("open should succeed")
        .execute_batch(
            "CREATE TABLE recall (hash TEXT PRIMARY KEY, command TEXT, created_at INTEGER, recalled INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE tee_reads (path TEXT PRIMARY KEY);
             CREATE TABLE recall_stats (slug TEXT, mode TEXT, elisions INTEGER, recalls INTEGER);
             CREATE TABLE idx_recall_created (x);",
        )
        .expect("the store should be set up");

    let output = gain(&tracking, &["--reset", "--yes"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stderr}");
    assert!(stdout.contains("recall stats reset to zero"), "{stdout}");
    assert!(
        stderr.contains("recall database was not upgraded: "),
        "{stderr}"
    );
}
