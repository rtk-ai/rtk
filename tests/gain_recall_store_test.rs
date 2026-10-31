//! `rtk gain` never fails because the recall store is unusable: the reset
//! still clears the tracking history, and the recall view says why it has
//! nothing to show.

mod common;

use rusqlite::Connection;
use std::path::Path;
use std::process::{Command, Output};

/// A tracking database path and a recall path that holds another
/// application's SQLite file, which the recall store refuses to open.
struct Scratch {
    _dir: tempfile::TempDir,
    tracking: std::path::PathBuf,
    recall: std::path::PathBuf,
}

fn scratch() -> Scratch {
    let dir = common::tempdir();
    let tracking = dir.path().join("history.db");
    let recall = dir.path().join("other_application.db");
    Connection::open(&recall)
        .expect("open should succeed")
        .pragma_update(None, "application_id", 0x1234_5678)
        .expect("the file should be tagged");
    Scratch {
        tracking,
        recall,
        _dir: dir,
    }
}

fn gain(scratch: &Scratch, args: &[&str]) -> Output {
    let mut cmd: Command = common::rtk_command();
    cmd.arg("gain")
        .args(args)
        .env("RTK_DB_PATH", &scratch.tracking)
        .env("RTK_RECALL_DB", &scratch.recall);
    cmd.output().expect("spawn rtk")
}

fn commands_recorded(tracking: &Path) -> i64 {
    Connection::open(tracking)
        .expect("open should succeed")
        .query_row("SELECT COUNT(*) FROM commands", [], |row| row.get(0))
        .expect("commands should be readable")
}

#[test]
fn reset_clears_the_tracking_history_when_the_recall_store_is_unusable() {
    let scratch = scratch();
    // The first run creates the tracking database; then it gets a row.
    assert!(gain(&scratch, &[]).status.success());
    Connection::open(&scratch.tracking)
        .expect("open should succeed")
        .execute(
            "INSERT INTO commands (timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens, saved_tokens, savings_pct)
             VALUES ('2026-01-01T00:00:00Z', 'git log', 'rtk git log', 100, 20, 80, 80.0)",
            [],
        )
        .expect("a row should be recorded");
    assert_eq!(commands_recorded(&scratch.tracking), 1);

    let output = gain(&scratch, &["--reset", "--yes"]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert!(
        stdout.contains("Token savings reset to zero."),
        "stdout: {stdout}"
    );
    assert!(
        !stdout.contains("recall stats reset"),
        "the recall part did not run: {stdout}"
    );
    let warnings = stderr
        .lines()
        .filter(|line| line.starts_with("rtk: recall stats were not reset: "))
        .count();
    assert_eq!(warnings, 1, "stderr: {stderr}");
    assert_eq!(commands_recorded(&scratch.tracking), 0);
}

#[test]
fn the_recall_view_says_why_it_is_empty_instead_of_failing() {
    let scratch = scratch();

    let output = gain(&scratch, &["--recalls"]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout
            .lines()
            .any(|line| line.starts_with("Recall store unavailable: ")),
        "stdout: {stdout}"
    );
}
