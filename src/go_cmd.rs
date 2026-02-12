use crate::tracking;
use crate::utils::truncate;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::ffi::OsString;
use std::process::Command;

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
}

#[derive(Debug, Default)]
struct PackageResult {
    pass: usize,
    fail: usize,
    skip: usize,
    failed_tests: Vec<(String, Vec<String>)>, // (test_name, output_lines)
}

pub fn run_test(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("go");
    cmd.arg("test");

    // Force JSON output if not already specified
    if !args.iter().any(|a| a == "-json") {
        cmd.arg("-json");
    }

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: go test -json {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run go test. Is Go installed?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = filter_go_test_json(&stdout);

    println!("{}", filtered);

    // Include stderr if present (build errors, etc.)
    if !stderr.trim().is_empty() {
        eprintln!("{}", stderr.trim());
    }

    timer.track(
        &format!("go test {}", args.join(" ")),
        &format!("rtk go test {}", args.join(" ")),
        &raw,
        &filtered,
    );

    // Preserve exit code for CI/CD
    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

pub fn run_build(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("go");
    cmd.arg("build");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: go build {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run go build. Is Go installed?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = filter_go_build(&raw);

    if !filtered.is_empty() {
        println!("{}", filtered);
    }

    timer.track(
        &format!("go build {}", args.join(" ")),
        &format!("rtk go build {}", args.join(" ")),
        &raw,
        &filtered,
    );

    // Preserve exit code for CI/CD
    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

pub fn run_vet(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("go");
    cmd.arg("vet");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: go vet {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run go vet. Is Go installed?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = filter_go_vet(&raw);

    if !filtered.is_empty() {
        println!("{}", filtered);
    }

    timer.track(
        &format!("go vet {}", args.join(" ")),
        &format!("rtk go vet {}", args.join(" ")),
        &raw,
        &filtered,
    );

    // Preserve exit code for CI/CD
    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

pub fn run_other(args: &[OsString], verbose: u8) -> Result<()> {
    if args.is_empty() {
        anyhow::bail!("go: no subcommand specified");
    }

    let timer = tracking::TimedExecution::start();

    let subcommand = args[0].to_string_lossy();
    let mut cmd = Command::new("go");
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

    // Preserve exit code
    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

/// Parse go test -json output (NDJSON format)
fn filter_go_test_json(output: &str) -> String {
    let mut packages: HashMap<String, PackageResult> = HashMap::new();
    let mut current_test_output: HashMap<(String, String), Vec<String>> = HashMap::new(); // (package, test) -> outputs

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let event: GoTestEvent = match serde_json::from_str(trimmed) {
            Ok(e) => e,
            Err(_) => continue, // Skip non-JSON lines
        };

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
                    pkg_result.fail += 1;

                    // Collect output for failed test
                    let key = (package.clone(), test.clone());
                    let outputs = current_test_output.remove(&key).unwrap_or_default();
                    pkg_result.failed_tests.push((test.clone(), outputs));
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

    if total_fail == 0 && total_pass == 0 {
        return "Go test: No tests found".to_string();
    }

    if total_fail == 0 {
        return format!(
            "✓ Go test: {} passed in {} packages",
            total_pass, total_packages
        );
    }

    let mut result = String::new();
    result.push_str(&format!(
        "Go test: {} passed, {} failed",
        total_pass, total_fail
    ));
    if total_skip > 0 {
        result.push_str(&format!(", {} skipped", total_skip));
    }
    result.push_str(&format!(" in {} packages\n", total_packages));
    result.push_str("═══════════════════════════════════════\n");

    // Show failed tests grouped by package
    for (package, pkg_result) in packages.iter() {
        if pkg_result.fail == 0 {
            continue;
        }

        result.push_str(&format!(
            "\n📦 {} ({} passed, {} failed)\n",
            compact_package_name(package),
            pkg_result.pass,
            pkg_result.fail
        ));

        for (test, outputs) in &pkg_result.failed_tests {
            result.push_str(&format!("  ❌ {}\n", test));

            // Show failure output (limit to key lines)
            let relevant_lines: Vec<&String> = outputs
                .iter()
                .filter(|line| {
                    let lower = line.to_lowercase();
                    !line.trim().is_empty()
                        && !line.starts_with("=== RUN")
                        && !line.starts_with("--- FAIL")
                        && (lower.contains("error")
                            || lower.contains("expected")
                            || lower.contains("got")
                            || lower.contains("panic")
                            || line.trim().starts_with("at "))
                })
                .take(5)
                .collect();

            for line in relevant_lines {
                result.push_str(&format!("     {}\n", truncate(line, 100)));
            }
        }
    }

    result.trim().to_string()
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
        return "✓ Go build: Success".to_string();
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
        return "✓ Go vet: No issues found".to_string();
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
        assert!(result.contains("✓ Go test"));
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
    fn test_filter_go_build_success() {
        let output = "";
        let result = filter_go_build(output);
        assert!(result.contains("✓ Go build"));
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
        assert!(result.contains("✓ Go vet"));
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
}
