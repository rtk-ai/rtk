#![cfg(unix)]
//! Tests for Codex init and uninstall exit code behavior (#4164).
//!
//! Specifically, `rtk init --codex --uninstall` should exit 2 when the containment
//! guard refuses the hook (e.g. symlinked outside project) while still cleaning
//! reachable project files.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, Stdio};

mod common;

fn rtk(dir: &Path) -> Command {
    let mut cmd = common::rtk_command();
    cmd.current_dir(dir)
        .env("HOME", dir)
        .env("RTK_DB_PATH", dir.join("rtk.db"))
        .env("LC_ALL", "C")
        .stdin(Stdio::null());
    cmd
}

#[test]
fn test_codex_uninstall_exit_codes() {
    let work = tempfile::tempdir().expect("tempdir");
    let proj = work.path().join("proj");
    let out = work.path().join("out");
    let codex_out = out.join("codex");
    fs::create_dir_all(&proj).expect("mkdir proj");
    fs::create_dir_all(&codex_out).expect("mkdir codex_out");

    let hooks_content = "{\"hooks\":{}}";

    // Case 1: .codex symlinked outside project -> exit 2
    symlink(&codex_out, proj.join(".codex")).expect("symlink .codex");
    fs::write(proj.join("RTK.md"), "my notes\n").expect("RTK.md");
    fs::write(proj.join("AGENTS.md"), "@RTK.md\n").expect("AGENTS.md");
    fs::write(codex_out.join("hooks.json"), hooks_content).expect("hooks.json");

    let output = rtk(&proj)
        .args(["init", "--codex", "--uninstall"])
        .output()
        .expect("run rtk");

    assert_eq!(
        output.status.code(),
        Some(2),
        "Expected exit code 2 when .codex is symlinked outside project"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("Not checked: the Codex hook in .codex/hooks.json"),
        "stdout should explain guard refusal: {stdout}"
    );
    assert!(
        stderr.is_empty(),
        "stderr should not repeat report on refusal: {stderr}"
    );
    assert!(
        !proj.join("RTK.md").exists()
            || fs::read_to_string(proj.join("RTK.md")).unwrap() == "my notes\n",
        "Project cleanup still performed"
    );

    // Case 2: refused case with --dry-run -> exit 2 and [dry-run] Nothing written footer printed
    let dry_output = rtk(&proj)
        .args(["init", "--codex", "--uninstall", "--dry-run"])
        .output()
        .expect("run rtk dry-run");
    assert_eq!(
        dry_output.status.code(),
        Some(2),
        "Expected exit code 2 on dry-run when hook is refused"
    );
    let dry_stdout = String::from_utf8_lossy(&dry_output.stdout);
    assert!(
        dry_stdout.contains("[dry-run] Nothing written."),
        "dry-run footer should still be printed: {dry_stdout}"
    );

    // Case 3: hooks.json.bak symlinked outside project -> exit 2
    fs::remove_file(proj.join(".codex")).expect("rm symlink");
    fs::create_dir_all(proj.join(".codex")).expect("mkdir .codex");
    fs::write(proj.join(".codex").join("hooks.json"), hooks_content).expect("hooks.json");
    symlink(
        out.join("stolen.json"),
        proj.join(".codex").join("hooks.json.bak"),
    )
    .expect("symlink bak");

    let bak_output = rtk(&proj)
        .args(["init", "--codex", "--uninstall"])
        .output()
        .expect("run rtk bak");
    assert_eq!(
        bak_output.status.code(),
        Some(2),
        "Expected exit code 2 when hooks.json.bak is symlinked outside project"
    );

    // Clean up for next cases
    let _ = fs::remove_dir_all(&proj);
    fs::create_dir_all(&proj).expect("mkdir clean proj");

    // Case 4: clean uninstall (nothing installed) -> exit 0
    let clean_output = rtk(&proj)
        .args(["init", "--codex", "--uninstall"])
        .output()
        .expect("run rtk clean");
    assert_eq!(
        clean_output.status.code(),
        Some(0),
        "Expected exit code 0 when nothing installed"
    );
    let clean_stdout = String::from_utf8_lossy(&clean_output.stdout);
    assert!(
        clean_stdout.contains("RTK was not installed for Codex CLI (nothing to remove)"),
        "stdout should report nothing to remove: {clean_stdout}"
    );

    // Case 5: normal installed then uninstalled -> exit 0
    let install_output = rtk(&proj)
        .args(["init", "--codex"])
        .output()
        .expect("run rtk init --codex");
    assert_eq!(install_output.status.code(), Some(0));

    let uninstall_output = rtk(&proj)
        .args(["init", "--codex", "--uninstall"])
        .output()
        .expect("run rtk uninstall");
    assert_eq!(
        uninstall_output.status.code(),
        Some(0),
        "Expected exit code 0 for normal clean uninstall"
    );
}
