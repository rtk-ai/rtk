use crate::core::runner::{self, RunOptions};
use crate::core::utils::{resolved_command, truncate};
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;
use std::ffi::OsString;

lazy_static! {
    /// Matches the ScalaTest summary line:
    /// Tests: succeeded N, failed N, canceled N, ignored N, pending N
    static ref TEST_SUMMARY_RE: Regex = Regex::new(
        r"Tests: succeeded (\d+), failed (\d+), canceled (\d+), ignored (\d+), pending (\d+)"
    ).unwrap();

    /// Matches the munit summary line (also used by discipline-munit / ZIO Test):
    /// [info] Passed: Total N, Failed N, Errors N, Passed N
    /// [info] Failed: Total N, Failed N, Errors N, Passed N
    static ref MUNIT_SUMMARY_RE: Regex = Regex::new(
        r"^\[info\] (?:Passed|Failed): Total \d+, Failed (\d+), Errors (\d+), Passed (\d+)"
    ).unwrap();

    /// Matches suite count line:
    /// Suites: completed N, aborted N
    static ref SUITE_SUMMARY_RE: Regex = Regex::new(
        r"Suites: completed (\d+), aborted (\d+)"
    ).unwrap();

    /// Matches the "Run completed in" timing line
    static ref RUN_TIME_RE: Regex = Regex::new(
        r"Run completed in (\d+) seconds?"
    ).unwrap();

    /// Matches [info] Compiling N Scala source(s)
    static ref COMPILE_COUNT_RE: Regex = Regex::new(
        r"\[info\] Compiling (\d+) Scala source"
    ).unwrap();

    /// Matches [success] Total time: Ns
    static ref SUCCESS_TIME_RE: Regex = Regex::new(
        r"\[success\] Total time: (\d+) s"
    ).unwrap();

    /// Matches [error] lines
    static ref ERROR_RE: Regex = Regex::new(
        r"^\[error\]"
    ).unwrap();

    /// Lines that are SBT noise (loading, resolving, downloading, etc.)
    static ref NOISE_RE: Regex = Regex::new(
        r"^\[info\] (welcome to sbt|loading |set current project|Updating |Resolved |Fetching |downloading |Done )"
    ).unwrap();
}

/// Integration test subcommand patterns (sbt configuration/task notation).
/// These produce ScalaTest output and should use the same filtering as `sbt test`.
fn is_integration_test_cmd(subcommand: &str) -> bool {
    matches!(
        subcommand,
        "it:test" | "IntegrationTest/test" | "integration-test/test"
    ) || (subcommand.ends_with(":test") || subcommand.ends_with("/test"))
}

fn is_test_task(subcommand: &str) -> bool {
    let task = subcommand.split_whitespace().next().unwrap_or(subcommand);
    let task = task.rsplit(['/', ':']).next().unwrap_or(task);
    matches!(task, "testOnly" | "testQuick")
}

/// Returns true if `s` is a scoped SBT task (e.g. `Test/test`, `it/Test/compile`).
fn is_scoped_task(s: &str) -> bool {
    !s.starts_with('-') && (s.contains('/') || s.contains(':'))
}

/// Run an SBT task through the shared runner (never_worse cap, tee hint, tracking,
/// and exit-code propagation all handled centrally).
///
/// A scoped task passed as the first arg (e.g. `Test/test`, `exampleJVM/run`) is used
/// verbatim so sbt runs the right configuration scope; otherwise `default_task` is used.
fn run_task(
    args: &[String],
    default_task: &str,
    filter: fn(&str) -> String,
    tee_label: &str,
    verbose: u8,
) -> Result<i32> {
    let mut cmd = resolved_command("sbt");

    let (sbt_task, rest) = match args.first() {
        Some(a) if is_scoped_task(a) => (a.as_str(), &args[1..]),
        _ => (default_task, args),
    };
    cmd.arg(sbt_task);
    for arg in rest {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: sbt {} {}", sbt_task, rest.join(" "));
    }

    let args_display = if rest.is_empty() {
        sbt_task.to_string()
    } else {
        format!("{} {}", sbt_task, rest.join(" "))
    };

    runner::run_filtered(
        cmd,
        "sbt",
        &args_display,
        filter,
        RunOptions::with_tee(tee_label),
    )
}

pub fn run_test(args: &[String], verbose: u8) -> Result<i32> {
    run_task(args, "test", filter_sbt_test, "sbt_test", verbose)
}

pub fn run_compile(args: &[String], verbose: u8) -> Result<i32> {
    run_task(args, "compile", filter_sbt_compile, "sbt_compile", verbose)
}

pub fn run_run(args: &[String], verbose: u8) -> Result<i32> {
    run_task(args, "run", filter_sbt_run, "sbt_run", verbose)
}

pub fn run_other(args: &[OsString], verbose: u8) -> Result<i32> {
    if args.is_empty() {
        anyhow::bail!("sbt: no subcommand specified");
    }

    let subcommand = args[0].to_string_lossy().into_owned();

    // Integration test commands (it:test, IntegrationTest/test, etc.) produce standard
    // ScalaTest output — filter them like `sbt test`, through the shared runner so the
    // never_worse cap and tee hint apply (an unrecognized output would otherwise be
    // reprinted verbatim plus the hint, i.e. more than the raw command produced).
    if is_integration_test_cmd(&subcommand) || is_test_task(&subcommand) {
        let mut cmd = resolved_command("sbt");
        cmd.arg(&subcommand);
        for arg in &args[1..] {
            cmd.arg(arg);
        }

        if verbose > 0 {
            eprintln!("Running: sbt {} ...", subcommand);
        }

        let tee_label = if is_integration_test_cmd(&subcommand) {
            "sbt_it_test"
        } else {
            "sbt_test"
        };

        let rest: Vec<String> = args[1..]
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let args_display = if rest.is_empty() {
            subcommand
        } else {
            format!("{} {}", subcommand, rest.join(" "))
        };

        return runner::run_filtered(
            cmd,
            "sbt",
            &args_display,
            filter_sbt_test,
            RunOptions::with_tee(tee_label),
        );
    }

    // Any other subcommand: pass through unfiltered (still tracked, exit code propagated).
    runner::run_passthrough("sbt", args, verbose)
}

/// A single test failure with its name and detail lines captured from the output.
struct FailureBlock {
    name: String,
    details: Vec<String>,
}

/// Filter SBT test output (ScalaTest format).
///
/// On success: compact single-line summary.
/// On failure: show each failed test with its detail lines (works for native
/// ScalaTest assertion failures, Mockito Scala verification failures, and
/// ScalaMock expectation failures — all of which emit details as [info] lines).
fn filter_sbt_test(output: &str) -> String {
    let mut succeeded: u32 = 0;
    let mut failed: u32 = 0;
    let mut ignored: u32 = 0;
    let mut canceled: u32 = 0;
    let mut pending: u32 = 0;
    let mut suites: u32 = 0;
    let mut run_time_secs: Option<u32> = None;
    let mut has_summary = false;

    let mut failures: Vec<FailureBlock> = Vec::new();
    let mut failed_suites: Vec<String> = Vec::new();
    let mut error_lines: Vec<String> = Vec::new();
    // true while we are inside the detail block of a failed test
    let mut in_failure_detail = false;

    for line in output.lines() {
        let trimmed = line.trim();

        // --- Summary lines (always reset failure-detail mode) ---

        if let Some(caps) = TEST_SUMMARY_RE.captures(trimmed) {
            succeeded = caps[1].parse().unwrap_or(0);
            failed = caps[2].parse().unwrap_or(0);
            canceled = caps[3].parse().unwrap_or(0);
            ignored = caps[4].parse().unwrap_or(0);
            pending = caps[5].parse().unwrap_or(0);
            has_summary = true;
            in_failure_detail = false;
            continue;
        }
        if let Some(caps) = MUNIT_SUMMARY_RE.captures(trimmed) {
            let munit_failed: u32 = caps[1].parse().unwrap_or(0);
            let errors: u32 = caps[2].parse().unwrap_or(0);
            succeeded = caps[3].parse().unwrap_or(0);
            failed = munit_failed + errors;
            has_summary = true;
            in_failure_detail = false;
            continue;
        }
        if let Some(caps) = SUITE_SUMMARY_RE.captures(trimmed) {
            suites = caps[1].parse().unwrap_or(0);
            in_failure_detail = false;
            continue;
        }
        if let Some(caps) = RUN_TIME_RE.captures(trimmed) {
            run_time_secs = caps[1].parse().ok();
            in_failure_detail = false;
            continue;
        }

        // --- Failed test header: "- test name *** FAILED ***" ---

        if trimmed.contains("*** FAILED ***") {
            let name = trimmed
                .strip_suffix(" *** FAILED ***")
                .unwrap_or(trimmed)
                .strip_prefix("[info]")
                .unwrap_or(trimmed)
                .trim()
                .trim_start_matches('-')
                .trim()
                .to_string();
            failures.push(FailureBlock { name, details: Vec::new() });
            in_failure_detail = true;
            continue;
        }

        // --- Detail lines inside a failure block ---
        //
        // ScalaTest places failure details as [info] lines with deeper
        // indentation (4+ spaces after "[info]"). This covers:
        //   - native assertion messages  ("42 was not equal to 43")
        //   - Mockito verification msgs  ("org.mockito.exceptions.verification...")
        //   - ScalaMock expectation msgs ("Unexpected call: ...")
        //
        // A line with shallower indentation (new test case or section header)
        // signals the end of the detail block.

        if in_failure_detail {
            if let Some(after_info) = trimmed.strip_prefix("[info]") {
                if after_info.starts_with("    ") {
                    let detail = after_info.trim();
                    if !detail.is_empty() {
                        // Skip raw JVM stack frames — they add noise without signal.
                        // Keep Mockito "-> at" pointers and ScalaMock locations
                        // (they include the file:line reference).
                        let is_stack_frame = detail.starts_with("at ")
                            || detail.starts_with("...");
                        if !is_stack_frame {
                            if let Some(block) = failures.last_mut() {
                                if block.details.len() < 4 {
                                    block.details.push(detail.to_string());
                                }
                            }
                        }
                    }
                    continue;
                } else {
                    // Shallower indentation → back to normal test output
                    in_failure_detail = false;
                }
            } else {
                in_failure_detail = false;
            }
        }

        // --- [error] lines: collect failed suite names, drop sbt boilerplate ---

        if ERROR_RE.is_match(trimmed) {
            let text = trimmed.strip_prefix("[error] ").unwrap_or(trimmed).trim();
            if text.is_empty()
                || text.starts_with("Total time:")
                || text.contains("TestsFailedException")
                || text.contains("compileIncremental")
            {
                continue;
            }
            // "Failed tests:" header + class names → collect separately
            if text == "Failed tests:" {
                continue; // the header is implicit from context
            }
            if text.starts_with("com.") || text.starts_with("org.") || text.starts_with("  ") {
                failed_suites.push(text.trim_start().to_string());
            } else {
                error_lines.push(text.to_string());
            }
        }
    }

    // --- Fallback: no summary line found ---

    if !has_summary {
        if !error_lines.is_empty() {
            let mut result = String::from("sbt test: errors\n");
            result.push_str("═══════════════════════════════════════\n");
            for line in error_lines.iter().take(20) {
                result.push_str(&format!("  {}\n", truncate(line, 120)));
            }
            return result.trim().to_string();
        }
        if output.trim().is_empty() {
            return String::new();
        }
        return output.to_string();
    }

    let time_str = run_time_secs.map(|s| format!("{}s", s)).unwrap_or_default();

    // --- All passed ---

    if failed == 0 && canceled == 0 {
        let mut summary = format!("sbt test: {} passed", succeeded);
        if ignored > 0 {
            summary.push_str(&format!(", {} ignored", ignored));
        }
        if pending > 0 {
            summary.push_str(&format!(", {} pending", pending));
        }
        if suites > 0 {
            summary.push_str(&format!(" ({} suites", suites));
            if !time_str.is_empty() {
                summary.push_str(&format!(", {}", time_str));
            }
            summary.push(')');
        } else if !time_str.is_empty() {
            summary.push_str(&format!(" ({})", time_str));
        }
        return summary;
    }

    // --- Failures present ---

    let mut result = format!("sbt test: {} passed, {} failed", succeeded, failed);
    if canceled > 0 {
        result.push_str(&format!(", {} canceled", canceled));
    }
    if ignored > 0 {
        result.push_str(&format!(", {} ignored", ignored));
    }
    if !time_str.is_empty() {
        result.push_str(&format!(" ({})", time_str));
    }
    result.push('\n');
    result.push_str("═══════════════════════════════════════\n");

    for block in &failures {
        result.push_str(&format!("  [FAIL] {}\n", truncate(&block.name, 120)));
        for detail in &block.details {
            result.push_str(&format!("         {}\n", truncate(detail, 120)));
        }
    }

    // Failed suite class names (useful for navigation)
    if !failed_suites.is_empty() {
        result.push('\n');
        for suite in &failed_suites {
            result.push_str(&format!("  {}\n", suite));
        }
    }

    // Any remaining [error] lines (e.g. build-level errors)
    if !error_lines.is_empty() {
        result.push('\n');
        for line in error_lines.iter().take(10) {
            result.push_str(&format!("  {}\n", truncate(line, 120)));
        }
    }

    result.trim().to_string()
}

/// Filter SBT compile output.
///
/// On success: compact summary with source count and time.
/// On failure: show all [error] lines.
fn filter_sbt_compile(output: &str) -> String {
    // Nothing in, nothing out (Transparency: never emit tokens the command didn't).
    if output.trim().is_empty() {
        return String::new();
    }

    let mut source_count: u32 = 0;
    let mut total_time_secs: Option<u32> = None;
    let mut error_lines: Vec<String> = Vec::new();
    let mut has_success = false;

    for line in output.lines() {
        let trimmed = line.trim();

        // Count compiled sources
        if let Some(caps) = COMPILE_COUNT_RE.captures(trimmed) {
            source_count += caps[1].parse::<u32>().unwrap_or(0);
            continue;
        }

        // Parse success time
        if let Some(caps) = SUCCESS_TIME_RE.captures(trimmed) {
            total_time_secs = caps[1].parse().ok();
            has_success = true;
            continue;
        }

        // Collect [error] lines
        if ERROR_RE.is_match(trimmed) {
            let error_text = trimmed.strip_prefix("[error] ").unwrap_or(trimmed);
            if !error_text.is_empty() {
                error_lines.push(error_text.to_string());
            }
        }
    }

    // Compilation errors
    if !error_lines.is_empty() {
        let mut result = format!("sbt compile: {} errors\n", error_lines.len());
        result.push_str("═══════════════════════════════════════\n");

        for (i, error) in error_lines.iter().take(30).enumerate() {
            result.push_str(&format!("{}. {}\n", i + 1, truncate(error, 120)));
        }

        if error_lines.len() > 30 {
            result.push_str(&format!("\n... +{} more errors\n", error_lines.len() - 30));
        }

        return result.trim().to_string();
    }

    // Success
    if has_success || source_count > 0 {
        let mut summary = String::from("sbt compile: ");
        if source_count > 0 {
            summary.push_str(&format!("{} sources", source_count));
        } else {
            summary.push_str("Success");
        }
        if let Some(secs) = total_time_secs {
            summary.push_str(&format!(" ({}s)", secs));
        }
        return summary;
    }

    // Fallback: nothing recognized
    "sbt compile: Success".to_string()
}

/// Filter SBT run output — light filtering.
///
/// Strips SBT preamble noise, keeps actual program output.
fn filter_sbt_run(output: &str) -> String {
    let mut result_lines: Vec<&str> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip empty lines at the start
        if trimmed.is_empty() && result_lines.is_empty() {
            continue;
        }

        // Skip SBT noise lines
        if NOISE_RE.is_match(trimmed) {
            continue;
        }

        // Skip [info] Compiling lines
        if COMPILE_COUNT_RE.is_match(trimmed) {
            continue;
        }

        // Skip [info] running ... preamble
        if trimmed.starts_with("[info] running ") || trimmed.starts_with("[info] Running ") {
            continue;
        }

        // Strip [info] prefix from program output
        if let Some(content) = trimmed.strip_prefix("[info] ") {
            result_lines.push(content);
        } else if let Some(content) = trimmed.strip_prefix("[success] ") {
            // Skip success time line
            if content.starts_with("Total time:") {
                continue;
            }
            result_lines.push(content);
        } else if ERROR_RE.is_match(trimmed) {
            let error_text = trimmed.strip_prefix("[error] ").unwrap_or(trimmed);
            result_lines.push(error_text);
        } else {
            result_lines.push(trimmed);
        }
    }

    result_lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    // --- sbt test: all-pass ---

    #[test]
    fn test_filter_sbt_test_all_pass() {
        let input = include_str!("../../../tests/fixtures/sbt/sbt_test_pass.txt");
        let output = filter_sbt_test(input);

        assert!(output.starts_with("sbt test:"), "output: {}", output);
        assert!(output.contains("30 passed"));
        assert!(output.contains("2 ignored"));
        assert!(output.contains("5 suites"));
        assert!(output.contains("5s"));
        assert!(!output.contains('\n'), "all-pass output should be a single line");
    }

    #[test]
    fn test_filter_sbt_test_all_pass_token_savings() {
        let input = include_str!("../../../tests/fixtures/sbt/sbt_test_pass.txt");
        let output = filter_sbt_test(input);
        let savings = 100.0
            - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "sbt test (pass): expected >=60% savings, got {:.1}%",
            savings
        );
    }

    // --- sbt test: ScalaTest failures ---

    #[test]
    fn test_filter_sbt_test_with_failures() {
        let input = include_str!("../../../tests/fixtures/sbt/sbt_test_fail.txt");
        let output = filter_sbt_test(input);

        assert!(output.contains("15 passed"), "output: {}", output);
        assert!(output.contains("3 failed"));
        assert!(output.contains("[FAIL]"));
        // Detail lines from the fixture should appear
        assert!(
            output.contains("Expected ServiceException"),
            "missing failure detail: {}",
            output
        );
        assert!(output.contains("MyServiceSpec.scala:45"));
        assert!(output.contains("timed out"));
        assert!(output.contains("42 was not equal to 43"));
    }

    #[test]
    fn test_filter_sbt_test_failures_no_noise() {
        let input = include_str!("../../../tests/fixtures/sbt/sbt_test_fail.txt");
        let output = filter_sbt_test(input);

        // SBT boilerplate must be stripped
        assert!(!output.contains("welcome to sbt"));
        assert!(!output.contains("loading settings"));
        assert!(!output.contains("TestsFailedException"));
        assert!(!output.contains("Total time:"));
    }

    #[test]
    fn test_filter_sbt_test_fail_token_savings() {
        let input = include_str!("../../../tests/fixtures/sbt/sbt_test_fail.txt");
        let output = filter_sbt_test(input);
        let savings = 100.0
            - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 40.0,
            "sbt test (fail): expected >=40% savings, got {:.1}%",
            savings
        );
    }

    // --- sbt test: Mockito Scala verification failures ---

    #[test]
    fn test_filter_sbt_test_mockito_failure_details() {
        let input = include_str!("../../../tests/fixtures/sbt/sbt_test_mockito_fail.txt");
        let output = filter_sbt_test(input);

        assert!(output.contains("4 passed"), "output: {}", output);
        assert!(output.contains("2 failed"));
        // Mockito-specific detail lines must appear
        assert!(
            output.contains("WantedButNotInvoked"),
            "missing Mockito detail: {}",
            output
        );
        assert!(output.contains("Wanted but not invoked"));
        assert!(
            output.contains("TooManyActualInvocations"),
            "missing second Mockito failure: {}",
            output
        );
        // Pure JVM stack frames ("at com.example...") must be suppressed;
        // Mockito pointer lines ("-> at com.example...") may remain — they
        // carry the file:line reference that identifies the assertion site.
        assert!(
            !output.lines().any(|l| l.trim_start().starts_with("at com.")),
            "bare stack frame leaked into output: {}",
            output
        );
    }

    #[test]
    fn test_filter_sbt_test_mockito_token_savings() {
        let input = include_str!("../../../tests/fixtures/sbt/sbt_test_mockito_fail.txt");
        let output = filter_sbt_test(input);
        let savings = 100.0
            - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 40.0,
            "sbt test (mockito): expected >=40% savings, got {:.1}%",
            savings
        );
    }

    // --- sbt test: ScalaMock expectation failures ---

    #[test]
    fn test_filter_sbt_test_scalamock_failure_details() {
        let input = include_str!("../../../tests/fixtures/sbt/sbt_test_scalamock_fail.txt");
        let output = filter_sbt_test(input);

        assert!(output.contains("5 passed"), "output: {}", output);
        assert!(output.contains("2 failed"));
        // ScalaMock-specific detail lines must appear
        assert!(
            output.contains("Unexpected call"),
            "missing ScalaMock detail: {}",
            output
        );
        assert!(output.contains("Unsatisfied expectation"));
    }

    #[test]
    fn test_filter_sbt_test_scalamock_token_savings() {
        let input = include_str!("../../../tests/fixtures/sbt/sbt_test_scalamock_fail.txt");
        let output = filter_sbt_test(input);
        let savings = 100.0
            - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 40.0,
            "sbt test (scalamock): expected >=40% savings, got {:.1}%",
            savings
        );
    }

    // --- integration tests (it:test, IntegrationTest/test) ---

    #[test]
    fn test_filter_sbt_it_test_pass() {
        let input = include_str!("../../../tests/fixtures/sbt/sbt_it_test_pass.txt");
        let output = filter_sbt_test(input); // same filter as sbt test

        assert!(output.starts_with("sbt test:"), "output: {}", output);
        assert!(output.contains("5 passed"));
        assert!(output.contains("2 suites"));
        assert!(output.contains("18s"));
        assert!(!output.contains('\n'), "all-pass output should be a single line");
    }

    #[test]
    fn test_is_integration_test_cmd() {
        assert!(is_integration_test_cmd("it:test"));
        assert!(is_integration_test_cmd("IntegrationTest/test"));
        assert!(is_integration_test_cmd("integration-test/test"));
        assert!(is_integration_test_cmd("e2e/test"));
        assert!(is_integration_test_cmd("it:test"));
        assert!(!is_integration_test_cmd("test"));
        assert!(!is_integration_test_cmd("compile"));
        assert!(!is_integration_test_cmd("assembly"));
    }

    #[test]
    fn test_is_test_task() {
        assert!(is_test_task("testOnly"));
        assert!(is_test_task("testQuick"));
        assert!(is_test_task("Test/testOnly"));
        assert!(is_test_task("core/testOnly"));
        assert!(is_test_task("it:testOnly"));
        assert!(is_test_task("testOnly com.example.MySpec"));
        assert!(is_test_task("testOnly *CalcSpec"));
        assert!(is_test_task("core/testOnly com.example.MySpec"));
        assert!(!is_test_task("test"));
        assert!(!is_test_task("Test/test"));
        assert!(!is_test_task("testOnlyFoo"));
        assert!(!is_test_task("compile"));
        assert!(!is_test_task("clean; testOnly com.example.MySpec"));
    }

    // --- sbt test: munit format ---

    #[test]
    fn test_filter_sbt_test_munit_pass() {
        let input = include_str!("../../../tests/fixtures/sbt/sbt_test_munit_pass.txt");
        let output = filter_sbt_test(input);

        assert!(output.starts_with("sbt test:"), "output: {}", output);
        assert!(output.contains("12 passed"), "output: {}", output);
        assert!(!output.contains('\n'), "all-pass output should be a single line");
        assert!(!output.contains("parse error"), "output: {}", output);
    }

    #[test]
    fn test_filter_sbt_test_munit_fail() {
        let input = include_str!("../../../tests/fixtures/sbt/sbt_test_munit_fail.txt");
        let output = filter_sbt_test(input);

        assert!(output.contains("8 passed"), "output: {}", output);
        assert!(output.contains("2 failed"), "output: {}", output);
        assert!(output.contains("[FAIL]"), "output: {}", output);
        assert!(!output.contains("parse error"), "output: {}", output);
    }

    #[test]
    fn test_filter_sbt_test_munit_token_savings() {
        let input = include_str!("../../../tests/fixtures/sbt/sbt_test_munit_pass.txt");
        let output = filter_sbt_test(input);
        let savings = 100.0
            - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "sbt test (munit): expected >=60% savings, got {:.1}%",
            savings
        );
    }

    // --- sbt compile ---

    #[test]
    fn test_filter_sbt_compile_success() {
        let input = "[info] loading settings for project root from build.sbt ...\n\
                     [info] Compiling 15 Scala sources to /target/scala-2.13/classes ...\n\
                     [success] Total time: 12 s, completed Jan 15, 2025";
        let output = filter_sbt_compile(input);

        assert!(output.contains("sbt compile:"));
        assert!(output.contains("15 sources"));
        assert!(output.contains("12s"));
    }

    #[test]
    fn test_filter_sbt_compile_errors() {
        let input = include_str!("../../../tests/fixtures/sbt/sbt_compile_error.txt");
        let output = filter_sbt_compile(input);

        assert!(output.contains("sbt compile:"));
        assert!(output.contains("errors"));
        assert!(output.contains("type mismatch"));
        assert!(output.contains("not found: value"));
    }

    #[test]
    fn test_filter_sbt_compile_error_token_savings() {
        let input = include_str!("../../../tests/fixtures/sbt/sbt_compile_error.txt");
        let output = filter_sbt_compile(input);
        let savings = 100.0
            - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 30.0,
            "sbt compile (error): expected >=30% savings, got {:.1}%",
            savings
        );
    }

    // --- sbt run ---

    #[test]
    fn test_filter_sbt_run_strips_noise() {
        let input = "[info] welcome to sbt 1.9.7\n\
                     [info] loading settings for project root from build.sbt ...\n\
                     [info] set current project to myapp\n\
                     [info] running com.example.Main\n\
                     [info] Hello, World!\n\
                     [info] Server started on port 8080\n\
                     [success] Total time: 3 s, completed Jan 15, 2025";
        let output = filter_sbt_run(input);

        assert!(output.contains("Hello, World!"));
        assert!(output.contains("Server started on port 8080"));
        assert!(!output.contains("welcome to sbt"));
        assert!(!output.contains("loading settings"));
        assert!(!output.contains("set current project"));
        assert!(!output.contains("running com.example"));
        assert!(!output.contains("Total time:"));
    }

    // --- edge cases ---

    #[test]
    fn test_filter_sbt_test_empty_input() {
        // Empty command output must produce empty filtered output (guard::never_worse
        // invariant): RTK never emits tokens the underlying command didn't.
        assert!(filter_sbt_test("").is_empty());
    }

    #[test]
    fn test_filter_sbt_compile_empty_input() {
        assert!(filter_sbt_compile("").is_empty());
    }

    #[test]
    fn test_filter_sbt_run_empty_input() {
        assert!(filter_sbt_run("").is_empty());
    }
}
