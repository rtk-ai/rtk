//! `rtk err <cmd> [args...]` reached the child as a shell string, so an argv the
//! shell had already tokenized was joined and re-split on whitespace. A quoted
//! argument lost its boundary, and a failure inside it could report success.
//! These pin the argv route and the shell-string route it sits next to.
//!
//! The probe checks its own argv and reports through its exit code, so the
//! assertion does not depend on what the err filter decides to show.

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

/// A probe that passes only when it is handed exactly `expected`, and otherwise
/// exits with its own code: 41 for the wrong count, then 42, 43, … per argument.
/// `expected` is literal test data, so it carries no single quotes.
fn argv_probe(dir: &Path, expected: &[&str]) {
    let mut body = format!("[ \"$#\" -eq {} ] || exit 41\n", expected.len());
    for (i, want) in expected.iter().enumerate() {
        body.push_str(&format!(
            "[ \"${}\" = '{}' ] || exit {}\n",
            i + 1,
            want,
            42 + i
        ));
    }
    body.push_str("exit 0");
    fake_tool(dir, "argvprobe", &body);
}

#[test]
fn quoted_argument_holding_a_space_arrives_whole() {
    let dir = tempfile::tempdir().expect("tempdir");
    argv_probe(dir.path(), &["a b", "c"]);

    let out = rtk_with(dir.path(), &["err", "argvprobe", "a b", "c"]);

    assert_eq!(
        out.status.code(),
        Some(0),
        "argv must reach the child as ['a b', 'c']; exit 41 = wrong argument count, \
         42 = the quoted argument was split"
    );
}

/// Unquoted multi-word argv is the documented spelling (`rtk err npm run build`).
#[test]
fn plain_argument_list_arrives_whole() {
    let dir = tempfile::tempdir().expect("tempdir");
    argv_probe(dir.path(), &["one", "two"]);

    let out = rtk_with(dir.path(), &["err", "argvprobe", "one", "two"]);

    assert_eq!(
        out.status.code(),
        Some(0),
        "argv must reach the child verbatim; a non-zero probe exit names the mismatch"
    );
}

/// `sh -c 'exit 7'` re-split to `sh -c exit 7` runs the one-word script `exit`
/// with `$0=7`, which exits 0 — a hard failure reported as success.
#[test]
fn failure_inside_a_quoted_child_script_still_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    argv_probe(dir.path(), &["a b", "c"]);

    let out = rtk_with(dir.path(), &["err", "sh", "-c", "exit 7"]);

    assert_eq!(
        out.status.code(),
        Some(7),
        "the child's exit code must reach the caller"
    );
}

/// A lone argument is a shell string, not an argv, and keeps the shell so that
/// `rtk err '<cmd> && <cmd>'` still composes.
#[test]
fn single_shell_string_still_composes_through_the_shell() {
    let dir = tempfile::tempdir().expect("tempdir");
    argv_probe(dir.path(), &["a b", "c"]);

    let out = rtk_with(dir.path(), &["err", "sh -c 'exit 9'"]);

    assert_eq!(
        out.status.code(),
        Some(9),
        "a single shell string must still run through the shell"
    );
}
