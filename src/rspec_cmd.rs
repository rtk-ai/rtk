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

fn extract_stats_regex(_output: &str) -> Option<TestResult> {
    // Placeholder — implemented in Task 3
    None
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
}
