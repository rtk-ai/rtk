//! Filters golangci-lint output, grouping issues by rule.

use crate::core::config;
use crate::core::runner;
use crate::core::stream::exec_capture;
use crate::core::truncate::CAP_ERRORS;
use crate::core::utils::{resolved_command, truncate};
use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;
use std::ffi::OsString;

const GOLANGCI_SUBCOMMANDS: &[&str] = &[
    "cache",
    "completion",
    "config",
    "custom",
    "fmt",
    "formatters",
    "help",
    "linters",
    "migrate",
    "run",
    "version",
];

const GLOBAL_FLAGS_WITH_VALUE: &[&str] = &[
    "-c",
    "--color",
    "--config",
    "--cpu-profile-path",
    "--mem-profile-path",
    "--trace-path",
];

#[derive(Debug, PartialEq, Eq)]
struct RunInvocation {
    global_args: Vec<String>,
    run_args: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum Invocation {
    FilteredRun(RunInvocation),
    Passthrough,
}

#[derive(Debug, Deserialize)]
struct Position {
    #[serde(rename = "Filename")]
    filename: String,
    #[serde(rename = "Line")]
    line: usize,
    #[serde(rename = "Column")]
    column: usize,
    #[serde(rename = "Offset", default)]
    #[allow(dead_code)]
    offset: usize,
}

#[derive(Debug, Deserialize)]
struct Issue {
    #[serde(rename = "FromLinter")]
    from_linter: String,
    #[serde(rename = "Text")]
    text: String,
    #[serde(rename = "Pos")]
    pos: Position,
    #[serde(rename = "SourceLines", default)]
    source_lines: Vec<String>,
    #[serde(rename = "Severity", default)]
    #[allow(dead_code)]
    severity: String,
}

#[derive(Debug, Deserialize)]
struct GolangciOutput {
    #[serde(rename = "Issues")]
    issues: Vec<Issue>,
}

/// Parse major version number from `golangci-lint --version` output.
/// Returns 1 on any failure (safe fallback — v1 behaviour).
pub(crate) fn parse_major_version(version_output: &str) -> u32 {
    // Handles:
    //   "golangci-lint version 1.59.1"
    //   "golangci-lint has version 2.10.0 built with ..."
    for word in version_output.split_whitespace() {
        if let Some(major) = word.split('.').next().and_then(|s| s.parse::<u32>().ok()) {
            if word.contains('.') {
                return major;
            }
        }
    }
    1
}

/// Run `golangci-lint --version` and return the major version number.
/// Returns 1 on any failure.
pub(crate) fn detect_major_version() -> u32 {
    let mut cmd = resolved_command("golangci-lint");
    cmd.arg("--version");

    match exec_capture(&mut cmd) {
        Ok(r) => {
            let version_text = if r.stdout.trim().is_empty() {
                &r.stderr
            } else {
                &r.stdout
            };
            parse_major_version(version_text)
        }
        Err(_) => 1,
    }
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    match classify_invocation(args) {
        // A user-chosen output format wins: forcing our JSON parse onto a
        // non-JSON format breaks and emits a parse-error string that reads as a
        // tool failure. Honor what they asked for verbatim.
        Invocation::FilteredRun(invocation) if has_output_flag(&invocation.run_args) => {
            run_passthrough(args, verbose)
        }
        Invocation::FilteredRun(invocation) => run_filtered(args, &invocation, verbose),
        Invocation::Passthrough => run_passthrough(args, verbose),
    }
}

fn run_filtered(original_args: &[String], invocation: &RunInvocation, verbose: u8) -> Result<i32> {
    let version = detect_major_version();

    let mut cmd = resolved_command("golangci-lint");
    for arg in build_filtered_args(invocation, version) {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!(
            "Running: {}",
            format_command("golangci-lint", &build_filtered_args(invocation, version))
        );
    }

    let exit_code = runner::run_filtered(
        cmd,
        "golangci-lint",
        &original_args.join(" "),
        |stdout| {
            // v2 outputs JSON on first line + trailing text; v1 outputs just JSON
            let json_output = if version >= 2 {
                stdout.lines().next().unwrap_or("")
            } else {
                stdout
            };
            filter_golangci_json(json_output, version)
        },
        crate::core::runner::RunOptions::stdout_only(),
    )?;

    // golangci-lint: exit 0 = clean, exit 1 = lint issues found (not an error),
    // exit 2+ = config/build error, None = killed by signal (OOM, SIGKILL)
    Ok(if exit_code == 1 { 0 } else { exit_code })
}

fn run_passthrough(args: &[String], verbose: u8) -> Result<i32> {
    let os_args: Vec<OsString> = args.iter().map(OsString::from).collect();
    runner::run_passthrough("golangci-lint", &os_args, verbose)
}

fn classify_invocation(args: &[String]) -> Invocation {
    match find_subcommand_index(args) {
        Some(idx) if args[idx] == "run" => Invocation::FilteredRun(RunInvocation {
            global_args: args[..idx].to_vec(),
            run_args: args[idx + 1..].to_vec(),
        }),
        _ => Invocation::Passthrough,
    }
}

fn find_subcommand_index(args: &[String]) -> Option<usize> {
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();

        if arg == "--" {
            return None;
        }

        if !arg.starts_with('-') {
            if GOLANGCI_SUBCOMMANDS.contains(&arg) {
                return Some(i);
            }
            return None;
        }

        if let Some(flag) = split_flag_name(arg) {
            if golangci_flag_takes_separate_value(arg, flag) {
                i += 1;
            }
        }

        i += 1;
    }

    None
}

fn split_flag_name(arg: &str) -> Option<&str> {
    if arg.starts_with("--") {
        return Some(arg.split_once('=').map(|(flag, _)| flag).unwrap_or(arg));
    }

    if arg.starts_with('-') {
        return Some(arg);
    }

    None
}

fn golangci_flag_takes_separate_value(arg: &str, flag: &str) -> bool {
    if !GLOBAL_FLAGS_WITH_VALUE.contains(&flag) {
        return false;
    }

    if arg.starts_with("--") && arg.contains('=') {
        return false;
    }

    true
}

fn build_filtered_args(invocation: &RunInvocation, version: u32) -> Vec<String> {
    let mut args = invocation.global_args.clone();
    args.push("run".to_string());

    if !has_output_flag(&invocation.run_args) {
        if version >= 2 {
            args.push("--output.json.path".to_string());
            args.push("stdout".to_string());
        } else {
            args.push("--out-format=json".to_string());
        }
    }

    args.extend(invocation.run_args.clone());
    args
}

fn has_output_flag(args: &[String]) -> bool {
    args.iter().any(|a| {
        a == "--out-format"
            || a.starts_with("--out-format=")
            || a == "--output.json.path"
            || a.starts_with("--output.json.path=")
    })
}

fn format_command(base: &str, args: &[String]) -> String {
    if args.is_empty() {
        base.to_string()
    } else {
        format!("{} {}", base, args.join(" "))
    }
}

/// Filter golangci-lint JSON output into standard `file:line:col: message (linter)`
/// findings, with per-linter counts as a header.
pub(crate) fn filter_golangci_json(output: &str, version: u32) -> String {
    let result: Result<GolangciOutput, _> = serde_json::from_str(output);

    let golangci_output = match result {
        Ok(o) => o,
        // Not JSON (user-supplied output format, or an unexpected shape): hand
        // back the linter's own output rather than a "JSON parse failed" string
        // that reads as a tool failure and provokes retries.
        Err(_) => return truncate(output.trim(), config::limits().passthrough_max_chars),
    };

    let issues = golangci_output.issues;

    if issues.is_empty() {
        return "golangci-lint: No issues found".to_string();
    }

    let total_issues = issues.len();
    let unique_files: std::collections::HashSet<_> =
        issues.iter().map(|i| &i.pos.filename).collect();
    let total_files = unique_files.len();

    let mut by_linter: HashMap<&str, usize> = HashMap::new();
    for issue in &issues {
        *by_linter.entry(issue.from_linter.as_str()).or_insert(0) += 1;
    }
    let mut linter_counts: Vec<_> = by_linter.into_iter().collect();
    linter_counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    let linters_summary = linter_counts
        .iter()
        .map(|(linter, count)| format!("{} ({}x)", linter, count))
        .collect::<Vec<_>>()
        .join(", ");

    let mut result = format!(
        "golangci-lint: {} issues in {} files\nLinters: {}\n\n",
        total_issues, total_files, linters_summary
    );

    const MAX_GOLANGCI_ISSUES: usize = CAP_ERRORS;
    for issue in issues.iter().take(MAX_GOLANGCI_ISSUES) {
        result.push_str(&format!(
            "{}:{}:{}: {} ({})\n",
            compact_path(&issue.pos.filename),
            issue.pos.line,
            issue.pos.column,
            issue.text.trim(),
            issue.from_linter,
        ));

        // v2 carries the offending source line — keep it as secondary context.
        if version >= 2 {
            if let Some(source_line) = issue.source_lines.first() {
                let trimmed = source_line.trim();
                if !trimmed.is_empty() {
                    let display = match trimmed.char_indices().nth(80) {
                        Some((i, _)) => &trimmed[..i],
                        None => trimmed,
                    };
                    result.push_str(&format!("    → {}\n", display));
                }
            }
        }
    }

    if total_issues > MAX_GOLANGCI_ISSUES {
        result.push_str(&format!(
            "\n... +{} more issues\n",
            total_issues - MAX_GOLANGCI_ISSUES
        ));
    }

    result.trim().to_string()
}

/// Compact file path (remove common prefixes)
fn compact_path(path: &str) -> String {
    let path = path.replace('\\', "/");

    if let Some(pos) = path.rfind("/pkg/") {
        format!("pkg/{}", &path[pos + 5..])
    } else if let Some(pos) = path.rfind("/cmd/") {
        format!("cmd/{}", &path[pos + 5..])
    } else if let Some(pos) = path.rfind("/internal/") {
        format!("internal/{}", &path[pos + 10..])
    } else if let Some(pos) = path.rfind('/') {
        path[pos + 1..].to_string()
    } else {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_golangci_no_issues() {
        let output = r#"{"Issues":[]}"#;
        let result = filter_golangci_json(output, 1);
        assert!(result.contains("golangci-lint"));
        assert!(result.contains("No issues found"));
    }

    #[test]
    fn test_filter_golangci_with_issues() {
        let output = r#"{
  "Issues": [
    {
      "FromLinter": "errcheck",
      "Text": "Error return value not checked",
      "Pos": {"Filename": "main.go", "Line": 42, "Column": 5}
    },
    {
      "FromLinter": "errcheck",
      "Text": "Error return value not checked",
      "Pos": {"Filename": "main.go", "Line": 50, "Column": 10}
    },
    {
      "FromLinter": "gosimple",
      "Text": "Should use strings.Contains",
      "Pos": {"Filename": "utils.go", "Line": 15, "Column": 2}
    }
  ]
}"#;

        let result = filter_golangci_json(output, 1);
        assert!(result.contains("3 issues"));
        assert!(result.contains("2 files"));
        assert!(result.contains("errcheck"));
        assert!(result.contains("gosimple"));
        assert!(result.contains("main.go"));
        assert!(result.contains("utils.go"));
    }

    #[test]
    fn test_filter_golangci_surfaces_message_and_location() {
        // The violation message (Text) is the actionable signal — counts alone
        // forced agents to retry to recover it.
        let output = r#"{
  "Issues": [
    {
      "FromLinter": "typecheck",
      "Text": "could not import github.com/google/gopacket/pcap (fatal error: pcap.h: No such file or directory)",
      "Pos": {"Filename": "internal/sniff/capture.go", "Line": 9, "Column": 2}
    }
  ]
}"#;

        let result = filter_golangci_json(output, 1);
        assert!(
            result.contains("capture.go:9:2:"),
            "Expected file:line:col, got: {}",
            result
        );
        assert!(
            result.contains("pcap.h: No such file or directory"),
            "Expected the violation message, got: {}",
            result
        );
        assert!(
            result.contains("(typecheck)"),
            "Expected the linter name per finding, got: {}",
            result
        );
    }

    #[test]
    fn test_filter_golangci_parse_failure_is_clean_passthrough() {
        // A non-JSON payload (e.g. user-supplied --out-format) must not emit a
        // scary "JSON parse failed" string that reads as a tool failure.
        let raw = "internal/foo.go:10:2: something is wrong (govet)";
        let result = filter_golangci_json(raw, 1);
        assert!(!result.contains("parse failed"), "got: {}", result);
        assert!(result.contains("something is wrong"), "got: {}", result);
    }

    #[test]
    fn test_compact_path() {
        assert_eq!(
            compact_path("/Users/foo/project/pkg/handler/server.go"),
            "pkg/handler/server.go"
        );
        assert_eq!(
            compact_path("/home/user/app/cmd/main/main.go"),
            "cmd/main/main.go"
        );
        assert_eq!(
            compact_path("/project/internal/config/loader.go"),
            "internal/config/loader.go"
        );
        assert_eq!(compact_path("relative/file.go"), "file.go");
    }

    #[test]
    fn test_parse_version_v1_format() {
        assert_eq!(parse_major_version("golangci-lint version 1.59.1"), 1);
    }

    #[test]
    fn test_parse_version_v2_format() {
        assert_eq!(
            parse_major_version("golangci-lint has version 2.10.0 built with go1.26.0 from 95dcb68a on 2026-02-17T13:05:51Z"),
            2
        );
    }

    #[test]
    fn test_parse_version_empty_returns_1() {
        assert_eq!(parse_major_version(""), 1);
    }

    #[test]
    fn test_parse_version_malformed_returns_1() {
        assert_eq!(parse_major_version("not a version string"), 1);
    }

    #[test]
    fn test_classify_invocation_run_uses_filtered_path() {
        assert_eq!(
            classify_invocation(&["run".into(), "./...".into()]),
            Invocation::FilteredRun(RunInvocation {
                global_args: vec![],
                run_args: vec!["./...".into()],
            })
        );
    }

    #[test]
    fn test_classify_invocation_with_global_flag_value_uses_filtered_path() {
        assert_eq!(
            classify_invocation(&[
                "--color".into(),
                "never".into(),
                "run".into(),
                "./...".into(),
            ]),
            Invocation::FilteredRun(RunInvocation {
                global_args: vec!["--color".into(), "never".into()],
                run_args: vec!["./...".into()],
            })
        );
    }

    #[test]
    fn test_classify_invocation_with_short_global_flag_uses_filtered_path() {
        assert_eq!(
            classify_invocation(&["-v".into(), "run".into(), "./...".into()]),
            Invocation::FilteredRun(RunInvocation {
                global_args: vec!["-v".into()],
                run_args: vec!["./...".into()],
            })
        );
    }

    #[test]
    fn test_classify_invocation_with_inline_value_flag_uses_filtered_path() {
        assert_eq!(
            classify_invocation(&["--color=never".into(), "run".into(), "./...".into()]),
            Invocation::FilteredRun(RunInvocation {
                global_args: vec!["--color=never".into()],
                run_args: vec!["./...".into()],
            })
        );
    }

    #[test]
    fn test_classify_invocation_with_inline_config_flag_uses_filtered_path() {
        assert_eq!(
            classify_invocation(&["--config=foo.yml".into(), "run".into(), "./...".into()]),
            Invocation::FilteredRun(RunInvocation {
                global_args: vec!["--config=foo.yml".into()],
                run_args: vec!["./...".into()],
            })
        );
    }

    #[test]
    fn test_classify_invocation_bare_command_is_passthrough() {
        assert_eq!(classify_invocation(&[]), Invocation::Passthrough);
    }

    #[test]
    fn test_classify_invocation_version_flag_is_passthrough() {
        assert_eq!(
            classify_invocation(&["--version".into()]),
            Invocation::Passthrough
        );
    }

    #[test]
    fn test_classify_invocation_version_subcommand_is_passthrough() {
        assert_eq!(
            classify_invocation(&["version".into()]),
            Invocation::Passthrough
        );
    }

    #[test]
    fn test_has_output_flag_detects_user_formats() {
        assert!(has_output_flag(&["--out-format=line-number".into()]));
        assert!(has_output_flag(&["--out-format".into(), "tab".into()]));
        assert!(has_output_flag(&["--output.json.path".into(), "out.json".into()]));
        assert!(has_output_flag(&["--output.json.path=out.json".into()]));
        assert!(!has_output_flag(&["./...".into()]));
        assert!(!has_output_flag(&["--fix".into()]));
    }

    #[test]
    fn test_build_filtered_args_does_not_duplicate_run() {
        let invocation = RunInvocation {
            global_args: vec![],
            run_args: vec!["./...".into()],
        };

        assert_eq!(
            build_filtered_args(&invocation, 2),
            vec!["run", "--output.json.path", "stdout", "./..."]
        );
    }

    #[test]
    fn test_filter_golangci_v2_fields_parse_cleanly() {
        // v2 JSON includes Severity, SourceLines, Offset — must not panic
        let output = r#"{
  "Issues": [
    {
      "FromLinter": "errcheck",
      "Text": "Error return value not checked",
      "Severity": "error",
      "SourceLines": ["    if err := foo(); err != nil {"],
      "Pos": {"Filename": "main.go", "Line": 42, "Column": 5, "Offset": 1024}
    }
  ]
}"#;
        let result = filter_golangci_json(output, 2);
        assert!(result.contains("errcheck"));
        assert!(result.contains("main.go"));
    }

    #[test]
    fn test_filter_v2_shows_source_lines() {
        let output = r#"{
  "Issues": [
    {
      "FromLinter": "errcheck",
      "Text": "Error return value not checked",
      "Severity": "error",
      "SourceLines": ["    if err := foo(); err != nil {"],
      "Pos": {"Filename": "main.go", "Line": 42, "Column": 5, "Offset": 0}
    }
  ]
}"#;
        let result = filter_golangci_json(output, 2);
        assert!(
            result.contains("→"),
            "v2 should show source line with → prefix"
        );
        assert!(result.contains("if err := foo()"));
    }

    #[test]
    fn test_filter_v1_does_not_show_source_lines() {
        let output = r#"{
  "Issues": [
    {
      "FromLinter": "errcheck",
      "Text": "Error return value not checked",
      "Severity": "error",
      "SourceLines": ["    if err := foo(); err != nil {"],
      "Pos": {"Filename": "main.go", "Line": 42, "Column": 5, "Offset": 0}
    }
  ]
}"#;
        let result = filter_golangci_json(output, 1);
        assert!(!result.contains("→"), "v1 should not show source lines");
    }

    #[test]
    fn test_filter_v2_empty_source_lines_graceful() {
        let output = r#"{
  "Issues": [
    {
      "FromLinter": "errcheck",
      "Text": "Error return value not checked",
      "Severity": "",
      "SourceLines": [],
      "Pos": {"Filename": "main.go", "Line": 42, "Column": 5, "Offset": 0}
    }
  ]
}"#;
        let result = filter_golangci_json(output, 2);
        assert!(result.contains("errcheck"));
        assert!(
            !result.contains("→"),
            "no source line to show, should degrade gracefully"
        );
    }

    #[test]
    fn test_filter_v2_source_line_truncated_to_80_chars() {
        let long_line = "x".repeat(120);
        let output = format!(
            r#"{{
  "Issues": [
    {{
      "FromLinter": "lll",
      "Text": "line too long",
      "Severity": "",
      "SourceLines": ["{}"],
      "Pos": {{"Filename": "main.go", "Line": 1, "Column": 1, "Offset": 0}}
    }}
  ]
}}"#,
            long_line
        );
        let result = filter_golangci_json(&output, 2);
        // Content truncated at 80 chars; prefix "      → " = 10 bytes (6 spaces + 3-byte arrow + space)
        // Total line max = 80 + 10 = 90 bytes
        for line in result.lines() {
            if line.trim_start().starts_with('→') {
                assert!(line.len() <= 90, "source line too long: {}", line.len());
            }
        }
    }

    #[test]
    fn test_filter_v2_source_line_truncated_non_ascii() {
        // Japanese characters are 3 bytes each; 30 chars = 90 bytes > 80 bytes naive slice would panic
        let long_line = "日".repeat(30); // 30 chars, 90 bytes
        let output = format!(
            r#"{{
  "Issues": [
    {{
      "FromLinter": "lll",
      "Text": "line too long",
      "Severity": "",
      "SourceLines": ["{}"],
      "Pos": {{"Filename": "main.go", "Line": 1, "Column": 1, "Offset": 0}}
    }}
  ]
}}"#,
            long_line
        );
        // Should not panic and output should be ≤ 80 chars
        let result = filter_golangci_json(&output, 2);
        for line in result.lines() {
            if line.trim_start().starts_with('→') {
                let content = line.trim_start().trim_start_matches('→').trim();
                assert!(
                    content.chars().count() <= 80,
                    "content chars: {}",
                    content.chars().count()
                );
            }
        }
    }

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_golangci_v2_token_savings() {
        let raw = include_str!("../../../tests/fixtures/golangci_v2_json.txt");

        let filtered = filter_golangci_json(raw, 2);
        let savings = 100.0 - (count_tokens(&filtered) as f64 / count_tokens(raw) as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "Expected ≥60% token savings, got {:.1}%\nFiltered output:\n{}",
            savings,
            filtered
        );
    }
}

