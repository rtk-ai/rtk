#![cfg(unix)]

//! A child killed by a signal has no exit code of its own, so `128 + signal` is
//! all that reaches the caller — and on its own that is indistinguishable from a
//! tool which genuinely exited with that code. The streaming path must therefore
//! leave a diagnostic on stderr, the way the other spawn paths already do.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

/// A directory holding a `git` that kills itself with SIGKILL, so the wrapper
/// sees a signal-terminated child without depending on a real repository.
fn fake_git_on_path() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let git = dir.path().join("git");
    let mut file = fs::File::create(&git).expect("create fake git");
    file.write_all(b"#!/bin/sh\nkill -9 $$\n")
        .expect("write fake git");
    drop(file);
    fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).expect("chmod fake git");
    let bin = dir.path().to_path_buf();
    (dir, bin)
}

#[test]
fn git_push_reports_a_signal_killed_child_on_stderr() {
    let (_dir, bin) = fake_git_on_path();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["git", "push"])
        .env("PATH", path)
        .output()
        .expect("run rtk git push");

    assert_eq!(
        out.status.code(),
        Some(137),
        "a SIGKILLed child keeps the 128 + signal exit code"
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("terminated by signal 9"),
        "the kill must be reported, stderr was: {:?}",
        stderr
    );
    assert!(
        stderr.contains("git"),
        "the diagnostic must name the command, stderr was: {:?}",
        stderr
    );
}
