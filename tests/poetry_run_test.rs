#![cfg(unix)]
//! `rtk poetry run` must hand the wrapped program's output on unchanged: a pipeline
//! consumer reads it, and `--help` / `--version` are the program's own page.
//!
//! A stand-in `poetry` that execs the wrapped program is put first on PATH, so the suite
//! needs no Poetry install.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

mod common;

fn script(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Returns the directory holding the fake `poetry` and a `tool` it can run.
fn fixture() -> tempfile::TempDir {
    let dir = common::tempdir();
    script(
        dir.path(),
        "poetry",
        "[ \"$1\" = run ] || exit 2\nshift\nexec \"$@\"",
    );
    script(
        dir.path(),
        "tool",
        "case \"$1\" in\n\
         --version) echo 'tool 1.2.3' ;;\n\
         --help) i=0; while [ $i -lt 40 ]; do echo \"  --option-$i  describes option $i\"; i=$((i+1)); done ;;\n\
         *) echo '{\"answer\": 42}' ;;\n\
         esac",
    );
    dir
}

fn rtk_poetry(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = common::rtk_command();
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    cmd.env("PATH", path).args(["poetry", "run"]).args(args);
    cmd
}

fn native(dir: &Path, args: &[&str]) -> Output {
    let mut cmd = common::native_command(dir.join("tool").to_str().unwrap());
    cmd.args(args).output().unwrap()
}

#[test]
fn json_survives_a_pipeline_consumer() {
    let dir = fixture();
    let mut producer = rtk_poetry(dir.path(), &["tool"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    // The consumer is a real parser: `ok` or a fragment fails it.
    let consumer = Command::new("sh")
        .args(["-c", "grep -q '\"answer\": 42'"])
        .stdin(producer.stdout.take().unwrap())
        .status()
        .unwrap();
    assert!(producer.wait().unwrap().success());
    assert!(
        consumer.success(),
        "the consumer did not receive the JSON object"
    );
}

#[test]
fn help_and_version_pass_through_unfiltered() {
    let dir = fixture();
    for flag in ["--help", "--version"] {
        let rtk = rtk_poetry(dir.path(), &["tool", flag]).output().unwrap();
        let want = native(dir.path(), &[flag]);
        assert_eq!(rtk.status.code(), want.status.code(), "{flag} exit code");
        assert_eq!(rtk.stdout, want.stdout, "{flag} output was altered");
    }
}

#[test]
fn silent_success_still_exits_zero_and_failure_keeps_its_code() {
    let dir = fixture();
    script(dir.path(), "quiet", "exit 0");
    script(dir.path(), "boom", "echo 'Error: boom' >&2\nexit 3");
    assert_eq!(
        rtk_poetry(dir.path(), &["quiet"]).status().unwrap().code(),
        Some(0)
    );
    let boom = rtk_poetry(dir.path(), &["boom"]).output().unwrap();
    assert_eq!(boom.status.code(), Some(3));
    // The caller must still see the program's own failure message.
    let seen =
        String::from_utf8_lossy(&boom.stdout).into_owned() + &String::from_utf8_lossy(&boom.stderr);
    assert!(seen.contains("boom"), "failure message lost: {seen:?}");
}
