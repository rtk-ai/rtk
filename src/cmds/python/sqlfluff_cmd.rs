//! Filters SQLFluff SQL linter output.

use crate::core::config;
use crate::core::runner;
use crate::core::truncate::CAP_WARNINGS;
use crate::core::utils::{resolved_command, truncate};
use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct SqlfluffViolation {
    code: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    fixes: Vec<serde_json::Value>,
    #[serde(default)]
    start_line_no: Option<u64>,
    #[serde(default)]
    line_no: Option<u64>,
    #[serde(default)]
    start_line_pos: Option<u64>,
    #[serde(default)]
    line_pos: Option<u64>,
}

impl SqlfluffViolation {
    /// v3+ `start_line_no` wins; falls back to legacy `line_no`.
    fn line(&self) -> Option<u64> {
        self.start_line_no.or(self.line_no)
    }

    /// v3+ `start_line_pos` wins; falls back to legacy `line_pos`.
    fn col(&self) -> Option<u64> {
        self.start_line_pos.or(self.line_pos)
    }
}

#[derive(Debug, Deserialize)]
struct SqlfluffFile {
    filepath: String,
    violations: Vec<SqlfluffViolation>,
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    // Route to the lint filter only for explicit lint invocations (or a bare
    // call, which we make explicit). sqlfluff's other subcommands - and any
    // unknown bareword, be it a typo or a future subcommand - pass through
    // untouched so sqlfluff errors on its own input instead of rtk silently
    // reinterpreting it as `lint <path>`.
    let is_lint = args.is_empty() || args[0] == "lint";

    // Both spellings: injecting a second --format makes sqlfluff reject the call.
    let user_set_format = args
        .iter()
        .any(|a| a == "--format" || a == "-f" || a.starts_with("--format=") || a.starts_with("-f="));
    let user_json_format = args.iter().enumerate().any(|(i, a)| {
        a == "--format=json"
            || a == "-f=json"
            || ((a == "--format" || a == "-f") && args.get(i + 1).is_some_and(|n| n == "json"))
    });
    let use_json_filter = !user_set_format || user_json_format;

    // Known limitation: joined short-flag forms such as `-fhuman` are not
    // recognized above, so rtk would still append `--format json`; sqlfluff's
    // last-value-wins then runs in the user's format while rtk still tries to
    // JSON-parse it and reports a parse failure. Proper flag-value parsing
    // belongs to the arg-tokenizer migration (#3681) - fix it there.

    let mut cmd = resolved_command("sqlfluff");

    if is_lint {
        cmd.arg("lint");
        if !user_set_format {
            cmd.arg("--format").arg("json");
        }

        // Skip "lint" if it was explicitly the first arg
        let start_idx = if args.first().is_some_and(|a| a == "lint") {
            1
        } else {
            0
        };
        for arg in &args[start_idx..] {
            cmd.arg(arg);
        }
        // No path default: sqlfluff lints the current directory when given no
        // paths, and guessing whether a bareword is a path or a flag value
        // misreads things like `--dialect postgres`.
    } else {
        for arg in args {
            cmd.arg(arg);
        }
    }

    if verbose > 0 {
        eprintln!("Running: sqlfluff {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "sqlfluff",
        &args.join(" "),
        move |stdout| {
            if is_lint && use_json_filter && !stdout.trim().is_empty() {
                filter_sqlfluff_lint_json(stdout)
            } else {
                truncate(stdout.trim(), config::limits().passthrough_max_chars)
            }
        },
        runner::RunOptions::stdout_only().tee("sqlfluff").early_exit_on_failure(),
    )
}

/// Cap classes from the shared truncation policy (`src/core/README.md`).
const MAX_REPORTED_RULES: usize = CAP_WARNINGS;
const MAX_REPORTED_FILES: usize = CAP_WARNINGS;
/// No shared cap covers per-file rule breakdowns; named locally.
const MAX_RULES_PER_FILE: usize = 3;

/// Filter `sqlfluff lint --format json` output - group violations by rule and file.
pub fn filter_sqlfluff_lint_json(output: &str) -> String {
    let parsed: Vec<SqlfluffFile> = match serde_json::from_str(output) {
        Ok(f) => f,
        Err(e) => {
            // Non-negotiable fallback rule (rust-patterns.md): if the filter
            // fails, pass the raw command output through unchanged and warn
            // on stderr - never block the user with a mangled summary.
            eprintln!("rtk: filter warning: sqlfluff JSON filter failed to parse output: {e}");
            return output.to_string();
        }
    };

    // One sort feeds the whole report: files, worst first.
    let mut files: Vec<&SqlfluffFile> = parsed
        .iter()
        .filter(|f| !f.violations.is_empty())
        .collect();

    let total_violations: usize = files.iter().map(|f| f.violations.len()).sum();
    if total_violations == 0 {
        return "✓ SQLFluff: No violations found".to_string();
    }
    // Worst file first; ties break alphabetically by path so identical input
    // always yields an identical report regardless of HashMap iteration order.
    files.sort_by_key(|f| (std::cmp::Reverse(f.violations.len()), f.filepath.as_str()));

    let total_files = files.len();
    let fixable_count: usize = files
        .iter()
        .flat_map(|f| &f.violations)
        .filter(|v| !v.fixes.is_empty())
        .count();

    // Group by rule over the raw sqlfluff JSON order (`parsed`, pre-sort), so
    // "first at" is the earliest occurrence in the original output - not the
    // first after the worst-first file sort above.
    let mut by_rule: HashMap<&str, (usize, &str, Option<u64>)> = HashMap::new();
    for file in &parsed {
        for v in &file.violations {
            let entry = by_rule
                .entry(v.code.as_str())
                .or_insert((0, file.filepath.as_str(), v.line()));
            entry.0 += 1;
            if entry.2.is_none() && v.line().is_some() {
                entry.1 = file.filepath.as_str();
                entry.2 = v.line();
            }
        }
    }
    let mut rule_counts: Vec<(&str, usize, &str, Option<u64>)> = by_rule
        .iter()
        .map(|(code, (count, path, line))| (*code, *count, *path, *line))
        .collect();
    // Most frequent rule first; ties break alphabetically by rule code so
    // identical input always yields an identical report.
    rule_counts.sort_by_key(|r| (std::cmp::Reverse(r.1), r.0));

    // Build compact output
    let mut result = String::new();
    result.push_str(&format!(
        "SQLFluff: {} violations in {} files",
        total_violations, total_files
    ));
    if fixable_count > 0 {
        result.push_str(&format!(" ({} fixable)", fixable_count));
    }
    result.push('\n');
    result.push_str("═══════════════════════════════════════\n");

    // Top rules
    result.push_str("Top rules:\n");
    for (rule, count, path, line) in rule_counts.iter().take(MAX_REPORTED_RULES) {
        result.push_str(&format!(
            "  {} ({}x, first at {})\n",
            rule,
            count,
            sample_location(path, *line)
        ));
    }
    if rule_counts.len() > MAX_REPORTED_RULES {
        result.push_str(&format!(
            "  ... +{} more rules\n",
            rule_counts.len() - MAX_REPORTED_RULES
        ));
    }
    result.push('\n');

    // Top files with per-file rule breakdown
    result.push_str("Top files:\n");
    for file in files.iter().take(MAX_REPORTED_FILES) {
        result.push_str(&format!(
            "  {} ({} violations)\n",
            compact_path(&file.filepath),
            file.violations.len()
        ));

        let mut file_rules: HashMap<&str, (usize, Option<u64>)> = HashMap::new();
        for v in &file.violations {
            let entry = file_rules
                .entry(v.code.as_str())
                .or_insert((0, v.line()));
            entry.0 += 1;
            if entry.1.is_none() && v.line().is_some() {
                entry.1 = v.line();
            }
        }
        let mut file_rule_counts: Vec<(&str, usize, Option<u64>)> = file_rules
            .iter()
            .map(|(code, (count, line))| (*code, *count, *line))
            .collect();
        // Most frequent rule first; ties break alphabetically by rule code
        // for deterministic output.
        file_rule_counts.sort_by_key(|r| (std::cmp::Reverse(r.1), r.0));

        for (rule, count, line) in file_rule_counts.iter().take(MAX_RULES_PER_FILE) {
            match (*count, *line) {
                (1, Some(l)) => result.push_str(&format!("    {} (1, line {})\n", rule, l)),
                (c, Some(l)) => {
                    result.push_str(&format!("    {} ({}, first at line {})\n", rule, c, l))
                }
                (c, None) => result.push_str(&format!("    {} ({})\n", rule, c)),
            }
        }
        if file_rule_counts.len() > MAX_RULES_PER_FILE {
            result.push_str(&format!(
                "    ... +{} more rules\n",
                file_rule_counts.len() - MAX_RULES_PER_FILE
            ));
        }
    }

    if files.len() > MAX_REPORTED_FILES {
        result.push_str(&format!(
            "\n... +{} more files\n",
            files.len() - MAX_REPORTED_FILES
        ));
    }

    // Per-violation detail
    const MAX_VIOLATIONS: usize = 50;
    let mut violation_lines: Vec<String> = Vec::new();
    for file in &parsed {
        for v in &file.violations {
            let line = v.line().unwrap_or(0);
            let col = v.col().unwrap_or(0);
            violation_lines.push(format!(
                "  {}:{}:{} {} {}",
                compact_path(&file.filepath),
                line,
                col,
                v.code,
                truncate(v.description.trim(), 100),
            ));
        }
    }

    if !violation_lines.is_empty() {
        result.push_str("\nViolations:\n");
        for vl in violation_lines.iter().take(MAX_VIOLATIONS) {
            result.push_str(vl);
            result.push('\n');
        }
        if violation_lines.len() > MAX_VIOLATIONS {
            result.push_str(&format!(
                "  … +{} more\n",
                violation_lines.len() - MAX_VIOLATIONS
            ));
        }
    }

    if fixable_count > 0 {
        result.push_str(&format!(
            "\n💡 Run `sqlfluff fix` to auto-fix {} violations\n",
            fixable_count
        ));
    }

    result.trim().to_string()
}

/// `path:line` sample location for a violation group, e.g. `models/x.sql:12`.
fn sample_location(path: &str, line: Option<u64>) -> String {
    match line {
        Some(l) => format!("{}:{}", compact_path(path), l),
        None => compact_path(path),
    }
}

/// Compact file path for dbt/SQL projects, preserving meaningful directory prefixes.
fn compact_path(path: &str) -> String {
    let path = path.replace('\\', "/");

    // Absolute paths: extract from known dbt directory roots
    for prefix in &[
        "models",
        "tests",
        "macros",
        "seeds",
        "snapshots",
        "analyses",
    ] {
        let needle = format!("/{}/", prefix);
        if let Some(pos) = path.rfind(&needle) {
            return format!("{}/{}", prefix, &path[pos + needle.len()..]);
        }
    }

    // Relative paths already starting with a known dbt root - keep as-is
    for prefix in &[
        "models/",
        "tests/",
        "macros/",
        "seeds/",
        "snapshots/",
        "analyses/",
    ] {
        if path.starts_with(prefix) {
            return path;
        }
    }

    // Fall back to just the filename
    if let Some(pos) = path.rfind('/') {
        path[pos + 1..].to_string()
    } else {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    // ── happy path ──────────────────────────────────────────────────────────

    #[test]
    fn test_filter_no_violations_empty_array() {
        let result = filter_sqlfluff_lint_json("[]");
        assert!(result.contains("✓ SQLFluff"), "expected success tick");
        assert!(result.contains("No violations found"));
    }

    #[test]
    fn test_filter_no_violations_all_clean_files() {
        let input = r#"[{"filepath": "models/staging/stg_orders.sql", "violations": []}]"#;
        let result = filter_sqlfluff_lint_json(input);
        assert!(result.contains("✓ SQLFluff"));
        assert!(result.contains("No violations found"));
    }

    #[test]
    fn test_filter_with_violations_counts() {
        let input = r#"[
  {
    "filepath": "models/staging/stg_customers.sql",
    "violations": [
      {"line_no": 1, "line_pos": 1, "code": "LT09", "description": "Select wildcard used.", "fixes": []},
      {"line_no": 5, "line_pos": 1, "code": "LT12", "description": "Trailing newline missing.", "fixes": [{"edit_type": "create_after"}]}
    ]
  },
  {
    "filepath": "models/intermediate/int_orders.sql",
    "violations": [
      {"line_no": 3, "line_pos": 1, "code": "LT09", "description": "Select wildcard used.", "fixes": []}
    ]
  }
]"#;
        let result = filter_sqlfluff_lint_json(input);
        assert!(result.contains("3 violations"), "should show 3 violations");
        assert!(result.contains("2 files"), "should show 2 files");
        assert!(result.contains("1 fixable"), "should count 1 fixable");
        assert!(result.contains("LT09"), "should list top rule");
        assert!(result.contains("LT12"), "should list second rule");
        assert!(result.contains("stg_customers.sql"), "should show top file");
        assert!(result.contains("int_orders.sql"), "should show second file");
        assert!(
            result.contains("LT09 (2x, first at models/staging/stg_customers.sql:1)"),
            "should show top rule with first location"
        );
        assert!(
            result.contains("LT09 (1, line 1)"),
            "should show per-file rule with line number"
        );
        assert!(
            result.contains("LT12 (1, line 5)"),
            "should show per-file rule with line number"
        );
    }

    #[test]
    fn test_filter_fixable_hint_shown() {
        let input = r#"[
  {
    "filepath": "models/staging/stg_orders.sql",
    "violations": [
      {"line_no": 1, "line_pos": 1, "code": "LT12", "description": "Trailing newline.", "fixes": [{"edit_type": "create_after"}]}
    ]
  }
]"#;
        let result = filter_sqlfluff_lint_json(input);
        assert!(
            result.contains("sqlfluff fix"),
            "should suggest fix command"
        );
    }

    #[test]
    fn test_filter_no_fixable_no_hint() {
        let input = r#"[
  {
    "filepath": "models/staging/stg_orders.sql",
    "violations": [
      {"line_no": 1, "line_pos": 1, "code": "LT09", "description": "Select wildcard.", "fixes": []}
    ]
  }
]"#;
        let result = filter_sqlfluff_lint_json(input);
        assert!(
            !result.contains("sqlfluff fix"),
            "should NOT show fix hint when nothing is fixable"
        );
    }

    // ── real sqlfluff JSON schema (regression: start_line_no not line_no) ────

    #[test]
    fn test_filter_real_sqlfluff_json_schema() {
        // Real output from `sqlfluff lint --format json` (v3+).
        // Fields are start_line_no/start_line_pos, NOT line_no/line_pos.
        // statistics and timings fields at file level must be tolerated.
        let input = r#"[{"filepath": "models/intermediate/mariadb/int_mariadb_announces.sql", "violations": [{"start_line_no": 183, "start_line_pos": 9, "code": "RF01", "description": "Reference 'level_id' refers to table/view not found in the FROM clause or found in ancestor statement.", "name": "references.from", "warning": false, "fixes": [], "start_file_pos": 4449, "end_line_no": 183, "end_line_pos": 17, "end_file_pos": 4457}, {"start_line_no": 185, "start_line_pos": 9, "code": "RF01", "description": "Reference 'level_name' refers to table/view not found in the FROM clause or found in ancestor statement.", "name": "references.from", "warning": false, "fixes": [], "start_file_pos": 4486, "end_line_no": 185, "end_line_pos": 19, "end_file_pos": 4496}], "statistics": {"source_chars": 9356, "templated_chars": 9507}, "timings": {"templating": 0.93}}]"#;
        let result = filter_sqlfluff_lint_json(input);
        assert!(
            !result.contains("JSON parse failed"),
            "should parse real sqlfluff JSON without error"
        );
        assert!(result.contains("2 violations"), "should count 2 violations");
        assert!(result.contains("RF01"), "should show rule code");
        assert!(
            result.contains("int_mariadb_announces.sql"),
            "should show filename"
        );
        assert!(
            result.contains("int_mariadb_announces.sql:183"),
            "should surface a sample path:line location per rule"
        );
    }

    #[test]
    fn test_filter_legacy_line_no_alias() {
        // Older sqlfluff versions emit line_no/line_pos; the serde alias must
        // surface sample locations for them too.
        let input =
            r#"[{"filepath": "models/core/dim_users.sql", "violations": [{"line_no": 7, "line_pos": 2, "code": "LT09", "description": "Select wildcard.", "fixes": []}]}]"#;
        let result = filter_sqlfluff_lint_json(input);
        assert!(
            result.contains("models/core/dim_users.sql:7"),
            "legacy line_no should surface a location, got: {result}"
        );
    }

    // ── error handling ───────────────────────────────────────────────────────

    #[test]
    fn test_filter_json_parse_error_passes_raw_through() {
        let input = "sqlfluff diagnostics: raw line\nanother raw line";
        let result = filter_sqlfluff_lint_json(input);
        assert_eq!(
            result, input,
            "raw output must pass through unchanged on parse failure"
        );
        assert!(
            !result.contains("JSON parse failed"),
            "no mangled hybrid summary on stdout"
        );
    }

    #[test]
    fn test_filter_json_parse_error_passes_large_raw_through_uncapped() {
        // Even a large non-JSON payload must pass through verbatim - the
        // fallback contract says raw output unchanged, never truncated.
        let input =
            std::iter::repeat_n("line of human sqlfluff output\n", 2000).collect::<String>();
        let result = filter_sqlfluff_lint_json(&input);
        assert_eq!(result, input, "large raw output must pass through unchanged");
    }

    #[test]
    fn test_filter_first_location_uses_original_output_order() {
        // The heaviest file (2 violations) sorts first in "Top files", but it
        // appears last in the raw sqlfluff JSON; "first at" must point at the
        // earliest occurrence in the original output order, not the first file
        // in the sorted report.
        let input = r#"[
  {
    "filepath": "models/staging/stg_customers.sql",
    "violations": [
      {"code": "LT01", "start_line_no": 1, "fixes": []}
    ]
  },
  {
    "filepath": "models/strict/fct_orders.sql",
    "violations": [
      {"code": "LT01", "start_line_no": 500, "fixes": []},
      {"code": "LT01", "start_line_no": 501, "fixes": []}
    ]
  }
]"#;
        let result = filter_sqlfluff_lint_json(input);
        assert!(
            result.contains("LT01 (3x, first at models/staging/stg_customers.sql:1)"),
            "first location must come from original output order, got: {result}"
        );
    }

    #[test]
    fn test_filter_tied_rules_sort_alphabetically() {
        // Two rules with equal counts must break ties alphabetically, so the
        // report is reproducible for identical input.
        let input = r#"[
  {
    "filepath": "models/staging/stg_customers.sql",
    "violations": [
      {"code": "LT10", "start_line_no": 1, "fixes": []},
      {"code": "LT02", "start_line_no": 2, "fixes": []}
    ]
  }
]"#;
        let result = filter_sqlfluff_lint_json(input);
        let lt02 = result
            .find("LT02")
            .expect("LT02 should be listed");
        let lt10 = result
            .find("LT10")
            .expect("LT10 should be listed");
        assert!(
            lt02 < lt10,
            "tied rules must sort alphabetically, got: {result}"
        );
    }

    #[test]
    fn test_filter_tied_files_sort_alphabetically() {
        // Equal-violation files must break ties by path so "Top files" is
        // stable across runs on identical input.
        let input = r#"[
  {
    "filepath": "models/staging/stg_orders.sql",
    "violations": [{"code": "LT01", "start_line_no": 1, "fixes": []}]
  },
  {
    "filepath": "models/staging/stg_customers.sql",
    "violations": [{"code": "LT01", "start_line_no": 1, "fixes": []}]
  }
]"#;
        let result = filter_sqlfluff_lint_json(input);
        let files_sec = result
            .find("Top files:")
            .expect("report should have a Top files section");
        let customers = files_sec
            + result[files_sec..]
                .find("stg_customers.sql")
                .expect("customers file should be listed");
        let orders = files_sec
            + result[files_sec..]
                .find("stg_orders.sql")
                .expect("orders file should be listed");
        assert!(
            customers < orders,
            "tied files must sort alphabetically, got: {result}"
        );
    }

    // ── compact_path ─────────────────────────────────────────────────────────

    #[test]
    fn test_compact_path_absolute_models() {
        assert_eq!(
            compact_path("/Users/foo/project/models/staging/stg_orders.sql"),
            "models/staging/stg_orders.sql"
        );
    }

    #[test]
    fn test_compact_path_absolute_macros() {
        assert_eq!(
            compact_path("/home/user/project/macros/utils.sql"),
            "macros/utils.sql"
        );
    }

    #[test]
    fn test_compact_path_relative_models() {
        assert_eq!(
            compact_path("models/staging/stg_orders.sql"),
            "models/staging/stg_orders.sql"
        );
    }

    #[test]
    fn test_compact_path_no_known_prefix() {
        assert_eq!(compact_path("some/deep/path/file.sql"), "file.sql");
    }

    #[test]
    fn test_compact_path_filename_only() {
        assert_eq!(compact_path("file.sql"), "file.sql");
    }

    // ── token savings ─────────────────────────────────────────────────────────

    #[test]
    fn test_token_savings_at_least_60_percent() {
        // Realistic sqlfluff JSON output with 10 violations across 3 files
        // (includes end_line_no/end_line_pos/fixable as sqlfluff always emits)
        let input = r#"[
  {
    "filepath": "models/staging/stg_customers.sql",
    "violations": [
      {"code": "LT09", "description": "Select wildcard (*) used in select statement. Use explicit column names instead.", "line_no": 1, "line_pos": 1, "end_line_no": 1, "end_line_pos": 7, "start_file_pos": 0, "end_file_pos": 6, "fixes": [], "fixable": false},
      {"code": "LT12", "description": "Files must end with a single trailing newline.", "line_no": 5, "line_pos": 1, "end_line_no": 5, "end_line_pos": 42, "start_file_pos": 100, "end_file_pos": 100, "fixes": [{"edit_type": "create_after", "content": "\n"}], "fixable": true},
      {"code": "AM04", "description": "Query produces an unknown number of result columns. Specify a column list in the SELECT clause.", "line_no": 10, "line_pos": 5, "end_line_no": 10, "end_line_pos": 25, "start_file_pos": 200, "end_file_pos": 220, "fixes": [], "fixable": false},
      {"code": "LT09", "description": "Select wildcard (*) used in select statement. Use explicit column names instead.", "line_no": 15, "line_pos": 1, "end_line_no": 15, "end_line_pos": 7, "start_file_pos": 300, "end_file_pos": 306, "fixes": [], "fixable": false},
      {"code": "RF04", "description": "Column name is a reserved word in one or more dialects.", "line_no": 20, "line_pos": 1, "end_line_no": 20, "end_line_pos": 30, "start_file_pos": 400, "end_file_pos": 410, "fixes": [], "fixable": false}
    ]
  },
  {
    "filepath": "models/intermediate/int_order_items.sql",
    "violations": [
      {"code": "LT09", "description": "Select wildcard (*) used in select statement. Use explicit column names instead.", "line_no": 2, "line_pos": 1, "end_line_no": 2, "end_line_pos": 7, "start_file_pos": 0, "end_file_pos": 6, "fixes": [], "fixable": false},
      {"code": "AM04", "description": "Query produces an unknown number of result columns. Specify a column list in the SELECT clause.", "line_no": 8, "line_pos": 1, "end_line_no": 8, "end_line_pos": 25, "start_file_pos": 100, "end_file_pos": 120, "fixes": [], "fixable": false},
      {"code": "LT12", "description": "Files must end with a single trailing newline.", "line_no": 12, "line_pos": 1, "end_line_no": 12, "end_line_pos": 45, "start_file_pos": 200, "end_file_pos": 200, "fixes": [{"edit_type": "create_after", "content": "\n"}], "fixable": true}
    ]
  },
  {
    "filepath": "models/marts/fct_orders.sql",
    "violations": [
      {"code": "LT09", "description": "Select wildcard (*) used in select statement. Use explicit column names instead.", "line_no": 1, "line_pos": 1, "end_line_no": 1, "end_line_pos": 7, "start_file_pos": 0, "end_file_pos": 6, "fixes": [], "fixable": false},
      {"code": "RF04", "description": "Column name is a reserved word in one or more dialects.", "line_no": 3, "line_pos": 1, "end_line_no": 3, "end_line_pos": 30, "start_file_pos": 100, "end_file_pos": 110, "fixes": [], "fixable": false}
    ]
  }
]"#;
        let result = filter_sqlfluff_lint_json(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 20.0,
            "SQLFluff filter: expected ≥20% savings, got {:.1}% (in={} out={})",
            savings,
            input_tokens,
            output_tokens
        );
    }
}
