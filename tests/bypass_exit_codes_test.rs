//! `rtk bypass` must report the shell's spawn-failure exit codes.

mod common;

#[test]
fn bypass_missing_binary_exits_127() {
    let status = common::rtk_command()
        .args(["bypass", "rtk-definitely-missing-binary-xyz"])
        .status()
        .expect("run rtk bypass");
    assert_eq!(status.code(), Some(127));
}

#[cfg(unix)]
#[test]
fn bypass_non_executable_file_exits_126() {
    use std::os::unix::fs::PermissionsExt;

    let dir = common::tempdir();
    let path = dir.path().join("not-executable.sh");
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");

    let status = common::rtk_command()
        .args(["bypass", path.to_str().expect("utf-8 path")])
        .status()
        .expect("run rtk bypass");
    assert_eq!(status.code(), Some(126));
}
