//! Filters pytest output to show only failures and the summary line.

use crate::core::config;
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

    runner::run_filtered_with_exit(
        cmd,
        "pytest",
        &args.join(" "),
        |raw, exit_code| {
            let clean = strip_ansi(raw);
            let filtered = filter_pytest_output(&clean);
            // Any other failure parsed as empty means the run broke before reporting.
            if exit_code != 0 && exit_code != PYTEST_EXIT_NO_TESTS && filtered == PYTEST_NO_TESTS {
                return truncate(clean.trim(), config::limits().passthrough_max_chars);
            }
            filtered
        },
        runner::RunOptions::stdout_only().tee("pytest"),
    )
}

const PYTEST_NO_TESTS: &str = "Pytest: No tests collected";
const PYTEST_EXIT_NO_TESTS: i32 = 5;

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
                || trimmed.contains("skipped"))
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
        }

        // Process based on state
        match state {
            ParseState::Header => {
                if trimmed.starts_with("collected") {
                    state = ParseState::TestProgress;
                } else if is_pure_progress_line(trimmed) {
                    // Real quiet-mode (-q) output on a recent pytest can omit
                    // both the "=== test session starts ===" banner and the
                    // "collected N items" line entirely — the first thing on
                    // stdout is the dot-progress line itself. Without this,
                    // the parser never leaves Header and silently drops the
                    // one signal that proves tests actually ran.
                    state = ParseState::TestProgress;
                    test_files.push(trimmed.to_string());
                }
            }
            ParseState::TestProgress => {
                // Lines like "tests/test_foo.py ....  [ 40%]", or just
                // "....  [ 40%]" / "...." with no file-path prefix.
                if !trimmed.is_empty()
                    && !trimmed.starts_with("===")
                    && (trimmed.contains(".py")
                        || trimmed.contains("%]")
                        || is_pure_progress_line(trimmed))
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
}

fn build_pytest_summary(
    summary: &str,
    test_files: &[String],
    failures: &[String],
    xfail_lines: &[String],
) -> String {
    let mut counts = parse_summary_line(summary);
    let mut used_progress_fallback = false;

    // pytest can crash while reporting a session (e.g. a teardown error in
    // pytest_sessionfinish, such as Windows' tmp_path cleanup raising
    // PermissionError) after every test already ran but *before* it prints
    // its own final summary footer. When that happens `summary` is empty
    // and every count here is zero even though tests genuinely ran — fall
    // back to tallying the per-test progress indicators (".", "F", "E",
    // "s", "x", "X") already captured from the test-progress lines rather
    // than trusting the absence of a footer (or a non-zero exit code) as
    // proof nothing ran.
    if counts.passed == 0
        && counts.failed == 0
        && counts.skipped == 0
        && counts.xfailed == 0
        && counts.xpassed == 0
    {
        let derived = count_progress_chars(test_files);
        if derived.passed > 0
            || derived.failed > 0
            || derived.skipped > 0
            || derived.xfailed > 0
            || derived.xpassed > 0
        {
            counts = derived;
            used_progress_fallback = true;
        }
    }

    let PytestCounts {
        passed,
        failed,
        skipped,
        xfailed,
        xpassed,
    } = counts;

    if passed == 0 && failed == 0 && skipped == 0 && xfailed == 0 && xpassed == 0 {
        return PYTEST_NO_TESTS.to_string();
    }

    let fallback_note = if used_progress_fallback {
        " (derived, no summary footer)"
    } else {
        ""
    };

    let extras_present = skipped > 0 || xfailed > 0 || xpassed > 0 || !xfail_lines.is_empty();

    if failed == 0 && passed > 0 && !extras_present {
        return format!("Pytest: {} passed{}", passed, fallback_note);
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
        return format!("{}{}", result.trim(), fallback_note);
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

    format!("{}{}", result.trim(), fallback_note)
}

/// True when `trimmed` is (optionally "<path> " prefixed, optionally
/// " [ NN%]" suffixed) nothing but pytest's per-test result characters
/// (`.` pass, `F` fail, `E` error, `s` skip, `x` xfail, `X` xpass,
/// `!` internal error). Lets the parser recognize a bare quiet-mode
/// progress line even when no path prefix or percentage marker is present.
fn is_pure_progress_line(trimmed: &str) -> bool {
    let without_pct = match (trimmed.rfind('['), trimmed.ends_with(']')) {
        (Some(idx), true) => trimmed[..idx].trim_end(),
        _ => trimmed,
    };
    let candidate = match without_pct.rfind(' ') {
        Some(idx) => &without_pct[idx + 1..],
        None => without_pct,
    };

    !candidate.is_empty()
        && candidate
            .chars()
            .all(|c| matches!(c, '.' | 'F' | 'E' | 's' | 'x' | 'X' | '!'))
}

/// Tally per-test result indicators from pytest's quiet-mode progress lines
/// (e.g. `"tests/test_foo.py .....F.. [100%]"`) when no final summary
/// footer was captured. Recognizes both the dot-per-test quiet format and
/// verbose-mode `PASSED`/`FAILED`/... lines.
fn count_progress_chars(test_files: &[String]) -> PytestCounts {
    let mut counts = PytestCounts::default();

    for line in test_files {
        let upper_has_verbose_marker = line.contains("PASSED")
            || line.contains("FAILED")
            || line.contains("ERROR")
            || line.contains("SKIPPED")
            || line.contains("XFAIL")
            || line.contains("XPASS");

        if upper_has_verbose_marker {
            // Verbose mode: one status keyword per line, e.g.
            // "tests/test_foo.py::test_bar PASSED [ 20%]".
            if line.contains("XPASS") {
                counts.xpassed += 1;
            } else if line.contains("XFAIL") {
                counts.xfailed += 1;
            } else if line.contains("PASSED") {
                counts.passed += 1;
            } else if line.contains("FAILED") || line.contains("ERROR") {
                counts.failed += 1;
            } else if line.contains("SKIPPED") {
                counts.skipped += 1;
            }
            continue;
        }

        // Quiet mode: strip the leading "<path>.py" token and the
        // trailing "[ NN%]" progress marker, leaving just the run
        // characters (padded with spaces).
        let without_pct = match line.rfind('[') {
            Some(idx) => &line[..idx],
            None => line.as_str(),
        };
        let chars_part = match without_pct.find(".py") {
            Some(idx) => &without_pct[idx + 3..],
            None => without_pct,
        };

        for ch in chars_part.chars() {
            match ch {
                '.' => counts.passed += 1,
                'F' | 'E' => counts.failed += 1,
                's' => counts.skipped += 1,
                'x' => counts.xfailed += 1,
                'X' => counts.xpassed += 1,
                _ => {}
            }
        }
    }

    counts
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
    fn test_filter_pytest_teardown_crash_before_summary() {
        // Reproduces a real bug: on Windows, pytest_sessionfinish teardown
        // (e.g. tmp_path cleanup) can raise PermissionError *after* every
        // test already ran but *before* the "=== N passed in Ys ===" footer
        // prints. rtk only ever filters stdout (RunOptions::stdout_only());
        // the crash traceback lands on stderr and never reaches this
        // function. What's left on stdout, confirmed against a real
        // pytest 9.1.1 -q run on Windows, is nothing but the bare dot
        // progress line — no "=== test session starts ===" banner, no
        // "collected N items" line, no final summary footer, and (for a
        // single target file) not even a file-path prefix.
        let output = ".....                                                                    [100%]";

        let result = filter_pytest_output(output);
        assert!(
            !result.contains("No tests collected"),
            "Should not report 'No tests collected' when the progress line shows 5/5 passed. Got: {}",
            result
        );
        assert!(
            result.contains("5 passed"),
            "Should derive the true count from the progress line. Got: {}",
            result
        );
        assert!(
            result.contains("derived, no summary footer"),
            "Should flag that counts were derived, not read from a real footer. Got: {}",
            result
        );
    }

    #[test]
    fn test_filter_pytest_teardown_crash_with_path_prefix() {
        // Same scenario, but with the file-path prefix pytest sometimes
        // includes ahead of the dots (also seen in the wild for this bug).
        let output = "interface\\tests\\test_chat_session.py .....                               [100%]";

        let result = filter_pytest_output(output);
        assert!(
            !result.contains("No tests collected"),
            "Got: {}",
            result
        );
        assert!(result.contains("5 passed"), "Got: {}", result);
    }

    #[test]
    fn test_count_progress_chars_mixed_quiet_mode() {
        let lines = vec!["test_foo.py ..F..sxX                    [100%]".to_string()];
        let counts = count_progress_chars(&lines);
        assert_eq!(counts.passed, 4);
        assert_eq!(counts.failed, 1);
        assert_eq!(counts.skipped, 1);
        assert_eq!(counts.xfailed, 1);
        assert_eq!(counts.xpassed, 1);
    }
}
