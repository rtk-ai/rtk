//! `rtk find` tells the user, on stderr, which directory it could not read (#5280).
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

mod common;

fn set_mode(dir: &Path, mode: u32) {
    fs::set_permissions(dir, fs::Permissions::from_mode(mode)).expect("chmod");
}

#[test]
fn find_reports_an_unreadable_directory_on_stderr() {
    let tmp = common::tempdir();
    let root = tmp.path();
    fs::create_dir(root.join("open")).expect("mkdir open");
    fs::write(root.join("open").join("a.txt"), "a").expect("write a.txt");
    let locked = root.join("locked");
    fs::create_dir(&locked).expect("mkdir locked");
    fs::write(locked.join("b.txt"), "b").expect("write b.txt");
    set_mode(&locked, 0o000);
    if fs::read_dir(&locked).is_ok() {
        // Root reads it anyway: nothing to report.
        set_mode(&locked, 0o755);
        return;
    }

    let out = common::rtk_command()
        .args(["find"])
        .arg(root)
        .args(["-name", "*.txt"])
        .output()
        .expect("rtk find");
    set_mode(&locked, 0o755);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let expected = format!("find: '{}': Permission denied", locked.display());

    assert_eq!(out.status.code(), Some(1), "stderr: {stderr}");
    assert!(
        stderr.lines().any(|l| l == expected),
        "expected {expected:?} on stderr, got: {stderr}"
    );
    assert!(
        !stdout.contains("Permission denied"),
        "the error belongs on stderr, stdout: {stdout}"
    );
    assert!(stdout.contains("a.txt"), "readable match listed: {stdout}");
}
