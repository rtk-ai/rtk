/// Token-efficient formatting trait for canonical types
use super::types::*;

/// Output formatting modes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatMode {
    /// Ultra-compact: Summary only (default)
    Compact,
    /// Verbose: Include details
    Verbose,
    /// Ultra-compressed: Symbols and abbreviations
    Ultra,
}

impl FormatMode {
    pub fn from_verbosity(verbosity: u8) -> Self {
        match verbosity {
            0 => FormatMode::Compact,
            1 => FormatMode::Verbose,
            _ => FormatMode::Ultra,
        }
    }
}

/// Trait for formatting canonical types into token-efficient strings
pub trait TokenFormatter {
    /// Format as compact summary (default)
    fn format_compact(&self) -> String;

    /// Format with details (verbose mode)
    fn format_verbose(&self) -> String;

    /// Format with symbols (ultra-compressed mode)
    fn format_ultra(&self) -> String;

    /// Format according to mode
    fn format(&self, mode: FormatMode) -> String {
        match mode {
            FormatMode::Compact => self.format_compact(),
            FormatMode::Verbose => self.format_verbose(),
            FormatMode::Ultra => self.format_ultra(),
        }
    }
}

impl TokenFormatter for TestResult {
    fn format_compact(&self) -> String {
        // Top-N failures keep their full error message (numbered, indented). Anything
        // beyond N collapses to a one-liner showing the test name and, when known,
        // the source file — the original `take(5) + "+N more failures"` truncation
        // hid every remaining failure name from the agent, which forced a re-read of
        // the tee log to find the failing files (see rtk-ai/rtk#1813).
        const DETAILED_FAILURES: usize = 5;

        let mut lines = vec![format!("PASS ({}) FAIL ({})", self.passed, self.failed)];

        if !self.failures.is_empty() {
            lines.push(String::new());
            for (idx, failure) in self.failures.iter().enumerate().take(DETAILED_FAILURES) {
                lines.push(format!("{}. {}", idx + 1, failure.test_name));
                for line in failure.error_message.lines() {
                    lines.push(format!("   {}", line));
                }
            }

            if self.failures.len() > DETAILED_FAILURES {
                lines.push(String::new());
                lines.push(format!(
                    "Remaining {} failures:",
                    self.failures.len() - DETAILED_FAILURES,
                ));
                for (idx, failure) in self.failures.iter().enumerate().skip(DETAILED_FAILURES) {
                    // `file_path` is set on the JSON happy-path (vitest/playwright Tier 1)
                    // but empty in regex-fallback Tier 2 — emit the bracket suffix only
                    // when populated so the line does not render as `1. name []`.
                    let path_hint = if failure.file_path.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", failure.file_path)
                    };
                    lines.push(format!("{}. {}{}", idx + 1, failure.test_name, path_hint,));
                }
            }
        }

        if let Some(duration) = self.duration_ms {
            lines.push(format!("\nTime: {}ms", duration));
        }

        lines.join("\n")
    }

    fn format_verbose(&self) -> String {
        let mut lines = vec![format!(
            "Tests: {} passed, {} failed, {} skipped (total: {})",
            self.passed, self.failed, self.skipped, self.total
        )];

        if !self.failures.is_empty() {
            lines.push("\nFailures:".to_string());
            for (idx, failure) in self.failures.iter().enumerate() {
                lines.push(format!(
                    "\n{}. {} ({})",
                    idx + 1,
                    failure.test_name,
                    failure.file_path
                ));
                lines.push(format!("   {}", failure.error_message));
                if let Some(stack) = &failure.stack_trace {
                    let stack_preview: String =
                        stack.lines().take(3).collect::<Vec<_>>().join("\n   ");
                    lines.push(format!("   {}", stack_preview));
                }
            }
        }

        if let Some(duration) = self.duration_ms {
            lines.push(format!("\nDuration: {}ms", duration));
        }

        lines.join("\n")
    }

    fn format_ultra(&self) -> String {
        format!(
            "[ok]{} [x]{} [skip]{} ({}ms)",
            self.passed,
            self.failed,
            self.skipped,
            self.duration_ms.unwrap_or(0)
        )
    }
}

impl TokenFormatter for DependencyState {
    fn format_compact(&self) -> String {
        if self.outdated_count == 0 {
            return "All packages up-to-date".to_string();
        }

        let mut lines = vec![format!(
            "{} outdated packages (of {})",
            self.outdated_count, self.total_packages
        )];

        for dep in self.dependencies.iter().take(10) {
            if let Some(latest) = &dep.latest_version {
                if &dep.current_version != latest {
                    lines.push(format!(
                        "{}: {} → {}",
                        dep.name, dep.current_version, latest
                    ));
                }
            }
        }

        if self.outdated_count > 10 {
            lines.push(format!("\n... +{} more", self.outdated_count - 10));
        }

        lines.join("\n")
    }

    fn format_verbose(&self) -> String {
        let mut lines = vec![format!(
            "Total packages: {} ({} outdated)",
            self.total_packages, self.outdated_count
        )];

        if self.outdated_count > 0 {
            lines.push("\nOutdated packages:".to_string());
            for dep in &self.dependencies {
                if let Some(latest) = &dep.latest_version {
                    if &dep.current_version != latest {
                        let dev_marker = if dep.dev_dependency { " (dev)" } else { "" };
                        lines.push(format!(
                            "  {}: {} → {}{}",
                            dep.name, dep.current_version, latest, dev_marker
                        ));
                        if let Some(wanted) = &dep.wanted_version {
                            if wanted != latest {
                                lines.push(format!("    (wanted: {})", wanted));
                            }
                        }
                    }
                }
            }
        }

        lines.join("\n")
    }

    fn format_ultra(&self) -> String {
        format!("pkg:{} ^{}", self.total_packages, self.outdated_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::types::{TestFailure, TestResult};

    fn make_failure(name: &str, error: &str) -> TestFailure {
        TestFailure {
            test_name: name.to_string(),
            file_path: "tests/e2e.spec.ts".to_string(),
            error_message: error.to_string(),
            stack_trace: None,
        }
    }

    fn make_result(passed: usize, failures: Vec<TestFailure>) -> TestResult {
        TestResult {
            total: passed + failures.len(),
            passed,
            failed: failures.len(),
            skipped: 0,
            duration_ms: Some(1500),
            failures,
        }
    }

    // RED: format_compact must show the full error message, not just 2 lines.
    // Playwright errors contain the expected/received diff and call log starting
    // at line 3+. Truncating to 2 lines leaves the agent with no debug info.
    #[test]
    fn test_compact_shows_full_error_message() {
        let error = "Error: expect(locator).toHaveText(expected)\n\nExpected: 'Submit'\nReceived: 'Loading'\n\nCall log:\n  - waiting for getByRole('button', { name: 'Submit' })";
        let result = make_result(5, vec![make_failure("should click submit", error)]);

        let output = result.format_compact();

        assert!(
            output.contains("Expected: 'Submit'"),
            "format_compact must preserve expected/received diff\nGot:\n{output}"
        );
        assert!(
            output.contains("Received: 'Loading'"),
            "format_compact must preserve received value\nGot:\n{output}"
        );
        assert!(
            output.contains("Call log:"),
            "format_compact must preserve call log\nGot:\n{output}"
        );
    }

    // RED: summary line stays compact regardless of failure detail
    #[test]
    fn test_compact_summary_line_is_concise() {
        let result = make_result(28, vec![make_failure("test", "some error")]);
        let output = result.format_compact();
        let first_line = output.lines().next().unwrap_or("");
        assert!(
            first_line.contains("28") && first_line.contains("1"),
            "First line must show pass/fail counts, got: {first_line}"
        );
    }

    // RED: all-pass output stays compact (no failure detail bloat)
    #[test]
    fn test_compact_all_pass_is_one_line() {
        let result = make_result(10, vec![]);
        let output = result.format_compact();
        assert!(
            output.lines().count() <= 3,
            "All-pass output should be compact, got {} lines:\n{output}",
            output.lines().count()
        );
    }

    // RED: error_message with only 1 line still works (no trailing noise)
    #[test]
    fn test_compact_single_line_error_no_trailing_noise() {
        let result = make_result(0, vec![make_failure("should work", "Timeout exceeded")]);
        let output = result.format_compact();
        assert!(
            output.contains("Timeout exceeded"),
            "Single-line error must appear\nGot:\n{output}"
        );
    }

    // Regression for rtk-ai/rtk#1813: when a test run has more than 5 failures the
    // first 5 keep their full error detail, and every remaining failure must still
    // be visible by name (plus file path when known) so the agent can locate every
    // failing file without re-reading the tee log.
    #[test]
    fn test_compact_lists_remaining_failures_with_file_path() {
        let failures: Vec<TestFailure> = (1..=7)
            .map(|i| TestFailure {
                test_name: format!("test number {i}"),
                file_path: format!("tests/spec_{i}.test.ts"),
                error_message: format!("AssertionError: test {i} failed"),
                stack_trace: None,
            })
            .collect();
        let result = make_result(40, failures);

        let output = result.format_compact();

        // Top 5 keep full detail (numbering + error message line).
        for i in 1..=5 {
            assert!(
                output.contains(&format!("{i}. test number {i}")),
                "detailed entry {i} missing\nGot:\n{output}"
            );
            assert!(
                output.contains(&format!("AssertionError: test {i} failed")),
                "error for detailed entry {i} missing\nGot:\n{output}"
            );
        }
        // Remaining failures must be listed by name with file_path bracketed.
        assert!(
            output.contains("Remaining 2 failures:"),
            "overflow section missing\nGot:\n{output}"
        );
        for i in 6..=7 {
            assert!(
                output.contains(&format!("{i}. test number {i} [tests/spec_{i}.test.ts]")),
                "overflow entry {i} missing or malformed\nGot:\n{output}"
            );
        }
        // The legacy "+N more failures" line must not appear — it was the bug.
        assert!(
            !output.contains("more failures"),
            "legacy '+N more failures' truncation leaked\nGot:\n{output}"
        );
    }

    // Tier-2 (regex fallback) leaves file_path empty for every failure; the
    // overflow line must render as `N. test_name` with no dangling `[]` suffix.
    #[test]
    fn test_compact_lists_remaining_failures_without_file_path() {
        let failures: Vec<TestFailure> = (1..=8)
            .map(|i| TestFailure {
                test_name: format!("orphan test {i}"),
                file_path: String::new(),
                error_message: format!("err {i}"),
                stack_trace: None,
            })
            .collect();
        let result = make_result(0, failures);

        let output = result.format_compact();

        // Overflow entries omit the bracket when file_path is empty.
        for i in 6..=8 {
            assert!(
                output.contains(&format!("{i}. orphan test {i}\n"))
                    || output.ends_with(&format!("{i}. orphan test {i}"))
                    || output.contains(&format!("{i}. orphan test {i}\nTime:")),
                "overflow entry {i} should be 'N. name' with no trailing brackets\nGot:\n{output}"
            );
            assert!(
                !output.contains(&format!("{i}. orphan test {i} []")),
                "empty file_path must not render as '[]'\nGot:\n{output}"
            );
        }
    }

    // Boundary: exactly 5 failures must not emit the "Remaining" section.
    #[test]
    fn test_compact_no_remaining_section_at_five_failure_boundary() {
        let failures: Vec<TestFailure> = (1..=5)
            .map(|i| make_failure(&format!("test {i}"), &format!("err {i}")))
            .collect();
        let result = make_result(10, failures);

        let output = result.format_compact();

        assert!(
            !output.contains("Remaining"),
            "no overflow section expected at boundary of 5\nGot:\n{output}"
        );
        assert!(
            !output.contains("more failures"),
            "legacy overflow line must not appear at boundary\nGot:\n{output}"
        );
    }

    // The core promise of rtk-ai/rtk#1813: every failure name must be visible
    // in the compact output regardless of count. With 49 failures the first 5
    // stay detailed and the other 44 appear as one-liners with their file paths.
    #[test]
    fn test_compact_keeps_every_failure_name_visible_on_large_set() {
        let failures: Vec<TestFailure> = (1..=49)
            .map(|i| TestFailure {
                test_name: format!(
                    "src/agents/agent-validation.test.ts > agent rejects malformed payload variant {i}"
                ),
                file_path: format!("src/agents/agent-validation-{i}.test.ts"),
                error_message: format!("AssertionError: expected 403 to be 201 (case {i})"),
                stack_trace: None,
            })
            .collect();
        let result = make_result(664, failures);

        let compact = result.format_compact();

        // Header + overflow announcement are present.
        assert!(compact.starts_with("PASS (664) FAIL (49)"));
        assert!(compact.contains("Remaining 44 failures:"));

        // Every variant 1..=49 is named in the output.
        for i in 1..=49 {
            assert!(
                compact.contains(&format!("variant {i}")),
                "failure {i} must be visible in compact output (issue #1813)"
            );
        }
        // Overflow entries 6..=49 must carry their file_path bracket.
        for i in 6..=49 {
            assert!(
                compact.contains(&format!("[src/agents/agent-validation-{i}.test.ts]")),
                "overflow entry {i} should include its file_path bracket"
            );
        }
        // Compact still comes out structurally smaller than the verbose dump
        // even with every name preserved — the per-failure stack-trace preview
        // verbose adds keeps it the larger of the two modes.
        let verbose = result.format_verbose();
        assert!(
            compact.len() < verbose.len(),
            "compact ({} chars) must stay smaller than verbose ({} chars)",
            compact.len(),
            verbose.len(),
        );
    }
}
