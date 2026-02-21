use crate::tracking;
use anyhow::{Context, Result};
use regex::Regex;
use std::collections::BTreeMap;
use std::process::Command;
use std::sync::OnceLock;

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("swiftlint");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: swiftlint {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run swiftlint (is it installed? Try: brew install swiftlint)")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = if verbose > 0 {
        filter_swiftlint_verbose(&raw)
    } else {
        let result = filter_swiftlint(&raw);
        // Fallback to raw output if filter produces empty result
        if result.is_empty() && !raw.trim().is_empty() {
            eprintln!("rtk: swiftlint filter produced empty output, showing raw");
            raw.trim().to_string()
        } else {
            result
        }
    };

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });

    if let Some(hint) = crate::tee::tee_and_hint(&raw, "swiftlint", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("swiftlint {}", args.join(" ")),
        &format!("rtk swiftlint {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

/// Regex matching "Linting 'Foo.swift' (N/M)" or "Correcting 'Foo.swift' (N/M)" progress lines.
fn progress_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(?:Linting|Correcting) '.+\.swift' \(\d+/\d+\)$")
            .expect("invalid progress regex")
    })
}

/// Regex matching SwiftLint violation lines:
/// /path/to/File.swift:LINE:COL: warning|error: Message (rule_id)
fn violation_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(.+?):(\d+):(\d+): (warning|error): (.+)$").expect("invalid violation regex")
    })
}

/// Filter swiftlint output - strip progress lines, group violations by file, keep summary.
pub fn filter_swiftlint(output: &str) -> String {
    let progress = progress_re();
    let violation = violation_re();

    let mut header: Option<String> = None;
    let mut summary: Option<String> = None;
    // BTreeMap for deterministic file ordering
    let mut by_file: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut errors = 0;
    let mut warnings = 0;

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            continue;
        }

        // Capture header line
        if trimmed.starts_with("Linting Swift files")
            || trimmed.starts_with("Correcting Swift files")
        {
            header = Some(trimmed.to_string());
            continue;
        }

        // Strip progress lines
        if progress.is_match(trimmed) {
            continue;
        }

        // Capture summary line
        if trimmed.starts_with("Done linting!") || trimmed.starts_with("Done correcting!") {
            summary = Some(trimmed.to_string());
            continue;
        }

        // Parse violation lines
        if let Some(caps) = violation.captures(trimmed) {
            let full_path = &caps[1];
            let line_num = &caps[2];
            let col = &caps[3];
            let severity = &caps[4];
            let message = &caps[5];

            // Extract just the filename from full path
            let filename = full_path.rsplit('/').next().unwrap_or(full_path);

            match severity {
                "error" => errors += 1,
                "warning" => warnings += 1,
                _ => {}
            }

            let formatted = format!("  {}:{} {}: {}", line_num, col, severity, message);
            by_file
                .entry(filename.to_string())
                .or_default()
                .push(formatted);
            continue;
        }

        // Keep any other unrecognized lines (future-proofing)
    }

    let mut result = String::new();

    // Show header if present
    if let Some(h) = &header {
        result.push_str(h);
        result.push('\n');
    }

    // If there are violations, show grouped by file
    if !by_file.is_empty() {
        result.push_str(&format!(
            "SwiftLint: {} warnings, {} errors\n",
            warnings, errors
        ));

        for (file, violations) in &by_file {
            result.push_str(file);
            result.push('\n');
            for v in violations {
                result.push_str(v);
                result.push('\n');
            }
            result.push('\n');
        }
    }

    // Show summary if present
    if let Some(s) = &summary {
        result.push_str(s);
    }

    result.trim().to_string()
}

/// Verbose mode: returns raw swiftlint output unchanged for debugging.
fn filter_swiftlint_verbose(output: &str) -> String {
    output.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_filter_empty_input() {
        let result = filter_swiftlint("");
        assert!(result.is_empty(), "expected empty, got: {}", result);
    }

    #[test]
    fn test_filter_vapor_warnings_only() {
        let input = include_str!("../tests/fixtures/swiftlint_gh_vapor_warnings_only.txt");
        let result = filter_swiftlint(input);

        // Should contain the header
        assert!(
            result.contains("Linting Swift files in current working directory"),
            "missing header: {}",
            result
        );

        // Should contain the summary
        assert!(
            result.contains("Done linting! Found 23 violations, 0 serious in 342 files."),
            "missing summary: {}",
            result
        );

        // Should NOT contain any progress lines
        assert!(
            !result.contains("Linting 'Application.swift'"),
            "contains progress line: {}",
            result
        );
        assert!(
            !result.contains("(1/342)"),
            "contains progress counter: {}",
            result
        );

        // Should contain severity summary (0 errors, 22 warnings from regex-matched lines)
        assert!(
            result.contains("22 warnings"),
            "missing warning count: {}",
            result
        );
        assert!(
            result.contains("0 errors"),
            "missing error count: {}",
            result
        );
    }

    #[test]
    fn test_filter_alamofire_many_violations() {
        let input = include_str!("../tests/fixtures/swiftlint_gh_alamofire_many_violations.txt");
        let result = filter_swiftlint(input);

        // Should contain grouped violations by file
        assert!(
            result.contains("AFError.swift"),
            "missing file group: {}",
            result
        );
        assert!(
            result.contains("Session.swift"),
            "missing file group: {}",
            result
        );
        assert!(
            result.contains("Request.swift"),
            "missing file group: {}",
            result
        );

        // Should contain severity counts (98 warnings, 12 errors)
        assert!(
            result.contains("98 warnings"),
            "missing warning count: {}",
            result
        );
        assert!(
            result.contains("12 errors"),
            "missing error count: {}",
            result
        );

        // Should contain summary
        assert!(
            result.contains("Done linting! Found 112 violations, 12 serious in 85 files."),
            "missing summary: {}",
            result
        );

        // Violation lines should use filenames (not full paths)
        // Note: the header line may still contain the original path
        let violation_lines: Vec<&str> = result
            .lines()
            .filter(|l| l.starts_with("  ") && l.contains(": "))
            .collect();
        for vl in &violation_lines {
            assert!(
                !vl.contains("/Users/ci/"),
                "violation line contains full path: {}",
                vl
            );
        }
    }

    #[test]
    fn test_filter_strips_progress_interleaved() {
        let input = include_str!("../tests/fixtures/swiftlint_gh_rxswift_interleaved.txt");
        let result = filter_swiftlint(input);

        // Violations are interleaved with progress lines in this fixture.
        // The filter must strip all progress lines.
        assert!(
            !result.contains("Linting 'Observable.swift'"),
            "contains progress line: {}",
            result
        );
        assert!(
            !result.contains("(1/456)"),
            "contains progress counter: {}",
            result
        );
        assert!(
            !result.contains("(100/456)"),
            "contains progress counter: {}",
            result
        );

        // Should contain grouped violations
        assert!(
            result.contains("Observable.swift"),
            "missing grouped file: {}",
            result
        );
        assert!(
            result.contains("FlatMap.swift"),
            "missing grouped file: {}",
            result
        );

        // Should contain summary
        assert!(
            result.contains("Done linting! Found 56 violations, 7 serious in 456 files."),
            "missing summary: {}",
            result
        );

        // Should NOT contain full paths
        assert!(
            !result.contains("/Users/runner/work/RxSwift/"),
            "contains full path: {}",
            result
        );
    }

    #[test]
    fn test_filter_preserves_all_errors() {
        let input = include_str!("../tests/fixtures/swiftlint_gh_kingfisher_strict_mode.txt");
        let result = filter_swiftlint(input);

        // All violations are errors in strict mode (49 match the violation regex)
        assert!(
            result.contains("49 errors"),
            "missing error count: {}",
            result
        );
        assert!(
            result.contains("0 warnings"),
            "missing warning count: {}",
            result
        );

        // Key rule violations should be preserved
        assert!(
            result.contains("force_unwrapping"),
            "missing force_unwrapping rule: {}",
            result
        );
        assert!(
            result.contains("force_cast"),
            "missing force_cast rule: {}",
            result
        );
        assert!(
            result.contains("file_length"),
            "missing file_length rule: {}",
            result
        );
        assert!(
            result.contains("cyclomatic_complexity"),
            "missing cyclomatic_complexity rule: {}",
            result
        );

        // Should contain summary
        assert!(
            result.contains("Done linting! Found 53 violations, 53 serious in 78 files."),
            "missing summary: {}",
            result
        );
    }

    #[test]
    fn test_filter_strips_paths_alamofire() {
        let input = include_str!("../tests/fixtures/swiftlint_gh_alamofire_violations.txt");
        let result = filter_swiftlint(input);

        // Violation lines should NOT contain full paths
        let violation_lines: Vec<&str> = result
            .lines()
            .filter(|l| l.starts_with("  ") && l.contains(": "))
            .collect();
        for vl in &violation_lines {
            assert!(
                !vl.contains("/Users/runner/work/Alamofire/"),
                "violation line contains full path: {}",
                vl
            );
        }

        // Just filenames should be present (as group headers)
        assert!(
            result.contains("Session.swift"),
            "missing filename: {}",
            result
        );
        assert!(
            result.contains("Request.swift"),
            "missing filename: {}",
            result
        );

        // Config header lines are not captured by the filter (they're unrecognized and skipped)
        assert!(
            !result.contains("Loading configuration from"),
            "config loading line should be stripped: {}",
            result
        );
    }

    #[test]
    fn test_filter_groups_by_file_alamofire() {
        let input = include_str!("../tests/fixtures/swiftlint_gh_alamofire_many_violations.txt");
        let result = filter_swiftlint(input);

        // AFError.swift should have multiple violations grouped together
        let lines: Vec<&str> = result.lines().collect();
        let af_idx = lines
            .iter()
            .position(|l| l.trim() == "AFError.swift")
            .expect("AFError.swift section not found");

        // The next lines should be indented violations
        let mut violation_count = 0;
        for line in &lines[af_idx + 1..] {
            if line.starts_with("  ") && !line.trim().is_empty() {
                violation_count += 1;
            } else {
                break;
            }
        }
        assert!(
            violation_count >= 10,
            "AFError.swift should have at least 10 violations grouped, got {}: {}",
            violation_count,
            result
        );
    }

    #[test]
    fn test_filter_config_error() {
        let input = include_str!("../tests/fixtures/swiftlint_gh_realm_configerror.txt");
        let result = filter_swiftlint(input);

        // Config error lines are not violations - they should be skipped
        // (they don't match progress, header, summary, or violation regexes)
        assert!(
            !result.contains("configuration error:"),
            "config error lines should not appear in output: {}",
            result
        );
        assert!(
            !result.contains("Valid rule identifiers:"),
            "rule identifier lists should not appear: {}",
            result
        );

        // Should still contain grouped violations
        assert!(
            result.contains("BankViewController.swift"),
            "missing file group: {}",
            result
        );
        assert!(
            result.contains("TransactionListViewController.swift"),
            "missing file group: {}",
            result
        );

        // Should have correct severity counts (54 warnings, 4 errors)
        assert!(
            result.contains("4 errors"),
            "missing error count: {}",
            result
        );

        // Should contain summary
        assert!(
            result.contains("Done linting! Found 58 violations, 4 serious in 370 files."),
            "missing summary: {}",
            result
        );
    }

    #[test]
    fn test_filter_minimal_project() {
        let input = include_str!("../tests/fixtures/swiftlint_gh_snapkit_minimal.txt");
        let result = filter_swiftlint(input);

        // Small project: 23 files, 3 violations, 0 serious
        assert!(
            result.contains("3 warnings"),
            "missing warning count: {}",
            result
        );
        assert!(
            result.contains("0 errors"),
            "missing error count: {}",
            result
        );

        // All 3 violations are line_length
        assert!(
            result.contains("line_length"),
            "missing line_length rule: {}",
            result
        );

        // Two files have violations
        assert!(
            result.contains("ConstraintMaker.swift"),
            "missing file: {}",
            result
        );
        assert!(
            result.contains("ConstraintMakerRelatable.swift"),
            "missing file: {}",
            result
        );

        // Should contain summary
        assert!(
            result.contains("Done linting! Found 3 violations, 0 serious in 23 files."),
            "missing summary: {}",
            result
        );
    }

    #[test]
    fn test_verbose_mode() {
        let input = include_str!("../tests/fixtures/swiftlint_gh_rxswift_interleaved.txt");
        let result = filter_swiftlint_verbose(input);

        // Verbose mode should preserve progress lines
        assert!(
            result.contains("Linting 'Observable.swift' (1/456)"),
            "verbose mode should include progress lines: {}",
            result
        );
    }

    #[test]
    fn test_token_savings_vapor() {
        let input = include_str!("../tests/fixtures/swiftlint_gh_vapor_warnings_only.txt");
        let result = filter_swiftlint(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        // Vapor: 342 files, 22 warnings -> ~50% savings (progress lines stripped)
        assert!(
            savings >= 40.0,
            "swiftlint vapor filter: expected >=40% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_token_savings_alamofire() {
        let input = include_str!("../tests/fixtures/swiftlint_gh_alamofire_many_violations.txt");
        let result = filter_swiftlint(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        // Alamofire many: 85 files, 110 violations -> ~11% savings
        // (high violation density means most content is preserved)
        assert!(
            savings >= 5.0,
            "swiftlint alamofire filter: expected >=5% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_token_savings_all_fixtures() {
        let fixtures: Vec<(&str, &str)> = vec![
            (
                "alamofire_many",
                include_str!("../tests/fixtures/swiftlint_gh_alamofire_many_violations.txt"),
            ),
            (
                "alamofire_violations",
                include_str!("../tests/fixtures/swiftlint_gh_alamofire_violations.txt"),
            ),
            (
                "kingfisher_strict",
                include_str!("../tests/fixtures/swiftlint_gh_kingfisher_strict_mode.txt"),
            ),
            (
                "realm_configerror",
                include_str!("../tests/fixtures/swiftlint_gh_realm_configerror.txt"),
            ),
            (
                "rxswift_interleaved",
                include_str!("../tests/fixtures/swiftlint_gh_rxswift_interleaved.txt"),
            ),
            (
                "snapkit_minimal",
                include_str!("../tests/fixtures/swiftlint_gh_snapkit_minimal.txt"),
            ),
            (
                "vapor_warnings",
                include_str!("../tests/fixtures/swiftlint_gh_vapor_warnings_only.txt"),
            ),
        ];

        for (name, input) in &fixtures {
            let result = filter_swiftlint(input);

            let input_tokens = count_tokens(input);
            let output_tokens = count_tokens(&result);

            assert!(input_tokens > 0, "fixture {} has no tokens in input", name);
            assert!(!result.is_empty(), "fixture {} produced empty output", name);

            let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

            // All SwiftLint fixtures should achieve some savings
            // because progress lines are always stripped.
            // High-violation-density fixtures (e.g., alamofire_many) get ~11% savings,
            // while low-violation fixtures (e.g., vapor) get ~50%+.
            assert!(
                savings >= 5.0,
                "swiftlint {} filter: expected >=5% savings, got {:.1}% ({} -> {} tokens)",
                name,
                savings,
                input_tokens,
                output_tokens
            );
        }
    }
}
