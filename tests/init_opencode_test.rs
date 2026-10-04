#![cfg(unix)]
//! `rtk init -g --opencode` installs the Claude Code setup and the OpenCode plugin
//! together, from a home that has neither, and re-running it repairs a missing plugin.

use std::path::Path;
use std::process::{Command, Stdio};

mod common;

/// Runs rtk against `home` with every directory it resolves from the
/// environment pinned inside it, so an exported `CLAUDE_CONFIG_DIR` or `XDG_*`
/// can neither change the outcome nor receive writes.
fn rtk(home: &Path) -> Command {
    let mut cmd = common::rtk_command();
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

/// Runs `rtk init <args>`, asserts it succeeded, and returns stdout.
fn init(home: &Path, args: &[&str]) -> String {
    let out = rtk(home).arg("init").args(args).output().unwrap();
    assert!(
        out.status.success(),
        "rtk init {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn plugin(home: &Path) -> std::path::PathBuf {
    home.join(".config")
        .join("opencode")
        .join("plugins")
        .join("rtk.ts")
}

fn claude_hook_registered(home: &Path) -> bool {
    std::fs::read_to_string(home.join(".claude").join("settings.json"))
        .is_ok_and(|settings| settings.contains("rtk hook claude"))
}

#[test]
fn opencode_installs_claude_hook_and_plugin() {
    let home = tempfile::tempdir().unwrap();
    init(home.path(), &["-g", "--opencode", "--auto-patch"]);
    assert!(
        claude_hook_registered(home.path()),
        "--opencode must also register the Claude hook"
    );
    assert!(
        home.path().join(".claude").join("RTK.md").exists(),
        "--opencode must also write RTK.md"
    );
    assert!(plugin(home.path()).exists(), "OpenCode plugin missing");
}

#[test]
fn opencode_hook_only_installs_claude_hook_and_plugin() {
    let home = tempfile::tempdir().unwrap();
    init(
        home.path(),
        &["-g", "--opencode", "--hook-only", "--auto-patch"],
    );
    assert!(
        claude_hook_registered(home.path()),
        "--opencode --hook-only must also register the Claude hook"
    );
    assert!(
        !home.path().join(".claude").join("RTK.md").exists(),
        "--hook-only must not write RTK.md"
    );
    assert!(plugin(home.path()).exists(), "OpenCode plugin missing");
}

#[test]
fn hook_only_without_patching_leaves_no_claude_dir() {
    let home = tempfile::tempdir().unwrap();
    init(home.path(), &["-g", "--hook-only", "--no-patch"]);
    assert!(
        !home.path().join(".claude").exists(),
        "nothing is written under ~/.claude, so it must not be created"
    );
}

#[test]
fn opencode_claude_md_rerun_restores_missing_plugin() {
    let home = tempfile::tempdir().unwrap();
    init(home.path(), &["-g", "--opencode", "--claude-md"]);
    std::fs::remove_file(plugin(home.path())).unwrap();

    // The CLAUDE.md block is unchanged now; the plugin must still come back.
    let stdout = init(home.path(), &["-g", "--opencode", "--claude-md"]);
    assert!(stdout.contains("already up to date"), "{stdout}");
    assert!(
        !stdout.contains("will now use rtk"),
        "an unchanged block stays silent:\n{stdout}"
    );
    assert!(
        plugin(home.path()).exists(),
        "re-running init must restore the OpenCode plugin"
    );
}

#[test]
fn opencode_dry_run_names_both_and_writes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let stdout = init(
        home.path(),
        &["-g", "--opencode", "--auto-patch", "--dry-run"],
    );
    assert!(
        stdout.contains("/.claude/settings.json"),
        "dry-run must name the Claude hook patch:\n{stdout}"
    );
    assert!(
        stdout.contains("/opencode/plugins/rtk.ts"),
        "dry-run must name the OpenCode plugin:\n{stdout}"
    );
    let leftovers: Vec<_> = std::fs::read_dir(home.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(leftovers.is_empty(), "dry-run wrote: {leftovers:?}");
}
