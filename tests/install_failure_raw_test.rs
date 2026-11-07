//! A failed install is shown as the installer printed it. When a package manager
//! refuses (a resolution error, or a policy gate that blocks installs and says
//! why), the refusal is the only output the user needs, and the install filters
//! dropped, capped or moved it. These pin that each wrapper shows the failure
//! verbatim on the stream it was written to, keeps the exit code, and still
//! filters a successful run.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;

mod common;

const REFUSAL: &str =
    "install-gate: installs are blocked in this workspace; ask an admin to allow it";

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
    common::rtk_command()
        .args(args)
        .env("PATH", path)
        .output()
        .expect("run rtk")
}

/// A tool that prints the refusal on stdout or stderr and exits 1.
fn refusing_tool(dir: &Path, name: &str, on_stderr: bool) {
    let redirect = if on_stderr { " >&2" } else { "" };
    fake_tool(
        dir,
        name,
        &format!("echo '{}'{}\nexit 1", REFUSAL, redirect),
    );
}

fn assert_refusal_verbatim(out: &Output, on_stderr: bool) {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let (shown, other) = if on_stderr {
        (&stderr, &stdout)
    } else {
        (&stdout, &stderr)
    };
    assert_eq!(
        shown.trim_end(),
        REFUSAL,
        "the refusal must reach the user unchanged, on the stream it was written to\n\
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !other.contains(REFUSAL),
        "the refusal must not move to the other stream\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(out.status.code(), Some(1), "the exit code must survive");
}

#[test]
fn cargo_install_refusal_on_stdout_is_shown_verbatim() {
    let dir = tempfile::tempdir().expect("tempdir");
    refusing_tool(dir.path(), "cargo", false);
    let out = rtk_with(dir.path(), &["cargo", "install", "ripgrep"]);
    assert_refusal_verbatim(&out, false);
}

#[test]
fn cargo_install_refusal_on_stderr_is_shown_verbatim() {
    let dir = tempfile::tempdir().expect("tempdir");
    refusing_tool(dir.path(), "cargo", true);
    let out = rtk_with(dir.path(), &["cargo", "install", "ripgrep"]);
    assert_refusal_verbatim(&out, true);
}

#[test]
fn bun_install_refusal_on_stdout_is_shown_verbatim() {
    let dir = tempfile::tempdir().expect("tempdir");
    refusing_tool(dir.path(), "bun", false);
    let out = rtk_with(dir.path(), &["bun", "install"]);
    assert_refusal_verbatim(&out, false);
}

#[test]
fn bun_add_refusal_on_stderr_is_shown_verbatim() {
    let dir = tempfile::tempdir().expect("tempdir");
    refusing_tool(dir.path(), "bun", true);
    let out = rtk_with(dir.path(), &["bun", "add", "zod"]);
    assert_refusal_verbatim(&out, true);
}

#[test]
fn composer_install_refusal_on_stdout_is_shown_verbatim() {
    let dir = tempfile::tempdir().expect("tempdir");
    refusing_tool(dir.path(), "composer", false);
    let out = rtk_with(dir.path(), &["composer", "install"]);
    assert_refusal_verbatim(&out, false);
}

#[test]
fn composer_install_refusal_on_stderr_is_shown_verbatim() {
    let dir = tempfile::tempdir().expect("tempdir");
    refusing_tool(dir.path(), "composer", true);
    let out = rtk_with(dir.path(), &["composer", "install"]);
    assert_refusal_verbatim(&out, true);
}

/// The composer filter keeps 30 lines; a failure longer than that is shown whole.
#[test]
fn a_long_composer_failure_is_not_capped() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_tool(
        dir.path(),
        "composer",
        "i=1\nwhile [ $i -le 40 ]; do echo \"Problem $i: package-$i requires php ^9\"; i=$((i+1)); done\nexit 2",
    );

    let out = rtk_with(dir.path(), &["composer", "update"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Problem 1:") && stdout.contains("Problem 40:"),
        "every problem composer reported must be shown, got:\n{stdout}"
    );
    assert_eq!(out.status.code(), Some(2));
}

/// The mise filter keeps 50 lines; a failure longer than that is shown whole.
#[test]
fn a_long_mise_install_failure_is_not_capped() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_tool(
        dir.path(),
        "mise",
        "i=1\nwhile [ $i -le 60 ]; do echo \"mise ERROR $i: python@3.$i is not available\"; i=$((i+1)); done\nexit 1",
    );

    let out = rtk_with(dir.path(), &["mise", "install", "python"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("mise ERROR 1:") && stdout.contains("mise ERROR 60:"),
        "every line mise reported must be shown, got:\n{stdout}"
    );
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn a_successful_composer_install_is_still_filtered() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_tool(
        dir.path(),
        "composer",
        "echo 'Loading composer repositories with package information'\n\
         echo 'Nothing to install, update or remove'",
    );

    let out = rtk_with(dir.path(), &["composer", "install"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The filter's summary, followed by the recall hint for the full output.
    assert!(
        stdout.starts_with("ok (up to date)\n"),
        "a successful install keeps its filter, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("Loading composer"),
        "a successful install keeps its filter, got:\n{stdout}"
    );
    assert_eq!(out.status.code(), Some(0));
}

/// Only installs show their failure raw: a failed `mise run` task is still filtered.
#[test]
fn a_failed_mise_run_is_still_filtered() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_tool(
        dir.path(),
        "mise",
        "echo 'mise Installing node@20.0.0'\necho 'Error: lint failed'\nexit 1",
    );

    let out = rtk_with(dir.path(), &["mise", "run", "lint"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("mise Installing"),
        "a task run keeps its filter, got:\n{stdout}"
    );
    assert!(stdout.contains("Error: lint failed"), "got:\n{stdout}");
    assert_eq!(out.status.code(), Some(1));
}
