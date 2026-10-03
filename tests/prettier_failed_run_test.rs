//! Prettier exits non-zero when a check finds unformatted files (1) and when it
//! cannot run the check at all (2: a syntax error, no matching files). Its findings
//! go to stderr, so the filter often has no file list to go on. Whatever the filter
//! makes of that, a run that failed must never be summarised as formatted. The
//! fixtures are what prettier 3.x prints, split the way it splits them.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

/// A `prettier` that prints `stdout_text` on stdout, `stderr_text` on stderr and exits
/// `code`.
fn fake_prettier(dir: &Path, stdout_text: &str, stderr_text: &str, code: i32) {
    fs::write(dir.join("out.txt"), stdout_text).expect("write stdout fixture");
    fs::write(dir.join("err.txt"), stderr_text).expect("write stderr fixture");
    let path = dir.join("prettier");
    fs::write(
        &path,
        format!(
            "#!/bin/sh\ncat '{}'\ncat '{}' >&2\nexit {}\n",
            dir.join("out.txt").display(),
            dir.join("err.txt").display(),
            code
        ),
    )
    .expect("write fake prettier");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod fake prettier");
}

/// Runs rtk with `dir` first on PATH, so the fake prettier shadows any real one.
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

const CHECKING: &str = "Checking formatting...\n";
const SYNTAX_ERROR_STDOUT: &str =
    "Checking formatting...\nError occurred when checking code style in the above file.\n";
const SYNTAX_ERROR_STDERR: &str = "[error] src/broken.js: SyntaxError: Unexpected token (1:11)\n\
     [error] > 1 | const z = ;\n";
const UNFORMATTED_STDERR: &str = "[warn] src/bad.js\n\
     [warn] Code style issues found in the above file. Run Prettier with --write to fix.\n";

#[test]
fn a_syntax_error_is_not_reported_as_formatted() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_prettier(dir.path(), SYNTAX_ERROR_STDOUT, SYNTAX_ERROR_STDERR, 2);

    let out = rtk_with(dir.path(), &["prettier", "--check", "src/broken.js"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        !stdout.contains("formatted correctly"),
        "prettier could not check the file, stdout must not say it is formatted: {stdout}"
    );
    assert!(
        stdout.contains("Error occurred when checking code style"),
        "what prettier printed on stdout must survive, got: {stdout}"
    );
    assert!(
        stderr.contains("SyntaxError"),
        "prettier's own error must reach the user, got: {stderr}"
    );
    assert_eq!(out.status.code(), Some(2), "exit code must propagate");
}

/// `rtk format` hands the filter stdout and stderr together, so it sees the
/// `[warn] <file>` lines that a failing `--check` writes.
#[test]
fn format_lists_the_files_prettier_warned_about() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_prettier(dir.path(), CHECKING, UNFORMATTED_STDERR, 1);

    let out = rtk_with(dir.path(), &["format", "prettier", "--check", "src/bad.js"]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        !stdout.contains("formatted correctly"),
        "a failing check must not be reported as formatted, got: {stdout}"
    );
    assert!(
        stdout.contains("1 files need formatting") && stdout.contains("src/bad.js"),
        "the file to fix must be listed, got: {stdout}"
    );
    assert_eq!(out.status.code(), Some(1), "exit code must propagate");
}

#[test]
fn format_does_not_report_a_syntax_error_as_formatted() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_prettier(dir.path(), SYNTAX_ERROR_STDOUT, SYNTAX_ERROR_STDERR, 2);

    let out = rtk_with(
        dir.path(),
        &["format", "prettier", "--check", "src/broken.js"],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        !stdout.contains("formatted correctly") && stdout.contains("SyntaxError"),
        "the error must be shown, not a success line, got: {stdout}"
    );
    assert_eq!(out.status.code(), Some(2), "exit code must propagate");
}

#[test]
fn a_clean_check_is_still_summarised() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_prettier(
        dir.path(),
        "Checking formatting...\nAll matched files use Prettier code style!\n",
        "",
        0,
    );

    let out = rtk_with(dir.path(), &["prettier", "--check", "src/good.js"]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("All files formatted correctly"),
        "a passing check keeps its one-line summary, got: {stdout}"
    );
    assert_eq!(out.status.code(), Some(0));
}
