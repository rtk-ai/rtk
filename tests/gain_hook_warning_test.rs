//! Integration tests: `rtk gain` hook-status warning coverage.
//!
//! Change: fix-gain-hook-warning (rtk-ai/rtk#4035) — the missing-hook warning
//! must appear on every text view (default, --daily, --weekly, --monthly,
//! --all) and on the empty-database report, exactly once per invocation.
//! JSON/CSV exports stay warning-free; the not-applicable state (no ~/.claude)
//! stays silent.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

const NEEDLE: &str = "[warn] No hook installed";

/// Run `rtk gain <args>` with HOME isolated to `home`, returning
/// (stdout, stderr) as lossy UTF-8 strings.
fn gain_split(args: &[&str], home: &Path) -> (String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(args)
        .env("HOME", home)
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("LC_ALL", "C")
        .output()
        .expect("spawn rtk gain");
    assert!(
        output.status.success(),
        "rtk gain {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Combined stdout+stderr (ANSI codes wrap, never split, the needle).
fn gain(args: &[&str], home: &Path) -> String {
    let (out, err) = gain_split(args, home);
    format!("{out}{err}")
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

/// HOME with a `.claude` dir but no hook registered → status() == Missing.
fn missing_hook_home() -> TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join(".claude")).expect("create .claude");
    tmp
}

/// Missing-hook home plus one tracked record (via `rtk proxy true`, which
/// records usage with 0% reduction).
fn seeded_home() -> TempDir {
    let tmp = missing_hook_home();
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["proxy", "true"])
        .env("HOME", tmp.path())
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("LC_ALL", "C")
        .output()
        .expect("spawn rtk proxy seed");
    assert!(
        out.status.success(),
        "rtk proxy true failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    tmp
}

#[test]
fn default_view_with_data_warns_exactly_once_on_stderr() {
    let home = seeded_home();
    let (stdout, stderr) = gain_split(&["gain"], home.path());
    assert_eq!(
        count(&format!("{stdout}{stderr}"), NEEDLE),
        1,
        "warning must appear exactly once"
    );
    assert!(
        !stdout.contains(NEEDLE),
        "warning must be routed to stderr, not stdout"
    );
}

#[test]
fn daily_view_warns_exactly_once() {
    let home = seeded_home();
    assert_eq!(count(&gain(&["gain", "--daily"], home.path()), NEEDLE), 1);
}

#[test]
fn weekly_view_warns_exactly_once() {
    let home = seeded_home();
    assert_eq!(count(&gain(&["gain", "--weekly"], home.path()), NEEDLE), 1);
}

#[test]
fn monthly_view_warns_exactly_once() {
    let home = seeded_home();
    assert_eq!(count(&gain(&["gain", "--monthly"], home.path()), NEEDLE), 1);
}

#[test]
fn all_view_warns_exactly_once() {
    let home = seeded_home();
    assert_eq!(count(&gain(&["gain", "--all"], home.path()), NEEDLE), 1);
}

#[test]
fn empty_database_still_warns_once() {
    // The rtk-ai/rtk#4035 regression: empty tracking data must not swallow
    // the hook warning (it explains WHY there is no data).
    let home = missing_hook_home();
    assert_eq!(count(&gain(&["gain"], home.path()), NEEDLE), 1);
}

#[test]
fn json_export_is_warning_free() {
    let home = seeded_home();
    let (stdout, stderr) = gain_split(&["gain", "--format", "json"], home.path());
    let combined = format!("{stdout}{stderr}");
    assert_eq!(count(&combined, NEEDLE), 0);
    assert!(
        !combined.contains("[warn]"),
        "no warning text in JSON export"
    );
    assert!(
        stdout.trim_start().starts_with('{'),
        "JSON export must start with '{{': {stdout}"
    );
}

#[test]
fn csv_export_is_warning_free() {
    let home = seeded_home();
    let combined = gain(&["gain", "--format", "csv"], home.path());
    assert_eq!(count(&combined, NEEDLE), 0);
    assert!(
        !combined.contains("[warn]"),
        "no warning text in CSV export"
    );
}

#[test]
fn no_claude_dir_stays_silent() {
    // Not-applicable state: bare HOME without ~/.claude → status() == Ok.
    let tmp = seeded_home_bare();
    let combined = gain(&["gain"], tmp.path());
    assert_eq!(
        count(&combined, NEEDLE),
        0,
        "no ~/.claude dir must stay silent"
    );
}

/// Bare HOME (no `.claude`) with one tracked record.
fn seeded_home_bare() -> TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["proxy", "true"])
        .env("HOME", tmp.path())
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("LC_ALL", "C")
        .output()
        .expect("spawn rtk proxy seed");
    assert!(
        out.status.success(),
        "rtk proxy true failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    tmp
}
