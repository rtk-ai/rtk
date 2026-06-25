//! Filters Vitest test output to show only failures.

use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;

use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::utils::{package_manager_exec, strip_ansi};
use crate::parser::{
    emit_degradation_warning, emit_passthrough_warning, extract_json_object, truncate_output,
    truncate_passthrough, FormatMode, OutputParser, ParseResult, TestFailure, TestResult,
    TokenFormatter,
};
use crate::Commands;

/// Vitest JSON output structures (tool-specific format)
#[derive(Debug, Deserialize)]
struct VitestJsonOutput {
    #[serde(rename = "testResults")]
    test_results: Vec<VitestTestFile>,
    #[serde(rename = "numTotalTests")]
    num_total_tests: usize,
    #[serde(rename = "numPassedTests")]
    num_passed_tests: usize,
    #[serde(rename = "numFailedTests")]
    num_failed_tests: usize,
    #[serde(rename = "numPendingTests", default)]
    num_pending_tests: usize,
}

#[derive(Debug, Deserialize)]
struct VitestTestFile {
    name: String,
    #[serde(rename = "assertionResults")]
    assertion_results: Vec<VitestTest>,
}

#[derive(Debug, Deserialize)]
struct VitestTest {
    #[serde(rename = "fullName")]
    full_name: String,
    status: String,
    #[serde(rename = "failureMessages")]
    failure_messages: Vec<String>,
}

/// Parser for Vitest JSON output
pub struct VitestParser;

impl OutputParser for VitestParser {
    type Output = TestResult;

    fn parse(input: &str) -> ParseResult<TestResult> {
        // Tier 1: Try JSON parsing (with extraction fallback for pnpm/dotenv prefixes)
        let json_result = serde_json::from_str::<VitestJsonOutput>(input).or_else(|first_err| {
            // Fallback: Try extracting JSON object from prefixed output
            if let Some(extracted) = extract_json_object(input) {
                serde_json::from_str::<VitestJsonOutput>(extracted)
            } else {
                Err(first_err)
            }
        });

        match json_result {
            Ok(json) => {
                let failures = extract_failures_from_json(&json);

                let result = TestResult {
                    total: json.num_total_tests,
                    passed: json.num_passed_tests,
                    failed: json.num_failed_tests,
                    skipped: json.num_pending_tests,
                    duration_ms: None,
                    failures,
                };

                ParseResult::Full(result)
            }
            Err(e) => {
                // Tier 2: Try regex extraction (only fires if user overrides --reporter flag)
                match extract_stats_regex(input) {
                    Some(result) => {
                        ParseResult::Degraded(result, vec![format!("JSON parse failed: {}", e)])
                    }
                    None => {
                        // Tier 3: Passthrough
                        ParseResult::Passthrough(truncate_passthrough(input))
                    }
                }
            }
        }
    }
}

/// Extract failures from JSON structure
fn extract_failures_from_json(json: &VitestJsonOutput) -> Vec<TestFailure> {
    let mut failures = Vec::new();

    for file in &json.test_results {
        for test in &file.assertion_results {
            if test.status == "failed" {
                let error_message = test.failure_messages.join("\n");
                failures.push(TestFailure {
                    test_name: test.full_name.clone(),
                    file_path: file.name.clone(),
                    error_message,
                    stack_trace: None,
                });
            }
        }
    }

    failures
}

/// Tier 2: Extract test statistics using regex (degraded mode)
fn extract_stats_regex(output: &str) -> Option<TestResult> {
    lazy_static::lazy_static! {
        static ref TEST_FILES_RE: Regex = Regex::new(
            r"Test Files\s+(?:(\d+)\s+failed\s+\|\s+)?(\d+)\s+passed"
        ).unwrap();
        static ref TESTS_RE: Regex = Regex::new(
            r"Tests\s+(?:(\d+)\s+failed\s+\|\s+)?(\d+)\s+passed"
        ).unwrap();
        static ref DURATION_RE: Regex = Regex::new(
            r"Duration\s+([\d.]+)(ms|s)"
        ).unwrap();
    }

    let clean_output = strip_ansi(output);

    let mut passed = 0;
    let mut failed = 0;
    let mut total = 0;

    // Parse test counts
    if let Some(caps) = TESTS_RE.captures(&clean_output) {
        if let Some(fail_str) = caps.get(1) {
            failed = fail_str.as_str().parse().unwrap_or(0);
        }
        if let Some(pass_str) = caps.get(2) {
            passed = pass_str.as_str().parse().unwrap_or(0);
        }
        total = passed + failed;
    }

    // Parse duration
    let duration_ms = DURATION_RE.captures(&clean_output).and_then(|caps| {
        let value: f64 = caps[1].parse().ok()?;
        let unit = &caps[2];
        Some(if unit == "ms" {
            value as u64
        } else {
            (value * 1000.0) as u64
        })
    });

    // Only return if we found valid data
    if total > 0 {
        Some(TestResult {
            total,
            passed,
            failed,
            skipped: 0,
            duration_ms,
            failures: extract_failures_regex(&clean_output),
        })
    } else {
        None
    }
}

/// Extract failures using regex
fn extract_failures_regex(output: &str) -> Vec<TestFailure> {
    let mut failures = Vec::new();
    let lines: Vec<&str> = output.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        if line.contains("[x]") || line.contains("FAIL") {
            let mut error_lines = vec![line.to_string()];
            i += 1;

            // Collect subsequent indented lines
            while i < lines.len() && lines[i].starts_with("  ") {
                error_lines.push(lines[i].trim().to_string());
                i += 1;
            }

            if !error_lines.is_empty() {
                failures.push(TestFailure {
                    test_name: error_lines[0].clone(),
                    file_path: String::new(),
                    error_message: error_lines[1..].join("\n"),
                    stack_trace: None,
                });
            }
        } else {
            i += 1;
        }
    }

    failures
}

pub fn run_test(command: &Commands, args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let mut passthrough_requested = false;

    let (framework, mut cmd) = match command {
        Commands::Vitest { .. } => {
            let framework = "vitest";
            let mut cmd = package_manager_exec(framework);
            let effective_args = build_vitest_effective_args(args);
            passthrough_requested = effective_args.passthrough;
            cmd.args(effective_args.args);
            (framework, cmd)
        }
        Commands::Jest { .. } => {
            let framework = "jest";
            let mut cmd = package_manager_exec(framework);
            cmd
                // Force non-watch mode
                .arg("--no-watch")
                // Enable JSON structured output
                .arg("--json");
            (framework, cmd)
        }
        _ => unreachable!(),
    };

    if !matches!(command, Commands::Vitest { .. }) {
        for arg in args {
            if arg == "run"
                || arg.starts_with("--json")
                || arg.starts_with("--reporter")
                || arg.starts_with("--watch")
            {
                continue;
            }
            cmd.arg(arg);
        }
    }

    let result = exec_capture(&mut cmd).context(format!("Failed to run {}", framework))?;
    let combined = result.combined();

    let filtered = format_test_output(
        framework,
        &result.stdout,
        &combined,
        passthrough_requested,
        verbose,
    );
    let tee_label = format!("{}_run", framework);

    let rendered = render_test_output(&filtered, &combined, &tee_label, result.exit_code);
    let shown = crate::core::runner::emit_guarded(&rendered, None, &combined);

    timer.track(
        format!("{} run", framework).as_str(),
        format!("rtk {} run", framework).as_str(),
        &combined,
        &shown,
    );

    if !result.success() {
        return Ok(result.exit_code);
    }
    Ok(0)
}

struct EffectiveVitestArgs {
    args: Vec<String>,
    passthrough: bool,
}

struct FormattedTestOutput {
    text: String,
    truncated: bool,
}

impl FormattedTestOutput {
    fn new(text: String) -> Self {
        Self {
            text,
            truncated: false,
        }
    }

    fn truncated(text: String) -> Self {
        Self {
            text,
            truncated: true,
        }
    }
}

fn build_vitest_effective_args(args: &[String]) -> EffectiveVitestArgs {
    let passthrough = has_explicit_vitest_reporter(args);
    let mut effective = vec!["run".to_string()];

    if !passthrough {
        effective.push("--reporter=json".to_string());
    }

    for arg in args {
        if should_skip_vitest_arg(arg) {
            continue;
        }
        effective.push(arg.clone());
    }

    EffectiveVitestArgs {
        args: effective,
        passthrough,
    }
}

fn has_explicit_vitest_reporter(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg == "--reporter" || arg.starts_with("--reporter="))
}

fn should_skip_vitest_arg(arg: &str) -> bool {
    arg == "run" || arg.starts_with("--json") || arg.starts_with("--watch")
}

fn format_test_output(
    framework: &str,
    stdout: &str,
    combined: &str,
    passthrough_requested: bool,
    verbose: u8,
) -> FormattedTestOutput {
    if passthrough_requested {
        return format_passthrough_output(combined);
    }

    let parse_result = VitestParser::parse(stdout);
    let mode = FormatMode::from_verbosity(verbose);
    match parse_result {
        ParseResult::Full(data) => {
            if verbose > 0 {
                eprintln!("{} run (Tier 1: Full JSON parse)", framework);
            }
            FormattedTestOutput::new(data.format(mode))
        }
        ParseResult::Degraded(data, warnings) => {
            if verbose > 0 {
                emit_degradation_warning(framework, &warnings.join(", "));
            }
            FormattedTestOutput::new(data.format(mode))
        }
        ParseResult::Passthrough(_) => {
            emit_passthrough_warning(framework, "All parsing tiers failed");
            format_passthrough_output(stdout)
        }
    }
}

fn format_passthrough_output(raw: &str) -> FormattedTestOutput {
    let max_chars = crate::core::config::limits().passthrough_max_chars;
    format_passthrough_output_with_limit(raw, max_chars)
}

fn format_passthrough_output_with_limit(raw: &str, max_chars: usize) -> FormattedTestOutput {
    let text = truncate_output(raw, max_chars);

    if raw.chars().count() > max_chars {
        FormattedTestOutput::truncated(text)
    } else {
        FormattedTestOutput::new(text)
    }
}

fn render_test_output(
    filtered: &FormattedTestOutput,
    raw: &str,
    tee_label: &str,
    exit_code: i32,
) -> String {
    render_test_output_with_hints(
        filtered,
        raw,
        tee_label,
        exit_code,
        crate::core::tee::force_tee_hint,
        crate::core::tee::tee_and_hint,
    )
}

fn render_test_output_with_hints<F, T>(
    filtered: &FormattedTestOutput,
    raw: &str,
    tee_label: &str,
    exit_code: i32,
    force_hint: F,
    tee_hint: T,
) -> String
where
    F: FnOnce(&str, &str) -> Option<String>,
    T: FnOnce(&str, &str, i32) -> Option<String>,
{
    let hint = if filtered.truncated {
        force_hint(raw, tee_label)
    } else {
        tee_hint(raw, tee_label, exit_code)
    };

    match hint {
        Some(hint) => format!("{}\n{}", filtered.text, hint),
        None => filtered.text.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn test_vitest_parser_json() {
        let json = r#"{
            "numTotalTests": 13,
            "numPassedTests": 13,
            "numFailedTests": 0,
            "numPendingTests": 0,
            "testResults": [],
            "startTime": 1000
        }"#;

        let result = VitestParser::parse(json);
        assert_eq!(result.tier(), 1);
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.total, 13);
        assert_eq!(data.passed, 13);
        assert_eq!(data.failed, 0);
        assert_eq!(data.duration_ms, None);
    }

    #[test]
    fn test_vitest_parser_regex_fallback() {
        let text = r#"
 Test Files  2 passed (2)
      Tests  13 passed (13)
   Duration  450ms
        "#;

        let result = VitestParser::parse(text);
        assert_eq!(result.tier(), 2); // Degraded
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.passed, 13);
        assert_eq!(data.failed, 0);
    }

    #[test]
    fn test_vitest_parser_passthrough() {
        let invalid = "random output with no structure";
        let result = VitestParser::parse(invalid);
        assert_eq!(result.tier(), 3); // Passthrough
        assert!(!result.is_ok());
    }

    #[test]
    fn test_strip_ansi() {
        let input = "\x1b[32m✓\x1b[0m test passed";
        let output = strip_ansi(input);
        assert_eq!(output, "✓ test passed");
        assert!(!output.contains("\x1b"));
    }

    #[test]
    fn test_vitest_parser_with_pnpm_prefix() {
        let input = r#"
Scope: all 6 workspace projects
 WARN  deprecated inflight@1.0.6: This module is not supported

{"numTotalTests": 13, "numPassedTests": 13, "numFailedTests": 0, "numPendingTests": 0, "testResults": [], "startTime": 1000}
"#;
        let result = VitestParser::parse(input);
        assert_eq!(result.tier(), 1, "Should succeed with Tier 1 (full parse)");
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.total, 13);
        assert_eq!(data.passed, 13);
        assert_eq!(data.failed, 0);
    }

    #[test]
    fn test_vitest_parser_with_dotenv_prefix() {
        let input = r#"[dotenv] Loading environment variables from .env
[dotenv] Injected 5 variables

{"numTotalTests": 5, "numPassedTests": 4, "numFailedTests": 1, "numPendingTests": 0, "testResults": [], "startTime": 2000}
"#;
        let result = VitestParser::parse(input);
        assert_eq!(result.tier(), 1, "Should succeed with Tier 1 (full parse)");
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.total, 5);
        assert_eq!(data.passed, 4);
        assert_eq!(data.failed, 1);
        assert_eq!(data.duration_ms, None);
    }

    #[test]
    fn test_vitest_parser_with_nested_json() {
        let input = r#"prefix text
{"numTotalTests": 2, "numPassedTests": 2, "numFailedTests": 0, "numPendingTests": 0, "testResults": [{"name": "test.js", "assertionResults": [{"fullName": "nested test", "status": "passed", "failureMessages": []}]}], "startTime": 1000}
"#;
        let result = VitestParser::parse(input);
        assert_eq!(result.tier(), 1, "Should succeed with Tier 1 (full parse)");
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.total, 2);
        assert_eq!(data.passed, 2);
    }

    #[test]
    fn test_vitest_effective_args_inject_json_reporter_by_default() {
        let effective = build_vitest_effective_args(&args(&["run", "constants.test.ts", "--watch"]));

        assert!(!effective.passthrough);
        assert_eq!(
            effective.args,
            args(&["run", "--reporter=json", "constants.test.ts"])
        );
    }

    #[test]
    fn test_vitest_effective_args_preserve_explicit_reporter_equals() {
        let effective =
            build_vitest_effective_args(&args(&["constants.test.ts", "--reporter=verbose"]));

        assert!(effective.passthrough);
        assert_eq!(
            effective.args,
            args(&["run", "constants.test.ts", "--reporter=verbose"])
        );
    }

    #[test]
    fn test_vitest_effective_args_preserve_explicit_reporter_value() {
        let effective =
            build_vitest_effective_args(&args(&["run", "constants.test.ts", "--reporter", "verbose"]));

        assert!(effective.passthrough);
        assert_eq!(
            effective.args,
            args(&["run", "constants.test.ts", "--reporter", "verbose"])
        );
    }

    #[test]
    fn test_vitest_explicit_reporter_keeps_verbose_output() {
        let output = r#"
 ✓ constants/publicPaths.test.ts > public paths > keeps docs path
 ✓ constants/publicPaths.test.ts > public paths > keeps app path

 Test Files  1 passed (1)
      Tests  2 passed (2)
   Duration  450ms
"#;

        let filtered = format_test_output("vitest", output, output, true, 0);

        assert!(filtered.text.contains("keeps docs path"));
        assert!(filtered.text.contains("keeps app path"));
        assert!(filtered.text.contains("Tests  2 passed"));
        assert!(!filtered.truncated);
    }

    #[test]
    fn test_vitest_explicit_reporter_truncated_output_adds_recovery_hint() {
        let output = format!(
            "{}\n Test Files  1 passed (1)\n      Tests  80 passed (80)\n",
            " ✓ constants/publicPaths.test.ts > public paths > keeps verbose case\n".repeat(80)
        );

        let filtered = format_passthrough_output_with_limit(&output, 200);
        let rendered = render_test_output_with_hints(
            &filtered,
            &output,
            "vitest_run",
            0,
            |raw, label| {
                assert_eq!(raw, output);
                assert_eq!(label, "vitest_run");
                Some("[full output: /tmp/vitest_run.log]".to_string())
            },
            |_, _, _| Some("[full output: wrong-path.log]".to_string()),
        );

        assert!(filtered.truncated);
        assert!(rendered.contains("[RTK:PASSTHROUGH] Output truncated"));
        assert!(rendered.contains("[full output: /tmp/vitest_run.log]"));
        assert!(!rendered.contains("wrong-path.log"));
    }
}
