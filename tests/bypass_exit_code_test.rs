#![cfg(unix)]
//! Spawn failures exit with the shell's codes, not 1 (#4819).
//!
//! `rtk bypass` is the "run without filtering" escape hatch, so callers wrap
//! arbitrary commands in `&&` chains and gate on its status. A shell reports
//! 127 when a command does not exist and 126 when it exists but cannot be
//! executed (a non-executable file, or a directory); exit 1 tells the caller
//! "the command ran and failed", which a missing binary never did.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

fn bypass_code(root: &PathBuf, args: &[&str]) -> i32 {
    let mut cmd = common::rtk_command();
    cmd.arg("bypass").args(args).current_dir(root);
    cmd.status().expect("spawn rtk").code().expect("clean exit")
}

#[test]
fn bypass_spawn_failure_uses_shell_exit_codes() {
    let dir = common::tempdir();
    let root = dir.path().to_path_buf();

    let script = root.join("nonexec.sh");
    std::fs::write(&script, "#!/bin/sh\necho hi\n").expect("write");
    let mut perms = std::fs::metadata(&script).expect("stat").permissions();
    perms.set_mode(0o644);
    std::fs::set_permissions(&script, perms).expect("chmod");
    let subdir = root.join("adir");
    std::fs::create_dir(&subdir).expect("mkdir");

    let missing = root.join("no-such-binary").to_string_lossy().into_owned();
    let nonexec = script.to_string_lossy().into_owned();
    let adir = subdir.to_string_lossy().into_owned();

    for (args, expected, why) in [
        (vec!["definitely-not-a-real-cmd-4819"], 127, "not found"),
        (vec![missing.as_str()], 127, "path not found"),
        (vec![nonexec.as_str()], 126, "not executable"),
        (vec![adir.as_str()], 126, "directory"),
    ] {
        assert_eq!(bypass_code(&root, &args), expected, "{}: {:?}", why, args);
    }

    // Commands that DO start keep their own status: the mapping only covers
    // the spawn itself.
    assert_eq!(
        bypass_code(&root, &["sh", "-c", "exit 7"]),
        7,
        "ran command"
    );
    assert_eq!(bypass_code(&root, &["false"]), 1, "ran command failing");
}

#[test]
fn proxy_alias_matches_bypass_exit_codes() {
    let dir = common::tempdir();
    let root = dir.path().to_path_buf();

    let mut cmd = common::rtk_command();
    cmd.arg("proxy")
        .arg("definitely-not-a-real-cmd-4819")
        .current_dir(root);
    let code = cmd.status().expect("spawn rtk").code().expect("clean exit");
    assert_eq!(code, 127, "proxy is a silent alias for bypass");
}
