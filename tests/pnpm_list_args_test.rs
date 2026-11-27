//! Explicit list options must reach pnpm without RTK's JSON/depth defaults.
#![cfg(unix)]

mod common;

fn run(args: &[&str], code: i32) -> (Vec<String>, std::process::Output) {
    let dir = common::tempdir();
    let stub = dir.path().join("pnpm");
    std::fs::write(&stub, format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > argv.txt\nprintf '%s' '[{{\"name\":\"workspace\",\"version\":\"1.0.0\",\"path\":\"/a/long/workspace/path/that/is/not/part/of/the/condensed/listing\"}}]'\nprintf '%s' 'pnpm diagnostic' >&2\nexit {code}\n"
    )).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = common::rtk_command()
        .env(
            "PATH",
            format!(
                "{}:{}",
                dir.path().display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .current_dir(dir.path())
        .arg("pnpm")
        .args(args)
        .output()
        .unwrap();
    let argv = std::fs::read_to_string(dir.path().join("argv.txt"))
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stderr)))
        .lines()
        .map(str::to_owned)
        .collect();
    (argv, output)
}

#[test]
fn explicit_json_preserves_argv_bytes_and_exit_status() {
    for code in [0, 2, 7] {
        let (argv, output) = run(&["list", "--json"], code);
        assert_eq!(argv, ["list", "--json"]);
        assert_eq!(output.status.code(), Some(code));
        assert_eq!(
            output.stdout,
            br#"[{"name":"workspace","version":"1.0.0","path":"/a/long/workspace/path/that/is/not/part/of/the/condensed/listing"}]"#
        );
        assert_eq!(output.stderr, b"pnpm diagnostic");
    }
}

#[test]
fn filtered_json_with_negative_depth_is_forwarded_once() {
    let (argv, output) = run(
        &[
            "--filter", "@demo/a", "list", "--json", "--depth", "-1", ".",
        ],
        0,
    );
    assert_eq!(
        argv,
        ["list", "--filter=@demo/a", "--json", "--depth", "-1", "."]
    );
    assert!(output.status.success());
}

#[test]
fn explicit_depth_in_each_position_has_no_injected_defaults() {
    for args in [
        vec!["list", "--depth", "-1"],
        vec!["list", "--depth=-1"],
        vec!["list", "-d", "-1"],
    ] {
        let (argv, output) = run(&args, 0);
        assert_eq!(argv, ["list", "--depth=-1"]);
        assert_eq!(
            output.stdout,
            br#"[{"name":"workspace","version":"1.0.0","path":"/a/long/workspace/path/that/is/not/part/of/the/condensed/listing"}]"#
        );
    }
    let (argv, _) = run(&["list", "--prod", "--depth=2"], 0);
    assert_eq!(argv, ["list", "--prod", "--depth=2"]);
    let (argv, _) = run(&["list", "--depth=2", "--json"], 0);
    assert_eq!(argv, ["list", "--depth=2", "--json"]);
}

#[test]
fn separator_and_flag_shaped_package_selectors_are_preserved() {
    for args in [
        vec!["list", "--", "--json", "--depth"],
        vec!["list", "some-package", "--", "--json"],
    ] {
        let (argv, output) = run(&args, 0);
        assert_eq!(argv, args);
        assert!(output.status.success());
    }
}

#[test]
fn default_list_still_injects_defaults_and_condenses() {
    let (argv, output) = run(&["list"], 0);
    assert_eq!(argv, ["list", "--depth=0", "--json"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("workspace 1.0.0"));
    assert!(!output.stdout.starts_with(b"[{"));
}
