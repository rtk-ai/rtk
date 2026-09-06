#![cfg(unix)]
//! A failing tool's stderr must reach the user (#3026). Filters that compress
//! stdout run with `RunOptions::stdout_only`, which keeps stderr out of the filter
//! input — so unless the runner emits it, the diagnostics are dropped and the exit
//! code becomes the only surviving signal.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

const DIAGNOSTIC: &str = "shim-diagnostic: dependency is missing";

/// A fake `ruff` that fails the way real tools do: message on stderr, nothing on
/// stdout, nonzero exit. `rtk ruff` filters stdout only, which is the path under
/// test; `resolved_command` resolves via PATH, so the shim takes precedence.
fn shim(exit_code: i32) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("ruff");
    fs::write(
        &bin,
        format!("#!/bin/sh\necho '{DIAGNOSTIC}' >&2\nexit {exit_code}\n"),
    )
    .unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

fn rtk_ruff(shim_dir: &tempfile::TempDir, db: &tempfile::TempDir) -> std::process::Output {
    let path = format!(
        "{}:{}",
        shim_dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["ruff", "check", "."])
        .env("PATH", path)
        // Keep tracking off the developer's real database.
        .env("RTK_DB_PATH", db.path().join("t.db"))
        .output()
        .expect("rtk")
}

#[test]
fn failing_tool_stderr_reaches_the_user() {
    let db = tempfile::tempdir().unwrap();
    let out = rtk_ruff(&shim(1), &db);

    assert_eq!(out.status.code(), Some(1), "exit code must propagate");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains(DIAGNOSTIC),
        "diagnostic swallowed — stderr was {:?}, stdout was {:?}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn successful_tool_stderr_stays_suppressed() {
    // On success stderr is the incidental noise RTK exists to compress away, and
    // the filtered stdout already carries the result.
    let db = tempfile::tempdir().unwrap();
    let out = rtk_ruff(&shim(0), &db);

    assert_eq!(out.status.code(), Some(0));
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains(DIAGNOSTIC),
        "successful run must not leak tool stderr"
    );
}
