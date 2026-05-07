//! Filters Playwright E2E test output to show only failures.

use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::utils::{detect_package_manager, resolved_command, strip_ansi};
use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;

use crate::parser::{
    emit_degradation_warning, emit_passthrough_warning, truncate_passthrough, FormatMode,
    OutputParser, ParseResult, TestFailure, TestResult, TokenFormatter,
};

/// Matches real Playwright JSON reporter output (suites → specs → tests → results)
#[derive(Debug, Deserialize)]
struct PlaywrightJsonOutput {
    stats: PlaywrightStats,
    #[serde(default)]
    suites: Vec<PlaywrightSuite>,
}

#[derive(Debug, Deserialize)]
struct PlaywrightStats {
    expected: usize,
    unexpected: usize,
    skipped: usize,
    /// Duration in milliseconds (float in real Playwright output)
    #[serde(default)]
    duration: f64,
}

/// File-level or describe-level suite
#[derive(Debug, Deserialize)]
struct PlaywrightSuite {
    title: String,
    #[serde(default)]
    file: Option<String>,
    /// Individual test specs (test functions)
    #[serde(default)]
    specs: Vec<PlaywrightSpec>,
    /// Nested describe blocks
    #[serde(default)]
    suites: Vec<PlaywrightSuite>,
}

/// A single test function (may run in multiple browsers/projects)
#[derive(Debug, Deserialize)]
struct PlaywrightSpec {
    title: String,
    /// Overall pass/fail status across all projects
    ok: bool,
    /// Per-project/browser executions
    #[serde(default)]
    tests: Vec<PlaywrightExecution>,
}

/// A test execution in a specific browser/project
#[derive(Debug, Deserialize)]
struct PlaywrightExecution {
    /// "expected", "unexpected", "skipped", "flaky"
    status: String,
    #[serde(default)]
    results: Vec<PlaywrightAttempt>,
}

/// A single attempt/result for a test execution
#[derive(Debug, Deserialize)]
struct PlaywrightAttempt {
    /// "passed", "failed", "timedOut", "interrupted"
    status: String,
    /// Error details (array in Playwright >= v1.30)
    #[serde(default)]
    errors: Vec<PlaywrightError>,
}

#[derive(Debug, Deserialize)]
struct PlaywrightError {
    #[serde(default)]
    message: String,
}

/// Parser for Playwright JSON output
pub struct PlaywrightParser;

impl OutputParser for PlaywrightParser {
    type Output = TestResult;

    fn parse(input: &str) -> ParseResult<TestResult> {
        // Tier 1: Try JSON parsing
        match serde_json::from_str::<PlaywrightJsonOutput>(input) {
            Ok(json) => {
                let mut failures = Vec::new();
                let mut total = 0;
                collect_test_results(&json.suites, &mut total, &mut failures);

                let result = TestResult {
                    total,
                    passed: json.stats.expected,
                    failed: json.stats.unexpected,
                    skipped: json.stats.skipped,
                    duration_ms: Some(json.stats.duration as u64),
                    failures,
                };

                ParseResult::Full(result)
            }
            Err(e) => {
                // Tier 2: Try regex extraction
                match extract_playwright_regex(input) {
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

fn collect_test_results(
    suites: &[PlaywrightSuite],
    total: &mut usize,
    failures: &mut Vec<TestFailure>,
) {
    for suite in suites {
        let file_path = suite.file.as_deref().unwrap_or(&suite.title);

        for spec in &suite.specs {
            *total += 1;

            if !spec.ok {
                // Find the first failed execution and its error message
                let error_msg = spec
                    .tests
                    .iter()
                    .find(|t| t.status == "unexpected")
                    .and_then(|t| {
                        t.results
                            .iter()
                            .find(|r| r.status == "failed" || r.status == "timedOut")
                    })
                    .and_then(|r| r.errors.first())
                    .map(|e| e.message.clone())
                    .unwrap_or_else(|| "Test failed".to_string());

                failures.push(TestFailure {
                    test_name: spec.title.clone(),
                    file_path: file_path.to_string(),
                    error_message: error_msg,
                    stack_trace: None,
                });
            }
        }

        // Recurse into nested suites (describe blocks)
        collect_test_results(&suite.suites, total, failures);
    }
}

/// Tier 2: Extract test statistics using regex (degraded mode)
fn extract_playwright_regex(output: &str) -> Option<TestResult> {
    lazy_static::lazy_static! {
        static ref SUMMARY_RE: Regex = Regex::new(
            r"(\d+)\s+(passed|failed|flaky|skipped)"
        ).unwrap();
        static ref DURATION_RE: Regex = Regex::new(
            r"\((\d+(?:\.\d+)?)(ms|s|m)\)"
        ).unwrap();
    }

    let clean_output = strip_ansi(output);

    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;

    // Parse summary counts
    for caps in SUMMARY_RE.captures_iter(&clean_output) {
        let count: usize = caps[1].parse().unwrap_or(0);
        match &caps[2] {
            "passed" => passed = count,
            "failed" => failed = count,
            "skipped" => skipped = count,
            _ => {}
        }
    }

    // Parse duration
    let duration_ms = DURATION_RE.captures(&clean_output).and_then(|caps| {
        let value: f64 = caps[1].parse().ok()?;
        let unit = &caps[2];
        Some(match unit {
            "ms" => value as u64,
            "s" => (value * 1000.0) as u64,
            "m" => (value * 60000.0) as u64,
            _ => value as u64,
        })
    });

    // Only return if we found valid data
    let total = passed + failed + skipped;
    if total > 0 {
        Some(TestResult {
            total,
            passed,
            failed,
            skipped,
            duration_ms,
            failures: extract_failures_regex(&clean_output),
        })
    } else {
        None
    }
}

/// Extract failures using regex
fn extract_failures_regex(output: &str) -> Vec<TestFailure> {
    lazy_static::lazy_static! {
        static ref TEST_PATTERN: Regex = Regex::new(
            r"[×✗]\s+.*?›\s+([^›]+\.spec\.[tj]sx?)"
        ).unwrap();
    }

    let mut failures = Vec::new();

    for caps in TEST_PATTERN.captures_iter(output) {
        if let Some(spec) = caps.get(1) {
            failures.push(TestFailure {
                test_name: caps[0].to_string(),
                file_path: spec.as_str().to_string(),
                error_message: String::new(),
                stack_trace: None,
            });
        }
    }

    failures
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    // Skip `which playwright` — it can find pyenv shims or other non-Node
    // binaries. Always resolve through the package manager.
    let pm = detect_package_manager();
    let mut cmd = match pm {
        "pnpm" => {
            let mut c = resolved_command("pnpm");
            c.arg("exec").arg("--").arg("playwright");
            c
        }
        "yarn" => {
            let mut c = resolved_command("yarn");
            c.arg("exec").arg("--").arg("playwright");
            c
        }
        _ => {
            let mut c = resolved_command("npx");
            c.arg("--no-install").arg("--").arg("playwright");
            c
        }
    };

    // Only inject --reporter=json for `playwright test` runs
    let is_test = args.first().map(|a| a == "test").unwrap_or(false);
    if is_test {
        cmd.arg("test");
        cmd.arg("--reporter=json");
        // Strip user's --reporter to avoid conflicts with our forced JSON
        for arg in &args[1..] {
            if !arg.starts_with("--reporter") {
                cmd.arg(arg);
            }
        }
    } else {
        for arg in args {
            cmd.arg(arg);
        }
    }

    if verbose > 0 {
        eprintln!("Running: playwright {}", args.join(" "));
    }

    let result = exec_capture(&mut cmd)
        .context("Failed to run playwright (try: npm install -g playwright)")?;

    let raw = format!("{}\n{}", result.stdout, result.stderr);

    // Parse output using PlaywrightParser
    let parse_result = PlaywrightParser::parse(&result.stdout);
    let mode = FormatMode::from_verbosity(verbose);

    let filtered = match parse_result {
        ParseResult::Full(data) => {
            if verbose > 0 {
                eprintln!("playwright test (Tier 1: Full JSON parse)");
            }
            data.format(mode)
        }
        ParseResult::Degraded(data, warnings) => {
            if verbose > 0 {
                emit_degradation_warning("playwright", &warnings.join(", "));
            }
            data.format(mode)
        }
        ParseResult::Passthrough(raw) => {
            emit_passthrough_warning("playwright", "All parsing tiers failed");
            raw
        }
    };

    if let Some(hint) = crate::core::tee::tee_and_hint(&raw, "playwright", result.exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("playwright {}", args.join(" ")),
        &format!("rtk playwright {}", args.join(" ")),
        &raw,
        &filtered,
    );

    // Preserve exit code for CI/CD
    if !result.success() {
        return Ok(result.exit_code);
    }

    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_playwright_parser_json() {
        // Real Playwright JSON structure: suites → specs, with float duration
        let json = r#"{
            "config": {},
            "stats": {
                "startTime": "2026-01-01T00:00:00.000Z",
                "expected": 1,
                "unexpected": 0,
                "skipped": 0,
                "flaky": 0,
                "duration": 7300.5
            },
            "suites": [
                {
                    "title": "auth",
                    "specs": [],
                    "suites": [
                        {
                            "title": "login.spec.ts",
                            "specs": [
                                {
                                    "title": "should login",
                                    "ok": true,
                                    "tests": [
                                        {
                                            "status": "expected",
                                            "results": [{"status": "passed", "errors": [], "duration": 2300}]
                                        }
                                    ]
                                }
                            ],
                            "suites": []
                        }
                    ]
                }
            ],
            "errors": []
        }"#;

        let result = PlaywrightParser::parse(json);
        assert_eq!(result.tier(), 1);
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.passed, 1);
        assert_eq!(data.failed, 0);
        assert_eq!(data.duration_ms, Some(7300));
    }

    #[test]
    fn test_playwright_parser_json_float_duration() {
        // Real Playwright output uses float duration (e.g. 3519.7039999999997)
        let json = r#"{
            "stats": {
                "startTime": "2026-02-18T10:17:53.187Z",
                "expected": 4,
                "unexpected": 0,
                "skipped": 0,
                "flaky": 0,
                "duration": 3519.7039999999997
            },
            "suites": [],
            "errors": []
        }"#;

        let result = PlaywrightParser::parse(json);
        assert_eq!(result.tier(), 1);
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.passed, 4);
        assert_eq!(data.duration_ms, Some(3519));
    }

    #[test]
    fn test_playwright_parser_json_with_failure() {
        let json = r#"{
            "stats": {
                "expected": 0,
                "unexpected": 1,
                "skipped": 0,
                "duration": 1500.0
            },
            "suites": [
                {
                    "title": "my.spec.ts",
                    "specs": [
                        {
                            "title": "should work",
                            "ok": false,
                            "tests": [
                                {
                                    "status": "unexpected",
                                    "results": [
                                        {
                                            "status": "failed",
                                            "errors": [{"message": "Expected true to be false"}],
                                            "duration": 500
                                        }
                                    ]
                                }
                            ]
                        }
                    ],
                    "suites": []
                }
            ],
            "errors": []
        }"#;

        let result = PlaywrightParser::parse(json);
        assert_eq!(result.tier(), 1);
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.failed, 1);
        assert_eq!(data.failures.len(), 1);
        assert_eq!(data.failures[0].test_name, "should work");
        assert_eq!(data.failures[0].error_message, "Expected true to be false");
    }

    #[test]
    fn test_playwright_parser_regex_fallback() {
        let text = "3 passed (7.3s)";
        let result = PlaywrightParser::parse(text);
        assert_eq!(result.tier(), 2); // Degraded
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.passed, 3);
        assert_eq!(data.failed, 0);
    }

    #[test]
    fn test_playwright_parser_passthrough() {
        let invalid = "random output";
        let result = PlaywrightParser::parse(invalid);
        assert_eq!(result.tier(), 3); // Passthrough
        assert!(!result.is_ok());
    }
}
