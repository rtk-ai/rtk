use crate::tracking;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::HashSet;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XcodebuildMode {
    BuildLike,
    Test,
    Info,
    Fallback,
}

#[derive(Debug)]
struct CommandRun {
    raw: String,
    filtered: String,
    exit_code: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Default)]
struct ScanResult {
    result: Option<String>,
    targets: Vec<String>,
    seen_targets: HashSet<String>,
    compiled_files: Vec<String>,
    seen_compiled_files: HashSet<String>,
    resolved_packages: Vec<String>,
    seen_packages: HashSet<String>,
    fetched_packages: usize,
    warnings: Vec<String>,
    seen_warnings: HashSet<String>,
    errors: Vec<String>,
    seen_errors: HashSet<String>,
    context_lines: Vec<String>,
    seen_context: HashSet<String>,
    signing_lines: Vec<String>,
    seen_signing: HashSet<String>,
    failed_commands: Vec<String>,
    seen_failed_commands: HashSet<String>,
    failure_count: Option<String>,
}

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let outcome = run_with_program("xcodebuild", args, verbose)?;

    if let Some(hint) = crate::tee::tee_and_hint(&outcome.raw, "xcodebuild", outcome.exit_code) {
        println!("{}\n{}", outcome.filtered, hint);
    } else {
        println!("{}", outcome.filtered);
    }

    timer.track(
        &format!("xcodebuild {}", args.join(" ")),
        &format!("rtk xcodebuild {}", args.join(" ")),
        &outcome.raw,
        &outcome.filtered,
    );

    if outcome.exit_code != 0 {
        std::process::exit(outcome.exit_code);
    }

    Ok(())
}

fn run_with_program(program: &str, args: &[String], verbose: u8) -> Result<CommandRun> {
    let mode = detect_mode(args);

    let mut cmd = Command::new(program);
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: {} {}", program, args.join(" "));
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run {} (is Xcode installed?)", program))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);
    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });

    let filtered = if verbose > 0 {
        raw.trim().to_string()
    } else {
        let filtered = filter_xcodebuild(&raw, mode);
        if should_fallback_to_raw(&raw, &filtered, mode) {
            raw.trim().to_string()
        } else {
            filtered
        }
    };

    Ok(CommandRun {
        raw,
        filtered,
        exit_code,
    })
}

fn detect_mode(args: &[String]) -> XcodebuildMode {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-list" | "-showBuildSettings"))
    {
        return XcodebuildMode::Info;
    }

    if args
        .iter()
        .any(|arg| arg == "test" || arg == "test-without-building")
    {
        return XcodebuildMode::Test;
    }

    if args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "build"
                | "archive"
                | "clean"
                | "build-for-testing"
                | "-exportArchive"
                | "exportArchive"
        )
    }) {
        return XcodebuildMode::BuildLike;
    }

    XcodebuildMode::Fallback
}

fn filter_xcodebuild(output: &str, mode: XcodebuildMode) -> String {
    match mode {
        XcodebuildMode::Info | XcodebuildMode::Fallback => clean_raw_output(output),
        XcodebuildMode::BuildLike => filter_build_like_output(output),
        XcodebuildMode::Test => filter_test_output(output),
    }
}

fn clean_raw_output(output: &str) -> String {
    let mut cleaned = Vec::new();
    let mut last_blank = false;

    for line in output.lines() {
        let trimmed_end = line.trim_end();
        if trimmed_end.is_empty() {
            if !last_blank {
                cleaned.push(String::new());
                last_blank = true;
            }
        } else {
            cleaned.push(trimmed_end.to_string());
            last_blank = false;
        }
    }

    cleaned.join("\n").trim().to_string()
}

fn filter_build_like_output(output: &str) -> String {
    let scan = scan_output(output, XcodebuildMode::BuildLike);
    let mut lines = Vec::new();

    lines.push(format!(
        "xcodebuild: {}",
        scan.result
            .clone()
            .unwrap_or_else(|| "completed".to_string())
    ));

    if !scan.targets.is_empty() {
        lines.push(format!("Targets: {}", scan.targets.join(", ")));
    }

    let package_summary = format_package_summary(&scan);
    if !package_summary.is_empty() {
        lines.push(package_summary);
    }

    if !scan.compiled_files.is_empty() {
        lines.push(format!(
            "Swift files: {}",
            format_limited_list(&scan.compiled_files, 6)
        ));
    }

    append_group(&mut lines, "Errors", &scan.errors);
    append_group(&mut lines, "Warnings", &scan.warnings);
    append_group(&mut lines, "Context", &scan.context_lines);

    if !scan.failed_commands.is_empty() {
        lines.push("Failed commands:".to_string());
        for command in &scan.failed_commands {
            lines.push(format!("  {}", command));
        }
    }

    if let Some(failure_count) = scan.failure_count {
        lines.push(failure_count);
    }

    append_group(&mut lines, "Signing/Export", &scan.signing_lines);

    lines.join("\n").trim().to_string()
}

fn filter_test_output(output: &str) -> String {
    let scan = scan_output(output, XcodebuildMode::Test);
    let mut lines = Vec::new();

    lines.push(format!(
        "xcodebuild: {}",
        scan.result
            .clone()
            .unwrap_or_else(|| "completed".to_string())
    ));

    if !scan.targets.is_empty() {
        lines.push(format!("Targets: {}", scan.targets.join(", ")));
    }

    let package_summary = format_package_summary(&scan);
    if !package_summary.is_empty() {
        lines.push(package_summary);
    }

    let mut kept_lines = Vec::new();
    let mut last_blank = false;
    for line in scan.context_lines {
        if line.is_empty() {
            if !last_blank {
                kept_lines.push(line);
                last_blank = true;
            }
        } else {
            kept_lines.push(line);
            last_blank = false;
        }
    }

    if !kept_lines.is_empty() {
        lines.push(String::new());
        lines.extend(kept_lines);
    }

    if !scan.failed_commands.is_empty() {
        lines.push(String::new());
        lines.push("Failed commands:".to_string());
        for command in &scan.failed_commands {
            lines.push(format!("  {}", command));
        }
    }

    if let Some(failure_count) = scan.failure_count {
        lines.push(failure_count);
    }

    lines.join("\n").trim().to_string()
}

fn scan_output(output: &str, mode: XcodebuildMode) -> ScanResult {
    let mut scan = ScanResult::default();
    let mut in_resolved_packages = false;
    let mut in_failed_commands = false;
    let mut last_blank = false;

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed == "Resolved source packages:" {
            in_resolved_packages = true;
            continue;
        }

        if in_resolved_packages {
            if let Some(package) = parse_resolved_package(line) {
                push_unique(
                    &mut scan.resolved_packages,
                    &mut scan.seen_packages,
                    package,
                );
                continue;
            }

            if trimmed.is_empty() {
                continue;
            }

            in_resolved_packages = false;
        }

        if trimmed == "The following build commands failed:" {
            in_failed_commands = true;
            continue;
        }

        if in_failed_commands {
            if trimmed.starts_with('(')
                && (trimmed.ends_with("failure)") || trimmed.ends_with("failures)"))
            {
                scan.failure_count = Some(trimmed.to_string());
                in_failed_commands = false;
                continue;
            }

            if trimmed.is_empty() {
                continue;
            }

            push_unique(
                &mut scan.failed_commands,
                &mut scan.seen_failed_commands,
                compact_failed_command(trimmed),
            );
            continue;
        }

        if let Some(result) = parse_result(trimmed) {
            scan.result = Some(result);
            continue;
        }

        if let Some(target) = parse_target(trimmed) {
            push_unique(&mut scan.targets, &mut scan.seen_targets, target);
            continue;
        }

        if let Some(file) = parse_compiled_file(trimmed) {
            push_unique(
                &mut scan.compiled_files,
                &mut scan.seen_compiled_files,
                file,
            );
            continue;
        }

        if trimmed.starts_with("Fetching from ") {
            scan.fetched_packages += 1;
            continue;
        }

        if let Some(signing) = parse_signing_or_export(trimmed) {
            push_unique(&mut scan.signing_lines, &mut scan.seen_signing, signing);
            continue;
        }

        if mode == XcodebuildMode::BuildLike {
            if let Some((severity, diagnostic)) = parse_compiler_diagnostic(trimmed) {
                match severity {
                    DiagnosticSeverity::Error => {
                        push_unique(&mut scan.errors, &mut scan.seen_errors, diagnostic)
                    }
                    DiagnosticSeverity::Warning => {
                        push_unique(&mut scan.warnings, &mut scan.seen_warnings, diagnostic)
                    }
                }
                continue;
            }

            if let Some(context) = parse_high_value_context(trimmed) {
                push_unique(&mut scan.context_lines, &mut scan.seen_context, context);
                continue;
            }

            continue;
        }

        if is_test_noise(trimmed) || is_build_noise(trimmed) {
            continue;
        }

        if let Some((severity, diagnostic)) = parse_compiler_diagnostic(trimmed) {
            let prefixed = match severity {
                DiagnosticSeverity::Error => format!("error: {}", diagnostic),
                DiagnosticSeverity::Warning => format!("warning: {}", diagnostic),
            };
            push_unique(&mut scan.context_lines, &mut scan.seen_context, prefixed);
            continue;
        }

        if trimmed.is_empty() {
            if !last_blank {
                scan.context_lines.push(String::new());
            }
            last_blank = true;
            continue;
        }

        last_blank = false;
        push_unique(
            &mut scan.context_lines,
            &mut scan.seen_context,
            line.trim_end().to_string(),
        );
    }

    scan
}

fn parse_result(line: &str) -> Option<String> {
    RESULT_RE
        .captures(line)
        .map(|caps| caps[1].trim().replace("  ", " "))
}

fn parse_target(line: &str) -> Option<String> {
    TARGET_RE
        .captures(line)
        .map(|caps| format!("{} ({})", &caps[1], &caps[2]))
}

fn parse_compiled_file(line: &str) -> Option<String> {
    if let Some(caps) = COMPILE_NEW_RE.captures(line) {
        return Some(caps[1].replace('\\', ""));
    }

    COMPILE_OLD_RE.captures(line).map(|caps| basename(&caps[1]))
}

fn parse_compiler_diagnostic(line: &str) -> Option<(DiagnosticSeverity, String)> {
    if let Some(caps) = DIAGNOSTIC_RE.captures(line) {
        let severity = match &caps[4] {
            "error" => DiagnosticSeverity::Error,
            "warning" => DiagnosticSeverity::Warning,
            _ => return None,
        };
        return Some((severity, line.to_string()));
    }

    if let Some(rest) = line.strip_prefix("error: ") {
        return Some((DiagnosticSeverity::Error, format!("error: {}", rest)));
    }

    if let Some(rest) = line.strip_prefix("warning: ") {
        return Some((DiagnosticSeverity::Warning, format!("warning: {}", rest)));
    }

    None
}

fn parse_high_value_context(line: &str) -> Option<String> {
    if line.starts_with("Command ") && line.contains(" failed")
        || line.contains("Code Signing Error")
        || line.contains("Provisioning profile")
        || line.contains("No signing certificate")
        || line.contains("requires a provisioning profile")
        || line.starts_with("Testing failed:")
        || line.starts_with("xcodebuild: error:")
        || line.starts_with("error: exportArchive")
    {
        return Some(line.to_string());
    }

    None
}

fn parse_signing_or_export(line: &str) -> Option<String> {
    if let Some(caps) = SIGNING_RE.captures(line) {
        return Some(format!("Signing identity: {}", &caps[1]));
    }

    if line.starts_with("Exported ")
        || line.starts_with("Successfully exported ")
        || line.starts_with("exportArchive")
        || line.contains("IDEDistribution")
    {
        return Some(line.to_string());
    }

    None
}

fn parse_resolved_package(line: &str) -> Option<String> {
    PACKAGE_RE.captures(line).map(|caps| {
        let name = caps[1].trim();
        let version = caps[2].trim();
        format!("{name} @ {version}")
    })
}

fn compact_failed_command(line: &str) -> String {
    if let Some(caps) = FAILED_FILE_RE.captures(line) {
        let step = line.split_whitespace().next().unwrap_or("Command");
        return format!("{step} {}", basename(&caps[1]));
    }

    line.to_string()
}

fn is_build_noise(line: &str) -> bool {
    if line.is_empty() {
        return false;
    }

    BUILD_NOISE_PREFIXES
        .iter()
        .any(|prefix| line.starts_with(prefix))
        || line.starts_with("builtin-")
        || line.starts_with("cd ")
        || line.starts_with("/usr/bin/")
        || line.starts_with("/Applications/Xcode")
        || line.starts_with("note: Building targets in dependency order")
        || line.starts_with("note: Target dependency graph")
        || line.starts_with("Prepare packages")
        || line.starts_with("ComputePackagePrebuildTargetDependencyGraph")
        || line.starts_with("Build description signature:")
        || line.starts_with("Build description path:")
}

fn is_test_noise(line: &str) -> bool {
    is_build_noise(line)
        || TEST_NOISE_PATTERNS
            .iter()
            .any(|pattern| pattern.is_match(line))
        || line.starts_with("Testing started")
        || line.starts_with("Test Suite 'All tests' started")
        || line.starts_with("Test Suite 'Selected tests' started")
        || line.starts_with("t =")
}

fn format_package_summary(scan: &ScanResult) -> String {
    if scan.resolved_packages.is_empty() && scan.fetched_packages == 0 {
        return String::new();
    }

    let mut parts = Vec::new();
    if !scan.resolved_packages.is_empty() {
        parts.push(format!(
            "resolved {}",
            format_limited_list(&scan.resolved_packages, 5)
        ));
    }
    if scan.fetched_packages > 0 {
        parts.push(format!("fetched {}", scan.fetched_packages));
    }

    format!("Packages: {}", parts.join("; "))
}

fn format_limited_list(items: &[String], limit: usize) -> String {
    if items.len() <= limit {
        return items.join(", ");
    }

    let mut shown = items[..limit].to_vec();
    shown.push(format!("+{} more", items.len() - limit));
    shown.join(", ")
}

fn append_group(lines: &mut Vec<String>, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }

    lines.push(format!("{title}:"));
    for item in items {
        lines.push(format!("  {item}"));
    }
}

fn should_fallback_to_raw(raw: &str, filtered: &str, mode: XcodebuildMode) -> bool {
    if filtered.trim().is_empty() {
        return !raw.trim().is_empty();
    }

    if matches!(mode, XcodebuildMode::Info | XcodebuildMode::Fallback) {
        return false;
    }

    let raw_nonempty = raw.lines().filter(|line| !line.trim().is_empty()).count();
    let filtered_nonempty = filtered
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    if raw_nonempty >= 20 && filtered_nonempty <= 2 {
        return true;
    }

    let raw_lower = raw.to_ascii_lowercase();
    let filtered_lower = filtered.to_ascii_lowercase();
    let raw_has_signal = raw_lower.contains("error:")
        || raw_lower.contains("warning:")
        || raw_lower.contains("failed");

    raw_has_signal
        && !filtered_lower.contains("error")
        && !filtered_lower.contains("warning")
        && !filtered_lower.contains("failed")
}

fn push_unique(target: &mut Vec<String>, seen: &mut HashSet<String>, value: String) {
    if seen.insert(value.clone()) {
        target.push(value);
    }
}

fn basename(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

lazy_static! {
    static ref TARGET_RE: Regex = Regex::new(r"Target '([^']+)' in project '([^']+)'").unwrap();
    static ref COMPILE_NEW_RE: Regex =
        Regex::new(r"^SwiftCompile\s+\S+\s+\S+\s+Compiling\\?\s+(\S+\.swift)").unwrap();
    static ref COMPILE_OLD_RE: Regex =
        Regex::new(r"^CompileSwift\s+\S+\s+\S+\s+(\S+\.swift)").unwrap();
    static ref DIAGNOSTIC_RE: Regex =
        Regex::new(r"^(.+?):(\d+):(\d+):\s+(error|warning):\s+(.+)$").unwrap();
    static ref SIGNING_RE: Regex = Regex::new(r#"Signing Identity:\s+"([^"]+)""#).unwrap();
    static ref PACKAGE_RE: Regex =
        Regex::new(r"^\s{2,}([^:]+):.*?([0-9][A-Za-z0-9.\-+]*)\s*$").unwrap();
    static ref FAILED_FILE_RE: Regex = Regex::new(r"(\S+\.swift)").unwrap();
    static ref RESULT_RE: Regex =
        Regex::new(r"^\*\*\s+([A-Z ]+(?:SUCCEEDED|FAILED))\s+\*\*$").unwrap();
    static ref TEST_NOISE_PATTERNS: Vec<Regex> = vec![
        Regex::new(r"^Test Case '.+' started\.$").unwrap(),
        Regex::new(r"^Test Case '.+' passed \(.+\)$").unwrap(),
        Regex::new(r"^Test Suite '.+' started at .+$").unwrap(),
    ];
}

const BUILD_NOISE_PREFIXES: &[&str] = &[
    "WriteAuxiliaryFile ",
    "WriteFile ",
    "MkDir ",
    "Touch ",
    "Ld ",
    "Libtool ",
    "CompileC ",
    "CompileAssetCatalog ",
    "GenerateAssetSymbols ",
    "CpResource ",
    "CpHeader ",
    "Copy ",
    "SwiftMergeGeneratedHeaders ",
    "SwiftDriver ",
    "SwiftDriverJobDiscovery ",
    "SwiftEmitModule ",
    "ProcessInfoPlistFile ",
    "ProcessProductPackaging ",
    "ProcessProductPackagingDER ",
    "ClangStatCache ",
    "GenerateDSYMFile ",
    "RegisterExecutionPolicyException ",
    "Validate ",
    "CodeSign ",
    "ExtractAppIntentsMetadata ",
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    fn fixture(name: &str) -> &'static str {
        match name {
            "build_success" => {
                include_str!("../tests/fixtures/xcodebuild/build_success.log")
            }
            "build_failure" => {
                include_str!("../tests/fixtures/xcodebuild/build_failure.log")
            }
            "test_failure" => {
                include_str!("../tests/fixtures/xcodebuild/test_failure.log")
            }
            "test_success" => {
                include_str!("../tests/fixtures/xcodebuild/test_success.log")
            }
            "list" => include_str!("../tests/fixtures/xcodebuild/list.log"),
            "build_settings" => {
                include_str!("../tests/fixtures/xcodebuild/show_build_settings.log")
            }
            "export_archive" => {
                include_str!("../tests/fixtures/xcodebuild/export_archive.log")
            }
            other => panic!("unknown fixture {other}"),
        }
    }

    #[test]
    fn detects_modes_from_args() {
        assert_eq!(detect_mode(&["build".into()]), XcodebuildMode::BuildLike);
        assert_eq!(detect_mode(&["test".into()]), XcodebuildMode::Test);
        assert_eq!(
            detect_mode(&["-showBuildSettings".into()]),
            XcodebuildMode::Info
        );
        assert_eq!(
            detect_mode(&["test-without-building".into()]),
            XcodebuildMode::Test
        );
        assert_eq!(detect_mode(&["-version".into()]), XcodebuildMode::Fallback);
    }

    #[test]
    fn build_success_filter_keeps_summary_and_compiled_files() {
        let filtered = filter_xcodebuild(fixture("build_success"), XcodebuildMode::BuildLike);

        assert!(filtered.contains("BUILD SUCCEEDED"));
        assert!(filtered.contains("App (AppProject)"));
        assert!(filtered.contains("alamofire @ 5.8.1"));
        assert!(filtered.contains("AppDelegate.swift"));
        assert!(filtered.contains("LegacyFeature.swift"));
        assert!(!filtered.contains("WriteAuxiliaryFile"));
        assert!(!filtered.contains("CodeSign /Users"));
    }

    #[test]
    fn build_success_filter_verifies_token_savings() {
        let input = fixture("build_success");
        let filtered = filter_xcodebuild(input, XcodebuildMode::BuildLike);
        let savings = 100.0 - (count_tokens(&filtered) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Expected >=60% savings, got {:.1}%\nfiltered:\n{}",
            savings,
            filtered
        );
    }

    #[test]
    fn build_failure_filter_keeps_errors_warnings_and_failed_commands() {
        let filtered = filter_xcodebuild(fixture("build_failure"), XcodebuildMode::BuildLike);

        assert!(filtered.contains("BUILD FAILED"));
        assert!(filtered.contains(
            "/Users/me/App/Sources/AppDelegate.swift:11:9: warning: variable 'unused' was never used"
        ));
        assert!(filtered.contains(
            "/Users/me/App/Sources/HomeView.swift:42:18: error: cannot find 'MissingType' in scope"
        ));
        assert!(filtered.contains("SwiftCompile HomeView.swift"));
        assert!(filtered.contains("(2 failures)"));
        assert!(filtered.contains("Command PhaseScriptExecution failed with a nonzero exit code"));
        assert!(!filtered.contains("builtin-SwiftDriver"));
    }

    #[test]
    fn build_failure_filter_verifies_token_savings() {
        let input = fixture("build_failure");
        let filtered = filter_xcodebuild(input, XcodebuildMode::BuildLike);
        let savings = 100.0 - (count_tokens(&filtered) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Expected >=60% savings, got {:.1}%\nfiltered:\n{}",
            savings,
            filtered
        );
    }

    #[test]
    fn test_filter_drops_passed_test_cases_but_keeps_failure_context() {
        let filtered = filter_xcodebuild(fixture("test_failure"), XcodebuildMode::Test);

        assert!(filtered.contains("TEST FAILED"));
        assert!(filtered.contains("Test Suite 'AppTests.xctest' failed"));
        assert!(filtered.contains("testLoginFlow"));
        assert!(filtered.contains("XCTAssertEqual failed"));
        assert!(filtered.contains("Testing failed:"));
        assert!(!filtered.contains("testHappyPath' passed"));
        assert!(!filtered.contains("WriteAuxiliaryFile"));
    }

    #[test]
    fn test_success_filter_keeps_result_and_suite_summary() {
        let filtered = filter_xcodebuild(fixture("test_success"), XcodebuildMode::Test);

        assert!(filtered.contains("TEST SUCCEEDED"));
        assert!(filtered.contains("Test Suite 'AppTests.xctest' passed"));
        assert!(!filtered.contains("testHappyPath' passed"));
    }

    #[test]
    fn info_mode_preserves_list_output() {
        let filtered = filter_xcodebuild(fixture("list"), XcodebuildMode::Info);

        assert!(filtered.contains("Information about project"));
        assert!(filtered.contains("Targets:"));
        assert!(filtered.contains("Schemes:"));
    }

    #[test]
    fn info_mode_preserves_show_build_settings_output() {
        let filtered = filter_xcodebuild(fixture("build_settings"), XcodebuildMode::Info);

        assert!(filtered.contains("Build settings for action build and target App:"));
        assert!(filtered.contains("PRODUCT_BUNDLE_IDENTIFIER = com.example.app"));
        assert!(filtered.contains("SWIFT_VERSION = 5.10"));
    }

    #[test]
    fn build_like_filter_keeps_export_context() {
        let filtered = filter_xcodebuild(fixture("export_archive"), XcodebuildMode::BuildLike);

        assert!(filtered.contains("ARCHIVE SUCCEEDED"));
        assert!(filtered.contains("Signing identity: Apple Distribution"));
        assert!(filtered.contains("exportArchive"));
    }

    #[test]
    fn suspicious_filter_result_falls_back_to_raw() {
        let raw = "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\nline 9\nline 10\nline 11\nline 12\nline 13\nline 14\nline 15\nline 16\nline 17\nline 18\nline 19\nline 20\nerror: something broke";
        let filtered = "xcodebuild: completed";
        assert!(should_fallback_to_raw(
            raw,
            filtered,
            XcodebuildMode::BuildLike
        ));
    }

    #[cfg(unix)]
    #[test]
    fn run_with_program_preserves_args_and_exit_code() {
        let temp = TempDir::new().unwrap();
        let script = temp.path().join("fake-xcodebuild.sh");
        fs::write(&script, "#!/bin/sh\nprintf 'ARGS:%s\\n' \"$*\"\nexit 7\n").unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();

        let outcome = run_with_program(
            script.to_str().unwrap(),
            &["-list".into(), "-project".into(), "Demo.xcodeproj".into()],
            0,
        )
        .unwrap();

        assert_eq!(outcome.exit_code, 7);
        assert!(outcome.raw.contains("ARGS:-list -project Demo.xcodeproj"));
        assert!(outcome
            .filtered
            .contains("ARGS:-list -project Demo.xcodeproj"));
    }

    #[cfg(unix)]
    #[test]
    fn run_with_program_uses_raw_output_in_verbose_mode() {
        let temp = TempDir::new().unwrap();
        let script = temp.path().join("fake-xcodebuild.sh");
        fs::write(
            &script,
            "#!/bin/sh\nprintf 'WriteAuxiliaryFile noisy\\n** BUILD SUCCEEDED **\\n'\n",
        )
        .unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();

        let outcome = run_with_program(script.to_str().unwrap(), &["build".into()], 1).unwrap();
        assert!(outcome.filtered.contains("WriteAuxiliaryFile noisy"));
        assert!(outcome.filtered.contains("** BUILD SUCCEEDED **"));
    }

    #[cfg(unix)]
    #[test]
    fn run_with_program_combines_stdout_and_stderr() {
        let temp = TempDir::new().unwrap();
        let script = temp.path().join("fake-xcodebuild.sh");
        fs::write(
            &script,
            "#!/bin/sh\nprintf 'stdout-line\\n'\nprintf 'stderr-line\\n' >&2\n",
        )
        .unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();

        let outcome = run_with_program(script.to_str().unwrap(), &["-list".into()], 0).unwrap();
        assert!(outcome.raw.contains("stdout-line"));
        assert!(outcome.raw.contains("stderr-line"));
    }
}
