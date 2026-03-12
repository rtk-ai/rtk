use crate::parser::{
    emit_degradation_warning, emit_passthrough_warning, truncate_output, FormatMode, OutputParser,
    ParseResult, TestFailure, TestResult, TokenFormatter,
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
                let passed = json
                    .summary
                    .example_count
                    .saturating_sub(json.summary.failure_count)
                    .saturating_sub(json.summary.pending_count);
                let failed =
                    json.summary.failure_count + json.summary.errors_outside_of_examples_count;

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
            Err(e) => match extract_stats_regex(input) {
                Some(result) => {
                    ParseResult::Degraded(result, vec![format!("JSON parse failed: {}", e)])
                }
                None => ParseResult::Passthrough(truncate_output(input, 500)),
            },
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

/// Check if a path is an executable file (unix: checks permission bits).
fn is_executable(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.is_file()
            && path
                .metadata()
                .map(|m| m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Check if user already passed a format flag to RSpec.
/// Handles: --format, -f, --format=..., -fj, -fjson, -fdocumentation
fn has_format_flag(args: &[String]) -> bool {
    args.iter().any(|a| {
        a == "--format"
            || a == "-f"
            || a.starts_with("--format=")
            || (a.starts_with("-f") && a.len() > 2 && !a.starts_with("--"))
    })
}

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    // Auto-detect invocation: bin/rspec (executable) → bundle exec rspec → rspec
    let mut cmd = if is_executable(std::path::Path::new("bin/rspec")) {
        Command::new("bin/rspec")
    } else if std::path::Path::new("Gemfile.lock").exists() {
        let mut c = Command::new("bundle");
        c.arg("exec").arg("rspec");
        c
    } else {
        Command::new("rspec")
    };

    // Pass through all user arguments first
    for arg in args {
        cmd.arg(arg);
    }

    // Determine if we can inject JSON format
    let inject_json = !has_format_flag(args);
    let json_tempfile = if inject_json {
        let path = std::env::temp_dir().join(format!("rtk-rspec-{}.json", std::process::id()));
        cmd.arg("--format").arg("json").arg("--out").arg(&path);
        Some(path)
    } else {
        None
    };

    if verbose > 0 {
        eprintln!("Running: rspec (inject_json={})", inject_json);
    }

    let output = cmd
        .output()
        .context("Failed to run rspec. Is it installed? Try: gem install rspec")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);

    // Try Tier 1: Parse JSON from temp file
    let parse_result = if let Some(ref path) = json_tempfile {
        match std::fs::read_to_string(path) {
            Ok(json_content) => {
                let _ = std::fs::remove_file(path);
                RspecParser::parse(&json_content)
            }
            Err(_) => {
                let _ = std::fs::remove_file(path);
                RspecParser::parse(&stdout)
            }
        }
    } else {
        RspecParser::parse(&stdout)
    };

    let mode = FormatMode::from_verbosity(verbose);

    let filtered = match parse_result {
        ParseResult::Full(data) => {
            if verbose > 0 {
                eprintln!("rspec (Tier 1: Full JSON parse)");
            }
            data.format(mode)
        }
        ParseResult::Degraded(data, warnings) => {
            if verbose > 0 {
                emit_degradation_warning("rspec", &warnings.join(", "));
            }
            data.format(mode)
        }
        ParseResult::Passthrough(raw) => {
            emit_passthrough_warning("rspec", "All parsing tiers failed");
            raw
        }
    };

    let exit_code = output.status.code().unwrap_or(1);
    if let Some(hint) = crate::tee::tee_and_hint(&combined, "rspec", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    // Stderr for Ruby warnings, Bundler messages
    if !stderr.trim().is_empty() {
        eprintln!("{}", stderr.trim());
    }

    timer.track(
        &format!("rspec {}", args.join(" ")),
        &format!("rtk rspec {}", args.join(" ")),
        &combined,
        &filtered,
    );

    // Propagate exit code
    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
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
        assert!(data.failures[0]
            .error_message
            .contains("Something went wrong"));
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

    #[test]
    fn test_rspec_parser_passthrough() {
        let garbage = "random output with no structure whatsoever";
        let result = RspecParser::parse(garbage);
        assert_eq!(result.tier(), 3);
        assert!(!result.is_ok());
    }

    #[test]
    fn test_rspec_token_savings() {
        let json = r#"{
            "examples": [
                {"id": "./spec/a_spec.rb[1:1]", "description": "test 1", "full_description": "Model test 1", "status": "passed", "file_path": "./spec/a_spec.rb", "line_number": 5, "run_time": 0.001},
                {"id": "./spec/a_spec.rb[1:2]", "description": "test 2", "full_description": "Model test 2", "status": "passed", "file_path": "./spec/a_spec.rb", "line_number": 10, "run_time": 0.002},
                {"id": "./spec/a_spec.rb[1:3]", "description": "test 3", "full_description": "Model test 3", "status": "passed", "file_path": "./spec/a_spec.rb", "line_number": 15, "run_time": 0.001},
                {"id": "./spec/b_spec.rb[1:1]", "description": "test 4", "full_description": "Service test 4", "status": "passed", "file_path": "./spec/b_spec.rb", "line_number": 5, "run_time": 0.003},
                {"id": "./spec/b_spec.rb[1:2]", "description": "test 5", "full_description": "Service test 5", "status": "passed", "file_path": "./spec/b_spec.rb", "line_number": 10, "run_time": 0.001}
            ],
            "summary": {
                "duration": 0.05,
                "example_count": 5,
                "failure_count": 0,
                "pending_count": 0,
                "errors_outside_of_examples_count": 0
            },
            "summary_line": "5 examples, 0 failures"
        }"#;

        let result = RspecParser::parse(json);
        let data = result.unwrap();
        let output = data.format(FormatMode::Compact);

        let input_tokens = json.split_whitespace().count();
        let output_tokens = output.split_whitespace().count();
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "Expected >=60% savings, got {:.1}% (input: {} tokens, output: {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_has_format_flag() {
        assert!(!has_format_flag(&[]));
        assert!(!has_format_flag(&[
            "spec/".to_string(),
            "--tag".to_string(),
            "focus".to_string()
        ]));
        assert!(has_format_flag(&[
            "--format".to_string(),
            "documentation".to_string()
        ]));
        assert!(has_format_flag(&["-f".to_string(), "progress".to_string()]));
        assert!(has_format_flag(&["--format=json".to_string()]));
        assert!(has_format_flag(&["-fj".to_string()]));
        assert!(has_format_flag(&["-fjson".to_string()]));
        assert!(has_format_flag(&["-fdocumentation".to_string()]));
    }
}
