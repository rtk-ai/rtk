//! `ruff check` fix-success at exit 1, driven through `run()`.
//!
//! `ruff --exit-non-zero-on-fix` exits 1 when it fixed files — success, not
//! failure. rtk must keep the green verdict there, and must not depend on argv
//! to recognize the form: fix can come from `[tool.ruff] fix = true` in
//! pyproject.toml as well as `--fix` on the command line. The signal that
//! frees this from argv is already in hand at the filter: zero diagnostics
//! parse to "Ruff: No issues found", and for `ruff check` an exit 1 with zero
//! diagnostics can only be the fix-success form, because a config error exits
//! 2. Review 3's four rows are pinned here, plus a mutation guard that fails
//! when the exit-1 restriction is dropped.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

mod common;

/// Writes an executable `name` in `dir` that runs `body`.
fn fake_tool(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{}\n", body)).expect("write fake tool");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod fake tool");
}

/// Runs rtk with `dir` first on PATH, so the fake tool shadows any real one.
fn rtk_with(dir: &Path, args: &[&str]) -> Output {
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    common::rtk_command()
        .args(args)
        .env("PATH", path)
        .output()
        .expect("run rtk")
}

/// Scratch directory with a stub ruff whose whole behavior is `body`.
fn stub_dir(name: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).expect("mkdir");
    fake_tool(&dir, "ruff", body);
    dir
}

/// Two F401s in one file — the parsed summary must survive at exit 1.
const TWO_VIOLATIONS: &str = r#"echo '[
  {"code": "F401", "message": "`os` imported but unused",
   "location": {"row": 1, "column": 8}, "end_location": null,
   "filename": "src/main.py", "fix": null},
  {"code": "F401", "message": "`sys` imported but unused",
   "location": {"row": 2, "column": 8}, "end_location": null,
   "filename": "src/main.py", "fix": null}
]' ; exit 1 ;"#;

#[test]
fn fix_success_from_cli_keeps_no_issues_found() {
    // `ruff check --fix --exit-non-zero-on-fix .`
    let dir = stub_dir("rtk_ruff_fix_cli", "echo '[]' ; exit 1 ;");
    let out = rtk_with(
        &dir,
        &["ruff", "check", "--fix", "--exit-non-zero-on-fix", "."],
    );
    let _ = fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Ruff: No issues found"),
        "fix-success arm lost its green verdict: {stdout:?}"
    );
    assert!(
        !stdout.contains("ruff: failed"),
        "success got a failure verdict: {stdout:?}"
    );
}

#[test]
fn fix_success_from_config_keeps_no_issues_found() {
    // `fix = true` lives in pyproject.toml; the CLI only shows
    // `--exit-non-zero-on-fix`. An argv-scan exemption misses this arm — this
    // row is the whole point of the diagnostics signal.
    let dir = stub_dir("rtk_ruff_fix_config", "echo '[]' ; exit 1 ;");
    let out = rtk_with(&dir, &["ruff", "check", "--exit-non-zero-on-fix", "."]);
    let _ = fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Ruff: No issues found"),
        "config-driven fix-success lost its green verdict: {stdout:?}"
    );
    assert!(
        !stdout.contains("ruff: failed"),
        "success got a failure verdict: {stdout:?}"
    );
}

#[test]
fn config_error_exit_two_refuses_green() {
    // Config error: exit 2, empty stdout, diagnostics on stderr. The filter
    // refuses the green verdict (guard_exit falls back), and `never_worse`
    // keeps the empty raw stdout — the config message reaches the user via
    // the forwarded stderr instead. What must never happen is "No issues
    // found" on a failed run.
    let dir = stub_dir(
        "rtk_ruff_cfg_err",
        "echo 'invalid TOML in pyproject.toml: cannot parse fix = = true' >&2 ; exit 2 ;",
    );
    let out = rtk_with(&dir, &["ruff", "check", "."]);
    let _ = fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("No issues found"),
        "config error must not render green, got stdout {stdout:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("invalid TOML in pyproject.toml"),
        "config error stderr must reach the user, got {stderr:?}"
    );
}

#[test]
fn violations_at_exit_one_keep_the_issue_summary() {
    let dir = stub_dir("rtk_ruff_violations", TWO_VIOLATIONS);
    let out = rtk_with(&dir, &["ruff", "check", "."]);
    let _ = fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("2 issues in 1 files"),
        "violations must keep the parsed summary: {stdout:?}"
    );
}

#[test]
fn exit_two_cannot_render_green_even_with_zero_diagnostics() {
    // Mutation pin, driven through `run()` like the rest: real ruff never
    // prints `[]` at exit 2, but dropping the `exit_code == 1` conjunct makes
    // any non-zero exit exempt as long as diagnostics are empty. A stub that
    // does exactly that must still refuse the verdict, so this test fails
    // when the restriction is relaxed and passes when it holds.
    let dir = stub_dir("rtk_ruff_mut_pin", "echo '[]' ; exit 2 ;");
    let out = rtk_with(&dir, &["ruff", "check", "."]);
    let _ = fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("No issues found"),
        "a non-1 exit must never render green, got: {stdout:?}"
    );
    assert!(
        stdout.contains("ruff: failed (exit 2)"),
        "expected the failure fallback, got: {stdout:?}"
    );
}
