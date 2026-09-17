//! `gh pr checks` exits non-zero as a normal result: 1 when a check failed, 8 while
//! checks are still pending. Its stdout is complete and valid in both cases, so
//! skipping the filter there dropped the whole table on the user exactly when they
//! were asking why CI was red. These pin that the summary survives a non-zero exit
//! without swallowing the exit code, the diagnostics on stderr, or a check row.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

/// Writes an executable `name` in `dir` that runs `body`.
fn fake_tool(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{}\n", body)).expect("write fake tool");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod fake tool");
}

/// Runs rtk with `dir` first on PATH, so the fake tool shadows any real one.
fn rtk_with(dir: &Path, args: &[&str]) -> Output {
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(args)
        .env("PATH", path)
        .output()
        .expect("run rtk")
}

/// A `gh` whose `pr checks` prints `table` and exits `code`, the way it does when
/// checks failed (1) or are still running (8).
fn fake_gh(dir: &Path, table: &str, code: i32) {
    let path = dir.join("checks.txt");
    fs::write(&path, table).expect("write table");
    fake_tool(dir, "gh", &format!("cat {}\nexit {}", path.display(), code));
}

/// Rows as `gh` 2.46 prints them: tab-separated name, state, elapsed, url, description.
const RED_TABLE: &str = "doc review\tfail\t13s\thttps://example.test/job/1\t\n\
fmt\tfail\t7s\thttps://example.test/job/2\t\n\
Analyze (rust)\tpass\t4m17s\thttps://example.test/job/3\t\n\
check\tpass\t5s\thttps://example.test/job/4\t\n";

#[test]
fn failing_checks_are_summarised_not_dumped_raw() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_gh(dir.path(), RED_TABLE, 1);

    let out = rtk_with(dir.path(), &["gh", "pr", "checks", "123"]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("CI Checks Summary:"),
        "filter must run on a non-zero exit, got:\n{stdout}"
    );
    assert!(stdout.contains("Passed: 2"), "got:\n{stdout}");
    assert!(stdout.contains("Failed: 2"), "got:\n{stdout}");
    assert!(
        stdout.contains("doc review") && stdout.contains("fmt"),
        "the failed checks are the reason the user ran this, got:\n{stdout}"
    );
}

#[test]
fn a_red_run_still_propagates_the_exit_code() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_gh(dir.path(), RED_TABLE, 1);

    let out = rtk_with(dir.path(), &["gh", "pr", "checks", "123"]);
    assert_eq!(out.status.code(), Some(1), "gh's exit code must survive");
}

#[test]
fn pending_checks_are_counted_and_exit_eight_survives() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_gh(
        dir.path(),
        "test (ubuntu)\tpending\t0\thttps://example.test/job/5\t\n\
test (macos)\tpending\t0\thttps://example.test/job/6\t\n\
test (windows)\tpending\t0\thttps://example.test/job/7\t\n\
clippy\tpass\t31s\thttps://example.test/job/8\t\n",
        8,
    );

    let out = rtk_with(dir.path(), &["gh", "pr", "checks", "123"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Pending: 3"), "got:\n{stdout}");
    assert_eq!(out.status.code(), Some(8), "gh exits 8 while checks pend");
}

#[test]
fn a_cancelled_run_does_not_read_as_nothing_wrong() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_gh(
        dir.path(),
        "build\tcancelled\t45s\thttps://example.test/job/1\t\n\
test (ubuntu)\tcancelled\t44s\thttps://example.test/job/2\t\n\
Security Scan\tskipping\t0\thttps://example.test/job/3\t\n",
        1,
    );

    let out = rtk_with(dir.path(), &["gh", "pr", "checks", "123"]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("Skipped/cancelled: 3"),
        "a cancelled run must not summarise to all zeros, got:\n{stdout}"
    );
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn an_error_with_empty_stdout_does_not_become_an_all_zero_summary() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_tool(
        dir.path(),
        "gh",
        "echo 'GraphQL: Could not resolve to a Repository' >&2\nexit 1",
    );

    let out = rtk_with(dir.path(), &["gh", "pr", "checks", "123"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        !stdout.contains("Passed: 0"),
        "a failed lookup must not read as a PR with zero checks, got:\n{stdout}"
    );
    assert!(
        stderr.contains("Could not resolve to a Repository"),
        "the real diagnostic must reach the user, got:\n{stderr}"
    );
    assert_eq!(out.status.code(), Some(1));
}
