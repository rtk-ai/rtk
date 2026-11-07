//! End-to-end coverage of Cargo's boundary with libtest. The shell fixtures are
//! Unix-only; argument classification also has platform-independent unit tests.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Output;

mod common;

fn run(args: &[&str], stdout: &str, stderr: &str, code: i32) -> (Output, Vec<String>) {
    let dir = tempfile::tempdir().expect("create fixture");
    fs::write(dir.path().join("stdout"), stdout).expect("write stdout");
    fs::write(dir.path().join("stderr"), stderr).expect("write stderr");
    let cargo = dir.path().join("cargo");
    fs::write(
        &cargo,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > argv\ncat stdout\ncat stderr >&2\nexit {code}\n"
        ),
    )
    .expect("write cargo");
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).expect("chmod cargo");
    let path = std::env::join_paths(std::iter::once(dir.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("fixture PATH");
    let out = common::rtk_command()
        .args(["cargo", "test"])
        .args(args)
        .env("PATH", path)
        .env("NO_COLOR", "1")
        .env("LC_ALL", "C")
        .current_dir(dir.path())
        .output()
        .expect("run rtk cargo test");
    let argv = fs::read_to_string(dir.path().join("argv"))
        .expect("read captured argv")
        .lines()
        .map(str::to_owned)
        .collect();
    (out, argv)
}

fn assert_listing(args: &[&str], stdout: &str, stderr: &str, code: i32) {
    let (out, argv) = run(args, stdout, stderr, code);
    assert_eq!(out.status.code(), Some(code));
    assert_eq!(out.stdout, stdout.as_bytes());
    assert_eq!(out.stderr, stderr.as_bytes());
    let expected: Vec<_> = std::iter::once("test")
        .chain(args.iter().copied())
        .collect();
    assert_eq!(argv, expected, "Cargo/libtest argument boundary changed");
}

#[test]
fn single_target_listing_preserves_both_streams() {
    assert_listing(
        &["--lib", "--", "--list"],
        "ok: test\n\n1 test, 0 benchmarks\n",
        "    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.01s\n     Running unittests src/lib.rs\n",
        0,
    );
}

#[test]
fn multiple_targets_and_benchmarks_are_not_swallowed() {
    assert_listing(
        &["--all-targets", "--", "--list"],
        "unit::ok: test\n\n1 test, 0 benchmarks\nintegration::ok: test\nsort: benchmark\n\n1 test, 1 benchmark\n",
        "     Running unittests src/lib.rs\n     Running tests/integration.rs\n",
        0,
    );
}

#[test]
fn empty_and_terse_listings_preserve_requested_format() {
    assert_listing(
        &["no_match", "--", "--list"],
        "0 tests, 0 benchmarks\n",
        "",
        0,
    );
    assert_listing(
        &["--", "--list", "--format", "terse"],
        "nested::ok: test\n",
        "",
        0,
    );
}

#[test]
fn compilation_failure_keeps_diagnostics_and_exit_status() {
    assert_listing(
        &["--", "--list"],
        "",
        "error[E0425]: cannot find value `missing` in this scope\n --> src/lib.rs:2:13\n  |\n2 | fn ok() { missing; }\n  |           ^^^^^^^ not found in this scope\nerror: could not compile `fixture` (lib test) due to 1 previous error\n",
        101,
    );
}

#[test]
fn ordinary_runs_and_literal_list_arguments_still_use_test_filter() {
    let raw = "running 2 tests\ntest one ... ok\ntest two ... ok\n\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n\n";
    for args in [
        vec![],
        vec!["--list"],                 // Cargo's own flag region is not libtest's.
        vec!["--", "--skip", "--list"], // A value, not a list flag.
        vec!["--", "--", "--list"],     // A libtest positional after its separator.
        vec!["--", "--skip", "--", "--", "--list"], // Value, then actual separator.
    ] {
        let (out, argv) = run(&args, raw, "", 0);
        assert_eq!(out.status.code(), Some(0));
        assert!(
            out.stdout.len() < raw.len(),
            "test output was not compacted: {args:?}"
        );
        let shown = String::from_utf8_lossy(&out.stdout);
        assert!(shown.contains("2 passed"), "missing test result: {shown}");
        assert!(!shown.contains("test one ... ok"));
        let expected: Vec<_> = std::iter::once("test")
            .chain(args.iter().copied())
            .collect();
        assert_eq!(argv, expected);
    }
}

#[test]
fn listing_preserves_whitespace_and_unterminated_streams() {
    assert_listing(
        &[
            "--manifest-path",
            "project with spaces/Cargo.toml",
            "--",
            "--list",
        ],
        "  nested::ok: test\r\n\r\n1 test, 0 benchmarks",
        "  warning: fixture diagnostic without newline  ",
        0,
    );
    assert_listing(&["--", "--list"], "", "", 0);
}

#[test]
fn libtest_option_can_consume_a_separator_before_list() {
    assert_listing(
        &["--", "--skip", "--", "--list"],
        "ok: test\n\n1 test, 0 benchmarks\n",
        "",
        0,
    );
}
