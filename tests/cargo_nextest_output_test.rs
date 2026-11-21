//! End-to-end tests for `rtk cargo nextest`: a test run is summarised, and output the run filter
//! has no reading of (a listing, a build error, a rejected option) reaches the user as the
//! command printed it, each stream on its own stream, with its exit code. Stubs `cargo` on PATH
//! with a script that prints a fixed stdout and a fixed stderr.

#![cfg(unix)]

mod common;

const LIST_STDOUT: &str = include_str!("fixtures/cargo_nextest_list_stdout_raw.txt");
const LIST_STDERR: &str = include_str!("fixtures/cargo_nextest_list_stderr_raw.txt");
const SHOW_CONFIG_STDOUT: &str = include_str!("fixtures/cargo_nextest_show_config_stdout_raw.txt");
const ARCHIVE_STDERR: &str = include_str!("fixtures/cargo_nextest_archive_stderr_raw.txt");
const COMPILE_ERROR_STDERR: &str =
    include_str!("fixtures/cargo_nextest_run_compile_error_stderr_raw.txt");
const BAD_OPTION_STDERR: &str =
    include_str!("fixtures/cargo_nextest_run_bad_option_stderr_raw.txt");
const INTERRUPTED_STDERR: &str = include_str!("fixtures/cargo_nextest_run_interrupted_raw.txt");
const FAILED_RUN_STDERR: &str = include_str!("fixtures/cargo_nextest_slow_flaky_failed_raw.txt");
const PASSING_COLOUR_STDERR: &str = include_str!("fixtures/cargo_nextest_pass_color_raw.txt");
const FAILING_COLOUR_STDERR: &str = include_str!("fixtures/cargo_nextest_fail_color_raw.txt");

/// Runs `rtk cargo nextest <args>` against a `cargo` stub that prints `stub_stdout` on stdout and
/// `stub_stderr` on stderr, and exits with `stub_code`. nextest prints its run status and its
/// build output on stderr and a listing on stdout, which is the split the stub reproduces.
fn run_with_stub(
    args: &[&str],
    stub_stdout: &str,
    stub_stderr: &str,
    stub_code: i32,
) -> common::Outcome {
    common::run_with_cargo_stub(
        &[&["nextest"], args].concat(),
        stub_stdout,
        stub_stderr,
        stub_code,
    )
}

fn assert_shown_as_printed(args: &[&str], printed_stdout: &str, printed_stderr: &str, code: i32) {
    let outcome = run_with_stub(args, printed_stdout, printed_stderr, code);
    assert_eq!(
        outcome.stdout, printed_stdout,
        "stdout must reach the user verbatim for {args:?}"
    );
    assert_eq!(
        outcome.stderr, printed_stderr,
        "stderr must reach the user verbatim for {args:?}"
    );
    assert_eq!(outcome.code, Some(code), "exit code for {args:?}");
}

/// The listing is the stdout a pipe reads; the `Finished` line stays on stderr.
#[test]
fn list_keeps_its_streams() {
    assert_shown_as_printed(&["list"], LIST_STDOUT, LIST_STDERR, 0);
}

#[test]
fn show_config_is_shown_as_printed() {
    assert_shown_as_printed(&["show-config", "version"], SHOW_CONFIG_STDOUT, "", 0);
}

#[test]
fn archive_report_stays_on_stderr() {
    assert_shown_as_printed(&["archive"], "", ARCHIVE_STDERR, 0);
}

#[test]
fn run_that_fails_to_compile_is_shown_as_printed() {
    assert_shown_as_printed(&["run"], "", COMPILE_ERROR_STDERR, 101);
}

/// Past the recovery store's floor a failed run that was summarised gets a recall hint; one that
/// is printed in full has nothing to recover, so no hint is added to either stream.
#[test]
fn long_failed_run_that_does_not_compile_is_shown_without_a_recall_hint() {
    let long = COMPILE_ERROR_STDERR.repeat(4);
    assert!(long.len() > 1000, "fixture must exceed the recovery floor");
    assert_shown_as_printed(&["run"], "", &long, 101);
}

/// A stream that is only whitespace is part of what the command printed.
#[test]
fn whitespace_only_streams_are_shown_as_printed() {
    assert_shown_as_printed(&["list"], "\n\n", "  \n", 0);
    assert_shown_as_printed(&["list"], " \n", "Finished\n", 0);
}

#[test]
fn rejected_option_is_shown_as_printed() {
    assert_shown_as_printed(&["run", "--bogus"], "", BAD_OPTION_STDERR, 2);
}

#[test]
fn run_without_a_summary_is_shown_as_printed() {
    assert_shown_as_printed(&["run"], "", INTERRUPTED_STDERR, 137);
}

#[test]
fn failed_run_is_summarised() {
    let outcome = run_with_stub(&["run"], "", FAILED_RUN_STDERR, 100);
    assert_eq!(outcome.code, Some(100));
    assert!(
        outcome.stdout.contains("cargo nextest: 2 passed, 1 failed"),
        "run output must be summarised: {}",
        outcome.stdout
    );
    assert!(
        outcome.stdout.len() < FAILED_RUN_STDERR.len(),
        "run output must be smaller than the raw output"
    );
}

#[test]
fn coloured_runs_are_summarised() {
    let passing = run_with_stub(&["run"], "", PASSING_COLOUR_STDERR, 0);
    assert_eq!(
        passing.stdout,
        "cargo nextest: 2 passed, 1 skipped (1 binary, 0.004s)\n"
    );
    assert_eq!(passing.code, Some(0));

    let failing = run_with_stub(&["run"], "", FAILING_COLOUR_STDERR, 100);
    assert!(
        failing
            .stdout
            .contains("cargo nextest: 2 passed, 1 failed (1 binary, 0.103s)"),
        "got: {}",
        failing.stdout
    );
    assert_eq!(failing.code, Some(100));
}
