//! Filters pytest output to show only failures and the summary line.

use crate::core::arg_tokenizer::{Dialect, TokenKind, ValueSpec, tokenize_grammar};
use crate::core::config;
use crate::core::runner;
use crate::core::shell::display_args;
use crate::core::truncate::CAP_WARNINGS;
use crate::core::utils::{resolved_command, strip_ansi, tool_exists, truncate};
use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

const MAX_XFAIL: usize = CAP_WARNINGS;
const MAX_PYTEST_FAILURES: usize = CAP_WARNINGS;
const MAX_TRACEBACK_LINES: usize = 3;
const MAX_ERROR_LINES: usize = 4;
static PYTEST_SUMMARY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:=+\s*)?\d+ [^,\r\n=]+?(?:, \d+ [^,\r\n=]+?)* in \d+(?:\.\d+)?(?:s| seconds)(?: \((?:\d+ days?, )?\d+:\d{2}:\d{2}\))?(?:\s*=+)?$",
    )
    .expect("valid pytest summary regex")
});

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

    let injected = injected_args(args);
    cmd.args(&injected);
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: pytest {} {}", injected.join(" "), args.join(" "));
    }

    runner::run_filtered_with_exit(
        cmd,
        "pytest",
        &display_args(args),
        |raw, exit_code| {
            let filtered = filter_pytest_output(raw);
            // Any other failure parsed as empty means the run broke before reporting.
            if exit_code != 0 && exit_code != PYTEST_EXIT_NO_TESTS && filtered == PYTEST_NO_TESTS {
                return truncate(
                    strip_ansi(raw).trim(),
                    config::limits().passthrough_max_chars,
                );
            }
            filtered
        },
        runner::RunOptions::stdout_only().tee("pytest"),
    )
}

fn injected_args(args: &[String]) -> Vec<&'static str> {
    let tokens = tokenize_grammar(
        args,
        &|kind, name| match (kind, name) {
            (TokenKind::Long, "tb")
            | (TokenKind::Short, "k" | "m" | "p" | "c" | "o" | "W" | "r") => {
                Some(ValueSpec::value())
            }
            _ => None,
        },
        Dialect::Posix,
    );
    let tb = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Long && t.text == "tb")
        .filter_map(|t| t.value(&tokens))
        .next_back();
    let has_flag = |short: &str, long: &str| {
        tokens.iter().any(|t| {
            (t.kind == TokenKind::Short && t.text == short)
                || (t.kind == TokenKind::Long && t.text == long)
        })
    };
    let has_quiet_flag = has_flag("q", "quiet");
    let has_report_flag = has_flag("r", "report-chars");

    let mut injected = Vec::new();
    if tb.is_none() {
        injected.push("--tb=short");
    }
    if !has_quiet_flag {
        injected.push("-q");
    }
    // Surface xfailed/xpassed (and their reasons) in the short summary section
    // so the compact output can report expected failures and — crucially —
    // unexpected passes (XPASS), which signal a behavior change.
    if !has_report_flag {
        injected.push(if tb == Some("no") { "-rfExX" } else { "-rxX" });
    }
    injected
}

const PYTEST_NO_TESTS: &str = "Pytest: No tests collected";
const PYTEST_EXIT_NO_TESTS: i32 = 5;

pub(crate) fn filter_pytest_output(output: &str) -> String {
    // pytest colors its summary even when piped (force-color CI/venv setups), and the
    // `N passed` / FAILURES anchors below never match through ANSI codes. The parser owns
    // the stripping, as `filter_phpunit_output` does, so no caller carries a precondition.
    let cleaned = strip_ansi(output);
    let output = cleaned.as_str();
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
        // pytest prints setup/teardown/collection errors in an ERRORS section
        // shaped like FAILURES, so both feed the same failure blocks.
        } else if trimmed.starts_with("===")
            && (trimmed.contains("FAILURES") || trimmed.contains("ERRORS"))
        {
            state = ParseState::Failures;
            if !current_failure.is_empty() {
                failures.push(current_failure.join("\n"));
                current_failure.clear();
            }
            continue;
        } else if trimmed.starts_with("===") && trimmed.contains("short test summary") {
            state = ParseState::Summary;
            // Save current failure if any
            if !current_failure.is_empty() {
                failures.push(current_failure.join("\n"));
                current_failure.clear();
            }
            continue;
        } else if PYTEST_SUMMARY_RE.is_match(trimmed) {
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
                if trimmed.starts_with("===")
                    && trimmed.trim_matches('=').trim() == "warnings summary"
                {
                    if !current_failure.is_empty() {
                        failures.push(current_failure.join("\n"));
                        current_failure.clear();
                    }
                    state = ParseState::TestProgress;
                } else if is_block_header(trimmed) {
                    // New failure section
                    if !current_failure.is_empty() {
                        failures.push(current_failure.join("\n"));
                        current_failure.clear();
                    }
                    current_failure.push(trimmed.to_string());
                } else if !trimmed.is_empty() && !trimmed.starts_with("===") {
                    current_failure.push(line.trim_end().to_string());
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
    duration: Option<String>,
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
        duration,
    } = counts;

    if passed == 0 && failed == 0 && skipped == 0 && xfailed == 0 && xpassed == 0 && errors == 0 {
        return PYTEST_NO_TESTS.to_string();
    }

    let extras_present = skipped > 0 || xfailed > 0 || xpassed > 0 || !xfail_lines.is_empty();
    let duration_suffix = duration
        .as_deref()
        .map(|value| format!(" in {value}"))
        .unwrap_or_default();

    if failed == 0 && errors == 0 && passed > 0 && !extras_present {
        return format!("Pytest: {passed} passed{duration_suffix}");
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
        let noun = if errors == 1 { "error" } else { "errors" };
        result.push_str(&format!(", {errors} {noun}"));
    }
    result.push_str(&duration_suffix);
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
            if let Some(hint) =
                crate::core::tee::force_tee_tail_hint(&all_xfail, "pytest-xfail", MAX_XFAIL + 1)
            {
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
            if is_block_header(first_line) {
                // Extract test name between ___
                let test_name = first_line.trim_matches('_').trim();
                // ERRORS section headers read "ERROR at setup of test_x" / "ERROR collecting x.py"
                match test_name.strip_prefix("ERROR ") {
                    Some(what) => result.push_str(&format!("{}. [ERROR] {}\n", i + 1, what)),
                    None => result.push_str(&format!("{}. [FAIL] {}\n", i + 1, test_name)),
                }
            } else if first_line.starts_with("FAILED") || first_line.starts_with("ERROR") {
                // Summary format: "FAILED tests/test_foo.py::test_bar - AssertionError"
                let parts: Vec<&str> = first_line.split(" - ").collect();
                if let Some(test_path) = parts.first() {
                    let (label, test_name) = match test_path.strip_prefix("ERROR ") {
                        Some(name) => ("ERROR", name),
                        None => ("FAIL", test_path.trim_start_matches("FAILED ")),
                    };
                    result.push_str(&format!("{}. [{label}] {}\n", i + 1, test_name));
                }
                if parts.len() > 1 {
                    result.push_str(&format!("     {}\n", truncate(parts[1], 100)));
                }
                continue;
            }
        }

        // Show relevant error lines (assertions, errors, file locations)
        let (body, collecting) = match lines.first() {
            Some(first_line) if is_block_header(first_line) => {
                (&lines[1..], first_line.contains(" ERROR collecting "))
            }
            _ => (&lines[..], false),
        };
        let traceback_end = body
            .iter()
            .position(|l| is_captured_section(l))
            .unwrap_or(body.len());
        let traceback = &body[..traceback_end];
        let frame = |l: &&str| !is_error_line(l) && frame_file(l).is_some();
        let location = traceback.iter().rposition(frame);
        let location_file = location.and_then(|at| frame_file(traceback[at]));
        let anchor = traceback
            .iter()
            .position(frame)
            .filter(|&at| !collecting && frame_file(traceback[at]) != location_file);
        let error_total = traceback.iter().filter(|l| is_error_line(l)).count();
        let error_shown = error_total.min(MAX_ERROR_LINES);
        let error_head = error_shown / 2;
        let error_tail_start = error_total - (error_shown - error_head);
        let mut context_left = MAX_TRACEBACK_LINES.saturating_sub(
            error_shown + usize::from(location.is_some()) + usize::from(anchor.is_some()),
        );
        let mut error_index = 0;
        for (index, line) in body.iter().enumerate() {
            let show = if index < traceback_end && is_error_line(line) {
                error_index += 1;
                if error_index == error_head + 1 && error_tail_start > error_head {
                    let hidden = error_tail_start - error_head;
                    let noun = if hidden == 1 { "line" } else { "lines" };
                    result.push_str(&format!("     … +{hidden} more E {noun}\n"));
                }
                error_index <= error_head || error_index > error_tail_start
            } else if Some(index) == location || Some(index) == anchor {
                true
            } else {
                let trimmed = line.trim();
                let line_lower = line.to_lowercase();
                let is_relevant = trimmed.starts_with('>')
                    || trimmed.starts_with('E')
                    || line_lower.contains("assert")
                    || line_lower.contains("error")
                    || line.contains(".py:");
                let fits = is_relevant && context_left > 0;
                if fits {
                    context_left -= 1;
                }
                fits
            };
            if show {
                result.push_str(&format!("     {}\n", truncate(line.trim(), 100)));
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

fn is_error_line(line: &str) -> bool {
    line.strip_prefix("E ")
        .is_some_and(|rest| !rest.trim_start().starts_with("= "))
}

fn is_block_header(line: &str) -> bool {
    line.trim_start_matches('_')
        .strip_prefix(' ')
        .and_then(|rest| rest.trim_end_matches('_').strip_suffix(' '))
        .is_some_and(|title| title.chars().any(|c| c != '_' && !c.is_whitespace()))
}

fn frame_file(line: &str) -> Option<&str> {
    if line.starts_with(|c: char| c.is_whitespace() || c == '>') {
        return None;
    }
    line.match_indices(".py:")
        .find(|(at, _)| line[at + 4..].starts_with(|c: char| c.is_ascii_digit()))
        .map(|(at, _)| &line[..at + 3])
        .filter(|file| !file.contains(" = "))
}

fn is_captured_section(line: &str) -> bool {
    line.starts_with("---") && line.contains(" Captured ")
}

fn parse_summary_line(summary: &str) -> PytestCounts {
    let mut counts = PytestCounts {
        duration: parse_duration(summary),
        ..PytestCounts::default()
    };

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
            } else if word.starts_with("error") {
                counts.errors = n;
            }
        }
    }

    counts
}

fn parse_duration(summary: &str) -> Option<String> {
    let unwrapped = summary.trim().trim_matches('=').trim();
    let (_, duration_tail) = unwrapped.rsplit_once(" in ")?;
    let mut words = duration_tail.split_whitespace();
    let duration = words.next()?;
    let seconds = if let Some(seconds) = duration.strip_suffix('s') {
        seconds
    } else if words.next() == Some("seconds") {
        duration
    } else {
        return None;
    };
    seconds.parse::<f64>().ok()?;
    Some(format!("{seconds}s"))
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
        assert_eq!(result, "Pytest: 5 passed in 0.50s");
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
        assert!(result.starts_with("Pytest: 4 passed, 1 failed in 0.50s"));
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
        assert_eq!(c.duration.as_deref(), Some("0.50s"));

        let c = parse_summary_line("=== 4 passed, 1 failed in 0.50s ===");
        assert_eq!((c.passed, c.failed, c.skipped), (4, 1, 0));
        assert_eq!(c.duration.as_deref(), Some("0.50s"));

        let c = parse_summary_line("=== 3 passed, 1 failed, 2 skipped in 1.0s ===");
        assert_eq!((c.passed, c.failed, c.skipped), (3, 1, 2));
        assert_eq!(c.duration.as_deref(), Some("1.0s"));

        let c = parse_summary_line("=== 2 passed, 1 failed, 2 xfailed, 1 xpassed in 1.0s ===");
        assert_eq!((c.passed, c.failed, c.xfailed, c.xpassed), (2, 1, 2, 1));
        assert_eq!(c.duration.as_deref(), Some("1.0s"));
    }

    #[test]
    fn test_filter_pytest_quiet_mode_preserves_duration() {
        let output = "2 passed in 3.04s";

        assert_eq!(filter_pytest_output(output), "Pytest: 2 passed in 3.04s");
    }

    #[test]
    fn test_filter_pytest_long_run_preserves_seconds_duration() {
        let output = "2 passed in 65.12s (0:01:05)";

        assert_eq!(filter_pytest_output(output), "Pytest: 2 passed in 65.12s");
    }

    #[test]
    fn test_parse_duration_rejects_tails_that_are_not_seconds() {
        assert_eq!(
            parse_duration("=== 2 passed in 0.50s ===").as_deref(),
            Some("0.50s")
        );
        assert_eq!(
            parse_duration("2 passed in 65.12s (0:01:05)").as_deref(),
            Some("65.12s")
        );
        assert_eq!(parse_duration("=== 2 passed ==="), None);
        assert_eq!(parse_duration("2 passed in seconds"), None);
        assert_eq!(parse_duration("2 passed in 0.50"), None);
    }

    #[test]
    fn test_filter_pytest_duration_on_every_summary_shape() {
        // Wrapped long-run form with a warnings part before the duration.
        let output = "=== 5 passed, 2 warnings in 65.12s (0:01:05) ===";
        assert_eq!(filter_pytest_output(output), "Pytest: 5 passed in 65.12s");

        // Only-skipped run takes the multi-line emit path.
        let output = "=== 3 skipped in 0.10s ===";
        assert_eq!(
            filter_pytest_output(output),
            "Pytest: 0 passed, 0 failed, 3 skipped in 0.10s"
        );

        // Quiet-mode failures line keeps its duration on the multi-line path too.
        let output = "5 failed, 1698 passed, 2 skipped in 108.89s";
        assert_eq!(
            filter_pytest_output(output),
            "Pytest: 1698 passed, 5 failed, 2 skipped in 108.89s"
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
        assert!(listed <= 10, "MAX_XFAIL cap not enforced: listed {listed}");
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
    fn test_filter_pytest_summary_over_60s_with_clock() {
        // Once elapsed time passes 60s, pytest appends a parenthetical clock.
        let long_runs = [
            "===== 40 passed in 65.12s (0:01:05) =====",
            "40 passed in 65.12s (0:01:05)",
        ];
        let short = filter_pytest_output("===== 40 passed in 0.12s =====");
        assert_eq!(short, "Pytest: 40 passed in 0.12s");
        assert_eq!(
            filter_pytest_output("40 passed in 12s"),
            "Pytest: 40 passed in 12s"
        );

        for line in long_runs {
            let result = filter_pytest_output(line);
            assert!(
                !result.contains("No tests collected"),
                "summary over 60s was not recognized: {line:?} -> {result}"
            );
            assert_eq!(
                result, "Pytest: 40 passed in 65.12s",
                "counts diverged for {line:?}: {result}"
            );
        }

        let not_a_summary = "see 40 passed in 65.12s (0:01:05) in the log";
        assert!(
            !PYTEST_SUMMARY_RE.is_match(not_a_summary),
            "non-summary lines must still be rejected"
        );
    }

    #[test]
    fn test_filter_pytest_uses_trailing_summary_not_traceback_source() {
        let output = r#"=== FAILURES ===
___ test_outer ___
--- Captured stdout call ---
1 passed in 0.01s
E   AssertionError: test_outer failed
=== short test summary info ===
FAILED test_outer - AssertionError
1 failed in 0.02s"#;

        let result = filter_pytest_output(output);
        assert!(
            result.starts_with("Pytest: 0 passed, 1 failed in 0.02s"),
            "{result}"
        );
        assert!(result.contains("test_outer"), "{result}");
    }

    #[test]
    fn test_filter_pytest_plugin_and_legacy_summaries() {
        for (input, expected) in [
            (
                "2 passed, 3 subtests passed in 0.00s",
                "Pytest: 2 passed in 0.00s",
            ),
            ("2 passed, 1 rerun in 0.01s", "Pytest: 2 passed in 0.01s"),
            (
                "===== 5 passed in 90000.00s (1 day, 1:00:00) =====",
                "Pytest: 5 passed in 90000.00s",
            ),
            (
                "5 passed in 180000.00s (2 days, 2:00:00)",
                "Pytest: 5 passed in 180000.00s",
            ),
            (
                "1 passed, 1 skipped, 1 xfailed in 0.01 seconds",
                "Pytest: 1 passed, 0 failed, 1 skipped, 1 xfailed in 0.01s",
            ),
            (
                "2 failed, 1 passed, 2 subtests passed in 0.02s",
                "Pytest: 1 passed, 2 failed in 0.02s",
            ),
        ] {
            assert_eq!(filter_pytest_output(input), expected, "{input}");
        }
    }

    #[test]
    fn test_filter_pytest_rejects_summary_fragments_without_final_result() {
        for input in [
            "=== FAILURES ===\n___ test_outer ___\nE + 2 passed in 0.01s",
            "2 passed in 0.01s, then 1 failed",
            "2 passed in 0.01s trailing junk",
            "40 passed",
            "40 passed in 65.12s (see log)",
        ] {
            assert_eq!(filter_pytest_output(input), PYTEST_NO_TESTS, "{input}");
        }
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
    fn test_filter_pytest_fixture_error_is_reported() {
        // Real pytest 9.1.1 `-q --tb=short -rxX` output for a fixture that raises (#4426).
        let output = r#"E.                                                                       [100%]
==================================== ERRORS ====================================
___________________________ ERROR at setup of test_e ___________________________
test_e.py:3: in boom
    def boom(): raise RuntimeError("fixture")
E   RuntimeError: fixture
1 passed, 1 error in 0.02s"#;

        let result = filter_pytest_output(output);
        assert!(
            result.starts_with("Pytest: 1 passed, 0 failed, 1 error in 0.02s"),
            "error count missing from summary: {result}"
        );
        assert_ne!(result, "Pytest: 1 passed in 0.02s");
        assert!(
            result.contains("[ERROR] at setup of test_e"),
            "error block missing: {result}"
        );
        assert!(result.contains("RuntimeError: fixture"), "got: {result}");
    }

    #[test]
    fn test_filter_pytest_errors_and_failures_together() {
        let output = r#"FEE.                                                                     [100%]
==================================== ERRORS ====================================
__________________________ ERROR at setup of test_one __________________________
test_x.py:5: in db
    raise ConnectionError("db down")
E   ConnectionError: db down
______________________ ERROR at teardown of test_two _______________________
test_x.py:11: in tmp
    os.remove(path)
E   FileNotFoundError: [Errno 2] No such file or directory: 'x'
=================================== FAILURES ===================================
__________________________________ test_bad ____________________________________
test_x.py:20: in test_bad
    assert 1 == 2
E   assert 1 == 2
1 failed, 1 passed, 2 errors in 0.05s"#;

        let result = filter_pytest_output(output);
        assert!(
            result.starts_with("Pytest: 1 passed, 1 failed, 2 errors in 0.05s"),
            "got: {result}"
        );
        assert!(
            result.contains("[ERROR] at setup of test_one"),
            "got: {result}"
        );
        assert!(
            result.contains("[ERROR] at teardown of test_two"),
            "got: {result}"
        );
        assert!(result.contains("[FAIL] test_bad"), "got: {result}");
        assert!(result.contains("ConnectionError: db down"), "got: {result}");
    }

    #[test]
    fn test_filter_pytest_collection_error_is_reported() {
        // `--continue-on-collection-errors`: other tests still run.
        let output = r#".                                                                        [100%]
==================================== ERRORS ====================================
_______________________ ERROR collecting test_bad.py ________________________
test_bad.py:1: in <module>
    raise RuntimeError("boom at import")
E   RuntimeError: boom at import
1 passed, 1 error in 0.03s"#;

        let result = filter_pytest_output(output);
        assert!(
            result.starts_with("Pytest: 1 passed, 0 failed, 1 error in 0.03s"),
            "got: {result}"
        );
        assert!(
            result.contains("[ERROR] collecting test_bad.py"),
            "got: {result}"
        );
        assert!(result.contains("boom at import"), "got: {result}");
    }

    #[test]
    fn test_filter_pytest_collection_import_error_keeps_message() {
        let output = include_str!("../../../tests/fixtures/pytest_collection_import_error_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 0 failed, 1 error in 0.04s

Failures:
1. [ERROR] collecting test_imp.py
     ImportError while importing test module '/home/user/project/test_imp.py'.
     test_imp.py:1: in <module>
     E   ModuleNotFoundError: No module named 'nonexistent_module_xyz'"#
        );
    }

    #[test]
    fn test_filter_pytest_collection_syntax_error_keeps_location() {
        let output = include_str!("../../../tests/fixtures/pytest_collection_syntax_error_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 0 failed, 1 error in 0.08s

Failures:
1. [ERROR] collecting test_syn.py
     /usr/lib/python3.13/ast.py:50: in parse
     E     File "/home/user/project/test_syn.py", line 1
     E       def test_x(:
     E                  ^
     E   SyntaxError: invalid syntax"#
        );
    }

    #[test]
    fn test_injected_args_follow_the_tb_style() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(injected_args(&args(&[])), ["--tb=short", "-q", "-rxX"]);
        assert_eq!(injected_args(&args(&["--tb=no"])), ["-q", "-rfExX"]);
        assert_eq!(injected_args(&args(&["--tb", "no"])), ["-q", "-rfExX"]);
        assert_eq!(
            injected_args(&args(&["--tb=short", "-x", "--tb=no"])),
            ["-q", "-rfExX"]
        );
        assert_eq!(injected_args(&args(&["--tb=line"])), ["-q", "-rxX"]);
        assert_eq!(
            injected_args(&args(&["--tb=no", "--tb=long"])),
            ["-q", "-rxX"]
        );
        assert_eq!(
            injected_args(&args(&["-k", "no"])),
            ["--tb=short", "-q", "-rxX"]
        );
        assert_eq!(
            injected_args(&args(&["--", "--tb=no"])),
            ["--tb=short", "-q", "-rxX"]
        );
        assert_eq!(injected_args(&args(&["--tb=no", "-rA"])), ["-q"]);
        assert_eq!(injected_args(&args(&["--tb=no", "-qx"])), ["-rfExX"]);
        assert_eq!(injected_args(&args(&["-xq"])), ["--tb=short", "-rxX"]);
        assert_eq!(injected_args(&args(&["--quiet"])), ["--tb=short", "-rxX"]);
        assert_eq!(injected_args(&args(&["-xrfE"])), ["--tb=short", "-q"]);
        assert_eq!(injected_args(&args(&["-r", "fE"])), ["--tb=short", "-q"]);
        assert_eq!(
            injected_args(&args(&["--report-chars=fE"])),
            ["--tb=short", "-q"]
        );
        assert_eq!(
            injected_args(&args(&["-pno:cacheprovider"])),
            ["--tb=short", "-q", "-rxX"]
        );
        for value_flag in [
            "-ktest_query",
            "-mquick",
            "-prerunfailures",
            "-Werror",
            "-cqa.ini",
            "-orequired_plugins=x",
        ] {
            assert_eq!(
                injected_args(&args(&[value_flag])),
                ["--tb=short", "-q", "-rxX"]
            );
        }
        assert_eq!(injected_args(&args(&["-rfEq"])), ["--tb=short", "-q"]);
        assert_eq!(
            injected_args(&args(&["q", "r"])),
            ["--tb=short", "-q", "-rxX"]
        );
        assert_eq!(
            injected_args(&args(&["--", "-q"])),
            ["--tb=short", "-q", "-rxX"]
        );
    }

    #[test]
    fn test_filter_pytest_tb_no_keeps_short_summary_messages() {
        let output = include_str!("../../../tests/fixtures/pytest_tb_no_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 1 passed, 1 failed, 1 xfailed, 1 xpassed, 1 error in 0.01s

Expected-failure outcomes:
  XFAIL test_tbno.py::test_xf - ticket 7
  XPASS test_tbno.py::test_xp - fixed upstream?

Failures:
1. [FAIL] test_tbno.py::test_bad
     AssertionError: wrong total
2. [ERROR] test_tbno.py::test_db
     ConnectionError: db down"#
        );
    }

    #[test]
    fn test_filter_pytest_deep_assertion_keeps_message() {
        let output = include_str!("../../../tests/fixtures/pytest_deep_assertion_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.01s

Failures:
1. [FAIL] test_deep
     test_deep.py:2: in check
     E   AssertionError: bad status
     E   assert {'code': 500, 'body': 'err'} == {'code': 200, 'body': 'ok'}
     … +2 more E lines
     E     {'body': 'err'} != {'body': 'ok'}
     E     Use -v to get more diff"#
        );
    }

    #[test]
    fn test_filter_pytest_dict_diff_keeps_location_and_message() {
        let output = include_str!("../../../tests/fixtures/pytest_dict_diff_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.01s

Failures:
1. [FAIL] test_dict
     test_cases.py:2: in test_dict
     E   AssertionError: assert {'a': 1, 'b': 2} == {'a': 1, 'b': 3}
     E     Omitting 1 identical items, use -vv to show
     … +1 more E line
     E     {'b': 2} != {'b': 3}
     E     Use -v to get more diff"#
        );
    }

    #[test]
    fn test_filter_pytest_tb_long_reads_only_column_zero_e_lines() {
        let output = include_str!("../../../tests/fixtures/pytest_tb_long_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 2 failed in 0.01s

Failures:
1. [FAIL] test_p
     E       AssertionError: state drifted
     E       assert {'b': 2} == {'b': 5}
     … +1 more E line
     E         {'b': 2} != {'b': 5}
     E         Use -v to get more diff
     test_long.py:4: AssertionError

2. [FAIL] test_raise
     >       raise ValueError("boom")
     E       ValueError: boom
     test_long.py:8: ValueError"#
        );
    }

    #[test]
    fn test_filter_pytest_warnings_summary_is_not_a_location() {
        let output = include_str!("../../../tests/fixtures/pytest_failure_with_warning_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 1 passed, 1 failed in 0.01s

Failures:
1. [FAIL] test_total
     test_warn.py:9: in test_total
     E   AssertionError: totals differ
     E   assert [0, 1, 2, 3, 4] == [1, 2, 3, 4, 5]
     E     At index 0 diff: 0 != 1
     E     Use -v to get more diff"#
        );
    }

    #[test]
    fn test_filter_pytest_config_warning_is_not_a_location() {
        let output = include_str!("../../../tests/fixtures/pytest_config_warning_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.01s

Failures:
1. [FAIL] test_bad
     test_w.py:6: in test_bad
     E   AssertionError: assert 1 == 2
     E    +  where 1 = helper({'k': 1})"#
        );
    }

    #[test]
    fn test_filter_pytest_location_is_not_an_e_line() {
        let output = include_str!("../../../tests/fixtures/pytest_lint_location_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.01s

Failures:
1. [FAIL] test_lint
     test_lint.py:6: in test_lint
     E   AssertionError: assert ['foo.py:3: E...ine too long'] == []
     E     Left contains one more item: 'foo.py:3: E501 line too long'
     E     Use -v to get more diff"#
        );
    }

    #[test]
    fn test_filter_pytest_captured_e_line_is_not_an_error_line() {
        let output = include_str!("../../../tests/fixtures/pytest_captured_e_line_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.01s

Failures:
1. [FAIL] test_noisy
     test_cap.py:3: in test_noisy
     E   AssertionError: Captured total mismatch
     E   assert 1 == 2"#
        );
    }

    #[test]
    fn test_filter_pytest_tb_long_locals_keeps_the_failing_frame() {
        let output = include_str!("../../../tests/fixtures/pytest_tb_long_locals_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.01s

Failures:
1. [FAIL] test_loc
     >       check(seen)
     E       assert 2 == 3
     test_loc.py:3: AssertionError"#
        );
    }

    #[test]
    fn test_filter_pytest_missing_fixture_keeps_location() {
        let output = include_str!("../../../tests/fixtures/pytest_missing_fixture_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 0 failed, 1 error in 0.01s

Failures:
1. [ERROR] at setup of test_fx
     E       fixture 'missing_fixture' not found
     >       available fixtures: capfd, capfdbinary, caplog, capsys, capsysbinary, capteesys, doctest_...
     /home/user/project/test_fx.py:1"#
        );
    }

    #[test]
    fn test_filter_pytest_helper_module_keeps_the_test_frame() {
        let output = include_str!("../../../tests/fixtures/pytest_helper_module_raise_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.02s

Failures:
1. [FAIL] test_frames
     test_lib.py:5: in test_frames
     mylib/checks.py:3: in assert_frame_equal
     E   AssertionError: DataFrame are different
     E   DataFrame shape mismatch
     E   [left]:  (3, 2)
     E   [right]: (3, 4)"#
        );
    }

    #[test]
    fn test_filter_pytest_tb_long_helper_module_keeps_the_test_frame() {
        let output = include_str!("../../../tests/fixtures/pytest_tb_long_helper_module_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.01s

Failures:
1. [FAIL] test_get
     test_key.py:5:
     E       KeyError: 'b'
     mylib/store.py:2: KeyError"#
        );
    }

    #[test]
    fn test_filter_pytest_locals_named_e_are_not_error_lines() {
        let output = include_str!("../../../tests/fixtures/pytest_tb_long_locals_named_e_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.02s

Failures:
1. [FAIL] test_p
     E       AssertionError: state drifted
     E       assert {'b': 2} == {'b': 5}
     … +1 more E line
     E         {'b': 2} != {'b': 5}
     E         Use -v to get more diff
     test_le.py:4: AssertionError"#
        );
    }

    #[test]
    fn test_filter_pytest_equals_run_diff_line_is_an_error_line() {
        let output = include_str!("../../../tests/fixtures/pytest_rst_heading_diff_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.01s

Failures:
1. [FAIL] test_rst
     test_rst.py:2: in test_rst
     E   AssertionError: assert '=====\nbody a\n' == '=====\nbody b\n'
     E       =====
     … +2 more E lines
     E     + body a
     E     ?      ^"#
        );
    }

    #[test]
    fn test_filter_pytest_every_captured_section_is_cut() {
        let output = include_str!("../../../tests/fixtures/pytest_two_captured_sections_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.02s

Failures:
1. [FAIL] test_two
     test_tc.py:9: in test_two
     E   AssertionError: two captured sections
     E   assert 0"#
        );
    }

    #[test]
    fn test_filter_pytest_captured_e_lines_do_not_count_against_the_diff() {
        let output = include_str!("../../../tests/fixtures/pytest_captured_dict_diff_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.01s

Failures:
1. [FAIL] test_td
     test_td.py:7: in test_td
     E   AssertionError: assert {'a': 1, 'b': 2} == {'a': 1, 'b': 3}
     E     Omitting 1 identical items, use -vv to show
     … +1 more E line
     E     {'b': 2} != {'b': 3}
     E     Use -v to get more diff"#
        );
    }

    #[test]
    fn test_filter_pytest_durations_are_not_a_location() {
        let output = include_str!("../../../tests/fixtures/pytest_durations_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 1 passed, 1 failed in 0.05s

Failures:
1. [FAIL] test_slow
     test_du.py:6: in test_slow
     E   AssertionError: slow fail
     E   assert 1 == 2"#
        );
    }

    #[test]
    fn test_filter_pytest_tb_native_fills_context_lines() {
        let output = include_str!("../../../tests/fixtures/pytest_tb_native_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 2 failed, 1 error in 0.01s

Failures:
1. [ERROR] at setup of test_setup
     raise ConnectionError('db down')
     ConnectionError: db down

2. [FAIL] test_a
     assert 1 + 1 == 3
     AssertionError: assert (1 + 1) == 3

3. [FAIL] test_b
     assert 'a' == 'b', 'inner mismatch'
     AssertionError: inner mismatch
     assert 'a' == 'b'"#
        );
    }

    #[test]
    fn test_filter_pytest_single_e_raise_keeps_the_source_line() {
        let output = include_str!("../../../tests/fixtures/pytest_single_e_raise_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.01s

Failures:
1. [FAIL] test_raise
     test_r.py:2: in test_raise
     raise ValueError("boom")
     E   ValueError: boom"#
        );
    }

    #[test]
    fn test_filter_pytest_indented_e_source_is_not_an_error_line() {
        let output =
            include_str!("../../../tests/fixtures/pytest_tb_long_indented_e_source_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.01s

Failures:
1. [FAIL] test_q
     E       AssertionError: list drifted
     E       assert [1, 2] == [3]
     … +1 more E line
     E         Left contains one more item: 2
     E         Use -v to get more diff
     test_ep.py:4: AssertionError"#
        );
    }

    #[test]
    fn test_filter_pytest_error_line_is_never_the_location() {
        let output = include_str!("../../../tests/fixtures/pytest_helper_module_lint_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.01s

Failures:
1. [FAIL] test_l
     test_hl.py:5: in test_l
     mylib/lint.py:3: in check
     E   AssertionError: ['foo.py:3: E501 line too long']"#
        );
    }

    #[test]
    fn test_filter_pytest_setup_error_keeps_the_fixture_frame() {
        let output = include_str!("../../../tests/fixtures/pytest_conftest_setup_error_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 0 failed, 1 error in 0.01s

Failures:
1. [ERROR] at setup of test_uses_db
     conftest.py:7: in db
     mylib/db.py:2: in connect
     E   ConnectionError: db down
     E   retry in 5s
     E   host: db.internal"#
        );
    }

    #[test]
    fn test_filter_pytest_tb_native_ignores_captured_log_frames() {
        let output = include_str!("../../../tests/fixtures/pytest_tb_native_captured_log_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.01s

Failures:
1. [FAIL] test_logged
     assert 0, "native failure"
     AssertionError: native failure
     assert 0"#
        );
    }

    #[test]
    fn test_filter_pytest_tb_line_compares_frame_files() {
        let output = include_str!("../../../tests/fixtures/pytest_tb_line_two_locations_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 2 failed in 0.01s

Failures:
     E   assert 1 == 2
     E   AssertionError: assert ['foo.py:3: E...ine too long'] == []
     /home/user/project/test_tl.py:10: AssertionError: assert ['foo.py:3: E...ine too long'] == []"#
        );
    }

    #[test]
    fn test_filter_pytest_shortened_header_starts_a_block() {
        let output = include_str!("../../../tests/fixtures/pytest_long_param_id_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 2 failed in 0.01s

Failures:
1. [FAIL] test_first
     test_lm.py:5: in test_first
     E   AssertionError: first test message
     E   assert {'a': 1, 'b': 2} == {'a': 1, 'b': 3}
     … +2 more E lines
     E     {'b': 2} != {'b': 3}
     E     Use -v to get more diff

2. [FAIL] test_second_with_a_long_parametrized_id[xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx]
     test_lm.py:10: in test_second_with_a_long_parametrized_id
     E   RuntimeError: second test message
     E   second detail one
     E   second detail two"#
        );
    }

    #[test]
    fn test_filter_pytest_source_line_is_never_a_frame() {
        let output =
            include_str!("../../../tests/fixtures/pytest_source_line_with_location_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.01s

Failures:
1. [FAIL] test_b
     test_sl.py:5: in test_b
     helpers.py:2: in boom
     E   RuntimeError: see app.py:12 for details
     E   second line
     E   third line"#
        );
    }

    #[test]
    fn test_filter_pytest_tb_long_source_and_args_are_never_frames() {
        let output = include_str!(
            "../../../tests/fixtures/pytest_tb_long_source_line_with_location_raw.txt"
        );

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.01s

Failures:
1. [FAIL] test_load
     test_sr.py:6:
     E       ValueError: bad config
     E       line two
     E       line three
     mylib/conf.py:2: ValueError"#
        );
    }

    #[test]
    fn test_filter_pytest_locals_are_never_frames() {
        let output =
            include_str!("../../../tests/fixtures/pytest_tb_long_locals_with_location_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.01s

Failures:
1. [FAIL] test_load
     test_sr.py:6:
     E       ValueError: bad config
     E       line two
     E       line three
     mylib/conf.py:2: ValueError"#
        );
    }

    #[test]
    fn test_filter_pytest_test_named_collecting_keeps_the_test_frame() {
        let output = include_str!("../../../tests/fixtures/pytest_test_named_collecting_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.01s

Failures:
1. [FAIL] test_collecting_metrics
     test_metrics.py:5: in test_collecting_metrics
     mylib/metrics.py:3: in collect
     E   ValueError: no samples
     E   source: metrics
     E   window: 5m"#
        );
    }

    #[test]
    fn test_filter_pytest_source_comment_is_never_a_frame() {
        let output = include_str!("../../../tests/fixtures/pytest_tb_short_source_comment_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.01s

Failures:
1. [FAIL] test_comment
     test_cm.py:10: in test_comment
     mylib/conf.py:2: in fail
     E   ValueError: bad config
     E   line two
     E   line three"#
        );
    }

    #[test]
    fn test_filter_pytest_tb_long_source_comment_is_never_a_frame() {
        let output = include_str!("../../../tests/fixtures/pytest_tb_long_source_comment_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.01s

Failures:
1. [FAIL] test_comment
     test_cm.py:10:
     E       ValueError: bad config
     E       line two
     E       line three
     mylib/conf.py:2: ValueError"#
        );
    }

    #[test]
    fn test_filter_pytest_captured_underscore_lines_are_not_headers() {
        let output =
            include_str!("../../../tests/fixtures/pytest_captured_underscore_lines_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 2 failed in 0.01s

Failures:
1. [FAIL] test_dunder
     test_du5.py:7: in test_dunder
     E   AssertionError: dunder fail
     E   assert {'a': 1} == {'a': 2}
     … +1 more E line
     E     {'a': 1} != {'a': 2}
     E     Use -v to get more diff

2. [FAIL] test_after
     test_du5.py:11: in test_after
     E   AssertionError: after dunder
     E   assert 0"#
        );
    }

    #[test]
    fn test_filter_pytest_locals_repr_is_never_a_frame() {
        let output =
            include_str!("../../../tests/fixtures/pytest_tb_long_locals_frame_repr_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.01s

Failures:
1. [FAIL] test_loc
     E       ValueError: helper boom
     E       l2
     E       l3
     test_dlo.py:7: ValueError"#
        );
    }

    #[test]
    fn test_filter_pytest_dir_with_spaces_keeps_location() {
        let output = include_str!("../../../tests/fixtures/pytest_dir_with_spaces_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 1 failed in 0.01s

Failures:
1. [FAIL] test_sp
     bob's dir/test_sp.py:2: in test_sp
     E   AssertionError: spaced dir
     E   assert {'a': 1} == {'a': 2}
     … +1 more E line
     E     {'a': 1} != {'a': 2}
     E     Use -v to get more diff"#
        );
    }

    #[test]
    fn test_filter_pytest_tb_line_closes_the_error_block() {
        let output = include_str!("../../../tests/fixtures/pytest_tb_line_setup_error_raw.txt");

        assert_eq!(
            filter_pytest_output(output),
            r#"Pytest: 0 passed, 2 failed, 1 error in 0.01s

Failures:
1. [ERROR] at setup of test_setup_err
     E   ConnectionError: db down

     E   assert (1 + 1) == 3
     E   AssertionError: inner mismatch
     /home/user/project/test_mixed.py:18: AssertionError: inner mismatch"#
        );
    }

    #[test]
    fn test_filter_pytest_error_short_summary_line() {
        // With `-rE`/`-ra` pytest also prints an `ERROR <nodeid> - <msg>` line.
        let output = r#"=== short test summary info ===
ERROR test_e.py::test_e - RuntimeError: fixture
=== 1 passed, 1 error in 0.02s ==="#;

        let result = filter_pytest_output(output);
        assert!(
            result.contains("[ERROR] test_e.py::test_e"),
            "got: {result}"
        );
        assert!(result.contains("RuntimeError: fixture"), "got: {result}");
    }

    #[test]
    fn test_parse_summary_line_errors() {
        let c = parse_summary_line("1 passed, 1 error in 0.02s");
        assert_eq!((c.passed, c.failed, c.errors), (1, 0, 1));

        let c = parse_summary_line("=== 1 failed, 3 passed, 2 errors in 0.05s ===");
        assert_eq!((c.passed, c.failed, c.errors), (3, 1, 2));
    }
}
