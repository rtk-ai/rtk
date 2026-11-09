#![cfg(unix)]

mod common;

use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;

#[test]
fn toolchain_rewrites_preserve_selector_and_arguments() {
    for toolchain in ["+stable", "+nightly", "+1.75.0", "+nightly-2026-08-01"] {
        for subcommand in ["build", "check", "clippy", "test", "install", "fmt"] {
            let command = format!("cargo {toolchain} {subcommand} -p demo");
            let output = common::rtk_command()
                .args(["rewrite", &command])
                .output()
                .unwrap();
            assert!(
                matches!(output.status.code(), Some(0 | 3)),
                "{command}: {output:?}"
            );
            assert_eq!(
                String::from_utf8(output.stdout).unwrap().trim(),
                format!("rtk {command}")
            );
        }
    }
    for command in [
        "cargo +stable metadata",
        "cargo +stable checker",
        "cargo",
        "cargo +stable",
        "cargo --help",
        "cargo --version",
        "cargo --list",
        "cargo --explain E0425",
    ] {
        let output = common::rtk_command()
            .args(["rewrite", command])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{command}");
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn global_options_and_aliases_rewrite_without_changing_shell_text() {
    for command in [
        "cargo --locked check",
        "cargo --config=-- check",
        // The command is identifiable even when Cargo will reject a value.
        "cargo + check",
        "cargo --color= check",
        "cargo --config= check",
        "cargo -Z= check",
        "cargo --offline test",
        "cargo --frozen build",
        "cargo --config net.retry=2 --config=build.jobs=1 b",
        r#"cargo --config 'build.rustflags=["--cfg", "demo"]' check"#,
        "cargo --config check t",
        "cargo --color never c",
        "cargo --color=never check",
        "cargo -q check",
        "cargo -vv build",
        "cargo --verbose --quiet check",
        r#"cargo +nightly -Z unstable-options -C 'some path' c"#,
        "cargo +nightly -Zunstable-options -Csome-path check",
        "cargo +nightly -Z=unstable-options -C=some-path check",
        "cargo +nightly -Zscript check",
        "cargo +nightly -vvZscript check",
        "cargo +nightly -Z=script check",
        "cargo +nightly -Z script check",
        "cargo b",
        "cargo c",
        "cargo t -- --nocapture",
        "cargo nextest run",
        "RUSTUP_TOOLCHAIN=nightly cargo --locked check",
    ] {
        let output = common::rtk_command()
            .args(["rewrite", command])
            .output()
            .unwrap();
        assert!(
            matches!(output.status.code(), Some(0 | 3)),
            "{command}: {output:?}"
        );
        let expected = if let Some(tail) = command.strip_prefix("RUSTUP_TOOLCHAIN=nightly ") {
            format!("RUSTUP_TOOLCHAIN=nightly rtk {tail}")
        } else {
            format!("rtk {command}")
        };
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), expected);
    }
    for command in [
        "cargo --config",
        "cargo --config -- check",
        "cargo --color -- check",
        "cargo -C -- check",
        "cargo -Z -- check",
        "cargo --color",
        "cargo --config check",
        "cargo -C check",
        "cargo --unknown check",
        "cargo +stable --help",
        "cargo --version",
        "cargo --list",
        "cargo --explain check",
        "cargo +nightly -Zscript file.rs check",
        "cargo +nightly -Z script ./check",
        "cargo --config check metadata",
    ] {
        let output = common::rtk_command()
            .args(["rewrite", command])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{command}: {output:?}");
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn toolchain_commands_filter_output_and_preserve_child_arguments_and_status() {
    let dir = common::tempdir();
    let cargo = dir.path().join("cargo");
    fs::write(
        &cargo,
        r#"#!/bin/sh
printf '%s\n' "$@" > "$CARGO_ARGV_FILE"
printf '    Checking demo v0.1.0\n' >&2
printf 'error[E0308]: mismatched types\n --> src/lib.rs:1:1\n\n' >&2
printf 'error: could not compile demo due to 1 previous error\n' >&2
exit 101
"#,
    )
    .unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(std::iter::once(dir.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    let argv_file = dir.path().join("argv");
    for prefix in [
        vec![],
        vec!["+stable"],
        vec!["--locked"],
        vec!["--offline", "--frozen"],
        vec!["--color", "never"],
        vec!["--color=never"],
        vec!["-q"],
        vec!["-vv"],
        vec!["--config", "net.retry=2", "--config=build.jobs=1"],
        vec!["+nightly", "-Z", "unstable-options", "-C", "some path"],
        vec!["+nightly", "-Zunstable-options", "-Csome path"],
        vec!["+nightly", "-Z=unstable-options", "-C=some path"],
        vec!["+nightly", "-vvZunstable-options", "-Cvery/quiet"],
        vec!["+nightly", "-vCsome path", "-Z=unstable-options"],
        vec!["+nightly", "-Zscript"],
        vec!["+nightly", "-Z", "script"],
    ] {
        for subcommand in [
            "build", "check", "clippy", "test", "b", "c", "t", "metadata", "fmt", "install",
            "nextest",
        ] {
            let canonical = match subcommand {
                "b" => "build",
                "c" => "check",
                "t" => "test",
                other => other,
            };
            for json in [false, true] {
                let args: Vec<&str> = prefix
                    .iter()
                    .copied()
                    .chain([subcommand, "-p", "demo"])
                    .chain(json.then_some("--message-format=json"))
                    .chain(["--", "--nocapture"])
                    .collect();
                let output = common::rtk_command()
                    .arg("cargo")
                    .args(&args)
                    .env("PATH", &path)
                    .env("CARGO_ARGV_FILE", &argv_file)
                    .output()
                    .unwrap();
                assert_eq!(output.status.code(), Some(101), "{subcommand}: {output:?}");
                assert_eq!(
                    fs::read_to_string(&argv_file).unwrap(),
                    format!("{}\n", args.join("\n"))
                );
                let text = format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                if matches!(subcommand, "metadata" | "fmt") {
                    assert!(text.contains("error[E0308]"), "{subcommand}: {text}");
                    assert!(text.contains("Checking demo"), "{text}");
                } else if matches!(canonical, "build" | "check" | "clippy" | "test") {
                    assert!(text.contains("error[E0308]"), "{subcommand}: {text}");
                    assert!(!text.contains("Checking demo"), "{subcommand}: {text}");
                    assert!(
                        text.contains(&format!("cargo {canonical}:")),
                        "{subcommand}: {text}"
                    );
                }
            }
        }
    }
    for args in [
        vec!["--unknown", "check"],
        vec!["+nightly", "-Zscript", "file.rs", "check"],
        vec!["--config"],
        vec!["+stable", "--help"],
        vec!["--version"],
        vec!["--list"],
        vec!["--explain", "check"],
        vec!["--config", "check", "metadata"],
        vec!["--locked", "t", "--", "--help"],
        vec!["--color", "never", "b", "--help"],
        vec!["check", "--color", "--help"],
    ] {
        let output = common::rtk_command()
            .arg("cargo")
            .args(&args)
            .env("PATH", &path)
            .env("CARGO_ARGV_FILE", &argv_file)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(101), "{args:?}: {output:?}");
        assert_eq!(
            fs::read_to_string(&argv_file).unwrap(),
            format!("{}\n", args.join("\n"))
        );
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(text.contains("Checking demo"), "{args:?}: {text}");
    }

    let args = [
        OsString::from("+stable"),
        OsString::from("--config"),
        OsString::from_vec(vec![0xff]),
        OsString::from("check"),
        OsString::from("--message-format=json"),
        OsString::from("--manifest-path"),
        OsString::from_vec(vec![0xfe]),
    ];
    let output = common::rtk_command()
        .arg("cargo")
        .args(&args)
        .env("PATH", &path)
        .env("CARGO_ARGV_FILE", &argv_file)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(101), "{output:?}");
    let expected: Vec<u8> = args
        .iter()
        .flat_map(|arg| arg.as_bytes().iter().copied().chain(std::iter::once(b'\n')))
        .collect();
    assert_eq!(fs::read(&argv_file).unwrap(), expected);
}

#[test]
fn tracking_and_verbose_output_preserve_the_full_argument_list() {
    let dir = common::tempdir();
    let cargo = dir.path().join("cargo");
    fs::write(
        &cargo,
        r#"#!/bin/sh
printf '%s\n' "$@" > "$CARGO_ARGV_FILE"
printf '    Checking demo v0.1.0\n' >&2
printf 'error[E0308]: mismatched types\n --> src/lib.rs:1:1\n\n' >&2
printf 'error: could not compile demo due to 1 previous error\n' >&2
exit 101
"#,
    )
    .unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(std::iter::once(dir.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    let argv_file = dir.path().join("argv");
    let cases: &[(&str, &[&str], bool)] = &[
        ("b", &[], false),
        ("check", &[], false),
        ("t", &[], false),
        ("clippy", &[], false),
        ("install", &[], false),
        ("nextest", &["run"], false),
        ("b", &["--message-format=json"], false),
        ("c", &["--message-format=json"], false),
        ("clippy", &["--message-format=json"], false),
        ("fmt", &[], true),
        ("check", &["--unit-graph"], true),
    ];
    for (case, &(subcommand, extra, passthrough)) in cases.iter().enumerate() {
        let args: Vec<_> = [
            "+nightly",
            "-Zunstable-options",
            "-C",
            "some path",
            "--config",
            "build.jobs=2",
            "--color",
            "never",
            subcommand,
        ]
        .into_iter()
        .chain(extra.iter().copied())
        .chain([
            "--manifest-path",
            "some workspace/Cargo.toml",
            "--",
            "",
            "--nocapture",
        ])
        .collect();
        let extra_display = if extra.is_empty() {
            String::new()
        } else {
            format!(" {}", extra.join(" "))
        };
        let expected = format!(
            "cargo +nightly -Zunstable-options -C 'some path' --config build.jobs=2 --color never {subcommand}{extra_display} --manifest-path 'some workspace/Cargo.toml' -- '' --nocapture"
        );
        for verbose in [false, true] {
            let db = dir.path().join(format!("{case}-{verbose}.db"));
            let mut cmd = common::rtk_command();
            if verbose {
                cmd.arg("-v");
            }
            let output = cmd
                .arg("cargo")
                .args(&args)
                .env("PATH", &path)
                .env("CARGO_ARGV_FILE", &argv_file)
                .env("RTK_DB_PATH", &db)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(101), "{args:?}: {output:?}");
            assert_eq!(
                fs::read_to_string(&argv_file).unwrap(),
                format!("{}\n", args.join("\n"))
            );
            let conn = rusqlite::Connection::open(&db).unwrap();
            let (original, rtk): (String, String) = conn
                .query_row("SELECT original_cmd, rtk_cmd FROM commands", [], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .unwrap();
            assert_eq!(original, expected, "{args:?}");
            assert_eq!(
                rtk,
                format!(
                    "{} {expected}",
                    if passthrough {
                        "rtk:passthrough"
                    } else {
                        "rtk"
                    }
                )
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            let log = if passthrough {
                format!("cargo passthrough: {args:?}")
            } else {
                format!("Running: {expected}")
            };
            assert_eq!(
                stderr.lines().filter(|line| *line == log).count(),
                usize::from(verbose),
                "{args:?}: {stderr}"
            );
        }
    }
}

#[test]
fn parsed_information_modes_are_interpreted_by_command_runners() {
    let dir = common::tempdir();
    let cargo = dir.path().join("cargo");
    let page = "Requested information: this is neither a build diagnostic nor a test result.\nEvery line of this output must be preserved exactly by the command runner.\n";
    fs::write(&cargo, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$CARGO_ARGV_FILE\"\nprintf '%s' '{page}'\nexit 37\n")).unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(std::iter::once(dir.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    let argv_file = dir.path().join("argv");
    for args in [
        vec![],
        vec!["+stable"],
        vec!["--locked", "--color", "never"],
        vec!["--help"],
        vec!["--version"],
        vec!["--list"],
        vec!["--explain", "E0425"],
        vec!["-Zhelp"],
        vec!["check", "--unit-graph"],
        vec!["build", "-Zhelp"],
        vec!["clippy", "--version"],
        vec!["clippy", "-V"],
        vec!["clippy", "--explain", "clippy::needless_return"],
        vec!["install", "--list"],
        vec!["install", "--message-format=json"],
        vec!["nextest", "list"],
        vec!["nextest", "show-config"],
        vec!["nextest", "run", "--message-format=libtest-json"],
        vec!["nextest", "run", "--cargo-message-format=json"],
        vec!["test", "--", "--list"],
        vec!["test", "--", "--format=json"],
        vec!["test", "--", "--help"],
    ] {
        let output = common::rtk_command()
            .arg("cargo")
            .args(&args)
            .env("PATH", &path)
            .env("CARGO_ARGV_FILE", &argv_file)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(37), "{args:?}: {output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), page, "{args:?}");
        assert_eq!(
            fs::read_to_string(&argv_file).unwrap(),
            format!("{}\n", args.join("\n")),
            "{args:?}"
        );
    }
}
