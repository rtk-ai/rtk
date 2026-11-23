//! A captured prefix must not be presented as a complete command report.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Output;

mod common;

const RAW_CAP: usize = 10_485_760;
const NOTICE: &str = "summary and recall are incomplete";

fn run_fixture(tool: &str, args: &[&str], stdout: &str, stderr: &str, code: i32) -> Output {
    let dir = common::tempdir();
    let stdout_path = dir.path().join("stdout");
    let stderr_path = dir.path().join("stderr");
    fs::write(&stdout_path, stdout).expect("write stdout");
    fs::write(&stderr_path, stderr).expect("write stderr");
    let executable = dir.path().join(tool);
    fs::write(
        &executable,
        format!(
            "#!/bin/sh\ncat \"$RTK_CAPTURE_STDOUT\"\ncat \"$RTK_CAPTURE_STDERR\" >&2\nexit {code}\n"
        ),
    )
    .expect("write fixture tool");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("chmod tool");

    common::rtk_command()
        .args(args)
        .env("PATH", format!("{}:/usr/bin:/bin", dir.path().display()))
        .env("RTK_CAPTURE_STDOUT", stdout_path)
        .env("RTK_CAPTURE_STDERR", stderr_path)
        .output()
        .expect("run rtk")
}

#[test]
fn truncated_go_test_report_is_explicitly_incomplete() {
    let mut report = String::from(
        "{\"Action\":\"pass\",\"Package\":\"example.com/proj\",\"Test\":\"TestVisible\"}\n",
    );
    let progress = format!(
        "{{\"Action\":\"output\",\"Package\":\"example.com/proj\",\"Output\":\"{}\"}}\n",
        "x".repeat(4096)
    );
    while report.len() <= RAW_CAP {
        report.push_str(&progress);
    }
    report.push_str(&format!(
        "{{\"Action\":\"fail\",\"Package\":\"example.com/proj\",\"Test\":\"TestHidden\",\"Padding\":\"{}\"}}\n",
        "x".repeat(4096)
    ));
    let output = run_fixture("go", &["go", "test"], &report, "", 1);
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 stdout");

    assert_eq!(output.status.code(), Some(1));
    assert!(stdout.contains("Go test: 1 passed"), "{stdout}");
    assert!(!stdout.contains("TestHidden"), "{stdout}");
    assert!(stdout.contains(NOTICE), "{stdout}");
    assert!(stdout.len() < report.len() / 5);
}

#[test]
fn complete_go_test_report_is_unchanged() {
    let report =
        "{\"Action\":\"pass\",\"Package\":\"example.com/proj\",\"Test\":\"TestVisible\"}\n";
    let output = run_fixture("go", &["go", "test"], report, "", 0);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"Go test: 1 passed in 1 packages\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn a_dropped_first_line_still_reports_partial_output() {
    let output = run_fixture("go", &["go", "test"], &"x".repeat(RAW_CAP + 1), "", 0);
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 stdout");

    assert_eq!(output.status.code(), Some(0));
    assert!(stdout.contains(NOTICE), "{stdout}");
    assert!(!stdout.contains("No tests found"), "{stdout}");
}

#[test]
fn merged_stderr_capture_is_also_qualified() {
    let stderr = "build progress\n".repeat(RAW_CAP / 15 + 2);
    let output = run_fixture("go", &["go", "build"], "", &stderr, 2);
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 stdout");

    assert_eq!(output.status.code(), Some(2));
    assert!(stdout.contains(NOTICE), "{stdout}");
}

#[test]
fn uv_run_qualifies_its_separate_capture_path() {
    let output = run_fixture(
        "uv",
        &["uv", "run", "example"],
        &"x".repeat(RAW_CAP + 1),
        "",
        0,
    );
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 stdout");

    assert_eq!(output.status.code(), Some(0));
    assert!(stdout.contains(NOTICE), "{stdout}");
    assert!(!stdout.contains("ok"), "{stdout}");
}

#[test]
fn streaming_cargo_test_qualifies_a_summary_reparsed_from_partial_raw() {
    let stderr = format!(
        "{}error: compile failure before capture cap\nCompiling {}\nerror: compile failure after capture cap\n",
        "   Compiling placeholder-package-with-a-long-name v0.0.0\n".repeat(20),
        "x".repeat(RAW_CAP + 1)
    );
    let output = run_fixture("cargo", &["cargo", "test"], "", &stderr, 1);
    let shown = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(shown.contains("cargo test: 1 errors"), "{shown}");
    assert!(shown.contains(NOTICE), "{shown}");
}

#[test]
fn streaming_tsc_qualifies_its_unparsed_failure_fallback() {
    let stderr = format!(
        "opaque first line\n{}\nopaque last line\n",
        "x".repeat(RAW_CAP + 1)
    );
    let output = run_fixture("tsc", &["tsc"], "", &stderr, 2);
    let shown = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(output.status.code(), Some(2));
    assert!(shown.contains("RTK parsed no diagnostics"), "{shown}");
    assert!(shown.contains(NOTICE), "{shown}");
}

#[test]
fn streaming_error_filter_qualifies_its_raw_tail_fallback() {
    let stderr = format!(
        "opaque first line\n{}\nopaque last line\n",
        "x".repeat(RAW_CAP + 1)
    );
    let output = run_fixture("fixture", &["err", "fixture"], "", &stderr, 9);
    let shown = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(output.status.code(), Some(9));
    assert!(shown.contains("Command failed (exit code: 9)"), "{shown}");
    assert!(shown.contains(NOTICE), "{shown}");
}

#[test]
fn streaming_tsc_counts_from_all_lines_remain_complete() {
    let stderr = format!(
        "error TS1234: first diagnostic\nFound {}\nerror TS5678: last diagnostic\n",
        "x".repeat(RAW_CAP + 1)
    );
    let output = run_fixture("tsc", &["tsc"], "", &stderr, 2);
    let shown = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(output.status.code(), Some(2));
    assert!(shown.contains("TypeScript: 2 errors"), "{shown}");
    assert!(shown.contains("last diagnostic"), "{shown}");
    assert!(!shown.contains(NOTICE), "{shown}");
}

#[test]
fn streaming_cargo_test_counts_from_all_lines_remain_complete() {
    let stderr = format!(
        "{}Compiling {}\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n",
        "   Compiling placeholder-package-with-a-long-name v0.0.0\n".repeat(20),
        "x".repeat(RAW_CAP + 1)
    );
    let output = run_fixture("cargo", &["cargo", "test"], "", &stderr, 0);
    let shown = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(output.status.code(), Some(0));
    assert!(shown.contains("1 passed"), "{shown}");
    assert!(!shown.contains(NOTICE), "{shown}");
}
