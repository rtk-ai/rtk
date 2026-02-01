use anyhow::{Context, Result};
use regex::Regex;
use std::process::Command;
use crate::tracking;

#[derive(Debug, Clone)]
pub enum VitestCommand {
    Run,
}

pub fn run(cmd: VitestCommand, args: &[String], verbose: u8) -> Result<()> {
    match cmd {
        VitestCommand::Run => run_vitest(args, verbose),
    }
}

fn run_vitest(args: &[String], verbose: u8) -> Result<()> {
    let mut cmd = Command::new("pnpm");
    cmd.arg("vitest");
    cmd.arg("run"); // Force non-watch mode

    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run vitest")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Vitest returns non-zero exit code when tests fail
    // This is expected behavior for test runners
    let combined = format!("{}{}", stdout, stderr);
    let filtered = filter_vitest_output(&combined);

    if verbose > 0 {
        eprintln!("vitest run (filtered):");
    }

    println!("{}", filtered);

    tracking::track(
        "vitest run",
        "rtk vitest run",
        &combined,
        &filtered,
    );

    // Propagate original exit code
    std::process::exit(output.status.code().unwrap_or(1))
}

/// Strip ANSI escape sequences from terminal output
fn strip_ansi(text: &str) -> String {
    // Match ANSI escape sequences: \x1b[...m
    let ansi_regex = Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    ansi_regex.replace_all(text, "").to_string()
}

/// Extract test statistics from Vitest output
#[derive(Debug, Default)]
struct TestStats {
    pass: usize,
    fail: usize,
    total: usize,
    duration: String,
}

fn parse_test_stats(output: &str) -> TestStats {
    let mut stats = TestStats::default();

    // Strip ANSI first for easier parsing
    let clean_output = strip_ansi(output);

    // Pattern: "Test Files  X failed | Y passed | Z skipped (T)"
    // Or: "Test Files  Y passed (T)" when no failures
    if let Some(caps) = Regex::new(r"Test Files\s+(?:(\d+)\s+failed\s+\|\s+)?(\d+)\s+passed").unwrap().captures(&clean_output) {
        if let Some(fail_str) = caps.get(1) {
            stats.fail = fail_str.as_str().parse().unwrap_or(0);
        }
        if let Some(pass_str) = caps.get(2) {
            stats.pass = pass_str.as_str().parse().unwrap_or(0);
        }
    }

    // Pattern: "Tests  X failed | Y passed (T)"
    // Capture total passed count from Tests line
    if let Some(caps) = Regex::new(r"Tests\s+(?:\d+\s+failed\s+\|\s+)?(\d+)\s+passed").unwrap().captures(&clean_output) {
        if let Some(total_str) = caps.get(1) {
            stats.total = total_str.as_str().parse().unwrap_or(0);
        }
    }

    // Pattern: "Duration  3.05s" (with optional details in parens)
    if let Some(caps) = Regex::new(r"Duration\s+([\d.]+[ms]+)").unwrap().captures(&clean_output) {
        if let Some(duration_str) = caps.get(1) {
            stats.duration = duration_str.as_str().to_string();
        }
    }

    stats
}

/// Extract failure details from Vitest output
fn extract_failures(output: &str) -> Vec<String> {
    let mut failures = Vec::new();
    let clean_output = strip_ansi(output);

    // Look for FAIL markers and extract test names + error messages
    let lines: Vec<&str> = clean_output.lines().collect();
    let mut in_failure = false;
    let mut current_failure = String::new();

    for line in lines {
        // Start of failure block: "✗ test_name"
        if line.contains('✗') || line.contains("FAIL") {
            if !current_failure.is_empty() {
                failures.push(current_failure.trim().to_string());
            }
            current_failure = line.to_string();
            in_failure = true;
            continue;
        }

        // Collect error details (indented lines after ✗)
        if in_failure {
            if line.trim().is_empty() || line.starts_with(" Test Files") || line.starts_with(" Tests") {
                in_failure = false;
                if !current_failure.is_empty() {
                    failures.push(current_failure.trim().to_string());
                    current_failure.clear();
                }
            } else if line.starts_with("  ") {
                current_failure.push('\n');
                current_failure.push_str(line.trim());
            }
        }
    }

    // Push last failure if exists
    if !current_failure.is_empty() {
        failures.push(current_failure.trim().to_string());
    }

    failures
}

/// Filter Vitest output - show summary + failures only
fn filter_vitest_output(output: &str) -> String {
    let stats = parse_test_stats(output);
    let failures = extract_failures(output);

    let mut result = Vec::new();

    // Summary line
    if stats.total > 0 {
        result.push(format!("PASS ({}) FAIL ({})", stats.pass, stats.fail));
    }

    // Failure details
    if !failures.is_empty() {
        result.push(String::new()); // Blank line
        for (idx, failure) in failures.iter().enumerate() {
            result.push(format!("{}. {}", idx + 1, failure));
        }
    }

    // Timing
    if !stats.duration.is_empty() {
        result.push(String::new());
        result.push(format!("Time: {}", stats.duration));
    }

    // If parsing failed, return cleaned output (fallback)
    if result.len() <= 1 {
        return strip_ansi(output)
            .lines()
            .filter(|line| {
                // Keep only meaningful lines
                let trimmed = line.trim();
                !trimmed.is_empty()
                    && !trimmed.starts_with("│")
                    && !trimmed.starts_with("├")
                    && !trimmed.starts_with("└")
            })
            .collect::<Vec<_>>()
            .join("\n");
    }

    result.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi() {
        let input = "\x1b[32m✓\x1b[0m test passed";
        let output = strip_ansi(input);
        assert_eq!(output, "✓ test passed");
        assert!(!output.contains("\x1b"));
    }

    #[test]
    fn test_parse_test_stats_success() {
        let output = r#"
 ✓ src/auth.test.ts (5)
 ✓ src/utils.test.ts (8)

 Test Files  2 passed (2)
      Tests  13 passed (13)
   Duration  450ms
"#;
        let stats = parse_test_stats(output);
        assert_eq!(stats.pass, 2);
        assert_eq!(stats.fail, 0);
        assert_eq!(stats.total, 13);
        assert_eq!(stats.duration, "450ms");
    }

    #[test]
    fn test_parse_test_stats_failures() {
        let output = r#"
 ✓ src/auth.test.ts (5)
 ✗ src/utils.test.ts (8) 2 failed

 Test Files  1 failed | 1 passed (2)
      Tests  2 failed | 11 passed (13)
   Duration  520ms
"#;
        let stats = parse_test_stats(output);
        assert_eq!(stats.pass, 1);
        assert_eq!(stats.fail, 1);
        assert_eq!(stats.total, 11); // Only passed count in this pattern
    }

    #[test]
    fn test_extract_failures() {
        let output = r#"
 ✗ test_edge_case
   AssertionError: expected 10 to equal 5
     at src/lib.rs:42

 ✗ test_overflow
   Panic: overflow at src/utils.rs:18
"#;
        let failures = extract_failures(output);
        assert_eq!(failures.len(), 2);
        assert!(failures[0].contains("test_edge_case"));
        assert!(failures[0].contains("AssertionError"));
        assert!(failures[1].contains("test_overflow"));
        assert!(failures[1].contains("Panic"));
    }

    #[test]
    fn test_filter_vitest_output_all_pass() {
        let output = r#"
 ✓ src/auth.test.ts (5)
 ✓ src/utils.test.ts (8)

 Test Files  2 passed (2)
      Tests  13 passed (13)
   Duration  450ms
"#;
        let result = filter_vitest_output(output);
        assert!(result.contains("PASS (2) FAIL (0)"));
        assert!(result.contains("Time: 450ms"));
        assert!(!result.contains("✓")); // Stripped
    }

    #[test]
    fn test_filter_vitest_output_with_failures() {
        let output = r#"
 ✓ src/auth.test.ts (5)
 ✗ src/utils.test.ts (8)
   ✗ test_parse_invalid
     Expected: valid | Received: invalid

 Test Files  1 failed | 1 passed (2)
      Tests  1 failed | 12 passed (13)
   Duration  520ms
"#;
        let result = filter_vitest_output(output);
        assert!(result.contains("PASS (1) FAIL (1)"));
        assert!(result.contains("test_parse_invalid"));
        assert!(result.contains("Time: 520ms"));
    }

    #[test]
    fn test_filter_ansi_colors() {
        let output = "\x1b[32m✓\x1b[0m \x1b[1mTests passed\x1b[22m\nTest Files  1 passed (1)\n     Tests  5 passed (5)\n  Duration  100ms";
        let result = filter_vitest_output(output);
        assert!(!result.contains("\x1b["));
        assert!(result.contains("PASS (1) FAIL (0)"));
    }
}
