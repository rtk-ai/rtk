use crate::tracking;
use crate::utils::strip_ansi;
use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashMap;
use std::process::Command;

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
        // Use pnpm exec - preserves CWD correctly
        let mut c = Command::new("pnpm");
        c.arg("exec");
        c.arg("--"); // Separator to prevent pnpm from interpreting tool args
        c.arg("playwright");
        c
    } else if is_yarn {
        // Use yarn exec - preserves CWD correctly
        let mut c = Command::new("yarn");
        c.arg("exec");
        c.arg("--"); // Separator
        c.arg("playwright");
        c
    } else {
        // Fallback to npx
        let mut c = Command::new("npx");
        c.arg("--no-install");
        c.arg("--"); // Separator
        c.arg("playwright");
        c
    };

    // Add user arguments
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

    let filtered = filter_playwright_output(&raw);

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

#[derive(Debug)]
struct TestResult {
    spec: String,
    passed: bool,
    // TODO: Use duration in detailed reports (token-efficient summary doesn't need it)
    #[allow(dead_code)]
    duration: Option<f64>,
}

/// Filter Playwright output - show only failures and summary stats
fn filter_playwright_output(output: &str) -> String {
    lazy_static::lazy_static! {
        // EXCEPTION: Static regex patterns, validated at compile time
        // Unwrap is safe here - panic indicates programming error caught during development

        // Pattern: ✓ [chromium] › auth/login.spec.ts:5:1 › should login (2.3s)
        static ref TEST_PATTERN: Regex = Regex::new(
            r"[✓✗×].*›\s+([^›]+\.spec\.[tj]sx?).*?(?:\((\d+(?:\.\d+)?)(ms|s)\))?"
        ).unwrap();

        // Pattern: Slow test file [chromium] › sessions/video.spec.ts (8.5s)
        static ref SLOW_TEST: Regex = Regex::new(
            r"Slow test.*?›\s+([^›]+\.spec\.[tj]sx?)\s+\((\d+(?:\.\d+)?)(ms|s)\)"
        ).unwrap();

        // Pattern: 45 passed (45.2s) or 2 failed, 43 passed
        static ref SUMMARY: Regex = Regex::new(
            r"(\d+)\s+(passed|failed|flaky|skipped)"
        ).unwrap();
    }

    let clean_output = strip_ansi(output);

    let mut tests: Vec<TestResult> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    let mut slow_tests: Vec<(String, f64)> = Vec::new();
    let mut passed = 0;
    let mut failed = 0;
    let mut _skipped = 0;
    let mut total_duration = String::new();

    // Parse test results
    for line in clean_output.lines() {
        // Detect failures (lines starting with × or ✗)
        if line.trim_start().starts_with('×') || line.trim_start().starts_with('✗') {
            if let Some(caps) = TEST_PATTERN.captures(line) {
                let spec = caps[1].to_string();
                failures.push(spec.clone());
                tests.push(TestResult {
                    spec,
                    passed: false,
                    duration: None,
                });
            }
        }

        // Detect successes
        if line.trim_start().starts_with('✓') {
            if let Some(caps) = TEST_PATTERN.captures(line) {
                let spec = caps[1].to_string();
                let duration = if caps.get(2).is_some() {
                    let time: f64 = caps[2].parse().unwrap_or(0.0);
                    let unit = &caps[3];
                    Some(if unit == "ms" { time / 1000.0 } else { time })
                } else {
                    None
                };

                tests.push(TestResult {
                    spec,
                    passed: true,
                    duration,
                });
            }
        }

        // Detect slow tests
        if let Some(caps) = SLOW_TEST.captures(line) {
            let spec = caps[1].to_string();
            let time: f64 = caps[2].parse().unwrap_or(0.0);
            let unit = &caps[3];
            let duration = if unit == "ms" { time / 1000.0 } else { time };
            slow_tests.push((spec, duration));
        }

        // Parse summary
        if line.contains("passed") || line.contains("failed") || line.contains("skipped") {
            for caps in SUMMARY.captures_iter(line) {
                let count: usize = caps[1].parse().unwrap_or(0);
                match &caps[2] {
                    "passed" => passed = count,
                    "failed" => failed = count,
                    "skipped" => _skipped = count,
                    _ => {}
                }
            }

            // Extract total duration
            if let Some(time_match) = extract_duration(line) {
                total_duration = time_match;
            }
        }
    }

    // Build filtered output
    let mut result = String::new();

    if failed == 0 && passed > 0 {
        result.push_str(&format!(
            "✓ Playwright: {} passed, {} failed",
            passed, failed
        ));
        if !total_duration.is_empty() {
            result.push_str(&format!(" ({})", total_duration));
        }
        result.push_str("\n═══════════════════════════════════════\n");
        result.push_str("All tests passed\n");
    } else if failed > 0 {
        result.push_str(&format!(
            "Playwright: {} passed, {} failed",
            passed, failed
        ));
        if !total_duration.is_empty() {
            result.push_str(&format!(" ({})", total_duration));
        }
        result.push_str("\n═══════════════════════════════════════\n");

        result.push_str(&format!("❌ {} test(s) failed:\n", failed));
        for failure in failures.iter().take(10) {
            result.push_str(&format!("  {}\n", failure));
        }

        if failures.len() > 10 {
            result.push_str(&format!("\n... +{} more failures\n", failures.len() - 10));
        }
    } else {
        // No test results found, return raw summary
        return clean_output
            .lines()
            .filter(|l| l.contains("passed") || l.contains("failed") || l.contains("Running"))
            .collect::<Vec<_>>()
            .join("\n");
    }

    // Add slow tests section
    if !slow_tests.is_empty() {
        result.push_str("\nSlow tests (>5s):\n");
        for (spec, duration) in slow_tests.iter().take(5) {
            result.push_str(&format!("  {} ({:.1}s)\n", spec, duration));
        }
    }

    // Group tests by spec directory
    let mut by_spec: HashMap<String, (usize, usize)> = HashMap::new();
    for test in &tests {
        let dir = extract_spec_dir(&test.spec);
        let entry = by_spec.entry(dir).or_insert((0, 0));
        if test.passed {
            entry.0 += 1;
        } else {
            entry.1 += 1;
        }
    }

    if by_spec.len() > 1 {
        result.push_str("\nTests by spec:\n");
        let mut specs: Vec<_> = by_spec.iter().collect();
        specs.sort_by(|a, b| (b.1 .0 + b.1 .1).cmp(&(a.1 .0 + a.1 .1)));

        for (dir, (pass, fail)) in specs.iter().take(5) {
            let total = pass + fail;
            let pass_rate = if total > 0 {
                (*pass as f64 / total as f64) * 100.0
            } else {
                0.0
            };
            result.push_str(&format!(
                "  {}* ({} tests, {:.0}% pass)\n",
                dir, total, pass_rate
            ));
        }
    }

    result.trim().to_string()
}

/// Extract duration from line (e.g., "(45.2s)" or "(1.2m)")
fn extract_duration(line: &str) -> Option<String> {
    lazy_static::lazy_static! {
        static ref DURATION_RE: Regex = Regex::new(r"\((\d+(?:\.\d+)?[smh])\)").unwrap();
    }

    DURATION_RE
        .captures(line)
        .map(|caps| caps[1].to_string())
}

/// Extract spec directory from full spec path
fn extract_spec_dir(spec: &str) -> String {
    if let Some(slash_pos) = spec.rfind('/') {
        spec[..slash_pos].to_string()
    } else {
        "root".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_all_passed() {
        let output = r#"
Running 3 tests using 1 worker

  ✓ [chromium] › auth/login.spec.ts:5:1 › should login (2.3s)
  ✓ [chromium] › auth/logout.spec.ts:8:1 › should logout (1.8s)
  ✓ [chromium] › dashboard.spec.ts:10:1 › should show dashboard (3.2s)

  3 passed (7.3s)
        "#;
        let result = filter_playwright_output(output);
        assert!(result.contains("✓ Playwright"));
        assert!(result.contains("3 passed, 0 failed"));
        assert!(result.contains("All tests passed"));
    }

    #[test]
    fn test_filter_with_failures() {
        let output = r#"
Running 5 tests using 2 workers

  ✓ [chromium] › auth/login.spec.ts:5:1 › should login (2.3s)
  × [chromium] › auth/logout.spec.ts:8:1 › should logout (1.8s)
  ✓ [chromium] › dashboard.spec.ts:10:1 › should show dashboard (3.2s)
  × [chromium] › profile.spec.ts:12:1 › should update profile (2.1s)
  ✓ [chromium] › settings.spec.ts:15:1 › should save settings (1.5s)

  3 passed, 2 failed (10.9s)
        "#;
        let result = filter_playwright_output(output);
        assert!(result.contains("3 passed, 2 failed"));
        assert!(result.contains("❌ 2 test(s) failed"));
        assert!(result.contains("logout.spec.ts"));
        assert!(result.contains("profile.spec.ts"));
    }

    #[test]
    fn test_extract_duration() {
        assert_eq!(extract_duration("3 passed (7.3s)"), Some("7.3s".to_string()));
        assert_eq!(
            extract_duration("10 passed (1.2m)"),
            Some("1.2m".to_string())
        );
        assert_eq!(extract_duration("no duration here"), None);
    }

    #[test]
    fn test_extract_spec_dir() {
        assert_eq!(extract_spec_dir("auth/login.spec.ts"), "auth");
        assert_eq!(
            extract_spec_dir("features/dashboard/home.spec.ts"),
            "features/dashboard"
        );
        assert_eq!(extract_spec_dir("simple.spec.ts"), "root");
    }
}
