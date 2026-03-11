use crate::parser::{
    emit_degradation_warning, emit_passthrough_warning, truncate_output,
    FormatMode, OutputParser, ParseResult, TestFailure, TestResult, TokenFormatter,
};
use crate::tracking;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::process::Command;

#[derive(Debug, Deserialize)]
struct RspecJsonOutput {
    examples: Vec<RspecExample>,
    summary: RspecSummary,
    #[serde(default)]
    summary_line: String,
}

#[derive(Debug, Deserialize)]
struct RspecExample {
    id: String,
    #[serde(default)]
    description: String,
    full_description: String,
    status: String,
    file_path: String,
    line_number: usize,
    #[serde(default)]
    run_time: f64,
    #[serde(default)]
    pending_message: Option<String>,
    #[serde(default)]
    exception: Option<RspecException>,
}

#[derive(Debug, Deserialize)]
struct RspecException {
    class: String,
    message: String,
    #[serde(default)]
    backtrace: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RspecSummary {
    example_count: usize,
    failure_count: usize,
    pending_count: usize,
    #[serde(default)]
    errors_outside_of_examples_count: usize,
    duration: f64,
}

pub struct RspecParser;

impl OutputParser for RspecParser {
    type Output = TestResult;

    fn parse(input: &str) -> ParseResult<TestResult> {
        match serde_json::from_str::<RspecJsonOutput>(input) {
            Ok(json) => {
                let failures = extract_failures_from_json(&json);
                let passed = json.summary.example_count
                    .saturating_sub(json.summary.failure_count)
                    .saturating_sub(json.summary.pending_count);
                let failed = json.summary.failure_count
                    + json.summary.errors_outside_of_examples_count;

                let result = TestResult {
                    total: json.summary.example_count,
                    passed,
                    failed,
                    skipped: json.summary.pending_count,
                    duration_ms: Some((json.summary.duration * 1000.0) as u64),
                    failures,
                };

                ParseResult::Full(result)
            }
            Err(e) => {
                match extract_stats_regex(input) {
                    Some(result) => {
                        ParseResult::Degraded(result, vec![format!("JSON parse failed: {}", e)])
                    }
                    None => {
                        ParseResult::Passthrough(truncate_output(input, 500))
                    }
                }
            }
        }
    }
}

fn extract_failures_from_json(json: &RspecJsonOutput) -> Vec<TestFailure> {
    json.examples
        .iter()
        .filter(|e| e.status == "failed")
        .map(|e| {
            let error_message = e
                .exception
                .as_ref()
                .map(|ex| ex.message.clone())
                .unwrap_or_else(|| "Test failed".to_string());

            let stack_trace = e.exception.as_ref().and_then(|ex| {
                if ex.backtrace.is_empty() {
                    None
                } else {
                    Some(ex.backtrace.join("\n"))
                }
            });

            TestFailure {
                test_name: e.full_description.clone(),
                file_path: format!("{} | rspec {}", e.file_path, e.id),
                error_message,
                stack_trace,
            }
        })
        .collect()
}

fn extract_stats_regex(output: &str) -> Option<TestResult> {
    lazy_static::lazy_static! {
        static ref SUMMARY_RE: regex::Regex = regex::Regex::new(
            r"(\d+) examples?, (\d+) failures?(?:, (\d+) pending)?"
        ).unwrap();
    }

    if let Some(caps) = SUMMARY_RE.captures(output) {
        let total: usize = caps.get(1)?.as_str().parse().ok()?;
        let failed: usize = caps.get(2)?.as_str().parse().ok()?;
        let skipped: usize = caps
            .get(3)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(0);
        let passed = total.saturating_sub(failed).saturating_sub(skipped);

        Some(TestResult {
            total,
            passed,
            failed,
            skipped,
            duration_ms: extract_duration(output),
            failures: Vec::new(),
        })
    } else {
        None
    }
}

fn extract_duration(output: &str) -> Option<u64> {
    lazy_static::lazy_static! {
        static ref DURATION_RE: regex::Regex = regex::Regex::new(
            r"Finished in ([\d.]+) seconds?"
        ).unwrap();
    }

    DURATION_RE.captures(output).and_then(|caps| {
        let secs: f64 = caps.get(1)?.as_str().parse().ok()?;
        Some((secs * 1000.0) as u64)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rspec_parser_json_all_pass() {
        let json = r#"{
            "examples": [
                {
                    "id": "./spec/models/user_spec.rb[1:1]",
                    "description": "returns true for valid users",
                    "full_description": "User#valid? returns true for valid users",
                    "status": "passed",
                    "file_path": "./spec/models/user_spec.rb",
                    "line_number": 5,
                    "run_time": 0.001,
                    "pending_message": null
                }
            ],
            "summary": {
                "duration": 0.05,
                "example_count": 1,
                "failure_count": 0,
                "pending_count": 0,
                "errors_outside_of_examples_count": 0
            },
            "summary_line": "1 example, 0 failures"
        }"#;

        let result = RspecParser::parse(json);
        assert_eq!(result.tier(), 1);
        assert!(result.is_ok());
        let data = result.unwrap();
        assert_eq!(data.total, 1);
        assert_eq!(data.passed, 1);
        assert_eq!(data.failed, 0);
        assert_eq!(data.skipped, 0);
        assert_eq!(data.duration_ms, Some(50));
    }

    #[test]
    fn test_rspec_parser_json_with_failures() {
        let json = r#"{
            "examples": [
                {
                    "id": "./spec/models/user_spec.rb[1:1]",
                    "description": "is valid",
                    "full_description": "User#valid? is valid",
                    "status": "passed",
                    "file_path": "./spec/models/user_spec.rb",
                    "line_number": 5,
                    "run_time": 0.001
                },
                {
                    "id": "./spec/models/user_spec.rb[1:2]",
                    "description": "saves to database",
                    "full_description": "User#save saves to database",
                    "status": "failed",
                    "file_path": "./spec/models/user_spec.rb",
                    "line_number": 15,
                    "run_time": 0.002,
                    "exception": {
                        "class": "RSpec::Expectations::ExpectationNotMetError",
                        "message": "expected true, got false",
                        "backtrace": ["./spec/models/user_spec.rb:16:in `block (2 levels)'"]
                    }
                }
            ],
            "summary": {
                "duration": 0.05,
                "example_count": 2,
                "failure_count": 1,
                "pending_count": 0,
                "errors_outside_of_examples_count": 0
            },
            "summary_line": "2 examples, 1 failure"
        }"#;

        let result = RspecParser::parse(json);
        assert_eq!(result.tier(), 1);
        let data = result.unwrap();
        assert_eq!(data.total, 2);
        assert_eq!(data.passed, 1);
        assert_eq!(data.failed, 1);
        assert_eq!(data.failures.len(), 1);
        assert!(data.failures[0].test_name.contains("saves to database"));
        assert!(data.failures[0].error_message.contains("expected true"));
        assert!(data.failures[0].file_path.contains("user_spec.rb"));
        assert!(data.failures[0].file_path.contains("rspec"));
    }

    #[test]
    fn test_rspec_parser_json_with_pending() {
        let json = r#"{
            "examples": [
                {
                    "id": "./spec/test_spec.rb[1:1]",
                    "description": "passed test",
                    "full_description": "Suite passed test",
                    "status": "passed",
                    "file_path": "./spec/test_spec.rb",
                    "line_number": 5,
                    "run_time": 0.001
                },
                {
                    "id": "./spec/test_spec.rb[1:2]",
                    "description": "pending test",
                    "full_description": "Suite pending test",
                    "status": "pending",
                    "file_path": "./spec/test_spec.rb",
                    "line_number": 10,
                    "run_time": 0.0,
                    "pending_message": "TODO: implement this"
                }
            ],
            "summary": {
                "duration": 0.05,
                "example_count": 2,
                "failure_count": 0,
                "pending_count": 1,
                "errors_outside_of_examples_count": 0
            },
            "summary_line": "2 examples, 0 failures, 1 pending"
        }"#;

        let result = RspecParser::parse(json);
        let data = result.unwrap();
        assert_eq!(data.total, 2);
        assert_eq!(data.passed, 1);
        assert_eq!(data.skipped, 1);
        assert_eq!(data.failed, 0);
    }

    #[test]
    fn test_rspec_parser_json_errors_outside_examples() {
        let json = r#"{
            "examples": [],
            "summary": {
                "duration": 0.01,
                "example_count": 0,
                "failure_count": 0,
                "pending_count": 0,
                "errors_outside_of_examples_count": 2
            },
            "summary_line": "0 examples, 0 failures, 2 errors occurred outside of examples"
        }"#;

        let result = RspecParser::parse(json);
        let data = result.unwrap();
        assert_eq!(data.total, 0);
        assert_eq!(data.failed, 2);
    }

    #[test]
    fn test_rspec_parser_json_no_examples() {
        let json = r#"{
            "examples": [],
            "summary": {
                "duration": 0.01,
                "example_count": 0,
                "failure_count": 0,
                "pending_count": 0,
                "errors_outside_of_examples_count": 0
            },
            "summary_line": "0 examples, 0 failures"
        }"#;

        let result = RspecParser::parse(json);
        let data = result.unwrap();
        assert_eq!(data.total, 0);
        assert_eq!(data.passed, 0);
        assert_eq!(data.failed, 0);
    }

    #[test]
    fn test_rspec_parser_exception_no_backtrace() {
        let json = r#"{
            "examples": [
                {
                    "id": "./spec/test_spec.rb[1:1]",
                    "description": "fails",
                    "full_description": "Test fails",
                    "status": "failed",
                    "file_path": "./spec/test_spec.rb",
                    "line_number": 5,
                    "run_time": 0.001,
                    "exception": {
                        "class": "RuntimeError",
                        "message": "Something went wrong",
                        "backtrace": []
                    }
                }
            ],
            "summary": {
                "duration": 0.05,
                "example_count": 1,
                "failure_count": 1,
                "pending_count": 0,
                "errors_outside_of_examples_count": 0
            },
            "summary_line": "1 example, 1 failure"
        }"#;

        let result = RspecParser::parse(json);
        let data = result.unwrap();
        assert_eq!(data.failed, 1);
        assert_eq!(data.failures.len(), 1);
        assert!(data.failures[0].stack_trace.is_none());
        assert!(data.failures[0].error_message.contains("Something went wrong"));
    }

    #[test]
    fn test_rspec_parser_progress_format_fallback() {
        let progress = r#"..F.

Failures:

  1) User#valid? should validate presence
     Failure/Error: expect(user.valid?).to eq(true)

       expected: true
            got: false

     # ./spec/models/user_spec.rb:10:in `block (2 levels)'

Finished in 0.05 seconds (files took 1.2 seconds to load)
4 examples, 1 failure"#;

        let result = RspecParser::parse(progress);
        assert_eq!(result.tier(), 2);
        assert!(result.is_ok());
        let data = result.unwrap();
        assert_eq!(data.total, 4);
        assert_eq!(data.failed, 1);
        assert_eq!(data.passed, 3);
    }

    #[test]
    fn test_rspec_parser_progress_all_pass() {
        let progress = "......\n\nFinished in 0.03 seconds\n6 examples, 0 failures";

        let result = RspecParser::parse(progress);
        assert_eq!(result.tier(), 2);
        let data = result.unwrap();
        assert_eq!(data.total, 6);
        assert_eq!(data.passed, 6);
        assert_eq!(data.failed, 0);
    }

    #[test]
    fn test_rspec_parser_progress_with_pending() {
        let progress = "..*..\n\nFinished in 0.04 seconds\n5 examples, 0 failures, 1 pending";

        let result = RspecParser::parse(progress);
        assert_eq!(result.tier(), 2);
        let data = result.unwrap();
        assert_eq!(data.total, 5);
        assert_eq!(data.skipped, 1);
        assert_eq!(data.passed, 4);
    }
}
