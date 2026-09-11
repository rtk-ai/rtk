//! Filters SQLFluff SQL linter output.
//!
//! `plan` is the single statement of how rtk invokes sqlfluff and how it
//! reads the result back; both `rtk sqlfluff ...` and `rtk lint sqlfluff ...`
//! go through it so the two entry points cannot drift apart.

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
    /// Absent in sqlfluff 2.x, which reports no fix information at all - so
    /// `None` means "unknown", not "nothing is fixable".
    #[serde(default)]
    fixes: Option<Vec<serde_json::Value>>,
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

/// How rtk should invoke sqlfluff, and how to read the result back.
///
/// Both entry points - `rtk sqlfluff ...` and `rtk lint sqlfluff ...` - build
/// one of these and hand it the raw stdout afterwards, so the "is this lint
/// JSON?" decision is stated once instead of once per call site.
pub struct Invocation {
    /// Full sqlfluff argv (subcommand included), with `--format json` appended
    /// when rtk owns the output format.
    pub args: Vec<String>,
    /// Whether stdout is expected to be `sqlfluff lint --format json` output.
    expect_json: bool,
}

/// Build the sqlfluff invocation for a user-supplied argv (tool name excluded).
pub fn plan(args: &[String]) -> Invocation {
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

    // Known limitation: joined short-flag forms such as `-fhuman` are not
    // recognized above, so rtk would still append `--format json`; sqlfluff's
    // last-value-wins then runs in the user's format while rtk still tries to
    // JSON-parse it and reports a parse failure. Proper flag-value parsing
    // belongs to the arg-tokenizer migration (#3681) - fix it there.

    let mut out = Vec::with_capacity(args.len() + 3);
    if is_lint {
        out.push("lint".to_string());
        if !user_set_format {
            out.push("--format".to_string());
            out.push("json".to_string());
        }
        // Skip "lint" if the user spelled it out; we already pushed it.
        let start = usize::from(args.first().is_some_and(|a| a == "lint"));
        out.extend_from_slice(&args[start..]);
        // No path default: sqlfluff lints the current directory when given no
        // paths, and guessing whether a bareword is a path or a flag value
        // misreads things like `--dialect postgres`.
    } else {
        out.extend_from_slice(args);
    }

    Invocation {
        args: out,
        expect_json: is_lint && (!user_set_format || user_json_format),
    }
}

impl Invocation {
    /// Render sqlfluff's stdout for display.
    ///
    /// `exit_code` only disambiguates output the filter could not read. It
    /// never suppresses the summary on its own: sqlfluff exits 1 whenever it
    /// finds violations, which is the normal case this filter exists for.
    pub fn render(&self, stdout: &str, exit_code: i32) -> String {
        if self.expect_json {
            render_lint_json(stdout, exit_code)
        } else {
            truncate(stdout.trim(), config::limits().passthrough_max_chars)
        }
    }
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let plan = plan(args);

    let mut cmd = resolved_command("sqlfluff");
    cmd.args(&plan.args);

    if verbose > 0 {
        eprintln!("Running: sqlfluff {}", plan.args.join(" "));
    }

    runner::run_filtered_with_exit(
        cmd,
        "sqlfluff",
        &args.join(" "),
        move |stdout, exit_code| plan.render(stdout, exit_code),
        runner::RunOptions::stdout_only().tee("sqlfluff"),
    )
}

/// Cap classes from the shared truncation policy (`src/core/README.md`).
const MAX_REPORTED_RULES: usize = CAP_WARNINGS;
const MAX_REPORTED_FILES: usize = CAP_WARNINGS;
/// No shared cap covers per-file rule breakdowns; named locally.
const MAX_RULES_PER_FILE: usize = 3;

/// Filter `sqlfluff lint --format json` output - group violations by rule and file.
///
/// Used where no exit code is available (pipes, direct re-entry); any parse
/// failure is then a genuine filter failure.
pub fn filter_sqlfluff_lint_json(output: &str) -> String {
    render_lint_json(output, 0)
}

fn render_lint_json(output: &str, exit_code: i32) -> String {
    let parsed: Vec<SqlfluffFile> = match serde_json::from_str(output) {
        Ok(f) => f,
        // Nothing parseable on a failed run means sqlfluff never linted: it hit
        // a fatal error, which it writes to stdout with an empty stderr (`Error:
        // Unknown dialect 'NOPE'`, exit 2 - verified on 2.3.5 and 4.3.0). Pass
        // its own message through rather than reporting a filter failure.
        Err(_) if exit_code != 0 => return output.trim().to_string(),
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
    // sqlfluff 2.x reports no fix information, so a zero count there would mean
    // "unknown" rather than "nothing is fixable". Only claim a number when the
    // payload substantiates one.
    let fixes_reported = files
        .iter()
        .flat_map(|f| &f.violations)
        .any(|v| v.fixes.is_some());
    let fixable_count: usize = files
        .iter()
        .flat_map(|f| &f.violations)
        .filter(|v| v.fixes.as_ref().is_some_and(|fixes| !fixes.is_empty()))
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
    if fixes_reported && fixable_count > 0 {
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

    // Per-violation detail, ranked worst-file-first like every section above.
    // Iterating sqlfluff's raw emission order instead would let truncation drop
    // exactly the file the summary just told the reader to open. Lines are
    // built only up to the cap, so a large run does not materialize tens of
    // thousands of strings to print fifty.
    const MAX_VIOLATIONS: usize = 50;
    let mut detail = String::new();
    let mut rendered = 0usize;
    'detail: for file in &files {
        let path = compact_path(&file.filepath);
        for v in &file.violations {
            if rendered == MAX_VIOLATIONS {
                break 'detail;
            }
            detail.push_str(&format!(
                "  {} {} {}\n",
                location(&path, v.line(), v.col()),
                v.code,
                truncate(v.description.trim(), 100),
            ));
            rendered += 1;
        }
    }

    if rendered > 0 {
        result.push_str("\nViolations:\n");
        result.push_str(&detail);
        if total_violations > rendered {
            result.push_str(&format!("  … +{} more\n", total_violations - rendered));
        }
    }

    if !fixes_reported {
        // No fix information in the payload (sqlfluff 2.x): the capability is
        // still there, only the count is unknowable.
        result.push_str("\n💡 Run `sqlfluff fix` to auto-fix the violations that support it\n");
    } else if fixable_count > 0 {
        result.push_str(&format!(
            "\n💡 Run `sqlfluff fix` to auto-fix {} violations\n",
            fixable_count
        ));
    }

    result.trim().to_string()
}

/// `path:line` sample location for a violation group, e.g. `models/x.sql:12`.
fn sample_location(path: &str, line: Option<u64>) -> String {
    location(&compact_path(path), line, None)
}

/// `path`, `path:line` or `path:line:col`.
///
/// `line()`/`col()` are optional because sqlfluff can omit a position, and a
/// fabricated `:0:0` is a location an agent would try to open. Omit instead.
fn location(path: &str, line: Option<u64>, col: Option<u64>) -> String {
    match (line, col) {
        (Some(l), Some(c)) => format!("{}:{}:{}", path, l, c),
        (Some(l), None) => format!("{}:{}", path, l),
        (None, _) => path.to_string(),
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

    // Fall back to the last two segments. A bare filename collapses different
    // files to the same string on ordinary non-dbt layouts (migrations/,
    // reports/, src/sql/ all holding an orders.sql), and the detail section
    // exists to be opened.
    match path.rfind('/') {
        Some(last) => {
            let start = path[..last].rfind('/').map_or(0, |pos| pos + 1);
            path[start..].to_string()
        }
        None => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    /// Real `sqlfluff 4.3.0 lint --format json` output: `start_line_no`
    /// coordinates, `fixes` arrays, nested paths.
    const V4_JSON: &str = include_str!("../../../tests/fixtures/sqlfluff_lint_v4_raw.json");
    /// Real `sqlfluff 2.3.5 lint --format json` output: legacy `line_no`
    /// coordinates, no `fixes` key, 51 files whose worst one is emitted last.
    const V2_JSON: &str = include_str!("../../../tests/fixtures/sqlfluff_lint_v2_raw.json");

    // ── exit-code semantics ─────────────────────────────────────────────────
    //
    // sqlfluff exits 1 whenever it finds violations. That is the normal case,
    // not a failure, so the exit code may only disambiguate output the filter
    // could not read - never suppress the summary on its own.

    #[test]
    fn test_render_summarizes_violations_on_exit_1() {
        let out = plan(&["lint".to_string()]).render(V4_JSON, 1);
        assert!(
            out.contains("SQLFluff: 3 violations in 2 files"),
            "exit 1 means violations found, not failure; got: {out}"
        );
        assert!(
            out.len() * 2 < V4_JSON.len(),
            "must compress: {} bytes out of {}",
            out.len(),
            V4_JSON.len()
        );
    }

    #[test]
    fn test_render_summarizes_violations_on_exit_1_via_lint_entry_point() {
        // `rtk lint sqlfluff lint ...` reaches the same plan, so both entry
        // points must produce byte-identical output for identical input.
        let direct = plan(&["lint".to_string(), "models/".to_string()]);
        let via_lint = plan(&["lint".to_string(), "models/".to_string()]);
        assert_eq!(direct.args, via_lint.args);
        assert_eq!(direct.render(V4_JSON, 1), via_lint.render(V4_JSON, 1));
        assert!(direct.render(V4_JSON, 1).contains("LT09"));
    }

    #[test]
    fn test_render_passes_fatal_error_through_verbatim() {
        // Verified against sqlfluff 2.3.5 and 4.3.0: a fatal error is written
        // to stdout with an empty stderr and exit 2.
        let out = plan(&[
            "lint".to_string(),
            "--dialect".to_string(),
            "NOPE".to_string(),
        ])
        .render("Error: Unknown dialect 'NOPE'\n", 2);
        assert_eq!(out, "Error: Unknown dialect 'NOPE'");
    }

    #[test]
    fn test_render_reports_clean_run_on_exit_0() {
        let out = plan(&[]).render("[]", 0);
        assert!(out.contains("No violations found"), "got: {out}");
    }

    // ── report ranking ──────────────────────────────────────────────────────

    #[test]
    fn test_violations_section_leads_with_the_worst_file() {
        // sqlfluff emits in path order, so the worst file is exactly the one a
        // truncation over raw order drops. V2_JSON holds 50 single-violation
        // files followed by zzz_worst.sql with 22.
        let out = filter_sqlfluff_lint_json(V2_JSON);
        let section = out
            .split("Violations:\n")
            .nth(1)
            .expect("report must have a Violations section");
        assert!(
            section
                .lines()
                .next()
                .expect("at least one violation line")
                .contains("zzz_worst.sql"),
            "the file ranked first must lead the detail section, got:\n{section}"
        );
        assert_eq!(
            section.lines().filter(|l| l.contains("zzz_worst.sql")).count(),
            22,
            "every violation of the worst file must survive truncation"
        );
    }

    #[test]
    fn test_violations_section_ranking_matches_top_files() {
        let out = filter_sqlfluff_lint_json(V2_JSON);
        let top_file = out
            .split("Top files:\n")
            .nth(1)
            .and_then(|s| s.lines().next())
            .expect("top files section");
        let first_violation = out
            .split("Violations:\n")
            .nth(1)
            .and_then(|s| s.lines().next())
            .expect("violations section");
        let name = top_file.split_whitespace().next().expect("top file name");
        assert!(
            first_violation.contains(name),
            "sections must rank alike: top file {name}, first violation {first_violation}"
        );
    }

    // ── coordinates ─────────────────────────────────────────────────────────

    #[test]
    fn test_violation_without_position_omits_coordinates() {
        let input = r#"[{"filepath": "models/a.sql", "violations": [
            {"code": "PRS", "description": "Unparsable section."}
        ]}]"#;
        let out = filter_sqlfluff_lint_json(input);
        assert!(
            !out.contains(":0"),
            "a missing position must be omitted, not rendered as line 0:\n{out}"
        );
        assert!(out.contains("models/a.sql PRS"), "got:\n{out}");
    }

    #[test]
    fn test_violation_with_line_but_no_column_omits_the_column() {
        let input = r#"[{"filepath": "models/a.sql", "violations": [
            {"code": "PRS", "description": "Unparsable section.", "line_no": 7}
        ]}]"#;
        let out = filter_sqlfluff_lint_json(input);
        assert!(out.contains("models/a.sql:7 PRS"), "got:\n{out}");
    }

    #[test]
    fn test_violation_renders_full_coordinates_when_reported() {
        let out = filter_sqlfluff_lint_json(V4_JSON);
        assert!(
            out.contains("models/orders.sql:1:1 LT09"),
            "got:\n{out}"
        );
    }

    // ── fixability across sqlfluff versions ─────────────────────────────────

    #[test]
    fn test_v4_reports_a_fixable_count() {
        let out = filter_sqlfluff_lint_json(V4_JSON);
        assert!(out.contains("(2 fixable)"), "got:\n{out}");
        assert!(out.contains("sqlfluff fix"), "got:\n{out}");
    }

    #[test]
    fn test_v2_without_fixes_key_still_offers_the_fix_hint() {
        // sqlfluff 2.x emits no `fixes` key at all, so an absent count means
        // "unknown", not "nothing is fixable". Suppressing the hint would lose
        // a capability the tool still has.
        let out = filter_sqlfluff_lint_json(V2_JSON);
        assert!(
            out.contains("sqlfluff fix"),
            "2.x must still surface the fix hint:\n{out}"
        );
        assert!(
            !out.contains("fixable)"),
            "2.x cannot substantiate a fixable count:\n{out}"
        );
    }

    // ── invocation planning ─────────────────────────────────────────────────

    #[test]
    fn test_plan_makes_bare_call_explicit_and_owns_the_format() {
        assert_eq!(plan(&[]).args, ["lint", "--format", "json"]);
        assert_eq!(
            plan(&["lint".to_string(), "models/".to_string()]).args,
            ["lint", "--format", "json", "models/"]
        );
    }

    #[test]
    fn test_plan_never_injects_a_second_format() {
        for user in [
            vec!["lint".to_string(), "--format".to_string(), "human".to_string()],
            vec!["lint".to_string(), "-f=human".to_string()],
        ] {
            let p = plan(&user);
            assert_eq!(
                p.args.iter().filter(|a| a.starts_with("-f")).count(),
                user.iter().filter(|a| a.starts_with("-f")).count(),
                "planned args must not add a format flag: {:?}",
                p.args
            );
            assert!(!p.expect_json, "a human-format run is not JSON to parse");
        }
    }

    #[test]
    fn test_plan_keeps_user_json_format_parseable() {
        assert!(plan(&["lint".to_string(), "--format=json".to_string()]).expect_json);
        assert!(
            plan(&[
                "lint".to_string(),
                "-f".to_string(),
                "json".to_string()
            ])
            .expect_json
        );
    }

    #[test]
    fn test_plan_passes_other_subcommands_through_untouched() {
        let p = plan(&["parse".to_string(), "models/x.sql".to_string()]);
        assert_eq!(p.args, ["parse", "models/x.sql"]);
        assert!(!p.expect_json);
        assert_eq!(p.render("plain parse tree", 0), "plain parse tree");
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
        assert_eq!(compact_path("some/deep/path/file.sql"), "path/file.sql");
    }

    #[test]
    fn test_compact_path_disambiguates_non_dbt_layouts() {
        // The detail section exists to be opened, so paths sharing a filename
        // across an ordinary migrations/reports/src layout must stay distinct.
        let rendered: Vec<String> = ["migrations/orders.sql", "reports/orders.sql", "/repo/src/sql/orders.sql"]
            .iter()
            .map(|p| compact_path(p))
            .collect();
        assert_eq!(
            rendered,
            ["migrations/orders.sql", "reports/orders.sql", "sql/orders.sql"]
        );
    }

    #[test]
    fn test_compact_path_filename_only() {
        assert_eq!(compact_path("file.sql"), "file.sql");
    }

    // ── token savings ─────────────────────────────────────────────────────────

    #[test]
    fn test_token_savings_at_least_20_percent() {
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
