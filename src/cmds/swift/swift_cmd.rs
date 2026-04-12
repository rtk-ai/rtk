//! Filters swift build and swift test output — compact SwiftPM and XCTest / Swift Testing logs.

use crate::core::runner;
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::Result;
use regex::{Regex, RegexSet};
use std::sync::OnceLock;

const MAX_LINES: usize = 40;

pub fn run_build(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("swift");
    cmd.arg("build");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: swift build {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "swift build",
        &args.join(" "),
        filter_swift_build_output,
        runner::RunOptions::with_tee("swift_build"),
    )
}

fn build_strip_line_set() -> &'static RegexSet {
    static SET: OnceLock<RegexSet> = OnceLock::new();
    SET.get_or_init(|| {
        RegexSet::new([r"^\s*$", r"^Compiling ", r"^Linking "]).expect("swift build strip RegexSet")
    })
}

fn build_complete_short_circuit_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"Build complete!").expect("Build complete regex"))
}

fn build_short_circuit_unless_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"warning:|error:").expect("swift build unless regex"))
}

/// Parse and compact swift build output.
fn filter_swift_build_output(raw: &str) -> String {
    let mut lines: Vec<String> = raw.lines().map(|l| strip_ansi(l)).collect();

    let blob = lines.join("\n");
    let unless = build_short_circuit_unless_re();
    if build_complete_short_circuit_re().is_match(&blob) && !unless.is_match(&blob) {
        return "ok (build complete)".to_string();
    }

    let set = build_strip_line_set();
    lines.retain(|l| !set.is_match(l));

    if lines.len() > MAX_LINES {
        let truncated = lines.len() - MAX_LINES;
        lines.truncate(MAX_LINES);
        lines.push(format!("... ({} lines truncated)", truncated));
    }

    lines.join("\n")
}

pub fn run_test(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("swift");
    cmd.arg("test");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: swift test {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "swift test",
        &args.join(" "),
        filter_swift_test_output,
        runner::RunOptions::with_tee("swift_test"),
    )
}

fn strip_line_set() -> &'static RegexSet {
    static SET: OnceLock<RegexSet> = OnceLock::new();
    SET.get_or_init(|| {
        RegexSet::new([
            r"^\s*$",
            r"^Building for debugging\.\.\.$",
            r"^\[\d+/\d+\]\s",
            r"^Compiling ",
            r"^Linking ",
            r"^Test Suite '.*' started at ",
            r"^Test Case '.*' started\.$",
            r"^.*Test run started\.?$",
            r"^.*Testing Library Version:",
            r"^.*Target Platform:",
            r"^Build complete!.*$",
        ])
        .expect("swift test strip RegexSet")
    })
}

fn xctest_all_pass_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"Executed \d+ tests?, with 0 failures").expect("regex"))
}

fn swift_testing_all_pass_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"Test run with .+ passed after \d").expect("regex"))
}

fn short_circuit_unless_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(error:|\bFAILED\b|XCTAssert)").expect("unless regex")
    })
}

/// Parse and compact swift test output.
fn filter_swift_test_output(raw: &str) -> String {
    let mut lines: Vec<String> = raw.lines().map(|l| strip_ansi(l)).collect();

    let blob = lines.join("\n");
    let unless = short_circuit_unless_re();

    if xctest_all_pass_re().is_match(&blob) && !unless.is_match(&blob) {
        return "ok (all tests passed)".to_string();
    }
    if swift_testing_all_pass_re().is_match(&blob) && !unless.is_match(&blob) {
        return "ok (all tests passed)".to_string();
    }

    let set = strip_line_set();
    lines.retain(|l| !set.is_match(l));

    if lines.len() > MAX_LINES {
        let truncated = lines.len() - MAX_LINES;
        lines.truncate(MAX_LINES);
        lines.push(format!("... ({} lines truncated)", truncated));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_swift_build_clean_short_circuits() {
        assert_eq!(filter_swift_build_output("Build complete!\n"), "ok (build complete)");
    }

    #[test]
    fn test_filter_swift_build_errors_kept_after_strip() {
        let input = r#"Compiling MyApp MyApp.swift
/home/user/MyApp/Sources/MyApp/main.swift:5:1: error: use of unresolved identifier 'foo'
foo()
^~~
Linking MyApp
error: build had 1 command failure"#;
        let out = filter_swift_build_output(input);
        assert_eq!(
            out,
            "/home/user/MyApp/Sources/MyApp/main.swift:5:1: error: use of unresolved identifier 'foo'\nfoo()\n^~~\nerror: build had 1 command failure"
        );
    }

    #[test]
    fn test_filter_swift_build_warnings_not_swallowed() {
        let input = r#"CompileSwift normal x86_64 MyFile.swift
/path/to/MyFile.swift:42:10: warning: unused variable 'x'
Build complete! (with warnings)"#;
        assert_eq!(
            filter_swift_build_output(input),
            "CompileSwift normal x86_64 MyFile.swift\n/path/to/MyFile.swift:42:10: warning: unused variable 'x'\nBuild complete! (with warnings)"
        );
    }

    #[test]
    fn test_filter_swift_test_xctest_all_pass() {
        let input = r#"Compiling MyPkgTests runner.swift
Linking MyPkgTests
Test Suite 'All tests' started at 2024-01-01 12:00:00.000
Test Case '-[MyPkgTests.MyTests testExample]' started.
Test Case '-[MyPkgTests.MyTests testExample]' passed (0.001 seconds).
	 Executed 1 test, with 0 failures (0 unexpected) in 0.001 (0.001) seconds"#;
        assert_eq!(filter_swift_test_output(input), "ok (all tests passed)");
    }

    #[test]
    fn test_filter_swift_test_swift_testing_event_stream_pass() {
        let input = r#"Building for debugging...
[0/2] Write swift-version--58304C5D6DBC2.txt
Build complete! (0.14s)
Test run with 1 test in 1 suites passed after 0.001 seconds."#;
        assert_eq!(filter_swift_test_output(input), "ok (all tests passed)");
    }

    #[test]
    fn test_filter_swift_test_compile_error_kept() {
        let input = r#"Compiling MyLib foo.swift
/home/pkg/Sources/MyLib/foo.swift:5:1: error: cannot find 'x' in scope
    let _ = x
            ^
error: fatalError"#;
        let out = filter_swift_test_output(input);
        assert!(out.contains("error: cannot find"));
        assert!(out.contains("fatalError"));
        assert!(!out.starts_with("ok (all tests passed)"));
    }

    #[test]
    fn test_filter_swift_test_failure_not_short_circuited() {
        let input = r#"Test Suite 'All tests' started at 2024-01-01 12:00:00.000
Test Case '-[PkgTests.T testBad]' started.
/path/PkgTests/Tests.swift:10: error: XCTAssertEqual failed: ("1") is not equal to ("2")
Test Case '-[PkgTests.T testBad]' failed (0.010 seconds).
	 Executed 1 test, with 1 failure (0 unexpected) in 0.012 (0.012) seconds"#;
        let out = filter_swift_test_output(input);
        assert!(out.contains("XCTAssertEqual failed"));
        assert!(out.contains("with 1 failure"));
        assert!(!out.contains("ok (all tests passed)"));
    }
}
