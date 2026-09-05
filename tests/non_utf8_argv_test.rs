#![cfg(unix)]
//! Non-UTF-8 bytes in argv must never abort rtk (#3025). Clap rejects such args as
//! InvalidUtf8, which routes them into the raw-execution fallback — so that fallback
//! must read argv without assuming UTF-8, and must forward the original bytes to the
//! wrapped tool rather than a lossy reinterpretation of them.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::process::Command;

/// Leading bytes of a double-encoded UTF-8 sequence: invalid UTF-8 on its own, and
/// the kind of fragment you grep for when hunting mojibake. From the issue repro.
const INVALID_PATTERN: &[u8] = b"\xc3\xa2\xe2\x80";

/// A line the pattern above matches, written as raw bytes.
const MATCHING_LINE: &[u8] = b"\xc3\xa2\xe2\x80\xa0\n";

fn os(bytes: &[u8]) -> OsString {
    OsString::from_vec(bytes.to_vec())
}

fn rtk(args: &[OsString]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(args)
        .output()
        .expect("rtk")
}

#[test]
fn non_utf8_pattern_does_not_abort() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("mojibake.txt");
    std::fs::write(&f, MATCHING_LINE).unwrap();

    let out = rtk(&[
        os(b"grep"),
        os(b"-c"),
        os(INVALID_PATTERN),
        f.clone().into_os_string(),
    ]);

    // Before the fix this aborted: SIGABRT (code() == None) under the release
    // profile's panic="abort", exit 101 under a debug build.
    assert_eq!(
        out.status.code(),
        Some(0),
        "expected a clean exit, got {:?} — stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("panicked"),
        "must not panic: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn non_utf8_pattern_forwards_original_bytes() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("mojibake.txt");
    std::fs::write(&f, MATCHING_LINE).unwrap();

    let rtk_out = rtk(&[
        os(b"grep"),
        os(b"-c"),
        os(INVALID_PATTERN),
        f.clone().into_os_string(),
    ]);

    // The bytes must reach the engine intact: a lossy conversion would search for
    // replacement characters instead and silently report zero matches.
    let native = Command::new("grep")
        .args([os(b"-c"), os(INVALID_PATTERN), f.into_os_string()])
        .env("LC_ALL", "C")
        .output()
        .expect("grep");

    assert_eq!(
        String::from_utf8_lossy(&rtk_out.stdout).trim(),
        String::from_utf8_lossy(&native.stdout).trim(),
        "rtk must report the same match count as native grep"
    );
}

#[test]
fn non_utf8_arg_to_missing_command_exits_cleanly() {
    // The fallback's spawn-failure path also walks argv; it must degrade to the
    // usual 127 rather than aborting.
    let out = rtk(&[os(b"rtk_no_such_command_xyz"), os(INVALID_PATTERN)]);

    assert_eq!(out.status.code(), Some(127), "expected command-not-found");
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("panicked"),
        "must not panic: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
