//! Exercise the actual CLI and runner with a pytest stub. The failure report was
//! captured from pytest 9.1.1; native argument-file expansion is pytest's job.
//! Capture: pytest @args.txt -q test_failures.py, with --tb=no in args.txt.
//! CPython 3.12.3, sys capture, external plugins disabled; only duration normalized.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Output;

mod common;

const FAILURE_REPORT: &str = include_str!("fixtures/pytest_argsfile_tb_no_raw.txt");

fn invoke(
    args: &[&str],
    stdout: &str,
    stderr: &str,
    exit: i32,
    python_fallback: bool,
) -> (Output, Vec<String>) {
    let dir = common::tempdir();
    let stub = dir
        .path()
        .join(if python_fallback { "python" } else { "pytest" });
    fs::write(
        &stub,
        r#"#!/bin/sh
printf 'called\n' >> "$RTK_TEST_CALLS"
printf '%s\000' "$@" > "$RTK_TEST_ARGV"
/bin/cat "$RTK_TEST_STDOUT"
/bin/cat "$RTK_TEST_STDERR" >&2
exit "$RTK_TEST_EXIT"
"#,
    )
    .expect("write pytest stub");
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("chmod stub");
    fs::write(dir.path().join("stdout"), stdout).expect("write stdout fixture");
    fs::write(dir.path().join("stderr"), stderr).expect("write stderr fixture");

    let out = common::rtk_command()
        .arg("pytest")
        .args(args)
        .current_dir(dir.path())
        // The fallback case must not find a real pytest elsewhere on PATH.
        .env("PATH", dir.path())
        .env("RTK_TEST_CALLS", dir.path().join("calls"))
        .env("RTK_TEST_ARGV", dir.path().join("argv"))
        .env("RTK_TEST_STDOUT", dir.path().join("stdout"))
        .env("RTK_TEST_STDERR", dir.path().join("stderr"))
        .env("RTK_TEST_EXIT", exit.to_string())
        .output()
        .expect("run rtk pytest");
    let captured = fs::read_to_string(dir.path().join("argv"))
        .unwrap_or_else(|err| panic!("read argv: {err}; output: {out:?}"));
    assert_eq!(
        fs::read_to_string(dir.path().join("calls")).expect("read invocation count"),
        "called\n",
        "pytest must run exactly once"
    );
    (
        out,
        captured
            .split_terminator('\0')
            .map(str::to_string)
            .collect(),
    )
}

#[test]
fn argument_file_preserves_failure_report() {
    let args = ["@args.txt", "test_failures.py"];
    let (out, argv) = invoke(&args, FAILURE_REPORT, "", 1, false);
    assert_eq!(argv, args);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert_eq!(out.stdout, FAILURE_REPORT.as_bytes());
    assert!(out.stderr.is_empty(), "{out:?}");
}

#[test]
fn argument_file_paths_keep_their_boundaries() {
    let args = [
        "@arguments with spaces.txt",
        "tests with spaces/test_example.py",
    ];
    let (out, argv) = invoke(&args, FAILURE_REPORT, "", 1, false);
    assert_eq!(argv, args);
    assert_eq!(out.stdout, FAILURE_REPORT.as_bytes());
}

#[test]
fn argument_file_option_value_and_delimiter_are_preserved() {
    for args in [vec!["-k", "@selection.txt"], vec!["-q", "--", "@args.txt"]] {
        let (out, argv) = invoke(&args, FAILURE_REPORT, "", 1, false);
        assert_eq!(argv, args);
        assert_eq!(out.stdout, FAILURE_REPORT.as_bytes());
        assert_eq!(out.status.code(), Some(1), "{out:?}");
    }
}

#[test]
fn argument_file_uses_python_module_when_pytest_is_not_on_path() {
    let (out, argv) = invoke(
        &["@args.txt", "test_failures.py"],
        FAILURE_REPORT,
        "",
        1,
        true,
    );
    assert_eq!(argv, ["-m", "pytest", "@args.txt", "test_failures.py"]);
    assert_eq!(out.stdout, FAILURE_REPORT.as_bytes());
    assert_eq!(out.status.code(), Some(1), "{out:?}");
}

#[test]
fn argument_file_error_keeps_empty_stdout_and_stderr() {
    let error = "ERROR: missing argument file\n";
    let (out, argv) = invoke(&["@missing.txt"], "", error, 4, false);
    assert_eq!(argv, ["@missing.txt"]);
    assert!(out.stdout.is_empty(), "{out:?}");
    assert_eq!(out.stderr, error.as_bytes());
    assert_eq!(out.status.code(), Some(4), "{out:?}");
}

#[test]
fn argument_file_output_is_not_truncated() {
    let raw = FAILURE_REPORT.repeat(100);
    assert!(raw.len() > 20_000);
    let (out, argv) = invoke(&["@args.txt"], &raw, "", 1, false);
    assert_eq!(argv, ["@args.txt"]);
    assert_eq!(out.stdout, raw.as_bytes());
    assert_eq!(out.status.code(), Some(1), "{out:?}");
}

#[test]
fn ordinary_calls_still_use_the_filter() {
    let raw = ".                                                                        [100%]\n1 passed in 0.01s\n";
    let (out, argv) = invoke(&["test_ok.py"], raw, "", 0, false);
    assert_eq!(argv, ["--tb=short", "-q", "-rxX", "test_ok.py"]);
    assert_eq!(out.stdout, b"Pytest: 1 passed in 0.01s\n");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
}
