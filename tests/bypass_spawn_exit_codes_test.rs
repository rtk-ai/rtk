//! Exit-code tests for `rtk bypass`: a command that cannot be started is an
//! execution outcome, not an RTK error, so bypass reports it the way the shell
//! it replaced would — 127 for a missing program, 126 for one that exists but
//! cannot be executed — instead of a generic failure exit. (#4819)
//!
//! The assertions are on exit codes and rtk's own stderr wording, never on a
//! third-party tool's message.

#![cfg(unix)]

mod common;

fn bypass(args: &[&str]) -> (i32, String) {
    let out = common::rtk_command()
        .args(args)
        .output()
        .expect("run rtk bypass");
    (
        out.status.code().expect("child exited with a code"),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn bypass_missing_command_exits_127() {
    let (code, stderr) = bypass(&["bypass", "definitely-not-a-real-command-4819"]);
    assert_eq!(code, 127, "stderr: {stderr}");
    assert!(stderr.contains("command not found"), "stderr: {stderr}");
}

#[test]
fn bypass_non_executable_file_exits_126() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("not-executable.sh");
    std::fs::write(&script, "#!/bin/sh\nexit 0\n").expect("write script");

    let path = script.to_str().expect("utf-8 path");
    let (code, stderr) = bypass(&["bypass", path]);
    assert_eq!(code, 126, "stderr: {stderr}");
    assert!(stderr.contains("Permission denied"), "stderr: {stderr}");
}

#[test]
fn bypass_directory_exits_126() {
    let dir = tempfile::tempdir().expect("tempdir");

    // Linux refuses to exec a directory with EISDIR, macOS with EACCES; both
    // are 126 for a shell. The wording differs with the errno, so accept both.
    let path = dir.path().to_str().expect("utf-8 path");
    let (code, stderr) = bypass(&["bypass", path]);
    assert_eq!(code, 126, "stderr: {stderr}");
    assert!(
        stderr.contains("Is a directory") || stderr.contains("Permission denied"),
        "stderr: {stderr}"
    );
}

#[test]
fn bypass_runnable_command_exits_with_child_status() {
    let (code, stderr) = bypass(&["bypass", "echo", "hi"]);
    assert_eq!(code, 0, "stderr: {stderr}");
}
