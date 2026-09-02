//! Runs arbitrary commands and captures only stderr or test failures.

use crate::core::stream::StreamFilter;
use crate::core::truncate::{CAP_LIST, CAP_WARNINGS};
use anyhow::Result;
use regex::Regex;
use std::process::Command;
use std::sync::LazyLock;

const MAX_RUNNER_FAILURES: usize = CAP_WARNINGS;
const MAX_RUNNER_LINES: usize = CAP_LIST;

static ERROR_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // Generic errors
        Regex::new(r"(?i)^.*error[\s:\[].*$").unwrap(),
        Regex::new(r"(?i)^.*\berr\b.*$").unwrap(),
        Regex::new(r"(?i)^.*warning[\s:\[].*$").unwrap(),
        Regex::new(r"(?i)^.*\bwarn\b.*$").unwrap(),
        Regex::new(r"(?i)^.*failed.*$").unwrap(),
        Regex::new(r"(?i)^.*failure.*$").unwrap(),
        Regex::new(r"(?i)^.*exception.*$").unwrap(),
        Regex::new(r"(?i)^.*panic.*$").unwrap(),
        // Rust specific
        Regex::new(r"^error\[E\d+\]:.*$").unwrap(),
        Regex::new(r"^\s*--> .*:\d+:\d+$").unwrap(),
        // Python
        Regex::new(r"^Traceback.*$").unwrap(),
        Regex::new(r#"^\s*File ".*", line \d+.*$"#).unwrap(),
        // JavaScript/TypeScript
        Regex::new(r"^\s*at .*:\d+:\d+.*$").unwrap(),
        // Go
        Regex::new(r"^.*\.go:\d+:.*$").unwrap(),
    ]
});

static TEST_FAIL_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^.*\bfail\b.*$").unwrap());
static ZERO_OUTCOME_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:0\s+(?:test\s+)?(?:failed|failures?|errors?|warnings?)|(?:failures?|errors?|warnings?)\s*:\s*0)\b",
    )
    .unwrap()
});

struct ErrorStreamFilter {
    in_error_block: bool,
    blank_count: usize,
    emitted_any: bool,
}

impl ErrorStreamFilter {
    fn new() -> Self {
        Self {
            in_error_block: false,
            blank_count: 0,
            emitted_any: false,
        }
    }
}

impl StreamFilter for ErrorStreamFilter {
    fn feed_line(&mut self, line: &str) -> Option<String> {
        let is_error = ERROR_PATTERNS.iter().any(|p| p.is_match(line));
        if is_error {
            self.in_error_block = true;
            self.blank_count = 0;
            self.emitted_any = true;
            Some(format!("{}\n", line))
        } else if self.in_error_block {
            if line.trim().is_empty() {
                self.blank_count += 1;
                if self.blank_count >= 2 {
                    self.in_error_block = false;
                    None
                } else {
                    self.emitted_any = true;
                    Some(format!("{}\n", line))
                }
            } else if line.starts_with(' ') || line.starts_with('\t') {
                self.blank_count = 0;
                self.emitted_any = true;
                Some(format!("{}\n", line))
            } else {
                self.in_error_block = false;
                None
            }
        } else {
            None
        }
    }

    fn flush(&mut self) -> String {
        String::new()
    }

    fn on_exit(&mut self, exit_code: i32, raw: &str) -> Option<String> {
        if self.emitted_any {
            return None;
        }
        if exit_code == 0 {
            Some("[ok] Command completed successfully (no errors)".to_string())
        } else {
            let mut msg = format!("[FAIL] Command failed (exit code: {})\n", exit_code);
            let lines: Vec<&str> = raw.lines().collect();
            for line in lines.iter().rev().take(10).rev() {
                msg.push_str(&format!("  {}\n", line));
            }
            Some(msg)
        }
    }
}

fn build_shell_command(command: &str) -> Command {
    if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    }
}

/// Run a command and filter output to show only errors/warnings
pub fn run_err(command: &str, verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("Running: {}", command);
    }
    let cmd = build_shell_command(command);
    crate::core::runner::run_streamed(
        cmd,
        "err",
        command,
        Box::new(ErrorStreamFilter::new()),
        crate::core::runner::RunOptions::with_tee("err"),
    )
}

/// Run tests and show only failures
pub fn run_test(command: &str, verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("Running tests: {}", command);
    }
    let cmd = build_shell_command(command);
    let command_owned = command.to_string();
    crate::core::runner::run_filtered_with_exit(
        cmd,
        "test",
        command,
        move |raw, exit_code| extract_test_summary(raw, &command_owned, exit_code),
        crate::core::runner::RunOptions::with_tee("test").authoritative_failure_output(),
    )
}

fn filter_errors(output: &str) -> String {
    let mut result = Vec::new();
    let mut in_error_block = false;
    let mut blank_count = 0;

    for line in output.lines() {
        // Ignore zero-valued outcome counters such as "12 passed, 0 failed";
        // otherwise they can hide the actual unclassified diagnostic by
        // suppressing the safer first/last excerpt fallback.
        let without_zero_outcomes = ZERO_OUTCOME_PATTERN.replace_all(line, "");
        let is_error_line = TEST_FAIL_PATTERN.is_match(&without_zero_outcomes)
            || ERROR_PATTERNS
                .iter()
                .any(|pattern| pattern.is_match(&without_zero_outcomes));

        if is_error_line {
            in_error_block = true;
            blank_count = 0;
            result.push(line.to_string());
        } else if in_error_block {
            if line.trim().is_empty() {
                blank_count += 1;
                if blank_count >= 2 {
                    in_error_block = false;
                } else {
                    result.push(line.to_string());
                }
            } else if line.starts_with(' ') || line.starts_with('\t') {
                result.push(line.to_string());
                blank_count = 0;
            } else {
                in_error_block = false;
            }
        }
    }

    result.join("\n")
}

fn append_failure_excerpt(output: &mut String, lines: &[&str], anchors: &[&str]) {
    let nonempty_indices: Vec<_> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (!line.trim().is_empty()).then_some(index))
        .collect();
    let mut selected = std::collections::BTreeSet::new();

    for index in nonempty_indices.iter().take(5) {
        selected.insert(*index);
    }
    for index in nonempty_indices.iter().rev().take(5) {
        selected.insert(*index);
    }

    let mut anchor_search_start = 0;
    for anchor in anchors.iter().take(MAX_RUNNER_LINES) {
        let Some(offset) = lines[anchor_search_start..]
            .iter()
            .position(|line| line == anchor)
        else {
            continue;
        };
        let index = anchor_search_start + offset;
        anchor_search_start = index + 1;

        let Ok(nonempty_position) = nonempty_indices.binary_search(&index) else {
            continue;
        };
        let start = nonempty_position.saturating_sub(2);
        let end = (nonempty_position + 2).min(nonempty_indices.len().saturating_sub(1));
        for context_index in nonempty_indices.iter().take(end + 1).skip(start) {
            selected.insert(*context_index);
        }
    }

    let mut previous = None;
    for index in selected {
        if let Some(previous_index) = previous {
            if index > previous_index + 1 {
                output.push_str(&format!(
                    "  ... {} lines omitted ...\n",
                    index - previous_index - 1
                ));
            }
        }
        output.push_str(&format!("  {}\n", lines[index]));
        previous = Some(index);
    }
}

fn extract_test_summary(raw_output: &str, command: &str, exit_code: i32) -> String {
    let mut result = Vec::new();
    let lines: Vec<&str> = raw_output.lines().collect();

    let is_cargo = command.contains("cargo test");
    let is_pytest = command.contains("pytest");
    let is_jest =
        command.contains("jest") || command.contains("npm test") || command.contains("yarn test");
    let is_go = command.contains("go test");

    let mut failures = Vec::new();
    let mut in_failure = false;
    let mut failure_lines = Vec::new();

    for line in lines.iter() {
        if is_cargo {
            if line.contains("test result:") {
                result.push(line.to_string());
            }
            if line.contains("FAILED") && !line.contains("test result") {
                failures.push(line.to_string());
            }
            if line.starts_with("failures:") {
                in_failure = true;
            }
            if in_failure && line.starts_with("    ") {
                failure_lines.push(line.to_string());
            }
        }

        if is_pytest {
            if line.contains(" passed") || line.contains(" failed") || line.contains(" error") {
                result.push(line.to_string());
            }
            if line.contains("FAILED") {
                failures.push(line.to_string());
            }
        }

        if is_jest {
            if line.contains("Tests:") || line.contains("Test Suites:") {
                result.push(line.to_string());
            }
            if line.contains("✕") || line.contains("FAIL") {
                failures.push(line.to_string());
            }
        }

        if is_go {
            if line.starts_with("ok") || line.starts_with("FAIL") || line.starts_with("---") {
                result.push(line.to_string());
            }
            if line.contains("FAIL") {
                failures.push(line.to_string());
            }
        }
    }

    let mut output = String::new();

    // The child status is authoritative. A successful runner may legitimately
    // mention recovered or expected failures, while a failing runner must never
    // be reduced to a success-looking summary.
    if exit_code == 0 {
        if !result.is_empty() {
            output.push_str("SUMMARY:\n");
            for line in &result {
                output.push_str(&format!("  {}\n", line));
            }
        } else {
            output.push_str("[ok] All tests passed (unrecognized test runner)\n");
        }
        return output;
    }

    if !failures.is_empty() {
        output.push_str("[FAIL] FAILURES:\n");
        for failure in failures.iter().take(MAX_RUNNER_FAILURES) {
            output.push_str(&format!("  {}\n", failure));
        }
        if failures.len() > MAX_RUNNER_FAILURES {
            output.push_str(&format!(
                "  ... +{} more failures\n",
                failures.len() - MAX_RUNNER_FAILURES
            ));
        }
        for line in failure_lines.iter().take(MAX_RUNNER_LINES) {
            output.push_str(&format!("  {}\n", line.trim()));
        }
        if failure_lines.len() > MAX_RUNNER_LINES {
            output.push_str(&format!(
                "  ... +{} more\n",
                failure_lines.len() - MAX_RUNNER_LINES
            ));
        }
        output.push('\n');
        output.push_str("CONTEXT:\n");
        let failure_anchors: Vec<_> = failures
            .iter()
            .take(MAX_RUNNER_FAILURES)
            .map(String::as_str)
            .collect();
        append_failure_excerpt(&mut output, &lines, &failure_anchors);
    } else {
        let detected_errors = filter_errors(raw_output);
        if !detected_errors.is_empty() {
            output.push_str("[FAIL] DETECTED FAILURES:\n");
            let error_lines: Vec<_> = detected_errors.lines().collect();
            for line in error_lines.iter().take(MAX_RUNNER_LINES) {
                output.push_str(&format!("  {}\n", line));
            }
            if error_lines.len() > MAX_RUNNER_LINES {
                output.push_str(&format!(
                    "  ... +{} more\n",
                    error_lines.len() - MAX_RUNNER_LINES
                ));
            }
            output.push('\n');
            output.push_str("CONTEXT:\n");
            append_failure_excerpt(&mut output, &lines, &error_lines);
        } else {
            output.push_str(&format!(
                "[FAIL] Command failed (exit code: {})\n",
                exit_code
            ));

            append_failure_excerpt(&mut output, &lines, &[]);
        }
    }

    if !result.is_empty() {
        output.push_str("SUMMARY:\n");
        for line in &result {
            output.push_str(&format!("  {}\n", line));
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_errors() {
        let output = "info: compiling\nerror: something failed\n  at line 10\ninfo: done";
        let filtered = filter_errors(output);
        assert!(filtered.contains("error"));
        assert!(!filtered.contains("info"));
    }

    #[test]
    fn test_filter_errors_ignores_zero_failure_counters() {
        assert!(filter_errors("12 passed, 0 failed").is_empty());
        assert!(filter_errors("12 passed, 0 test failures").is_empty());
        assert!(filter_errors("Failures: 0").is_empty());
        assert!(filter_errors("12 passed, 1 failed").contains("1 failed"));
        assert!(filter_errors("Failures: 1").contains("Failures: 1"));
    }

    #[test]
    fn test_standalone_fail_is_scoped_to_test_filter() {
        let mut stream = ErrorStreamFilter::new();
        assert_eq!(stream.feed_line("FAIL: expected assertion"), None);
        assert!(filter_errors("FAIL: expected assertion").contains("FAIL:"));
    }

    #[test]
    fn test_nonzero_classified_runner_keeps_unrecognized_diagnostic() {
        let mut raw = String::from("12 passed, 0 failed\nprocess terminated by sentinel 417\n");
        for index in 1..=20 {
            raw.push_str(&format!("cleanup step {index}\n"));
        }
        let filtered = extract_test_summary(&raw, "pytest-wrapper", 9);
        assert!(filtered.contains("process terminated by sentinel 417"));
        assert!(filtered.contains("cleanup step 20"));
        assert!(filtered.contains("SUMMARY:"));
    }

    #[test]
    fn test_anchor_context_keeps_middle_adjacent_diagnostic() {
        let lines = [
            "setup 1",
            "setup 2",
            "setup 3",
            "setup 4",
            "setup 5",
            "setup 6",
            "FAILED test_middle",
            "diagnostic sentinel MIDDLE-417",
            "cleanup 1",
            "cleanup 2",
            "cleanup 3",
            "cleanup 4",
            "cleanup 5",
            "cleanup 6",
        ];
        let mut output = String::new();
        append_failure_excerpt(&mut output, &lines, &["FAILED test_middle"]);
        assert!(output.contains("diagnostic sentinel MIDDLE-417"));
    }

    #[test]
    fn test_zero_exit_never_emits_failure_banner() {
        let filtered = extract_test_summary("FAILED but recovered\n", "pytest-wrapper", 0);
        assert!(!filtered.contains("[FAIL]"));
    }
}
