//! A curl killed by a signal (OOM killer, `timeout`, a stray SIGTERM) has no exit
//! code of its own. `rtk curl` must report the POSIX `128 + signal` for it, the way
//! the shell does, instead of collapsing every signal death into a generic 1.

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

#[test]
fn curl_killed_by_sigterm_exits_128_plus_signal() {
    let dir = tempfile::tempdir().expect("tempdir");
    // `kill -TERM $$` makes the shell terminate itself by signal 15, so the parent
    // sees a wait status with a signal and no exit code.
    fake_tool(dir.path(), "curl", "kill -TERM $$");

    let out = rtk_with(dir.path(), &["curl", "http://example.invalid/"]);

    assert_eq!(
        out.status.code(),
        Some(128 + 15),
        "a SIGTERM'd curl must surface as 143, not a generic 1; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn curl_exit_code_is_unchanged_when_it_exits_normally() {
    let dir = tempfile::tempdir().expect("tempdir");
    // 6 is curl's "could not resolve host".
    fake_tool(dir.path(), "curl", "exit 6");

    let out = rtk_with(dir.path(), &["curl", "http://example.invalid/"]);

    assert_eq!(
        out.status.code(),
        Some(6),
        "curl's own exit code must survive"
    );
}
