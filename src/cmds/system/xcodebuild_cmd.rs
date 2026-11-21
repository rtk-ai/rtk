//! Compact Xcode build and test output while preserving diagnostics and summaries.

use anyhow::Result;
use std::collections::HashSet;

use crate::core::runner::{self, RunOptions};
use crate::core::shell::display_args;
use crate::core::truncate::{self, CAP_WARNINGS};
use crate::core::utils::{resolved_command, strip_ansi};

const MAX_DIAGNOSTICS: usize = CAP_WARNINGS;
const MAX_TEST_RECORDS: usize = truncate::reduced(CAP_WARNINGS, 20);

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("xcodebuild");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: xcodebuild {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "xcodebuild",
        &display_args(args),
        filter_xcodebuild_output,
        RunOptions::with_tee("xcodebuild"),
    )
}

pub(crate) fn filter_xcodebuild_output(output: &str) -> String {
    let clean = strip_ansi(output);
    let trimmed = clean.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let lines: Vec<&str> = clean.lines().collect();
    let status = lines.iter().rev().find_map(|line| {
        let trimmed = line.trim();
        is_status_line(trimmed).then_some(trimmed)
    });
    let has_test_records = lines.iter().any(|line| is_test_record(line.trim()));

    if status.is_none() && !has_test_records {
        return trimmed.to_string();
    }

    let diagnostics = collect_diagnostics(&lines);
    let test_records = collect_test_records(&lines);
    let mut rendered = Vec::new();

    rendered.extend(diagnostics.iter().take(MAX_DIAGNOSTICS).cloned());
    if diagnostics.len() > MAX_DIAGNOSTICS {
        rendered.push(format!(
            "... +{} more diagnostics",
            diagnostics.len() - MAX_DIAGNOSTICS
        ));
    }

    if !rendered.is_empty() && !test_records.is_empty() {
        rendered.push(String::new());
    }
    rendered.extend(test_records.iter().take(MAX_TEST_RECORDS).cloned());
    if test_records.len() > MAX_TEST_RECORDS {
        rendered.push(format!(
            "... +{} more test records",
            test_records.len() - MAX_TEST_RECORDS
        ));
    }

    if let Some(status) = status {
        if !rendered.is_empty() {
            rendered.push(String::new());
        }
        rendered.push(status.to_string());
    }

    let filtered = rendered.join("\n");
    if filtered.trim().is_empty() {
        trimmed.to_string()
    } else {
        filtered
    }
}

fn collect_diagnostics(lines: &[&str]) -> Vec<String> {
    let mut diagnostics = Vec::new();
    let mut seen = HashSet::new();

    for line in lines {
        let trimmed = line.trim_end();
        if is_diagnostic(trimmed) && seen.insert(trimmed.to_string()) {
            diagnostics.push(trimmed.to_string());
        }
    }

    diagnostics
}

fn collect_test_records(lines: &[&str]) -> Vec<String> {
    let mut records = Vec::new();
    let mut seen = HashSet::new();

    for line in lines {
        let trimmed = line.trim();
        if is_test_record(trimmed) && seen.insert(trimmed.to_string()) {
            records.push(trimmed.to_string());
        }
    }

    records
}

fn is_diagnostic(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.contains(" error:")
        || trimmed.contains(" warning:")
        || trimmed.starts_with("error:")
        || trimmed.starts_with("warning:")
        || trimmed.starts_with("fatal error:")
        || trimmed.starts_with("Testing failed:")
        || trimmed.starts_with("Build input file cannot be found:")
        || trimmed.starts_with("Command ") && trimmed.ends_with(" failed with a nonzero exit code")
}

fn is_test_record(line: &str) -> bool {
    line.starts_with("Test Case ")
        || line.starts_with("Test Suite ")
        || line.starts_with("Executed ")
        || line.starts_with("Failing tests:")
        || line.starts_with("Test session results")
}

fn is_status_line(line: &str) -> bool {
    matches!(
        line,
        "** BUILD SUCCEEDED **"
            | "** BUILD FAILED **"
            | "** TEST SUCCEEDED **"
            | "** TEST FAILED **"
            | "** TEST BUILD SUCCEEDED **"
            | "** TEST BUILD FAILED **"
            | "** TEST EXECUTE SUCCEEDED **"
            | "** TEST EXECUTE FAILED **"
            | "** ARCHIVE SUCCEEDED **"
            | "** ARCHIVE FAILED **"
            | "** ANALYZE SUCCEEDED **"
            | "** ANALYZE FAILED **"
            | "** CLEAN SUCCEEDED **"
            | "** CLEAN FAILED **"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compacts_successful_build_to_diagnostics_and_status() {
        let input = r#"Command line invocation:
    /Applications/Xcode.app/Contents/Developer/usr/bin/xcodebuild build

ResolvePackageGraph
Prepare packages
CreateBuildRequest
CompileSwift normal x86_64 /tmp/App.swift
CompileAssetCatalog /tmp/Assets.xcassets
2026-10-22 17:40:49.582 appintentsmetadataprocessor[69791:1574194] warning: Metadata extraction skipped. No AppIntents.framework dependency found.
Touch /tmp/App.app

** BUILD SUCCEEDED **
"#;

        assert_eq!(
            filter_xcodebuild_output(input),
            "2026-10-22 17:40:49.582 appintentsmetadataprocessor[69791:1574194] warning: Metadata extraction skipped. No AppIntents.framework dependency found.\n\n** BUILD SUCCEEDED **"
        );
    }

    #[test]
    fn preserves_selected_test_records_and_final_status() {
        let input = r#"Command line invocation:
    /Applications/Xcode.app/Contents/Developer/usr/bin/xcodebuild test-without-building

Test Suite 'Selected tests' started at 2026-10-22 17:41:34.943.
Test Suite 'SignalCrownUITests.xctest' started at 2026-10-22 17:41:34.946.
Test Suite 'SignalCrownUITests' started at 2026-10-22 17:41:34.946.
Test Case '-[SignalCrownUITests.SignalCrownUITests testJourney]' started.
Application launched
Waiting for accessibility hierarchy
Test Case '-[SignalCrownUITests.SignalCrownUITests testJourney]' passed (89.254 seconds).
Test Suite 'SignalCrownUITests' passed at 2026-10-22 17:43:04.202.
     Executed 1 test, with 0 failures (0 unexpected) in 89.254 (89.256) seconds
Test Suite 'SignalCrownUITests.xctest' passed at 2026-10-22 17:43:04.204.
Test Suite 'Selected tests' passed at 2026-10-22 17:43:04.204.

** TEST EXECUTE SUCCEEDED **
"#;

        let filtered = filter_xcodebuild_output(input);

        assert!(filtered.contains("testJourney]' passed (89.254 seconds)."));
        assert!(filtered.contains("Executed 1 test, with 0 failures"));
        assert!(filtered.ends_with("** TEST EXECUTE SUCCEEDED **"));
        assert!(!filtered.contains("Waiting for accessibility hierarchy"));
    }

    #[test]
    fn preserves_failure_diagnostics_and_test_failure() {
        let input = r#"CompileSwift normal x86_64 /tmp/CombatScene.swift
/tmp/CombatScene.swift:42:17: error: cannot find 'cameraTarget' in scope
/tmp/CombatScene.swift:50:9: warning: immutable value 'offset' was never used
Test Case '-[SignalCrownTests.CombatTests testCamera]' failed (0.123 seconds).
Testing failed:
    Cannot compile module SignalCrown

** TEST FAILED **
"#;

        let filtered = filter_xcodebuild_output(input);

        assert!(filtered.contains("cannot find 'cameraTarget' in scope"));
        assert!(filtered.contains("immutable value 'offset' was never used"));
        assert!(filtered.contains("testCamera]' failed"));
        assert!(filtered.contains("Testing failed:"));
        assert!(filtered.ends_with("** TEST FAILED **"));
    }

    #[test]
    fn unknown_output_passes_through_unchanged_apart_from_edges() {
        let input = "custom xcodebuild help text\nwith no build or test result\n";
        assert_eq!(
            filter_xcodebuild_output(input),
            "custom xcodebuild help text\nwith no build or test result"
        );
    }

    #[test]
    fn real_shape_exceeds_the_savings_floor() {
        let mut input = String::from("Command line invocation:\n");
        for index in 0..80 {
            input.push_str(&format!(
                "CompileSwift normal x86_64 /tmp/GeneratedSource{index}.swift\n"
            ));
        }
        input.push_str("** BUILD SUCCEEDED **\n");

        let filtered = filter_xcodebuild_output(&input);
        let input_words = input.split_whitespace().count();
        let output_words = filtered.split_whitespace().count();
        let savings = 100.0 - output_words as f64 / input_words as f64 * 100.0;

        assert!(savings >= 20.0, "expected >=20% savings, got {savings:.1}%");
    }
}
