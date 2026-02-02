use crate::tracking;
use crate::utils::strip_ansi;
use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::process::Command;

use crate::parser::{
    emit_degradation_warning, emit_passthrough_warning, truncate_output, FormatMode,
    OutputParser, ParseResult, TestFailure, TestResult, TokenFormatter,
};

/// Playwright JSON output structures (tool-specific format)
#[derive(Debug, Deserialize)]
struct PlaywrightJsonOutput {
    #[serde(rename = "stats")]
    stats: PlaywrightStats,
    #[serde(rename = "suites")]
    suites: Vec<PlaywrightSuite>,
}

#[derive(Debug, Deserialize)]
struct PlaywrightStats {
    #[serde(rename = "expected")]
    expected: usize,
    #[serde(rename = "unexpected")]
    unexpected: usize,
    #[serde(rename = "skipped")]
    skipped: usize,
    #[serde(rename = "duration", default)]
    duration: u64,
}

#[derive(Debug, Deserialize)]
struct PlaywrightSuite {
    title: String,
    #[serde(rename = "tests")]
    tests: Vec<PlaywrightTest>,
    #[serde(rename = "suites", default)]
    suites: Vec<PlaywrightSuite>,
}

#[derive(Debug, Deserialize)]
struct PlaywrightTest {
    title: String,
    #[serde(rename = "status")]
    status: String,
    #[serde(rename = "results")]
    results: Vec<PlaywrightTestResult>,
}

#[derive(Debug, Deserialize)]
struct PlaywrightTestResult {
    #[serde(rename = "status")]
    status: String,
    #[serde(rename = "error")]
    error: Option<PlaywrightError>,
    #[serde(rename = "duration", default)]
    duration: u64,
}

#[derive(Debug, Deserialize)]
struct PlaywrightError {
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
                    duration_ms: Some(json.stats.duration),
                    failures,
                };

                ParseResult::Full(result)
            }
            Err(e) => {
                // Tier 2: Try regex extraction
                match extract_playwright_regex(input) {
                    Some(result) => ParseResult::Degraded(
                        result,
                        vec![format!("JSON parse failed: {}", e)],
                    ),
                    None => {
                        // Tier 3: Passthrough
                        ParseResult::Passthrough(truncate_output(input, 500))
                    }
                }
            }
        }
    }
}

/// Recursively collect test results from suites
fn collect_test_results(
    suites: &[PlaywrightSuite],
    total: &mut usize,
    failures: &mut Vec<TestFailure>,
) {
    for suite in suites {
        for test in &suite.tests {
            *total += 1;

            if test.status == "failed" || test.status == "timedOut" {
                let error_msg = test
                    .results
                    .first()
                    .and_then(|r| r.error.as_ref())
                    .map(|e| e.message.clone())
                    .unwrap_or_else(|| "Unknown error".to_string());

                failures.push(TestFailure {
                    test_name: test.title.clone(),
                    file_path: suite.title.clone(),
                    error_message: error_msg,
                    stack_trace: None,
                });
            }
        }

        // Recurse into nested suites
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

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    // Try playwright directly first, fallback to package manager exec
    let playwright_exists = Command::new("which")
        .arg("playwright")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    // Detect package manager (pnpm/yarn have better CWD handling than npx)
    let is_pnpm = std::path::Path::new("pnpm-lock.yaml").exists();
    let is_yarn = std::path::Path::new("yarn.lock").exists();

    let mut cmd = if playwright_exists {
        Command::new("playwright")
    } else if is_pnpm {
        let mut c = Command::new("pnpm");
        c.arg("exec");
        c.arg("--");
        c.arg("playwright");
        c
    } else if is_yarn {
        let mut c = Command::new("yarn");
        c.arg("exec");
        c.arg("--");
        c.arg("playwright");
        c
    } else {
        let mut c = Command::new("npx");
        c.arg("--no-install");
        c.arg("--");
        c.arg("playwright");
        c
    };

    // Add JSON reporter for structured output
    cmd.arg("--reporter=json");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        let tool = if playwright_exists {
            "playwright"
        } else if is_pnpm {
            "pnpm exec playwright"
        } else if is_yarn {
            "yarn exec playwright"
        } else {
            "npx playwright"
        };
        eprintln!("Running: {} {}", tool, args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run playwright (try: npm install -g playwright)")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    // Parse output using PlaywrightParser
    let parse_result = PlaywrightParser::parse(&stdout);
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

    println!("{}", filtered);

    tracking::track(
        &format!("playwright {}", args.join(" ")),
        &format!("rtk playwright {}", args.join(" ")),
        &raw,
        &filtered,
    );

    // Preserve exit code for CI/CD
    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_playwright_parser_json() {
        let json = r#"{
            "stats": {
                "expected": 3,
                "unexpected": 0,
                "skipped": 0,
                "duration": 7300
            },
            "suites": [
                {
                    "title": "auth/login.spec.ts",
                    "tests": [
                        {
                            "title": "should login",
                            "status": "passed",
                            "results": [{"status": "passed", "duration": 2300}]
                        }
                    ],
                    "suites": []
                }
            ]
        }"#;

        let result = PlaywrightParser::parse(json);
        assert_eq!(result.tier(), 1);
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.passed, 3);
        assert_eq!(data.failed, 0);
        assert_eq!(data.duration_ms, Some(7300));
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
