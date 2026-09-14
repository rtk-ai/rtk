#![cfg(unix)]
//! `rtk curl` must surface curl's own error message on failure: a bare `-s`
//! suppresses error output, so a DNS failure / connection refused printed only
//! "FAILED: curl " with no reason. `-sS` keeps the progress bar off while
//! re-enabling error messages.

use std::process::Command;

fn curl_available() -> bool {
    Command::new("curl")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn curl_failure_surfaces_reason() {
    if !curl_available() {
        eprintln!("curl not installed; skipping");
        return;
    }

    // Port 1 on localhost: refused immediately, no network needed.
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["curl", "http://127.0.0.1:1/"])
        .output()
        .expect("run rtk curl");

    assert!(
        !out.status.success(),
        "connection to port 1 should fail, got exit {:?}",
        out.status.code()
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    let failed_line = stderr
        .lines()
        .find(|l| l.contains("FAILED: curl"))
        .unwrap_or_else(|| panic!("no FAILED line in stderr: {stderr}"));

    // With plain -s the line was exactly "FAILED: curl " — no reason at all.
    assert!(
        failed_line.trim_end() != "FAILED: curl",
        "curl error message swallowed, got only: {failed_line:?}"
    );
    assert!(
        failed_line.contains("curl:") || failed_line.to_lowercase().contains("connect"),
        "expected curl's own error text on the FAILED line, got: {failed_line:?}"
    );
}
