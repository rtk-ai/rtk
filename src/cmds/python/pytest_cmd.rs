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
        } else if trimmed.starts_with("===")
            && (trimmed.contains("FAILURES") || trimmed.contains("ERRORS"))
        {
            // ERRORS sections (broken fixtures, collection errors) carry the
            // same kind of detail blocks as FAILURES and must not be dropped.
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
        // quiet mode (-q): bare summary without === wrapper, e.g.
        // "5 failed, 1698 passed, 2 skipped in 108.89s". Captured stdout from
        // failing tests flows through this loop too, so match the full summary
        // grammar instead of substrings, and let the last match win: the
        // genuine footer is always the final such line of a pytest run.
        } else if is_bare_summary_line(trimmed) {
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

/// Returns true only for a genuine quiet-mode bare summary line, e.g.
/// "1 failed, 4 passed in 0.02s" or "2 passed in 61.02s (0:01:01)".
///
/// Substring checks are not enough here: a failing test's captured stdout is
/// scanned by the same loop, and prose such as "retrying after error in
/// connection pool" must never be mistaken for the summary — it would
/// displace the real footer and misreport a red run as "No tests collected".
fn is_bare_summary_line(line: &str) -> bool {
    // Strip the human-readable "(h:mm:ss)" suffix pytest adds for runs > 60s.
    let mut rest = line;
    if rest.ends_with(')') {
        match rest.rfind(" (") {
            Some(idx) => rest = rest[..idx].trim_end(),
            None => return false,
        }
    }
    // The line must end with "in <duration>s", e.g. "in 108.89s".
    let Some((stats, duration)) = rest.rsplit_once(" in ") else {
        return false;
    };
    let ends_with_seconds = duration
        .strip_suffix('s')
        .is_some_and(|secs| secs.parse::<f64>().is_ok());
    if !ends_with_seconds {
        return false;
    }
    // Every comma-separated stat must have the "<count> <category>" shape
    // ("no tests ran in 0.01s" is intentionally rejected: with no counts to
    // parse it reports "No tests collected" either way).
    stats.split(", ").all(|part| {
        let Some((count, category)) = part.split_once(' ') else {
            return false;
        };
        count.parse::<usize>().is_ok()
            && matches!(
                category,
                "passed"
                    | "failed"
                    | "skipped"
                    | "deselected"
                    | "xfailed"
                    | "xpassed"
                    | "error"
                    | "errors"
                    | "warning"
                    | "warnings"
                    | "rerun"
            )
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
    } = counts;

    if passed == 0 && failed == 0 && skipped == 0 && xfailed == 0 && xpassed == 0 && errors == 0 {
        return "Pytest: No tests collected".to_string();
    }

    let extras_present = skipped > 0 || xfailed > 0 || xpassed > 0 || !xfail_lines.is_empty();

    // Errors (or any collected failure detail) must never collapse into the
    // all-green short form: that would hide real breakage from the caller.
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
            } else if first_line.starts_with("FAILED") || first_line.starts_with("ERROR") {
                // Summary format: "FAILED tests/test_foo.py::test_bar - AssertionError"
                // or "ERROR tests/test_foo.py::test_bar - RuntimeError" — without
                // this arm, single ERROR entries would render as empty items.
                let parts: Vec<&str> = first_line.split(" - ").collect();
                if let Some(test_path) = parts.first() {
                    let label = if first_line.starts_with("ERROR") {
                        "ERROR"
                    } else {
                        "FAIL"
                    };
                    let test_name = test_path
                        .trim_start_matches("FAILED ")
                        .trim_start_matches("ERROR ");
                    result.push_str(&format!("{}. [{}] {}\n", i + 1, label, test_name));
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
                // Matches both "1 error" and "2 errors" (setup/teardown or
                // collection errors). These are real failures for the caller.
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
    fn test_parse_summary_line_errors() {
        // -q mode: bare summary, singular "error"
        let c = parse_summary_line("20 passed, 1 error in 1.94s");
        assert_eq!((c.passed, c.failed, c.errors), (20, 0, 1));

        // plural "errors"
        let c = parse_summary_line("=== 3 passed, 2 errors in 0.50s ===");
        assert_eq!((c.passed, c.errors), (3, 2));

        // collection error: errors only
        let c = parse_summary_line("=== 1 error in 0.12s ===");
        assert_eq!((c.passed, c.failed, c.errors), (0, 0, 1));

        // mixed passed/failed/errors
        let c = parse_summary_line("1 passed, 2 failed, 3 errors in 1.00s");
        assert_eq!((c.passed, c.failed, c.errors), (1, 2, 3));
    }

    #[test]
    fn test_filter_pytest_errors_not_collapsed_to_all_green() {
        // Regression: "20 passed, 1 error" must not be reported as an
        // all-green "Pytest: 20 passed" — the error count was silently lost.
        let output = r#"=== test session starts ===
collected 21 items

tests/test_foo.py ....................E                            [100%]

=== ERRORS ===
___ ERROR at setup of test_broken ___
file tests/test_foo.py, line 30
E       fixture 'missing_fixture' not found

=== short test summary info ===
ERROR tests/test_foo.py::test_broken
20 passed, 1 error in 1.94s"#;

        let result = filter_pytest_output(output);
        assert!(result.contains("20 passed"), "got: {result}");
        assert!(result.contains("1 error"), "error count lost: {result}");
        assert!(result.contains("test_broken"), "error detail lost: {result}");
    }

    #[test]
    fn test_filter_pytest_collection_error() {
        // A collection error aborts the run (exit code 2); reporting it as
        // "No tests collected" hides the breakage from the caller.
        let output = r#"=== test session starts ===
collected 0 items / 1 error

=== ERRORS ===
___ ERROR collecting tests/test_bad.py ___
ImportError while importing test module 'tests/test_bad.py'.
E   ModuleNotFoundError: No module named 'nonexistent'

=== short test summary info ===
ERROR tests/test_bad.py
!!!!!!! Interrupted: 1 error during collection !!!!!!!
=== 1 error in 0.12s ==="#;

        let result = filter_pytest_output(output);
        assert!(
            !result.contains("No tests collected"),
            "collection error misreported as empty run: {result}"
        );
        assert!(result.contains("1 error"), "got: {result}");
        assert!(result.contains("test_bad.py"), "got: {result}");
    }

    #[test]
    fn test_filter_pytest_mixed_failures_and_errors() {
        let output = r#"=== test session starts ===
collected 10 items

tests/test_foo.py .......FFE                                       [100%]

=== FAILURES ===
___ test_one ___
E   AssertionError: expected 5

=== short test summary info ===
FAILED tests/test_foo.py::test_one - AssertionError: expected 5
FAILED tests/test_foo.py::test_two - ValueError
ERROR tests/test_foo.py::test_three - RuntimeError: broken fixture
7 passed, 2 failed, 1 error in 0.30s"#;

        let result = filter_pytest_output(output);
        assert!(
            result.contains("7 passed, 2 failed, 1 error"),
            "got: {result}"
        );
        assert!(result.contains("test_three"), "got: {result}");
        assert!(result.contains("[ERROR]"), "got: {result}");
    }

    #[test]
    fn test_is_bare_summary_line_anchoring() {
        // Genuine quiet-mode footers must match.
        assert!(is_bare_summary_line("5 passed in 0.01s"));
        assert!(is_bare_summary_line("1 failed, 4 passed in 0.02s"));
        assert!(is_bare_summary_line("20 passed, 1 error in 1.94s"));
        assert!(is_bare_summary_line("5 failed, 1698 passed, 2 skipped in 108.89s"));
        assert!(is_bare_summary_line("2 passed, 2 xfailed, 1 xpassed in 0.05s"));
        // Runs over 60s get a human-readable suffix.
        assert!(is_bare_summary_line("1 failed, 1 passed in 61.02s (0:01:01)"));
        // Prose from captured stdout must never match.
        assert!(!is_bare_summary_line("retrying after error in connection pool"));
        assert!(!is_bare_summary_line("worker 3 passed in shard cleanup"));
        assert!(!is_bare_summary_line("5 failed widgets in 1.2s"));
        assert!(!is_bare_summary_line("processed 12 items in 0.5s"));
        // Lines owned by other parser arms must not match either.
        assert!(!is_bare_summary_line("=== 5 passed in 0.50s ==="));
        assert!(!is_bare_summary_line("FAILED tests/test_a.py::test_x - retry error in 2s"));
        // Zero counts to parse: reports "No tests collected" with or without
        // capture, so rejecting keeps behavior identical.
        assert!(!is_bare_summary_line("no tests ran in 0.01s"));
    }

    #[test]
    fn test_filter_pytest_stdout_prose_not_mistaken_for_summary() {
        // Regression (review-found): captured stdout of a failing test
        // containing "... error ... in ..." prose used to be captured as the
        // summary line, displacing the real footer and reporting a red run
        // as "No tests collected".
        let output = r#"=== test session starts ===
collected 2 items

tests/test_net.py .F                                               [100%]

=== FAILURES ===
___ test_pool_retry ___
tests/test_net.py:14: in test_pool_retry
    assert pool.fetch() == "ok"
E   AssertionError: assert 'fail' == 'ok'
---------- Captured stdout call ----------
retrying after error in connection pool
worker 3 passed in shard cleanup
1 failed, 1 passed in 0.02s"#;

        let result = filter_pytest_output(output);
        assert!(
            !result.contains("No tests collected"),
            "stdout prose displaced the real summary: {result}"
        );
        assert!(result.contains("1 passed, 1 failed"), "got: {result}");
        assert!(result.contains("test_pool_retry"), "got: {result}");
    }

    #[test]
    fn test_filter_pytest_verbatim_inner_summary_in_stdout() {
        // A test driving pytest itself can print a verbatim summary line;
        // the genuine footer is always the last one and must win.
        let output = r#"=== test session starts ===
collected 3 items

tests/test_runner.py ..F                                           [100%]

=== FAILURES ===
___ test_wrapper_output ___
tests/test_runner.py:9: in test_wrapper_output
    assert run_inner() == expected
E   AssertionError
---------- Captured stdout call ----------
2 passed in 1.50s
1 failed, 2 passed in 0.03s"#;

        let result = filter_pytest_output(output);
        assert!(result.contains("2 passed, 1 failed"), "got: {result}");
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
}
