//! Flutter command filters.

use anyhow::Result;
use crate::core::runner::{self, RunOptions};
use crate::core::utils::{resolved_command, truncate};
use std::collections::HashMap;
use std::ffi::OsString;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Severity {
    Error,
    Warning,
    Info,
    Other,
}

impl Severity {
    fn rank(self) -> u8 {
        match self {
            Severity::Error => 3,
            Severity::Warning => 2,
            Severity::Info => 1,
            Severity::Other => 0,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
            Severity::Other => "other",
        }
    }
}

#[derive(Debug, Clone)]
struct AnalyzeIssue {
    severity: Severity,
    file: String,
    line: usize,
    column: usize,
    rule: String,
    message: String,
}

#[derive(Debug, Clone)]
struct AnalyzeGroup {
    severity: Severity,
    rule: String,
    message: String,
    line: usize,
    column: usize,
    count: usize,
}

#[derive(Debug, Clone)]
struct FlutterTestFailure {
    file: String,
    title: String,
    details: Vec<String>,
}

pub fn run_analyze(args: &[String], verbose: u8) -> Result<i32> {
    run_flutter_filtered(
        &["analyze"],
        args,
        "flutter analyze",
        verbose,
        "flutter_analyze",
        |raw| filter_analyze_output(raw, "flutter"),
    )
}

pub fn run_pub_get(args: &[String], verbose: u8) -> Result<i32> {
    run_flutter_filtered(
        &["pub", "get"],
        args,
        "flutter pub get",
        verbose,
        "flutter_pub_get",
        filter_pub_get_output,
    )
}

pub fn run_test(args: &[String], verbose: u8) -> Result<i32> {
    run_flutter_filtered(
        &["test"],
        args,
        "flutter test",
        verbose,
        "flutter_test",
        filter_flutter_test_output,
    )
}

pub fn run_other(args: &[OsString], verbose: u8) -> Result<i32> {
    run_passthrough_command("flutter", &[], args, verbose)
}

pub fn run_pub_other(args: &[OsString], verbose: u8) -> Result<i32> {
    run_passthrough_command("flutter", &["pub"], args, verbose)
}

pub(crate) fn run_flutter_filtered<F>(
    base_args: &[&str],
    args: &[String],
    display_command: &str,
    verbose: u8,
    tee_label: &str,
    filter: F,
) -> Result<i32>
where
    F: Fn(&str) -> String,
{
    let mut cmd = resolved_command("flutter");
    for base in base_args {
        cmd.arg(base);
    }
    for arg in args {
        cmd.arg(arg);
    }

    let command_text = if args.is_empty() {
        display_command.to_string()
    } else {
        format!("{} {}", display_command, args.join(" "))
    };

    if verbose > 0 {
        eprintln!("Running: {}", command_text);
    }

    runner::run_filtered(
        cmd,
        display_command,
        &command_text,
        filter,
        RunOptions::with_tee(tee_label),
    )
}

fn run_passthrough_command(
    binary: &str,
    base_args: &[&str],
    args: &[OsString],
    verbose: u8,
) -> Result<i32> {
    let mut os_args: Vec<OsString> = base_args.iter().map(|arg| OsString::from(*arg)).collect();
    os_args.extend(args.iter().cloned());

    if verbose > 0 {
        eprintln!("Running: {} {}", binary, format_os_args(&os_args));
    }

    runner::run_passthrough(binary, &os_args, verbose)
}

pub(crate) fn filter_analyze_output(output: &str, tool_name: &str) -> String {
    let mut issues = Vec::new();
    let mut reported_total = None;
    let mut duration = None;

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("Analyzing ") {
            continue;
        }

        if let Some((total, parsed_duration)) = parse_analyze_summary_line(trimmed) {
            reported_total = Some(total);
            duration = parsed_duration;
            continue;
        }

        if let Some(issue) = parse_analyze_issue(trimmed) {
            issues.push(issue);
        }
    }

    if issues.is_empty() {
        return format!("{}: no issues", tool_name);
    }

    format_analyze_issues(tool_name, &issues, reported_total, duration.as_deref())
}

fn parse_analyze_issue(line: &str) -> Option<AnalyzeIssue> {
    let mut parts = line.splitn(4, " • ");
    let severity = parse_severity(parts.next()?.trim());
    let message = parts.next()?.trim().to_string();
    let location = parts.next()?.trim();
    let rule = parts.next()?.trim().to_string();

    let mut location_parts = location.rsplitn(3, ':');
    let column = location_parts.next()?.parse::<usize>().ok()?;
    let line_num = location_parts.next()?.parse::<usize>().ok()?;
    let file = compact_flutter_path(location_parts.next()?);

    Some(AnalyzeIssue {
        severity,
        file,
        line: line_num,
        column,
        rule,
        message,
    })
}

fn parse_severity(label: &str) -> Severity {
    match label {
        "error" => Severity::Error,
        "warning" => Severity::Warning,
        "info" => Severity::Info,
        _ => Severity::Other,
    }
}

fn parse_analyze_summary_line(line: &str) -> Option<(usize, Option<String>)> {
    if line.contains("No issues found") {
        return Some((0, None));
    }

    if !line.contains("issue found") && !line.contains("issues found") {
        return None;
    }

    let issues = line.split_whitespace().next()?.parse().ok()?;
    let duration = line.find("(ran in ").and_then(|start| {
        let rest = &line[start + "(ran in ".len()..];
        let end = rest.find('s')?;
        Some(rest[..end].to_string())
    });

    Some((issues, duration))
}

fn format_analyze_issues(
    tool_name: &str,
    issues: &[AnalyzeIssue],
    reported_total: Option<usize>,
    duration: Option<&str>,
) -> String {
    let mut files: HashMap<String, Vec<AnalyzeIssue>> = HashMap::new();

    for issue in issues {
        files
            .entry(issue.file.clone())
            .or_default()
            .push(issue.clone());
    }

    let total_issues = reported_total.unwrap_or(issues.len());
    let total_files = files.len();

    let mut file_entries: Vec<(String, Vec<AnalyzeIssue>)> = files.into_iter().collect();
    file_entries.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));

    let mut result = String::new();
    result.push_str(&format!(
        "{} issues found in {} files",
        total_issues, total_files
    ));
    if let Some(duration) = duration {
        result.push_str(&format!(" (ran in {}s)", duration));
    }
    result.push('\n');

    for (file, file_issues) in file_entries {
        result.push_str(&format!("{} ({})\n", file, file_issues.len()));

        let mut groups: HashMap<(Severity, String, String), AnalyzeGroup> = HashMap::new();
        for issue in file_issues {
            let key = (issue.severity, issue.rule.clone(), issue.message.clone());
            groups
                .entry(key)
                .and_modify(|group| {
                    group.count += 1;
                    group.line = group.line.min(issue.line);
                    group.column = group.column.min(issue.column);
                })
                .or_insert(AnalyzeGroup {
                    severity: issue.severity,
                    rule: issue.rule.clone(),
                    message: issue.message.clone(),
                    line: issue.line,
                    column: issue.column,
                    count: 1,
                });
        }

        let mut group_entries: Vec<AnalyzeGroup> = groups.into_values().collect();
        group_entries.sort_by(|a, b| {
            b.severity
                .rank()
                .cmp(&a.severity.rank())
                .then_with(|| b.count.cmp(&a.count))
                .then_with(|| a.rule.cmp(&b.rule))
                .then_with(|| a.line.cmp(&b.line))
        });

        for group in group_entries {
            result.push_str(&format!(
                "  {} {} L{}:{} ({}x) {}\n",
                group.severity.as_str(),
                group.rule,
                group.line,
                group.column,
                group.count,
                truncate(&group.message, 96),
            ));
        }

        result.push('\n');
    }

    let result = result.trim().to_string();
    if result.is_empty() {
        format!("{}: no issues", tool_name)
    } else {
        result
    }
}

fn filter_pub_get_output(output: &str) -> String {
    let mut lines: Vec<String> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || is_pub_noise_line(trimmed) {
            continue;
        }

        lines.push(trimmed.to_string());
    }

    if lines.is_empty() {
        return "flutter pub get: ok".to_string();
    }

    dedupe_consecutive(lines).join("\n")
}

fn is_pub_noise_line(line: &str) -> bool {
    line == "Downloading packages..."
        || line == "Got dependencies!"
        || line.starts_with("Resolving dependencies")
        || line.starts_with('+')
        || line.starts_with('>')
        || line.starts_with("Precompiling executables")
        || line.starts_with("Running " )
}

fn dedupe_consecutive(lines: Vec<String>) -> Vec<String> {
    let mut deduped: Vec<String> = Vec::new();
    for line in lines {
        if deduped.last().map(|prev| prev == &line).unwrap_or(false) {
            continue;
        }
        deduped.push(line);
    }
    deduped
}

pub(crate) fn filter_flutter_test_output(output: &str) -> String {
    let mut passed: Option<usize> = None;
    let mut failed: Option<usize> = None;
    let mut failure_blocks: Vec<FlutterTestFailure> = Vec::new();
    let mut current_failure: Option<FlutterTestFailure> = None;
    let mut context_lines: Vec<String> = Vec::new();
    let mut failing_tests: Vec<String> = Vec::new();
    let mut in_failing_tests = false;

    for line in output.lines() {
        let trimmed = line.trim_end();

        if let Some((seen_passed, seen_failed)) = parse_test_counts(trimmed) {
            passed = Some(seen_passed);
            failed = Some(seen_failed);
        }

        if trimmed == "Failing tests:" {
            if let Some(block) = current_failure.take() {
                failure_blocks.push(block);
            }
            in_failing_tests = true;
            continue;
        }

        if in_failing_tests {
            if trimmed.is_empty() {
                continue;
            }
            failing_tests.push(trimmed.to_string());
            continue;
        }

        if is_test_progress_line(trimmed) {
            if let Some(block) = current_failure.take() {
                failure_blocks.push(block);
            }

            if trimmed.contains("[E]") {
                if let Some(block) = parse_test_failure_header(trimmed) {
                    current_failure = Some(block);
                }
            }
            continue;
        }

        if let Some(block) = current_failure.as_mut() {
            if trimmed.is_empty() {
                if !block.details.last().map(|s| s.is_empty()).unwrap_or(false) {
                    block.details.push(String::new());
                }
                continue;
            }

            if is_test_failure_detail(trimmed) || block.details.len() < 6 {
                block.details.push(trimmed.to_string());
            }
            continue;
        }

        if !trimmed.is_empty() && !trimmed.starts_with("loading ") {
            context_lines.push(trimmed.to_string());
        }
    }

    if let Some(block) = current_failure.take() {
        failure_blocks.push(block);
    }

    if passed.is_none() && failed.is_none() && failure_blocks.is_empty() {
        return "flutter test: no output".to_string();
    }

    let mut result = String::new();
    if let (Some(pass_count), Some(fail_count)) = (passed, failed) {
        result.push_str(&format!("{} passed, {} failed", pass_count, fail_count));
    } else if let Some(fail_count) = failed {
        result.push_str(&format!("{} failed", fail_count));
    } else {
        result.push_str(&format!("{} failures", failure_blocks.len()));
    }
    result.push('\n');

    if !context_lines.is_empty() {
        result.push('\n');
        result.push_str("Context:\n");
        for line in context_lines.iter().take(5) {
            result.push_str(&format!("  {}\n", truncate(line, 120)));
        }
        if context_lines.len() > 5 {
            result.push_str(&format!("  ... +{} more\n", context_lines.len() - 5));
        }
    }

    if !failure_blocks.is_empty() {
        result.push('\n');
        result.push_str("Failures:\n");
        for (idx, block) in failure_blocks.iter().enumerate() {
            result.push_str(&format!("{}. {} ({})\n", idx + 1, block.title, block.file));
            for detail in &block.details {
                if detail.is_empty() {
                    result.push('\n');
                } else {
                    result.push_str(&format!("   {}\n", truncate(detail, 120)));
                }
            }
            result.push('\n');
        }
    }

    if !failing_tests.is_empty() {
        result.push_str("Failing tests:\n");
        for line in failing_tests {
            result.push_str(&format!("  {}\n", truncate(&line, 120)));
        }
    }

    result.trim().to_string()
}

fn is_test_progress_line(line: &str) -> bool {
    let bytes = line.as_bytes();
    bytes.len() >= 7
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2] == b':'
        && bytes[3].is_ascii_digit()
        && bytes[4].is_ascii_digit()
        && bytes[5] == b' '
        && bytes[6] == b'+'
}

fn parse_test_counts(line: &str) -> Option<(usize, usize)> {
    if !is_test_progress_line(line) {
        return None;
    }

    let after_plus = line.split_once('+')?.1;
    let passed = after_plus.split_whitespace().next()?.parse().ok()?;
    let failed = after_plus
        .split_once(" -")
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .and_then(|value| value.trim_end_matches(':').parse().ok())
        .unwrap_or(0);

    Some((passed, failed))
}

fn parse_test_failure_header(line: &str) -> Option<FlutterTestFailure> {
    let mut parts = line.splitn(3, ": ");
    let _time = parts.next()?;
    let file = compact_flutter_path(parts.next()?);
    let title = parts.next()?.trim_end_matches(" [E]").trim().to_string();

    Some(FlutterTestFailure {
        file,
        title,
        details: Vec::new(),
    })
}

fn is_test_failure_detail(line: &str) -> bool {
    line.starts_with("══╡")
        || line.starts_with("The following TestFailure")
        || line.starts_with("The test description was:")
        || line.starts_with("This was caught")
        || line.starts_with("When the exception was thrown")
        || line.starts_with("Expected:")
        || line.starts_with("Actual:")
        || line.starts_with("Which:")
        || line.starts_with("Test failed.")
        || line.starts_with('#')
        || line.starts_with("package:")
        || line.starts_with("file://")
}

fn compact_flutter_path(path: &str) -> String {
    let path = path.trim_start_matches("file://");
    if let Some(idx) = path.find("/rtk_flutter/") {
        path[idx + "/rtk_flutter/".len()..].to_string()
    } else {
        path.to_string()
    }
}

fn format_os_args(args: &[OsString]) -> String {
    args.iter()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_filter_analyze_output_groups_by_file() {
        let input = include_str!("../../../tests/fixtures/flutter_analyze_raw.txt");
        let output = filter_analyze_output(input, "flutter");
        assert!(output.contains("20 issues found in 3 files"));
        assert!(output.contains("lib/main.dart"));
        assert!(output.contains("L21:5"));
        assert!(output.contains("unused_element"));
        assert!(!output.contains("Analyzing rtk_flutter"));
    }

    #[test]
    fn test_filter_analyze_output_savings() {
        let input = include_str!("../../../tests/fixtures/flutter_analyze_raw.txt");
        let output = filter_analyze_output(input, "flutter");
        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(savings >= 34.0, "Expected at least 34% savings, got {:.1}%", savings);
    }

    #[test]
    fn test_filter_pub_get_output_strips_package_noise() {
        let input = include_str!("../../../tests/fixtures/flutter_pub_get_raw.txt");
        let output = filter_pub_get_output(input);
        assert!(output.contains("Changed 58 dependencies"));
        assert!(output.contains("packages have newer versions"));
        assert!(!output.contains("Downloading packages"));
    }

    #[test]
    fn test_filter_pub_get_output_savings() {
        let input = include_str!("../../../tests/fixtures/flutter_pub_get_raw.txt");
        let output = filter_pub_get_output(input);
        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(savings >= 60.0, "Expected at least 60% savings, got {:.1}%", savings);
    }

    #[test]
    fn test_filter_flutter_test_output_keeps_failures() {
        let input = include_str!("../../../tests/fixtures/flutter_test_raw.txt");
        let output = filter_flutter_test_output(input);
        assert!(output.contains("11 passed, 3 failed"));
        assert!(output.contains("Failures:"));
        assert!(output.contains("Failing tests:"));
        assert!(output.contains("counter starts at 5"));
    }

    #[test]
    fn test_filter_flutter_test_output_savings() {
        let input = include_str!("../../../tests/fixtures/flutter_test_verbose_raw.txt");
        let output = filter_flutter_test_output(input);
        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(savings >= 60.0, "Expected at least 60% savings, got {:.1}%", savings);
    }
}