//! Verify that the lint shortcut preserves package-runner diagnostics.

#![cfg(unix)]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Output;

fn lint_with_pnpm(script: &str) -> Output {
    let dir = common::tempdir();
    let program = dir.path().join("pnpm");
    fs::write(&program, format!("#!/bin/sh\n{script}\n")).expect("write pnpm stub");
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).expect("chmod pnpm stub");

    common::rtk_command()
        .current_dir(dir.path())
        .env("PATH", dir.path())
        .args(["lint", "pnpm", "exec", "eslint"])
        .output()
        .expect("run rtk lint")
}

#[test]
fn lint_preserves_a_command_not_found_exit_without_claiming_oom() {
    let out = lint_with_pnpm(
        "printf '%s\\n' 'fixture output'\n\
         printf '%s\\n' 'ERR_PNPM_RECURSIVE_EXEC_FIRST_FAIL Command \"eslint\" not found' >&2\n\
         exit 254",
    );

    assert_eq!(out.status.code(), Some(254), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("out of memory"), "{stderr}");
    assert!(
        stderr.contains("ERR_PNPM_RECURSIVE_EXEC_FIRST_FAIL Command \"eslint\" not found"),
        "{stderr}"
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "fixture output\n");
}

#[test]
fn lint_reports_actual_signal_termination_without_guessing_oom() {
    let out = lint_with_pnpm("printf '%s\\n' 'stopping' >&2\nkill -TERM $$");

    assert_eq!(out.status.code(), Some(143), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("process terminated by signal 15"),
        "{stderr}"
    );
    assert!(!stderr.contains("out of memory"), "{stderr}");
    assert!(stderr.contains("stopping"), "{stderr}");
}

#[test]
fn lint_keeps_stderr_when_the_runner_exits_without_eslint_results() {
    let out = lint_with_pnpm("printf '%s\\n' 'Command \"eslint\" not found' >&2\nexit 1");

    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "Command \"eslint\" not found\n"
    );
}
