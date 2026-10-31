//! CLI regressions using pytest 9.1.1 reports, captured with CPython 3.12.3
//! and sys capture. Only the project root and elapsed time are normalized.
//! Tests exercise pipe filtering and actual recovery of failures beyond the cap.

use std::io::Write;
use std::process::Stdio;

mod common;

const MANY: &str = include_str!("fixtures/pytest_line_many_named_raw.txt");
const UNNAMED: &str = include_str!("fixtures/pytest_line_many_no_summary_raw.txt");
const MIXED: &str = include_str!("fixtures/pytest_line_mixed_raw.txt");
const LOCATIONS: &str = include_str!("fixtures/pytest_line_locations_raw.txt");

fn pipe(raw: &str) -> String {
    let mut child = common::rtk_command()
        .args(["pipe", "-f", "pytest"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start rtk pipe");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(raw.as_bytes())
        .expect("write pytest report");
    let output = child.wait_with_output().expect("wait for rtk pipe");
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).expect("UTF-8 filtered report")
}

fn recover(shown: &str) -> String {
    let Some((_, rest)) = shown.split_once("rtk recall ") else {
        return shown.to_string();
    };
    let hash = rest.split(']').next().expect("recall reference").trim();
    let output = common::rtk_command()
        .args(["recall", hash])
        .output()
        .expect("recall hidden failures");
    assert!(output.status.success(), "{output:?}");
    format!(
        "{shown}\n{}",
        String::from_utf8(output.stdout).expect("UTF-8 recovered report")
    )
}

#[test]
fn named_line_failures_count_once_and_recover_the_rest() {
    let shown = pipe(MANY);
    assert!(shown.starts_with("Pytest: 0 passed, 14 failed"), "{shown}");
    assert_eq!(shown.matches("[FAIL]").count(), 10, "{shown}");
    assert!(shown.contains("… +4 more failures"), "{shown}");
    for index in 0..10 {
        assert!(
            shown.contains(&format!("test_many.py::test_f{index:02}")),
            "{shown}"
        );
        assert!(
            shown.contains(&format!("failure number {index:02}")),
            "{shown}"
        );
        let heading = format!("{}. [FAIL] test_many.py::test_f{index:02}", index + 1);
        let section = shown.split_once(&heading).expect("numbered failure").1;
        let section = section.split("\n\n").next().expect("failure details");
        assert!(
            section.contains(&format!("failure number {index:02}")),
            "{section}"
        );
    }
    let complete = recover(&shown);
    for index in 0..14 {
        assert!(
            complete.contains(&format!("test_many.py::test_f{index:02}")),
            "{complete}"
        );
        assert!(
            complete.contains(&format!("failure number {index:02}")),
            "{complete}"
        );
    }
    assert!(
        shown.len() * 100 <= MANY.len() * 80,
        "real report must save at least 20%"
    );
}

#[test]
fn line_failures_without_short_summaries_remain_recoverable() {
    let complete = recover(&pipe(UNNAMED));
    for index in 0..14 {
        assert!(
            complete.contains(&format!("failure number {index:02}")),
            "{complete}"
        );
    }
}

#[test]
fn mixed_line_reports_keep_errors_outcomes_and_real_locations() {
    let shown = pipe(MIXED);
    assert_eq!(shown.matches("[FAIL]").count(), 2, "{shown}");
    assert_eq!(shown.matches("[ERROR]").count(), 1, "{shown}");
    for detail in [
        "test_mixed.py::test_assertion",
        "test_mixed.py::test_error",
        "test_setup",
        "database unavailable",
        "last failure survives",
        "/home/user/project/test_mixed.py:12:",
        "/home/user/project/test_mixed.py:15:",
        "XFAIL test_mixed.py::test_expected",
        "XPASS test_mixed.py::test_unexpected",
    ] {
        assert!(shown.contains(detail), "{detail}: {shown}");
    }
    assert!(!shown.contains("captured.py"), "{shown}");
    assert!(
        shown.len() * 100 <= MIXED.len() * 80,
        "real report must save at least 20%"
    );
}

#[test]
fn a_location_in_an_error_message_is_not_an_extra_failure() {
    let shown = pipe(LOCATIONS);
    assert_eq!(shown.matches("[FAIL]").count(), 2, "{shown}");
    assert!(shown.contains("test_locations.py::test_plain"), "{shown}");
    assert!(shown.contains("test_locations.py::test_message"), "{shown}");
    assert!(shown.contains("unrelated.py:99:"), "{shown}");
    assert!(
        shown.contains("/home/user/project/test_locations.py:2:"),
        "{shown}"
    );
    assert!(
        shown.contains("/home/user/project/test_locations.py:5:"),
        "{shown}"
    );
}

#[test]
fn ambiguous_captured_locations_preserve_the_report() {
    let raw = MIXED.replace(
        "captured.py:7: this is not a traceback",
        "/home/user/project/test_mixed.py:999: this is not a traceback",
    );
    assert_eq!(pipe(&raw), raw);
}

#[test]
fn a_mismatched_short_summary_does_not_mislabel_failures() {
    let raw = LOCATIONS.replace(
        "FAILED test_locations.py::test_message",
        "FAILED other.py::test_message",
    );
    assert_eq!(pipe(&raw), raw);
}

#[test]
fn ordinary_short_tracebacks_keep_their_existing_details() {
    let raw = include_str!("fixtures/pytest_helper_module_raise_raw.txt");
    assert_eq!(
        pipe(raw),
        "Pytest: 0 passed, 1 failed in 0.02s\n\nFailures:\n1. [FAIL] test_frames\n     test_lib.py:5: in test_frames\n     mylib/checks.py:3: in assert_frame_equal\n     E   AssertionError: DataFrame are different\n     E   DataFrame shape mismatch\n     E   [left]:  (3, 2)\n     E   [right]: (3, 4)"
    );
}
