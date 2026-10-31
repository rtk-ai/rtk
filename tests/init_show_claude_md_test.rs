#![cfg(unix)]
//! Regression for #4650: `rtk init --show` must only report the global
//! CLAUDE.md as configured when `@RTK.md` is a line of its own, not when a
//! sentence merely mentions it.

use std::fs;
use std::process::Stdio;

mod common;

fn show(claude_md: &str) -> String {
    let home = common::tempdir();
    let claude_dir = home.path().join(".claude");
    fs::create_dir_all(&claude_dir).unwrap();
    fs::write(claude_dir.join("CLAUDE.md"), claude_md).unwrap();

    let out = common::rtk_command()
        .args(["init", "--show"])
        .env("HOME", home.path())
        .env("CLAUDE_CONFIG_DIR", &claude_dir)
        .env("LC_ALL", "C")
        .current_dir(home.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "rtk init --show failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn show_reports_prose_mention_as_not_configured() {
    let stdout = show("# Notes\n\n@RTK.md is my notes file.\n");
    assert!(
        stdout.contains("Global (~/.claude/CLAUDE.md): exists but rtk not configured"),
        "{stdout}"
    );
}

#[test]
fn show_reports_reference_line_as_configured() {
    let stdout = show("# Notes\n\n@RTK.md\n");
    assert!(
        stdout.contains("[ok] Global (~/.claude/CLAUDE.md): @RTK.md reference"),
        "{stdout}"
    );
}
