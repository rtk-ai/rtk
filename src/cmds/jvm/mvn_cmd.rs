use crate::core::tracking;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use std::process::Command;

lazy_static! {
    // Lines to always strip: download/upload progress noise
    static ref RE_DOWNLOADING: Regex =
        Regex::new(r"^\[INFO\] Downloading(?: from \S+)?:").unwrap();
    static ref RE_DOWNLOADED: Regex =
        Regex::new(r"^\[INFO\] Downloaded(?: from \S+)?:").unwrap();
    static ref RE_PROGRESS: Regex = Regex::new(r"^Progress \(\d+\):").unwrap();

    // Plugin separator lines: "[INFO] --- plugin:version:goal ..."
    static ref RE_SEPARATOR: Regex = Regex::new(r"^\[INFO\] ---").unwrap();

    // Empty [INFO] lines (just "[INFO]" with optional trailing whitespace)
    static ref RE_EMPTY_INFO: Regex = Regex::new(r"^\[INFO\]\s*$").unwrap();

    // Build result lines to always preserve
    static ref RE_BUILD_RESULT: Regex =
        Regex::new(r"BUILD (SUCCESS|FAILURE)").unwrap();

    // Tests run summary line (surefire output) — any line containing "Tests run:"
    static ref RE_TESTS_RUN: Regex = Regex::new(r"Tests run:").unwrap();

    // Tests run summary that has actual failures or errors (non-zero counts)
    static ref RE_TESTS_RUN_FAILURE: Regex =
        Regex::new(r"Tests run:.*?(?:Failures: [1-9]\d*|Errors: [1-9]\d*)").unwrap();

    // Per-class "Tests run:" line — ends with "- in com.example.XxxTest"
    // (as opposed to the global aggregate summary which has no such suffix)
    static ref RE_TESTS_RUN_PER_CLASS: Regex =
        Regex::new(r"Tests run:.*\s-\s+in\s+\S+$").unwrap();

    // Reactor summary header
    static ref RE_REACTOR_SUMMARY: Regex =
        Regex::new(r"^\[INFO\] Reactor Summary").unwrap();

    // Warning lines
    static ref RE_WARNING: Regex = Regex::new(r"^\[WARNING\]").unwrap();

    // Error lines
    static ref RE_ERROR: Regex = Regex::new(r"^\[ERROR\]").unwrap();

    // Surefire failure marker within test run output
    static ref RE_FAILURE_MARKER: Regex =
        Regex::new(r"<<<\s*(FAILURE|ERROR)").unwrap();

    // Stack trace lines (indented with whitespace + "at ")
    static ref RE_STACK_TRACE: Regex = Regex::new(r"^\s+at ").unwrap();

    // Exception / assertion line patterns in surefire output
    static ref RE_EXCEPTION_LINE: Regex =
        Regex::new(r"^(java\.|org\.|com\.|net\.|AssertionError|NullPointer|IllegalArgument|RuntimeException)").unwrap();
}

/// Determine whether args represent a test-oriented Maven invocation.
pub fn is_test_command(args: &[String]) -> bool {
    args.iter().any(|a| {
        matches!(
            a.as_str(),
            "test" | "verify" | "integration-test" | "surefire:test"
        )
    })
}

/// Filter Maven output, selecting lines relevant to the developer.
///
/// For test commands: show errors, surefire failure details, and final summary.
/// For build commands: show warnings, errors, and final BUILD line.
pub fn filter_mvn_output(output: &str, args: &[String]) -> String {
    let test_mode = is_test_command(args);
    let mut result: Vec<&str> = Vec::new();

    // Track whether we are inside a stack trace block so we can skip it.
    // We keep the first exception line but drop deep stack frames.
    let mut in_stack_trace = false;
    let mut stack_trace_lines_kept = 0;

    for line in output.lines() {
        // --- Universal strip rules (applied before any mode check) ---

        // Strip download / upload noise
        if RE_DOWNLOADING.is_match(line)
            || RE_DOWNLOADED.is_match(line)
            || RE_PROGRESS.is_match(line)
        {
            continue;
        }

        // Strip plugin separator banners "[INFO] ---"
        if RE_SEPARATOR.is_match(line) {
            continue;
        }

        // Strip empty [INFO] lines
        if RE_EMPTY_INFO.is_match(line) {
            continue;
        }

        // --- Always-preserve rules ---

        if RE_BUILD_RESULT.is_match(line) || RE_REACTOR_SUMMARY.is_match(line) {
            in_stack_trace = false;
            stack_trace_lines_kept = 0;
            result.push(line);
            continue;
        }

        // "Tests run:" summary lines:
        //   - In test mode: drop per-class lines that show no failures/errors
        //     (e.g. "[INFO] Tests run: 5, Failures: 0 … - in com.example.UserServiceTest")
        //     Keep per-class lines with failures and always keep the global aggregate summary.
        //   - In build mode: always keep.
        if RE_TESTS_RUN.is_match(line) {
            in_stack_trace = false;
            stack_trace_lines_kept = 0;
            let is_per_class_passing = test_mode
                && RE_TESTS_RUN_PER_CLASS.is_match(line)
                && !RE_TESTS_RUN_FAILURE.is_match(line);
            if !is_per_class_passing {
                result.push(line);
            }
            continue;
        }

        // --- Stack trace handling ---
        // Keep first 2 exception/stack lines for context, drop the rest.
        if in_stack_trace {
            if RE_STACK_TRACE.is_match(line) || RE_EXCEPTION_LINE.is_match(line) {
                if stack_trace_lines_kept < 2 {
                    result.push(line);
                    stack_trace_lines_kept += 1;
                }
                continue;
            } else {
                // Non-stack line ends the trace block
                in_stack_trace = false;
                stack_trace_lines_kept = 0;
            }
        }

        // --- Mode-specific rules ---
        if test_mode {
            // Show errors (includes surefire failures) and failure markers
            if RE_ERROR.is_match(line) || RE_FAILURE_MARKER.is_match(line) {
                result.push(line);
                // The lines that follow might be a stack trace
                in_stack_trace = true;
                stack_trace_lines_kept = 0;
                continue;
            }
        } else {
            // Build mode: show warnings and errors
            if RE_WARNING.is_match(line) || RE_ERROR.is_match(line) {
                result.push(line);
                continue;
            }
        }

        // Everything else is stripped
    }

    result.join("\n")
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("mvn");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: mvn {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run mvn. Is Maven installed? Try: brew install maven")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = filter_mvn_output(&stdout, args);

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });

    if let Some(hint) = crate::core::tee::tee_and_hint(&raw, "mvn", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    // Forward stderr (rare: Maven almost always uses stdout)
    if !stderr.trim().is_empty() {
        eprintln!("{}", stderr.trim());
    }

    timer.track(
        &format!("mvn {}", args.join(" ")),
        &format!("rtk mvn {}", args.join(" ")),
        &raw,
        &filtered,
    );

    // Preserve exit code for CI/CD
    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(exit_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    fn savings_pct(input: &str, output: &str) -> f64 {
        let in_tok = count_tokens(input);
        let out_tok = count_tokens(output);
        if in_tok == 0 {
            return 0.0;
        }
        100.0 - (out_tok as f64 / in_tok as f64 * 100.0)
    }

    // --- is_test_command ---

    #[test]
    fn test_is_test_command_test() {
        let args: Vec<String> = vec!["test".into()];
        assert!(is_test_command(&args));
    }

    #[test]
    fn test_is_test_command_verify() {
        let args: Vec<String> = vec!["verify".into()];
        assert!(is_test_command(&args));
    }

    #[test]
    fn test_is_test_command_integration_test() {
        let args: Vec<String> = vec!["integration-test".into()];
        assert!(is_test_command(&args));
    }

    #[test]
    fn test_is_test_command_build_is_not_test() {
        let args: Vec<String> = vec!["package".into(), "-DskipTests".into()];
        assert!(!is_test_command(&args));
    }

    // --- filter_mvn_output: test fixture ---

    #[test]
    fn test_filter_mvn_test_preserves_failures() {
        let input = include_str!("../../../tests/fixtures/mvn_test_raw.txt");
        let args: Vec<String> = vec!["test".into()];
        let output = filter_mvn_output(input, &args);

        // Must show the failure class and assertion
        assert!(
            output.contains("ProductServiceTest"),
            "Should contain failing test class"
        );
        assert!(
            output.contains("expected:<150.0>"),
            "Should contain assertion detail"
        );
        // Must show the error class
        assert!(
            output.contains("OrderServiceTest"),
            "Should contain erroring test class"
        );
        // Must show final BUILD FAILURE
        assert!(
            output.contains("BUILD FAILURE"),
            "Should contain BUILD FAILURE"
        );
        // Must show Tests run summary
        assert!(
            output.contains("Tests run:"),
            "Should contain Tests run summary"
        );
    }

    #[test]
    fn test_filter_mvn_test_strips_noise() {
        let input = include_str!("../../../tests/fixtures/mvn_test_raw.txt");
        let args: Vec<String> = vec!["test".into()];
        let output = filter_mvn_output(input, &args);

        // Plugin separator lines must be gone
        assert!(
            !output.contains("[INFO] ---"),
            "Should strip plugin separator lines"
        );
        // No download noise in the test fixture, but ensure empty [INFO] lines are stripped
        assert!(
            !output.lines().any(|l| l.trim() == "[INFO]"),
            "Should strip empty [INFO] lines"
        );
    }

    #[test]
    fn test_filter_mvn_test_token_savings() {
        let input = include_str!("../../../tests/fixtures/mvn_test_raw.txt");
        let args: Vec<String> = vec!["test".into()];
        let output = filter_mvn_output(input, &args);

        let savings = savings_pct(input, &output);
        assert!(
            savings >= 80.0,
            "Expected >=80% token savings for mvn test, got {:.1}%",
            savings
        );
    }

    // --- filter_mvn_output: build fixture ---

    #[test]
    fn test_filter_mvn_build_preserves_warnings_and_result() {
        let input = include_str!("../../../tests/fixtures/mvn_build_raw.txt");
        let args: Vec<String> = vec!["package".into()];
        let output = filter_mvn_output(input, &args);

        // Warnings must be preserved
        assert!(
            output.contains("[WARNING]"),
            "Should preserve WARNING lines"
        );
        assert!(
            output.contains("unchecked or unsafe operations"),
            "Should preserve warning details"
        );
        // Build result must be present
        assert!(
            output.contains("BUILD SUCCESS"),
            "Should contain BUILD SUCCESS"
        );
    }

    #[test]
    fn test_filter_mvn_build_strips_download_noise() {
        let input = include_str!("../../../tests/fixtures/mvn_build_raw.txt");
        let args: Vec<String> = vec!["package".into()];
        let output = filter_mvn_output(input, &args);

        assert!(
            !output.contains("Downloading from"),
            "Should strip Downloading lines"
        );
        assert!(
            !output.contains("Downloaded from"),
            "Should strip Downloaded lines"
        );
        assert!(
            !output.contains("Progress ("),
            "Should strip Progress lines"
        );
        assert!(
            !output.contains("[INFO] ---"),
            "Should strip plugin separator lines"
        );
    }

    #[test]
    fn test_filter_mvn_build_token_savings() {
        let input = include_str!("../../../tests/fixtures/mvn_build_raw.txt");
        let args: Vec<String> = vec!["package".into()];
        let output = filter_mvn_output(input, &args);

        let savings = savings_pct(input, &output);
        assert!(
            savings >= 80.0,
            "Expected >=80% token savings for mvn package, got {:.1}%",
            savings
        );
    }

    // --- edge cases ---

    #[test]
    fn test_filter_mvn_empty_input() {
        let args: Vec<String> = vec!["test".into()];
        let output = filter_mvn_output("", &args);
        assert!(output.is_empty(), "Empty input should produce empty output");
    }

    #[test]
    fn test_filter_mvn_all_pass() {
        let input = "[INFO] Tests run: 10, Failures: 0, Errors: 0, Skipped: 0\n\
                     [INFO] BUILD SUCCESS\n";
        let args: Vec<String> = vec!["test".into()];
        let output = filter_mvn_output(input, &args);
        assert!(output.contains("BUILD SUCCESS"));
        assert!(output.contains("Tests run:"));
    }

    #[test]
    fn test_filter_strips_separator_lines() {
        let input =
            "[INFO] --- maven-compiler-plugin:3.11.0:compile (default-compile) @ myapp ---\n\
                     [INFO] BUILD SUCCESS\n";
        let args: Vec<String> = vec!["package".into()];
        let output = filter_mvn_output(input, &args);
        assert!(!output.contains("[INFO] ---"), "Separator must be stripped");
        assert!(output.contains("BUILD SUCCESS"));
    }

    #[test]
    fn test_filter_strips_download_lines() {
        let input = "[INFO] Downloading from central: https://repo.maven.apache.org/maven2/foo.jar\n\
                     [INFO] Downloaded from central: https://repo.maven.apache.org/maven2/foo.jar (10 kB at 50 kB/s)\n\
                     Progress (1): foo.jar (5/10 kB)\n\
                     [INFO] BUILD SUCCESS\n";
        let args: Vec<String> = vec!["package".into()];
        let output = filter_mvn_output(input, &args);
        assert!(!output.contains("Downloading"));
        assert!(!output.contains("Downloaded"));
        assert!(!output.contains("Progress ("));
        assert!(output.contains("BUILD SUCCESS"));
    }
}
