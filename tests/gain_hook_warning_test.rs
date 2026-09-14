//! `rtk gain` must report a missing hook on every view.
//!
//! Without the hook nothing is tracked, so a missing hook is the likeliest
//! reason a savings report is low or empty — which is exactly when the advice
//! is worth printing.
//!
//! Unix only: the check resolves the Claude directory through the home
//! directory, and only Unix takes that from `$HOME`.
#![cfg(unix)]

use std::process::Command;

/// Runs `rtk gain <args>` against a home directory holding a Claude config
/// directory with no hook registered, and returns stderr.
fn gain_stderr(args: &[&str]) -> String {
    let home = tempfile::tempdir().expect("tempdir");
    // The status check reports "not applicable" when the directory is absent,
    // so an empty-but-present one is what makes a missing hook detectable.
    std::fs::create_dir_all(home.path().join(".claude")).expect("claude dir");

    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("gain")
        .args(args)
        .env("HOME", home.path())
        .env("RTK_DB_PATH", home.path().join("rtk.db"))
        .output()
        .expect("run rtk gain");

    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn warns(args: &[&str]) -> bool {
    gain_stderr(args).contains("No hook installed")
}

#[test]
fn every_gain_view_reports_a_missing_hook() {
    // The default view has always warned; the breakdown views did not, because
    // the check sat inside the default-view branch.
    assert!(warns(&[]), "default view");
    assert!(warns(&["--daily"]), "--daily");
    assert!(warns(&["--weekly"]), "--weekly");
    assert!(warns(&["--monthly"]), "--monthly");
    assert!(warns(&["--all"]), "--all");
}

#[test]
fn an_empty_report_still_reports_a_missing_hook() {
    // Nothing tracked at all is the strongest signal the hook is missing, and
    // the case where the early return used to drop the advice.
    let stderr = gain_stderr(&[]);
    assert!(
        stderr.contains("No hook installed"),
        "empty database should still explain why: {stderr}"
    );
}

#[test]
fn machine_readable_output_stays_free_of_the_warning() {
    for format in ["json", "csv"] {
        let stderr = gain_stderr(&["--format", format]);
        assert!(
            !stderr.contains("No hook installed"),
            "{format} export must not carry the warning: {stderr}"
        );
    }
}
