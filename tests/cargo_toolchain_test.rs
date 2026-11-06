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
    for command in [
        "cargo +stable metadata",
        "cargo +stable checker",
        "cargo + check",
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
    for subcommand in [
        "build", "check", "clippy", "test", "metadata", "fmt", "install", "nextest",
    ] {
        let output = common::rtk_command()
            .args([
                "cargo",
                "+stable",
                subcommand,
                "-p",
                "demo",
                "--",
                "--nocapture",
            ])
            .env("PATH", &path)
            .env("CARGO_ARGV_FILE", &argv_file)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(101), "{subcommand}: {output:?}");
        assert_eq!(
            fs::read_to_string(&argv_file).unwrap(),
            format!("+stable\n{subcommand}\n-p\ndemo\n--\n--nocapture\n")
        );
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if matches!(subcommand, "metadata" | "fmt") {
            assert!(text.contains("error[E0308]"), "{subcommand}: {text}");
            assert!(text.contains("Checking demo"), "{text}");
        } else if matches!(subcommand, "build" | "check" | "clippy" | "test") {
            assert!(text.contains("error[E0308]"), "{subcommand}: {text}");
            assert!(!text.contains("Checking demo"), "{subcommand}: {text}");
            assert!(
                text.contains(&format!("cargo {subcommand}:")),
                "{subcommand}: {text}"
            );
        }
    }
}
