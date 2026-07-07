use serde_json::Value;
use std::process::Command;

fn rtk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
}

fn gain_total_saved(db_path: &std::path::Path) -> u64 {
    let output = rtk()
        .env("RTK_DB_PATH", db_path)
        .args(["gain", "--format", "json"])
        .output()
        .expect("rtk gain");
    assert!(
        output.status.success(),
        "rtk gain failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout).expect("gain json");
    json["summary"]["total_saved"]
        .as_u64()
        .expect("summary.total_saved")
}

#[test]
fn read_tail_window_reports_no_savings_against_tail_baseline() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("tracking.db");
    let file = dir.path().join("large.log");
    let content: String = (0..12)
        .map(|i| format!("line-{i} {}\n", "x".repeat(120)))
        .collect();
    std::fs::write(&file, content).expect("write fixture");

    let output = rtk()
        .env("RTK_DB_PATH", &db_path)
        .args(["read", "--tail-lines", "2", file.to_str().unwrap()])
        .output()
        .expect("rtk read");
    assert!(
        output.status.success(),
        "rtk read failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("line-10 {}\nline-11 {}\n", "x".repeat(120), "x".repeat(120))
    );

    assert_eq!(gain_total_saved(&db_path), 0);
}

#[test]
fn read_max_window_reports_savings_against_head_baseline_when_shorter() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("tracking.db");
    let file = dir.path().join("large.log");
    let content: String = (0..12)
        .map(|i| format!("line-{i} {}\n", "x".repeat(120)))
        .collect();
    std::fs::write(&file, content).expect("write fixture");

    let output = rtk()
        .env("RTK_DB_PATH", &db_path)
        .args(["read", "--max-lines", "4", file.to_str().unwrap()])
        .output()
        .expect("rtk read");
    assert!(
        output.status.success(),
        "rtk read failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("more lines"));

    assert!(
        gain_total_saved(&db_path) > 0,
        "shortened --max-lines output should record real savings"
    );
}

#[test]
fn read_head_window_guard_falls_back_to_head_baseline() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("tracking.db");
    let file = dir.path().join("tiny.txt");
    std::fs::write(&file, "a\nb\n").expect("write fixture");

    let output = rtk()
        .env("RTK_DB_PATH", &db_path)
        .args(["read", "--max-lines", "1", file.to_str().unwrap()])
        .output()
        .expect("rtk read");
    assert!(
        output.status.success(),
        "rtk read failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "a\n");

    assert_eq!(gain_total_saved(&db_path), 0);
}

#[test]
fn uncapped_grep_reports_no_savings_against_faithful_baseline() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("tracking.db");
    let file = dir.path().join("sample.txt");
    std::fs::write(&file, format!("foo {}\n", "x".repeat(120))).expect("write fixture");

    let probe = Command::new("grep")
        .args(["--null", "-n", "-H", "foo", file.to_str().unwrap()])
        .output();
    if !probe.map(|o| o.status.success()).unwrap_or(false) {
        return;
    }

    let output = rtk()
        .env("RTK_DB_PATH", &db_path)
        .args(["grep", "-n", "foo", file.to_str().unwrap()])
        .output()
        .expect("rtk grep");
    assert!(
        output.status.success(),
        "rtk grep failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("1:foo {}\n", "x".repeat(120))
    );

    assert_eq!(gain_total_saved(&db_path), 0);
}

#[test]
fn capped_grep_reports_savings_against_faithful_baseline() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("tracking.db");
    let file = dir.path().join("sample.txt");
    let filler = "x".repeat(160);
    let content: String = (0..40)
        .map(|i| format!("foo line {i} {filler}\n"))
        .collect();
    std::fs::write(&file, content).expect("write fixture");

    let probe = Command::new("grep")
        .args(["--null", "-n", "-H", "foo", file.to_str().unwrap()])
        .output();
    if !probe.map(|o| o.status.success()).unwrap_or(false) {
        return;
    }

    let output = rtk()
        .env("RTK_DB_PATH", &db_path)
        .args(["grep", "--max", "5", "foo", file.to_str().unwrap()])
        .output()
        .expect("rtk grep");
    assert!(
        output.status.success(),
        "rtk grep failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let saved = gain_total_saved(&db_path);
    assert!(saved > 0, "capped grep should record real savings");
}
