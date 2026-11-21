#![cfg(windows)]
//! Windows-only end-to-end coverage.
//!
//! The rest of `tests/` is gated `#![cfg(unix)]`, so before this file the
//! Windows CI target compiled the binary and then exercised none of it. These
//! are smoke tests, not exhaustive ones: they pin the behaviours that are
//! genuinely different on Windows — CRLF line endings, backslash paths, paths
//! containing spaces, and which shell `rtk summary` hands a command to.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// Run rtk with its tracking DB redirected into `dir` and telemetry off, so a
/// test run never writes to the developer's own history. (`RTK_DATA_DIR` is a
/// compile-time constant, not an environment variable, so the config file
/// location cannot be redirected — these tests only ever read it.)
fn rtk_in(dir: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_rtk"));
    c.env("RTK_DB_PATH", dir.join("history.db"))
        .env("RTK_TELEMETRY_DISABLED", "1");
    c
}

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn run_with_stdin(command: &mut Command, input: &[u8]) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(input)
        .expect("write stdin");
    child.wait_with_output().expect("wait for rtk")
}

// --- basic binary health -------------------------------------------------

#[test]
fn version_runs_on_windows() {
    let dir = tempfile::tempdir().unwrap();
    let out = rtk_in(dir.path()).arg("--version").output().unwrap();
    assert!(out.status.success(), "rtk --version failed");
    assert!(
        stdout_of(&out).contains(env!("CARGO_PKG_VERSION")),
        "version output missing crate version: {}",
        stdout_of(&out)
    );
}

// --- CRLF handling -------------------------------------------------------

#[test]
fn wc_counts_crlf_lines_from_stdin() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_with_stdin(rtk_in(dir.path()).args(["wc", "-l"]), b"alpha\r\nbeta\r\n");
    assert!(out.status.success(), "rtk wc -l failed");
    assert_eq!(stdout_of(&out).trim(), "2");
}

#[test]
fn read_does_not_drop_crlf_content() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("crlf.txt");
    std::fs::write(&file, "first\r\nsecond\r\nthird\r\n").unwrap();

    let out = rtk_in(dir.path())
        .args(["read", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "rtk read failed");

    let text = stdout_of(&out);
    for expected in ["first", "second", "third"] {
        assert!(text.contains(expected), "missing {expected} in:\n{text}");
    }
}

// --- Windows paths -------------------------------------------------------

#[test]
fn ls_handles_backslash_path_with_spaces() {
    let dir = tempfile::tempdir().unwrap();
    let spaced = dir.path().join("Program Files Test");
    std::fs::create_dir(&spaced).unwrap();
    std::fs::write(spaced.join("alpha.txt"), "a").unwrap();
    std::fs::write(spaced.join("beta.txt"), "b").unwrap();

    let arg = spaced.to_str().unwrap();
    assert!(arg.contains('\\'), "temp path was not a backslash path");

    let out = rtk_in(dir.path()).args(["ls", arg]).output().unwrap();
    assert!(out.status.success(), "rtk ls failed on {arg}");

    let text = stdout_of(&out);
    assert!(text.contains("alpha.txt"), "alpha.txt missing in:\n{text}");
    assert!(text.contains("beta.txt"), "beta.txt missing in:\n{text}");
}

// --- shell selection (see src/core/shell.rs) -----------------------------

#[test]
fn summary_runs_cmd_builtins() {
    let dir = tempfile::tempdir().unwrap();
    let out = rtk_in(dir.path())
        .args(["summary", "echo RTK_CMD_OK"])
        .output()
        .unwrap();
    assert!(out.status.success(), "rtk summary of a cmd builtin failed");
    assert!(
        stdout_of(&out).contains("RTK_CMD_OK"),
        "cmd builtin output missing:\n{}",
        stdout_of(&out)
    );
}

// --- hooks ---------------------------------------------------------------

#[test]
fn init_show_reports_configuration_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let out = rtk_in(dir.path())
        .current_dir(dir.path())
        // Report on a throwaway config dir rather than the developer's own.
        .env("CLAUDE_CONFIG_DIR", dir.path().join(".claude"))
        .args(["init", "--show"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "rtk init --show failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !dir.path().join("CLAUDE.md").exists(),
        "init --show must not write CLAUDE.md"
    );
}
