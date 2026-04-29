//! Filters Vitest test output to show only failures.

use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;

use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::utils::{package_manager_exec, strip_ansi};
use crate::parser::{
    build_json_envelope, emit_degradation_warning, emit_passthrough_warning, extract_json_object,
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

pub fn run_test(command: &Commands, args: &[String], verbose: u8, json: bool) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let (framework, mut cmd) = match command {
        Commands::Vitest { .. } => {
            let framework = "vitest";
            let mut cmd = package_manager_exec(framework);
            cmd
                // Force non-watch mode
                .arg("run")
                // Enable JSON structured output
                .arg("--reporter=json");
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

    let result = exec_capture(&mut cmd).context(format!("Failed to run {}", framework))?;
    let combined = result.combined();

    // Parse output using VitestParser
    let parse_result = VitestParser::parse(&result.stdout);

    // R7: --json bypasses formatter, suppresses tier-warning stderr, emits stable envelope
    if json {
        let tool_name: &'static str = match framework {
            "vitest" => "vitest",
            "jest" => "jest",
            _ => "vitest",
        };
        let envelope = build_json_envelope(tool_name, parse_result, result.exit_code);
        let serialized = serde_json::to_string(&envelope)
            .context("Failed to serialize JSON envelope")?;
        println!("{}", serialized);

        timer.track(
            format!("{} run", framework).as_str(),
            format!("rtk {} run --json", framework).as_str(),
            &combined,
            &serialized,
        );

        return Ok(result.exit_code);
    }

    let mode = FormatMode::from_verbosity(verbose);

    let filtered = match parse_result {
        ParseResult::Full(data) => {
            if verbose > 0 {
                eprintln!("{} run (Tier 1: Full JSON parse)", framework);
            }
            data.format(mode)
        }
        ParseResult::Degraded(data, warnings) => {
            if verbose > 0 {
                emit_degradation_warning(framework, &warnings.join(", "));
            }
            data.format(mode)
        }
        ParseResult::Passthrough(raw) => {
            emit_passthrough_warning(framework, "All parsing tiers failed");
            raw
        }
    };

    if let Some(hint) =
        crate::core::tee::tee_and_hint(&combined, format!("{}_run", framework).as_str(), result.exit_code)
    {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        format!("{} run", framework).as_str(),
        format!("rtk {} run", framework).as_str(),
        &combined,
        &filtered,
    );

    if !result.success() {
        return Ok(result.exit_code);
    }
    Ok(0)
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

    // --- JSON envelope tests (--json global flag) ---

    use crate::parser::{build_json_envelope, JsonEnvelope};

    /// T1 (vitest): full-tier envelope round-trips with all fields populated.
    #[test]
    fn test_vitest_json_envelope_full_tier() {
        let json = r#"{
            "numTotalTests": 13,
            "numPassedTests": 13,
            "numFailedTests": 0,
            "numPendingTests": 0,
            "testResults": [],
            "startTime": 1000
        }"#;

        let parse_result = VitestParser::parse(json);
        let envelope = build_json_envelope("vitest", parse_result, 0);
        let serialized = serde_json::to_string(&envelope).expect("serialize");

        let value: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(value["tool"], "vitest");
        assert_eq!(value["tier"], "full");
        assert_eq!(value["exit"], 0);
        assert_eq!(value["data"]["total"], 13);
        assert_eq!(value["data"]["passed"], 13);
        assert!(value.get("warnings").is_none());
        assert!(value.get("raw").is_none());
    }

    /// T2 (vitest): JSON path preserves all failures (no take(5) truncation).
    /// Fixture has 11 failures so it crosses the existing format_compact boundary.
    #[test]
    fn test_vitest_json_envelope_includes_all_failures() {
        let mut assertion_results = String::new();
        for i in 0..11 {
            if i > 0 {
                assertion_results.push(',');
            }
            assertion_results.push_str(&format!(
                r#"{{"fullName": "test {}", "status": "failed", "failureMessages": ["boom {}"]}}"#,
                i, i
            ));
        }
        let json = format!(
            r#"{{
                "numTotalTests": 11,
                "numPassedTests": 0,
                "numFailedTests": 11,
                "numPendingTests": 0,
                "testResults": [{{"name": "spec.ts", "assertionResults": [{}]}}],
                "startTime": 1000
            }}"#,
            assertion_results
        );

        let parse_result = VitestParser::parse(&json);
        let envelope = build_json_envelope("vitest", parse_result, 1);
        let serialized = serde_json::to_string(&envelope).unwrap();

        let parsed: JsonEnvelope<TestResult> =
            serde_json::from_str(&serialized).expect("envelope deserialize");
        let data = parsed.data.expect("data present on full tier");
        assert_eq!(data.failed, 11);
        assert_eq!(data.failures.len(), 11, "all 11 failures must survive --json");
        for (i, failure) in data.failures.iter().enumerate() {
            assert_eq!(failure.test_name, format!("test {}", i));
        }
    }

    /// T3 (vitest): malformed input yields passthrough envelope with raw field.
    #[test]
    fn test_vitest_json_envelope_passthrough() {
        let parse_result = VitestParser::parse("not json at all and no test markers");
        let envelope = build_json_envelope("vitest", parse_result, 2);
        let serialized = serde_json::to_string(&envelope).unwrap();

        let value: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(value["tier"], "passthrough");
        assert_eq!(value["exit"], 2);
        assert!(!value["raw"].as_str().unwrap_or("").is_empty());
        assert!(value.get("data").is_none());
    }

    /// T4 (vitest): degraded tier (regex fallback) carries warnings array.
    #[test]
    fn test_vitest_json_envelope_degraded_with_warnings() {
        let text = r#"
 Test Files  2 passed (2)
      Tests  13 passed (13)
   Duration  450ms
        "#;

        let parse_result = VitestParser::parse(text);
        let envelope = build_json_envelope("vitest", parse_result, 0);
        let serialized = serde_json::to_string(&envelope).unwrap();

        let value: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(value["tier"], "degraded");
        assert!(value.get("data").is_some());
        let warnings = value["warnings"].as_array().expect("warnings array present");
        assert!(!warnings.is_empty(), "degraded tier must carry warnings");
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
}
