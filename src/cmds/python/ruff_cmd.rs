//! Filters Ruff linter and formatter output.

use crate::core::config;
use crate::core::runner;
use crate::core::truncate::CAP_WARNINGS;
use crate::core::utils::{resolved_command, truncate};
use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;

const RUFF_SUBCOMMANDS: &[&str] = &[
    "analyze",
    "check",
    "clean",
    "config",
    "format",
    "generate-shell-completion",
    "help",
    "linter",
    "rule",
    "server",
    "version",
];

#[derive(Debug, Deserialize)]
struct RuffLocation {
    row: usize,
    column: usize,
}

#[derive(Debug, Deserialize)]
struct RuffFix {
    #[allow(dead_code)]
    applicability: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RuffDiagnostic {
    code: String,
    message: String,
    location: RuffLocation,
    #[allow(dead_code)]
    end_location: Option<RuffLocation>,
    filename: String,
    fix: Option<RuffFix>,
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let is_check = is_check_invocation(args);

    let is_format = args.iter().any(|a| a == "format");

    let mut cmd = resolved_command("ruff");

    // Both spellings: injecting a second --output-format makes ruff reject the call.
    let user_set_output_format = args
        .iter()
        .any(|a| a == "--output-format" || a.starts_with("--output-format="));

    let user_json_format = args.iter().enumerate().any(|(i, a)| {
        a == "--output-format=json"
            || (a == "--output-format" && args.get(i + 1).is_some_and(|n| n == "json"))
    });
    let use_json_filter = !user_set_output_format || user_json_format;

    if is_check {
        if user_set_output_format {
            cmd.arg("check");
        } else {
            cmd.arg("check").arg("--output-format=json");
        }

        let start_idx = if !args.is_empty() && args[0] == "check" {
            1
        } else {
            0
        };
        for arg in &args[start_idx..] {
            cmd.arg(arg);
        }

        if args
            .iter()
            .skip(start_idx)
            .all(|a| a.starts_with('-') || a.contains('='))
        {
            cmd.arg(".");
        }
    } else {
        for arg in args {
            cmd.arg(arg);
        }
    }

    if verbose > 0 {
        eprintln!("Running: ruff {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "ruff",
        &args.join(" "),
        move |stdout| {
            if is_check && use_json_filter && !stdout.trim().is_empty() {
                filter_ruff_check_json(stdout)
            } else if is_format {
                filter_ruff_format(stdout)
            } else {
                truncate(stdout.trim(), config::limits().passthrough_max_chars)
            }
        },
        runner::RunOptions::stdout_only(),
    )
}

fn is_check_invocation(args: &[String]) -> bool {
    args.first().is_none_or(|arg| {
        arg == "check" || (!arg.starts_with('-') && !RUFF_SUBCOMMANDS.contains(&arg.as_str()))
    })
}

/// Filter ruff check JSON output - group by rule and file
pub fn filter_ruff_check_json(output: &str) -> String {
    let diagnostics: Result<Vec<RuffDiagnostic>, _> = serde_json::from_str(output);

    let diagnostics = match diagnostics {
        Ok(d) => d,
        Err(e) => {
            // Fallback if JSON parsing fails
            return format!(
                "Ruff check (JSON parse failed: {})\n{}",
                e,
                truncate(output, config::limits().passthrough_max_chars)
            );
        }
    };

    if diagnostics.is_empty() {
        return "Ruff: No issues found".to_string();
    }

    let total_issues = diagnostics.len();
    let fixable_count = diagnostics.iter().filter(|d| d.fix.is_some()).count();

    // Count unique files
    let unique_files: std::collections::HashSet<_> =
        diagnostics.iter().map(|d| &d.filename).collect();
    let total_files = unique_files.len();

    // Group by rule code
    let mut by_rule: HashMap<String, usize> = HashMap::new();
    for diag in &diagnostics {
        *by_rule.entry(diag.code.clone()).or_insert(0) += 1;
    }

    // Group by file
    let mut by_file: HashMap<&str, usize> = HashMap::new();
    for diag in &diagnostics {
        *by_file.entry(&diag.filename).or_insert(0) += 1;
    }

    let mut file_counts: Vec<_> = by_file.iter().collect();
    file_counts.sort_by(|a, b| b.1.cmp(a.1));

    // Build output
    let mut result = String::new();
    result.push_str(&format!(
        "Ruff: {} issues in {} files",
        total_issues, total_files
    ));

    if fixable_count > 0 {
        result.push_str(&format!(" ({} fixable)", fixable_count));
    }
    result.push('\n');

    // Show top rules
    let mut rule_counts: Vec<_> = by_rule.iter().collect();
    rule_counts.sort_by(|a, b| b.1.cmp(a.1));

    const MAX_RUFF_RULES: usize = CAP_WARNINGS;
    const MAX_RUFF_FILES: usize = CAP_WARNINGS;
    if !rule_counts.is_empty() {
        result.push_str("Top rules:\n");
        for (rule, count) in rule_counts.iter().take(MAX_RUFF_RULES) {
            result.push_str(&format!("  {} ({}x)\n", rule, count));
        }
        result.push('\n');
    }

    // Show top files
    result.push_str("Top files:\n");
    for (file, count) in file_counts.iter().take(MAX_RUFF_FILES) {
        let short_path = compact_path(file);
        result.push_str(&format!("  {} ({} issues)\n", short_path, count));

        // Show top 3 rules in this file
        let mut file_rules: HashMap<String, usize> = HashMap::new();
        for diag in diagnostics.iter().filter(|d| &d.filename == *file) {
            *file_rules.entry(diag.code.clone()).or_insert(0) += 1;
        }

        let mut file_rule_counts: Vec<_> = file_rules.iter().collect();
        file_rule_counts.sort_by(|a, b| b.1.cmp(a.1));

        for (rule, count) in file_rule_counts.iter().take(3) {
            result.push_str(&format!("    {} ({})\n", rule, count));
        }
    }

    if file_counts.len() > MAX_RUFF_FILES {
        result.push_str(&format!(
            "\n... +{} more files\n",
            file_counts.len() - MAX_RUFF_FILES
        ));
    }

    const MAX_VIOLATIONS: usize = 50;
    let violation_lines: Vec<String> = diagnostics
        .iter()
        .map(|diag| {
            format!(
                "  {}:{}:{} {} {}\n",
                compact_path(&diag.filename),
                diag.location.row,
                diag.location.column,
                diag.code,
                truncate(diag.message.trim(), 100),
            )
        })
        .collect();

    result.push_str("\nViolations:\n");
    for line in violation_lines.iter().take(MAX_VIOLATIONS) {
        result.push_str(line);
    }
    if violation_lines.len() > MAX_VIOLATIONS {
        result.push_str(&format!(
            "  … +{} more\n",
            violation_lines.len() - MAX_VIOLATIONS
        ));
        let full: String = violation_lines.concat();
        if let Some(hint) =
            crate::core::tee::force_tee_tail_hint(&full, "ruff-check", MAX_VIOLATIONS + 1)
        {
            result.push_str(&format!("  {}\n", hint));
        }
    }

    if fixable_count > 0 {
        result.push_str(&format!(
            "\n[hint] Run `ruff check --fix` to auto-fix {} issues\n",
            fixable_count
        ));
    }

    result.trim().to_string()
}

/// Filter ruff format output - show files that need formatting
pub fn filter_ruff_format(output: &str) -> String {
    let output = crate::core::utils::strip_ansi(output);
    let mut files_to_format: Vec<String> = Vec::new();
    let mut files_checked = 0;

    for line in output.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_lowercase();

        // Pre-0.12: "Would reformat: path/to/file.py"
        // Current:   "unformatted: path/to/file.py" (or a diagnostic message after the colon)
        if lower.contains("would reformat:") || lower.starts_with("unformatted:") {
            if let Some(filename) = path_after_first_colon(trimmed) {
                push_unique_file(&mut files_to_format, filename);
            }
        }

        // Default ruff ≥0.12 full format: " --> src/main.py:1:1"
        if let Some(filename) = path_from_arrow_location(trimmed) {
            push_unique_file(&mut files_to_format, filename);
        }

        // Concise format: "src/main.py:1:1: unformatted: File would be reformatted"
        if let Some(filename) = path_from_concise_unformatted(trimmed, &lower) {
            push_unique_file(&mut files_to_format, filename);
        }

        // Pre-0.12 / write mode: "3 files left unchanged"
        // Current check:         "3 files already formatted"
        if let Some(count) = parse_already_ok_count(trimmed, &lower) {
            files_checked = count;
        }
    }

    let output_lower = output.to_lowercase();
    let saw_ok_summary = output_lower.contains("left unchanged")
        || output_lower.contains("already formatted");
    // "would reformat" does not match "would be reformatted" — keep both.
    let saw_check_rewrite = output_lower.contains("would reformat:")
        || output_lower.contains("unformatted:")
        || output_lower.contains("would be reformatted");

    // Check if all files are formatted
    if files_to_format.is_empty() && saw_ok_summary && !saw_check_rewrite {
        return "Ruff format: All files formatted correctly".to_string();
    }

    let mut result = String::new();

    if saw_check_rewrite {
        // Check mode: show files that need formatting
        if files_to_format.is_empty() {
            result.push_str("Ruff format: files need formatting\n");
            result.push_str("\n[hint] Run `ruff format` to format these files\n");
        } else {
            result.push_str(&format!(
                "Ruff format: {} files need formatting\n",
                files_to_format.len()
            ));

            const MAX_RUFF_FORMAT_FILES: usize = CAP_WARNINGS;
            for (i, file) in files_to_format
                .iter()
                .take(MAX_RUFF_FORMAT_FILES)
                .enumerate()
            {
                result.push_str(&format!("{}. {}\n", i + 1, compact_path(file)));
            }

            if files_to_format.len() > MAX_RUFF_FORMAT_FILES {
                result.push_str(&format!(
                    "\n... +{} more files\n",
                    files_to_format.len() - MAX_RUFF_FORMAT_FILES
                ));
            }

            if files_checked > 0 {
                result.push_str(&format!("\n{} files already formatted\n", files_checked));
            }

            result.push_str("\n[hint] Run `ruff format` to format these files\n");
        }
    } else {
        // Write mode or other output - show summary
        result.push_str(output.trim());
    }

    result.trim().to_string()
}

/// Split after the first colon so `unformatted: C:\foo\bar.py` keeps the drive letter.
fn path_after_first_colon(line: &str) -> Option<String> {
    let rest = line.split_once(':')?.1.trim();
    looks_like_source_path(rest).then(|| rest.to_string())
}

fn looks_like_source_path(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let has_sep = s.contains('/') || s.contains('\\');
    if s.contains(' ') && !has_sep {
        return false;
    }
    has_sep || s.contains('.')
}

/// ` --> src/main.py:1:1` from ruff's default `full` format --check output.
fn path_from_arrow_location(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("-->")?.trim();
    strip_trailing_line_col(rest).map(str::to_string)
}

/// `src/main.py:1:1: unformatted: File would be reformatted`
fn path_from_concise_unformatted(line: &str, lower: &str) -> Option<String> {
    let idx = lower.find(": unformatted:")?;
    strip_trailing_line_col(line[..idx].trim()).map(str::to_string)
}

fn strip_trailing_line_col(s: &str) -> Option<&str> {
    let mut parts = s.rsplitn(3, ':');
    let col = parts.next()?;
    let row = parts.next()?;
    let path = parts.next()?;
    if !row.is_empty()
        && !col.is_empty()
        && row.chars().all(|c| c.is_ascii_digit())
        && col.chars().all(|c| c.is_ascii_digit())
    {
        Some(path)
    } else {
        None
    }
}

fn parse_already_ok_count(trimmed: &str, lower: &str) -> Option<usize> {
    if !lower.contains("left unchanged") && !lower.contains("already formatted") {
        return None;
    }
    for part in trimmed.split(',') {
        let part_lower = part.to_lowercase();
        if !part_lower.contains("left unchanged") && !part_lower.contains("already formatted") {
            continue;
        }
        let words: Vec<&str> = part.split_whitespace().collect();
        for (i, word) in words.iter().enumerate() {
            if (*word == "file" || *word == "files") && i > 0 {
                if let Ok(count) = words[i - 1].parse::<usize>() {
                    return Some(count);
                }
            }
        }
    }
    None
}

fn push_unique_file(files: &mut Vec<String>, filename: String) {
    if !files.iter().any(|existing| existing == &filename) {
        files.push(filename);
    }
}

/// Compact file path (remove common prefixes)
fn compact_path(path: &str) -> String {
    let path = path.replace('\\', "/");

    if let Some(pos) = path.rfind("/src/") {
        format!("src/{}", &path[pos + 5..])
    } else if let Some(pos) = path.rfind("/lib/") {
        format!("lib/{}", &path[pos + 5..])
    } else if let Some(pos) = path.rfind("/tests/") {
        format!("tests/{}", &path[pos + 7..])
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
    fn known_ruff_subcommands_do_not_route_through_check() {
        for subcommand in RUFF_SUBCOMMANDS {
            let args = vec![subcommand.to_string()];
            assert_eq!(
                is_check_invocation(&args),
                *subcommand == "check",
                "unexpected routing for ruff {subcommand}"
            );
        }
    }

    /// `known_ruff_subcommands_do_not_route_through_check` iterates the constant, so it
    /// cannot catch an entry going missing. This pins the list against `ruff --help`.
    #[test]
    fn ruff_subcommands_cover_the_ruff_cli() {
        assert_eq!(
            RUFF_SUBCOMMANDS,
            [
                "analyze",
                "check",
                "clean",
                "config",
                "format",
                "generate-shell-completion",
                "help",
                "linter",
                "rule",
                "server",
                "version",
            ]
        );
    }

    #[test]
    fn paths_and_explicit_check_route_through_check() {
        assert!(is_check_invocation(&[]));
        assert!(is_check_invocation(&["check".to_string(), ".".to_string()]));
        assert!(is_check_invocation(&["src".to_string()]));
        assert!(is_check_invocation(&["pyproject.toml".to_string()]));
    }

    #[test]
    fn top_level_flags_are_not_misclassified_as_paths() {
        assert!(!is_check_invocation(&["--version".to_string()]));
        assert!(!is_check_invocation(&["--help".to_string()]));
    }

    #[test]
    fn test_filter_ruff_check_no_issues() {
        let output = "[]";
        let result = filter_ruff_check_json(output);
        assert!(result.contains("Ruff"));
        assert!(result.contains("No issues found"));
    }

    #[test]
    fn test_filter_ruff_check_with_issues() {
        let output = r#"[
  {
    "code": "F401",
    "message": "`os` imported but unused",
    "location": {"row": 1, "column": 8},
    "end_location": {"row": 1, "column": 10},
    "filename": "src/main.py",
    "fix": {"applicability": "safe"}
  },
  {
    "code": "F401",
    "message": "`sys` imported but unused",
    "location": {"row": 2, "column": 8},
    "end_location": {"row": 2, "column": 11},
    "filename": "src/main.py",
    "fix": null
  },
  {
    "code": "E501",
    "message": "Line too long (100 > 88 characters)",
    "location": {"row": 10, "column": 89},
    "end_location": {"row": 10, "column": 100},
    "filename": "src/utils.py",
    "fix": null
  }
]"#;
        let result = filter_ruff_check_json(output);
        assert!(result.contains("3 issues"));
        assert!(result.contains("2 files"));
        assert!(result.contains("1 fixable"));
        assert!(result.contains("F401"));
        assert!(result.contains("E501"));
        assert!(result.contains("main.py"));
        assert!(result.contains("utils.py"));
        assert!(result.contains("Violations:"), "Violations section missing");
        assert!(result.contains("1:8"), "line:col location missing");
    }

    #[test]
    fn test_filter_ruff_format_all_formatted() {
        let output = "5 files left unchanged";
        let result = filter_ruff_format(output);
        assert!(result.contains("Ruff format"));
        assert!(result.contains("All files formatted correctly"));
    }

    #[test]
    fn test_filter_ruff_format_needs_formatting() {
        let output = r#"Would reformat: src/main.py
Would reformat: tests/test_utils.py
2 files would be reformatted, 3 files left unchanged"#;
        let result = filter_ruff_format(output);
        assert!(result.contains("2 files need formatting"));
        assert!(result.contains("main.py"));
        assert!(result.contains("test_utils.py"));
        assert!(result.contains("3 files already formatted"));
    }

    /// ruff ≥0.12 `format --check` when every file is already clean.
    #[test]
    fn test_filter_ruff_format_all_formatted_current_check() {
        let output = "5 files already formatted";
        let result = filter_ruff_format(output);
        assert!(result.contains("Ruff format"));
        assert!(result.contains("All files formatted correctly"));
    }

    /// ruff ≥0.12 per-file line is `unformatted: <path>` (split after first colon).
    #[test]
    fn test_filter_ruff_format_needs_formatting_current_check_paths() {
        let output = r#"unformatted: src/main.py
unformatted: tests/test_utils.py
unformatted: C:\Users\foo\project\src\legacy.py
3 files would be reformatted, 3 files already formatted"#;
        let result = filter_ruff_format(output);
        assert!(result.contains("3 files need formatting"));
        assert!(result.contains("main.py"));
        assert!(result.contains("test_utils.py"));
        assert!(result.contains("legacy.py"), "split after first colon must keep the Windows path");
        assert!(result.contains("3 files already formatted"));
        assert!(result.contains("[hint] Run `ruff format`"));
    }

    /// Default ruff 0.16.x `format --check` (full diagnostic + summary).
    /// This is the wording that currently falls through to identity (0% savings).
    #[test]
    fn test_filter_ruff_format_needs_formatting_current_check_full() {
        let output = r#"unformatted: File would be reformatted
 --> src/main.py:1:1
  |
1 | import   os
1 + import os
  |

unformatted: File would be reformatted
 --> tests/test_utils.py:3:1
  |
3 | def  bad( x ):
3 + def bad(x):
  |

2 files would be reformatted, 3 files already formatted"#;
        let result = filter_ruff_format(output);
        assert!(
            result.contains("2 files need formatting"),
            "current ruff check output should compress, got:\n{result}"
        );
        assert!(result.contains("main.py"));
        assert!(result.contains("test_utils.py"));
        assert!(!result.contains("import   os"), "must drop the diagnostic diff");
        assert!(result.contains("3 files already formatted"));

        let raw_tokens = output.split_whitespace().count();
        let out_tokens = result.split_whitespace().count();
        let savings = 100.0 - (out_tokens as f64 / raw_tokens as f64) * 100.0;
        assert!(
            savings >= 20.0,
            "current ruff check path must compress: {savings:.1}%"
        );
    }

    /// ruff ≥0.12 `--output-format concise`.
    #[test]
    fn test_filter_ruff_format_needs_formatting_current_check_concise() {
        let output = r#"src/main.py:1:1: unformatted: File would be reformatted
tests/test_utils.py:3:1: unformatted: File would be reformatted
2 files would be reformatted, 3 files already formatted"#;
        let result = filter_ruff_format(output);
        assert!(result.contains("2 files need formatting"));
        assert!(result.contains("main.py"));
        assert!(result.contains("test_utils.py"));
        assert!(result.contains("3 files already formatted"));
    }

    #[test]
    fn test_filter_ruff_check_caps_violations_and_emits_hint() {
        // Mirror ruff's pretty-printed JSON shape so the input-vs-output
        // comparison reflects what a real `ruff check --output-format=json` emits.
        let mut diags = Vec::new();
        for i in 0..200 {
            diags.push(format!(
                "  {{\n    \"code\": \"F401\",\n    \"message\": \"`module_{i}` imported but unused\",\n    \"location\": {{\"row\": {i}, \"column\": 4}},\n    \"end_location\": {{\"row\": {i}, \"column\": 20}},\n    \"filename\": \"/Users/dev/project/src/feature_{i}.py\",\n    \"fix\": null\n  }}"
            ));
        }
        let json = format!("[\n{}\n]", diags.join(",\n"));
        let result = filter_ruff_check_json(&json);

        let in_section = result.split("Violations:").nth(1).unwrap_or("");
        let listed = in_section
            .lines()
            .filter(|l| l.trim().starts_with("src/"))
            .count();
        assert!(listed <= 50, "violations cap not enforced: got {listed}");
        assert!(
            result.contains("… +150 more"),
            "missing '+N more' indicator"
        );

        let raw_tokens = json.split_whitespace().count();
        let out_tokens = result.split_whitespace().count();
        let savings = 100.0 - (out_tokens as f64 / raw_tokens as f64) * 100.0;
        assert!(
            savings >= 60.0,
            "token savings dropped below 60%: {savings:.1}%"
        );
    }

    #[test]
    fn test_compact_path() {
        assert_eq!(
            compact_path("/Users/foo/project/src/main.py"),
            "src/main.py"
        );
        assert_eq!(compact_path("/home/user/app/lib/utils.py"), "lib/utils.py");
        assert_eq!(
            compact_path("C:\\Users\\foo\\project\\tests\\test.py"),
            "tests/test.py"
        );
        assert_eq!(compact_path("relative/file.py"), "file.py");
    }
}
