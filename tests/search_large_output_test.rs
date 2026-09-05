use std::fs;
use std::process::Command;

#[test]
fn rg_large_line_is_summarized_without_raw_fallback() {
    let temp = tempfile::tempdir().expect("tempdir");
    let file = temp.path().join("huge.log");
    let content = format!("needle {}\n", "x".repeat(70 * 1024));
    fs::write(&file, content).expect("fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["rg", "needle", file.to_str().expect("path")])
        .env("RTK_TEE", "0")
        .output()
        .expect("rtk rg");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("matches=1"), "stdout: {stdout}");
    assert!(stdout.contains("truncated_lines=1"), "stdout: {stdout}");
    assert!(stdout.contains("recovery=unavailable"), "stdout: {stdout}");
    assert!(
        stdout.len() < 4_096,
        "raw line was replayed: {} bytes",
        stdout.len()
    );
}

#[test]
fn rg_large_result_set_reports_total_and_omission() {
    let temp = tempfile::tempdir().expect("tempdir");
    let file = temp.path().join("many.log");
    let content = (1..=250)
        .map(|line| format!("needle result-{line} {}\n", "x".repeat(70 * 1024)))
        .collect::<String>();
    fs::write(&file, content).expect("fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["rg", "needle", file.to_str().expect("path")])
        .env("RTK_TEE", "0")
        .output()
        .expect("rtk rg");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("matches=250"), "stdout: {stdout}");
    assert!(stdout.contains("omitted items="), "stdout: {stdout}");
    assert!(
        !stdout.contains("result-250"),
        "uncapped result leaked: {stdout}"
    );
}
