#![cfg(unix)]

use std::io::Write;
use std::process::{Command, Output, Stdio};

fn run_with_stdin(command: &mut Command, input: &[u8]) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn command");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(input)
        .expect("write stdin");
    child.wait_with_output().expect("wait for command")
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
