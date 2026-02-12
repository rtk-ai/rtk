use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;

use crate::parser::{
    emit_degradation_warning, emit_passthrough_warning, extract_json_object, truncate_output,
    FormatMode, OutputParser, ParseResult, TestFailure, TestResult, TokenFormatter,
};
use crate::tracking;
use crate::utils::{package_manager_exec, strip_ansi};

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
    #[serde(rename = "startTime")]
    start_time: Option<u64>,
    #[serde(rename = "endTime")]
    end_time: Option<u64>,
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
                let duration_ms = match (json.start_time, json.end_time) {
                    (Some(start), Some(end)) => Some(end.saturating_sub(start)),
                    _ => None,
                };

                let result = TestResult {
                    total: json.num_total_tests,
                    passed: json.num_passed_tests,
                    failed: json.num_failed_tests,
                    skipped: json.num_pending_tests,
                    duration_ms,
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
                        ParseResult::Passthrough(truncate_output(input, 500))
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
        if line.contains('✗') || line.contains("FAIL") {
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
    let timer = tracking::TimedExecution::start();

    let mut cmd = package_manager_exec("vitest");
    cmd.arg("run"); // Force non-watch mode

    // Add JSON reporter for structured output
    cmd.arg("--reporter=json");

    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run vitest")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);

    // Parse output using VitestParser
    let parse_result = VitestParser::parse(&stdout);
    let mode = FormatMode::from_verbosity(verbose);

    let filtered = match parse_result {
        ParseResult::Full(data) => {
            if verbose > 0 {
                eprintln!("vitest run (Tier 1: Full JSON parse)");
            }
            data.format(mode)
        }
        ParseResult::Degraded(data, warnings) => {
            if verbose > 0 {
                emit_degradation_warning("vitest", &warnings.join(", "));
            }
            data.format(mode)
        }
        ParseResult::Passthrough(raw) => {
            emit_passthrough_warning("vitest", "All parsing tiers failed");
            raw
        }
    };

    println!("{}", filtered);

    timer.track("vitest run", "rtk vitest run", &combined, &filtered);

    // Propagate original exit code
    std::process::exit(output.status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vitest_parser_json() {
        let json = r#"{
            "numTotalTests": 13,
            "numPassedTests": 13,
            "numFailedTests": 0,
            "numPendingTests": 0,
            "testResults": [],
            "startTime": 1000,
            "endTime": 1450
        }"#;

        let result = VitestParser::parse(json);
        assert_eq!(result.tier(), 1);
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.total, 13);
        assert_eq!(data.passed, 13);
        assert_eq!(data.failed, 0);
        assert_eq!(data.duration_ms, Some(450));
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

{"numTotalTests": 13, "numPassedTests": 13, "numFailedTests": 0, "numPendingTests": 0, "testResults": [], "startTime": 1000, "endTime": 1450}
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

{"numTotalTests": 5, "numPassedTests": 4, "numFailedTests": 1, "numPendingTests": 0, "testResults": [], "startTime": 2000, "endTime": 2300}
"#;
        let result = VitestParser::parse(input);
        assert_eq!(result.tier(), 1, "Should succeed with Tier 1 (full parse)");
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.total, 5);
        assert_eq!(data.passed, 4);
        assert_eq!(data.failed, 1);
        assert_eq!(data.duration_ms, Some(300));
    }

    #[test]
    fn test_vitest_parser_with_nested_json() {
        let input = r#"prefix text
{"numTotalTests": 2, "numPassedTests": 2, "numFailedTests": 0, "numPendingTests": 0, "testResults": [{"name": "test.js", "assertionResults": [{"fullName": "nested test", "status": "passed", "failureMessages": []}]}], "startTime": 1000, "endTime": 1100}
"#;
        let result = VitestParser::parse(input);
        assert_eq!(result.tier(), 1, "Should succeed with Tier 1 (full parse)");
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.total, 2);
        assert_eq!(data.passed, 2);
    }
}
