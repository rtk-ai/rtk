use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn read_stdin_falls_back_to_raw_when_filter_removes_all_content() {
    let input = "// only comment\n";
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["read", "-", "-l", "minimal"])
        .env("RTK_DB_PATH", temp_dir.path().join("tracking.db"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk");

    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");

    let output = child.wait_with_output().expect("wait for rtk");

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), input);
    assert!(String::from_utf8_lossy(&output.stderr).contains("showing raw content"));
}
