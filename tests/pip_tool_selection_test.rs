#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;

mod common;

#[test]
fn pip_is_preferred_and_uv_fallback_is_disclosed_without_verbose() {
    for available in [&["pip"][..], &["pip", "uv"], &["uv"]] {
        let dir = common::tempdir();
        let args_file = dir.path().join("child-args");
        for tool in available {
            let path = dir.path().join(tool);
            std::fs::write(&path, format!(
                "#!/bin/sh\nprintf '%s\\n' '{tool}' \"$@\" > \"$PIP_ARGS_FILE\"\nprintf 'tool stdout\\n'\nprintf 'tool stderr\\n' >&2\nexit \"$PIP_EXIT\"\n"
            )).expect("write tool fixture");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("make tool executable");
        }
        let fallback = !available.contains(&"pip");
        for exit_code in [0, 7] {
            let output = common::rtk_command()
                .env("PATH", dir.path())
                .env_remove("VIRTUAL_ENV")
                .env("PIP_ARGS_FILE", &args_file)
                .env("PIP_EXIT", exit_code.to_string())
                .args(["pip", "install", "package with spaces", "--user"])
                .output()
                .expect("run RTK");
            assert_eq!(output.status.code(), Some(exit_code), "{output:?}");
            let expected = if fallback {
                "uv\npip\ninstall\npackage with spaces\n--user\n"
            } else {
                "pip\ninstall\npackage with spaces\n--user\n"
            };
            assert_eq!(
                std::fs::read_to_string(&args_file).expect("read argv"),
                expected
            );
            assert_eq!(output.stdout, b"tool stdout\n");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("tool stderr"), "{stderr}");
            assert_eq!(
                stderr.contains("falling back to `uv pip`"),
                fallback,
                "{stderr}"
            );
        }
    }
}
