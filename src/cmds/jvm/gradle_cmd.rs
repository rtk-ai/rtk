//! Filters Gradle build and test output (75-90% token reduction).
//!
//! Strips Gradle daemon noise, download progress, UP-TO-DATE task lines,
//! and shows only failures and summary for test runs.

use crate::core::tracking;
use crate::core::utils::{exit_code_from_output, resolved_command, truncate};
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use std::ffi::OsString;

lazy_static! {
    // All noise patterns collapsed into one alternation (O(1) per line).
    // Note: welcome-banner bullet lines ("^ - ") are handled via in_welcome_block
    // state in filter_gradle_build rather than here — the pattern was too broad.
    static ref NOISE_RE: Regex = Regex::new(
        r"^(?:Downloading https://|Download https://|\.+\d+%|Welcome to Gradle|For more details see https://docs\.gradle\.org|Starting a Gradle Daemon|Daemon will be stopped|> Configure project|> Resolving dependencies|> Transform |> Task :\S+ UP-TO-DATE$|> Task :\S+ NO-SOURCE$|> Task :\S+ FROM-CACHE$|\s*<-+>\s*$|Note: .*(deprecated|Recompile with))"
    ).unwrap();

    // Gradle test result line: "ClassName > methodName PASSED/FAILED"
    static ref TEST_RESULT_RE: Regex =
        Regex::new(r"^(\S+)\s+>\s+(\S+)\s+(PASSED|FAILED)\s*$").unwrap();

    // Gradle test summary: "N tests completed, M failed[, K skipped]"
    static ref TEST_SUMMARY_RE: Regex =
        Regex::new(r"^(\d+)\s+tests?\s+completed,\s+(\d+)\s+failed(?:,\s+(\d+)\s+skipped)?").unwrap();

    // Stack trace line (indented, starts with "at ")
    static ref STACK_TRACE_RE: Regex =
        Regex::new(r"^\s+at\s+").unwrap();

    // Assertion/error line
    static ref ERROR_LINE_RE: Regex =
        Regex::new(r"(?i)(error|exception|assert|expected|but was)").unwrap();

    // Compiler error line (file:line: error:)
    static ref COMPILER_ERROR_RE: Regex =
        Regex::new(r"^.+\.java:\d+:.*error:").unwrap();
}

/// Detect whether to use ./gradlew or gradle
fn gradle_command() -> std::process::Command {
    // Prefer ./gradlew wrapper if it exists in current directory
    let gradlew = if cfg!(windows) {
        "gradlew.bat"
    } else {
        "./gradlew"
    };

    if std::path::Path::new(gradlew).exists() {
        resolved_command(gradlew)
    } else {
        resolved_command("gradle")
    }
}

pub fn run_build(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = gradle_command();
    cmd.arg("build");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: gradle build {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run gradle. Is Gradle installed? Check ./gradlew or gradle on PATH")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = exit_code_from_output(&output, "gradle build");
    let filtered = filter_gradle_build(&raw);

    if let Some(hint) = crate::core::tee::tee_and_hint(&raw, "gradle_build", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("gradle build {}", args.join(" ")),
        &format!("rtk gradle build {}", args.join(" ")),
        &raw,
        &filtered,
    );

    Ok(exit_code)
}

pub fn run_test(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = gradle_command();
    cmd.arg("test");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: gradle test {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run gradle test. Is Gradle installed?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = exit_code_from_output(&output, "gradle test");
    let filtered = filter_gradle_test(&raw);

    if let Some(hint) = crate::core::tee::tee_and_hint(&raw, "gradle_test", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("gradle test {}", args.join(" ")),
        &format!("rtk gradle test {}", args.join(" ")),
        &raw,
        &filtered,
    );

    Ok(exit_code)
}

pub fn run_other(args: &[OsString], verbose: u8) -> Result<i32> {
    if args.is_empty() {
        anyhow::bail!("gradle: no subcommand specified");
    }

    let timer = tracking::TimedExecution::start();

    let subcommand = args[0].to_string_lossy();
    let mut cmd = gradle_command();
    cmd.arg(&*subcommand);

    for arg in &args[1..] {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: gradle {} ...", subcommand);
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run gradle {}", subcommand))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = exit_code_from_output(&output, "gradle");

    // Apply basic noise stripping for any subcommand
    let filtered = filter_gradle_build(&raw);

    if let Some(hint) =
        crate::core::tee::tee_and_hint(&raw, &format!("gradle_{}", subcommand), exit_code)
    {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("gradle {}", subcommand),
        &format!("rtk gradle {}", subcommand),
        &raw,
        &filtered,
    );

    Ok(exit_code)
}

/// Filter Gradle build output: strip noise, keep errors and summary
fn filter_gradle_build(output: &str) -> String {
    let mut result_lines: Vec<String> = Vec::new();
    let mut in_welcome_block = false;

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip empty lines; an empty line also ends the welcome banner block
        if trimmed.is_empty() {
            in_welcome_block = false;
            continue;
        }

        // Detect the welcome-banner highlights block and drop its bullet lines
        if trimmed.starts_with("Here are the highlights") {
            in_welcome_block = true;
            continue;
        }
        if in_welcome_block {
            if trimmed.starts_with("- ") {
                continue;
            }
            in_welcome_block = false;
        }

        // Skip noise patterns
        if is_noise_line(trimmed) {
            continue;
        }

        // Keep task lines that actually ran (not UP-TO-DATE/NO-SOURCE/FROM-CACHE)
        // Keep error lines, build status, task counts
        result_lines.push(truncate(trimmed, 150).to_string());
    }

    if result_lines.is_empty() {
        return "Gradle build: ok".to_string();
    }

    result_lines.join("\n")
}

/// Filter Gradle test output: show only failures and summary
fn filter_gradle_test(output: &str) -> String {
    let mut passed: usize = 0;
    let mut failed: usize = 0;
    let mut failures: Vec<TestFailure> = Vec::new();
    let mut current_failure: Option<TestFailure> = None;
    let mut in_failure_block = false;
    let mut has_build_failure = false;
    let mut build_errors: Vec<String> = Vec::new();
    let mut time_str = String::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // Extract timing from BUILD line
        if trimmed.starts_with("BUILD SUCCESSFUL in") || trimmed.starts_with("BUILD FAILED in") {
            if let Some(pos) = trimmed.rfind(" in ") {
                time_str = trimmed[pos + 4..].to_string();
            }
            if trimmed.contains("FAILED") {
                has_build_failure = true;
            }
            continue;
        }

        // Capture test summary line
        if let Some(caps) = TEST_SUMMARY_RE.captures(trimmed) {
            let total: usize = caps[1].parse().unwrap_or(0);
            let fail_count: usize = caps[2].parse().unwrap_or(0);
            let skip_count: usize = caps
                .get(3)
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(0);
            passed = total.saturating_sub(fail_count + skip_count);
            failed = fail_count;
            continue;
        }

        // Capture individual test results
        if let Some(caps) = TEST_RESULT_RE.captures(trimmed) {
            let class = caps[1].to_string();
            let method = caps[2].to_string();
            let status = &caps[3];

            if status == "FAILED" {
                // Save previous failure if any
                if let Some(f) = current_failure.take() {
                    failures.push(f);
                }
                current_failure = Some(TestFailure {
                    class: compact_class_name(&class),
                    method,
                    message: String::new(),
                    location: String::new(),
                });
                in_failure_block = true;
            } else {
                in_failure_block = false;
            }
            continue;
        }

        // Inside a failure block, capture error details
        if in_failure_block {
            if let Some(ref mut f) = current_failure {
                if ERROR_LINE_RE.is_match(trimmed) && f.message.is_empty() {
                    f.message = truncate(trimmed, 120).to_string();
                } else if STACK_TRACE_RE.is_match(trimmed) && f.location.is_empty() {
                    // Extract just the location (first non-framework stack line)
                    if !trimmed.contains("org.junit.")
                        && !trimmed.contains("java.base/")
                        && !trimmed.contains("jdk.internal")
                    {
                        f.location = trimmed.trim_start_matches("at ").trim().to_string();
                    }
                }
            }
        }

        // Capture compiler errors (build failures, not test failures)
        if COMPILER_ERROR_RE.is_match(trimmed) {
            build_errors.push(truncate(trimmed, 120).to_string());
        }

        // Capture FAILURE explanation blocks
        if trimmed.starts_with("* What went wrong:")
            || trimmed.starts_with("Execution failed for task")
        {
            has_build_failure = true;
        }
    }

    // Save last failure
    if let Some(f) = current_failure.take() {
        failures.push(f);
    }

    // Build output
    let total = passed + failed;

    // If no tests were found but there were build errors
    if total == 0 && !build_errors.is_empty() {
        let mut result = format!("Gradle: {} build errors\n", build_errors.len());
        result.push_str("=======================================\n");
        for error in build_errors.iter().take(10) {
            result.push_str(&format!("  {}\n", error));
        }
        if build_errors.len() > 10 {
            result.push_str(&format!("  ... +{} more errors\n", build_errors.len() - 10));
        }
        return result.trim().to_string();
    }

    // No tests ran (and no build errors or explicit failure)
    if total == 0 && !has_build_failure {
        let time_info = if time_str.is_empty() {
            String::new()
        } else {
            format!(" ({})", time_str)
        };
        return format!("Gradle test: no tests found{}", time_info);
    }

    // All passed
    if failed == 0 && !has_build_failure {
        let time_info = if time_str.is_empty() {
            String::new()
        } else {
            format!(" ({})", time_str)
        };
        return format!("Gradle test: {} passed{}", total, time_info);
    }

    // Has failures
    let time_info = if time_str.is_empty() {
        String::new()
    } else {
        format!(" ({})", time_str)
    };

    let mut result = format!("FAILED: {}/{} tests{}\n", failed, total, time_info);
    result.push_str("=======================================\n");

    for f in &failures {
        result.push_str(&format!("  {} > {}() FAILED\n", f.class, f.method));
        if !f.message.is_empty() {
            result.push_str(&format!("    {}\n", f.message));
        }
        if !f.location.is_empty() {
            result.push_str(&format!("    at {}\n", truncate(&f.location, 100)));
        }
    }

    if has_build_failure {
        result.push_str("\nBUILD FAILED");
    }

    result.trim().to_string()
}

struct TestFailure {
    class: String,
    method: String,
    message: String,
    location: String,
}

/// Check if a line matches any noise pattern
fn is_noise_line(line: &str) -> bool {
    NOISE_RE.is_match(line)
}

/// Compact class name: "com.edeal.frontline.UserServiceTest" -> "UserServiceTest"
fn compact_class_name(class: &str) -> String {
    if let Some(pos) = class.rfind('.') {
        class[pos + 1..].to_string()
    } else {
        class.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    // ============================================================
    // Build filter tests
    // ============================================================

    #[test]
    fn test_filter_gradle_build_success() {
        let input = include_str!("../../../tests/fixtures/gradle_build_success_raw.txt");
        let output = filter_gradle_build(input);

        // Should strip noise (downloads, UP-TO-DATE, Welcome, Daemon)
        assert!(!output.contains("Downloading https://"));
        assert!(!output.contains("Welcome to Gradle"));
        assert!(!output.contains("Starting a Gradle Daemon"));
        assert!(!output.contains("UP-TO-DATE"));
        assert!(!output.contains("NO-SOURCE"));
        assert!(!output.contains("Configure project"));

        // Should keep build result
        assert!(output.contains("BUILD SUCCESSFUL"));
        assert!(output.contains("14 actionable tasks"));
    }

    #[test]
    fn test_filter_gradle_build_fail() {
        let input = include_str!("../../../tests/fixtures/gradle_build_fail_raw.txt");
        let output = filter_gradle_build(input);

        // Should keep error details
        assert!(output.contains("FAILED"));
        assert!(output.contains("cannot find symbol"));
        assert!(output.contains("error"));

        // Should strip noise
        assert!(!output.contains("Starting a Gradle Daemon"));
        assert!(!output.contains("UP-TO-DATE"));
    }

    #[test]
    fn test_filter_gradle_build_empty() {
        let output = filter_gradle_build("");
        assert_eq!(output, "Gradle build: ok");
    }

    #[test]
    fn test_filter_gradle_build_savings() {
        let input = include_str!("../../../tests/fixtures/gradle_build_success_raw.txt");
        let output = filter_gradle_build(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "Gradle build filter: expected >=60% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    // ============================================================
    // Test filter tests
    // ============================================================

    #[test]
    fn test_filter_gradle_test_all_pass() {
        let input = include_str!("../../../tests/fixtures/gradle_test_pass_raw.txt");
        let output = filter_gradle_test(input);

        assert!(output.contains("Gradle test:"));
        assert!(output.contains("20 passed"));
        assert!(!output.contains("FAILED"));
    }

    #[test]
    fn test_filter_gradle_test_with_failures() {
        let input = include_str!("../../../tests/fixtures/gradle_test_fail_raw.txt");
        let output = filter_gradle_test(input);

        assert!(output.contains("FAILED: 2/20"));
        assert!(output.contains("testUpdateUserProfile"));
        assert!(output.contains("testAuthRequired"));
        // Should contain error messages
        assert!(
            output.contains("Expected user name")
                || output.contains("AssertionError")
                || output.contains("expected")
        );
    }

    #[test]
    fn test_filter_gradle_test_savings_pass() {
        let input = include_str!("../../../tests/fixtures/gradle_test_pass_raw.txt");
        let output = filter_gradle_test(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 85.0,
            "Gradle test (pass) filter: expected >=85% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_filter_gradle_test_savings_fail() {
        let input = include_str!("../../../tests/fixtures/gradle_test_fail_raw.txt");
        let output = filter_gradle_test(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 70.0,
            "Gradle test (fail) filter: expected >=70% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_filter_gradle_test_empty() {
        let output = filter_gradle_test("");
        // No tests found, no build errors => "no tests found"
        assert!(output.contains("Gradle test: no tests found"));
    }

    #[test]
    fn test_filter_gradle_test_with_build_errors() {
        // Exercises the total==0 && !build_errors.is_empty() branch:
        // compile failure before any tests ran
        let input = include_str!("../../../tests/fixtures/gradle_build_fail_raw.txt");
        let output = filter_gradle_test(input);
        assert!(
            output.contains("build errors") || output.contains("error"),
            "Expected build error summary, got: {}",
            output
        );
        assert!(!output.contains("passed"), "Should not report passed tests");
    }

    // ============================================================
    // Utility tests
    // ============================================================

    #[test]
    fn test_compact_class_name() {
        assert_eq!(
            compact_class_name("com.edeal.frontline.UserServiceTest"),
            "UserServiceTest"
        );
        assert_eq!(compact_class_name("SimpleTest"), "SimpleTest");
    }

    // ============================================================
    // Snapshot tests
    // ============================================================

    #[test]
    fn test_snapshot_gradle_build_success() {
        let input = include_str!("../../../tests/fixtures/gradle_build_success_raw.txt");
        insta::assert_snapshot!(filter_gradle_build(input));
    }

    #[test]
    fn test_snapshot_gradle_build_fail() {
        let input = include_str!("../../../tests/fixtures/gradle_build_fail_raw.txt");
        insta::assert_snapshot!(filter_gradle_build(input));
    }

    #[test]
    fn test_snapshot_gradle_test_pass() {
        let input = include_str!("../../../tests/fixtures/gradle_test_pass_raw.txt");
        insta::assert_snapshot!(filter_gradle_test(input));
    }

    #[test]
    fn test_snapshot_gradle_test_fail() {
        let input = include_str!("../../../tests/fixtures/gradle_test_fail_raw.txt");
        insta::assert_snapshot!(filter_gradle_test(input));
    }

    #[test]
    fn test_is_noise_line() {
        assert!(is_noise_line(
            "Downloading https://services.gradle.org/dist"
        ));
        assert!(is_noise_line("Welcome to Gradle 9.1.0!"));
        assert!(is_noise_line(
            "Starting a Gradle Daemon (subsequent builds will be faster)"
        ));
        assert!(is_noise_line("> Task :app:compileJava UP-TO-DATE"));
        assert!(is_noise_line("> Task :app:processResources NO-SOURCE"));
        assert!(is_noise_line("> Configure project :app"));

        assert!(!is_noise_line("BUILD SUCCESSFUL in 28s"));
        assert!(!is_noise_line("> Task :app:compileJava FAILED"));
        assert!(!is_noise_line("3 errors"));
    }
}
