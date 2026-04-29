//! Filters ctest output — passing tests collapsed, failures shown verbatim.

use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    // "Test #N: name ........... Passed   0.42 sec"
    static ref PASS_RE: Regex = Regex::new(r"^\s*Test\s+#\d+:\s+\S+.*\bPassed\b").unwrap();
    // "Start N: TestName"
    static ref START_RE: Regex = Regex::new(r"^\s*Start\s+\d+:\s+\S+").unwrap();
    // "100% tests passed, 0 tests failed out of 15"
    static ref SUMMARY_RE: Regex =
        Regex::new(r"^\s*\d+%\s+tests passed,\s+\d+\s+tests? failed out of \d+").unwrap();
    // "Total Test time (real) =   0.42 sec"
    static ref TIME_RE: Regex = Regex::new(r"^\s*Total Test time").unwrap();
    // "The following tests FAILED:"
    static ref FAILED_HEADER_RE: Regex = Regex::new(r"^\s*The following tests FAILED:").unwrap();
    // Failed test entry: "    1 - TestName (Failed)"
    static ref FAILED_ENTRY_RE: Regex = Regex::new(r"^\s+\d+\s+-\s+\S+\s+\(.*\)").unwrap();
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("ctest");
    for a in args {
        cmd.arg(a);
    }
    if verbose > 0 {
        eprintln!("Running: ctest {}", args.join(" "));
    }
    runner::run_filtered(
        cmd,
        "ctest",
        &args.join(" "),
        filter_output,
        RunOptions::with_tee("ctest"),
    )
}

fn filter_output(raw: &str) -> String {
    let mut summary: Option<String> = None;
    let mut total_time: Option<String> = None;
    let mut failures: Vec<String> = Vec::new();
    let mut failure_capture: Vec<String> = Vec::new();
    let mut in_failed_block = false;
    let mut in_output_block = false;
    let mut current_output: Vec<String> = Vec::new();
    let mut passed = 0usize;
    let mut total_tests = 0usize;

    for line in raw.lines() {
        if PASS_RE.is_match(line) {
            passed += 1;
            total_tests += 1;
            in_output_block = false;
            if !current_output.is_empty() {
                current_output.clear();
            }
            continue;
        }
        if START_RE.is_match(line) {
            total_tests = total_tests.max(extract_test_number(line));
            continue;
        }
        if TIME_RE.is_match(line) {
            total_time = Some(line.trim().to_string());
            continue;
        }
        if let Some(s) = parse_summary(line) {
            summary = Some(s);
            continue;
        }
        if FAILED_HEADER_RE.is_match(line) {
            in_failed_block = true;
            failure_capture.push(line.to_string());
            continue;
        }
        if in_failed_block && (FAILED_ENTRY_RE.is_match(line) || line.trim().is_empty()) {
            failure_capture.push(line.to_string());
            if line.trim().is_empty() {
                in_failed_block = false;
            }
            continue;
        }
        if line.contains("Failed") && line.contains("Test #") {
            failures.push(line.to_string());
            in_output_block = true;
            current_output.clear();
            continue;
        }
        if in_output_block {
            // Captured failed-test output ends at the next "Test #" line or summary block
            if PASS_RE.is_match(line) || line.trim().starts_with("Total Test time") {
                in_output_block = false;
                failure_capture.append(&mut current_output);
            } else {
                current_output.push(line.to_string());
            }
        }
    }

    if !current_output.is_empty() {
        failure_capture.extend(current_output);
    }

    let mut out = String::new();
    if failures.is_empty() && failure_capture.is_empty() {
        let time = total_time
            .as_deref()
            .and_then(extract_seconds)
            .unwrap_or_default();
        if let Some(s) = summary {
            out.push_str(&format!("ctest: {}", s));
            if !time.is_empty() {
                out.push_str(&format!("  {}", time));
            }
        } else if total_tests > 0 {
            out.push_str(&format!("ctest: {}/{} passed", passed, total_tests));
            if !time.is_empty() {
                out.push_str(&format!("  {}", time));
            }
        } else {
            out.push_str("ctest: ok");
        }
        return out;
    }

    for f in &failures {
        out.push_str(f.trim_end());
        out.push('\n');
    }
    if !failure_capture.is_empty() {
        out.push('\n');
        for line in &failure_capture {
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    if let Some(s) = summary {
        out.push('\n');
        out.push_str(&s);
    }
    out
}

fn extract_test_number(line: &str) -> usize {
    line.split_whitespace()
        .find_map(|t| t.trim_end_matches(':').parse::<usize>().ok())
        .unwrap_or(0)
}

fn parse_summary(line: &str) -> Option<String> {
    if !SUMMARY_RE.is_match(line) {
        return None;
    }
    Some(line.trim().to_string())
}

fn extract_seconds(line: &str) -> Option<String> {
    line.split('=').nth(1).map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    #[test]
    fn test_all_pass() {
        let raw = "Test project /tmp/build\n\
            Start 1: test_one\n\
            1/3 Test #1: test_one ......................   Passed    0.01 sec\n\
            Start 2: test_two\n\
            2/3 Test #2: test_two ......................   Passed    0.01 sec\n\
            Start 3: test_three\n\
            3/3 Test #3: test_three ....................   Passed    0.01 sec\n\
            \n\
            100% tests passed, 0 tests failed out of 3\n\
            \n\
            Total Test time (real) =   0.42 sec\n";
        let out = filter_output(raw);
        assert!(out.starts_with("ctest:"));
        assert!(out.contains("100%") || out.contains("3/3"));
        assert!(!out.contains("Start 1:"));
    }

    #[test]
    fn test_failure_keeps_summary() {
        let raw = "Start 1: test_a\n\
            1/2 Test #1: test_a ......................   Passed    0.01 sec\n\
            Start 2: test_b\n\
            2/2 Test #2: test_b ......................***Failed    0.05 sec\n\
                assertion failed: expected 4, got 5\n\
            \n\
            50% tests passed, 1 tests failed out of 2\n\
            \n\
            The following tests FAILED:\n\
            \t  2 - test_b (Failed)\n\
            \n\
            Errors while running CTest\n";
        let out = filter_output(raw);
        assert!(out.contains("test_b"));
        assert!(out.contains("Failed"));
        assert!(out.contains("50% tests passed"));
        assert!(!out.contains("Start 1:"));
    }

    #[test]
    fn test_fixture_success() {
        let raw = include_str!("../../../tests/fixtures/cpp/ctest_success.txt");
        let out = filter_output(raw);
        assert!(out.starts_with("ctest:"));
        assert!(!out.contains("Start  1:"));
    }

    #[test]
    fn test_fixture_failure() {
        let raw = include_str!("../../../tests/fixtures/cpp/ctest_failure.txt");
        let out = filter_output(raw);
        assert!(out.contains("test_parser_failure"));
        assert!(out.contains("test_runtime_failure"));
        assert!(out.contains("60% tests passed"));
    }

    #[test]
    fn test_savings() {
        let mut raw = String::new();
        for i in 1..=20 {
            raw.push_str(&format!("Start {}: test_{}\n", i, i));
            raw.push_str(&format!(
                "{}/20 Test #{}: test_{} ........................   Passed    0.01 sec\n",
                i, i, i
            ));
        }
        raw.push_str("100% tests passed, 0 tests failed out of 20\n");
        let out = filter_output(&raw);
        let savings = 100.0 - (count_tokens(&out) as f64 / count_tokens(&raw) as f64 * 100.0);
        assert!(savings >= 60.0, "expected >=60%, got {:.1}%", savings);
    }
}
