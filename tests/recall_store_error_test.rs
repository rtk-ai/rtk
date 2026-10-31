//! `rtk recall` reports why the store cannot be used, with the cause of a
//! failed migration step and not only the name of the step.

mod common;

use rusqlite::Connection;

#[test]
fn recall_names_the_cause_of_a_failed_migration_step() {
    let dir = common::tempdir();
    let store = dir.path().join("recall.db");
    let conn = Connection::open(&store).expect("open should succeed");
    // A recall store stamped at its first version whose `recall` cannot take
    // the column the second step adds. 0x5254_4B02 is the recall kind.
    conn.execute_batch(
        "PRAGMA application_id = 0x52544B02;
         PRAGMA user_version = 1;
         CREATE VIEW recall AS SELECT 'abc' AS hash;",
    )
    .expect("store should be seeded");
    drop(conn);

    let output = common::rtk_command()
        .args(["recall", "--list"])
        .env("RTK_RECALL_DB", &store)
        .output()
        .expect("rtk should run");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("store unavailable") && stderr.contains("recall.recalled"),
        "stderr should carry the cause: {stderr}"
    );
}
