//! Windows twin of the `--help` cases in `help_forwarding_test.rs` (which is
//! `#![cfg(unix)]`): a forwarded `--help` on a captured tool is shown as the
//! tool prints it, through a `.cmd` shim found via PATH.
#![cfg(windows)]
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn fake_cmd_tool(dir: &Path, name: &str, body: &str) {
    fs::write(
        dir.join(format!("{name}.cmd")),
        format!("@echo off\r\n{body}\r\n"),
    )
    .expect("write fake tool");
}

fn rtk_with(dir: &Path, args: &[&str]) -> Output {
    let path = format!(
        "{};{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(args)
        .env("PATH", path)
        .env("RTK_DB_PATH", dir.join("rtk-test.db"))
        .output()
        .expect("run rtk")
}

#[test]
fn forwarded_help_on_a_captured_tool_is_shown_verbatim() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_cmd_tool(
        dir.path(),
        "wget",
        "echo Usage: wget [OPTION]... [URL]...\r\necho Try --help for more options.\r\nexit /b 0",
    );
    let out = rtk_with(dir.path(), &["wget", "http://x/f", "--help"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stdout.contains("Usage: wget [OPTION]"),
        "usage must reach stdout verbatim, got {stdout:?}"
    );
    assert!(
        !stdout.contains(" ok |") && !stdout.to_lowercase().contains("fail"),
        "usage read as a completed download, got {stdout:?}"
    );
}

#[test]
fn forwarded_help_exits_with_the_tools_code() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_cmd_tool(dir.path(), "wget", "echo usage: wget\r\nexit /b 129");
    let out = rtk_with(dir.path(), &["wget", "http://x/f", "--help"]);
    assert_eq!(out.status.code(), Some(129));
}

#[test]
fn forwarded_help_on_a_filtered_tool_is_shown_verbatim() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_cmd_tool(
        dir.path(),
        "cargo",
        "echo Usage: cargo build [OPTIONS]\r\necho   --release  Build artifacts in release mode\r\nexit /b 0",
    );
    let out = rtk_with(dir.path(), &["cargo", "build", "--help"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stdout.contains("Usage: cargo build"),
        "usage must reach stdout verbatim, got {stdout:?}"
    );
    assert!(
        !stdout.contains("crates compiled"),
        "filter summary leaked under the usage, got {stdout:?}"
    );
}

#[test]
fn dash_h_where_the_tool_means_help_is_shown_verbatim() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_cmd_tool(
        dir.path(),
        "pytest",
        "echo usage: pytest [options] [file_or_dir] [file_or_dir] [...]\r\nexit /b 0",
    );
    let out = rtk_with(dir.path(), &["pytest", "-h"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stdout.contains("usage: pytest"),
        "pytest's usage must reach stdout verbatim, got {stdout:?}"
    );
    assert!(
        !stdout.contains("No tests collected"),
        "usage read as an empty run, got {stdout:?}"
    );
}

#[test]
fn dash_h_where_the_tool_defines_it_stays_filtered() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_cmd_tool(
        dir.path(),
        "psql",
        "echo  id ^| name\r\necho ----+------\r\necho   1 ^| a\r\necho (1 row)\r\nexit /b 0",
    );
    let out = rtk_with(dir.path(), &["psql", "-h", "localhost", "-c", "select 1"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stdout.contains("1\ta"),
        "table must be filtered to tab-separated rows, got {stdout:?}"
    );
    assert!(
        !stdout.contains("(1 row)") && !stdout.contains("----+"),
        "`-h host` was treated as a usage request and shown verbatim, got {stdout:?}"
    );
}

#[test]
fn forwarded_help_through_the_package_runner_is_shown_verbatim() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_cmd_tool(
        dir.path(),
        "npx",
        "echo Usage: playwright test [options] [test-filter...]\r\nexit /b 0",
    );
    let out = rtk_with(dir.path(), &["playwright", "test", "--help"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stdout.contains("Usage: playwright test"),
        "usage must reach stdout verbatim, got {stdout:?}"
    );
    assert!(
        stderr.trim().is_empty(),
        "usage was handed to the filter, got stderr {stderr:?}"
    );
}
