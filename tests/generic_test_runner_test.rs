#![cfg(unix)]
//! `rtk test <script>` for a runner rtk has no parser for (#2420).
//!
//! The failure of a custom runner is often not among its last lines, and its output is
//! not in any format rtk recognises, so what reaches the user is decided by the generic
//! fallback alone. These run the binary against small scripts standing in for that runner.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

/// Writes an executable `name` in `dir` that runs `body`.
fn write_script(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write script");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod script");
}

/// `rtk test ./<name>` run from `dir`, with rtk's recovery hints off so the output is
/// exactly what the fallback chose.
fn rtk_test(dir: &Path, name: &str) -> Output {
    fs::create_dir_all(dir.join("home")).expect("create home");
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["test", &format!("./{name}")])
        .current_dir(dir)
        .env("HOME", dir.join("home"))
        .env("RTK_RECALL", "0")
        .output()
        .expect("run rtk")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The script from the issue: the failing case comes first, then fifty passing ones.
#[test]
fn a_failure_above_a_passing_tail_is_shown() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_script(
        dir.path(),
        "vt.sh",
        r#"echo "FAIL: test_login broke"
for i in $(seq 1 50); do echo "PASS: case_$i"; done
exit 1"#,
    );

    let out = rtk_test(dir.path(), "vt.sh");
    let shown = stdout(&out);

    assert_eq!(
        out.status.code(),
        Some(1),
        "the exit code must pass through"
    );
    assert!(shown.contains("FAIL: test_login broke"), "{shown}");
    assert!(
        !shown.contains("PASS: case_50"),
        "the passing tail is not the report: {shown}"
    );
}

#[test]
fn a_failure_with_no_failure_line_reports_the_exit_code_and_the_tail() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_script(
        dir.path(),
        "quiet.sh",
        r#"for i in $(seq 1 30); do echo "step $i done"; done
exit 3"#,
    );

    let out = rtk_test(dir.path(), "quiet.sh");
    let shown = stdout(&out);

    assert_eq!(out.status.code(), Some(3));
    assert!(
        shown.contains("[FAIL] Command failed (exit code: 3)"),
        "{shown}"
    );
    assert!(shown.contains("step 30 done"), "{shown}");
    assert!(!shown.contains("step 1 done"), "{shown}");
}

/// A run that exited 0 is not a failure because its output says "failed" or "warning".
#[test]
fn a_green_run_is_never_reported_as_failed() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_script(
        dir.path(),
        "green.sh",
        r#"echo "warning: flag --old is deprecated"
for i in $(seq 1 30); do echo "case $i ok"; done
echo "Summary: 0 failed, 30 passed""#,
    );

    let out = rtk_test(dir.path(), "green.sh");
    let shown = stdout(&out);

    assert_eq!(out.status.code(), Some(0));
    assert!(!shown.contains("[FAIL]"), "{shown}");
    assert!(shown.contains("Summary: 0 failed, 30 passed"), "{shown}");
}
