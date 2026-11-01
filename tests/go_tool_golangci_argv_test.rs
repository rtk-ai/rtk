//! Pin the arguments passed through the public go-tool CLI to golangci-lint.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;

mod common;

fn shell_quote(path: &std::path::Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn golangci_argv(version: u32, args: &[&str]) -> Vec<String> {
    let dir = common::tempdir();
    let argv_file = dir.path().join("argv.txt");
    let fixture = dir.path().join("issues.json");
    fs::write(
        &fixture,
        include_str!("fixtures/golangci_v2_issues_raw.json"),
    )
    .expect("write lint fixture");
    let stub = dir.path().join("go");
    fs::write(
        &stub,
        format!(
            "#!/bin/sh\nif [ \"$3\" = \"--version\" ]; then\n  echo 'golangci-lint has version {version}.0.0'\n  exit 0\nfi\nprintf '%s\\n' \"$@\" > {}\n/bin/cat {}\nexit 1\n",
            shell_quote(&argv_file),
            shell_quote(&fixture),
        ),
    )
    .expect("write go stub");
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("chmod go stub");

    let output = common::rtk_command()
        .args(["go", "tool", "golangci-lint"])
        .args(args)
        .env("PATH", dir.path())
        .current_dir(dir.path())
        .output()
        .expect("run rtk go tool golangci-lint");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stdout={stdout}; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("errcheck"), "{stdout}");
    fs::read_to_string(&argv_file)
        .expect("read captured go arguments")
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn go_tool_golangci_run_is_forwarded_once() {
    assert_eq!(
        golangci_argv(2, &["run", "./..."]),
        [
            "tool",
            "golangci-lint",
            "run",
            "--output.json.path",
            "stdout",
            "./...",
        ]
    );
}

#[test]
fn go_tool_golangci_v1_run_is_forwarded_once() {
    assert_eq!(
        golangci_argv(1, &["run", "./..."]),
        ["tool", "golangci-lint", "run", "--out-format=json", "./..."]
    );
}

#[test]
fn go_tool_golangci_shorthand_keeps_implicit_run() {
    assert_eq!(
        golangci_argv(2, &["./..."]),
        [
            "tool",
            "golangci-lint",
            "run",
            "--output.json.path",
            "stdout",
            "./...",
        ]
    );
    assert_eq!(
        golangci_argv(1, &["./..."]),
        ["tool", "golangci-lint", "run", "--out-format=json", "./..."]
    );
}

#[test]
fn go_tool_golangci_keeps_explicit_output_flags() {
    assert_eq!(
        golangci_argv(2, &["run", "--output.json.path=stdout", "./..."]),
        [
            "tool",
            "golangci-lint",
            "run",
            "--output.json.path=stdout",
            "./...",
        ]
    );
    assert_eq!(
        golangci_argv(1, &["run", "--out-format=json", "./..."]),
        ["tool", "golangci-lint", "run", "--out-format=json", "./..."]
    );
}

#[test]
fn go_tool_golangci_preserves_run_as_a_package_argument() {
    assert_eq!(
        golangci_argv(2, &["run", "run"]),
        [
            "tool",
            "golangci-lint",
            "run",
            "--output.json.path",
            "stdout",
            "run",
        ]
    );
    assert_eq!(
        golangci_argv(2, &["--", "run"]),
        [
            "tool",
            "golangci-lint",
            "run",
            "--output.json.path",
            "stdout",
            "--",
            "run",
        ]
    );
}
