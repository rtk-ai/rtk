//! Filters Playwright E2E test output to show only failures.

use crate::core::shell::{display_args, with_args};
use crate::core::stream::{exec_capture, status_to_exit_code};
use crate::core::tracking;
#[cfg(test)]
use crate::core::utils::strip_ansi;
use crate::core::utils::{detect_package_manager, resolved_command};
use anyhow::{Context, Result};
#[cfg(test)]
use regex::Regex;
#[cfg(test)]
use serde::Deserialize;
use serde_json::Value;
use std::process::Stdio;
#[cfg(test)]
use std::sync::LazyLock;

use crate::parser::emit_passthrough_warning;
#[cfg(test)]
use crate::parser::{OutputParser, ParseResult, TestFailure, TestResult, truncate_passthrough};

/// Legacy parser kept under tests while the command only filters explicitly requested JSON.
#[cfg(test)]
#[derive(Debug, Deserialize)]
struct PlaywrightJsonOutput {
    stats: PlaywrightStats,
    #[serde(default)]
    suites: Vec<PlaywrightSuite>,
}

#[cfg(test)]
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
#[cfg(test)]
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
#[cfg(test)]
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
#[cfg(test)]
#[derive(Debug, Deserialize)]
struct PlaywrightExecution {
    /// "expected", "unexpected", "skipped", "flaky"
    status: String,
    #[serde(default)]
    results: Vec<PlaywrightAttempt>,
}

/// A single attempt/result for a test execution
#[cfg(test)]
#[derive(Debug, Deserialize)]
struct PlaywrightAttempt {
    /// "passed", "failed", "timedOut", "interrupted"
    status: String,
    /// Error details (array in Playwright >= v1.30)
    #[serde(default)]
    errors: Vec<PlaywrightError>,
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
struct PlaywrightError {
    #[serde(default)]
    message: String,
}

/// Parser for Playwright JSON output
#[cfg(test)]
pub struct PlaywrightParser;

#[cfg(test)]
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

#[cfg(test)]
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
#[cfg(test)]
fn extract_playwright_regex(output: &str) -> Option<TestResult> {
    static SUMMARY_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(\d+)\s+(passed|failed|flaky|skipped)").unwrap());
    static DURATION_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\((\d+(?:\.\d+)?)(ms|s|m)\)").unwrap());

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
#[cfg(test)]
fn extract_failures_regex(output: &str) -> Vec<TestFailure> {
    static TEST_PATTERN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[×✗]\s+.*?›\s+([^›]+\.spec\.[tj]sx?)").unwrap());

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

    // Do not override Playwright's configured reporter. Reporter injection
    // changes list/debug/watch/UI behavior and can suppress custom reporters
    // such as Allure. JSON is filtered only when the caller explicitly asks
    // for the standalone JSON reporter.
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: playwright {}", args.join(" "));
    }

    if should_passthrough_interactive(args) {
        let status = cmd
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .context("Failed to run playwright in interactive mode")?;
        return Ok(status_to_exit_code(status));
    }

    let result = exec_capture(&mut cmd)
        .context("Failed to run playwright (try: npm install -g playwright)")?;

    let raw = format!("{}\n{}", result.stdout, result.stderr);

    let filtered = if is_standalone_json_reporter(args) && result.stderr.is_empty() {
        filter_playwright_json(&result.stdout).unwrap_or_else(|| {
            emit_passthrough_warning("playwright", "Unknown JSON reporter schema");
            raw.clone()
        })
    } else {
        raw.clone()
    };

    let hint = crate::core::tee::tee_and_hint(&raw, "playwright", result.exit_code);
    let shown = crate::core::runner::emit_guarded(&filtered, hint.as_deref(), &raw);

    let tracked = with_args("playwright", &display_args(args));
    timer.track(&tracked, &format!("rtk {tracked}"), &raw, &shown);

    // Preserve exit code for CI/CD
    if !result.success() {
        return Ok(result.exit_code);
    }

    Ok(0)
}

fn should_passthrough_interactive(args: &[String]) -> bool {
    args.first().map(String::as_str) == Some("show-trace")
        || args.iter().any(|arg| {
            matches!(arg.as_str(), "--debug" | "--ui" | "--watch")
                || arg.starts_with("--debug=")
                || arg.starts_with("--ui-")
        })
}

fn is_standalone_json_reporter(args: &[String]) -> bool {
    if args.first().map(String::as_str) != Some("test")
        || args.iter().any(|arg| {
            matches!(
                arg.as_str(),
                "--list" | "--debug" | "--ui" | "--watch" | "--ui-host" | "--ui-port"
            ) || arg.starts_with("--debug=")
        })
    {
        return false;
    }

    args.iter().enumerate().any(|(index, arg)| {
        arg == "--reporter=json"
            || (arg == "--reporter" && args.get(index + 1).map(String::as_str) == Some("json"))
    })
}

fn filter_playwright_json(input: &str) -> Option<String> {
    let report: Value = serde_json::from_str(input).ok()?;
    let report_object = report.as_object()?;
    if report_object
        .keys()
        .any(|key| !matches!(key.as_str(), "config" | "stats" | "suites" | "errors"))
        || report
            .get("errors")
            .and_then(Value::as_array)
            .is_some_and(|errors| !errors.is_empty())
    {
        return None;
    }
    if contains_preserved_metadata(&report) {
        return None;
    }
    let stats = report.get("stats")?.as_object()?;
    let expected = stats.get("expected")?.as_u64()?;
    let unexpected = stats.get("unexpected")?.as_u64()?;
    let skipped = stats.get("skipped")?.as_u64()?;
    let flaky = stats.get("flaky").and_then(Value::as_u64);
    let duration = stats.get("duration").and_then(Value::as_f64);
    let suites = report.get("suites")?.as_array()?;

    let mut specs = Vec::new();
    for suite in suites {
        collect_specs(suite, None, &[], &mut specs)?;
    }

    let retries: u64 = specs
        .iter()
        .filter_map(|(spec, _, _)| spec.get("tests").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_object)
        .filter_map(|test| test.get("results").and_then(Value::as_array))
        .flatten()
        .filter_map(|result| result.get("retry").and_then(Value::as_u64))
        .filter(|retry| *retry > 0)
        .count() as u64;

    let mut output = format!(
        "PASS ({expected}) FAIL ({unexpected}) FLAKY ({}) SKIPPED ({skipped}) STATS {}\n",
        flaky.map_or_else(|| "unknown".to_string(), |count| count.to_string()),
        serde_json::to_string(&report["stats"]).ok()?
    );
    if let Some(duration) = duration {
        output.push_str(&format!("Time: {:.0}ms\n", duration));
    }
    output.push_str(&format!("Retries observed: {retries}\n"));

    if !specs.is_empty() {
        output.push_str("\nTest names, locations, statuses and relevant diagnostics:\n");
        for (spec, suite_file, suite_titles) in specs {
            if spec_needs_full_diagnostics(spec)? {
                output.push_str(&format!(
                    "- {}\n",
                    spec_identity(spec, suite_file, &suite_titles)?
                ));
                output.push_str(&serde_json::to_string_pretty(spec).ok()?);
            } else {
                output.push_str(&summarize_spec(spec, suite_file, &suite_titles)?);
            }
            output.push('\n');
        }
    }
    Some(output.trim_end().to_string())
}

fn collect_specs<'a>(
    suite: &'a Value,
    parent_file: Option<&'a str>,
    parent_titles: &[String],
    output: &mut Vec<(&'a Value, Option<&'a str>, Vec<String>)>,
) -> Option<()> {
    let object = suite.as_object()?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "title" | "file" | "line" | "column" | "location" | "specs" | "suites"
        )
    }) {
        return None;
    }
    let file = object.get("file").and_then(Value::as_str).or(parent_file);
    let mut suite_titles = parent_titles.to_vec();
    if let Some(title) = object.get("title").and_then(Value::as_str)
        && !title.is_empty()
    {
        suite_titles.push(title.to_string());
    }
    if let Some(specs) = object.get("specs") {
        for spec in specs.as_array()? {
            spec_statuses(spec)?;
            output.push((spec, file, suite_titles.clone()));
        }
    }
    if let Some(suites) = object.get("suites") {
        for child in suites.as_array()? {
            collect_specs(child, file, &suite_titles, output)?;
        }
    }
    Some(())
}

struct TestStatusSummary {
    project: String,
    result: String,
    retry: u64,
}

struct SpecStatusSummary {
    needs_diagnostics: bool,
    statuses: Vec<TestStatusSummary>,
}

fn spec_statuses(spec: &Value) -> Option<SpecStatusSummary> {
    let tests = spec.get("tests")?.as_array()?;
    let mut needs_diagnostics = spec.get("ok").and_then(Value::as_bool) == Some(false);
    let mut statuses = Vec::new();
    for test in tests {
        let status = test.get("status")?.as_str()?;
        if !matches!(status, "expected" | "unexpected" | "skipped" | "flaky") {
            return None;
        }
        needs_diagnostics |= matches!(status, "unexpected" | "flaky");
        let project = test
            .get("projectName")
            .or_else(|| test.get("projectId"))
            .and_then(Value::as_str)
            .unwrap_or("default")
            .to_string();
        let results = test.get("results").and_then(Value::as_array)?;
        for result in results {
            let result_status = result.get("status")?.as_str()?;
            if !matches!(
                result_status,
                "passed" | "failed" | "timedOut" | "interrupted" | "skipped"
            ) {
                return None;
            }
            let retry = result.get("retry").and_then(Value::as_u64).unwrap_or(0);
            needs_diagnostics |=
                matches!(result_status, "failed" | "timedOut" | "interrupted") || retry > 0;
            statuses.push(TestStatusSummary {
                project: project.clone(),
                result: result_status.to_string(),
                retry,
            });
        }
        if results.is_empty() {
            statuses.push(TestStatusSummary {
                project,
                result: status.to_string(),
                retry: 0,
            });
        }
    }
    Some(SpecStatusSummary {
        needs_diagnostics,
        statuses,
    })
}

fn spec_needs_full_diagnostics(spec: &Value) -> Option<bool> {
    Some(spec_statuses(spec)?.needs_diagnostics)
}

fn spec_identity(
    spec: &Value,
    suite_file: Option<&str>,
    suite_titles: &[String],
) -> Option<String> {
    let title = spec.get("title")?.as_str()?;
    let location = spec.get("location").and_then(Value::as_object);
    let file = location
        .and_then(|value| value.get("file"))
        .and_then(Value::as_str)
        .or(suite_file)
        .or_else(|| spec.get("file").and_then(Value::as_str))?;
    let line = location
        .and_then(|value| value.get("line"))
        .and_then(Value::as_u64)
        .or_else(|| spec.get("line").and_then(Value::as_u64));
    let column = location
        .and_then(|value| value.get("column"))
        .and_then(Value::as_u64)
        .or_else(|| spec.get("column").and_then(Value::as_u64));
    let path = match (line, column) {
        (Some(line), Some(column)) => format!("{file}:{line}:{column}"),
        (Some(line), None) => format!("{file}:{line}"),
        _ => file.to_string(),
    };
    let title_path = suite_titles
        .iter()
        .filter(|suite_title| suite_title.as_str() != title)
        .cloned()
        .chain([title.to_string()])
        .collect::<Vec<_>>()
        .join(" / ");
    Some(format!("{path} {title_path}"))
}

fn summarize_spec(
    spec: &Value,
    suite_file: Option<&str>,
    suite_titles: &[String],
) -> Option<String> {
    let statuses = spec_statuses(spec)?;
    if statuses.needs_diagnostics {
        return None;
    }
    if spec.as_object()?.keys().any(|key| {
        !matches!(
            key.as_str(),
            "title" | "ok" | "location" | "line" | "column" | "tests"
        )
    }) || spec.get("tests")?.as_array()?.iter().any(|test| {
        test.as_object().is_none_or(|object| {
            object.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "projectName"
                        | "projectId"
                        | "status"
                        | "results"
                        | "timeout"
                        | "annotations"
                        | "expectedStatus"
                )
            })
        }) || test
            .get("results")
            .and_then(Value::as_array)
            .is_none_or(|results| {
                results.iter().any(|result| {
                    result.as_object().is_none_or(|object| {
                        object.keys().any(|key| {
                            !matches!(
                                key.as_str(),
                                "status"
                                    | "retry"
                                    | "errors"
                                    | "attachments"
                                    | "duration"
                                    | "workerIndex"
                                    | "parallelIndex"
                                    | "stdout"
                                    | "stderr"
                                    | "startTime"
                                    | "steps"
                            )
                        })
                    })
                })
            })
    }) {
        return None;
    }
    let status = statuses
        .statuses
        .into_iter()
        .map(|status| {
            format!(
                "{}={}(retry={})",
                status.project, status.result, status.retry
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    Some(format!(
        "- {} [{status}]",
        spec_identity(spec, suite_file, suite_titles)?
    ))
}

fn contains_preserved_metadata(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, child)| {
            let normalized = key.to_ascii_lowercase();
            normalized.contains("allure")
                || normalized.contains("testid")
                || matches!(normalized.as_str(), "annotations" | "tags" | "metadata")
                    && !child.is_null()
                    && child.as_array().is_none_or(|items| !items.is_empty())
                    && child.as_object().is_none_or(|items| !items.is_empty())
                    && child.as_str().is_none_or(|item| !item.is_empty())
                || matches!(normalized.as_str(), "attachments")
                    && child.as_array().is_some_and(|items| !items.is_empty())
                || matches!(
                    normalized.as_str(),
                    "errors" | "steps" | "stdout" | "stderr" | "error"
                ) && !child.is_null()
                    && child.as_array().is_none_or(|items| !items.is_empty())
                    && child.as_object().is_none_or(|items| !items.is_empty())
                    && child.as_str().is_none_or(|item| !item.is_empty())
                || contains_preserved_metadata(child)
        }),
        Value::Array(items) => items.iter().any(contains_preserved_metadata),
        _ => false,
    }
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

    #[test]
    fn explicit_json_filter_preserves_failure_and_retry_diagnostics() {
        let failing_raw = include_str!("../../../tests/fixtures/playwright/failing.json");
        let failing =
            filter_playwright_json(failing_raw).unwrap_or_else(|| failing_raw.to_string());
        assert_eq!(
            failing, failing_raw,
            "attachments and Allure data use raw fallback"
        );
        for expected in [
            "failing scenario",
            "tests/sample.spec.ts",
            "Expected 2, received 1",
            "sample.spec.ts:4:9",
            "test-results/failure/trace.zip",
            "test-results/failure/test-failed-1.png",
            "test-results/failure/error-context.md",
            "allure-results/result.json",
        ] {
            assert!(failing.contains(expected), "missing {expected}");
        }

        let flaky_raw = include_str!("../../../tests/fixtures/playwright/flaky.json");
        let flaky = filter_playwright_json(flaky_raw).unwrap_or_else(|| flaky_raw.to_string());
        assert_eq!(flaky, flaky_raw, "flaky diagnostics use raw fallback");
        assert!(flaky.contains("\"flaky\": 1"));
        assert!(flaky.contains("transient failure"));
        assert!(flaky.contains("\"retry\": 1"));

        let retried_raw = include_str!("../../../tests/fixtures/playwright/retried.json");
        let retried =
            filter_playwright_json(retried_raw).unwrap_or_else(|| retried_raw.to_string());
        assert_eq!(
            retried, retried_raw,
            "failed retry details use raw fallback"
        );
        assert!(retried.contains("\"retry\": 1"));
        assert!(retried.contains("retried scenario"));

        let timed_out_raw = include_str!("../../../tests/fixtures/playwright/timed-out.json");
        let timed_out =
            filter_playwright_json(timed_out_raw).unwrap_or_else(|| timed_out_raw.to_string());
        assert_eq!(
            timed_out, timed_out_raw,
            "trace attachments use raw fallback"
        );
        assert!(timed_out.contains("timed-out scenario"));
        assert!(timed_out.contains("Test timeout of 30000ms exceeded"));
        assert!(timed_out.contains("test-results/timeout/trace.zip"));

        let passing = filter_playwright_json(include_str!(
            "../../../tests/fixtures/playwright/passing.json"
        ))
        .unwrap();
        assert!(
            passing.len() < include_str!("../../../tests/fixtures/playwright/passing.json").len()
        );
        assert!(passing.contains("PASS (1)"));
        assert!(passing.contains("passing scenario"));
        assert!(passing.contains("tests/sample.spec.ts:3:1"));

        let skipped = filter_playwright_json(include_str!(
            "../../../tests/fixtures/playwright/skipped.json"
        ))
        .unwrap();
        assert!(skipped.contains("SKIPPED (1)"));
    }

    #[test]
    fn allure_metadata_uses_raw_report_fallback() {
        let report = r#"{"stats":{"expected":1,"unexpected":0,"skipped":0},"suites":[{"file":"a.spec.ts","specs":[{"title":"case","tests":[{"projectId":"chromium","status":"expected","annotations":[{"type":"allure.id","description":"123"}],"results":[{"status":"passed","retry":0,"attachments":[]}]}]}]}]}"#;
        assert!(filter_playwright_json(report).is_none());
        assert!(contains_preserved_metadata(
            &serde_json::from_str(report).unwrap()
        ));
    }

    #[test]
    fn only_explicit_standalone_json_reporter_is_filtered() {
        let json = vec!["test".into(), "--reporter=json".into()];
        assert!(is_standalone_json_reporter(&json));
        assert!(!is_standalone_json_reporter(&[
            "test".into(),
            "--list".into()
        ]));
        assert!(!is_standalone_json_reporter(&[
            "test".into(),
            "--debug=cli".into(),
            "--reporter=json".into(),
        ]));
        assert!(!is_standalone_json_reporter(&[
            "test".into(),
            "--ui".into(),
            "--reporter=json".into(),
        ]));
        assert!(should_passthrough_interactive(&[
            "test".into(),
            "--debug=cli".into(),
        ]));
        assert!(should_passthrough_interactive(&["show-trace".into()]));
        assert!(!should_passthrough_interactive(&[
            "test".into(),
            "--list".into()
        ]));
        assert!(!is_standalone_json_reporter(&[
            "test".into(),
            "--reporter".into(),
            "allure".into(),
        ]));
    }

    #[test]
    fn unknown_json_reporter_schema_is_not_filtered() {
        assert!(filter_playwright_json(r#"{"version": 2, "events": []}"#).is_none());
    }
}
