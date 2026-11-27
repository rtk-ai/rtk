//! Real pytest 9.1.1 reports captured with CPython 3.12.3 and sys capture.
//! New fixtures normalize only elapsed time and trailing whitespace.

use std::io::Write;
use std::process::Stdio;

mod common;

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

fn assert_error_window(shown: &str, total: usize, hidden: usize) {
    assert!(shown.starts_with("Pytest: 0 passed, 1 failed"), "{shown}");
    assert_eq!(shown.matches("[FAIL]").count(), 1, "{shown}");
    let visible = shown
        .lines()
        .filter(|line| line.starts_with("     E"))
        .count();
    assert_eq!(visible, 4, "{shown}");
    let noun = if hidden == 1 { "line" } else { "lines" };
    assert!(
        shown.contains(&format!("… +{hidden} more E {noun}")),
        "{shown}"
    );
    assert_eq!(shown.matches("more E line").count(), 1, "{shown}");
    assert!(!shown.contains("\n     E\n"), "{shown}");
    assert_eq!(visible + hidden, total, "{shown}");
}

#[test]
fn heading_diffs_count_all_hidden_error_lines() {
    for raw in [
        include_str!("fixtures/pytest_equals_heading_short_raw.txt"),
        include_str!("fixtures/pytest_equals_heading_open_raw.txt"),
        // This heading has exactly the same indentation as a local named E.
        include_str!("fixtures/pytest_equals_heading_indented_raw.txt"),
    ] {
        let shown = pipe(raw);
        assert_error_window(&shown, 7, 3);
        assert!(
            shown.lines().any(|line| line
                .strip_prefix("     E ")
                .is_some_and(|rest| rest.trim_start().starts_with("= Title"))),
            "{shown}"
        );
        assert!(shown.contains("E     + body a"), "{shown}");
        assert!(
            shown.len() * 100 <= raw.len() * 80,
            "real heading report must save at least 20%: {shown}"
        );
    }
}

#[test]
fn long_traceback_separates_error_lines_from_locals() {
    let raw = include_str!("fixtures/pytest_equals_heading_locals_raw.txt");
    let shown = pipe(raw);
    assert_error_window(&shown, 7, 3);
    assert!(!shown.contains("context"), "{shown}");
    assert!(
        shown.contains("test_locals.py:3: AssertionError"),
        "{shown}"
    );
    assert!(shown.contains("E         + body a"), "{shown}");
}

#[test]
fn blank_error_separators_count_before_and_after_trimming() {
    let raw = include_str!("fixtures/pytest_rst_heading_diff_raw.txt");
    let trimmed = raw
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    for report in [raw, trimmed.as_str()] {
        let shown = pipe(report);
        assert_error_window(&shown, 7, 3);
        assert!(shown.contains("E       ====="), "{shown}");
    }
}

#[test]
fn a_local_named_e_does_not_enter_the_error_budget() {
    let raw = include_str!("fixtures/pytest_tb_long_locals_named_e_raw.txt");
    let shown = pipe(raw);
    assert_error_window(&shown, 6, 2);
    assert!(!shown.contains("E          ="), "{shown}");
    assert!(!shown.contains("Ex         ="), "{shown}");
    assert!(shown.contains("AssertionError: state drifted"), "{shown}");
    assert!(shown.contains("Use -v to get more diff"), "{shown}");
}

#[test]
fn blank_error_separators_do_not_displace_useful_messages() {
    let raw = include_str!("fixtures/pytest_helper_module_raise_raw.txt");
    let shown = pipe(raw);
    assert_error_window(&shown, 5, 1);
    assert_eq!(
        shown,
        "Pytest: 0 passed, 1 failed in 0.02s\n\nFailures:\n1. [FAIL] test_frames\n     test_lib.py:5: in test_frames\n     mylib/checks.py:3: in assert_frame_equal\n     E   AssertionError: DataFrame are different\n     E   DataFrame shape mismatch\n     … +1 more E line\n     E   [left]:  (3, 2)\n     E   [right]: (3, 4)"
    );
}

#[test]
fn ordinary_short_tracebacks_keep_the_existing_report() {
    let raw = include_str!("fixtures/pytest_single_e_raise_raw.txt");
    assert_eq!(
        pipe(raw),
        "Pytest: 0 passed, 1 failed in 0.01s\n\nFailures:\n1. [FAIL] test_raise\n     test_r.py:2: in test_raise\n     raise ValueError(\"boom\")\n     E   ValueError: boom"
    );
}
