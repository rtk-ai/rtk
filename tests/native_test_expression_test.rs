#![cfg(unix)]
//! End-to-end coverage for `Commands::Test` routing.
//!
//! `rtk test` doubles as a wrapper around a test runner and as the POSIX `test`
//! utility. The unit tests in `main.rs` cover `is_native_test_expression` on its
//! own; these run the binary so an inverted or removed branch in the `Commands::Test`
//! arm fails here instead of shipping.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn exit_code(cwd: &Path, args: &[&str]) -> i32 {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("LC_ALL", "C")
        .env("HOME", cwd.join("home"))
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn rtk")
        .status
        .code()
        .expect("rtk exited via signal")
}

fn fixture() -> TempDir {
    let dir = TempDir::new().expect("create tempdir");
    std::fs::create_dir(dir.path().join("home")).expect("create home");
    std::fs::create_dir(dir.path().join("adir")).expect("create adir");
    std::fs::write(dir.path().join("afile"), b"").expect("create afile");
    dir
}

/// A native expression must reach the system `test`, whose exit code is the answer.
/// Routing it through the shell wrapper instead yields `sh: 0: Illegal option -d`
/// and exit 2 for every one of these.
#[test]
fn native_expressions_carry_the_test_exit_code() {
    let dir = fixture();
    let cwd = dir.path();

    assert_eq!(exit_code(cwd, &["test", "-d", "adir"]), 0);
    assert_eq!(exit_code(cwd, &["test", "-d", "nodir"]), 1);
    assert_eq!(exit_code(cwd, &["test", "-f", "afile"]), 0);
    assert_eq!(exit_code(cwd, &["test", "-f", "nofile"]), 1);
    assert_eq!(exit_code(cwd, &["test", "-e", "adir"]), 0);
}

/// `!` negates, so these are the cases the shell wrapper answered backwards
/// rather than loudly: it reported "not a directory" as true for a directory
/// that exists, because `-d` was a missing command and `!` inverted its 127.
#[test]
fn negated_expressions_are_not_answered_backwards() {
    let dir = fixture();
    let cwd = dir.path();

    assert_eq!(exit_code(cwd, &["test", "!", "-d", "adir"]), 1);
    assert_eq!(exit_code(cwd, &["test", "!", "-d", "nodir"]), 0);
}

/// `!` and `(` are shell syntax as well as `test` syntax. They mark a native
/// expression only when what they apply to is one, so a command behind `!` still
/// runs under the shell and keeps the exit code negation it had before.
#[test]
fn bang_before_a_command_still_runs_the_command() {
    let dir = fixture();
    let cwd = dir.path();

    assert_eq!(exit_code(cwd, &["test", "!", "false"]), 0);
    assert_eq!(exit_code(cwd, &["test", "!", "true"]), 1);
}

/// Argument boundaries survive: the passthrough passes argv through, where the
/// shell wrapper re-split a joined string and saw three arguments here.
#[test]
fn native_expressions_keep_argument_boundaries() {
    let dir = fixture();
    std::fs::write(dir.path().join("a file"), b"").expect("create spaced file");

    assert_eq!(exit_code(dir.path(), &["test", "-f", "a file"]), 0);
}

/// The other direction: a command to run under the test filter still goes to the
/// filter, not to the system `test`, which would reject it as too many arguments.
#[test]
fn commands_still_reach_the_test_runner() {
    let dir = fixture();

    assert_eq!(exit_code(dir.path(), &["test", "echo", "hello"]), 0);
}
