#![cfg(unix)]
//! `rtk init` replays the recall store's migration steps, so a table dropped
//! behind a stamped store comes back, and never creates a store that is not
//! there.

mod common;

use rusqlite::Connection;
use std::path::Path;
use std::process::Output;

fn init(project: &Path, store: &Path) -> Output {
    common::rtk_command()
        .env("LC_ALL", "C")
        .env("HOME", project.join("home"))
        .env("RTK_RECALL_DB", store)
        .args(["init", "--global", "--no-patch"])
        .current_dir(project)
        .output()
        .expect("spawn rtk")
}

#[test]
fn init_restores_a_dropped_recall_table() {
    let project = common::tempdir();
    let store = project.path().join("recall.db");
    // Initialize the current schema rather than hard-coding its last stamp.
    Connection::open(&store).expect("create existing empty store");
    let initialized = init(project.path(), &store);
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    Connection::open(&store)
        .expect("open should succeed")
        .execute("DROP TABLE tee_reads", [])
        .expect("drop tee_reads");

    let output = init(project.path(), &store);
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let conn = Connection::open(&store).expect("open should succeed");
    let restored: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'tee_reads')",
            [],
            |row| row.get(0),
        )
        .expect("sqlite_master should be readable");
    assert!(restored, "init should recreate tee_reads");
}

#[test]
fn init_does_not_create_a_missing_recall_store() {
    let project = common::tempdir();
    let store = project.path().join("never_created.db");

    let output = init(project.path(), &store);
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!store.exists(), "init must not create the recall store");
}

#[test]
fn init_warns_and_carries_on_when_the_recall_store_is_not_the_recall_database() {
    let project = common::tempdir();
    let store = project.path().join("other_application.db");
    Connection::open(&store)
        .expect("open should succeed")
        .execute_batch(
            "PRAGMA application_id = 0x12345678;
             CREATE TABLE theirs (x INTEGER);
             INSERT INTO theirs VALUES (1);",
        )
        .expect("the file should be written");
    let before = std::fs::read(&store).expect("file should be readable");

    let output = init(project.path(), &store);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    let warnings = stderr
        .lines()
        .filter(|line| line.starts_with("  [warn] Failed to prepare recall database: "))
        .count();
    assert_eq!(warnings, 1, "stderr: {stderr}");
    assert_eq!(
        std::fs::read(&store).expect("file should be readable"),
        before,
        "another application's file is left as it was"
    );
}
