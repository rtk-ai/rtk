#![cfg(unix)]
//! `rtk curl` must surface curl's own error message on failure: a bare `-s`
//! suppresses error output, so a DNS failure / connection refused printed only
//! "FAILED: curl " with no reason. `-sS` keeps the progress bar off while
//! re-enabling error messages.

use std::fs;
use std::os::unix::fs::PermissionsExt;
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

/// `-S` only adds curl's reason; a response body that came with the failure
/// (`--fail-with-body`) must not be dropped in its favour.
#[test]
fn curl_failure_keeps_the_response_body_next_to_the_reason() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Acts like `curl -sS --fail-with-body` on a 401: the reason goes to stderr
    // only when -S was passed, the body goes to stdout, exit code 22.
    let shim = dir.path().join("curl");
    fs::write(
        &shim,
        "#!/bin/sh\n\
         case \"$1\" in *S*) echo 'curl: (22) The requested URL returned error: 401' >&2;; esac\n\
         printf '%s' '{\"error\":\"invalid token\"}'\n\
         exit 22\n",
    )
    .expect("write fake curl");
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).expect("chmod fake curl");

    let path = format!(
        "{}:{}",
        dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["curl", "--fail-with-body", "http://example.invalid/"])
        .env("PATH", path)
        .output()
        .expect("run rtk curl");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(22), "curl's exit code must survive");
    assert!(
        stderr.contains("curl: (22)"),
        "curl's reason is missing, got: {stderr}"
    );
    assert!(
        stderr.contains(r#"{"error":"invalid token"}"#),
        "the response body explaining the failure was dropped, got: {stderr}"
    );
}
