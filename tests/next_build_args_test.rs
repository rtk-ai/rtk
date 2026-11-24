#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;

mod common;

#[test]
fn next_build_subcommand_is_passed_once() {
    let dir = tempfile::tempdir().expect("create test directory");
    let next = dir.path().join("next");
    let args_file = dir.path().join("next-args");
    std::fs::write(
        &next,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$NEXT_ARGS_FILE\"\nexit 0\n",
    )
    .expect("create executable fixture");
    std::fs::set_permissions(&next, std::fs::Permissions::from_mode(0o755))
        .expect("create executable fixture");

    for (args, expected) in [
        (vec![], "build\n"),
        (vec!["build"], "build\n"),
        (vec!["build", "--no-lint"], "build\n--no-lint\n"),
        (vec!["--no-lint"], "build\n--no-lint\n"),
        (
            vec!["build", "site with spaces", "--debug"],
            "build\nsite with spaces\n--debug\n",
        ),
        (vec!["./build"], "build\n./build\n"),
        (vec!["build", "build"], "build\nbuild\n"),
    ] {
        let output = common::rtk_command()
            .current_dir(dir.path())
            .env("PATH", dir.path())
            .env("NEXT_ARGS_FILE", &args_file)
            .arg("next")
            .args(&args)
            .output()
            .expect("run RTK");
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(&args_file).expect("read captured child arguments"),
            expected,
            "rtk next {args:?}"
        );
    }
}
