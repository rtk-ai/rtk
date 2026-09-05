use rusqlite::Connection;
use std::process::Command;

#[test]
fn cargo_human_failure_uses_ai_contract_and_preserves_exit() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = temp.path().join("history.db");
    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["cargo", "check", "--quiet"])
        .current_dir(temp.path())
        .env("RTK_DB_PATH", &db)
        .env("RTK_TEE", "0")
        .output()
        .expect("run cargo check");

    assert_eq!(output.status.code(), Some(101));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("could not find `Cargo.toml`"),
        "stdout: {stdout}"
    );
    assert!(
        !stdout.contains("status=ok"),
        "failure became success: {stdout}"
    );

    let conn = Connection::open(&db).expect("tracking database");
    let contract: String = conn
        .query_row(
            "SELECT output_contract FROM commands ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("tracked output contract");
    assert_eq!(contract, "ai_owned");
}

#[test]
fn cargo_machine_help_stays_exact() {
    let temp = tempfile::tempdir().expect("tempdir");
    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["cargo", "check", "--message-format=json"])
        .current_dir(temp.path())
        .env("RTK_TEE", "0")
        .output()
        .expect("run cargo help");

    assert_eq!(output.status.code(), Some(101));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("status="),
        "machine output was rewritten: {stdout}"
    );
    assert!(
        !stderr.contains("status="),
        "machine output was rewritten: {stderr}"
    );
}
