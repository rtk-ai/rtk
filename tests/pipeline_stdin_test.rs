#![cfg(unix)]

use std::io::{ErrorKind, Write};
use std::process::{Command, Output, Stdio};

fn run_with_stdin(command: &mut Command, input: &[u8]) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn command");
    if let Err(error) = child.stdin.take().expect("piped stdin").write_all(input) {
        assert_eq!(error.kind(), ErrorKind::BrokenPipe, "write stdin: {error}");
    }
    child.wait_with_output().expect("wait for command")
}

#[test]
fn early_exit_preserves_output_and_status_when_stdin_is_closed() {
    // More than a pipe buffer ensures the writer observes the closed reader,
    // regardless of whether the child exits before or during write_all.
    let input = vec![b'x'; 8 * 1024 * 1024];
    let output = run_with_stdin(
        Command::new("sh").args(["-c", "exec 0<&-; printf 'early exit\\n'; exit 7"]),
        &input,
    );

    assert_eq!(output.status.code(), Some(7));
    assert_eq!(output.stdout, b"early exit\n");
}

#[test]
fn wc_reads_piped_stdin() {
    let output = run_with_stdin(
        Command::new(env!("CARGO_BIN_EXE_rtk")).args(["wc", "-l"]),
        b"alpha\nbeta\n",
    );

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "2");
}

#[test]
fn wc_preserves_native_failure_exit_code() {
    let invalid_option = "--definitely-invalid-rtk-test-option";
    let rtk = run_with_stdin(
        Command::new(env!("CARGO_BIN_EXE_rtk")).args(["wc", invalid_option]),
        b"input\n",
    );
    let native = run_with_stdin(Command::new("wc").arg(invalid_option), b"input\n");

    assert!(!rtk.status.success());
    assert_eq!(rtk.status.code(), native.status.code());
}
