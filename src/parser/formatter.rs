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
        let mut lines = vec![format!("PASS ({}) FAIL ({})", self.passed, self.failed)];

        if !self.failures.is_empty() {
            lines.push(String::new());
            for (idx, failure) in self.failures.iter().enumerate().take(5) {
                lines.push(format!("{}. {}", idx + 1, failure.test_name));
                for line in failure.error_message.lines() {
                    lines.push(format!("   {}", line));
                }
            }

            if self.failures.len() > 5 {
                lines.push(format!("\n... +{} more failures", self.failures.len() - 5));
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
}
