//! `rtk cargo test -- --format json|junit` passes through unfiltered: machine-readable
//! reports must preserve the libtest events/XML without losing failing test details.
#![cfg(unix)]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;

const SHIM: &str = r#"#!/bin/sh
for arg in "$@"; do printf 'arg: %s\n' "$arg"; done
echo '{"type":"suite","event":"started","test_count":1}'
echo '{"type":"test","event":"failed","name":"tests::fails","stdout":"panicked at assert_eq!(1, 2)"}'
exit 101
"#;

fn shim_dir() -> tempfile::TempDir {
    let dir = common::tempdir();
    let cargo = dir.path().join("cargo");
    fs::write(&cargo, SHIM).expect("write cargo shim");
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).expect("chmod cargo shim");
    dir
}

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

#[test]
fn machine_readable_test_format_passes_through() {
    let dir = shim_dir();
    for args in [
        &["test", "--", "--format", "json"][..],
        &["test", "--", "--format=json"],
        &["test", "--", "-Zunstable-options", "--format", "json"],
        &["test", "--", "--format", "junit"],
        &["test", "--", "--format=junit"],
        &["test", "-p", "my-crate", "--", "--format=json"],
    ] {
        let rtk = rtk_cargo(dir.path(), args);
        let native = native_cargo(dir.path(), args);
        assert_eq!(
            rtk.stdout,
            native.stdout,
            "stdout mismatch for rtk cargo {}",
            args.join(" ")
        );
        assert_eq!(
            rtk.status.code(),
            native.status.code(),
            "exit code mismatch for rtk cargo {}",
            args.join(" ")
        );
    }
}
