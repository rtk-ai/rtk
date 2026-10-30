//! Filters Vitest test output to show only failures.

use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;
use std::sync::LazyLock;

use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::utils::{package_manager_exec, strip_ansi};
use crate::parser::{
    FormatMode, OutputParser, ParseResult, TestFailure, TestResult, TokenFormatter,
    emit_degradation_warning, emit_passthrough_warning, extract_json_object, truncate_output,
    truncate_passthrough,
};

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
    #[serde(rename = "assertionResults", default)]
    assertion_results: Vec<VitestTest>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    message: String,
    #[serde(rename = "testExecError", default)]
    test_exec_error: Option<VitestSuiteError>,
}

#[derive(Debug, Deserialize)]
struct VitestSuiteError {
    message: String,
    #[serde(default)]
    stack: Option<String>,
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
        // A suite can fail to load before it reports any assertions. Preserve
        // that diagnostic without counting it as a test that never ran. When
        // assertions did fail, their messages already describe the suite.
        if !file
            .assertion_results
            .iter()
            .any(|test| test.status == "failed")
            && (file.status == "failed" || file.test_exec_error.is_some())
        {
            let mut messages = Vec::new();
            if !file.message.trim().is_empty() {
                messages.push(file.message.as_str());
            }
            if let Some(error) = &file.test_exec_error {
                if !error.message.trim().is_empty() && !messages.contains(&error.message.as_str()) {
                    messages.push(error.message.as_str());
                }
                if let Some(stack) = error.stack.as_deref()
                    && !stack.trim().is_empty()
                    && !messages.contains(&stack)
                {
                    messages.push(stack);
                }
            }
            if messages.is_empty() {
                messages.push("Test suite failed before reporting assertions.");
            }
            failures.push(TestFailure {
                test_name: format!("Suite failed: {}", file.name),
                file_path: file.name.clone(),
                error_message: messages.join("\n"),
                stack_trace: None,
            });
        }
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
    static TESTS_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"Tests\s+(?:(\d+)\s+failed\s+\|\s+)?(\d+)\s+passed").unwrap());
    static DURATION_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"Duration\s+([\d.]+)(ms|s)").unwrap());

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

pub fn run_vitest(args: &[String], verbose: u8) -> Result<i32> {
    run_framework_test(vitest_invocation(args), verbose)
}

pub fn run_jest(args: &[String], verbose: u8) -> Result<i32> {
    run_framework_test(jest_invocation(args), verbose)
}

/// What `rtk vitest` or `rtk jest` spawns: the tool, the arguments it gets,
/// and whether the user's own reporter output is shown instead of parsed.
struct TestInvocation {
    framework: &'static str,
    args: Vec<String>,
    passthrough: bool,
}

fn vitest_invocation(args: &[String]) -> TestInvocation {
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

    TestInvocation {
        framework: "vitest",
        args: effective,
        passthrough,
    }
}

fn jest_invocation(args: &[String]) -> TestInvocation {
    let mut effective = vec![
        // Force non-watch mode
        "--no-watch".to_string(),
        // Enable JSON structured output
        "--json".to_string(),
    ];
    effective.extend(
        args.iter()
            .filter(|arg| !should_skip_jest_arg(arg))
            .cloned(),
    );
    TestInvocation {
        framework: "jest",
        args: effective,
        passthrough: false,
    }
}

fn should_skip_jest_arg(arg: &str) -> bool {
    arg == "run"
        || arg.starts_with("--json")
        || arg.starts_with("--reporter")
        || arg.starts_with("--watch")
}

fn run_framework_test(invocation: TestInvocation, verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let framework = invocation.framework;
    let mut cmd = package_manager_exec(framework);
    cmd.args(&invocation.args);

    let result = exec_capture(&mut cmd).context(format!("Failed to run {}", framework))?;
    let report = read_vitest_file_report(framework, &result.stdout, invocation.passthrough)?;
    let combined = match &report {
        Some(report) => format!("{}\n{}", result.combined(), report),
        None => result.combined(),
    };

    let filtered = format_test_output(
        framework,
        report.as_deref().unwrap_or(&result.stdout),
        &combined,
        invocation.passthrough,
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

// Vitest 5 writes its JSON reporter to an artifact by default, whereas older
// releases print JSON. Consume the path reported by this invocation so caller
// outputFile settings and the artifact itself remain intact on every OS.
fn read_vitest_file_report(
    framework: &str,
    stdout: &str,
    passthrough_requested: bool,
) -> Result<Option<String>> {
    if framework != "vitest" || passthrough_requested {
        return Ok(None);
    }
    let clean = strip_ansi(stdout);
    let report_path = clean
        .lines()
        .filter_map(|line| line.strip_prefix("JSON report written to "))
        .next_back();
    report_path
        .map(|path| {
            std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read Vitest JSON report: {}", path))
        })
        .transpose()
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
            format_passthrough_output(combined)
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

    crate::core::runner::with_hint(&filtered.text, hint.as_deref()).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_loss_stderr_only_startup_failure_is_visible() {
        let raw = "OUTPUT_LOSS_MARKER: Vitest startup failed\n";
        let filtered = format_test_output("vitest", "", raw, false, 0);
        assert_eq!(filtered.text, raw);
    }

    #[test]
    fn output_loss_unparsed_stdout_and_stderr_are_both_visible() {
        let stdout = "unknown reporter output\n";
        let combined = format!("{stdout}OUTPUT_LOSS_MARKER: cannot load config\n");
        let filtered = format_test_output("vitest", stdout, &combined, false, 0);
        assert_eq!(filtered.text, combined);
    }

    #[test]
    fn output_loss_suite_load_failure_keeps_name_reason_and_test_counts() {
        let raw = r#"{"numTotalTests":0,"numPassedTests":0,"numFailedTests":0,"numPendingTests":0,"numFailedTestSuites":1,"testResults":[{"name":"broken.test.js","status":"failed","assertionResults":[],"message":"OUTPUT_LOSS_MARKER: Cannot load suite","testExecError":{"message":"Cannot load suite","stack":"at broken.test.js:3:1"}}]}"#;
        let result = VitestParser::parse(raw).unwrap();
        assert_eq!(result.total, 0);
        assert_eq!(
            result.failed, 0,
            "suite failures must not fabricate failed tests"
        );
        let formatted = result.format(FormatMode::Compact);
        assert!(
            formatted.contains("Suite failed: broken.test.js"),
            "{formatted}"
        );
        assert!(
            formatted.contains("OUTPUT_LOSS_MARKER: Cannot load suite"),
            "{formatted}"
        );
        assert!(formatted.contains("at broken.test.js:3:1"), "{formatted}");
    }

    #[test]
    fn output_loss_suite_failure_without_assertion_array_is_visible() {
        let raw = r#"{"numTotalTests":0,"numPassedTests":0,"numFailedTests":0,"testResults":[{"name":"broken.test.js","status":"failed","testExecError":{"message":"OUTPUT_LOSS_MARKER: Cannot find module"}}]}"#;
        let result = VitestParser::parse(raw).unwrap();
        let formatted = result.format(FormatMode::Compact);
        assert!(
            formatted.contains("Suite failed: broken.test.js"),
            "{formatted}"
        );
        assert!(
            formatted.contains("OUTPUT_LOSS_MARKER: Cannot find module"),
            "{formatted}"
        );
    }

    #[test]
    fn output_loss_passed_suite_message_is_not_a_failure() {
        let raw = r#"{"numTotalTests":1,"numPassedTests":1,"numFailedTests":0,"testResults":[{"name":"passed.test.js","status":"passed","message":"reporter note","assertionResults":[{"fullName":"adds","status":"passed","failureMessages":[]}]}]}"#;
        let result = VitestParser::parse(raw).unwrap();
        assert_eq!(result.passed, 1);
        assert!(result.failures.is_empty());
    }

    #[test]
    fn output_loss_vitest_file_report_is_read_without_changing_the_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report with spaces.json");
        let json = r#"{"numTotalTests":1,"numPassedTests":1,"numFailedTests":0,"testResults":[]}"#;
        std::fs::write(&path, json).unwrap();
        let stdout = format!("JSON report written to {}\n", path.display());
        let report = read_vitest_file_report("vitest", &stdout, false)
            .unwrap()
            .unwrap();
        let result = VitestParser::parse(&report).unwrap();
        assert_eq!(result.passed, 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), json);
        assert!(
            read_vitest_file_report("vitest", &stdout, true)
                .unwrap()
                .is_none()
        );
        assert!(
            read_vitest_file_report("jest", &stdout, false)
                .unwrap()
                .is_none()
        );
        std::fs::remove_file(&path).unwrap();
        assert!(read_vitest_file_report("vitest", &stdout, false).is_err());
    }

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn test_vitest_invocation_runs_vitest() {
        let invocation = vitest_invocation(&args(&["src/a.test.ts"]));

        assert_eq!(invocation.framework, "vitest");
    }

    #[test]
    fn test_jest_invocation_runs_jest_once_with_json_output() {
        let invocation = jest_invocation(&args(&["src/a.test.js"]));

        assert_eq!(invocation.framework, "jest");
        assert_eq!(
            invocation.args,
            args(&["--no-watch", "--json", "src/a.test.js"])
        );
        assert!(!invocation.passthrough);
    }

    #[test]
    fn test_jest_invocation_drops_run_json_reporter_and_watch_args() {
        let invocation = jest_invocation(&args(&[
            "run",
            "--json",
            "--reporters=default",
            "--watch",
            "-t",
            "adds",
            "src/a.test.js",
        ]));

        assert_eq!(
            invocation.args,
            args(&["--no-watch", "--json", "-t", "adds", "src/a.test.js"])
        );
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
        let effective = vitest_invocation(&args(&["run", "constants.test.ts", "--watch"]));

        assert!(!effective.passthrough);
        assert_eq!(
            effective.args,
            args(&["run", "--reporter=json", "constants.test.ts"])
        );
    }

    #[test]
    fn test_vitest_effective_args_preserve_explicit_reporter_equals() {
        let effective = vitest_invocation(&args(&["constants.test.ts", "--reporter=verbose"]));

        assert!(effective.passthrough);
        assert_eq!(
            effective.args,
            args(&["run", "constants.test.ts", "--reporter=verbose"])
        );
    }

    #[test]
    fn test_vitest_effective_args_preserve_explicit_reporter_value() {
        let effective = vitest_invocation(&args(&[
            "run",
            "constants.test.ts",
            "--reporter",
            "verbose",
        ]));

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

    /// The hint goes on the line after the report, reusing the report's own terminator,
    /// and stands alone when there is no report.
    #[test]
    fn test_vitest_hint_joins_without_a_blank_line() {
        let render = |text: &str| {
            render_test_output_with_hints(
                &FormattedTestOutput::new(text.to_string()),
                "raw",
                "vitest_run",
                1,
                |_, _| None,
                |_, _, _| Some("[full output: vitest_run.log]".to_string()),
            )
        };

        assert_eq!(
            render("Tests: 1 failed\n"),
            "Tests: 1 failed\n[full output: vitest_run.log]"
        );
        assert_eq!(render(""), "[full output: vitest_run.log]");
    }
}
