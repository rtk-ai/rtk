//! Regression tests for #3025: non-UTF-8 bytes in argv must not abort the process.
//!
//! Unix-only: argv is arbitrary bytes here, whereas Windows argv is UTF-16.

#![cfg(unix)]

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::process::Command;

/// Leading bytes of a double-encoded UTF-8 sequence — invalid UTF-8 on their own.
fn invalid_utf8_arg() -> OsString {
    OsString::from_vec(vec![0xc3, 0xa2, 0xe2, 0x80])
}

#[test]
fn non_utf8_arg_does_not_abort() {
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("grep")
        .arg("-c")
        .arg(invalid_utf8_arg())
        .arg("/dev/null")
        .output()
        .expect("rtk should spawn");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("panicked"),
        "rtk panicked on non-UTF-8 argv: {stderr}"
    );
    assert!(
        out.status.code().is_some(),
        "rtk was killed by a signal (abort) on non-UTF-8 argv"
    );
}

/// The fallback path executes the wrapped command, so argv bytes must reach it
/// verbatim — a lossy String conversion would silently replace them with U+FFFD.
#[test]
fn non_utf8_arg_reaches_the_wrapped_command_verbatim() {
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("printf")
        .arg("%s")
        .arg(invalid_utf8_arg())
        .output()
        .expect("rtk should spawn");

    assert_eq!(
        out.stdout,
        invalid_utf8_arg().into_vec(),
        "argv bytes were altered before reaching the wrapped command"
    );
}
