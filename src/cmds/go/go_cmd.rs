//! Filters Go command output — test results, build errors, vet warnings.

use crate::core::runner;
use crate::core::tracking;
use crate::core::utils::{exit_code_from_output, resolved_command, truncate};
use crate::golangci_cmd;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::ffi::OsString;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GoTestEvent {
    #[serde(rename = "Time")]
    time: Option<String>,
    #[serde(rename = "Action")]
    action: String,
    #[serde(rename = "Package")]
    package: Option<String>,
    #[serde(rename = "Test")]
    test: Option<String>,
    #[serde(rename = "Output")]
    output: Option<String>,
    #[serde(rename = "Elapsed")]
    elapsed: Option<f64>,
    #[serde(rename = "ImportPath")]
    import_path: Option<String>,
    #[serde(rename = "FailedBuild")]
    failed_build: Option<String>,
}

#[derive(Debug, Default)]
struct PackageResult {
    pass: usize,
    fail: usize,
    skip: usize,
    build_failed: bool,
    build_errors: Vec<String>,
    failed_tests: Vec<(String, Vec<String>)>, // (test_name, output_lines)
    package_failed: bool,                     // package-level failure (timeout, signal, etc.)
    package_fail_output: Vec<String>,         // output lines collected before the package fail
}

pub fn run_test(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("go");
    cmd.arg("test");

    if !args.iter().any(|a| a == "-json") {
        cmd.arg("-json");
    }

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: go test -json {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "go test",
        &args.join(" "),
        filter_go_test_json,
        crate::core::runner::RunOptions::stdout_only().tee("go_test"),
    )
}

pub fn run_build(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("go");
    cmd.arg("build");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: go build {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "go build",
        &args.join(" "),
        filter_go_build,
        crate::core::runner::RunOptions::with_tee("go_build"),
    )
}

pub fn run_vet(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("go");
    cmd.arg("vet");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: go vet {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "go vet",
        &args.join(" "),
        filter_go_vet,
        crate::core::runner::RunOptions::with_tee("go_vet"),
    )
}

pub fn run_other(args: &[OsString], verbose: u8) -> Result<i32> {
    if args.is_empty() {
        anyhow::bail!("go: no subcommand specified");
    }

    // Intercept: `go tool <known>` invocations for filtered output
    if let Some((tool, tool_args)) = match_go_tool(args) {
        match tool {
            GoTool::GolangciLint => return run_go_tool_golangci_lint(tool_args, verbose),
        }
    }

    let timer = tracking::TimedExecution::start();

    let subcommand = args[0].to_string_lossy();
    let mut cmd = resolved_command("go");
    cmd.arg(&*subcommand);

    for arg in &args[1..] {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: go {} ...", subcommand);
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run go {}", subcommand))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    print!("{}", stdout);
    eprint!("{}", stderr);

    timer.track(
        &format!("go {}", subcommand),
        &format!("rtk go {}", subcommand),
        &raw,
        &raw, // No filtering for unsupported commands
    );

    Ok(exit_code_from_output(&output, "go"))
}

/// Detect golangci-lint major version when invoked via `go tool`.
/// Returns 1 on any failure (safe fallback — v1 behaviour).
fn detect_go_tool_golangci_version() -> u32 {
    let output = resolved_command("go")
        .arg("tool")
        .arg("golangci-lint")
        .arg("--version")
        .output();

    match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            let version_text = if stdout.trim().is_empty() {
                &*stderr
            } else {
                &*stdout
            };
            golangci_cmd::parse_major_version(version_text)
        }
        Err(_) => 1,
    }
}

fn has_golangci_format_flag(args: &[OsString]) -> bool {
    args.iter().any(|a| {
        let s = a.to_string_lossy();
        s == "--out-format"
            || s.starts_with("--out-format=")
            || s == "--output.json.path"
            || s.starts_with("--output.json.path=")
    })
}

/// Known `go tool` subcommands that RTK provides filtered output for.
#[derive(Debug, Clone, Copy, PartialEq)]
enum GoTool {
    GolangciLint,
}

impl GoTool {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "golangci-lint" => Some(Self::GolangciLint),
            _ => None,
        }
    }
}

/// If the first arg is `tool` identify if it is a tool we already handle.
fn match_go_tool(args: &[OsString]) -> Option<(GoTool, &[OsString])> {
    if args.first().map(|a| a == "tool").unwrap_or(false) {
        if let Some(tool_arg) = args.get(1) {
            if let Some(tool) = GoTool::from_name(&tool_arg.to_string_lossy()) {
                return Some((tool, &args[2..]));
            }
        }
    }
    None
}

/// Run `go tool golangci-lint` and filter its output via the golangci JSON filter.
/// Reusing parts of golangci_cmd.
fn run_go_tool_golangci_lint(args: &[OsString], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let version = detect_go_tool_golangci_version();

    let mut cmd = resolved_command("go");
    cmd.arg("tool").arg("golangci-lint");

    let has_format = has_golangci_format_flag(args);

    if !has_format {
        if version >= 2 {
            cmd.arg("run").arg("--output.json.path").arg("stdout");
        } else {
            cmd.arg("run").arg("--out-format=json");
        }
    } else {
        cmd.arg("run");
    }

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        if version >= 2 {
            eprintln!("Running: go tool golangci-lint run --output.json.path stdout");
        } else {
            eprintln!("Running: go tool golangci-lint run --out-format=json");
        }
    }

    let output = cmd
        .output()
        .context("Failed to run go tool golangci-lint")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    // v2 outputs JSON on first line + trailing text; v1 outputs just JSON
    let json_output = if version >= 2 {
        stdout.lines().next().unwrap_or("")
    } else {
        &*stdout
    };

    let filtered = golangci_cmd::filter_golangci_json(json_output, version);
    println!("{}", filtered);

    if !stderr.trim().is_empty() && verbose > 0 {
        eprintln!("{}", stderr.trim());
    }

    timer.track(
        "go tool golangci-lint",
        "rtk go tool golangci-lint",
        &raw,
        &filtered,
    );

    let exit_code = exit_code_from_output(&output, "go tool golangci-lint");
    // golangci-lint: exit 0 = clean, exit 1 = lint issues found (not an error),
    // exit 2+ = config/build error, None = killed by signal (OOM, SIGKILL)
    Ok(if exit_code == 1 { 0 } else { exit_code })
}

/// Parse go test -json output (NDJSON format)
pub(crate) fn filter_go_test_json(output: &str) -> String {
    let mut packages: HashMap<String, PackageResult> = HashMap::new();
    let mut current_test_output: HashMap<(String, String), Vec<String>> = HashMap::new(); // (package, test) -> outputs
    let mut build_output: HashMap<String, Vec<String>> = HashMap::new(); // import_path -> error lines

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let event: GoTestEvent = match serde_json::from_str(trimmed) {
            Ok(e) => e,
            Err(_) => continue, // Skip non-JSON lines
        };

        // Handle build-output/build-fail events (use ImportPath, no Package)
        match event.action.as_str() {
            "build-output" => {
                if let (Some(import_path), Some(output_text)) = (&event.import_path, &event.output)
                {
                    let text = output_text.trim_end().to_string();
                    if !text.is_empty() {
                        build_output
                            .entry(import_path.clone())
                            .or_default()
                            .push(text);
                    }
                }
                continue;
            }
            "build-fail" => {
                // build-fail has ImportPath — we'll handle it when the package-level fail arrives
                continue;
            }
            _ => {}
        }

        let package = event.package.unwrap_or_else(|| "unknown".to_string());
        let pkg_result = packages.entry(package.clone()).or_default();

        match event.action.as_str() {
            "pass" => {
                if event.test.is_some() {
                    pkg_result.pass += 1;
                }
            }
            "fail" => {
                if let Some(test) = &event.test {
                    // Individual test failure
                    pkg_result.fail += 1;

                    // Collect output for failed test
                    let key = (package.clone(), test.clone());
                    let outputs = current_test_output.remove(&key).unwrap_or_default();
                    pkg_result.failed_tests.push((test.clone(), outputs));
                } else if event.failed_build.is_some() {
                    // Package-level build failure
                    pkg_result.build_failed = true;
                    // Collect build errors from the import path
                    if let Some(import_path) = &event.failed_build {
                        if let Some(errors) = build_output.remove(import_path) {
                            pkg_result.build_errors = errors;
                        }
                    }
                } else {
                    // Package-level failure without a specific test or build error
                    // (timeout, signal kill, panic before test execution, etc.)
                    pkg_result.package_failed = true;
                }
            }
            "skip" => {
                if event.test.is_some() {
                    pkg_result.skip += 1;
                }
            }
            "output" => {
                if let Some(output_text) = &event.output {
                    if let Some(test) = &event.test {
                        // Collect output for current test
                        let key = (package.clone(), test.clone());
                        current_test_output
                            .entry(key)
                            .or_default()
                            .push(output_text.trim_end().to_string());
                    } else {
                        // Package-level output (timeout messages, signal info, etc.)
                        let trimmed = output_text.trim();
                        if !trimmed.is_empty() {
                            pkg_result.package_fail_output.push(trimmed.to_string());
                        }
                    }
                }
            }
            _ => {} // run, pause, cont, etc.
        }
    }

    // Build summary
    let total_packages = packages.len();
    let total_pass: usize = packages.values().map(|p| p.pass).sum();
    let total_fail: usize = packages.values().map(|p| p.fail).sum();
    let total_skip: usize = packages.values().map(|p| p.skip).sum();
    let total_build_fail: usize = packages.values().filter(|p| p.build_failed).count();
    // Only count package-level fails for packages with no individual test or build failures.
    // go test -json emits a trailing package-level {"action":"fail"} after any test failure
    // too, but that event is just a cascade — the individual test failures are already counted.
    let total_pkg_fail: usize = packages
        .values()
        .filter(|p| p.package_failed && p.fail == 0 && !p.build_failed)
        .count();

    let has_failures = total_fail > 0 || total_build_fail > 0 || total_pkg_fail > 0;

    if !has_failures && total_pass == 0 {
        return "Go test: No tests found".to_string();
    }

    if !has_failures {
        return format!(
            "Go test: {} passed in {} packages",
            total_pass, total_packages
        );
    }

    let mut result = String::new();
    result.push_str(&format!(
        "Go test: {} passed, {} failed",
        total_pass,
        total_fail + total_build_fail + total_pkg_fail
    ));
    if total_skip > 0 {
        result.push_str(&format!(", {} skipped", total_skip));
    }
    result.push_str(&format!(" in {} packages\n", total_packages));
    result.push_str("═══════════════════════════════════════\n");

    // Show package-level failures first (timeouts, signals, panics).
    // Skip packages that already have individual test-level failures — those are displayed
    // in the per-package section below and the package-level event is just a cascade.
    for (package, pkg_result) in packages.iter() {
        if !pkg_result.package_failed || pkg_result.fail > 0 || pkg_result.build_failed {
            continue;
        }

        result.push_str(&format!("\n{} [FAIL]\n", compact_package_name(package)));

        for line in &pkg_result.package_fail_output {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                result.push_str(&format!("  {}\n", truncate(trimmed, 120)));
            }
        }
    }

    // Show build failures
    for (package, pkg_result) in packages.iter() {
        if !pkg_result.build_failed {
            continue;
        }

        result.push_str(&format!(
            "\n{} [build failed]\n",
            compact_package_name(package)
        ));

        for line in &pkg_result.build_errors {
            let trimmed = line.trim();
            // Skip the "# package" header line
            if !trimmed.starts_with('#') && !trimmed.is_empty() {
                result.push_str(&format!("  {}\n", truncate(trimmed, 120)));
            }
        }
    }

    // Show failed tests grouped by package
    for (package, pkg_result) in packages.iter() {
        if pkg_result.fail == 0 {
            continue;
        }

        result.push_str(&format!(
            "\n{} ({} passed, {} failed)\n",
            compact_package_name(package),
            pkg_result.pass,
            pkg_result.fail
        ));

        for (test, outputs) in &pkg_result.failed_tests {
            result.push_str(&format!("  [FAIL] {}\n", test));

            for line in select_go_test_failure_lines(outputs) {
                result.push_str(&format!("     {}\n", truncate(&line, 100)));
            }
        }
    }

    result.trim().to_string()
}

fn select_go_test_failure_lines(outputs: &[String]) -> Vec<String> {
    let mut relevant = Vec::new();
    let mut keep_next_context_line = false;

    for line in outputs {
        let trimmed = line.trim();

        if trimmed.is_empty()
            || trimmed.starts_with("=== RUN")
            || trimmed.starts_with("--- FAIL")
            || trimmed.starts_with("--- PASS")
        {
            keep_next_context_line = false;
            continue;
        }

        let is_location = is_go_test_location_line(trimmed);
        let is_failure = is_go_test_failure_line(trimmed);

        if is_location || is_failure || keep_next_context_line {
            relevant.push(trimmed.to_string());
            keep_next_context_line = is_location;
        } else {
            keep_next_context_line = false;
        }

        if relevant.len() >= 5 {
            break;
        }
    }

    if relevant.is_empty() {
        if let Some(line) = outputs.iter().map(|line| line.trim()).find(|line| {
            !line.is_empty()
                && !line.starts_with("=== RUN")
                && !line.starts_with("--- FAIL")
                && !line.starts_with("--- PASS")
        }) {
            relevant.push(line.to_string());
        }
    }

    relevant
}

fn is_go_test_location_line(line: &str) -> bool {
    if let Some((_, rest)) = line.split_once(".go:") {
        rest.chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
    } else {
        false
    }
}

fn is_go_test_failure_line(line: &str) -> bool {
    let lower = line.to_lowercase();

    lower.starts_with("panic:")
        || lower.starts_with("error:")
        || lower.contains(" error:")
        || lower.contains("expected")
        || lower.contains("got")
        || lower.contains("want")
        || lower.contains("actual")
        || lower.contains("assert")
        || lower.contains("mismatch")
        || lower.contains("unexpected")
        || lower.contains("fatal")
        || line.starts_with("at ")
}

/// Filter go build output - show only errors
pub(crate) fn filter_go_build(output: &str) -> String {
    let mut errors: Vec<String> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if is_go_build_error_line(trimmed) {
            errors.push(trimmed.to_string());
        }
    }

    if errors.is_empty() {
        return "Go build: Success".to_string();
    }

    let mut result = String::new();
    result.push_str(&format!("Go build: {} errors\n", errors.len()));
    result.push_str("═══════════════════════════════════════\n");

    for (i, error) in errors.iter().take(20).enumerate() {
        result.push_str(&format!("{}. {}\n", i + 1, truncate(error, 120)));
    }

    if errors.len() > 20 {
        result.push_str(&format!("\n... +{} more errors\n", errors.len() - 20));
    }

    result.trim().to_string()
}

fn is_go_build_error_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }

    let lower = trimmed.to_lowercase();

    // Go download/progress lines often contain package names like pkg/errors,
    // xerrors, or multierror. These are not compilation failures.
    if lower.starts_with("go: downloading ")
        || lower.starts_with("go: finding ")
        || lower.starts_with("go: extracting ")
    {
        return false;
    }

    // Package headers are context, not errors by themselves.
    if trimmed.starts_with('#') {
        return false;
    }

    // Canonical compiler/config error locations: file:line:col: ...
    let is_go_config_location = !lower.starts_with("go: ")
        && (lower.contains("go.mod:")
            || lower.contains("go.work:")
            || lower.contains("go.sum:"));
    if trimmed.contains(".go:") || is_go_config_location {
        return true;
    }

    // Some compiler/module failures do not include a file.go:line:col location.
    let non_file_error_prefixes = [
        "undefined: ",
        "cannot use ",
        "cannot find package ",
        "no required module provides package ",
        "missing go.sum entry for module providing package ",
        "found packages ",
        "go: go.mod file not found in current directory or any parent directory",
        "go: cannot load module ",
        "go: build failed",
        "go: error ",
        "error: ",
        "go: updates to go.mod needed",
        "go: inconsistent vendoring",
        "no go files in ",
    ];

    non_file_error_prefixes
        .iter()
        .any(|prefix| lower.starts_with(prefix))
        || lower.contains("import cycle not allowed")
        || lower.contains("build constraints exclude all go files")
        || lower.contains("function main is undeclared in the main package")
}

/// Filter go vet output - show issues
fn filter_go_vet(output: &str) -> String {
    let mut issues: Vec<String> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // Collect issue lines (vet reports issues with file:line:col format)
        if !trimmed.is_empty() && !trimmed.starts_with('#') && trimmed.contains(".go:") {
            issues.push(trimmed.to_string());
        }
    }

    if issues.is_empty() {
        return "Go vet: No issues found".to_string();
    }

    let mut result = String::new();
    result.push_str(&format!("Go vet: {} issues\n", issues.len()));
    result.push_str("═══════════════════════════════════════\n");

    for (i, issue) in issues.iter().take(20).enumerate() {
        result.push_str(&format!("{}. {}\n", i + 1, truncate(issue, 120)));
    }

    if issues.len() > 20 {
        result.push_str(&format!("\n... +{} more issues\n", issues.len() - 20));
    }

    result.trim().to_string()
}

/// Compact package name (remove long paths)
fn compact_package_name(package: &str) -> String {
    // Remove common module prefixes like github.com/user/repo/
    if let Some(pos) = package.rfind('/') {
        package[pos + 1..].to_string()
    } else {
        package.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_go_test_all_pass() {
        let output = r#"{"Time":"2024-01-01T10:00:00Z","Action":"run","Package":"example.com/foo","Test":"TestBar"}
{"Time":"2024-01-01T10:00:01Z","Action":"output","Package":"example.com/foo","Test":"TestBar","Output":"=== RUN   TestBar\n"}
{"Time":"2024-01-01T10:00:02Z","Action":"pass","Package":"example.com/foo","Test":"TestBar","Elapsed":0.5}
{"Time":"2024-01-01T10:00:02Z","Action":"pass","Package":"example.com/foo","Elapsed":0.5}"#;

        let result = filter_go_test_json(output);
        assert!(result.contains("Go test"));
        assert!(result.contains("1 passed"));
        assert!(result.contains("1 packages"));
    }

    #[test]
    fn test_filter_go_test_with_failures() {
        let output = r#"{"Time":"2024-01-01T10:00:00Z","Action":"run","Package":"example.com/foo","Test":"TestFail"}
{"Time":"2024-01-01T10:00:01Z","Action":"output","Package":"example.com/foo","Test":"TestFail","Output":"=== RUN   TestFail\n"}
{"Time":"2024-01-01T10:00:02Z","Action":"output","Package":"example.com/foo","Test":"TestFail","Output":"    Error: expected 5, got 3\n"}
{"Time":"2024-01-01T10:00:03Z","Action":"fail","Package":"example.com/foo","Test":"TestFail","Elapsed":0.5}
{"Time":"2024-01-01T10:00:03Z","Action":"fail","Package":"example.com/foo","Elapsed":0.5}"#;

        let result = filter_go_test_json(output);
        assert!(result.contains("1 failed"));
        assert!(result.contains("TestFail"));
        assert!(result.contains("expected 5, got 3"));
    }

    #[test]
    fn test_filter_go_test_preserves_file_location_and_followup_context() {
        let output = r#"{"Time":"2024-01-01T10:00:00Z","Action":"run","Package":"example.com/foo","Test":"TestFail"}
{"Time":"2024-01-01T10:00:01Z","Action":"output","Package":"example.com/foo","Test":"TestFail","Output":"=== RUN   TestFail\n"}
{"Time":"2024-01-01T10:00:02Z","Action":"output","Package":"example.com/foo","Test":"TestFail","Output":"    foo_test.go:42:\n"}
{"Time":"2024-01-01T10:00:03Z","Action":"output","Package":"example.com/foo","Test":"TestFail","Output":"        values differ after normalization\n"}
{"Time":"2024-01-01T10:00:04Z","Action":"fail","Package":"example.com/foo","Test":"TestFail","Elapsed":0.5}
{"Time":"2024-01-01T10:00:04Z","Action":"fail","Package":"example.com/foo","Elapsed":0.5}"#;

        let result = filter_go_test_json(output);
        assert!(result.contains("foo_test.go:42:"));
        assert!(result.contains("values differ after normalization"));
    }

    #[test]
    fn test_filter_go_test_timeout_package_fail() {
        // When go test times out, the JSON stream has a package-level "fail"
        // with no Test field and no FailedBuild field. This should be reported
        // as a failure, not "No tests found".
        let output = r#"{"Time":"2024-01-01T10:00:00Z","Action":"start","Package":"example.com/foo"}
{"Time":"2024-01-01T10:01:03Z","Action":"output","Package":"example.com/foo","Output":"*** Test killed with quit: ran too long (1m3s).\n"}
{"Time":"2024-01-01T10:01:03Z","Action":"output","Package":"example.com/foo","Output":"FAIL\texample.com/foo\t63.001s\n"}
{"Time":"2024-01-01T10:01:03Z","Action":"fail","Package":"example.com/foo","Elapsed":63.003}"#;

        let result = filter_go_test_json(output);
        assert!(
            result.contains("1 failed"),
            "Expected '1 failed' in output, got: {}",
            result
        );
        assert!(
            !result.contains("No tests found"),
            "Should not say 'No tests found' on timeout, got: {}",
            result
        );
        assert!(
            result.contains("FAIL"),
            "Expected failure output in summary, got: {}",
            result
        );
    }

    #[test]
    fn test_filter_go_test_no_double_count_on_test_failure() {
        // go test -json always emits a package-level {"action":"fail"} after each
        // test-level failure. The package-level event is a cascade, not an additional
        // failure. The summary header must show "1 failed", not "2 failed".
        let output = r#"{"Time":"2024-01-01T10:00:00Z","Action":"run","Package":"example.com/foo","Test":"TestFail"}
{"Time":"2024-01-01T10:00:01Z","Action":"output","Package":"example.com/foo","Test":"TestFail","Output":"=== RUN   TestFail\n"}
{"Time":"2024-01-01T10:00:02Z","Action":"output","Package":"example.com/foo","Test":"TestFail","Output":"    Error: expected 5, got 3\n"}
{"Time":"2024-01-01T10:00:03Z","Action":"fail","Package":"example.com/foo","Test":"TestFail","Elapsed":0.5}
{"Time":"2024-01-01T10:00:03Z","Action":"fail","Package":"example.com/foo","Elapsed":0.5}"#;

        let result = filter_go_test_json(output);
        // The summary header must say "1 failed", not "2 failed" (no double-counting).
        assert!(
            result.starts_with("Go test: 0 passed, 1 failed"),
            "Expected header 'Go test: 0 passed, 1 failed', got: {}",
            result
        );
        assert!(result.contains("TestFail"));
        assert!(result.contains("expected 5, got 3"));
        // The package must NOT appear twice (once as "[FAIL]" and once with test details).
        assert_eq!(
            result.matches("foo").count(),
            1,
            "Package name should appear exactly once, got: {}",
            result
        );
    }

    #[test]
    fn test_filter_go_test_timeout_with_signal_quit_output() {
        // Exact reproduction of the scenario from issue #958: the signal: quit line
        // appears as a separate JSON output event.
        let output = r#"{"Action":"start","Package":"example.com/pkg"}
{"Action":"output","Package":"example.com/pkg","Output":"*** Test killed with quit: ran too long (1m30s).\n"}
{"Action":"output","Package":"example.com/pkg","Output":"signal: quit\n"}
{"Action":"output","Package":"example.com/pkg","Output":"FAIL\texample.com/pkg\t90.000s\n"}
{"Action":"fail","Package":"example.com/pkg","Elapsed":90.001}"#;

        let result = filter_go_test_json(output);
        assert!(
            result.starts_with("Go test: 0 passed, 1 failed"),
            "Expected 'Go test: 0 passed, 1 failed', got: {}",
            result
        );
        assert!(
            !result.contains("No tests found"),
            "Must not say 'No tests found' on timeout, got: {}",
            result
        );
        assert!(
            result.contains("Test killed with quit"),
            "Should show the timeout message, got: {}",
            result
        );
    }

    #[test]
    fn test_filter_go_test_timeout_with_passing_tests_before_kill() {
        // Some tests pass before the package times out.
        // Summary should show both pass and fail counts.
        let output = r#"{"Action":"run","Package":"example.com/foo","Test":"TestFast"}
{"Action":"pass","Package":"example.com/foo","Test":"TestFast","Elapsed":0.001}
{"Action":"run","Package":"example.com/foo","Test":"TestHang"}
{"Action":"output","Package":"example.com/foo","Output":"*** Test killed with quit: ran too long (30s).\n"}
{"Action":"fail","Package":"example.com/foo","Elapsed":30.001}"#;

        let result = filter_go_test_json(output);
        assert!(
            result.starts_with("Go test: 1 passed, 1 failed"),
            "Expected 'Go test: 1 passed, 1 failed', got: {}",
            result
        );
        assert!(
            !result.contains("No tests found"),
            "Must not say 'No tests found', got: {}",
            result
        );
        assert!(
            result.contains("Test killed with quit"),
            "Should show timeout message, got: {}",
            result
        );
    }

    #[test]
    fn test_filter_go_build_success() {
        let output = "";
        let result = filter_go_build(output);
        assert!(result.contains("Go build"));
        assert!(result.contains("Success"));
    }

    #[test]
    fn test_filter_go_build_errors() {
        let output = r#"# example.com/foo
main.go:10:5: undefined: missingFunc
main.go:15:2: cannot use x (type int) as type string"#;

        let result = filter_go_build(output);
        assert!(result.contains("2 errors"));
        assert!(result.contains("undefined: missingFunc"));
        assert!(result.contains("cannot use x"));
    }

    #[test]
    fn test_filter_go_build_ignores_download_lines_with_error_in_package_names() {
        let output = r#"go: downloading github.com/go-errors/errors v1.5.1
go: finding module for package example.com/foo
go: extracting github.com/pkg/errors v0.9.1
go: downloading github.com/pkg/errors v0.9.1
go: downloading github.com/hashicorp/go-multierror v1.1.1
go: downloading golang.org/x/xerrors v0.0.0-20220907171357-04be3eba64a2"#;

        let result = filter_go_build(output);
        assert_eq!(result, "Go build: Success");
    }

    #[test]
    fn test_is_go_build_error_line_recognizes_real_compiler_errors() {
        assert!(is_go_build_error_line("undefined: missingFunc"));
        assert!(is_go_build_error_line(
            "cannot find package \"foo/bar\""
        ));
        assert!(is_go_build_error_line(
            "found packages a (a.go) and b (b.go) in /tmp/rtk-go-build-probe-mix"
        ));
        assert!(is_go_build_error_line(
            "imports example.com/cycle/a: import cycle not allowed"
        ));
        assert!(is_go_build_error_line(
            "package example.com/buildtag: build constraints exclude all Go files in /tmp/rtk-go-build-probe-buildtag"
        ));
        assert!(is_go_build_error_line(
            "go.mod:3: invalid go version 'not-a-version': must match format 1.23.0"
        ));
        assert!(is_go_build_error_line(
            "go.work:1: invalid go version 'not-a-version': must match format 1.23.0"
        ));
        assert!(is_go_build_error_line(
            "go: go.mod file not found in current directory or any parent directory; see 'go help modules'"
        ));
        assert!(is_go_build_error_line("no Go files in /tmp/example"));
        assert!(is_go_build_error_line(
            "go: cannot load module missing listed in go.work file: open missing/go.mod: no such file or directory"
        ));
        assert!(is_go_build_error_line(
            "runtime.main_main·f: function main is undeclared in the main package"
        ));
        assert!(is_go_build_error_line(
            "main.go:10:5: undefined: missingFunc"
        ));
        assert!(is_go_build_error_line("error: failed to load module"));
        assert!(!is_go_build_error_line(
            "go: downloading github.com/pkg/errors v0.9.1"
        ));
        assert!(!is_go_build_error_line(
            "go: finding module for package example.com/foo"
        ));
        assert!(!is_go_build_error_line(
            "go: extracting github.com/pkg/errors v0.9.1"
        ));
        assert!(!is_go_build_error_line("# example.com/foo"));
    }

    #[test]
    fn test_filter_go_build_preserves_non_file_error_shapes() {
        let output = r#"undefined: missingFunc
cannot find package "foo/bar"
found packages a (a.go) and b (b.go) in /tmp/rtk-go-build-probe-mix
imports example.com/cycle/a: import cycle not allowed
package example.com/buildtag: build constraints exclude all Go files in /tmp/rtk-go-build-probe-buildtag
runtime.main_main·f: function main is undeclared in the main package"#;

        let result = filter_go_build(output);
        assert!(result.contains("6 errors"));
        assert!(result.contains("undefined: missingFunc"));
        assert!(result.contains("cannot find package \"foo/bar\""));
        assert!(result.contains("found packages a (a.go) and b (b.go)"));
        assert!(result.contains("import cycle not allowed"));
        assert!(result.contains("build constraints exclude all Go files"));
        assert!(result.contains("function main is undeclared in the main package"));
    }

    #[test]
    fn test_filter_go_build_preserves_go_config_parse_errors() {
        let output = r#"go: errors parsing go.mod:
go.mod:3: invalid go version 'not-a-version': must match format 1.23.0
go: errors parsing go.work:
go.work:1: invalid go version 'not-a-version': must match format 1.23.0"#;

        let result = filter_go_build(output);
        assert!(result.contains("2 errors"));
        assert!(result.contains("go.mod:3: invalid go version"));
        assert!(result.contains("go.work:1: invalid go version"));
        assert!(!result.contains("go: errors parsing go.mod:"));
        assert!(!result.contains("go: errors parsing go.work:"));
    }

    #[test]
    fn test_filter_go_build_preserves_module_root_and_workspace_errors() {
        let output = r#"go: go.mod file not found in current directory or any parent directory; see 'go help modules'
no Go files in /tmp/example
go: cannot load module missing listed in go.work file: open missing/go.mod: no such file or directory"#;

        let result = filter_go_build(output);
        assert!(result.contains("3 errors"));
        assert!(result.contains("go.mod file not found in current directory or any parent directory"));
        assert!(result.contains("no Go files in /tmp/example"));
        assert!(result.contains("go: cannot load module missing listed in go.work file"));
    }

    #[test]
    fn test_filter_go_vet_no_issues() {
        let output = "";
        let result = filter_go_vet(output);
        assert!(result.contains("Go vet"));
        assert!(result.contains("No issues found"));
    }

    #[test]
    fn test_filter_go_vet_with_issues() {
        let output = r#"main.go:42:2: Printf format %d has arg x of wrong type string
utils.go:15:5: unreachable code"#;

        let result = filter_go_vet(output);
        assert!(result.contains("2 issues"));
        assert!(result.contains("Printf format"));
        assert!(result.contains("unreachable code"));
    }

    #[test]
    fn test_compact_package_name() {
        assert_eq!(compact_package_name("github.com/user/repo/pkg"), "pkg");
        assert_eq!(compact_package_name("example.com/foo"), "foo");
        assert_eq!(compact_package_name("simple"), "simple");
    }

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn test_match_go_tool_golangci_lint() {
        let args = os(&["tool", "golangci-lint", "run", "./..."]);
        let (tool, rest) = match_go_tool(&args).expect("should match");
        assert_eq!(tool, GoTool::GolangciLint);
        assert_eq!(rest.len(), 2); // ["run", "./..."]
    }

    #[test]
    fn test_match_go_tool_bare() {
        let args = os(&["tool", "golangci-lint"]);
        let (tool, rest) = match_go_tool(&args).expect("should match");
        assert_eq!(tool, GoTool::GolangciLint);
        assert!(rest.is_empty());
    }

    #[test]
    fn test_match_go_tool_rejects_unknown() {
        assert!(match_go_tool(&os(&["tool", "pprof"])).is_none());
        assert!(match_go_tool(&os(&["tool"])).is_none());
        assert!(match_go_tool(&os(&["test", "./..."])).is_none());
        assert!(match_go_tool(&os(&[])).is_none());
    }

    #[test]
    fn test_has_golangci_format_flag_v1() {
        assert!(has_golangci_format_flag(&os(&["--out-format=json"])));
        assert!(has_golangci_format_flag(&os(&[
            "./...",
            "--out-format",
            "json"
        ])));
    }

    #[test]
    fn test_has_golangci_format_flag_v2() {
        assert!(has_golangci_format_flag(&os(&[
            "--output.json.path",
            "stdout"
        ])));
        assert!(has_golangci_format_flag(&os(&[
            "--output.json.path=stdout"
        ])));
    }

    #[test]
    fn test_has_golangci_format_flag_absent() {
        assert!(!has_golangci_format_flag(&os(&["run", "./..."])));
        assert!(!has_golangci_format_flag(&os(&[])));
        assert!(!has_golangci_format_flag(&os(&["--fix"])));
    }
}
