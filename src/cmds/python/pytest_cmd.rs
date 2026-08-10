//! Filters pytest output to show only failures and the summary line.

use crate::core::runner;
use crate::core::truncate::CAP_WARNINGS;
use crate::core::utils::{resolved_command, strip_ansi, tool_exists, truncate};
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

    runner::run_filtered_with_facts(
        cmd,
        "pytest",
        &args.join(" "),
        |raw, exit_code, capture| {
            let clean = strip_ansi(raw);
            filter_pytest_output_with_exit(&clean, exit_code, !capture.complete())
        },
        runner::RunOptions::with_tee("pytest").force_tee_on_unparseable(),
    )
}

const PYTEST_NO_TESTS: &str = "Pytest: No tests collected";
const PYTEST_EXIT_NO_TESTS: i32 = 5;

pub(crate) fn filter_pytest_output(output: &str) -> String {
    validate_and_render_pytest_output(output, None, false)
}

fn filter_pytest_output_with_exit(
    output: &str,
    exit_code: i32,
    capture_truncated: bool,
) -> String {
    validate_and_render_pytest_output(output, Some(exit_code), capture_truncated)
}

fn validate_and_render_pytest_output(
    output: &str,
    exit_code: Option<i32>,
    capture_truncated: bool,
) -> String {
    let summaries = terminal_summary_lines(output);
    let (collected, collected_conflict) = parse_collected(output);
    let exit_display = exit_code
        .map(|code| code.to_string())
        .unwrap_or_else(|| "unavailable".to_string());

    let explicit_no_tests = summaries.len() == 1
        && summaries[0].to_ascii_lowercase().contains("no tests ran")
        && collected.unwrap_or(0) == 0
        && !collected_conflict;
    if explicit_no_tests && exit_code.is_none_or(|code| code == PYTEST_EXIT_NO_TESTS) {
        return PYTEST_NO_TESTS.to_string();
    }

    let mut reason = None;
    if capture_truncated {
        reason = Some("stdout/stderr capture truncated");
    } else if collected_conflict {
        reason = Some("conflicting collected counts");
    } else if summaries.len() != 1 {
        reason = Some(if summaries.is_empty() {
            "terminal summary missing"
        } else {
            "multiple terminal summaries"
        });
    } else if matches!(exit_code, Some(2 | 3 | 4)) {
        reason = Some("pytest interrupted or usage/internal error");
    } else if exit_code == Some(PYTEST_EXIT_NO_TESTS) {
        reason = Some("exit code 5 without explicit zero collection");
    }

    let counts = summaries
        .first()
        .map(|line| parse_summary_line(line))
        .unwrap_or_default();
    let failure_outcomes = counts.failed + counts.errors + counts.subtests_failed;
    let success_outcomes = counts.passed
        + counts.skipped
        + counts.xfailed
        + counts.xpassed
        + counts.subtests_passed;

    if reason.is_none() {
        match exit_code {
            Some(0) if failure_outcomes > 0 || success_outcomes == 0 => {
                reason = Some("exit code conflicts with terminal summary")
            }
            Some(1) if failure_outcomes == 0 => {
                reason = Some("exit code conflicts with terminal summary")
            }
            Some(code) if code != 0 && code != 1 => {
                reason = Some("unsupported pytest exit code for parsed summary")
            }
            None if failure_outcomes + success_outcomes == 0 => {
                reason = Some("terminal summary has no recognized outcomes")
            }
            _ => {}
        }
    }

    if let Some(reason) = reason {
        return unparseable_pytest_result(output, &exit_display, collected, reason);
    }

    let rendered = render_pytest_output(output);
    match collected {
        Some(value) => format!("{rendered}\ncollected: {value}"),
        None => rendered,
    }
}

fn terminal_summary_lines(output: &str) -> Vec<&str> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            let outcome = lower.contains(" passed")
                || lower.contains(" failed")
                || lower.contains(" skipped")
                || lower.contains(" error")
                || lower.contains(" xfailed")
                || lower.contains(" xpassed")
                || lower.contains(" subtest")
                || lower.contains("no tests ran");
            let terminal_shape = lower.contains(" in ")
                && (!line.starts_with("===") || line.ends_with("==="));
            outcome && terminal_shape && !line.starts_with("FAILED") && !line.starts_with("ERROR")
        })
        .collect()
}

fn parse_collected(output: &str) -> (Option<usize>, bool) {
    let mut values = Vec::new();
    for line in output.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix("collected ") else {
            continue;
        };
        let Some(word) = rest.split_whitespace().next() else {
            continue;
        };
        if let Ok(value) = word.parse::<usize>() {
            values.push(value);
        }
    }
    let first = values.first().copied();
    let conflict = first.is_some_and(|value| values.iter().any(|other| *other != value));
    (first, conflict)
}

fn unparseable_pytest_result(
    output: &str,
    exit_display: &str,
    collected: Option<usize>,
    reason: &str,
) -> String {
    let mut result = format!(
        "Pytest: result could not be parsed (exit code {exit_display})\nreason: {reason}"
    );
    if let Some(value) = collected {
        result.push_str(&format!("\ncollected: {value}"));
    }
    let locations = failure_locations(output);
    if !locations.is_empty() {
        result.push_str("\nFailure positions:\n");
        for location in locations.iter().take(MAX_PYTEST_FAILURES) {
            result.push_str("  ");
            result.push_str(location);
            result.push('\n');
        }
    }
    result.trim_end().to_string()
}

fn failure_locations(output: &str) -> Vec<String> {
    let mut locations = Vec::new();
    for line in output.lines().map(str::trim) {
        let looks_like_location = line.starts_with("FAILED ")
            || line.starts_with("ERROR ")
            || line.split_whitespace().any(|word| {
                let Some((path, number)) = word.rsplit_once(':') else {
                    return false;
                };
                path.ends_with(".py") && number.trim_end_matches(':').parse::<usize>().is_ok()
            });
        if looks_like_location && !locations.iter().any(|item| item == line) {
            locations.push(truncate(line, 160));
        }
    }
    locations
}

fn render_pytest_output(output: &str) -> String {
    let mut state = ParseState::Header;
    let mut test_files: Vec<String> = Vec::new();
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
        } else if trimmed.starts_with("===")
            && (trimmed.contains("passed")
                || trimmed.contains("failed")
                || trimmed.contains("skipped")
                || trimmed.contains("error")
                || trimmed.contains("subtest"))
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
                || trimmed.contains(" skipped")
                || trimmed.contains(" error")
                || trimmed.contains(" subtest"))
            && trimmed.contains(" in ")
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

    // Build compact output
    build_pytest_summary(&summary_line, &test_files, &failures, &xfail_lines)
}

#[derive(Default)]
struct PytestCounts {
    passed: usize,
    failed: usize,
    skipped: usize,
    xfailed: usize,
    xpassed: usize,
    errors: usize,
    subtests_passed: usize,
    subtests_failed: usize,
}

fn build_pytest_summary(
    summary: &str,
    _test_files: &[String],
    failures: &[String],
    xfail_lines: &[String],
) -> String {
    let counts = parse_summary_line(summary);
    let PytestCounts {
        passed,
        failed,
        skipped,
        xfailed,
        xpassed,
        errors,
        subtests_passed,
        subtests_failed,
    } = counts;

    if passed == 0
        && failed == 0
        && skipped == 0
        && xfailed == 0
        && xpassed == 0
        && errors == 0
        && subtests_passed == 0
        && subtests_failed == 0
    {
        return PYTEST_NO_TESTS.to_string();
    }

    let extras_present = skipped > 0
        || xfailed > 0
        || xpassed > 0
        || errors > 0
        || subtests_passed > 0
        || subtests_failed > 0
        || !xfail_lines.is_empty();

    if failed == 0 && passed > 0 && !extras_present {
        return format!("Pytest: {} passed", passed);
    }

    let mut result = String::new();
    result.push_str(&format!("Pytest: {} passed, {} failed", passed, failed));
    if skipped > 0 {
        result.push_str(&format!(", {} skipped", skipped));
    }
    if xfailed > 0 {
        result.push_str(&format!(", {} xfailed", xfailed));
    }
    if xpassed > 0 {
        result.push_str(&format!(", {} xpassed", xpassed));
    }
    if errors > 0 {
        result.push_str(&format!(", {} error{}", errors, if errors == 1 { "" } else { "s" }));
    }
    if subtests_passed > 0 {
        result.push_str(&format!(", {} subtests passed", subtests_passed));
    }
    if subtests_failed > 0 {
        result.push_str(&format!(", {} subtest{} failed", subtests_failed, if subtests_failed == 1 { "" } else { "s" }));
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
    result.push_str("\nFailures:\n");

    for (i, failure) in failures.iter().take(MAX_PYTEST_FAILURES).enumerate() {
        // Extract test name and key error info
        let lines: Vec<&str> = failure.lines().collect();

        // First line is usually test name (after ___)
        if let Some(first_line) = lines.first() {
            if first_line.starts_with("___") {
                // Extract test name between ___
                let test_name = first_line.trim_matches('_').trim();
                result.push_str(&format!("{}. [FAIL] {}\n", i + 1, test_name));
            } else if first_line.starts_with("FAILED") {
                // Summary format: "FAILED tests/test_foo.py::test_bar - AssertionError"
                let parts: Vec<&str> = first_line.split(" - ").collect();
                if let Some(test_path) = parts.first() {
                    let test_name = test_path.trim_start_matches("FAILED ");
                    result.push_str(&format!("{}. [FAIL] {}\n", i + 1, test_name));
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
        let normalized = part
            .trim_matches('=')
            .trim()
            .to_ascii_lowercase();
        let Some(n) = normalized
            .split_whitespace()
            .find_map(|word| word.parse::<usize>().ok())
        else {
            continue;
        };
        if normalized.contains("subtest") && normalized.contains("failed") {
            counts.subtests_failed = n;
        } else if normalized.contains("subtest") && normalized.contains("passed") {
            counts.subtests_passed = n;
        } else if normalized.contains("xpassed") {
            counts.xpassed = n;
        } else if normalized.contains("xfailed") {
            counts.xfailed = n;
        } else if normalized.contains("passed") {
            counts.passed = n;
        } else if normalized.contains("failed") {
            counts.failed = n;
        } else if normalized.contains("skipped") {
            counts.skipped = n;
        } else if normalized.contains("error") {
            counts.errors = n;
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

    #[test]
    fn test_truth_matrix_only_exit_five_with_explicit_zero_collection_is_no_tests() {
        let no_tests = "=== test session starts ===\ncollected 0 items\n\n=== no tests ran in 0.00s ===\n";
        assert_eq!(
            filter_pytest_output_with_exit(no_tests, PYTEST_EXIT_NO_TESTS, false),
            PYTEST_NO_TESTS
        );
        assert_eq!(
            filter_pytest_output_with_exit(
                "no tests ran in 0.00s\n",
                PYTEST_EXIT_NO_TESTS,
                false,
            ),
            PYTEST_NO_TESTS
        );

        let positive_collection =
            "=== test session starts ===\ncollected 3 items\n\n=== no tests ran in 0.00s ===\n";
        let result = filter_pytest_output_with_exit(
            positive_collection,
            PYTEST_EXIT_NO_TESTS,
            false,
        );
        assert!(result.contains("result could not be parsed (exit code 5)"));
        assert!(result.contains("collected: 3"));
    }

    #[test]
    fn test_truth_matrix_rejects_exit_and_summary_conflicts() {
        let success = "collected 3 items\n=== 3 passed in 0.10s ===\n";
        let result = filter_pytest_output_with_exit(success, 1, false);
        assert!(result.contains("result could not be parsed (exit code 1)"));
        assert!(!result.contains("Pytest: 3 passed"));

        let failure = "collected 1 item\nFAILED tests/test_x.py::test_x - AssertionError\n=== 1 failed in 0.10s ===\n";
        let result = filter_pytest_output_with_exit(failure, 0, false);
        assert!(result.contains("result could not be parsed (exit code 0)"));
        assert!(result.contains("tests/test_x.py::test_x"));
    }

    #[test]
    fn test_truth_matrix_rejects_missing_multiple_and_truncated_summaries() {
        let missing = "=== test session starts ===\ncollected 2 items\ntests/test_x.py ..\n";
        assert!(filter_pytest_output_with_exit(missing, 0, false)
            .contains("result could not be parsed (exit code 0)"));

        let multiple = "collected 2 items\n=== 1 passed in 0.01s ===\n=== 2 passed in 0.02s ===\n";
        assert!(filter_pytest_output_with_exit(multiple, 0, false)
            .contains("result could not be parsed (exit code 0)"));

        let truncated = "collected 2 items\n=== 2 passed in 0.02s ===\n";
        let result = filter_pytest_output_with_exit(truncated, 0, true);
        assert!(result.contains("result could not be parsed (exit code 0)"));
        assert!(result.contains("capture truncated"));
    }

    #[test]
    fn test_truth_matrix_preserves_collected_errors_subtests_and_failure_locations() {
        let output = r#"=== test session starts ===
collected 7 items

=== FAILURES ===
___ test_parent ___
tests/test_parent.py:42: AssertionError

=== short test summary info ===
FAILED tests/test_parent.py::test_parent - AssertionError
=== 4 passed, 1 failed, 1 error, 3 subtests passed, 1 subtest failed in 0.20s ==="#;
        let result = filter_pytest_output_with_exit(output, 1, false);
        assert!(result.contains("collected: 7"));
        assert!(result.contains("4 passed, 1 failed"));
        assert!(result.contains("1 error"));
        assert!(result.contains("3 subtests passed"));
        assert!(result.contains("1 subtest failed"));
        assert!(result.contains("tests/test_parent.py:42"));
    }

    #[test]
    fn test_truth_matrix_exit_two_three_four_are_always_unparseable() {
        let looks_successful = "collected 1 item\n=== 1 passed in 0.01s ===\n";
        for exit_code in [2, 3, 4] {
            let result = filter_pytest_output_with_exit(looks_successful, exit_code, false);
            assert!(result.contains(&format!(
                "result could not be parsed (exit code {exit_code})"
            )));
        }
    }
}
