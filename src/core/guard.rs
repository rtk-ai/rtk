//! Never-worse output guard: RTK never emits more tokens than the raw command,
//! and never renders an all-green summary when the child process exited non-zero.

use crate::core::tracking::estimate_tokens;

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

/// True when `text` is a compact "all-good" verdict that must never be rendered
/// for a non-zero child exit.
fn looks_green(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    if t.is_empty() {
        return false;
    }
    if t.ends_with("success") {
        return true;
    }
    if t.contains("no issues found")
        || t.contains("no errors found")
        || t.contains("no tests collected")
        || t.contains("no tests found")
        || t.contains("formatted correctly")
    {
        return true;
    }
    // "N passed" verdicts (incl. formatter output like "PASS (N) FAIL (0)").
    // A non-zero FAIL count or any "failed"/"errors" mention means it is NOT green.
    if t.contains("passed") || t.contains("pass (") {
        if t.contains("failed") || t.contains("errors") {
            return false;
        }
        return !t.contains("fail (") || t.contains("fail (0)");
    }
    if t.contains("compiled") || t.contains("already installed") {
        return true;
    }
    false
}

/// Enforce the exit-code invariant: a filter must NEVER render an all-green
/// summary when the child exited non-zero.
///
/// When `exit_code != 0` and `filtered` reads as a green verdict, falls back to
/// `failure_fallback`; otherwise `filtered` is returned unchanged.
pub fn guard_exit(raw: &str, exit_code: i32, tool: &str, filtered: &str) -> String {
    if exit_code == 0 || !looks_green(filtered) {
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
