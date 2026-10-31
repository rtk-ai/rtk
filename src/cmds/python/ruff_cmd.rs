//! Filters Ruff linter and formatter output.

use crate::core::args_utils;
use crate::core::config;
use crate::core::runner;
use crate::core::shell::display_args;
use crate::core::truncate::CAP_WARNINGS;
use crate::core::utils::{resolved_command, truncate};
use anyhow::Result;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

/// A tool with a list like this also belongs in `core::tracking::SUBCOMMAND_ROUTERS`,
/// which keeps the subcommand in its telemetry label.
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
    let args = &args_utils::restore_double_dash(args);
    let is_check = is_check_invocation(args);

    let is_format = args.iter().any(|a| a == "format");
    let is_analyze_graph = is_analyze_graph_invocation(args);

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
    // `[]` is a clean run only when rtk chose the format; a user who asked for JSON gets `[]` back.
    let clean_outputs: &[&str] = if is_check && !user_set_output_format {
        &["[]"]
    } else {
        &[]
    };

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
        &display_args(args),
        move |stdout| {
            if is_check && use_json_filter && !stdout.trim().is_empty() {
                filter_ruff_check_json(stdout)
            } else if is_format {
                filter_ruff_format(stdout)
            } else {
                filter_passthrough_output(stdout, is_analyze_graph)
            }
        },
        runner::RunOptions::stdout_only().clean_outputs(clean_outputs),
    )
}

fn is_analyze_graph_invocation(args: &[String]) -> bool {
    matches!(args, [subcommand, action, ..] if subcommand == "analyze" && action == "graph")
}

fn filter_passthrough_output(stdout: &str, is_analyze_graph: bool) -> String {
    if is_analyze_graph {
        stdout.trim().to_string()
    } else {
        truncate(stdout.trim(), config::limits().passthrough_max_chars)
    }
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
    let mut by_rule: HashMap<&str, (usize, Option<&str>)> = HashMap::new();
    for diag in &diagnostics {
        let message = diag.message.trim();
        let entry = by_rule.entry(&diag.code).or_insert((0, Some(message)));
        entry.0 += 1;
        // F401, for example, includes the imported name. Never replace
        // different messages with the first message seen for that code.
        if entry.1 != Some(message) {
            entry.1 = None;
        }
    }

    // Group by file
    let mut by_file: HashMap<&str, usize> = HashMap::new();
    for diag in &diagnostics {
        *by_file.entry(&diag.filename).or_insert(0) += 1;
    }

    let mut file_counts: Vec<_> = by_file.iter().collect();
    file_counts.sort_by_key(|(file, count)| (std::cmp::Reverse(**count), **file));

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
    let mut rule_counts: Vec<_> = by_rule
        .iter()
        .map(|(&code, &(count, message))| (code, count, message))
        .collect();
    rule_counts.sort_by_key(|&(code, count, _)| (std::cmp::Reverse(count), code));

    const MAX_RUFF_RULES: usize = CAP_WARNINGS;
    const MAX_RUFF_FILES: usize = CAP_WARNINGS;
    let mut described_rules = HashSet::new();
    if !rule_counts.is_empty() {
        result.push_str("Top rules:\n");
        for (rule, count, message) in rule_counts.iter().take(MAX_RUFF_RULES) {
            result.push_str(&format!("  {} ({}x)", rule, count));
            if let Some(message) = message.filter(|s| !s.is_empty()) {
                result.push_str(&format!(": {}", truncate(message, 100)));
                described_rules.insert(*rule);
            }
            result.push('\n');
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
        file_rule_counts.sort_by_key(|(rule, count)| (std::cmp::Reverse(**count), *rule));

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
    for (diag, line) in diagnostics
        .iter()
        .zip(&violation_lines)
        .take(MAX_VIOLATIONS)
    {
        if described_rules.contains(diag.code.as_str()) {
            result.push_str(&format!(
                "  {}:{}:{} {}\n",
                compact_path(&diag.filename),
                diag.location.row,
                diag.location.column,
                diag.code,
            ));
        } else {
            result.push_str(line);
        }
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
    let mut files_to_format: Vec<String> = Vec::new();
    let mut files_checked = 0;

    for line in output.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_lowercase();

        // Count "would reformat" lines (check mode) - case insensitive
        if lower.contains("would reformat:") {
            // Extract filename from "Would reformat: path/to/file.py"
            if let Some(filename) = trimmed.split(':').nth(1) {
                files_to_format.push(filename.trim().to_string());
            }
        }

        // Count total checked files - look for patterns like "3 files left unchanged"
        if lower.contains("left unchanged") {
            // Find "X file(s) left unchanged" pattern specifically
            // Split by comma to handle "2 files would be reformatted, 3 files left unchanged"
            let parts: Vec<&str> = trimmed.split(',').collect();
            for part in parts {
                let part_lower = part.to_lowercase();
                if part_lower.contains("left unchanged") {
                    let words: Vec<&str> = part.split_whitespace().collect();
                    // Look for number before "file" or "files"
                    for (i, word) in words.iter().enumerate() {
                        if (word == &"file" || word == &"files")
                            && i > 0
                            && let Ok(count) = words[i - 1].parse::<usize>()
                        {
                            files_checked = count;
                            break;
                        }
                    }
                    break;
                }
            }
        }
    }

    let output_lower = output.to_lowercase();

    // Check if all files are formatted
    if files_to_format.is_empty() && output_lower.contains("left unchanged") {
        return "Ruff format: All files formatted correctly".to_string();
    }

    let mut result = String::new();

    if output_lower.contains("would reformat") {
        // Check mode: show files that need formatting
        if files_to_format.is_empty() {
            result.push_str("Ruff format: All files formatted correctly\n");
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
    fn analyze_graph_preserves_large_json_output() {
        let raw = include_str!("../../../tests/fixtures/ruff_analyze_graph_raw.json");
        assert!(raw.len() > config::limits().passthrough_max_chars);

        let args = ["analyze".to_string(), "graph".to_string()];
        let result = filter_passthrough_output(raw, is_analyze_graph_invocation(&args));

        assert_eq!(result, raw.trim());
        serde_json::from_str::<serde_json::Value>(&result)
            .expect("analyze graph output must remain valid JSON");
    }

    /// Only invocations starting `analyze graph` are exempt from the passthrough cap:
    /// `analyze help graph` and every other passthrough subcommand still get it.
    #[test]
    fn only_analyze_graph_skips_the_passthrough_cap() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(is_analyze_graph_invocation(&args(&["analyze", "graph"])));
        assert!(is_analyze_graph_invocation(&args(&[
            "analyze",
            "graph",
            "--direction",
            "dependents",
            "src"
        ])));
        assert!(!is_analyze_graph_invocation(&args(&[
            "analyze", "help", "graph"
        ])));
        assert!(!is_analyze_graph_invocation(&args(&["rule", "E501"])));

        let raw = include_str!("../../../tests/fixtures/ruff_rule_e501_raw.txt");
        assert!(raw.len() > config::limits().passthrough_max_chars);
        let result =
            filter_passthrough_output(raw, is_analyze_graph_invocation(&args(&["rule", "E501"])));
        assert!(
            result.ends_with("...") && raw.starts_with(result.trim_end_matches("...")),
            "rule output must be a capped prefix of ruff's, got {} of {} bytes",
            result.len(),
            raw.len()
        );
    }

    #[test]
    fn test_filter_ruff_check_no_issues() {
        let output = "[]";
        let result = filter_ruff_check_json(output);
        assert!(result.contains("Ruff"));
        assert!(result.contains("No issues found"));
    }

    #[test]
    fn repeated_descriptions_are_listed_once_in_visible_top_rules() {
        let input = r#"[
            {"code":"F401","message":"`os` imported but unused","location":{"row":1,"column":8},"filename":"/project/src/a.py","fix":null},
            {"code":"F401","message":"`os` imported but unused","location":{"row":2,"column":8},"filename":"/project/src/b.py","fix":null}
        ]"#;
        let out = filter_ruff_check_json(input);
        let (summary, detail) = out.split_once("Violations:\n").expect("detail section");
        assert!(
            summary.contains("F401 (2x): `os` imported but unused"),
            "{out}"
        );
        assert_eq!(out.matches("`os` imported but unused").count(), 1, "{out}");
        assert!(detail.contains("src/a.py:1:8 F401"));
        assert!(detail.contains("src/b.py:2:8 F401"));
    }

    #[test]
    fn distinct_messages_for_one_rule_stay_attached_to_their_locations() {
        let input = r#"[
            {"code":"F401","message":"`os` imported but unused","location":{"row":1,"column":8},"filename":"/project/src/a.py","fix":null},
            {"code":"F401","message":"`sys` imported but unused","location":{"row":2,"column":8},"filename":"/project/src/a.py","fix":null}
        ]"#;
        let out = filter_ruff_check_json(input);
        assert!(out.contains("src/a.py:1:8 F401 `os` imported but unused"));
        assert!(out.contains("src/a.py:2:8 F401 `sys` imported but unused"));
    }

    #[test]
    fn descriptions_for_rules_outside_the_summary_cap_stay_in_details() {
        let diagnostics: Vec<_> = (0..=CAP_WARNINGS)
            .map(|i| {
                serde_json::json!({
                    "code": format!("RULE{i:02}"), "message": format!("Distinct diagnostic {i}"),
                    "location": {"row": i + 1, "column": 1}, "filename": "src/a.py", "fix": null,
                })
            })
            .collect();
        let out =
            filter_ruff_check_json(&serde_json::to_string(&diagnostics).expect("fixture JSON"));
        let detail = out.split_once("Violations:\n").expect("detail section").1;
        assert!(detail.contains("RULE10 Distinct diagnostic 10"), "{out}");
    }

    #[test]
    fn real_repeated_diagnostics_keep_locations_and_reduce_output() {
        // Captured from Ruff 0.16.6:
        // `ruff check --isolated --select F401,E701 --output-format=json src/four.py`
        // where four.py imports os and has two one-line if statements. Only
        // the machine-specific filename prefix was normalized to /fixture.
        let input = include_str!("../../../tests/fixtures/ruff_check_repeated_raw.json");
        let out = filter_ruff_check_json(input);
        assert_eq!(
            out.matches("Multiple statements on one line (colon)")
                .count(),
            1
        );
        assert_eq!(out.matches("`os` imported but unused").count(), 1);
        for location in [
            "src/four.py:1:8 F401",
            "src/four.py:3:8 E701",
            "src/four.py:4:9 E701",
        ] {
            assert!(out.contains(location), "{out}");
        }
        assert!(out.contains("3 issues in 1 files (1 fixable)"), "{out}");
        assert!(
            out.len() * 100 <= input.len() * 80,
            "must save at least 20%"
        );
        for _ in 0..10 {
            assert_eq!(filter_ruff_check_json(input), out);
        }
    }

    #[test]
    fn tied_files_and_rules_are_listed_by_name() {
        // Captured from Ruff 0.16.6:
        // `ruff check --isolated --select ALL --output-format=json src` over
        // alpha.py, beta.py and gamma.py, each containing `import os, sys`, so
        // every file and every single-occurrence rule ties on its count. Only
        // the machine-specific filename prefix was normalized to /fixture.
        let input = include_str!("../../../tests/fixtures/ruff_check_tied_raw.json");
        let expected_rules = "Top rules:\n  F401 (6x)\n  CPY001 (3x): Missing copyright notice at top of file\n  D100 (3x): Missing docstring in public module\n  E401 (3x): Multiple imports on one line\n  I001 (3x): Import block is un-sorted or un-formatted\n";
        let expected_files = "Top files:\n\
            \x20 src/alpha.py (6 issues)\n    F401 (2)\n    CPY001 (1)\n    D100 (1)\n\
            \x20 src/beta.py (6 issues)\n    F401 (2)\n    CPY001 (1)\n    D100 (1)\n\
            \x20 src/gamma.py (6 issues)\n    F401 (2)\n    CPY001 (1)\n    D100 (1)\n";
        // Each run hashes its maps differently; a count-only sort would pass
        // one run by luck but not ten.
        for _ in 0..10 {
            let out = filter_ruff_check_json(input);
            assert!(out.contains(expected_rules), "{out}");
            assert!(out.contains(expected_files), "{out}");
        }
    }

    #[test]
    fn long_shared_descriptions_are_truncated_in_top_rules() {
        // Captured from Ruff 0.16.6:
        // `ruff check --isolated --select F401 --output-format=json src` over two
        // modules importing the same long module name. Only the machine-specific
        // filename prefix was normalized to /fixture.
        let input = include_str!("../../../tests/fixtures/ruff_check_long_message_raw.json");
        let out = filter_ruff_check_json(input);
        assert!(
            out.contains("  F401 (2x): `a_really_long_package_name_for_testing.with_a_long_submodule_name.and_yet_another_module` import...\n"),
            "{out}"
        );
        assert!(!out.contains("imported but unused"), "{out}");
        assert!(out.contains("  src/pkg_a/mod.py:1:8 F401\n"), "{out}");
        assert!(out.contains("  src/pkg_b/mod.py:1:8 F401\n"), "{out}");
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
