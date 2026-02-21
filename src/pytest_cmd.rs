use crate::stream::{FilterMode, StdinMode, StreamFilter};
use crate::tracking;
use crate::utils::truncate;
use anyhow::{Context, Result};
use std::process::Command;

#[derive(Debug, PartialEq, Default)]
enum ParseState {
    #[default]
    Header,
    TestProgress,
    Failures,
    Summary,
}

/// Progressive streaming filter for `pytest` output.
///
/// Replicates the `filter_pytest_output` state machine line-by-line.
/// Defers all output to `flush()` so the summary section is always included.
#[derive(Default)]
pub struct PyTestStreamFilter {
    state: ParseState,
    test_files: Vec<String>,
    failures: Vec<String>,
    current_failure: Vec<String>,
    summary_line: String,
}

impl PyTestStreamFilter {
    pub fn new() -> Self {
        Self::default()
    }
}

impl StreamFilter for PyTestStreamFilter {
    fn feed_line(&mut self, line: &str) -> Option<String> {
        let trimmed = line.trim();

        // State transitions (same as filter_pytest_output loop body)
        if trimmed.starts_with("===") && trimmed.contains("test session starts") {
            self.state = ParseState::Header;
            return None;
        } else if trimmed.starts_with("===") && trimmed.contains("FAILURES") {
            self.state = ParseState::Failures;
            return None;
        } else if trimmed.starts_with("===") && trimmed.contains("short test summary") {
            self.state = ParseState::Summary;
            if !self.current_failure.is_empty() {
                let block = self.current_failure.join("\n");
                self.failures.push(block);
                self.current_failure.clear();
            }
            return None;
        } else if trimmed.starts_with("===")
            && (trimmed.contains("passed") || trimmed.contains("failed"))
        {
            self.summary_line = trimmed.to_string();
            return None;
        }

        // Per-state processing
        match self.state {
            ParseState::Header => {
                if trimmed.starts_with("collected") {
                    self.state = ParseState::TestProgress;
                }
            }
            ParseState::TestProgress => {
                if !trimmed.is_empty()
                    && !trimmed.starts_with("===")
                    && (trimmed.contains(".py") || trimmed.contains("%]"))
                {
                    self.test_files.push(trimmed.to_string());
                }
            }
            ParseState::Failures => {
                if trimmed.starts_with("___") {
                    if !self.current_failure.is_empty() {
                        let block = self.current_failure.join("\n");
                        self.failures.push(block);
                        self.current_failure.clear();
                    }
                    self.current_failure.push(trimmed.to_string());
                } else if !trimmed.is_empty() && !trimmed.starts_with("===") {
                    self.current_failure.push(trimmed.to_string());
                }
            }
            ParseState::Summary => {
                if trimmed.starts_with("FAILED") || trimmed.starts_with("ERROR") {
                    self.failures.push(trimmed.to_string());
                }
            }
        }

        None
    }

    fn flush(&mut self) -> String {
        if !self.current_failure.is_empty() {
            let block = self.current_failure.join("\n");
            self.failures.push(block);
            self.current_failure.clear();
        }
        build_pytest_summary(&self.summary_line, &self.test_files, &self.failures)
    }
}

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    // Try to detect pytest command (could be "pytest", "python -m pytest", etc.)
    let mut cmd = if which_command("pytest").is_some() {
        Command::new("pytest")
    } else {
        // Fallback to python -m pytest
        let mut c = Command::new("python");
        c.arg("-m").arg("pytest");
        c
    };

    // Force short traceback and quiet mode for compact output
    let has_tb_flag = args.iter().any(|a| a.starts_with("--tb"));
    let has_quiet_flag = args.iter().any(|a| a == "-q" || a == "--quiet");

    if !has_tb_flag {
        cmd.arg("--tb=short");
    }
    if !has_quiet_flag {
        cmd.arg("-q");
    }

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: pytest --tb=short -q {}", args.join(" "));
    }

    let filter = PyTestStreamFilter::new();
    let result = crate::stream::run_streaming(
        &mut cmd,
        StdinMode::Inherit,
        FilterMode::Streaming(Box::new(filter)),
    )
    .context("Failed to run pytest. Is it installed? Try: pip install pytest")?;

    if let Some(hint) = crate::tee::tee_and_hint(&result.raw, "pytest", result.exit_code) {
        println!("{}\n{}", result.filtered, hint);
    } else {
        println!("{}", result.filtered);
    }

    timer.track(
        &format!("pytest {}", args.join(" ")),
        &format!("rtk pytest {}", args.join(" ")),
        &result.raw,
        &result.filtered,
    );

    if result.exit_code != 0 {
        std::process::exit(result.exit_code);
    }

    Ok(())
}

/// Check if a command exists in PATH
fn which_command(cmd: &str) -> Option<String> {
    Command::new("which")
        .arg(cmd)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Parse pytest output using state machine.
///
/// Buffered variant — for use when input is already fully accumulated (e.g.
/// `rtk pipe --filter pytest`). For live subprocess output, prefer
/// `PyTestStreamFilter` with `run_streaming`.
pub(crate) fn filter_pytest_output(output: &str) -> String {
    let mut filter = PyTestStreamFilter::new();
    for line in output.lines() {
        filter.feed_line(line);
    }
    filter.flush()
}

fn build_pytest_summary(summary: &str, _test_files: &[String], failures: &[String]) -> String {
    // Parse summary line
    let (passed, failed, skipped) = parse_summary_line(summary);

    if failed == 0 && passed > 0 {
        return format!("✓ Pytest: {} passed", passed);
    }

    if passed == 0 && failed == 0 {
        return "Pytest: No tests collected".to_string();
    }

    let mut result = String::new();
    result.push_str(&format!("Pytest: {} passed, {} failed", passed, failed));
    if skipped > 0 {
        result.push_str(&format!(", {} skipped", skipped));
    }
    result.push('\n');
    result.push_str("═══════════════════════════════════════\n");

    if failures.is_empty() {
        return result.trim().to_string();
    }

    // Show failures (limit to key information)
    result.push_str("\nFailures:\n");

    for (i, failure) in failures.iter().take(5).enumerate() {
        // Extract test name and key error info
        let lines: Vec<&str> = failure.lines().collect();

        // First line is usually test name (after ___)
        if let Some(first_line) = lines.first() {
            if first_line.starts_with("___") {
                // Extract test name between ___
                let test_name = first_line.trim_matches('_').trim();
                result.push_str(&format!("{}. ❌ {}\n", i + 1, test_name));
            } else if first_line.starts_with("FAILED") {
                // Summary format: "FAILED tests/test_foo.py::test_bar - AssertionError"
                let parts: Vec<&str> = first_line.split(" - ").collect();
                if let Some(test_path) = parts.first() {
                    let test_name = test_path.trim_start_matches("FAILED ");
                    result.push_str(&format!("{}. ❌ {}\n", i + 1, test_name));
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

    if failures.len() > 5 {
        result.push_str(&format!("\n... +{} more failures\n", failures.len() - 5));
    }

    result.trim().to_string()
}

fn parse_summary_line(summary: &str) -> (usize, usize, usize) {
    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;

    // Parse lines like "=== 4 passed, 1 failed in 0.50s ==="
    let parts: Vec<&str> = summary.split(',').collect();

    for part in parts {
        let words: Vec<&str> = part.split_whitespace().collect();
        for (i, word) in words.iter().enumerate() {
            if i > 0 {
                if word.contains("passed") {
                    if let Ok(n) = words[i - 1].parse::<usize>() {
                        passed = n;
                    }
                } else if word.contains("failed") {
                    if let Ok(n) = words[i - 1].parse::<usize>() {
                        failed = n;
                    }
                } else if word.contains("skipped") {
                    if let Ok(n) = words[i - 1].parse::<usize>() {
                        skipped = n;
                    }
                }
            }
        }
    }

    (passed, failed, skipped)
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
        assert!(result.contains("✓ Pytest"));
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
        assert_eq!(parse_summary_line("=== 5 passed in 0.50s ==="), (5, 0, 0));
        assert_eq!(
            parse_summary_line("=== 4 passed, 1 failed in 0.50s ==="),
            (4, 1, 0)
        );
        assert_eq!(
            parse_summary_line("=== 3 passed, 1 failed, 2 skipped in 1.0s ==="),
            (3, 1, 2)
        );
    }

    // ── PyTestStreamFilter tests ───────────────────────────────────────────────

    const PYTEST_ALL_PASS: &str = r#"=== test session starts ===
platform darwin -- Python 3.11.0
collected 5 items

tests/test_foo.py .....                                            [100%]

=== 5 passed in 0.50s ==="#;

    const PYTEST_WITH_FAILURE: &str = r#"=== test session starts ===
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

    #[test]
    fn test_pytest_stream_filter_feed_and_flush_all_pass() {
        let mut f = PyTestStreamFilter::new();
        for line in PYTEST_ALL_PASS.lines() {
            assert_eq!(
                f.feed_line(line),
                None,
                "streaming filter must defer output"
            );
        }
        let output = f.flush();
        assert!(output.contains("✓ Pytest"), "output={}", output);
        assert!(output.contains("5 passed"), "output={}", output);
    }

    #[test]
    fn test_pytest_stream_filter_feed_and_flush_with_failure() {
        let mut f = PyTestStreamFilter::new();
        for line in PYTEST_WITH_FAILURE.lines() {
            f.feed_line(line);
        }
        let output = f.flush();
        assert!(output.contains("4 passed, 1 failed"), "output={}", output);
        assert!(output.contains("test_something"), "output={}", output);
    }

    #[test]
    fn test_pytest_stream_filter_matches_buffered_all_pass() {
        let buffered = filter_pytest_output(PYTEST_ALL_PASS);
        let mut f = PyTestStreamFilter::new();
        for line in PYTEST_ALL_PASS.lines() {
            f.feed_line(line);
        }
        let streamed = f.flush();
        assert_eq!(streamed.trim(), buffered.trim());
    }

    #[test]
    fn test_pytest_stream_filter_matches_buffered_with_failures() {
        let buffered = filter_pytest_output(PYTEST_WITH_FAILURE);
        let mut f = PyTestStreamFilter::new();
        for line in PYTEST_WITH_FAILURE.lines() {
            f.feed_line(line);
        }
        let streamed = f.flush();
        assert_eq!(streamed.trim(), buffered.trim());
    }

    #[test]
    fn test_pytest_stream_filter_default_equals_new() {
        let mut f1 = PyTestStreamFilter::new();
        let mut f2 = PyTestStreamFilter::default();
        assert_eq!(f1.flush(), f2.flush());
    }
}
