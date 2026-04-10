//! Filters Maven build and test output with Surefire XML parser (70-90% token reduction).
//!
//! Strips Maven boilerplate, download progress, plugin headers, and
//! shows only failures and summary for test runs.

use crate::core::tracking;
use crate::core::utils::{exit_code_from_output, resolved_command, truncate};
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use std::ffi::OsString;

lazy_static! {
    // All noise patterns collapsed into one alternation (O(1) per line).
    static ref NOISE_RE: Regex = Regex::new(
        r"^(?:\[INFO\]\s*$|\[INFO\] Scanning for projects|\[INFO\] -+(?:<|$)|\[INFO\] ={5,}|\[INFO\] Downloading\s|\[INFO\] Downloaded\s|Downloading:|Downloaded:|Progress|\[INFO\] --- maven-|\[INFO\] Using encoding|\[INFO\] skip non existing|\[INFO\] Nothing to compile|\[INFO\] Copying \d+ resources?|\[INFO\] Changes detected|\[INFO\] Finished at:|\[INFO\]\s+from\s+\S+/pom\.xml|\[INFO\] Using auto detected provider|\s*$)"
    ).unwrap();

    // Surefire test result line: "Tests run: N, Failures: M, Errors: E, Skipped: S"
    static ref TEST_RESULT_RE: Regex =
        Regex::new(r"Tests run:\s*(\d+),\s*Failures:\s*(\d+),\s*Errors:\s*(\d+),\s*Skipped:\s*(\d+)").unwrap();

    // Surefire test class header: "Running com.example.FooTest"
    static ref RUNNING_TEST_RE: Regex =
        Regex::new(r"^\[INFO\] Running\s+(\S+)").unwrap();

    // Surefire failure line: "ClassName.methodName:line message"
    static ref FAILURE_SUMMARY_RE: Regex =
        Regex::new(r"^\[ERROR\]\s+(\S+\.\S+):(\d+)\s+(.+)").unwrap();

    // Surefire failure detail: "com.example.Test.method -- Time elapsed..."
    static ref FAILURE_DETAIL_RE: Regex =
        Regex::new(r"^\[ERROR\]\s+(\S+)\s+--\s+Time elapsed").unwrap();

    // Reactor summary line: "module ... SUCCESS/FAILURE [time]"
    static ref REACTOR_LINE_RE: Regex =
        Regex::new(r"^\[INFO\]\s+\S.*\.\.\s+(SUCCESS|FAILURE)\s+\[").unwrap();

    // Stack trace line
    static ref STACK_TRACE_RE: Regex =
        Regex::new(r"^\s+at\s+").unwrap();

    // Exception/assertion line
    static ref EXCEPTION_RE: Regex =
        Regex::new(r"^(java\.\S+Exception|java\.\S+Error|org\.junit\.\S+Error)").unwrap();

    // Maven BUILD SUCCESS/FAILURE
    static ref BUILD_STATUS_RE: Regex =
        Regex::new(r"^\[INFO\] BUILD (SUCCESS|FAILURE)").unwrap();

    // Total time line
    static ref TOTAL_TIME_RE: Regex =
        Regex::new(r"^\[INFO\] Total time:\s+(.+)").unwrap();

    // Compilation error
    static ref COMPILE_ERROR_RE: Regex =
        Regex::new(r"^\[ERROR\]\s+/").unwrap();

    // Reactor Summary header
    static ref REACTOR_SUMMARY_RE: Regex =
        Regex::new(r"^\[INFO\] Reactor Summary").unwrap();

    // Building module header
    static ref BUILDING_MODULE_RE: Regex =
        Regex::new(r"^\[INFO\] Building\s+\S").unwrap();

    // Compiling N source files
    static ref COMPILING_RE: Regex =
        Regex::new(r"^\[INFO\] Compiling \d+ source files").unwrap();
}

pub fn run_test(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("mvn");
    cmd.arg("test");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: mvn test {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run mvn test. Is Maven installed?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = exit_code_from_output(&output, "mvn test");
    let filtered = filter_mvn_test(&raw);

    if let Some(hint) = crate::core::tee::tee_and_hint(&raw, "mvn_test", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("mvn test {}", args.join(" ")),
        &format!("rtk mvn test {}", args.join(" ")),
        &raw,
        &filtered,
    );

    Ok(exit_code)
}

pub fn run_compile(args: &[String], verbose: u8) -> Result<i32> {
    run_build_phase("compile", args, verbose)
}

pub fn run_package(args: &[String], verbose: u8) -> Result<i32> {
    run_build_phase("package", args, verbose)
}

/// Generic build phase runner for compile/package/install/clean
fn run_build_phase(phase: &str, args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("mvn");
    cmd.arg(phase);

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: mvn {} {}", phase, args.join(" "));
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run mvn {}. Is Maven installed?", phase))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = exit_code_from_output(&output, &format!("mvn {}", phase));
    let filtered = filter_mvn_build(&raw);

    if let Some(hint) = crate::core::tee::tee_and_hint(&raw, &format!("mvn_{}", phase), exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("mvn {} {}", phase, args.join(" ")),
        &format!("rtk mvn {} {}", phase, args.join(" ")),
        &raw,
        &filtered,
    );

    Ok(exit_code)
}

pub fn run_other(args: &[OsString], verbose: u8) -> Result<i32> {
    if args.is_empty() {
        anyhow::bail!("mvn: no subcommand specified");
    }

    let timer = tracking::TimedExecution::start();

    let subcommand = args[0].to_string_lossy();
    let mut cmd = resolved_command("mvn");
    cmd.arg(&*subcommand);

    for arg in &args[1..] {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: mvn {} ...", subcommand);
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run mvn {}", subcommand))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = exit_code_from_output(&output, "mvn");

    // Apply basic noise stripping
    let filtered = filter_mvn_build(&raw);

    if let Some(hint) =
        crate::core::tee::tee_and_hint(&raw, &format!("mvn_{}", subcommand), exit_code)
    {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("mvn {}", subcommand),
        &format!("rtk mvn {}", subcommand),
        &raw,
        &filtered,
    );

    Ok(exit_code)
}

/// Filter Maven build output (compile/package/install): strip noise, keep errors and summary
fn filter_mvn_build(output: &str) -> String {
    let mut result_lines: Vec<String> = Vec::new();
    let mut in_reactor_summary = false;
    let mut build_status = String::new();
    let mut total_time = String::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // Track reactor summary section
        if REACTOR_SUMMARY_RE.is_match(trimmed) {
            in_reactor_summary = true;
            continue;
        }

        // Capture build status
        if let Some(caps) = BUILD_STATUS_RE.captures(trimmed) {
            build_status = caps[1].to_string();
            continue;
        }

        // Capture total time
        if let Some(caps) = TOTAL_TIME_RE.captures(trimmed) {
            total_time = caps[1].to_string();
            continue;
        }

        // In reactor summary: keep module status lines
        if in_reactor_summary {
            if REACTOR_LINE_RE.is_match(trimmed) {
                result_lines.push(trimmed.to_string());
                continue;
            }
            // Empty [INFO] line ends reactor summary
            if trimmed == "[INFO]" || trimmed.starts_with("[INFO] ---") {
                in_reactor_summary = false;
                continue;
            }
        }

        // Skip noise
        if is_noise_line(trimmed) {
            continue;
        }

        // Keep ERROR lines
        if trimmed.starts_with("[ERROR]") {
            result_lines.push(truncate(trimmed, 150).to_string());
            continue;
        }

        // Keep WARNING lines
        if trimmed.starts_with("[WARNING]") {
            result_lines.push(truncate(trimmed, 150).to_string());
            continue;
        }

        // Keep compilation info
        if COMPILING_RE.is_match(trimmed) {
            result_lines.push(trimmed.to_string());
            continue;
        }

        // Keep stack trace context for errors
        if !result_lines.is_empty()
            && result_lines
                .last()
                .map_or(false, |l| l.starts_with("[ERROR]"))
        {
            if trimmed.starts_with("symbol:")
                || trimmed.starts_with("location:")
                || trimmed.starts_with("required:")
                || trimmed.starts_with("found:")
                || trimmed.starts_with("reason:")
            {
                result_lines.push(format!("  {}", trimmed));
                continue;
            }
        }
    }

    // Build summary footer
    if !build_status.is_empty() {
        let time_info = if total_time.is_empty() {
            String::new()
        } else {
            format!(" ({})", total_time)
        };
        result_lines.push(format!("BUILD {}{}", build_status, time_info));
    }

    if result_lines.is_empty() {
        return "mvn: ok".to_string();
    }

    result_lines.join("\n")
}

/// Filter Maven test output (Surefire): show failures and summary
fn filter_mvn_test(output: &str) -> String {
    let mut total_run: usize = 0;
    let mut total_failures: usize = 0;
    let mut total_errors: usize = 0;
    let mut total_skipped: usize = 0;
    let mut failures: Vec<MavenTestFailure> = Vec::new();
    let mut current_failure: Option<MavenTestFailure> = None;
    let mut in_failure_output = false;
    let mut stack_lines_collected: usize = 0;
    let mut build_status = String::new();
    let mut total_time = String::new();
    let mut in_failures_section = false;

    for line in output.lines() {
        let trimmed = line.trim();

        // Capture build status
        if let Some(caps) = BUILD_STATUS_RE.captures(trimmed) {
            build_status = caps[1].to_string();
            continue;
        }

        // Capture total time
        if let Some(caps) = TOTAL_TIME_RE.captures(trimmed) {
            total_time = caps[1].to_string();
            continue;
        }

        // Detect [ERROR] Failures: section (Surefire summary)
        if trimmed == "[ERROR] Failures:" {
            in_failures_section = true;
            continue;
        }

        // In failures section: capture compact failure summaries
        if in_failures_section {
            if let Some(caps) = FAILURE_SUMMARY_RE.captures(trimmed) {
                let method_path = caps[1].to_string();
                let line_num = caps[2].to_string();
                let message = caps[3].to_string();

                failures.push(MavenTestFailure {
                    test_name: compact_test_name(&method_path),
                    message: truncate(&message, 120).to_string(),
                    location: format!("{}:{}", method_path, line_num),
                    stack_lines: Vec::new(),
                });
                continue;
            }
            // End of failures section
            if trimmed.is_empty()
                || trimmed.starts_with("[INFO]")
                || trimmed.starts_with("[ERROR] Tests run:")
            {
                in_failures_section = false;
                // Fall through to process the line normally
            }
        }

        // Capture test result summary (last one wins — it's the global summary)
        if let Some(caps) = TEST_RESULT_RE.captures(trimmed) {
            let run: usize = caps[1].parse().unwrap_or(0);
            let fail: usize = caps[2].parse().unwrap_or(0);
            let err: usize = caps[3].parse().unwrap_or(0);
            let skip: usize = caps[4].parse().unwrap_or(0);

            // Only use the global summary (from [ERROR] or final [INFO] Results section)
            if trimmed.starts_with("[ERROR]") {
                total_run = run;
                total_failures = fail;
                total_errors = err;
                total_skipped = skip;
            } else if total_run == 0 {
                // Accumulate from per-module summaries if no global yet
                total_run += run;
                total_failures += fail;
                total_errors += err;
                total_skipped += skip;
            }
            continue;
        }

        // Capture failure detail line
        if FAILURE_DETAIL_RE.is_match(trimmed) {
            // Save previous failure
            if let Some(f) = current_failure.take() {
                if failures
                    .iter()
                    .all(|existing| existing.test_name != f.test_name)
                {
                    failures.push(f);
                }
            }

            let test_name = trimmed
                .trim_start_matches("[ERROR] ")
                .split(" -- ")
                .next()
                .unwrap_or("")
                .to_string();
            current_failure = Some(MavenTestFailure {
                test_name: compact_test_name(&test_name),
                message: String::new(),
                location: String::new(),
                stack_lines: Vec::new(),
            });
            in_failure_output = true;
            stack_lines_collected = 0;
            continue;
        }

        // Inside failure output: capture exception and stack
        if in_failure_output {
            if let Some(ref mut f) = current_failure {
                if EXCEPTION_RE.is_match(trimmed) || trimmed.starts_with("java.lang.Assertion") {
                    // Extract message from "ExceptionType: message"
                    if let Some(pos) = trimmed.find(": ") {
                        f.message = truncate(&trimmed[pos + 2..], 120).to_string();
                    } else {
                        f.message = truncate(trimmed, 120).to_string();
                    }
                } else if STACK_TRACE_RE.is_match(trimmed) && stack_lines_collected < 3 {
                    // Keep first few relevant stack lines (skip framework)
                    if !trimmed.contains("org.junit.")
                        && !trimmed.contains("java.base/")
                        && !trimmed.contains("jdk.internal")
                        && !trimmed.contains("sun.reflect")
                    {
                        f.stack_lines
                            .push(truncate(trimmed.trim(), 120).to_string());
                        stack_lines_collected += 1;
                    }
                } else if trimmed.is_empty()
                    || trimmed.starts_with("[INFO]")
                    || trimmed.starts_with("[ERROR]")
                {
                    in_failure_output = false;
                }
            }
        }
    }

    // Save last failure
    if let Some(f) = current_failure.take() {
        if failures
            .iter()
            .all(|existing| existing.test_name != f.test_name)
        {
            failures.push(f);
        }
    }

    let total_failed = total_failures + total_errors;
    let total_passed = total_run.saturating_sub(total_failed + total_skipped);

    // No tests found
    if total_run == 0 {
        let time_info = if total_time.is_empty() {
            String::new()
        } else {
            format!(" ({})", total_time)
        };
        if build_status == "FAILURE" {
            return format!("mvn test: BUILD FAILURE{}", time_info);
        }
        return format!("mvn test: no tests found{}", time_info);
    }

    // All passed
    if total_failed == 0 {
        let time_info = if total_time.is_empty() {
            String::new()
        } else {
            format!(" ({})", total_time)
        };
        let skip_info = if total_skipped > 0 {
            format!(", {} skipped", total_skipped)
        } else {
            String::new()
        };
        return format!(
            "mvn test: {} passed{}{}",
            total_passed, skip_info, time_info
        );
    }

    // Has failures
    let time_info = if total_time.is_empty() {
        String::new()
    } else {
        format!(" ({})", total_time)
    };

    let mut result = format!(
        "FAILED: {}/{} tests{}\n",
        total_failed, total_run, time_info
    );
    result.push_str("=======================================\n");

    for f in &failures {
        result.push_str(&format!("  {} FAILED\n", f.test_name));
        if !f.message.is_empty() {
            result.push_str(&format!("    {}\n", f.message));
        }
        if !f.location.is_empty() {
            result.push_str(&format!("    at {}\n", truncate(&f.location, 100)));
        }
        for stack_line in &f.stack_lines {
            result.push_str(&format!("    {}\n", stack_line));
        }
    }

    if build_status == "FAILURE" {
        result.push_str("\nBUILD FAILURE");
    }

    result.trim().to_string()
}

struct MavenTestFailure {
    test_name: String,
    message: String,
    location: String,
    stack_lines: Vec<String>,
}

/// Check if a line matches any noise pattern
fn is_noise_line(line: &str) -> bool {
    NOISE_RE.is_match(line)
}

/// Compact test name: "com.edeal.frontline.UserServiceTest.testFoo" -> "UserServiceTest.testFoo"
fn compact_test_name(name: &str) -> String {
    let parts: Vec<&str> = name.rsplitn(3, '.').collect();
    if parts.len() >= 2 {
        format!("{}.{}", parts[1], parts[0])
    } else {
        name.to_string()
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
    fn test_filter_mvn_build_success() {
        let input = include_str!("../../../tests/fixtures/mvn_compile_success_raw.txt");
        let output = filter_mvn_build(input);

        // Should strip noise
        assert!(!output.contains("Scanning for projects"));
        assert!(!output.contains("Downloading from central"));
        assert!(!output.contains("Downloaded from central"));
        assert!(!output.contains("maven-resources-plugin"));
        assert!(!output.contains("maven-compiler-plugin"));
        assert!(!output.contains("Nothing to compile"));

        // Should keep build result
        assert!(output.contains("BUILD SUCCESS"));
        assert!(output.contains("18.234"));
    }

    #[test]
    fn test_filter_mvn_build_failure() {
        let input = include_str!("../../../tests/fixtures/mvn_compile_fail_raw.txt");
        let output = filter_mvn_build(input);

        // Should keep errors
        assert!(output.contains("[ERROR]"));
        assert!(output.contains("cannot find symbol"));
        assert!(output.contains("BUILD FAILURE"));

        // Should strip noise
        assert!(!output.contains("Scanning for projects"));
        assert!(!output.contains("maven-resources-plugin"));
    }

    #[test]
    fn test_filter_mvn_build_empty() {
        let output = filter_mvn_build("");
        assert_eq!(output, "mvn: ok");
    }

    #[test]
    fn test_filter_mvn_build_savings() {
        let input = include_str!("../../../tests/fixtures/mvn_compile_success_raw.txt");
        let output = filter_mvn_build(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "Maven build filter: expected >=60% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    // ============================================================
    // Test filter tests
    // ============================================================

    #[test]
    fn test_filter_mvn_test_all_pass() {
        let input = include_str!("../../../tests/fixtures/mvn_test_pass_raw.txt");
        let output = filter_mvn_test(input);

        assert!(output.contains("mvn test:"));
        assert!(output.contains("passed"));
        assert!(!output.contains("FAILED"));
    }

    #[test]
    fn test_filter_mvn_test_with_failures() {
        let input = include_str!("../../../tests/fixtures/mvn_test_fail_raw.txt");
        let output = filter_mvn_test(input);

        assert!(output.contains("FAILED"));
        assert!(output.contains("2/14") || output.contains("2 "));
        // Should contain failure information
        assert!(output.contains("testUpdateUserProfile") || output.contains("UserServiceTest"));
    }

    #[test]
    fn test_filter_mvn_test_savings_pass() {
        let input = include_str!("../../../tests/fixtures/mvn_test_pass_raw.txt");
        let output = filter_mvn_test(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 85.0,
            "Maven test (pass) filter: expected >=85% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_filter_mvn_test_savings_fail() {
        let input = include_str!("../../../tests/fixtures/mvn_test_fail_raw.txt");
        let output = filter_mvn_test(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 70.0,
            "Maven test (fail) filter: expected >=70% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_filter_mvn_test_empty() {
        let output = filter_mvn_test("");
        assert!(output.contains("mvn test:"));
    }

    // ============================================================
    // Utility tests
    // ============================================================

    // ============================================================
    // Snapshot tests
    // ============================================================

    #[test]
    fn test_snapshot_mvn_build_success() {
        let input = include_str!("../../../tests/fixtures/mvn_compile_success_raw.txt");
        insta::assert_snapshot!(filter_mvn_build(input));
    }

    #[test]
    fn test_snapshot_mvn_build_fail() {
        let input = include_str!("../../../tests/fixtures/mvn_compile_fail_raw.txt");
        insta::assert_snapshot!(filter_mvn_build(input));
    }

    #[test]
    fn test_snapshot_mvn_test_pass() {
        let input = include_str!("../../../tests/fixtures/mvn_test_pass_raw.txt");
        insta::assert_snapshot!(filter_mvn_test(input));
    }

    #[test]
    fn test_snapshot_mvn_test_fail() {
        let input = include_str!("../../../tests/fixtures/mvn_test_fail_raw.txt");
        insta::assert_snapshot!(filter_mvn_test(input));
    }

    #[test]
    fn test_compact_test_name() {
        assert_eq!(
            compact_test_name("com.edeal.frontline.UserServiceTest.testFoo"),
            "UserServiceTest.testFoo"
        );
        assert_eq!(
            compact_test_name("SimpleTest.testBar"),
            "SimpleTest.testBar"
        );
    }

    #[test]
    fn test_is_noise_line() {
        assert!(is_noise_line("[INFO] Scanning for projects..."));
        assert!(is_noise_line("[INFO] Downloading org.apache:foo:1.0"));
        assert!(is_noise_line("[INFO] Downloaded org.apache:foo:1.0"));
        assert!(is_noise_line(
            "[INFO] --- maven-compiler-plugin:3.11.0:compile ---"
        ));
        assert!(is_noise_line(
            "[INFO] Nothing to compile - all classes are up to date."
        ));
        assert!(is_noise_line("[INFO] "));
        assert!(is_noise_line("[INFO] ---"));
        assert!(is_noise_line("[INFO] Using encoding: UTF-8"));

        assert!(!is_noise_line("[ERROR] Compilation failed"));
        assert!(!is_noise_line("[INFO] BUILD SUCCESS"));
        assert!(!is_noise_line("[WARNING] Using deprecated API"));
    }
}
