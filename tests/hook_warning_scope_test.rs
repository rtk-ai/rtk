//! The "hook missing/outdated" reminder is a once-a-day nudge, so where it is
//! spent matters: a command the user ran to inspect or install the hook would
//! both duplicate that command's own reporting and burn the daily marker,
//! leaving the next filtered command silent.
//!
//! Unix only: the check resolves the Claude directory through the home
//! directory, and only Unix takes that from `$HOME`.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

const REMINDER: &str = "No hook installed";

/// Runs `rtk <args>` against a home directory whose Claude config directory
/// exists but registers no hook, and returns stderr.
fn run(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(args)
        .env("HOME", home)
        .env("RTK_DB_PATH", home.join("rtk.db"))
        .output()
        .expect("run rtk");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn fresh_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("tempdir");
    // Absent, the status check reports "not applicable"; present-but-empty is
    // what makes a missing hook detectable.
    std::fs::create_dir_all(home.path().join(".claude")).expect("claude dir");
    home
}

#[test]
fn commands_that_report_hook_state_do_not_repeat_the_reminder() {
    for args in [
        vec!["init", "-g"],
        vec!["verify"],
        vec!["verify", "--filter", "cargo"],
        vec!["gain"],
    ] {
        let home = fresh_home();
        let stderr = run(home.path(), &args);
        assert!(
            !stderr.contains(&format!("[rtk] /!\\ {REMINDER}")),
            "rtk {} must not print the daily reminder: {stderr}",
            args.join(" ")
        );
    }
}

#[test]
fn grok_hook_counts_as_installed() {
    let home = fresh_home();
    let grok_hooks = home.path().join(".grok").join("hooks");
    std::fs::create_dir_all(&grok_hooks).expect("grok hooks dir");
    std::fs::write(grok_hooks.join("rtk-rewrite.json"), b"{}\n").expect("grok hook file");

    let stderr = run(home.path(), &["git", "status"]);
    assert!(
        !stderr.contains(REMINDER),
        "a Grok hook must suppress the Claude-missing reminder: {stderr}"
    );
}

#[test]
fn filtered_commands_still_get_the_reminder() {
    for args in [vec!["ls"], vec!["git", "status"]] {
        let home = fresh_home();
        let stderr = run(home.path(), &args);
        assert!(
            stderr.contains(REMINDER),
            "rtk {} must still warn: {stderr}",
            args.join(" ")
        );
    }
}

#[test]
fn a_meta_command_leaves_the_daily_reminder_for_the_next_filtered_command() {
    // The regression this guards: `maybe_warn` touches the once-a-day marker,
    // so warning on a meta command would spend the reminder there and leave
    // the next filtered command — the one that should have been hooked —
    // silent for 24 hours.
    let home = fresh_home();
    run(home.path(), &["verify", "--filter", "cargo"]);
    let stderr = run(home.path(), &["ls"]);
    assert!(
        stderr.contains(REMINDER),
        "the reminder must survive a preceding meta command: {stderr}"
    );
}
