use crate::tracking;
use anyhow::{Context, Result};
use regex::Regex;
use std::process::{Command, Stdio};

/// Run a command and filter output to show only errors/warnings
pub fn run_err(command: &str, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Running: {}", command);
    }

    let output = if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", command])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
    } else {
        Command::new("sh")
            .args(["-c", command])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
    }
    .context("Failed to execute command")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);
    let filtered = filter_errors(&raw);
    let mut rtk = String::new();

    if filtered.is_empty() {
        if output.status.success() {
            rtk.push_str("✅ Command completed successfully (no errors)");
        } else {
            rtk.push_str(&format!(
                "❌ Command failed (exit code: {:?})\n",
                output.status.code()
            ));
            let lines: Vec<&str> = raw.lines().collect();
            for line in lines.iter().rev().take(10).rev() {
                rtk.push_str(&format!("  {}\n", line));
            }
        }
    } else {
        rtk.push_str(&filtered);
    }

    println!("{}", rtk);
    timer.track(command, "rtk run-err", &raw, &rtk);
    Ok(())
}

/// Run tests and show only failures
pub fn run_test(command: &str, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Running tests: {}", command);
    }

    let output = if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", command])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
    } else {
        Command::new("sh")
            .args(["-c", command])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
    }
    .context("Failed to execute test command")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let summary = extract_test_summary(&raw, command);
    println!("{}", summary);
    timer.track(command, "rtk run-test", &raw, &summary);
    Ok(())
}

fn filter_errors(output: &str) -> String {
    lazy_static::lazy_static! {
        static ref ERROR_PATTERNS: Vec<Regex> = vec![
            // Generic errors
            Regex::new(r"(?i)^.*error[\s:\[].*$").unwrap(),
            Regex::new(r"(?i)^.*\berr\b.*$").unwrap(),
            Regex::new(r"(?i)^.*warning[\s:\[].*$").unwrap(),
            Regex::new(r"(?i)^.*\bwarn\b.*$").unwrap(),
            Regex::new(r"(?i)^.*failed.*$").unwrap(),
            Regex::new(r"(?i)^.*failure.*$").unwrap(),
            Regex::new(r"(?i)^.*exception.*$").unwrap(),
            Regex::new(r"(?i)^.*panic.*$").unwrap(),
            // Rust specific
            Regex::new(r"^error\[E\d+\]:.*$").unwrap(),
            Regex::new(r"^\s*--> .*:\d+:\d+$").unwrap(),
            // Python
            Regex::new(r"^Traceback.*$").unwrap(),
            Regex::new(r#"^\s*File ".*", line \d+.*$"#).unwrap(),
            // JavaScript/TypeScript
            Regex::new(r"^\s*at .*:\d+:\d+.*$").unwrap(),
            // Go
            Regex::new(r"^.*\.go:\d+:.*$").unwrap(),
        ];
    }

    let mut result = Vec::new();
    let mut in_error_block = false;
    let mut blank_count = 0;

    for line in output.lines() {
        let is_error_line = ERROR_PATTERNS.iter().any(|p| p.is_match(line));

        if is_error_line {
            in_error_block = true;
            blank_count = 0;
            result.push(line.to_string());
        } else if in_error_block {
            if line.trim().is_empty() {
                blank_count += 1;
                if blank_count >= 2 {
                    in_error_block = false;
                } else {
                    result.push(line.to_string());
                }
            } else if line.starts_with(' ') || line.starts_with('\t') {
                // Continuation of error
                result.push(line.to_string());
                blank_count = 0;
            } else {
                in_error_block = false;
            }
        }
    }

    result.join("\n")
}

fn extract_test_summary(output: &str, command: &str) -> String {
    let mut result = Vec::new();
    let lines: Vec<&str> = output.lines().collect();

    // Detect test framework
    let is_cargo = command.contains("cargo test");
    let is_pytest = command.contains("pytest");
    let is_jest =
        command.contains("jest") || command.contains("npm test") || command.contains("yarn test");
    let is_go = command.contains("go test");

    // Collect failures
    let mut failures = Vec::new();
    let mut in_failure = false;
    let mut failure_lines = Vec::new();

    for line in lines.iter() {
        // Cargo test
        if is_cargo {
            if line.contains("test result:") {
                result.push(line.to_string());
            }
            if line.contains("FAILED") && !line.contains("test result") {
                failures.push(line.to_string());
            }
            if line.starts_with("failures:") {
                in_failure = true;
            }
            if in_failure && line.starts_with("    ") {
                failure_lines.push(line.to_string());
            }
        }

        // Pytest
        if is_pytest {
            if line.contains(" passed") || line.contains(" failed") || line.contains(" error") {
                result.push(line.to_string());
            }
            if line.contains("FAILED") {
                failures.push(line.to_string());
            }
        }

        // Jest
        if is_jest {
            if line.contains("Tests:") || line.contains("Test Suites:") {
                result.push(line.to_string());
            }
            if line.contains("✕") || line.contains("FAIL") {
                failures.push(line.to_string());
            }
        }

        // Go test
        if is_go {
            if line.starts_with("ok") || line.starts_with("FAIL") || line.starts_with("---") {
                result.push(line.to_string());
            }
            if line.contains("FAIL") {
                failures.push(line.to_string());
            }
        }
    }

    // Build output
    let mut output = String::new();

    if !failures.is_empty() {
        output.push_str("❌ FAILURES:\n");
        for f in failures.iter().take(10) {
            output.push_str(&format!("  {}\n", f));
        }
        if failures.len() > 10 {
            output.push_str(&format!("  ... +{} more failures\n", failures.len() - 10));
        }
        output.push('\n');
    }

    if !result.is_empty() {
        output.push_str("📊 SUMMARY:\n");
        for r in &result {
            output.push_str(&format!("  {}\n", r));
        }
    } else {
        // Fallback: show last few lines
        output.push_str("📊 OUTPUT (last 5 lines):\n");
        let start = lines.len().saturating_sub(5);
        for line in &lines[start..] {
            if !line.trim().is_empty() {
                output.push_str(&format!("  {}\n", line));
            }
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_errors() {
        let output = "info: compiling\nerror: something failed\n  at line 10\ninfo: done";
        let filtered = filter_errors(output);
        assert!(filtered.contains("error"));
        assert!(!filtered.contains("info"));
    }
}
