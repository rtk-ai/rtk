#![cfg(unix)]
//! Regression for #2097: `rtk init -g --agent cursor` must only touch `.cursor/`
//! and never create or patch Claude Code files under `~/.claude`.

use std::path::Path;
use std::process::{Command, Stdio};

/// Runs rtk against `home` with every directory it resolves from the
/// environment pinned inside it, so an exported `CLAUDE_CONFIG_DIR` or `XDG_*`
/// can neither change the outcome nor receive writes.
fn rtk(home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rtk"));
    cmd.env("HOME", home)
        .env("RTK_DB_PATH", home.join("rtk.db"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local").join("share"))
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("RTK_TELEMETRY_DISABLED", "1")
        .env("LC_ALL", "C")
        .stdin(Stdio::null());
    cmd
}

/// Runs `rtk init <args> --dry-run`, asserts it succeeded, and returns stdout.
fn dry_run(home: &Path, args: &[&str]) -> String {
    let out = rtk(home)
        .arg("init")
        .args(args)
        .arg("--dry-run")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "rtk init {args:?} --dry-run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn agent_cursor_dry_run_never_touches_claude() {
    let home = tempfile::tempdir().unwrap();
    let stdout = dry_run(home.path(), &["-g", "--agent", "cursor"]);
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
    let out = rtk(home.path())
        .args(["init", "-g", "--agent", "cursor", "--auto-patch"])
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
    let stdout = dry_run(home.path(), &["-g"]);
    assert!(
        stdout.contains("/.claude/"),
        "default init must still install Claude Code files:\n{stdout}"
    );
}

#[test]
fn explicit_claude_agent_dry_run_still_installs_claude() {
    let home = tempfile::tempdir().unwrap();
    let stdout = dry_run(home.path(), &["-g", "--agent", "claude"]);
    assert!(
        stdout.contains("/.claude/"),
        "--agent claude must still install Claude Code files:\n{stdout}"
    );
}

#[test]
fn agent_cursor_with_opencode_dry_run_installs_both_without_claude() {
    let home = tempfile::tempdir().unwrap();
    let stdout = dry_run(home.path(), &["-g", "--agent", "cursor", "--opencode"]);
    assert!(
        !stdout.contains("/.claude/"),
        "cursor + opencode init must not touch .claude:\n{stdout}"
    );
    assert!(
        stdout.contains("/opencode/plugins/rtk.ts"),
        "cursor + opencode init must install the OpenCode plugin:\n{stdout}"
    );
    assert!(
        stdout.contains("/.cursor/hooks.json"),
        "cursor + opencode init must patch .cursor hooks.json:\n{stdout}"
    );
}
