#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;

mod common;

#[test]
fn npx_next_only_filters_builds() {
    let dir = tempfile::tempdir().expect("create test directory");
    let args_file = dir.path().join("child-args");
    for tool in ["npx", "next"] {
        let path = dir.path().join(tool);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' '{tool}' \"$@\" > \"$NEXT_ARGS_FILE\"\nprintf 'ready\\n'\nprintf 'diagnostic\\n' >&2\nexit \"$NEXT_EXIT\"\n"
            ),
        )
        .expect("create executable fixture");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("create executable fixture");
    }

    for (args, expected, exit_code) in [
        (vec!["next"], "npx\nnext\n", 0),
        (
            vec!["next", "dev", "site with spaces", "--port", "4000"],
            "npx\nnext\ndev\nsite with spaces\n--port\n4000\n",
            0,
        ),
        (vec!["next", "start"], "npx\nnext\nstart\n", 7),
        (vec!["next", "--help"], "npx\nnext\n--help\n", 0),
        (vec!["next", "build"], "next\nbuild\n", 0),
        (
            vec!["next", "build", "build", "--debug"],
            "next\nbuild\nbuild\n--debug\n",
            0,
        ),
    ] {
        let output = common::rtk_command()
            .current_dir(dir.path())
            .env("PATH", dir.path())
            .env("NEXT_ARGS_FILE", &args_file)
            .env("NEXT_EXIT", exit_code.to_string())
            .arg("npx")
            .args(&args)
            .output()
            .expect("run RTK");
        assert_eq!(output.status.code(), Some(exit_code), "{args:?}");
        assert_eq!(
            std::fs::read_to_string(&args_file).expect("read captured child arguments"),
            expected,
            "{args:?}"
        );
        if args.get(1) != Some(&"build") {
            assert_eq!(output.stdout, b"ready\n", "{args:?}");
            assert_eq!(output.stderr, b"diagnostic\n", "{args:?}");
        }
    }
}
