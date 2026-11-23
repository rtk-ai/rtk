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
