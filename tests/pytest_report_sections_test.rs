//! Real pytest 9.1.1 reports captured on Linux with --tb=short and -rxX.
//! Exercises the public pipe adapter; outcome reports must not become failures,
//! and an explicitly requested duration table must survive as its own section.
use std::io::Write;
use std::process::Stdio;

mod common;

fn pipe(raw: &str) -> String {
    let mut child = common::rtk_command()
        .args(["pipe", "--filter", "pytest"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start pipe");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(raw.as_bytes())
        .expect("write report");
    let output = child.wait_with_output().expect("read result");
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).expect("UTF-8 report")
}

#[test]
fn expected_outcomes_are_not_failures() {
    let shown = pipe(include_str!("fixtures/pytest_outcome_sections_raw.txt"));
    assert_eq!(shown.matches("[FAIL]").count(), 1, "{shown}");
    assert!(shown.contains("[FAIL] test_key"), "{shown}");
    assert!(shown.contains("REAL_KEY_FAILURE"), "{shown}");
    assert!(
        shown.contains("XFAIL test_reports.py::test_xf - known issue"),
        "{shown}"
    );
    assert!(
        shown.contains("XPASS test_reports.py::test_xp - already fixed"),
        "{shown}"
    );
    assert!(!shown.contains("EXPECTED_XFAIL_DETAIL"), "{shown}");
    assert!(!shown.contains("CAPTURED_XPASS_OUTPUT"), "{shown}");
}

#[test]
fn an_expected_failure_only_run_has_no_failure_blocks() {
    let shown = pipe(include_str!("fixtures/pytest_xfail_only_raw.txt"));
    assert!(
        shown.starts_with("Pytest: 0 passed, 0 failed, 1 xfailed"),
        "{shown}"
    );
    assert!(!shown.contains("[FAIL]"), "{shown}");
    assert!(!shown.contains("EXPECTED_XFAIL_DETAIL"), "{shown}");
    assert!(
        shown.contains("XFAIL test_reports.py::test_xf - known issue"),
        "{shown}"
    );
}

#[test]
fn requested_durations_remain_separate_from_failure_details() {
    let raw = include_str!("fixtures/pytest_duration_sections_raw.txt");
    let shown = pipe(raw);
    assert_eq!(shown.matches("[FAIL]").count(), 1, "{shown}");
    assert!(shown.contains("REAL_KEY_FAILURE"), "{shown}");
    let header = raw
        .lines()
        .find(|line| line.contains("slowest 1 durations"))
        .expect("duration header");
    assert!(
        shown.contains(&format!(
            "\n\n{header}\n0.01s call     test_reports.py::test_key"
        )),
        "{shown}"
    );
    assert!(!shown.contains("     0.01s call"), "{shown}");
}

#[test]
fn passing_runs_keep_all_requested_duration_rows() {
    let raw = include_str!("fixtures/pytest_pass_durations_raw.txt");
    let shown = pipe(raw);
    assert!(shown.starts_with("Pytest: 1 passed"), "{shown}");
    let (header, _) = raw
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains("slowest durations"))
        .expect("duration header");
    let table = raw
        .lines()
        .skip(header)
        .take_while(|line| !line.contains("passed in"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(shown.trim_end().ends_with(table.trim_end()), "{shown}");
}

#[test]
fn application_banners_keep_both_real_failures() {
    let raw = "=== FAILURES ===\n___ test_first ___\nE FIRST_FAILURE\n=== application diagnostics ===\n=== slowest application durations ===\n___ test_second ___\nE SECOND_FAILURE\n=== 2 failed in 0.01s ===\n";
    let shown = pipe(raw);
    assert_eq!(shown.matches("[FAIL]").count(), 2, "{shown}");
    assert!(
        shown.contains("FIRST_FAILURE") && shown.contains("SECOND_FAILURE"),
        "{shown}"
    );
}
