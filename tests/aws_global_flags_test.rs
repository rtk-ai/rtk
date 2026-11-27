#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;

mod common;

#[test]
fn aws_global_flags_reach_the_native_cli() {
    let dir = tempfile::tempdir().expect("create test directory");
    let aws = dir.path().join("aws");
    let args_file = dir.path().join("aws-args");
    std::fs::write(
        &aws,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$AWS_ARGS_FILE\"\nprintf 'aws stdout\\n'\nprintf 'aws stderr\\n' >&2\nexit \"$AWS_EXIT\"\n",
    )
    .expect("create executable fixture");
    std::fs::set_permissions(&aws, std::fs::Permissions::from_mode(0o755))
        .expect("create executable fixture");

    for args in [
        vec!["--version"],
        vec!["--help"],
        vec!["--profile", "a profile", "--version"],
        vec!["--region", "us-east-1", "sts", "get-caller-identity"],
    ] {
        for exit_code in [0, 2] {
            let output = common::rtk_command()
                .current_dir(dir.path())
                .env("PATH", dir.path())
                .env("AWS_ARGS_FILE", &args_file)
                .env("AWS_EXIT", exit_code.to_string())
                .arg("aws")
                .args(&args)
                .output()
                .expect("run RTK");
            assert_eq!(output.status.code(), Some(exit_code), "{args:?}");
            assert_eq!(
                std::fs::read_to_string(&args_file).expect("read captured child arguments"),
                format!("{}\n", args.join("\n")),
                "{args:?}"
            );
            assert_eq!(output.stdout, b"aws stdout\n", "{args:?}");
            assert_eq!(output.stderr, b"aws stderr\n", "{args:?}");
        }
    }
}
