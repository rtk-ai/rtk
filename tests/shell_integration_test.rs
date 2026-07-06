//! End-to-end integration tests for the `rtk-shell` binary
//! (src/bin/rtk_shell.rs, src/shell/*).
//!
//! These spawn the real compiled `rtk-shell` binary (via
//! `CARGO_BIN_EXE_rtk-shell`) rather than calling into `src/shell` directly,
//! so they exercise the actual argv handling, process spawning, and exit
//! code propagation a user would see.

use std::io::Write;
use std::process::{Command, Stdio};

fn rtk_shell_exe() -> &'static str {
    env!("CARGO_BIN_EXE_rtk-shell")
}

fn init_git_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for args in [
        &["init", "-q"][..],
        &["config", "user.email", "t@t.t"][..],
        &["config", "user.name", "t"][..],
        &["commit", "-q", "--allow-empty", "-m", "init"][..],
    ] {
        let ok = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(ok, "git setup failed: {args:?}");
    }
    dir
}

#[test]
fn oneshot_git_status_in_temp_repo_is_filtered_with_exit_zero() {
    let dir = init_git_repo();
    std::fs::write(dir.path().join("untracked.txt"), "hello\n").expect("write untracked file");

    let out = Command::new(rtk_shell_exe())
        .args(["-c", "git status"])
        .current_dir(dir.path())
        .output()
        .expect("spawn rtk-shell -c 'git status'");

    assert_eq!(out.status.code(), Some(0), "git status should succeed");

    let stdout = String::from_utf8_lossy(&out.stdout);
    // Raw `git status` is verbose ("On branch ...", "Untracked files:",
    // hint text, etc). rtk's filter condenses it to short porcelain-style
    // lines; assert both the filtered marker content and that it is far
    // shorter than the verbose raw form would be.
    assert!(
        stdout.contains("untracked.txt"),
        "filtered output should still mention the untracked file: {stdout:?}"
    );
    assert!(
        !stdout.contains("nothing added to commit but untracked files present"),
        "output should be filtered, not raw verbose git status: {stdout:?}"
    );
}

#[test]
fn oneshot_false_exits_with_code_one() {
    let out = Command::new(rtk_shell_exe())
        .args(["-c", "false"])
        .output()
        .expect("spawn rtk-shell -c 'false'");
    assert_eq!(out.status.code(), Some(1));
}

#[cfg(unix)]
#[test]
fn oneshot_signal_killed_child_maps_to_128_plus_signum() {
    // "kill -9 $$" kills the `sh -c` process running the forwarded segment
    // itself (SIGKILL = signal 9), so rtk-shell must report 128 + 9 = 137,
    // matching core::utils::exit_code_from_status's signal-mapping contract.
    let out = Command::new(rtk_shell_exe())
        .args(["-c", "kill -9 $$"])
        .output()
        .expect("spawn rtk-shell -c 'kill -9 $$'");
    assert_eq!(out.status.code(), Some(137));
}

#[test]
fn persistent_session_cd_then_pwd_shares_state_across_lines() {
    // Two sequential lines piped over stdin to a persistent-mode session
    // (no -c, no args) must be executed by the *same* backing shell process,
    // so a `cd` on one line is visible to a `pwd` on the next line.
    let mut child = Command::new(rtk_shell_exe())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn rtk-shell (persistent session mode)");

    {
        let stdin = child.stdin.as_mut().expect("stdin");
        stdin
            .write_all(b"cd /tmp\npwd\nexit\n")
            .expect("write session input");
    }

    let out = child.wait_with_output().expect("wait rtk-shell session");
    assert!(out.status.success(), "session should exit cleanly");

    let stdout = String::from_utf8_lossy(&out.stdout);
    // macOS may resolve /tmp to /private/tmp; accept either canonical form.
    assert!(
        stdout
            .lines()
            .any(|l| l.trim() == "/tmp" || l.trim().ends_with("/tmp")),
        "expected a line reflecting the earlier `cd /tmp`, got stdout: {stdout:?}"
    );
}
