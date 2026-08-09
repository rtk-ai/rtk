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

    runner::run_filtered_with_capture(
        cmd,
        "pytest",
        &args.join(" "),
        filter_pytest_output_with_capture,
        runner::RunOptions::stdout_only()
            .guard_combined_output()
            .force_tee_when_output_contains("result could not be parsed")
            .tee("pytest"),
    )
}

/// Compatibility entry point for `rtk <input> | rtk pytest`.
///
/// The direct command uses the exit-aware variant below. Piped input has no
/// child exit status, so it can only claim an explicit summary, never infer
/// "no tests" from a missing summary.
pub(crate) fn filter_pytest_output(output: &str) -> String {
    filter_pytest_output_inner(output, None, false)
}

#[cfg(test)]
fn filter_pytest_output_with_exit(output: &str, exit_code: i32) -> String {
    filter_pytest_output_inner(output, Some(exit_code), false)
}

fn filter_pytest_output_with_capture(output: &str, exit_code: i32, stdout_truncated: bool) -> String {
    filter_pytest_output_inner(output, Some(exit_code), stdout_truncated)
}

fn filter_pytest_output_inner(output: &str, exit_code: Option<i32>, stdout_truncated: bool) -> String {
    let mut state = ParseState::Header;
    let mut failures: Vec<String> = Vec::new();
    let mut current_failure: Vec<String> = Vec::new();
    let mut xfail_lines: Vec<String> = Vec::new();
    let mut summary_line: Option<String> = None;
    let mut collected: Option<usize> = None;
    let mut explicit_no_tests = false;
    let mut summary_signal_count = 0usize;
    let lines: Vec<String> = output.lines().map(strip_ansi).collect();
    let last_nonempty = lines.iter().rposition(|line| !line.trim().is_empty());

    for (line_index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        // State transitions
        if trimmed.starts_with("===") && trimmed.contains("test session starts") {
            state = ParseState::Header;
            continue;
        } else if state == ParseState::Header {
            // Only accept pytest's collection line while still in the session
            // header. A test's own output can contain "collected N", but it
            // cannot rewrite the collection count once progress has begun.
            if let Some(count) = parse_collected_count(trimmed) {
                collected = Some(count);
                state = ParseState::TestProgress;
                continue;
            }
        }

        if is_explicit_no_tests_line(trimmed) {
            summary_signal_count += 1;
            // Pytest's terminal summary is the final non-empty output line.
            // Earlier lookalikes may be arbitrary test output and are ignored.
            if Some(line_index) == last_nonempty && collected.is_none_or(|count| count == 0) {
                explicit_no_tests = true;
            }
            // Do not let a no-tests phrase from test output become a normal
            // zero-count result candidate.
            continue;
        }

        if trimmed.starts_with("===")
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
        } else if is_summary_candidate(trimmed) {
            summary_signal_count += 1;
            // Only pytest's terminal line can be its final result summary.
            // A test may print an identical-looking line while it is running.
            if Some(line_index) == last_nonempty {
                summary_line = Some(trimmed.to_string());
            }
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

    if summary_signal_count != 1 {
        summary_line = None;
        explicit_no_tests = false;
    }

    let report = PytestReport {
        collected,
        summary_line,
        explicit_no_tests,
    };

    build_pytest_summary(
        &report,
        &failures,
        &xfail_lines,
        exit_code,
        stdout_truncated,
    )
}

fn strip_ansi(line: &str) -> String {
    let mut clean = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // Skip CSI escape sequences such as "\x1b[32m" and
            // "\x1b[0m" before applying textual matching.
            for sequence_char in chars.by_ref() {
                if sequence_char.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            clean.push(ch);
        }
    }
    clean
}

fn normalized_tokens(line: &str) -> Vec<&str> {
    line.split_whitespace()
        .map(|token| token.trim_matches(|ch: char| "=,;()".contains(ch)))
        .filter(|token| !token.is_empty())
        .collect()
}

fn parse_collected_count(line: &str) -> Option<usize> {
    let tokens = normalized_tokens(line);
    tokens.windows(2).find_map(|window| {
        if window[0].eq_ignore_ascii_case("collected") {
            window[1].parse::<usize>().ok()
        } else {
            None
        }
    })
}

fn is_explicit_no_tests_line(line: &str) -> bool {
    if !line.starts_with('=') || !line.ends_with('=') {
        return false;
    }
    let content = line.trim_matches('=').trim().to_ascii_lowercase();
    let tokens: Vec<&str> = content.split_whitespace().collect();
    let phrase_len = if tokens.starts_with(&["no", "tests", "ran"])
        || tokens.starts_with(&["no", "tests", "collected"])
    {
        3
    } else {
        return false;
    };
    tokens.len() == phrase_len
        || (tokens.len() == phrase_len + 2
            && tokens[phrase_len] == "in"
            && tokens[phrase_len + 1]
                .trim_end_matches('s')
                .parse::<f64>()
                .is_ok())
}

fn is_result_label(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "passed" | "failed" | "skipped" | "xfailed" | "xpassed" | "subtests"
            | "error" | "errors"
    )
}

fn is_summary_candidate(line: &str) -> bool {
    if is_explicit_no_tests_line(line) {
        return true;
    }

    let tokens = normalized_tokens(line);
    if tokens
        .first()
        .is_none_or(|token| token.parse::<usize>().is_err())
    {
        return false;
    }
    let has_result_count = tokens
        .windows(2)
        .any(|window| window[0].parse::<usize>().is_ok() && is_result_label(window[1]));
    if !has_result_count || line.starts_with("FAILED") || line.starts_with("ERROR") {
        return false;
    }

    // Pytest result summaries carry a duration. Requiring it makes a test's
    // ordinary "N passed" log line insufficient evidence, while still
    // accepting canonical, quiet-mode, ANSI-cleaned, and non-standard
    // summaries.
    let tokens = normalized_tokens(line);
    tokens.windows(2).any(|window| {
        window[0].eq_ignore_ascii_case("in")
            && window[1]
                .trim_end_matches('s')
                .parse::<f64>()
                .is_ok()
    })
}

#[derive(Default)]
struct PytestCounts {
    passed: usize,
    failed: usize,
    skipped: usize,
    xfailed: usize,
    xpassed: usize,
    errors: usize,
    subtests: usize,
    subtest_passed: usize,
    subtest_failed: usize,
}

struct PytestReport {
    collected: Option<usize>,
    summary_line: Option<String>,
    explicit_no_tests: bool,
}

fn build_pytest_summary(
    report: &PytestReport,
    failures: &[String],
    xfail_lines: &[String],
    exit_code: Option<i32>,
    stdout_truncated: bool,
) -> String {
    // Direct pytest reports no collection with exit 5. Piped compatibility
    // input has no child status, so only an exact terminal summary can support
    // the claim there.
    if (exit_code == Some(5) || (exit_code.is_none() && report.explicit_no_tests))
        && report.collected.is_none_or(|count| count == 0)
    {
        return if exit_code.is_none() {
            "Pytest: No tests collected".to_string()
        } else {
            format!(
                "Pytest: No tests collected (exit code {})",
                exit_code.unwrap_or_default()
            )
        };
    }

    if stdout_truncated {
        return build_unparsed_result(
            report,
            failures,
            exit_code,
            true,
        );
    }

    let Some(summary) = report.summary_line.as_deref() else {
        return build_unparsed_result(
            report,
            failures,
            exit_code,
            false,
        );
    };

    let counts = parse_summary_line(summary);
    let PytestCounts {
        passed,
        failed,
        skipped,
        xfailed,
        xpassed,
        errors,
        subtests,
        subtest_passed,
        subtest_failed,
    } = counts;

    let exit_conflicts_with_summary = match exit_code {
        Some(0) => failed > 0 || errors > 0 || subtest_failed > 0,
        Some(1) => failed == 0 && errors == 0 && subtest_failed == 0,
        Some(_) => true,
        None => false,
    };
    if exit_conflicts_with_summary {
        return build_unparsed_result(
            report,
            failures,
            exit_code,
            false,
        );
    }

    let extras_present = skipped > 0
        || xfailed > 0
        || xpassed > 0
        || errors > 0
        || subtests > 0
        || !xfail_lines.is_empty();

    let collection_suffix = report
        .collected
        .map(|count| format!(" (collected {})", count))
        .unwrap_or_default();

    if failed == 0 && passed > 0 && !extras_present {
        return format!("Pytest: {} passed{}", passed, collection_suffix);
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
        result.push_str(&format!(", {} errors", errors));
    }
    if subtests > 0 {
        if subtest_passed > 0 {
            result.push_str(&format!(", {} subtests passed", subtest_passed));
        }
        if subtest_failed > 0 {
            result.push_str(&format!(", {} subtests failed", subtest_failed));
        }
        if subtest_passed == 0 && subtest_failed == 0 {
            result.push_str(&format!(", {} subtests", subtests));
        }
    }
    result.push_str(&collection_suffix);
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

fn build_unparsed_result(
    report: &PytestReport,
    failures: &[String],
    exit_code: Option<i32>,
    stdout_truncated: bool,
) -> String {
    let exit = exit_code
        .map(|code| code.to_string())
        .unwrap_or_else(|| "unavailable".to_string());
    let mut result = format!("Pytest: result could not be parsed (exit code {})", exit);
    if stdout_truncated {
        result.push_str(", stdout capture truncated");
    }
    if let Some(collected) = report.collected {
        result.push_str(&format!(", collected {}", collected));
    }
    if !failures.is_empty() {
        result.push_str("\nFailure positions:\n");
        for failure in failures.iter().take(MAX_PYTEST_FAILURES) {
            if let Some(position) = failure.lines().find(|line| {
                line.starts_with("FAILED")
                    || line.starts_with("ERROR")
                    || line.contains(".py:")
            }) {
                result.push_str(&format!("  {}\n", truncate(position, 160)));
            }
        }
    }
    result.trim().to_string()
}

fn parse_summary_line(summary: &str) -> PytestCounts {
    let mut counts = PytestCounts::default();

    // Parse canonical, quiet-mode, ANSI-decorated, and subtest summaries such
    // as "=== 4 passed, 3 subtests passed in 0.50s ===".
    let normalized = strip_ansi(summary);
    let tokens = normalized_tokens(&normalized);
    for (index, token) in tokens.iter().enumerate() {
        let Ok(count) = token.parse::<usize>() else {
            continue;
        };
        let Some(label) = tokens.get(index + 1) else {
            continue;
        };
        match label.to_ascii_lowercase().as_str() {
            "xpassed" => counts.xpassed = count,
            "xfailed" => counts.xfailed = count,
            "passed" => {
                if index == 0 || tokens[index - 1] != "subtests" {
                    counts.passed = count;
                }
            }
            "failed" => {
                if index == 0 || tokens[index - 1] != "subtests" {
                    counts.failed = count;
                }
            }
            "skipped" => counts.skipped = count,
            "error" | "errors" => counts.errors = count,
            "subtests" => {
                counts.subtests += count;
                match tokens.get(index + 2).map(|outcome| outcome.to_ascii_lowercase()) {
                    Some(outcome) if outcome == "passed" => counts.subtest_passed += count,
                    Some(outcome) if outcome == "failed" => counts.subtest_failed += count,
                    _ => {}
                }
            }
            _ => {}
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
        assert!(result.contains("collected 5"));
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
    fn test_filter_pytest_log_text_cannot_override_real_result() {
        let output = r#"=== test session starts ===
collected 2 items

tests/test_output.py ..                                             [100%]
test output: === no tests ran in 0.00s ===
test output: 1 passed in 0.01s
collected 99 items

=== short test summary info ===
=== 2 passed in 0.10s ==="#;

        let result = filter_pytest_output_with_exit(output, 0);
        assert!(result.contains("2 passed"), "unexpected result: {result}");
        assert!(result.contains("collected 2"), "unexpected result: {result}");
        assert!(!result.contains("No tests collected"));
        assert!(!result.contains("collected 99"));
    }

    #[test]
    fn test_filter_pytest_exact_fake_summary_degrades_instead_of_lying() {
        let output = r#"=== test session starts ===
collected 1 item

=== 1 failed in 0.01s ===
tests/test_output.py .                                             [100%]
=== 1 passed in 0.10s ==="#;

        let result = filter_pytest_output_with_exit(output, 0);
        assert!(result.contains("could not be parsed"), "unexpected result: {result}");
        assert!(result.contains("exit code 0"));
        assert!(!result.contains("No tests collected"));
    }

    #[test]
    fn test_filter_pytest_fake_no_tests_banner_degrades() {
        let output = r#"=== test session starts ===
=== no tests ran in 0.01s ===
tests/test_output.py .                                             [100%]
=== 1 passed in 0.10s ==="#;

        let result = filter_pytest_output_with_exit(output, 0);
        assert!(result.contains("could not be parsed"), "unexpected result: {result}");
        assert!(!result.contains("No tests collected"));
    }

    #[test]
    fn test_filter_pytest_no_tests_banner_conflicting_with_exit_degrades() {
        let output = "=== test session starts ===\ncollected 0 items\n=== no tests ran in 0.01s ===";
        let result = filter_pytest_output_with_exit(output, 0);
        assert!(result.contains("could not be parsed"), "unexpected result: {result}");
        assert!(result.contains("exit code 0"));
        assert!(!result.contains("No tests collected"));
    }

    #[test]
    fn test_filter_pytest_non_result_exit_codes_always_degrade() {
        let output = "=== test session starts ===\ncollected 1 item\n=== 1 passed in 0.01s ===";
        for exit_code in [2, 3, 4] {
            let result = filter_pytest_output_with_exit(output, exit_code);
            assert!(result.contains("could not be parsed"), "unexpected result: {result}");
            assert!(
                result.contains(&format!("exit code {exit_code}")),
                "unexpected result: {result}"
            );
            assert!(!result.contains("Pytest: 1 passed"));
        }
    }

    #[test]
    fn test_filter_pytest_capture_truncation_rejects_fake_terminal_summary() {
        let output = "=== test session starts ===\ncollected 1 item\n=== 1 passed in 0.01s ===";
        let result = filter_pytest_output_inner(output, Some(0), true);
        assert!(result.contains("could not be parsed"), "unexpected result: {result}");
        assert!(result.contains("stdout capture truncated"));
        assert!(!result.contains("Pytest: 1 passed"));
    }

    #[test]
    fn test_filter_pytest_summary_conflict_keeps_failure_position() {
        let output = r#"=== test session starts ===
collected 1 item

=== FAILURES ===
___ test_truth ___
tests/test_truth.py:7: AssertionError

=== short test summary info ===
FAILED tests/test_truth.py::test_truth - AssertionError
=== 1 passed in 0.01s ==="#;
        let result = filter_pytest_output_with_exit(output, 1);
        assert!(result.contains("could not be parsed"), "unexpected result: {result}");
        assert!(result.contains("tests/test_truth.py:7"), "unexpected result: {result}");
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
        assert_eq!((c.subtests, c.subtest_passed, c.subtest_failed), (0, 0, 0));

        let c = parse_summary_line("=== 4 passed, 3 subtests passed in 0.5s ===");
        assert_eq!((c.passed, c.subtests, c.subtest_passed), (4, 3, 3));

        let c = parse_summary_line(
            "=== 4 passed, 3 subtests passed, 1 subtests failed in 0.5s ===",
        );
        assert_eq!((c.subtests, c.subtest_passed, c.subtest_failed), (4, 3, 1));

        let c = parse_summary_line("=== 1 error in 0.2s ===");
        assert_eq!(c.errors, 1);
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
    fn test_filter_pytest_truncated_before_summary_is_not_no_tests() {
        // A long-running pytest process can have collected tests and emitted
        // progress before its final summary is unavailable to the parser.
        let output = r#"=== test session starts ===
platform darwin -- Python 3.12.0
collected 4 items

tests/test_slow.py ....                                            [100%]
"#;

        let result = filter_pytest_output_with_exit(output, 1);
        assert!(
            !result.contains("No tests collected"),
            "Missing summary must not be rewritten as no tests: {result}"
        );
        assert!(result.contains("could not be parsed"));
        assert!(result.contains("exit code 1"));
        assert!(result.contains("collected 4"));
    }

    #[test]
    fn test_filter_pytest_subtest_summary_preserves_subtest_count() {
        let output = r#"=== test session starts ===
collected 4 items

tests/test_subtests.py ....                                         [100%]

=== 4 passed, 3 subtests passed in 0.50s ==="#;

        let result = filter_pytest_output(output);
        assert!(result.contains("4 passed"), "missing ordinary count: {result}");
        assert!(
            result.contains("3 subtests"),
            "missing subtest count: {result}"
        );
        assert!(result.contains("collected 4"));
    }

    #[test]
    fn test_filter_pytest_mixed_subtest_outcomes_preserve_both_counts() {
        let output = r#"=== test session starts ===
collected 4 items

tests/test_subtests.py ....                                         [100%]

=== 4 passed, 3 subtests passed, 1 subtests failed in 0.50s ==="#;

        let result = filter_pytest_output_with_exit(output, 1);
        assert!(result.contains("3 subtests passed"), "unexpected result: {result}");
        assert!(result.contains("1 subtests failed"), "unexpected result: {result}");
        assert!(!result.contains("exit code 1"), "unexpected result: {result}");
    }

    #[test]
    fn test_filter_pytest_real_no_tests_requires_explicit_signal_or_exit_code() {
        let explicit = filter_pytest_output_with_exit(
            "=== test session starts ===\ncollected 0 items\n=== no tests ran in 0.00s ===",
            5,
        );
        assert!(explicit.contains("No tests collected"));
        assert!(explicit.contains("exit code 5"));

        let missing_summary = filter_pytest_output_with_exit("collected 0 items\n", 1);
        assert!(!missing_summary.contains("No tests collected"));
        assert!(missing_summary.contains("could not be parsed"));
    }

    #[test]
    fn test_filter_pytest_long_silence_then_pass_keeps_final_summary() {
        // The absence of progress output is not evidence that collection
        // failed. A final summary after a silent interval remains authoritative.
        let output = r#"=== test session starts ===
collected 4 items

=== 4 passed in 420.00s ==="#;

        let result = filter_pytest_output_with_exit(output, 0);
        assert!(result.contains("4 passed"));
        assert!(result.contains("collected 4"));
        assert!(!result.contains("No tests collected"));
    }

    #[test]
    fn test_filter_pytest_ansi_and_nonstandard_summary_is_parseable() {
        let output = "=== test session starts ===\ncollected 2 items\n\n\x1b[32m= 2 passed in 0.10s =\x1b[0m\n";
        let result = filter_pytest_output_with_exit(output, 0);
        assert!(result.contains("2 passed"), "unexpected result: {result}");
        assert!(result.contains("collected 2"));
    }

    #[test]
    fn test_filter_pytest_ambiguous_bare_summaries_degrade() {
        let output = r#"=== test session starts ===
collected 2 items

2 passed in 0.10s
test output: 1 passed in 0.01s
"#;

        let result = filter_pytest_output_with_exit(output, 0);
        assert!(result.contains("could not be parsed"), "unexpected result: {result}");
        assert!(result.contains("exit code 0"));
        assert!(!result.contains("No tests collected"));
    }

    #[test]
    fn test_filter_pytest_capture_truncated_before_summary_is_unparseable() {
        let mut output = String::from("=== test session starts ===\ncollected 1 items\n");
        output.push_str(&"x".repeat(crate::core::stream::RAW_CAP));

        let result = filter_pytest_output_with_exit(&output, 0);
        assert!(result.contains("could not be parsed"));
        assert!(result.contains("collected 1"));
        assert!(!result.contains("No tests collected"));
    }
}
