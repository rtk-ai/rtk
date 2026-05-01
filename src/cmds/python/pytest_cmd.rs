//! Filters pytest output to show only failures and the summary line.

use crate::core::runner;
use crate::core::utils::{resolved_command, tool_exists, truncate};
use anyhow::Result;

#[derive(Debug, PartialEq)]
enum ParseState {
    Header,
    TestProgress,
    Failures,
    Summary,
}

#[derive(Debug, Default, PartialEq)]
struct SummaryCounts {
    passed: usize,
    failed: usize,
    errors: usize,
    skipped: usize,
    xfailed: usize,
    xpassed: usize,
    deselected: usize,
}

impl SummaryCounts {
    fn total(&self) -> usize {
        self.passed
            + self.failed
            + self.errors
            + self.skipped
            + self.xfailed
            + self.xpassed
            + self.deselected
    }

    fn has_failure_details(&self) -> bool {
        self.failed > 0 || self.errors > 0
    }

    fn display_parts(&self) -> Vec<String> {
        let mut parts = Vec::new();

        if self.passed > 0 {
            parts.push(format_outcome(self.passed, "passed", "passed"));
        }
        if self.failed > 0 {
            parts.push(format_outcome(self.failed, "failed", "failed"));
        }
        if self.errors > 0 {
            parts.push(format_outcome(self.errors, "error", "errors"));
        }
        if self.skipped > 0 {
            parts.push(format_outcome(self.skipped, "skipped", "skipped"));
        }
        if self.xfailed > 0 {
            parts.push(format_outcome(self.xfailed, "xfailed", "xfailed"));
        }
        if self.xpassed > 0 {
            parts.push(format_outcome(self.xpassed, "xpassed", "xpassed"));
        }
        if self.deselected > 0 {
            parts.push(format_outcome(self.deselected, "deselected", "deselected"));
        }

        parts
    }
}

fn format_outcome(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("1 {}", singular)
    } else {
        format!("{} {}", count, plural)
    }
}

fn is_summary_line(line: &str) -> bool {
    let normalized = line.trim().trim_matches('=').trim().to_lowercase();
    if normalized.is_empty() {
        return false;
    }

    if normalized.starts_with("no tests ran") {
        return true;
    }

    let Some(first_token) = normalized.split_whitespace().next() else {
        return false;
    };
    if first_token.parse::<usize>().is_err() {
        return false;
    }

    normalized.contains(" passed")
        || normalized.contains(" failed")
        || normalized.contains(" skipped")
        || normalized.contains(" deselected")
        || normalized.contains(" xfailed")
        || normalized.contains(" xpassed")
        || normalized.contains(" error")
        || normalized.contains(" errors")
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = if tool_exists("pytest") {
        resolved_command("pytest")
    } else {
        let mut c = resolved_command("python");
        c.arg("-m").arg("pytest");
        c
    };

    let has_tb_flag = args.iter().any(|a| a.starts_with("--tb"));
    let has_quiet_flag = args.iter().any(|a| a == "-q" || a == "--quiet");

    if !has_tb_flag {
        cmd.arg("--tb=short");
    }
    if !has_quiet_flag {
        cmd.arg("-q");
    }

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: pytest --tb=short -q {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "pytest",
        &args.join(" "),
        filter_pytest_output,
        runner::RunOptions::stdout_only().tee("pytest"),
    )
}

pub(crate) fn filter_pytest_output(output: &str) -> String {
    let mut state = ParseState::Header;
    let mut test_files: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    let mut current_failure: Vec<String> = Vec::new();
    let mut summary_line = String::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // State transitions
        if trimmed.starts_with("===") && trimmed.contains("test session starts") {
            state = ParseState::Header;
            continue;
        } else if trimmed.starts_with("===") && trimmed.contains("FAILURES") {
            state = ParseState::Failures;
            continue;
        } else if trimmed.starts_with("===") && trimmed.contains("short test summary") {
            state = ParseState::Summary;
            // Save current failure if any
            if !current_failure.is_empty() {
                failures.push(current_failure.join("\n"));
                current_failure.clear();
            }
            continue;
        } else if is_summary_line(trimmed) {
            summary_line = trimmed.to_string();
            continue;
        }

        // Process based on state
        match state {
            ParseState::Header => {
                if trimmed.starts_with("collected") {
                    state = ParseState::TestProgress;
                }
            }
            ParseState::TestProgress => {
                // Lines like "tests/test_foo.py ....  [ 40%]"
                if !trimmed.is_empty()
                    && !trimmed.starts_with("===")
                    && (trimmed.contains(".py") || trimmed.contains("%]"))
                {
                    test_files.push(trimmed.to_string());
                }
            }
            ParseState::Failures => {
                // Collect failure details
                if trimmed.starts_with("___") {
                    // New failure section
                    if !current_failure.is_empty() {
                        failures.push(current_failure.join("\n"));
                        current_failure.clear();
                    }
                    current_failure.push(trimmed.to_string());
                } else if !trimmed.is_empty() && !trimmed.starts_with("===") {
                    current_failure.push(trimmed.to_string());
                }
            }
            ParseState::Summary => {
                // FAILED test lines
                if trimmed.starts_with("FAILED") || trimmed.starts_with("ERROR") {
                    failures.push(trimmed.to_string());
                }
            }
        }
    }

    // Save last failure if any
    if !current_failure.is_empty() {
        failures.push(current_failure.join("\n"));
    }

    // Build compact output
    build_pytest_summary(&summary_line, &test_files, &failures)
}

fn build_pytest_summary(summary: &str, _test_files: &[String], failures: &[String]) -> String {
    if summary.to_lowercase().contains("no tests ran") {
        return "Pytest: No tests collected".to_string();
    }

    let counts = parse_summary_line(summary);

    if counts.total() == 0 {
        return "Pytest: No tests collected".to_string();
    }

    if counts.failed == 0
        && counts.errors == 0
        && counts.skipped == 0
        && counts.xfailed == 0
        && counts.xpassed == 0
        && counts.deselected == 0
        && counts.passed > 0
    {
        return format!("Pytest: {} passed", counts.passed);
    }

    let mut result = format!("Pytest: {}", counts.display_parts().join(", "));

    if !counts.has_failure_details() {
        return result;
    }

    result.push('\n');
    result.push_str("═══════════════════════════════════════\n");

    if failures.is_empty() {
        return result.trim().to_string();
    }

    // Show failures (limit to key information)
    result.push_str("\nFailures:\n");

    for (i, failure) in failures.iter().take(5).enumerate() {
        // Extract test name and key error info
        let lines: Vec<&str> = failure.lines().collect();

        // First line is usually test name (after ___)
        if let Some(first_line) = lines.first() {
            if first_line.starts_with("___") {
                // Extract test name between ___
                let test_name = first_line.trim_matches('_').trim();
                result.push_str(&format!("{}. [FAIL] {}\n", i + 1, test_name));
            } else if first_line.starts_with("FAILED") || first_line.starts_with("ERROR") {
                // Summary format: "FAILED/ERROR tests/test_foo.py::test_bar - AssertionError"
                let parts: Vec<&str> = first_line.split(" - ").collect();
                if let Some(test_path) = parts.first() {
                    let test_name = test_path
                        .trim_start_matches("FAILED ")
                        .trim_start_matches("ERROR ");
                    let status = if first_line.starts_with("ERROR") {
                        "ERROR"
                    } else {
                        "FAIL"
                    };
                    result.push_str(&format!("{}. [{}] {}\n", i + 1, status, test_name));
                }
                if parts.len() > 1 {
                    result.push_str(&format!("     {}\n", truncate(parts[1], 100)));
                }
                continue;
            }
        }

        // Show relevant error lines (assertions, errors, file locations)
        let mut relevant_lines = 0;
        for line in &lines[1..] {
            let line_lower = line.to_lowercase();
            let is_relevant = line.trim().starts_with('>')
                || line.trim().starts_with('E')
                || line_lower.contains("assert")
                || line_lower.contains("error")
                || line.contains(".py:");

            if is_relevant && relevant_lines < 3 {
                result.push_str(&format!("     {}\n", truncate(line, 100)));
                relevant_lines += 1;
            }
        }

        if i < failures.len() - 1 {
            result.push('\n');
        }
    }

    if failures.len() > 5 {
        result.push_str(&format!("\n... +{} more failures\n", failures.len() - 5));
    }

    result.trim().to_string()
}

fn parse_summary_line(summary: &str) -> SummaryCounts {
    let mut counts = SummaryCounts::default();

    // Parse lines like "=== 4 passed, 1 failed in 0.50s ===" or
    // "1 passed, 1 error, 2 deselected in 0.50s"
    let parts: Vec<&str> = summary.split(',').collect();

    for part in parts {
        let words: Vec<&str> = part.split_whitespace().collect();
        for (i, word) in words.iter().enumerate() {
            if i == 0 {
                continue;
            }

            let Ok(n) = words[i - 1].parse::<usize>() else {
                continue;
            };

            match word.trim_matches(|c: char| c == ',' || c == '=') {
                "passed" => counts.passed = n,
                "failed" => counts.failed = n,
                "error" | "errors" => counts.errors = n,
                "skipped" => counts.skipped = n,
                "xfailed" => counts.xfailed = n,
                "xpassed" => counts.xpassed = n,
                "deselected" => counts.deselected = n,
                _ => {}
            }
        }
    }

    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_pytest_all_pass() {
        let output = r#"=== test session starts ===
platform darwin -- Python 3.11.0
collected 5 items

tests/test_foo.py .....                                            [100%]

=== 5 passed in 0.50s ==="#;

        let result = filter_pytest_output(output);
        assert!(result.contains("Pytest"));
        assert!(result.contains("5 passed"));
    }

    #[test]
    fn test_filter_pytest_quiet_all_pass() {
        let output = r#".........................                                                [100%]
25 passed in 3.92s"#;

        let result = filter_pytest_output(output);
        assert_eq!(result, "Pytest: 25 passed");
    }

    #[test]
    fn test_filter_pytest_with_failures() {
        let output = r#"=== test session starts ===
collected 5 items

tests/test_foo.py ..F..                                            [100%]

=== FAILURES ===
___ test_something ___

    def test_something():
>       assert False
E       assert False

tests/test_foo.py:10: AssertionError

=== short test summary info ===
FAILED tests/test_foo.py::test_something - assert False
=== 4 passed, 1 failed in 0.50s ==="#;

        let result = filter_pytest_output(output);
        assert!(result.contains("4 passed, 1 failed"));
        assert!(result.contains("test_something"));
        assert!(result.contains("assert False"));
    }

    #[test]
    fn test_filter_pytest_multiple_failures() {
        let output = r#"=== test session starts ===
collected 3 items

tests/test_foo.py FFF                                              [100%]

=== FAILURES ===
___ test_one ___
E   AssertionError: expected 5

___ test_two ___
E   ValueError: invalid value

=== short test summary info ===
FAILED tests/test_foo.py::test_one - AssertionError: expected 5
FAILED tests/test_foo.py::test_two - ValueError: invalid value
FAILED tests/test_foo.py::test_three - KeyError
=== 3 failed in 0.20s ==="#;

        let result = filter_pytest_output(output);
        assert!(result.contains("3 failed"));
        assert!(result.contains("test_one"));
        assert!(result.contains("test_two"));
        assert!(result.contains("expected 5"));
    }

    #[test]
    fn test_filter_pytest_no_tests() {
        let output = r#"=== test session starts ===
collected 0 items

=== no tests ran in 0.00s ==="#;

        let result = filter_pytest_output(output);
        assert!(result.contains("No tests collected"));
    }

    #[test]
    fn test_filter_pytest_quiet_no_tests() {
        let output = r#"no tests ran in 0.00s"#;

        let result = filter_pytest_output(output);
        assert_eq!(result, "Pytest: No tests collected");
    }

    #[test]
    fn test_filter_pytest_quiet_failures() {
        let output = r#"..F..                                                                    [100%]

=================================== FAILURES ===================================
_______________________________ test_something ________________________________

    def test_something():
>       assert False
E       assert False

tests/test_foo.py:10: AssertionError

=========================== short test summary info ============================
FAILED tests/test_foo.py::test_something - assert False
4 passed, 1 failed in 0.50s"#;

        let result = filter_pytest_output(output);
        assert!(result.contains("4 passed, 1 failed"));
        assert!(result.contains("test_something"));
    }

    #[test]
    fn test_filter_pytest_quiet_error_summary() {
        let output = r#"E                                                                       [100%]

=========================== short test summary info ============================
ERROR tests/test_foo.py::test_setup - RuntimeError: boom
1 error in 0.20s"#;

        let result = filter_pytest_output(output);
        assert!(result.contains("Pytest: 1 error"));
        assert!(result.contains("test_setup"));
        assert!(result.contains("RuntimeError: boom"));
    }

    #[test]
    fn test_filter_pytest_quiet_xfailed_summary() {
        let output = r#".x                                                                      [100%]
1 passed, 1 xfailed in 0.10s"#;

        let result = filter_pytest_output(output);
        assert_eq!(result, "Pytest: 1 passed, 1 xfailed");
    }

    #[test]
    fn test_filter_pytest_quiet_deselected_summary() {
        let output = r#"2 deselected in 0.02s"#;

        let result = filter_pytest_output(output);
        assert_eq!(result, "Pytest: 2 deselected");
    }

    #[test]
    fn test_is_summary_line_detects_quiet_summary() {
        assert!(is_summary_line("25 passed in 3.92s"));
        assert!(is_summary_line("4 passed, 1 failed in 0.50s"));
        assert!(is_summary_line("no tests ran in 0.00s"));
        assert!(is_summary_line("2 deselected in 0.02s"));
        assert!(!is_summary_line("E       AssertionError: expected 5"));
    }

    #[test]
    fn test_parse_summary_line() {
        assert_eq!(
            parse_summary_line("=== 5 passed in 0.50s ==="),
            SummaryCounts {
                passed: 5,
                ..Default::default()
            }
        );
        assert_eq!(
            parse_summary_line("=== 4 passed, 1 failed in 0.50s ==="),
            SummaryCounts {
                passed: 4,
                failed: 1,
                ..Default::default()
            }
        );
        assert_eq!(
            parse_summary_line("=== 3 passed, 1 failed, 2 skipped in 1.0s ==="),
            SummaryCounts {
                passed: 3,
                failed: 1,
                skipped: 2,
                ..Default::default()
            }
        );
        assert_eq!(
            parse_summary_line("1 passed, 1 error, 2 deselected, 3 xfailed, 4 xpassed in 1.0s"),
            SummaryCounts {
                passed: 1,
                errors: 1,
                deselected: 2,
                xfailed: 3,
                xpassed: 4,
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_filter_pytest_quiet_mode_failures() {
        // In -q mode, the final summary line has NO === wrapper
        // This was causing "No tests collected" to be reported incorrectly
        let output = r#"=== test session starts ===
platform linux -- Python 3.12.11, pytest-8.1.0
collected 1705 items

.......F.......

=== FAILURES ===
___ test_something ___

E   AssertionError: expected True

=== short test summary info ===
FAILED tests/test_foo.py::test_something - AssertionError
5 failed, 1698 passed, 2 skipped in 108.89s"#;

        let result = filter_pytest_output(output);
        assert!(
            !result.contains("No tests collected"),
            "Should not report 'No tests collected' when tests ran. Got: {}",
            result
        );
        assert!(
            result.contains("1698") || result.contains("5 failed"),
            "Should show actual test counts. Got: {}",
            result
        );
    }

    #[test]
    fn test_filter_pytest_only_skipped() {
        // If only skipped tests, should NOT say "No tests collected"
        let output = r#"=== test session starts ===
collected 3 items

=== 3 skipped in 0.10s ==="#;

        let result = filter_pytest_output(output);
        assert!(
            !result.contains("No tests collected"),
            "Should not say 'No tests collected' when tests were skipped. Got: {}",
            result
        );
    }
}
