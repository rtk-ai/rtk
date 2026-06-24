use rusqlite::Connection;
use std::fs;
use std::process::Command;

#[test]
fn read_piped_to_head_does_not_panic_or_record_partial_gain() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let file = tmp.path().join("large.txt");
    let db = tmp.path().join("history.db");
    fs::write(&file, "abcdef0123456789\n".repeat(20_000)).expect("write test file");

    let output = Command::new("sh")
        .arg("-c")
        .arg("\"$RTK_BIN\" read \"$RTK_FILE\" | head -c 10 >/dev/null")
        .env("RTK_BIN", env!("CARGO_BIN_EXE_rtk"))
        .env("RTK_FILE", &file)
        .env("RTK_DB_PATH", &db)
        .env("RTK_TELEMETRY_DISABLED", "1")
        .output()
        .expect("run piped rtk read");

    assert!(
        output.status.success(),
        "pipeline should succeed: status={:?}, stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked") && !stderr.contains("failed printing to stdout"),
        "broken pipe should be handled without panic, got stderr: {stderr}"
    );

    if db.exists() {
        let conn = Connection::open(&db).expect("open history db");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM commands", [], |row| row.get(0))
            .expect("count commands");
        assert_eq!(count, 0, "partial read output must not be counted as gain");
    }
}
