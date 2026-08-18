//! Never-worse output guard: RTK never emits more tokens than the raw command,
//! and never renders an all-green summary when the child process exited non-zero.
//!
//! The exit guard uses a failure DENYLIST: on a non-zero child exit a filter
//! result is kept only when it already communicates failure, otherwise it is
//! replaced with a standardized `<tool>: failed (exit N)` verdict. This inverts
//! the old green-phrase allowlist, which let any reworded summary or new filter
//! silently bypass the guard by default.

use crate::core::tracking::estimate_tokens;
use regex::Regex;
use std::sync::LazyLock;

/// "1 failed" / "44 passed, 1 failed" — a NON-ZERO failed count. "0 failed" is a
/// green verdict and never matches (the digit boundary prevents a standalone
/// "0" match inside "10 failed"/"20 failed", whose counts merely end in a zero).
static NONZERO_FAILED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[^0-9])[1-9]\d* failed").unwrap());

/// "3 errors" / "10 errors" — but NOT "0 errors" ("No errors found" is green).
static NONZERO_ERRORS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[^0-9])[1-9]\d* errors").unwrap());

/// "2 issues" / "golangci-lint: 5 issues in 3 files" — but NOT "No issues found".
static NONZERO_ISSUES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[^0-9])[1-9]\d* issues").unwrap());

static NONZERO_WARNINGS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[^0-9])[1-9]\d* warnings").unwrap());

static NONZERO_PROBLEMS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[^0-9])[1-9]\d* problems").unwrap());

/// Bare "FAIL"/"FAIL" — nextest `FAIL [...]`, pytest `[FAIL]`, vitest
/// `FAIL (2)`. Suppressed when the whole text only carries "FAIL (0)", the
/// green formatter verdict, via `FAIL_ZERO`.
static FAILED_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bfail\b").unwrap());

/// "FAIL (0)" — the green formatter verdict; suppresses `FAILED_WORD`.
static FAIL_ZERO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bfail\s*\(\s*0\s*\)").unwrap());

/// "Failures:" / "1 failure". Suppressed when a "Failures: 0"/"failures = 0"
/// count (green) is present, via `FAILURES_ZERO`.
static FAILURES_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bfailures?\b").unwrap());

/// "Failures: 0" / "failures = 0" — green counts that suppress `FAILURES_WORD`.
static FAILURES_ZERO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bfailures?\s*[:=]\s*0(?:\D|$)").unwrap());

/// Compiler error lines: "error TS2322", "Error:", "error[E0308]". The word
/// boundary never matches the plural "errors", so "No errors found" stays green.
static BARE_ERROR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\berror\b").unwrap());

/// Panics, exceptions, crashes (AssertionError, panicked, Process crashed).
static EXCEPTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:panic(?:ked)?|exception|assertionerror|crashed)\b").unwrap()
});

/// ruff format --check: files that need reformatting.
static NEED_FORMATTING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"needs? formatting|would (be )?reformat").unwrap());

/// True when `text` contains "failed" that is NOT part of a "0 failed" verdict
/// ("build failed", "1 failed", "10 failed", "test run failed"). The optional
/// leading count is captured so "0 failed" stays green while "10 failed" /
/// "20 failed" (counts merely ending in a zero) read as failures.
fn has_failed_word(text: &str) -> bool {
    static FAILED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?:(\d+)\s+)?failed\b").unwrap());
    FAILED
        .captures_iter(text)
        .any(|caps| caps.get(1).is_none_or(|m| m.as_str() != "0"))
}

/// Returns `filtered`, or `raw` when `filtered` would emit more tokens.
pub fn never_worse<'a>(raw: &'a str, filtered: &'a str) -> &'a str {
    if estimate_tokens(filtered) > estimate_tokens(raw) {
        raw
    } else {
        filtered
    }
}

/// Fallback rendering for an unparsed non-zero exit:
/// `"<tool>: failed (exit N)"` followed by a capped raw tail.
///
/// Mirrors `filter_go_build_with_exit`'s failure shape so every tool reports
/// hard failures identically instead of an all-green verdict.
pub fn failure_fallback(tool: &str, exit_code: i32, output: &str) -> String {
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();

    if lines.is_empty() {
        return format!("{}: failed (exit {})", tool, exit_code);
    }

    let mut result = format!("{}: failed (exit {})", tool, exit_code);
    result.push_str("\n═══════════════════════════════════════\n");

    const MAX_FAILURE_LINES: usize = crate::core::truncate::CAP_ERRORS;
    for (i, line) in lines.iter().take(MAX_FAILURE_LINES).enumerate() {
        result.push_str(&format!(
            "{}. {}\n",
            i + 1,
            crate::core::utils::truncate(line, 120)
        ));
    }

    if lines.len() > MAX_FAILURE_LINES {
        result.push_str(&format!(
            "\n… +{} more output lines\n",
            lines.len() - MAX_FAILURE_LINES
        ));
    }

    result.trim().to_string()
}

/// True when `text` already communicates failure.
///
/// IMPORTANT: this is a DENYLIST. The exit guard fires unless a non-zero-exit
/// filter result reads as a failure, so a new filter or a reworded green summary
/// can never silently bypass the guard — it defaults to the failure fallback.
/// Only phrases that genuinely communicate failure belong here; green verdicts
/// ("N passed", "No issues found", "All files formatted correctly") must stay
/// marker-free so the guard catches them. New failure shapes must be added here
/// and to the `looks_failed_detects_common_failure_shapes` test in the same
/// change.
fn looks_failed(text: &str) -> bool {
    let t = text.to_lowercase();
    NONZERO_FAILED.is_match(&t)
        || NONZERO_ERRORS.is_match(&t)
        || NONZERO_ISSUES.is_match(&t)
        || NONZERO_WARNINGS.is_match(&t)
        || NONZERO_PROBLEMS.is_match(&t)
        || (FAILED_WORD.is_match(&t) && !FAIL_ZERO.is_match(&t))
        || (FAILURES_WORD.is_match(&t) && !FAILURES_ZERO.is_match(&t))
        || BARE_ERROR.is_match(&t)
        || EXCEPTION.is_match(&t)
        || NEED_FORMATTING.is_match(&t)
        || has_failed_word(&t)
}

/// Enforce the exit-code invariant: a filter must NEVER render an all-green
/// summary when the child exited non-zero.
///
/// When `exit_code != 0` and `filtered` does not already communicate failure,
/// falls back to `failure_fallback`; otherwise `filtered` is returned unchanged.
pub fn guard_exit(raw: &str, exit_code: i32, tool: &str, filtered: &str) -> String {
    if exit_code == 0 || looks_failed(filtered) {
        return filtered.to_string();
    }
    failure_fallback(tool, exit_code, raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_filtered_when_smaller() {
        let raw = "a".repeat(400);
        assert_eq!(never_worse(&raw, "ok"), "ok");
    }

    #[test]
    fn falls_back_to_raw_when_filtered_bigger() {
        let raw = "{}";
        let filtered = "{\n  \"pretty\": true\n}";
        assert_eq!(never_worse(raw, filtered), raw);
    }

    #[test]
    fn tie_keeps_filtered() {
        assert_eq!(never_worse("abcd", "wxyz"), "wxyz");
    }

    #[test]
    fn token_boundary_follows_estimate_tokens() {
        assert_eq!(never_worse("abcd", "abcde"), "abcd");
        assert_eq!(never_worse("abcdefgh", "ijklmnop"), "ijklmnop");
    }

    #[test]
    fn empty_raw_returns_raw() {
        assert_eq!(never_worse("", "0 matches"), "");
    }

    #[test]
    fn empty_filtered_returns_filtered() {
        assert_eq!(never_worse("data", ""), "");
    }

    #[test]
    fn both_empty_returns_filtered() {
        assert_eq!(never_worse("", ""), "");
    }

    #[test]
    fn guard_keeps_filtered_on_exit_zero() {
        assert_eq!(
            guard_exit("raw", 0, "pytest", "Pytest: 5 passed"),
            "Pytest: 5 passed"
        );
    }

    #[test]
    fn guard_keeps_failure_summary_on_nonzero_exit() {
        let filtered = "Pytest: 4 passed, 1 failed\n\nFailures:\n1. [FAIL] test_x";
        assert_eq!(guard_exit("raw", 1, "pytest", filtered), filtered);
    }

    #[test]
    fn guard_falls_back_on_green_summary_with_nonzero_exit() {
        let result = guard_exit("opaque failure output", 1, "pytest", "Pytest: 5 passed");
        assert_eq!(result, "pytest: failed (exit 1)\n═══════════════════════════════════════\n1. opaque failure output");
    }

    #[test]
    fn guard_falls_back_on_clean_verdict_with_nonzero_exit() {
        let result = guard_exit(
            "config broken",
            2,
            "ruff",
            "Ruff format: All files formatted correctly",
        );
        assert!(result.contains("ruff: failed (exit 2)"), "got: {}", result);
        assert!(result.contains("config broken"), "got: {}", result);
    }

    #[test]
    fn guard_detects_formatter_pass_fail_zero_as_green() {
        let result = guard_exit("killed", 137, "vitest", "PASS (13) FAIL (0)");
        assert!(
            result.contains("vitest: failed (exit 137)"),
            "got: {}",
            result
        );
    }

    #[test]
    fn guard_keeps_formatter_pass_fail_nonzero_count() {
        let filtered = "PASS (13) FAIL (2)\n1. a_test\n   boom";
        assert_eq!(guard_exit("raw", 1, "vitest", filtered), filtered);
    }

    #[test]
    fn guard_keeps_counts_ending_in_zero() {
        let filtered = "Lint: 10 errors, 0 warnings\n1. bad line";
        assert_eq!(guard_exit("raw", 1, "lint", filtered), filtered);
    }

    #[test]
    fn zero_failed_counts_are_not_failures() {
        // "0 failed" is a green verdict: it must NOT be flagged as failure,
        // while "10 failed"/"20 failed" (counts merely ending in a zero) are.
        assert!(!looks_failed("0 failed"));
        assert!(!looks_failed("Pytest: 44 passed, 0 failed, 3 skipped"));
        assert!(!looks_failed("PASS (13) FAIL (0)"));
        assert!(!looks_failed("Failures: 0, Errors: 0"));
        assert!(looks_failed("1 failed"));
        assert!(looks_failed("4 passed, 1 failed"));
        assert!(looks_failed("10 failed"));
        assert!(looks_failed("20 failed"));
        assert!(looks_failed("build failed"));
        assert!(looks_failed("error: test run failed"));
    }

    #[test]
    fn looks_failed_detects_common_failure_shapes() {
        // The denylist must recognize every failure shape the guard-wired
        // filters produce so their non-zero-exit output is preserved.
        assert!(looks_failed(
            "Pytest: 4 passed, 1 failed\nFailures:\n1. [FAIL] test_x"
        ));
        assert!(looks_failed("PASS (13) FAIL (2)\n1. a_test\n   boom"));
        assert!(looks_failed("Lint: 10 errors, 0 warnings\n1. bad line"));
        assert!(looks_failed(
            "Ruff: 3 issues in 2 files\nViolations:\n  1:8 E501 too long"
        ));
        assert!(looks_failed(
            "ESLint: 2 errors, 1 warnings in 2 files\nTop rules:"
        ));
        assert!(looks_failed(
            "golangci-lint: 5 issues in 3 files\nTop linters:"
        ));
        assert!(looks_failed("Pylint: 4 issues in 2 files"));
        assert!(looks_failed(
            "Go vet: 2 issues\n1. main.go:42:2: Printf format %d"
        ));
        assert!(looks_failed(
            "TypeScript: 2 errors in 0 files\n1. error TS5083"
        ));
        assert!(looks_failed(
            "mypy: 15 errors in 15 files\nsrc/file1.py:1: error: bad"
        ));
        assert!(looks_failed(
            "Ruff format: 2 files need formatting\n1. main.py"
        ));
        assert!(looks_failed(
            "Go test: 1 passed, 1 failed\n\nFailures:\n1. TestFail"
        ));
        assert!(looks_failed("cargo nextest: 2 passed, 2 failed, 1 skipped"));
        assert!(looks_failed(
            "testBar FAILED\n    java.lang.AssertionError: expected true"
        ));
        assert!(looks_failed(
            "FAIL [   0.006s] (2/4) test-proj tests::failing_test"
        ));
        assert!(looks_failed("BUILD FAILED in 12s"));
        assert!(looks_failed(
            "connectedAndroidTest failed: No connected devices!"
        ));
    }

    #[test]
    fn looks_failed_stays_quiet_on_green_verdicts() {
        // Green verdicts carry no failure marker and must fall through to the
        // failure fallback on a non-zero exit (asserted in
        // guard_catches_known_green_verdicts). This is the denylist contract:
        // if a filter rewords a summary, the guard still fires by default.
        assert!(!looks_failed("Pytest: 5 passed"));
        assert!(!looks_failed("Ruff: No issues found"));
        assert!(!looks_failed("Ruff: No errors found"));
        assert!(!looks_failed("ESLint: No issues found"));
        assert!(!looks_failed("Pylint: No issues found"));
        assert!(!looks_failed("golangci-lint: No issues found"));
        assert!(!looks_failed("Go vet: No issues found"));
        assert!(!looks_failed("mypy: No issues found"));
        assert!(!looks_failed("TypeScript: No errors found"));
        assert!(!looks_failed("Go test: 5 passed in 1 packages"));
        assert!(!looks_failed(
            "cargo nextest: 301 passed (1 binary, 0.192s)"
        ));
        assert!(!looks_failed("Ruff format: All files formatted correctly"));
        assert!(!looks_failed("ok ✓ (connected tests passed)"));
        assert!(!looks_failed("Prettier: All files formatted correctly"));
        assert!(!looks_failed(""));
    }

    #[test]
    fn guard_catches_reworded_green_summaries_by_default() {
        // The denylist fires for ANY summary that does not communicate failure —
        // even reworded ones the allowlist never saw.
        let reworded = [
            "All good — 5 suites green",
            "Everything passed cleanly",
            "No problems detected across 12 files",
            "Compilation completed without complaints",
            "ok ✓ (connected tests passed)",
        ];
        for verdict in reworded {
            let result = guard_exit("opaque failure", 1, "tool", verdict);
            assert!(
                result.contains("tool: failed (exit 1)"),
                "guard missed reworded green verdict {verdict:?}: got {}",
                result
            );
        }
    }

    #[test]
    fn guard_detects_zero_failed_with_skips_as_green() {
        // "Pytest: 44 passed, 0 failed, 3 skipped" is a GREEN verdict — the guard
        // must fall back on non-zero exit instead of rendering it.
        let result = guard_exit(
            "opaque failure output",
            1,
            "pytest",
            "Pytest: 44 passed, 0 failed, 3 skipped",
        );
        assert!(
            result.contains("pytest: failed (exit 1)"),
            "expected failure fallback, got: {}",
            result
        );
    }

    #[test]
    fn guard_keeps_nonzero_failed_with_skips() {
        let filtered = "Pytest: 44 passed, 1 failed, 3 skipped\n\nFailures:\n1. [FAIL] test_x";
        assert_eq!(guard_exit("raw", 1, "pytest", filtered), filtered);
    }

    #[test]
    fn guard_catches_known_green_verdicts() {
        // Every green verdict string a filter can currently produce must be caught
        // by the guard. Unlike the old allowlist, no new green string can bypass
        // the guard — it fires unless the text communicates failure (see
        // `guard_catches_reworded_green_summaries_by_default`). This list guards
        // the denylist markers themselves from accidentally matching green text.
        let green_verdicts = [
            "Pytest: 5 passed",
            "Pytest: 44 passed, 0 failed, 3 skipped",
            "Ruff: No issues found",
            "Ruff format: All files formatted correctly",
            "PASS (13) FAIL (0)",
            "ESLint: No issues found",
            "Prettier: All files formatted correctly",
        ];
        for verdict in green_verdicts {
            let result = guard_exit("opaque failure", 1, "tool", verdict);
            assert!(
                result.contains("tool: failed (exit 1)"),
                "guard missed green verdict {verdict:?}: got {}",
                result
            );
        }
    }

    #[test]
    fn failure_fallback_empty_output() {
        assert_eq!(
            failure_fallback("go build", 101, ""),
            "go build: failed (exit 101)"
        );
    }

    #[test]
    fn failure_fallback_includes_raw_tail() {
        let result = failure_fallback("tsc", 2, "error TS5083: cannot read file\nline two");
        assert!(result.contains("tsc: failed (exit 2)"));
        assert!(result.contains("1. error TS5083: cannot read file"));
        assert!(result.contains("2. line two"));
    }

    #[test]
    fn failure_fallback_caps_tail() {
        let lines: Vec<String> = (0..40).map(|i| format!("line {i}")).collect();
        let result = failure_fallback("tool", 1, &lines.join("\n"));
        assert!(
            result.contains("… +20 more output lines"),
            "got: {}",
            result
        );
    }
}
