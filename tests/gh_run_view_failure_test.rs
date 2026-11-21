//! `gh run view --exit-status` exits 1 for a failed run while printing the same
//! complete summary as without the flag. Skipping the filter on that exit passed
//! the red run through raw, which is the case callers most need compressed. These
//! pin that the summary survives the non-zero exit without swallowing the exit
//! code or a real error's diagnostics.

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

/// A `gh` whose `run view` prints `summary` and exits `code`.
fn fake_gh(dir: &Path, summary: &str, code: i32) {
    let path = dir.join("run.txt");
    fs::write(&path, summary).expect("write summary");
    fake_tool(dir, "gh", &format!("cat {}\nexit {}", path.display(), code));
}

const FAILED_RUN: &str = "\nX main CI · 34681475906\nTriggered via push about 1 hour ago\n\n\
JOBS\n\
✓ lint in 32s (ID 1)\n\
X test in 1m4s (ID 2)\n\
  ✓ Set up job\n\
  X Run cargo test\n\n\
ANNOTATIONS\n\
X Process completed with exit code 101.\n\
test: .github#12\n";

#[test]
fn a_failed_run_with_exit_status_is_filtered_not_passed_through() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_gh(dir.path(), FAILED_RUN, 1);

    let out = rtk_with(
        dir.path(),
        &["gh", "run", "view", "34681475906", "--exit-status"],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("Workflow Run #34681475906"),
        "filter must run on a non-zero exit, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("Triggered via push"),
        "raw output must not be passed through, got:\n{stdout}"
    );
}

#[test]
fn the_exit_status_still_propagates() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_gh(dir.path(), FAILED_RUN, 1);

    let out = rtk_with(
        dir.path(),
        &["gh", "run", "view", "34681475906", "--exit-status"],
    );
    assert_eq!(out.status.code(), Some(1), "gh's exit code must survive");
}

#[test]
fn an_error_with_empty_stdout_keeps_its_diagnostic() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_tool(
        dir.path(),
        "gh",
        "echo 'could not find any workflow run with ID: 1' >&2\nexit 1",
    );

    let out = rtk_with(dir.path(), &["gh", "run", "view", "1", "--exit-status"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        !stdout.contains("Workflow Run #1"),
        "a failed lookup must not invent a summary, got:\n{stdout}"
    );
    assert!(
        stderr.contains("could not find any workflow run"),
        "the real diagnostic must reach the user, got:\n{stderr}"
    );
    assert_eq!(out.status.code(), Some(1));
}
