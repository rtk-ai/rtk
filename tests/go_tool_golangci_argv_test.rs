//! Pin the arguments passed through the public go-tool CLI to golangci-lint.

#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::PermissionsExt;
use std::process::Output;

mod common;

fn shell_quote(path: &std::path::Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn golangci_output(
    version: u32,
    args: &[OsString],
    stdout: &str,
    stderr: &str,
    exit_code: i32,
) -> (Output, Vec<OsString>) {
    let dir = common::tempdir();
    let argv_file = dir.path().join("argv.bin");
    let stdout_file = dir.path().join("stdout.txt");
    let stderr_file = dir.path().join("stderr.txt");
    fs::write(&stdout_file, stdout).expect("write stdout fixture");
    fs::write(&stderr_file, stderr).expect("write stderr fixture");
    let stub = dir.path().join("go");
    fs::write(
        &stub,
        format!(
            "#!/bin/sh\nprintf '%s\\0' \"$@\" >> {}\nif [ \"$3\" = \"--version\" ]; then\n  echo 'golangci-lint has version {version}.0.0'\n  exit 0\nfi\n/bin/cat {}\n/bin/cat {} >&2\nexit {exit_code}\n",
            shell_quote(&argv_file),
            shell_quote(&stdout_file),
            shell_quote(&stderr_file),
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
    let mut bytes = fs::read(&argv_file).expect("read captured go arguments");
    assert_eq!(bytes.pop(), Some(0));
    let argv = bytes
        .split(|byte| *byte == 0)
        .map(|arg| OsString::from_vec(arg.to_vec()))
        .collect();
    (output, argv)
}

fn golangci_argv(version: u32, args: &[&str]) -> Vec<String> {
    let args: Vec<_> = args.iter().map(OsString::from).collect();
    let (output, argv) = golangci_output(
        version,
        &args,
        include_str!("fixtures/golangci_v2_issues_raw.json"),
        "",
        1,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stdout={stdout}; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("errcheck"), "{stdout}");
    assert_eq!(&argv[..3], ["tool", "golangci-lint", "--version"]);
    argv[3..]
        .iter()
        .map(|arg| arg.clone().into_string().expect("UTF-8 test argument"))
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
fn go_tool_golangci_global_flags_before_run_are_preserved() {
    for version in [1, 2] {
        for globals in [
            vec!["-c", ".golangci.yml"],
            vec!["--color", "never"],
            vec!["--config=run"],
            vec!["--config", "fmt"],
        ] {
            let mut input = globals.clone();
            input.extend(["run", "./..."]);
            let mut expected = vec!["tool", "golangci-lint"];
            expected.extend(globals);
            expected.push("run");
            expected.extend(if version == 2 {
                vec!["--output.json.path", "stdout"]
            } else {
                vec!["--out-format=json"]
            });
            expected.push("./...");
            assert_eq!(golangci_argv(version, &input), expected);
        }
    }
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

#[test]
fn go_tool_golangci_non_run_subcommands_pass_through() {
    for args in [
        vec!["fmt", "./..."],
        vec!["--color", "never", "fmt", "./..."],
        vec!["--config", "run", "fmt", "./..."],
        vec!["version"],
        vec!["help"],
    ] {
        let args: Vec<_> = args.into_iter().map(OsString::from).collect();
        let (output, argv) = golangci_output(2, &args, "formatted source\n", "format warning\n", 1);
        let mut expected = vec![OsString::from("tool"), OsString::from("golangci-lint")];
        expected.extend(args);
        // No version probe, inserted run/JSON flags, filtering, or lint exit-code mapping.
        assert_eq!(argv, expected);
        assert_eq!(output.stdout, b"formatted source\n");
        assert_eq!(output.stderr, b"format warning\n");
        assert_eq!(output.status.code(), Some(1));
    }
}

#[test]
fn go_tool_golangci_preserves_non_utf8_arguments() {
    let config = OsString::from_vec(b"config-\xff.yml".to_vec());
    let package = OsString::from_vec(b"./package-\xfe".to_vec());
    let args = vec![
        OsString::from("-c"),
        config.clone(),
        OsString::from("run"),
        package.clone(),
    ];
    let (output, argv) = golangci_output(
        2,
        &args,
        include_str!("fixtures/golangci_v2_issues_raw.json"),
        "",
        1,
    );
    assert!(output.status.success());
    assert_eq!(
        argv,
        vec![
            "tool".into(),
            "golangci-lint".into(),
            "--version".into(),
            "tool".into(),
            "golangci-lint".into(),
            "-c".into(),
            config,
            "run".into(),
            "--output.json.path".into(),
            "stdout".into(),
            package,
        ]
    );
}
