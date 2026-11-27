#![cfg(unix)]

mod common;

use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

fn command(root: &Path) -> Command {
    let mut cmd = common::rtk_command();
    cmd.current_dir(root)
        .env("RTK_CONFIG_DIR", root.join("config"))
        .env_remove("RTK_NO_TOML")
        .env("PATH", root.join("bin"))
        .env("ARGV_FILE", root.join("argv"))
        .env("REPORT", "noise progress\n".repeat(40) + "keep result\n");
    cmd
}

#[test]
fn docker_exec_uses_only_trusted_matching_enabled_filters() {
    for filter_stderr in [false, true] {
        let dir = common::tempdir();
        let root = dir.path();
        fs::create_dir(root.join("bin")).expect("create executable directory");
        fs::create_dir(root.join(".rtk")).expect("create project configuration");
        let executable = root.join("bin/docker");
        fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$ARGV_FILE\"\nprintf '%s' \"$REPORT\"\nprintf 'noise stderr\\nkeep diagnostic\\n' >&2\nexit \"${DOCKER_EXIT:-0}\"\n",
        )
        .expect("write docker fixture");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
            .expect("make docker fixture executable");
        let config = root.join(".rtk/filters.toml");
        let contents = format!(
            "schema_version = 1\n[filters.container_test]\nmatch_command = '^docker exec demo\\b'\nstrip_lines_matching = ['^noise']\nfilter_stderr = {filter_stderr}\n"
        );
        fs::write(&config, &contents).expect("write project filter");
        let args = [
            "docker",
            "exec",
            "demo container",
            "vendor/bin/phpunit",
            "a b",
        ];
        let untrusted = command(root)
            .args(args)
            .output()
            .expect("run without trust");
        assert!(untrusted.status.success());
        assert!(String::from_utf8_lossy(&untrusted.stdout).contains("noise progress"));

        let trust = command(root)
            .args(["trust", "--yes"])
            .output()
            .expect("trust fixture");
        assert!(
            trust.status.success(),
            "{}",
            String::from_utf8_lossy(&trust.stderr)
        );
        for code in [0, 7] {
            let output = command(root)
                .args(args)
                .env("DOCKER_EXIT", code.to_string())
                .output()
                .expect("run with trusted filter");
            assert_eq!(output.status.code(), Some(code));
            let shown = String::from_utf8_lossy(&output.stdout);
            assert!(shown.contains("keep result"), "{shown}");
            assert!(!shown.contains("noise progress"), "{shown}");
            if filter_stderr {
                assert!(output.stderr.is_empty());
                assert!(shown.contains("keep diagnostic"), "{shown}");
                assert!(!shown.contains("noise stderr"), "{shown}");
            } else {
                assert_eq!(output.stderr, b"noise stderr\nkeep diagnostic\n");
            }
            assert_eq!(
                fs::read_to_string(root.join("argv")).expect("captured args"),
                "exec\ndemo container\nvendor/bin/phpunit\na b\n"
            );
        }
        let disabled = command(root)
            .args(args)
            .env("RTK_NO_TOML", "1")
            .output()
            .expect("run with filters disabled");
        assert!(String::from_utf8_lossy(&disabled.stdout).contains("noise progress"));
        let non_utf8 = command(root)
            .args(args)
            .arg(OsString::from_vec(vec![b'x', 0xff]))
            .output()
            .expect("run with a non-UTF8 argument");
        assert!(non_utf8.status.success());
        assert!(String::from_utf8_lossy(&non_utf8.stdout).contains("noise progress"));
        assert!(
            fs::read(root.join("argv"))
                .expect("captured bytes")
                .ends_with(b"x\xff\n")
        );
        let unmatched = command(root)
            .args(["docker", "exec", "another", "phpunit"])
            .output()
            .expect("run unmatched invocation");
        assert!(String::from_utf8_lossy(&unmatched.stdout).contains("noise progress"));
        fs::write(&config, contents + "\n# changed since trust\n").expect("change filter");
        let changed = command(root)
            .args(args)
            .output()
            .expect("run with changed filter");
        assert!(String::from_utf8_lossy(&changed.stdout).contains("noise progress"));

        fs::write(
            &config,
            "schema_version = 1\n[filters.container_test]\nmatch_command = '^docker exec demo\\b'\nhead_lines = 2\n",
        )
        .expect("write truncating filter");
        assert!(
            command(root)
                .args(["trust", "--yes"])
                .status()
                .expect("trust new filter")
                .success()
        );
        let truncated = command(root)
            .args(args)
            .output()
            .expect("run truncating filter");
        assert!(truncated.status.success());
        let shown = String::from_utf8_lossy(&truncated.stdout);
        assert!(!shown.contains("keep result"), "{shown}");
        let hash = shown
            .split("rtk recall ")
            .nth(1)
            .expect("recovery hint")
            .split(']')
            .next()
            .expect("recall hash");
        let recalled = command(root)
            .args(["recall", hash])
            .output()
            .expect("recover hidden output");
        assert!(recalled.status.success());
        assert!(String::from_utf8_lossy(&recalled.stdout).contains("keep result"));
        let no_recovery = command(root)
            .args(args)
            .env("RTK_RECALL", "0")
            .output()
            .expect("run with recovery disabled");
        assert!(no_recovery.status.success());
        assert!(String::from_utf8_lossy(&no_recovery.stdout).contains("keep result"));
    }
}
