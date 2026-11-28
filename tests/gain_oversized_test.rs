//! Integration tests: `rtk gain` when every tracked row is oversized.
//!
//! The headline aggregates skip rows whose `input_tokens` pass
//! `HEADLINE_INPUT_CAP_TOKENS` (7,500), counting them in
//! `GainSummary::oversized_excluded` instead (see `core::tracking`). A tracker
//! whose rows are all oversized therefore has rows but no headline rows, which
//! must not read as an empty database.
//!
//! Change: fix-4893-gain-oversized-empty-state (rtk-ai/rtk#4893) — the empty
//! state is for a tracker with no rows at all; a tracker whose rows are all
//! oversized reports the exclusion and the diagnostic listings instead.

use std::path::Path;
use tempfile::TempDir;

mod common;

/// Pin every directory the binary resolves from the environment, so a runner
/// that exports `XDG_*` cannot reach past the temporary home.
fn isolating_env(home: &Path) -> Vec<(String, std::ffi::OsString)> {
    vec![
        ("HOME".into(), home.as_os_str().to_owned()),
        ("RTK_DB_PATH".into(), home.join("rtk.db").into_os_string()),
        (
            "XDG_CONFIG_HOME".into(),
            home.join(".config").into_os_string(),
        ),
        (
            "XDG_DATA_HOME".into(),
            home.join(".local").join("share").into_os_string(),
        ),
    ]
}

/// Run `rtk gain <args>` with HOME isolated to `home`, returning
/// (stdout, stderr) as lossy UTF-8 strings.
fn gain_split(args: &[&str], home: &Path) -> (String, String) {
    // `rtk_command` clears the inherited `RTK_*` and agent-home variables and
    // pins rtk's own paths; `isolating_env` then moves them onto this test's
    // temporary home. `LC_ALL` is this test's to set: it reads rtk's own
    // wording, not a native tool's. The temporary home is also the working
    // directory, so `-p` scopes to the project the fixture recorded into.
    let mut cmd = common::rtk_command();
    cmd.args(args)
        .current_dir(home)
        .envs(isolating_env(home))
        .env("LC_ALL", "C");
    let output = cmd.output().expect("spawn rtk gain");
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

/// Book one `rtk read` row in the tracker under `home`, for a file of `lines`
/// lines written there. Returns the file's size in bytes.
///
/// `rtk read` applies no line window unless one is asked for, so the row it
/// books is the whole file.
fn record_read(home: &Path, lines: usize) -> usize {
    let mut content = String::new();
    for i in 0..lines {
        content.push_str(&format!("line {i:04} lorem ipsum dolor sit amet\n"));
    }
    let file = home.join(format!("read-{lines}.txt"));
    std::fs::write(&file, &content).expect("write fixture file");

    let mut cmd = common::rtk_command();
    let output = cmd
        .arg("read")
        .arg(&file)
        .current_dir(home)
        .envs(isolating_env(home))
        .env("LC_ALL", "C")
        .output()
        .expect("spawn rtk read");
    assert!(
        output.status.success(),
        "rtk read failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    content.len()
}

/// A tracker holding one row whose input passes the headline cap: a read of a
/// file larger than the ~30,000-char agent cap — 37,000 bytes, ~9,250 tokens.
fn oversized_only_home() -> TempDir {
    let tmp = common::tempdir();
    let bytes = record_read(tmp.path(), 1_000);
    assert!(
        bytes > 30_000,
        "the fixture must pass the ~30k-char agent cap"
    );
    tmp
}

/// A tracker holding one row far under the headline cap (10 lines, ~370
/// bytes), so nothing is excluded.
fn under_cap_home() -> TempDir {
    let tmp = common::tempdir();
    record_read(tmp.path(), 10);
    tmp
}

/// Whether `stdout` has a line that is exactly `line`, so "By Command" does
/// not match "By Command (unfiltered)".
fn has_line(stdout: &str, line: &str) -> bool {
    stdout.lines().any(|l| l.trim_end() == line)
}

/// A tracker with no rows at all.
fn empty_home() -> TempDir {
    common::tempdir()
}

#[test]
fn oversized_only_tracker_is_not_an_empty_database() {
    let home = oversized_only_home();
    let (stdout, _) = gain_split(&["gain"], home.path());

    assert!(
        !stdout.contains("No tracking data yet."),
        "a tracker whose rows are all oversized must not read as empty: {stdout}"
    );
    assert!(
        stdout.contains("Total commands:"),
        "the headline KPIs must still be rendered: {stdout}"
    );
    assert!(
        stdout.contains("Excluded:"),
        "the exclusion that emptied the headline must be named: {stdout}"
    );
    assert!(
        stdout.contains("oversized outputs"),
        "the exclusion must say what was excluded: {stdout}"
    );
    assert!(
        has_line(&stdout, "By Command (unfiltered)"),
        "the uncapped per-command listing must be rendered and labelled: {stdout}"
    );
}

#[test]
fn a_tracker_with_no_oversized_rows_keeps_the_plain_header_and_no_exclusion() {
    // Nothing was excluded, so the per-command table and the headline count
    // the same rows: no label, and no `Excluded:` line in the summary or any
    // period view.
    let home = under_cap_home();
    let (stdout, _) = gain_split(&["gain"], home.path());
    assert!(
        has_line(&stdout, "By Command"),
        "with nothing excluded the per-command header stays plain: {stdout}"
    );
    assert!(
        !stdout.contains("Excluded:"),
        "with nothing excluded the summary names no exclusion: {stdout}"
    );

    for flag in ["--all", "--daily", "--monthly", "--weekly"] {
        let (stdout, _) = gain_split(&["gain", flag], home.path());
        assert!(
            stdout.contains("TOTAL"),
            "the `rtk gain {flag}` table must hold the row: {stdout}"
        );
        assert!(
            !stdout.contains("Excluded:"),
            "with nothing excluded `rtk gain {flag}` names no exclusion: {stdout}"
        );
    }
}

#[test]
fn oversized_only_tracker_is_excluded_and_counted_in_the_json_export() {
    // The JSON export returns before the empty-state guard, so it is the
    // ground truth for what the tracker holds: one row, excluded from the
    // headline and counted as oversized.
    let home = oversized_only_home();
    let (stdout, _) = gain_split(&["gain", "--format", "json"], home.path());

    let export: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("json export: {e}: {stdout}"));
    let summary = &export["summary"];
    assert_eq!(summary["total_commands"], 0, "the row is over the cap");
    assert_eq!(
        summary["oversized_excluded"], 1,
        "the row is tracked, and excluded from the headline: {stdout}"
    );
}

#[test]
fn oversized_only_tracker_reports_no_daily_data_rather_than_no_tracking_data() {
    let home = oversized_only_home();
    let (stdout, _) = gain_split(&["gain", "--daily"], home.path());

    assert!(
        !stdout.contains("No tracking data yet."),
        "the daily view must not fall back to the empty state: {stdout}"
    );
    assert!(
        stdout.contains("No daily data available."),
        "the capped daily table is genuinely empty, and must say so: {stdout}"
    );
    assert!(
        stdout.contains("Excluded:") && stdout.contains("1 oversized outputs"),
        "the daily view must name the exclusion that emptied its table: {stdout}"
    );
}

#[test]
fn oversized_only_tracker_names_the_exclusion_in_every_period_view() {
    let home = oversized_only_home();
    for flag in ["--all", "--monthly", "--weekly"] {
        let (stdout, _) = gain_split(&["gain", flag], home.path());
        assert!(
            !stdout.contains("No tracking data yet."),
            "`rtk gain {flag}` must not fall back to the empty state: {stdout}"
        );
        assert!(
            stdout.contains("Excluded:") && stdout.contains("1 oversized outputs"),
            "`rtk gain {flag}` must name the exclusion: {stdout}"
        );
    }
}

#[test]
fn oversized_only_project_is_not_an_empty_database() {
    // `-p` scopes the summary to the working directory, which is where the
    // fixture recorded its one oversized row.
    let home = oversized_only_home();
    let (stdout, _) = gain_split(&["gain", "-p"], home.path());

    assert!(
        !stdout.contains("No tracking data yet."),
        "a project whose rows are all oversized must not read as empty: {stdout}"
    );
    assert!(
        stdout.contains("RTK Token Savings (Project Scope)"),
        "the project-scoped headline must be rendered: {stdout}"
    );
    assert!(
        stdout.contains("Excluded:") && stdout.contains("1 oversized outputs"),
        "the project's exclusion must be named: {stdout}"
    );
}

#[test]
fn a_tracker_with_no_rows_still_reports_the_empty_state() {
    // The other half of the fix: the empty state is for a tracker with no
    // rows at all. Without this, deleting the guard would pass the suite.
    let home = empty_home();
    let (stdout, _) = gain_split(&["gain"], home.path());

    assert!(
        stdout.contains("No tracking data yet."),
        "a tracker with no rows must still report the empty state: {stdout}"
    );
    assert!(
        stdout.contains("Run some rtk commands to start tracking savings."),
        "the empty state must still tell the user what to do: {stdout}"
    );
}
