//! `rtk cargo build|check|clippy|test --message-format=short` prints what cargo prints, stream
//! for stream, with cargo's exit code: the user already asked for one line per diagnostic.
//!
//! A `cargo` shim on PATH stands in for cargo, so the run needs no toolchain and the output
//! is the same on every machine. It echoes its arguments on stdout and prints short
//! diagnostics on stderr, the stream cargo prints them on.
#![cfg(unix)]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;

const SHIM: &str = r#"#!/bin/sh
for arg in "$@"; do printf 'arg: %s\n' "$arg"; done
echo 'src/main.rs:1:4: warning: function `f` is never used' >&2
echo 'src/main.rs:2:9: error[E0425]: cannot find value `x` in this scope' >&2
echo 'error: could not compile `demo` (bin "demo") due to 1 previous error; 1 warning emitted' >&2
exit 101
"#;

fn shim_dir() -> tempfile::TempDir {
    let dir = common::tempdir();
    let cargo = dir.path().join("cargo");
    fs::write(&cargo, SHIM).expect("write cargo shim");
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).expect("chmod cargo shim");
    dir
}

/// Runs rtk with `dir` first on PATH, so the cargo shim shadows any real cargo.
fn rtk_cargo(dir: &Path, args: &[&str]) -> Output {
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    common::rtk_command()
        .arg("cargo")
        .args(args)
        .env("PATH", path)
        .output()
        .expect("run rtk cargo")
}

fn native_cargo(dir: &Path, args: &[&str]) -> Output {
    common::native_command(dir.join("cargo").to_str().expect("utf-8 shim path"))
        .args(args)
        .output()
        .expect("run cargo shim")
}

fn assert_same(args: &[&str], rtk: &Output, native: &Output) {
    let show = |o: &Output| {
        format!(
            "stdout:\n{}\nstderr:\n{}\nexit: {:?}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr),
            o.status.code()
        )
    };
    assert!(
        rtk.stdout == native.stdout
            && rtk.stderr == native.stderr
            && rtk.status.code() == native.status.code(),
        "rtk cargo {}\n--- rtk\n{}\n--- cargo\n{}",
        args.join(" "),
        show(rtk),
        show(native)
    );
}

#[test]
fn short_message_format_passes_through() {
    let dir = shim_dir();
    for args in [
        &["build", "--message-format=short"][..],
        &["check", "--message-format", "short"],
        &["clippy", "--message-format=short", "--", "-D", "warnings"],
        &["test", "--message-format=short"],
    ] {
        let rtk = rtk_cargo(dir.path(), args);
        let native = native_cargo(dir.path(), args);
        assert_same(args, &rtk, &native);
    }
}

/// Cargo refuses two message formats before building anything, with an error of its own.
#[test]
fn rejected_message_format_passes_through() {
    let dir = shim_dir();
    for args in [
        &["build", "--message-format=short", "--message-format=json"][..],
        &["check", "--message-format=json,human"],
        &["clippy", "--message-format=human", "--message-format=short"],
        &["test", "--message-format=json,json"],
    ] {
        let rtk = rtk_cargo(dir.path(), args);
        let native = native_cargo(dir.path(), args);
        assert_same(args, &rtk, &native);
    }
}

/// The control: the same shim output in the human format is filtered, so the tests above
/// fail when the short format is not routed to the passthrough. Past `--` the flag is not
/// cargo's (it goes to the test binary or to clippy-driver), and cargo prints its human
/// format.
#[test]
fn human_message_format_is_filtered() {
    let dir = shim_dir();
    for args in [
        &["build", "--message-format=human"][..],
        &["build", "--", "--message-format=short"],
        &["check", "--", "--message-format=short"],
        &["clippy", "--", "--message-format=short"],
        &["test", "--", "--message-format=short"],
    ] {
        let rtk = rtk_cargo(dir.path(), args);
        let native = native_cargo(dir.path(), args);
        assert_eq!(rtk.status.code(), Some(101), "rtk cargo {}", args.join(" "));
        assert_ne!(
            (rtk.stdout, rtk.stderr),
            (native.stdout, native.stderr),
            "rtk cargo {} must filter",
            args.join(" ")
        );
    }
}
