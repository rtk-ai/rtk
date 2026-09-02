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
        crate::core::runner::RunOptions::with_tee("test"),
    )
}

#[cfg(test)]
fn filter_errors(output: &str) -> String {
    let mut result = Vec::new();
    let mut in_error_block = false;
    let mut blank_count = 0;

    for line in output.lines() {
        let is_error_line = ERROR_PATTERNS.iter().any(|p| p.is_match(line));

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

fn extract_test_summary(output: &str, command: &str, exit_code: i32) -> String {
    let mut result = Vec::new();
    let lines: Vec<&str> = output.lines().collect();

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

    if !failures.is_empty() {
        output.push_str("[FAIL] FAILURES:\n");
        for f in failures.iter().take(MAX_RUNNER_FAILURES) {
            output.push_str(&format!("  {}\n", f));
        }
        if failures.len() > MAX_RUNNER_FAILURES {
            output.push_str(&format!(
                "  ... +{} more failures\n",
                failures.len() - MAX_RUNNER_FAILURES
            ));
        }
        for f in failure_lines.iter().take(MAX_RUNNER_LINES) {
            output.push_str(&format!("  {}\n", f.trim()));
        }
        if failure_lines.len() > MAX_RUNNER_LINES {
            output.push_str(&format!(
                "  ... +{} more\n",
                failure_lines.len() - MAX_RUNNER_LINES
            ));
        }
        output.push('\n');
    }

    if !result.is_empty() {
        output.push_str("SUMMARY:\n");
        for r in &result {
            output.push_str(&format!("  {}\n", r));
        }
    } else {
        let mut error_lines = Vec::new();
        let mut in_error_block = false;
        let mut blank_count: usize = 0;
        for line in lines.iter() {
            let is_error = ERROR_PATTERNS.iter().any(|p| p.is_match(line));
            if is_error {
                in_error_block = true;
                blank_count = 0;
                error_lines.push(*line);
            } else if in_error_block {
                if line.trim().is_empty() {
                    blank_count += 1;
                    if blank_count >= 2 {
                        in_error_block = false;
                    } else {
                        error_lines.push(*line);
                    }
                } else if line.starts_with(' ') || line.starts_with('\t') {
                    blank_count = 0;
                    error_lines.push(*line);
                } else {
                    in_error_block = false;
                }
            }
        }

        if !error_lines.is_empty() {
            output.push_str("[FAIL] DETECTED FAILURES:\n");
            for line in error_lines.iter().take(MAX_RUNNER_LINES) {
                output.push_str(&format!("  {}\n", line));
            }
            if error_lines.len() > MAX_RUNNER_LINES {
                output.push_str(&format!(
                    "  ... +{} more\n",
                    error_lines.len() - MAX_RUNNER_LINES
                ));
            }
        } else if exit_code != 0 {
            output.push_str(&format!(
                "[FAIL] Command failed (exit code: {})\n",
                exit_code
            ));
            let start = lines.len().saturating_sub(10);
            for line in &lines[start..] {
                if !line.trim().is_empty() {
                    output.push_str(&format!("  {}\n", line));
                }
            }
        } else {
            output.push_str("[ok] All tests passed (no recognized test runner)\n");
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
    fn test_generic_runner_surfaces_failure_lines() {
        let output = "running tests\nsetup complete\nerror: assertion failed at test_foo\n  expected: 42\n  got: 0\ncleanup done";
        let result = extract_test_summary(output, "make test", 1);
        assert!(
            result.contains("[FAIL] DETECTED FAILURES:"),
            "should detect error patterns: {}",
            result
        );
        assert!(result.contains("error: assertion failed"));
    }

    #[test]
    fn test_generic_runner_nonzero_no_patterns() {
        let output = "running tests\ntest 1 ok\ntest 2 ok\ntest 3 ok\nsome info\nmore info\nfinal line";
        let result = extract_test_summary(output, "make test", 2);
        assert!(
            result.contains("[FAIL] Command failed (exit code: 2)"),
            "should show exit code: {}",
            result
        );
        assert!(result.contains("final line"));
    }

    #[test]
    fn test_generic_runner_success() {
        let output = "running tests\ntest 1 ok\ntest 2 ok\nall good";
        let result = extract_test_summary(output, "make test", 0);
        assert!(
            result.contains("[ok] All tests passed (no recognized test runner)"),
            "should show success: {}",
            result
        );
    }

    #[test]
    fn test_recognized_runners_unchanged() {
        let output = "test result: ok. 5 passed; 0 failed\n";
        let result = extract_test_summary(output, "cargo test", 0);
        assert!(result.contains("SUMMARY:"));
        assert!(result.contains("test result:"));
    }
}
