//! `pnpm outdated` exits 1 both when it found outdated packages and when it failed,
//! so `rtk pnpm outdated` has to pass that exit code on without mistaking a failure's
//! error text for a package list. These pin the exit code and what the user sees for
//! each of those runs.

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

/// What `pnpm outdated --format json` prints when two packages are behind.
const OUTDATED_JSON: &str = r#"{
  "left-pad": {"current": "1.0.0", "latest": "1.3.0", "wanted": "1.0.0", "isDeprecated": true, "dependencyType": "dependencies"},
  "is-odd": {"current": "2.0.0", "latest": "3.0.1", "wanted": "2.0.0", "isDeprecated": false, "dependencyType": "dependencies"}
}"#;

/// A `pnpm` that prints `json` on stdout, `stderr_text` on stderr and exits `code`.
fn fake_pnpm(dir: &Path, json: &str, stderr_text: &str, code: i32) {
    fs::write(dir.join("out.json"), json).expect("write stdout fixture");
    fs::write(dir.join("err.txt"), stderr_text).expect("write stderr fixture");
    fake_tool(
        dir,
        "pnpm",
        &format!(
            "cat {}\ncat {} >&2\nexit {}",
            dir.join("out.json").display(),
            dir.join("err.txt").display(),
            code
        ),
    );
}

#[test]
fn outdated_packages_keep_their_summary_and_the_exit_code() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_pnpm(dir.path(), OUTDATED_JSON, "", 1);

    let out = rtk_with(dir.path(), &["pnpm", "outdated"]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("2 outdated packages") && stdout.contains("is-odd: 2.0.0"),
        "outdated packages must still be summarised, got:\n{stdout}"
    );
    assert_eq!(out.status.code(), Some(1), "pnpm's exit 1 must survive");
}

/// Node prints its own deprecation warnings on stderr, whatever the command: they say
/// nothing about the outdated list, so they must not replace it.
#[test]
fn stderr_warnings_do_not_replace_the_outdated_summary() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_pnpm(
        dir.path(),
        OUTDATED_JSON,
        "(node:1234) [DEP0040] DeprecationWarning: The `punycode` module is deprecated.\n",
        1,
    );

    let out = rtk_with(dir.path(), &["pnpm", "outdated"]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("2 outdated packages"),
        "a stderr warning must not hide the outdated list, got:\n{stdout}"
    );
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn nothing_outdated_exits_zero() {
    let dir = tempfile::tempdir().expect("tempdir");
    // An up-to-date project makes pnpm print an empty map and exit 0.
    fake_pnpm(dir.path(), "{}", "", 0);

    let out = rtk_with(dir.path(), &["pnpm", "outdated"]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        !stdout.contains("outdated packages"),
        "nothing is outdated, got:\n{stdout}"
    );
    assert_eq!(out.status.code(), Some(0));
}

/// pnpm reports its own errors (`ERR_PNPM_*`) on stdout. That text has whitespace-
/// separated words like a package table, so parsing it produced an "outdated package"
/// named after the error code, with exit 0.
#[test]
fn a_pnpm_error_on_stdout_is_shown_as_is_and_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    let error = " ERR_PNPM_NO_IMPORTER_MANIFEST_FOUND  No package.json (or package.yaml, or package.json5) was found in \"/work\".\n";
    fake_pnpm(dir.path(), error, "", 1);

    let out = rtk_with(dir.path(), &["pnpm", "outdated"]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("ERR_PNPM_NO_IMPORTER_MANIFEST_FOUND  No package.json"),
        "the error must reach the user untouched, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("outdated packages"),
        "an error is not an outdated list, got:\n{stdout}"
    );
    assert_eq!(out.status.code(), Some(1), "a failed run must not exit 0");
}

/// An error with nothing on stdout (an older pnpm rejecting `--format`) is just as much
/// a failure and must not turn into "All packages up-to-date".
#[test]
fn a_pnpm_error_on_stderr_is_shown_and_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_pnpm(
        dir.path(),
        "",
        " ERROR  Unknown option: 'format'\nFor help, run: pnpm help outdated\n",
        1,
    );

    let out = rtk_with(dir.path(), &["pnpm", "outdated"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        stderr.contains("Unknown option: 'format'"),
        "the error must reach the user, got stderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("up-to-date"),
        "a failed run must not claim everything is up to date, got:\n{stdout}"
    );
    assert_eq!(out.status.code(), Some(1));
}
