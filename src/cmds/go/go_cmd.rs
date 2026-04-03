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
fn filter_go_test_json(output: &str) -> String {
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
                }
            }
            "skip" => {
                if event.test.is_some() {
                    pkg_result.skip += 1;
                }
            }
            "output" => {
                // Collect output for current test
                if let (Some(test), Some(output_text)) = (&event.test, &event.output) {
                    let key = (package.clone(), test.clone());
                    current_test_output
                        .entry(key)
                        .or_default()
                        .push(output_text.trim_end().to_string());
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

    let has_failures = total_fail > 0 || total_build_fail > 0;

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
        total_fail + total_build_fail
    ));
    if total_skip > 0 {
        result.push_str(&format!(", {} skipped", total_skip));
    }
    result.push_str(&format!(" in {} packages\n", total_packages));
    result.push_str("═══════════════════════════════════════\n");

    // Show build failures first
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
fn filter_go_build(output: &str) -> String {
    let mut errors: Vec<String> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_lowercase();

        // Skip package markers (# package/name lines without errors)
        if trimmed.starts_with('#') && !lower.contains("error") {
            continue;
        }

        // Collect error lines (file:line:col format or error keywords)
        if !trimmed.is_empty()
            && (lower.contains("error")
                || trimmed.contains(".go:")
                || lower.contains("undefined")
                || lower.contains("cannot"))
        {
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
