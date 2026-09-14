#![cfg(unix)]
//! Regression for #2097: `rtk init -g --agent cursor` must only touch `.cursor/`
//! and never create or patch Claude Code files under `~/.claude`.

use std::process::{Command, Stdio};

fn rtk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
}

#[test]
fn agent_cursor_dry_run_never_touches_claude() {
    let home = tempfile::tempdir().unwrap();
    let out = rtk()
        .args(["init", "-g", "--agent", "cursor", "--dry-run"])
        .env("HOME", home.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("/.claude/"),
        "cursor init must not touch .claude:\n{stdout}"
    );
    assert!(
        stdout.contains("/.cursor/"),
        "cursor init must patch .cursor hooks.json:\n{stdout}"
    );
    // Dry-run must not create anything on disk.
    assert!(
        !home.path().join(".claude").exists(),
        "dry-run must not create ~/.claude"
    );
    assert!(
        !home.path().join(".cursor").exists(),
        "dry-run must not create ~/.cursor"
    );
}

#[test]
fn agent_cursor_real_write_creates_no_claude_dir() {
    // The original #2097 failure was a hard error while creating a temp file
    // under a non-existent ~/.claude. Prove a real (non-dry-run) install never
    // creates .claude and does write .cursor/hooks.json.
    let home = tempfile::tempdir().unwrap();
    let out = rtk()
        .args(["init", "-g", "--agent", "cursor", "--auto-patch"])
        .env("HOME", home.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "cursor init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !home.path().join(".claude").exists(),
        "cursor init must not create ~/.claude"
    );
    assert!(
        home.path().join(".cursor").join("hooks.json").exists(),
        "cursor init must write .cursor/hooks.json"
    );
}

#[test]
fn default_dry_run_still_installs_claude() {
    let home = tempfile::tempdir().unwrap();
    let out = rtk()
        .args(["init", "-g", "--dry-run"])
        .env("HOME", home.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("/.claude/"),
        "default init must still install Claude Code files:\n{stdout}"
    );
}
