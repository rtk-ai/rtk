//! A failed `pnpm install` / `pnpm list` must not lose the reason when pnpm
//! (or a wrapper around it) reports it on stdout only. See #2023, #4858.
//! The fake-pnpm approach comes from #2593.
#![cfg(unix)]

mod common;

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;

/// Write a fake `pnpm` that runs `body` as a shell script.
fn fake_pnpm(dir: &Path, body: &str) {
    let path = dir.join("pnpm");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fake pnpm");
    let mut permissions = fs::metadata(&path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("chmod fake pnpm");
}

fn run_rtk(dir: &Path, args: &[&str]) -> Output {
    let current = env::var_os("PATH").unwrap_or_default();
    let path =
        env::join_paths(std::iter::once(dir.to_path_buf()).chain(env::split_paths(&current)))
            .expect("join PATH");
    // rtk_command() keeps the child's tracking data off the real ~/.local/share/rtk.
    common::rtk_command()
        .args(args)
        .env("PATH", path)
        .output()
        .expect("run rtk")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn install_failure_on_stdout_is_shown_filtered() {
    let temp = tempfile::tempdir().expect("tempdir");
    fake_pnpm(
        temp.path(),
        "echo 'Progress: resolved 12, reused 0, downloaded 3, added 0'\n\
         echo 'ERR_PNPM_FETCH_404 GET https://registry.npmjs.org/@missing/pkg: Not Found - 404'\n\
         exit 1",
    );

    let output = run_rtk(temp.path(), &["pnpm", "install"]);
    let stdout = text(&output.stdout);

    assert_eq!(output.status.code(), Some(1), "exit code must propagate");
    assert!(stdout.contains("ERR_PNPM_FETCH_404"), "got: {stdout:?}");
    assert!(
        !stdout.contains("Progress:"),
        "progress must be filtered, got: {stdout:?}"
    );
}

#[test]
fn install_failure_without_error_keyword_is_not_reported_as_ok() {
    // A policy gate in front of pnpm: refusal on stdout, no "ERR"/"error" word,
    // so filter_pnpm_install keeps nothing and would print "ok".
    let temp = tempfile::tempdir().expect("tempdir");
    fake_pnpm(
        temp.path(),
        "echo 'BLOCKED by policy: lockfile changes need review'\nexit 3",
    );

    let output = run_rtk(temp.path(), &["pnpm", "install"]);
    let stdout = text(&output.stdout);

    assert_eq!(output.status.code(), Some(3), "exit code must propagate");
    assert!(stdout.contains("BLOCKED by policy"), "got: {stdout:?}");
    assert_ne!(stdout.trim(), "ok", "a failure must never read as ok");
}

#[test]
fn install_failure_with_stderr_keeps_stderr_only() {
    let temp = tempfile::tempdir().expect("tempdir");
    fake_pnpm(
        temp.path(),
        "echo 'Progress: resolved 1'\necho 'ERR_PNPM_NO_LOCKFILE missing lockfile' >&2\nexit 1",
    );

    let output = run_rtk(temp.path(), &["pnpm", "install"]);
    let stdout = text(&output.stdout);
    let stderr = text(&output.stderr);

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains("ERR_PNPM_NO_LOCKFILE"), "got: {stderr:?}");
    assert!(
        stdout.trim().is_empty(),
        "stdout unchanged when stderr has the reason, got: {stdout:?}"
    );
}

#[test]
fn list_json_failure_on_stdout_is_shown() {
    let temp = tempfile::tempdir().expect("tempdir");
    fake_pnpm(
        temp.path(),
        r#"echo '{"error":{"code":"ERR_PNPM_NO_IMPORTER_MANIFEST_FOUND","message":"No package.json found"}}'
exit 1"#,
    );

    let output = run_rtk(temp.path(), &["pnpm", "list"]);
    let stdout = text(&output.stdout);

    assert_eq!(output.status.code(), Some(1), "exit code must propagate");
    assert!(
        stdout.contains("ERR_PNPM_NO_IMPORTER_MANIFEST_FOUND"),
        "got: {stdout:?}"
    );
}
