//! What the legacy-hook integrity gate says must match what it checked: the
//! hook it actually read, a way out of a state that stops the command, and no
//! warning for a state it deliberately skips.
mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const ORPHANED_WARNING: &str = "hash file exists but hook is missing";

/// rtk with `$CLAUDE_CONFIG_DIR` at `root/claude` and its home at `root/home`,
/// so `~/.claude` is never the directory under test.
fn rtk(root: &Path) -> Command {
    let home = root.join("home");
    let mut cmd = common::rtk_command();
    cmd.env("CLAUDE_CONFIG_DIR", root.join("claude"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("HOME", home);
    cmd
}

/// `rtk read` passes through the gate and needs no external tool, so its output
/// shows whether the command ran.
fn run_gated(root: &Path) -> Output {
    let input = root.join("input.txt");
    std::fs::write(&input, "fixture input\n").expect("write isolated fixture");
    rtk(root).arg("read").arg(input).output().expect("run rtk")
}

fn hooks_dir(root: &Path) -> PathBuf {
    let hooks = root.join("claude").join("hooks");
    std::fs::create_dir_all(&hooks).expect("write isolated fixture");
    hooks
}

/// A well-formed baseline that no hook content hashes to.
fn write_baseline(hooks: &Path) {
    std::fs::write(
        hooks.join(".rtk-hook.sha256"),
        format!("{}  rtk-rewrite.sh\n", "0".repeat(64)),
    )
    .expect("write isolated fixture");
}

#[test]
fn tamper_message_names_the_hook_it_checked() {
    let dir = common::tempdir();
    let hooks = hooks_dir(dir.path());
    let hook = hooks.join("rtk-rewrite.sh");
    std::fs::write(&hook, "#!/bin/sh\n# modified fixture\n").expect("write isolated fixture");
    write_baseline(&hooks);

    let out = run_gated(dir.path());
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "The hook at {} has been modified.",
            hook.display()
        )),
        "{stderr}"
    );
    assert!(!stderr.contains("~/.claude"), "{stderr}");
    assert!(out.stdout.is_empty(), "the gate must stop the command");
}

#[test]
fn damaged_baseline_message_offers_the_restore_command() {
    let dir = common::tempdir();
    let hooks = hooks_dir(dir.path());
    std::fs::write(hooks.join("rtk-rewrite.sh"), "#!/bin/sh\n").expect("write isolated fixture");
    std::fs::write(hooks.join(".rtk-hook.sha256"), "not a hash").expect("write isolated fixture");

    let out = run_gated(dir.path());
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("rtk: Invalid hash format in "), "{stderr}");
    assert!(
        stderr.contains("\n  To restore:  rtk init -g --auto-patch\n"),
        "{stderr}"
    );
    assert!(out.stdout.is_empty(), "the gate must stop the command");
}

#[test]
fn orphaned_baseline_warns_only_in_rtk_verify() {
    let dir = common::tempdir();
    write_baseline(&hooks_dir(dir.path()));

    let out = run_gated(dir.path());
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("fixture input"),
        "{stderr}"
    );
    assert!(!stderr.contains(ORPHANED_WARNING), "{stderr}");

    let out = rtk(dir.path()).arg("verify").output().expect("run rtk");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(ORPHANED_WARNING), "{stderr}");
}
