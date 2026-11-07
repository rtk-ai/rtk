#![cfg(unix)]

mod common;

use std::fs;
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
    for command in ["cargo +stable metadata", "cargo +stable checker"] {
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
        "cargo +nightly -Zscript check",
        "cargo +nightly -vvZscript check",
        "cargo +nightly -Z=script check",
        "cargo +nightly -Z script check",
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
        vec!["+nightly", "-Zscript", "check"],
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
}
