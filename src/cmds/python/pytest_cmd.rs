//! Filters pytest output to show only failures and the summary line.

use crate::core::runner;
use crate::core::truncate::CAP_WARNINGS;
use crate::core::utils::{resolved_command, tool_exists, truncate};
use anyhow::Result;

const MAX_XFAIL: usize = CAP_WARNINGS;
const MAX_PYTEST_FAILURES: usize = CAP_WARNINGS;

#[derive(Debug, PartialEq)]
enum ParseState {
    Header,
    TestProgress,
    Failures,
    Summary,
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
    // Only treat a short `-r…` as pytest's report flag (not `--randomly-seed` etc.)
    let has_report_flag = args.iter().any(|a| a.starts_with("-r") && !a.starts_with("--"));

    if !has_tb_flag {
        cmd.arg("--tb=short");
    }
    if !has_quiet_flag {
        cmd.arg("-q");
    }
    // Surface xfailed/xpassed (and their reasons) in the short summary section
    // so the compact output can report expected failures and — crucially —
    // unexpected passes (XPASS), which signal a behavior change.
    if !has_report_flag {
        cmd.arg("-rxX");
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
    let mut failures: Vec<String> = Vec::new();
    let mut current_failure: Vec<String> = Vec::new();
    let mut xfail_lines: Vec<String> = Vec::new();
    let mut summary_line = String::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // State transitions
        if trimmed.starts_with("===") && trimmed.contains("test session starts") {
            state = ParseState::Header;
            continue;
        } else if trimmed.starts_with("===")
            && (trimmed.contains("FAILURES") || trimmed.contains("ERRORS"))
        {
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
        } else if trimmed.starts_with("===")
            && (trimmed.contains("passed")
                || trimmed.contains("failed")
                || trimmed.contains("skipped")
                || trimmed.contains("error"))
        {
            summary_line = trimmed.to_string();
            continue;
        // quiet mode (-q): bare summary without === wrapper, e.g. "5 failed, 1698 passed, 2 skipped in 108.89s"
        } else if summary_line.is_empty()
            && !trimmed.starts_with("===")
            && !trimmed.starts_with("FAILED")
            && !trimmed.starts_with("ERROR")
            && (trimmed.contains(" passed")
                || trimmed.contains(" failed")
                || trimmed.contains(" skipped"))
            && trimmed.contains(" in ")
        {
            summary_line = trimmed.to_string();
            continue;
        // quiet mode, errors only: "1 error in 0.06s" (collection/setup errors).
        // Require a leading count so traceback prose can't masquerade as summary.
        } else if summary_line.is_empty()
            && trimmed.contains(" error")
            && trimmed.contains(" in ")
            && trimmed
                .split_whitespace()
                .next()
                .is_some_and(|w| w.parse::<usize>().is_ok())
        {
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
            ParseState::TestProgress => {}
            ParseState::Failures => {
                // Collect failure details
                if trimmed.starts_with("___") {
                    // New failure section
                    if !current_failure.is_empty() {
                        failures.push(current_failure.join("\n"));
                        current_failure.clear();
                    }
                    current_failure.push(trimmed.to_string());
                } else if !trimmed.is_empty()
                    && !trimmed.starts_with("===")
                    && !trimmed.starts_with('!')
                {
                    // '!' lines are Interrupted/stopping banners — the error
                    // count already lands in the summary header.
                    current_failure.push(trimmed.to_string());
                }
            }
            ParseState::Summary => {
                // FAILED test lines
                if trimmed.starts_with("FAILED") || trimmed.starts_with("ERROR") {
                    failures.push(trimmed.to_string());
                } else if trimmed.starts_with("XFAIL") || trimmed.starts_with("XPASS") {
                    xfail_lines.push(trimmed.to_string());
                }
            }
        }
    }

    // Save last failure if any
    if !current_failure.is_empty() {
        failures.push(current_failure.join("\n"));
    }

    let counts = parse_summary_line(&summary_line);

    if counts.is_empty() && failures.is_empty() && xfail_lines.is_empty() {
        // Nothing recognizable parsed. Only claim "No tests collected" when
        // pytest actually said so; otherwise (--version, --help, plugin
        // banners…) pass the output through unchanged.
        let trimmed_out = output.trim();
        if trimmed_out.is_empty()
            || output.contains("no tests ran")
            || output.contains("collected 0 items")
        {
            return "Pytest: No tests collected".to_string();
        }
        return trimmed_out.to_string();
    }

    // Build compact output
    build_pytest_summary(&counts, &failures, &xfail_lines)
}

#[derive(Default)]
struct PytestCounts {
    passed: usize,
    failed: usize,
    skipped: usize,
    xfailed: usize,
    xpassed: usize,
    errors: usize,
}

impl PytestCounts {
    fn is_empty(&self) -> bool {
        self.passed == 0
            && self.failed == 0
            && self.skipped == 0
            && self.xfailed == 0
            && self.xpassed == 0
            && self.errors == 0
    }
}

fn build_pytest_summary(
    counts: &PytestCounts,
    failures: &[String],
    xfail_lines: &[String],
) -> String {
    let PytestCounts {
        passed,
        failed,
        skipped,
        xfailed,
        xpassed,
        errors,
    } = *counts;

    let extras_present = skipped > 0 || xfailed > 0 || xpassed > 0 || !xfail_lines.is_empty();

    if failed == 0 && errors == 0 && passed > 0 && !extras_present && failures.is_empty() {
        return format!("Pytest: {} passed", passed);
    }

    let mut result = String::new();
    result.push_str(&format!("Pytest: {} passed, {} failed", passed, failed));
    if errors > 0 {
        result.push_str(&format!(
            ", {} error{}",
            errors,
            if errors == 1 { "" } else { "s" }
        ));
    }
    if skipped > 0 {
        result.push_str(&format!(", {} skipped", skipped));
    }
    if xfailed > 0 {
        result.push_str(&format!(", {} xfailed", xfailed));
    }
    if xpassed > 0 {
        result.push_str(&format!(", {} xpassed", xpassed));
    }
    result.push('\n');

    // Surface xfail/xpass entries (with their reasons) — XPASS in particular
    // signals that something expected-to-fail now passes.
    if !xfail_lines.is_empty() {
        result.push_str("\nExpected-failure outcomes:\n");
        for line in xfail_lines.iter().take(MAX_XFAIL) {
            result.push_str(&format!("  {}\n", truncate(line, 120)));
        }
        if xfail_lines.len() > MAX_XFAIL {
            result.push_str(&format!("  … +{} more\n", xfail_lines.len() - MAX_XFAIL));
            let all_xfail = xfail_lines.join("\n");
            if let Some(hint) = crate::core::tee::force_tee_tail_hint(&all_xfail, "pytest-xfail", MAX_XFAIL + 1) {
                result.push_str(&format!("  {}\n", hint));
            }
        }
    }

    if failures.is_empty() {
        return result.trim().to_string();
    }

    // Show failures (limit to key information)
    result.push_str(if failed == 0 && errors > 0 {
        "\nErrors:\n"
    } else {
        "\nFailures:\n"
    });

    for (i, failure) in failures.iter().take(MAX_PYTEST_FAILURES).enumerate() {
        // Extract test name and key error info
        let lines: Vec<&str> = failure.lines().collect();

        // First line is usually test name (after ___)
        if let Some(first_line) = lines.first() {
            if first_line.starts_with("___") {
                // Extract test name between ___
                let test_name = first_line.trim_matches('_').trim();
                let (tag, test_name) = match test_name.strip_prefix("ERROR ") {
                    Some(rest) => ("[ERR]", rest),
                    None => ("[FAIL]", test_name),
                };
                result.push_str(&format!("{}. {} {}\n", i + 1, tag, test_name));
            } else if first_line.starts_with("FAILED") || first_line.starts_with("ERROR") {
                // Summary format: "FAILED tests/test_foo.py::test_bar - AssertionError"
                let parts: Vec<&str> = first_line.split(" - ").collect();
                if let Some(test_path) = parts.first() {
                    let (tag, test_name) = match test_path.strip_prefix("ERROR ") {
                        Some(rest) => ("[ERR]", rest),
                        None => ("[FAIL]", test_path.trim_start_matches("FAILED ")),
                    };
                    result.push_str(&format!("{}. {} {}\n", i + 1, tag, test_name));
                }
                if parts.len() > 1 {
                    result.push_str(&format!("     {}\n", truncate(parts[1], 100)));
                }
                continue;
            }
        }

        // Show relevant error lines. Assertion/exception lines ('>' and 'E'
        // markers) are the root cause — select them first so traceback
        // location boilerplate can't crowd them out of the 3-line cap,
        // then render in original order.
        let body = &lines[1..];
        let is_strong = |line: &str| {
            let t = line.trim();
            t.starts_with('>') || t.starts_with('E')
        };
        let is_relevant = |line: &str| {
            let line_lower = line.to_lowercase();
            is_strong(line)
                || line_lower.contains("assert")
                || line_lower.contains("error")
                || line.contains(".py:")
        };

        let mut selected: Vec<usize> = body
            .iter()
            .enumerate()
            .filter(|(_, line)| is_strong(line))
            .map(|(idx, _)| idx)
            .take(3)
            .collect();
        for (idx, line) in body.iter().enumerate() {
            if selected.len() >= 3 {
                break;
            }
            if is_relevant(line) && !selected.contains(&idx) {
                selected.push(idx);
            }
        }
        selected.sort_unstable();

        for idx in selected {
            result.push_str(&format!("     {}\n", truncate(body[idx], 100)));
        }

        if i < failures.len() - 1 {
            result.push('\n');
        }
    }

    if failures.len() > MAX_PYTEST_FAILURES {
        result.push_str(&format!(
            "\n… +{} more failures\n",
            failures.len() - MAX_PYTEST_FAILURES
        ));
        let all_failures = failures.join("\n\n");
        if let Some(hint) = crate::core::tee::force_tee_hint(&all_failures, "pytest-failures") {
            result.push_str(&format!("  {}\n", hint));
        }
    }

    result.trim().to_string()
}

fn parse_summary_line(summary: &str) -> PytestCounts {
    let mut counts = PytestCounts::default();

    // Parse lines like "=== 4 passed, 1 failed, 2 xfailed, 1 xpassed in 0.50s ==="
    for part in summary.split(',') {
        let words: Vec<&str> = part.split_whitespace().collect();
        for (i, word) in words.iter().enumerate() {
            if i == 0 {
                continue;
            }
            let Ok(n) = words[i - 1].parse::<usize>() else {
                continue;
            };
            // Order matters: "xpassed"/"xfailed" contain "passed"/"failed".
            if word.contains("xpassed") {
                counts.xpassed = n;
            } else if word.contains("xfailed") {
                counts.xfailed = n;
            } else if word.contains("passed") {
                counts.passed = n;
            } else if word.contains("failed") {
                counts.failed = n;
            } else if word.contains("skipped") {
                counts.skipped = n;
            } else if word.contains("error") {
                counts.errors = n;
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
    fn test_parse_summary_line() {
        let c = parse_summary_line("=== 5 passed in 0.50s ===");
        assert_eq!((c.passed, c.failed, c.skipped), (5, 0, 0));

        let c = parse_summary_line("=== 4 passed, 1 failed in 0.50s ===");
        assert_eq!((c.passed, c.failed, c.skipped), (4, 1, 0));

        let c = parse_summary_line("=== 3 passed, 1 failed, 2 skipped in 1.0s ===");
        assert_eq!((c.passed, c.failed, c.skipped), (3, 1, 2));

        let c = parse_summary_line("=== 2 passed, 1 failed, 2 xfailed, 1 xpassed in 1.0s ===");
        assert_eq!(
            (c.passed, c.failed, c.xfailed, c.xpassed),
            (2, 1, 2, 1)
        );
    }

    #[test]
    fn test_filter_pytest_xfail_caps_and_tee_hint() {
        let mut lines = String::from("=== test session starts ===\ncollected 30 items\n\n");
        lines.push_str("test_x.py ");
        for _ in 0..15 {
            lines.push('x');
        }
        lines.push_str("\n\n=== short test summary info ===\n");
        for i in 0..15 {
            lines.push_str(&format!(
                "XFAIL test_x.py::test_case_{i} - known issue #{i}\n"
            ));
        }
        lines.push_str("=== 0 passed, 15 xfailed in 0.05s ===\n");

        let result = filter_pytest_output(&lines);
        let xfail_in_section = result
            .split("Expected-failure outcomes:")
            .nth(1)
            .unwrap_or("");
        let listed = xfail_in_section
            .lines()
            .filter(|l| l.trim().starts_with("XFAIL"))
            .count();
        assert!(
            listed <= 10,
            "MAX_XFAIL cap not enforced: listed {listed}"
        );
        assert!(result.contains("… +5 more"), "missing '+N more': {result}");
    }

    #[test]
    fn test_filter_pytest_xfail_xpass() {
        let output = r#"=== test session starts ===
collected 5 items

test_math.py ..xxX                                                 [100%]

=== short test summary info ===
XFAIL test_math.py::test_division_by_zero - known bug in division
XFAIL test_math.py::test_float_precision - float precision issue — bug #42
XPASS test_math.py::test_unexpected_pass - this should fail but currently passes
=== 2 passed, 2 xfailed, 1 xpassed in 0.05s ==="#;

        let result = filter_pytest_output(output);
        assert!(result.contains("xfailed"), "got: {result}");
        assert!(result.contains("xpassed"), "got: {result}");
        assert!(result.contains("XPASS"), "got: {result}");
        assert!(result.contains("float precision"), "got: {result}");
        assert!(result.contains("test_division_by_zero"), "got: {result}");
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
    fn test_filter_pytest_collection_error_quiet() {
        // Real `pytest --tb=short -q -rxX` output (pytest 9) for an import
        // error during collection: no session banner, no short summary,
        // just an ERRORS section and a bare "1 error in 0.06s" line.
        let output = include_str!("../../../tests/fixtures/pytest_collection_error_q.txt");

        let result = filter_pytest_output(output);
        assert!(
            !result.contains("No tests collected"),
            "Collection errors must not be reported as 'No tests collected'. Got: {}",
            result
        );
        assert!(result.contains("1 error"), "got: {}", result);
        assert!(
            result.contains("collecting tests/test_broken.py"),
            "got: {}",
            result
        );
        assert!(
            result.contains("ModuleNotFoundError"),
            "The root-cause E line must survive filtering. Got: {}",
            result
        );
    }

    #[test]
    fn test_filter_pytest_collection_error_banner_mode() {
        // Non-quiet variant: banner + short test summary + === error summary ===
        let output = r#"=== test session starts ===
collected 0 items / 1 error

=== ERRORS ===
___ ERROR collecting tests/test_broken.py ___
ImportError while importing test module 'tests/test_broken.py'.
E   ModuleNotFoundError: No module named 'nonexistent_module'
=== short test summary info ===
ERROR tests/test_broken.py
!!! Interrupted: 1 error during collection !!!
=== 1 error in 0.12s ==="#;

        let result = filter_pytest_output(output);
        assert!(!result.contains("No tests collected"), "got: {}", result);
        assert!(result.contains("1 error"), "got: {}", result);
        assert!(result.contains("ModuleNotFoundError"), "got: {}", result);
    }

    #[test]
    fn test_filter_pytest_version_passthrough() {
        // `pytest --version` produces no test session at all — pass through
        // instead of claiming "No tests collected".
        let result = filter_pytest_output("pytest 9.0.3\n");
        assert_eq!(result, "pytest 9.0.3");
    }

    #[test]
    fn test_parse_summary_line_errors() {
        let c = parse_summary_line("=== 2 passed, 1 error in 0.5s ===");
        assert_eq!((c.passed, c.errors), (2, 1));

        let c = parse_summary_line("1 error in 0.06s");
        assert_eq!(c.errors, 1);

        let c = parse_summary_line("=== 3 errors in 1.2s ===");
        assert_eq!(c.errors, 3);
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
