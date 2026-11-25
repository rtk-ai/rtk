//! Missing tables in an otherwise current database need the `rtk init` recovery hint.

mod common;

use rusqlite::Connection;
use std::path::Path;
use std::process::Output;

fn gain(database: &Path, args: &[&str]) -> Output {
    common::rtk_command()
        .arg("gain")
        .args(args)
        .env("RTK_DB_PATH", database)
        .env("RTK_RECALL_DB", database.with_file_name("recall.db"))
        .output()
        .expect("run rtk gain")
}

#[test]
fn gain_explains_how_to_repair_a_missing_commands_table() {
    let dir = common::tempdir();
    let database = dir.path().join("history.db");
    let initialized = gain(&database, &[]);
    assert!(initialized.status.success(), "{initialized:?}");
    Connection::open(&database)
        .expect("open the initialized database")
        .execute_batch("DROP TABLE commands;")
        .expect("drop the table without changing the schema version");

    for args in [
        &[][..],
        &["--history"],
        &["--daily"],
        &["--weekly"],
        &["--monthly"],
        &["--format", "json"],
        &["--format", "csv", "--daily"],
        &["--format", "csv", "--weekly"],
        &["--format", "csv", "--monthly"],
    ] {
        let output = gain(&database, args);

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        assert!(
            stderr.contains("no such table: commands"),
            "{args:?}: {stderr}"
        );
        assert!(
            stderr.contains("tracking database looks corrupted"),
            "{args:?}: {stderr}"
        );
        assert_eq!(
            stderr.matches("Run `rtk init`").count(),
            1,
            "{args:?}: {stderr}"
        );
    }
}

#[test]
fn gain_explains_how_to_repair_missing_parse_failure_data() {
    let dir = common::tempdir();
    let database = dir.path().join("history.db");
    assert!(gain(&database, &[]).status.success());
    Connection::open(&database)
        .expect("open the initialized database")
        .execute_batch("DROP TABLE parse_failures;")
        .expect("drop the table without changing the schema version");

    let output = gain(&database, &["--failures"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stderr.contains("no such table: parse_failures"), "{stderr}");
    assert!(stderr.contains("Run `rtk init`"), "{stderr}");
}

#[test]
fn gain_keeps_other_schema_errors_without_a_table_repair_hint() {
    let dir = common::tempdir();
    let database = dir.path().join("history.db");
    assert!(gain(&database, &[]).status.success());
    Connection::open(&database)
        .expect("open the initialized database")
        .execute_batch("ALTER TABLE commands RENAME COLUMN input_tokens TO old_input_tokens;")
        .expect("rename a column without changing the schema version");

    let output = gain(&database, &[]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stderr.contains("no such column: input_tokens"), "{stderr}");
    assert!(!stderr.contains("looks corrupted"), "{stderr}");
    assert!(!stderr.contains("Run `rtk init`"), "{stderr}");
}

#[test]
fn gain_reads_a_healthy_database_without_a_repair_hint() {
    let dir = common::tempdir();
    let output = gain(&dir.path().join("history.db"), &["--format", "json"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{output:?}");
    assert!(!stderr.contains("looks corrupted"), "{stderr}");
    serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .expect("a healthy database still produces a JSON report");
}

#[test]
fn discover_keeps_its_existing_missing_table_warnings() {
    let dir = common::tempdir();
    let database = dir.path().join("history.db");
    assert!(gain(&database, &[]).status.success());
    Connection::open(&database)
        .expect("open the initialized database")
        .execute_batch("DROP TABLE hook_decisions;")
        .expect("drop the table without changing the schema version");

    let output = common::rtk_command()
        .arg("discover")
        .env("RTK_DB_PATH", &database)
        .env("RTK_RECALL_DB", database.with_file_name("recall.db"))
        .output()
        .expect("run rtk discover");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stderr
            .matches("failed to read hook_decisions log (")
            .count(),
        1,
        "{stderr}"
    );
    assert_eq!(
        stderr
            .matches("failed to read hook_decisions log timestamp")
            .count(),
        1,
        "{stderr}"
    );
    assert_eq!(stderr.matches("Run `rtk init`").count(), 1, "{stderr}");
}

#[test]
fn gain_does_not_label_a_missing_recall_table_as_tracking_corruption() {
    let dir = common::tempdir();
    let database = dir.path().join("history.db");
    let recall = database.with_file_name("recall.db");
    common::recall_store(&recall, 0, "");
    let initialized = gain(&database, &["--recalls"]);
    assert!(initialized.status.success(), "{initialized:?}");
    Connection::open(&recall)
        .expect("open the initialized recall store")
        .execute_batch("DROP TABLE recall_stats;")
        .expect("drop the table without changing the schema version");

    let output = gain(&database, &["--recalls"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        stderr.contains("recall database looks corrupted"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("tracking database looks corrupted"),
        "{stderr}"
    );
    assert_eq!(stderr.matches("Run `rtk init`").count(), 1, "{stderr}");
}

#[test]
fn gain_explains_missing_tracking_data_in_json_history() {
    let dir = common::tempdir();
    let database = dir.path().join("history.db");
    assert!(gain(&database, &[]).status.success());
    Connection::open(&database)
        .expect("open the initialized database")
        .execute_batch("DROP TABLE recall_events;")
        .expect("drop tracking events without changing the schema version");

    let output = gain(&database, &["--format", "json", "--history"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stderr.contains("no such table: recall_events"), "{stderr}");
    assert!(
        stderr.contains("tracking database looks corrupted"),
        "{stderr}"
    );
    assert_eq!(stderr.matches("Run `rtk init`").count(), 1, "{stderr}");
}

#[test]
fn gain_explains_missing_attribution_data_in_each_report() {
    let dir = common::tempdir();
    let database = dir.path().join("history.db");
    assert!(gain(&database, &[]).status.success());
    Connection::open(&database)
        .expect("open the initialized database")
        .execute_batch("DROP TABLE engine_attribution;")
        .expect("drop attribution without changing the schema version");

    for args in [
        &["--by-method"][..],
        &["--by-rule"],
        &["--by-layer"],
        &["--by-pipeline"],
        &["--shadow"],
        &["--pipeline", "example"],
        &["--by-method", "--format", "json"],
    ] {
        let output = gain(&database, args);

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        assert!(
            stderr.contains("no such table: engine_attribution"),
            "{args:?}: {stderr}"
        );
        assert!(
            stderr.contains("tracking database looks corrupted"),
            "{args:?}: {stderr}"
        );
        assert_eq!(
            stderr.matches("Run `rtk init`").count(),
            1,
            "{args:?}: {stderr}"
        );
    }
}

#[cfg(feature = "engine")]
#[test]
fn pipeline_card_keeps_optional_tracking_reads_quiet() {
    let dir = common::tempdir();
    let database = dir.path().join("history.db");
    assert!(gain(&database, &[]).status.success());
    Connection::open(&database)
        .expect("open the initialized database")
        .execute_batch("DROP TABLE engine_attribution;")
        .expect("drop optional card data without changing the schema version");

    let output = common::rtk_command()
        .args(["pipeline", "share", "--md"])
        .env("RTK_DB_PATH", &database)
        .env("RTK_RECALL_DB", database.with_file_name("recall.db"))
        .output()
        .expect("render a local pipeline card without publishing it");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("```toml"),
        "{output:?}"
    );
    assert!(!stderr.contains("looks corrupted"), "{stderr}");
    assert!(!stderr.contains("Run `rtk init`"), "{stderr}");
}
