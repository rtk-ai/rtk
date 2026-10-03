//! Byte-exactness of `rtk diff -`'s structural fallback.
//!
//! When the piped stream is not a parseable diff, `rtk diff -` falls back to
//! emitting the input bytes verbatim. That fallback is documented as
//! byte-exact (condense_stdin's `None` contract), so it must not append a
//! newline the native pipe would not produce.

use std::io::Write;
use std::process::{Command, Output, Stdio};

fn rtk_diff_stdin(input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("LC_ALL", "C")
        .args(["diff", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk diff");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input)
        .expect("write stdin");
    child.wait_with_output().expect("wait for rtk diff")
}

#[test]
fn fallback_is_byte_exact_when_input_has_no_trailing_newline() {
    let input: &[u8] = b"not a diff at all";
    let out = rtk_diff_stdin(input);

    assert!(
        out.status.success(),
        "raw fallback must succeed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        out.stdout, input,
        "fallback must emit the exact input bytes with no trailing newline"
    );
}

#[test]
fn fallback_round_trips_input_with_trailing_newline() {
    let input: &[u8] = b"not a diff at all\n";
    let out = rtk_diff_stdin(input);

    assert!(out.status.success());
    assert_eq!(
        out.stdout, input,
        "newline-terminated input must round-trip byte-for-byte"
    );
}

#[test]
fn fallback_preserves_non_utf8_bytes() {
    // 0xFF is not valid UTF-8; the raw fallback must not try to re-encode it.
    let input: &[u8] = &[b'a', b'b', 0xFF, b'\n'];
    let out = rtk_diff_stdin(input);

    assert!(out.status.success());
    assert_eq!(
        out.stdout, input,
        "non-UTF-8 bytes must pass through untouched"
    );
}
