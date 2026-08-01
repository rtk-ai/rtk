#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

fn run_script(body: &str) -> std::process::Output {
    run_named_script("custom-test-runner.sh", body)
}

fn run_named_script(name: &str, body: &str) -> std::process::Output {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join(name);
    std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).expect("write script");
    let mut permissions = std::fs::metadata(&script)
        .expect("script metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).expect("make script executable");

    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["test", script.to_str().expect("utf-8 script path")])
        .output()
        .expect("run rtk test")
}

#[test]
fn generic_runner_surfaces_failure_before_passing_tail() {
    let output = run_script(
        r#"
printf '%s\n' 'FAIL: test_login broke'
i=1
while [ "$i" -le 20 ]; do
  printf 'PASS: case_%s\n' "$i"
  i=$((i + 1))
done
exit 1
"#,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stdout.contains("FAIL: test_login broke"),
        "failure line must remain visible: {stdout:?}"
    );
    assert!(
        !stdout.contains("OUTPUT (last 5 lines):"),
        "a passing tail must not replace the failure: {stdout:?}"
    );
}

#[test]
fn generic_runner_with_zero_exit_does_not_report_failure_words() {
    let output = run_script(
        r#"
printf '%s\n' '12 passed, 0 failed'
exit 0
"#,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(0));
    assert!(
        !stdout.contains("[FAIL]"),
        "zero exit must take precedence over scary words: {stdout:?}"
    );
    assert!(
        !stdout.contains("SUMMARY:\n  12 passed, 0 failed"),
        "a passing summary must not be promoted as a failure: {stdout:?}"
    );
}

#[test]
fn generic_runner_nonzero_exit_overrides_success_looking_summary() {
    let output = run_named_script(
        "pytest-wrapper.sh",
        r#"
printf '%s\n' '12 passed, 0 failed'
printf '%s\n' 'process terminated by sentinel 417'
i=1
while [ "$i" -le 20 ]; do
  printf 'cleanup step %s\n' "$i"
  i=$((i + 1))
done
exit 9
"#,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(9));
    assert!(
        stdout.starts_with("[FAIL]"),
        "nonzero exit must be presented as a failure: {stdout:?}"
    );
    assert!(
        stdout.contains("process terminated by sentinel 417"),
        "nonzero exit must preserve diagnostics even after a success-looking line: {stdout:?}"
    );
    assert!(
        stdout.contains("cleanup step 20") && stdout.contains("lines omitted"),
        "failure excerpt must retain both the head and tail: {stdout:?}"
    );
}

#[test]
fn generic_runner_short_nonzero_success_summary_keeps_failure_banner() {
    let output = run_named_script(
        "pytest-wrapper.sh",
        r#"
printf '%s\n' '12 passed, 0 failed'
exit 9
"#,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(9));
    assert!(
        stdout.starts_with("[FAIL]"),
        "never-worse must not replace authoritative failure output: {stdout:?}"
    );
}

#[test]
fn generic_runner_empty_nonzero_output_is_not_silent() {
    let output = run_named_script("pytest-wrapper.sh", "exit 9");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(9));
    assert!(
        stdout.contains("[FAIL] Command failed (exit code: 9)"),
        "empty nonzero output must produce an actionable failure: {stdout:?}"
    );
}

#[test]
fn generic_runner_detected_error_keeps_adjacent_causal_context() {
    let output = run_script(
        r#"
for i in 1 2 3 4 5 6 7 8 9 10; do printf 'setup step %s\n' "$i"; done
printf '%s\n' 'error: wrapper failed'
printf '\n\n'
printf '%s\n' 'process terminated by sentinel MIDDLE-417'
i=1
while [ "$i" -le 20 ]; do
  printf 'cleanup step %s\n' "$i"
  i=$((i + 1))
done
exit 9
"#,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(9));
    assert!(stdout.starts_with("[FAIL]"));
    assert!(
        stdout.contains("process terminated by sentinel MIDDLE-417"),
        "detected error must retain adjacent causal context: {stdout:?}"
    );
}

#[test]
fn generic_runner_recognized_failure_keeps_nearby_diagnostic_context() {
    let output = run_named_script(
        "pytest-wrapper.sh",
        r#"
for i in 1 2 3 4 5 6 7 8 9 10; do printf 'setup step %s\n' "$i"; done
printf '%s\n' 'FAILED test_middle - assertion mismatch'
printf '\n\n'
printf '%s\n' 'diagnostic sentinel MIDDLE-417'
i=1
while [ "$i" -le 20 ]; do
  printf 'cleanup step %s\n' "$i"
  i=$((i + 1))
done
printf '%s\n' '================ 1 failed in 0.1s ================'
exit 9
"#,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(9));
    assert!(
        stdout.contains("FAILED test_middle"),
        "nonzero recognized failure must remain visible: {stdout:?}"
    );
    assert!(
        stdout.contains("diagnostic sentinel MIDDLE-417"),
        "recognized failure must retain nearby diagnostic context: {stdout:?}"
    );
}

#[test]
fn generic_runner_duplicate_failure_anchors_remain_bounded() {
    let output = run_named_script(
        "pytest-wrapper.sh",
        r#"
i=1
while [ "$i" -le 200 ]; do
  printf '%s\n' 'FAILED duplicate_test - assertion mismatch'
  printf 'diagnostic %s\n' "$i"
  i=$((i + 1))
done
exit 9
"#,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(9));
    assert!(stdout.contains("FAILED duplicate_test"));
    assert!(
        stdout.lines().count() < 100,
        "duplicate anchors must not force raw-output fallback: {} lines",
        stdout.lines().count()
    );
}

#[test]
fn generic_runner_with_zero_exit_never_emits_failure_banner() {
    let output = run_named_script(
        "not-pytest-check.sh",
        r#"
printf '%s\n' 'FAILED but recovered'
i=1
while [ "$i" -le 20 ]; do
  printf 'recovery step %s\n' "$i"
  i=$((i + 1))
done
exit 0
"#,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(0));
    assert!(
        !stdout.contains("[FAIL]"),
        "zero-exit output must never receive a failure banner: {stdout:?}"
    );
}

#[test]
fn generic_runner_without_error_keywords_keeps_diagnostics_and_status() {
    let output = run_script(
        r#"
printf '%s\n' 'setup aborted by sentinel 417'
printf '%s\n' 'diagnostic payload'
exit 7
"#,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(7));
    assert!(
        stdout.contains("setup aborted by sentinel 417") && stdout.contains("diagnostic payload"),
        "unclassified failure diagnostics must remain visible: {stdout:?}"
    );
}
