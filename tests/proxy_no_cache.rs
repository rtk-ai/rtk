//! Regression test for issue #2148:
//! "rtk proxy returns stale/incorrect git + filesystem state (caching not bypassed)"
//!
//! The issue hypothesized that `rtk proxy` serves a cached/stale snapshot of
//! filesystem state instead of re-reading from disk. The proxy handler in
//! `main.rs` execs the underlying binary and streams its real stdout, so it
//! must always reflect the *current* contents of a file — even when the file
//! is overwritten between two consecutive `rtk proxy` invocations.
//!
//! This test writes a file, reads it through `rtk proxy cat`, overwrites it,
//! and reads it again. If proxy cached the first read, the second read would
//! return stale content and the assertion would fail.
//!
//! Unix-only: it relies on `cat`, which is not present on the Windows CI
//! runner. The reported platforms for #2148 are macOS and Linux.

#![cfg(unix)]

use std::fs;
use std::process::Command;

/// Path to the `rtk` binary built by Cargo for integration tests.
fn rtk_bin() -> &'static str {
    env!("CARGO_BIN_EXE_rtk")
}

#[test]
fn proxy_reads_live_file_state_not_cache() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let file = dir.path().join("probe.txt");

    // First state on disk.
    fs::write(&file, "ORIGINAL\n").expect("write original content");
    let first = Command::new(rtk_bin())
        .args(["proxy", "cat", file.to_str().expect("utf8 path")])
        .output()
        .expect("run rtk proxy cat (first)");
    assert!(
        first.status.success(),
        "first `rtk proxy cat` failed: stderr={}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_out = String::from_utf8_lossy(&first.stdout);
    assert!(
        first_out.contains("ORIGINAL"),
        "expected proxy to read the original content, got: {first_out:?}"
    );

    // Mutate the file *outside* of rtk, exactly like the #2148 scenario where
    // the write happened via a different tool than the one rtk routed.
    fs::write(&file, "MUTATED\n").expect("overwrite content");

    let second = Command::new(rtk_bin())
        .args(["proxy", "cat", file.to_str().expect("utf8 path")])
        .output()
        .expect("run rtk proxy cat (second)");
    assert!(
        second.status.success(),
        "second `rtk proxy cat` failed: stderr={}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second_out = String::from_utf8_lossy(&second.stdout);

    // The core of issue #2148: the second read MUST reflect the new on-disk
    // content. A caching layer would still report "ORIGINAL" here.
    assert!(
        second_out.contains("MUTATED"),
        "proxy returned stale content after the file changed on disk \
         (issue #2148 regression): got {second_out:?}"
    );
    assert!(
        !second_out.contains("ORIGINAL"),
        "proxy still shows pre-mutation content — stale state served \
         (issue #2148 regression): got {second_out:?}"
    );
}
