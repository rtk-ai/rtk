//! End-to-end tests for `rtk cargo test`: a failed run that shows no failure block reaches the
//! user as the command printed it, each stream on its own stream, with its exit code, while a
//! passing run, a run with failure blocks and a compile error are summarised. Stubs `cargo` on
//! PATH with a script that prints a fixed stdout and a fixed stderr.

#![cfg(unix)]

mod common;

const NOCAPTURE_STDOUT: &str = include_str!("fixtures/cargo_test_nocapture_failed_stdout_raw.txt");
const NOCAPTURE_STDERR: &str = include_str!("fixtures/cargo_test_nocapture_failed_stderr_raw.txt");
const SINGLE_THREAD_STDOUT: &str =
    include_str!("fixtures/cargo_test_nocapture_single_thread_stdout_raw.txt");
const SINGLE_THREAD_STDERR: &str =
    include_str!("fixtures/cargo_test_nocapture_single_thread_stderr_raw.txt");
const LOGGING_STDOUT: &str = include_str!("fixtures/cargo_test_nocapture_logging_stdout_raw.txt");
const LOGGING_STDERR: &str = include_str!("fixtures/cargo_test_nocapture_logging_stderr_raw.txt");
const ABORT_STDOUT: &str = include_str!("fixtures/cargo_test_abort_only_stdout_raw.txt");
const ABORT_STDERR: &str = include_str!("fixtures/cargo_test_abort_only_stderr_raw.txt");
const BLOCKS_STDOUT: &str = include_str!("fixtures/cargo_test_failed_blocks_stdout_raw.txt");
const BLOCKS_STDERR: &str = include_str!("fixtures/cargo_test_failed_blocks_stderr_raw.txt");

/// The three `--nocapture` runs are longer than the recovery store's floor, so a summarised run
/// would get a recall hint: matching their streams byte for byte also shows none is added.
#[test]
fn failed_runs_without_a_failure_block_are_shown_as_printed() {
    for (name, stdout, stderr) in [
        ("--nocapture", NOCAPTURE_STDOUT, NOCAPTURE_STDERR),
        (
            "--nocapture --test-threads=1",
            SINGLE_THREAD_STDOUT,
            SINGLE_THREAD_STDERR,
        ),
        (
            "--nocapture, a test logging to stderr",
            LOGGING_STDOUT,
            LOGGING_STDERR,
        ),
        ("a test binary that aborts", ABORT_STDOUT, ABORT_STDERR),
    ] {
        let outcome = common::run_with_cargo_stub(&["test"], stdout, stderr, 101);
        assert_eq!(outcome.stdout, stdout, "stdout of {name}");
        assert_eq!(outcome.stderr, stderr, "stderr of {name}");
        assert_eq!(outcome.code, Some(101), "exit code of {name}");
    }
}

#[test]
fn failed_run_with_failure_blocks_is_summarised() {
    let outcome = common::run_with_cargo_stub(&["test"], BLOCKS_STDOUT, BLOCKS_STDERR, 101);
    let shown = format!("{}{}", outcome.stdout, outcome.stderr);
    assert_eq!(outcome.code, Some(101));
    assert!(shown.contains("lib reason"), "got: {shown}");
    assert!(shown.contains("it reason"), "got: {shown}");
    assert!(
        shown.contains("test result: FAILED. 0 passed; 1 failed;"),
        "got: {shown}"
    );
    assert!(!shown.contains("Compiling"), "got: {shown}");
}

/// The summary goes to the stream the last line came from, which depends on which pipe is read
/// last, so these only look at what the two streams show together.
#[test]
fn passing_run_is_summarised() {
    let outcome = common::run_with_cargo_stub(
        &["test"],
        concat!(
            "\n",
            "running 2 tests\n",
            "test tests::a ... ok\n",
            "test tests::b ... ok\n",
            "\n",
            "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n",
            "\n",
        ),
        concat!(
            "    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.01s\n",
            "     Running unittests src/lib.rs (target/debug/deps/x-0123456789abcdef)\n",
        ),
        0,
    );
    assert_eq!(
        format!("{}{}", outcome.stdout, outcome.stderr),
        "cargo test: 2 passed (1 suite, 0.00s)\n"
    );
    assert_eq!(outcome.code, Some(0));
}

#[test]
fn compile_error_is_summarised() {
    let outcome = common::run_with_cargo_stub(
        &["test"],
        "",
        concat!(
            "   Compiling x v0.1.0 (/tmp/x)\n",
            "error[E0425]: cannot find value `m` in this scope\n",
            " --> src/lib.rs:1:1\n",
            "  |\n",
            "1 | m\n",
            "  | ^ not found in this scope\n",
            "\n",
            "For more information about this error, try `rustc --explain E0425`.\n",
            "error: could not compile `x` (lib test) due to 1 previous error\n",
        ),
        101,
    );
    let shown = format!("{}{}", outcome.stdout, outcome.stderr);
    assert!(shown.starts_with("cargo test: 1 errors"), "got: {shown}");
    assert_eq!(outcome.code, Some(101));
}
