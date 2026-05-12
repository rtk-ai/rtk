//! Luacheck output filter.
//!
//! Keeps the total summary and the first actionable diagnostics while dropping
//! per-file progress lines.

use crate::core::runner;
use crate::core::utils::{resolved_command, strip_ansi, truncate};
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;

#[derive(Debug, PartialEq, Eq)]
struct LuacheckIssue {
    path: String,
    line: usize,
    column: usize,
    code: Option<String>,
    message: String,
}

#[derive(Debug, PartialEq, Eq)]
struct LuacheckSummary {
    warnings: usize,
    errors: usize,
    files: usize,
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("luacheck");

    if !has_codes_flag(args) {
        cmd.arg("--codes");
    }

    cmd.args(args);

    if verbose > 0 {
        let injected = if has_codes_flag(args) { "" } else { " --codes" };
        eprintln!("Running: luacheck{} {}", injected, args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "luacheck",
        &args.join(" "),
        filter_luacheck_output,
        runner::RunOptions::stdout_only().tee("luacheck"),
    )
}

fn has_codes_flag(args: &[String]) -> bool {
    args.iter().any(|a| a == "--codes")
}

pub(crate) fn filter_luacheck_output(output: &str) -> String {
    let clean = strip_ansi(output);
    let summary = parse_summary(&clean);
    let issues = parse_issues(&clean);

    if clean.trim().is_empty() {
        return "luacheck: no output".to_string();
    }

    if let Some(summary) = &summary {
        if summary.warnings == 0 && summary.errors == 0 {
            return format!("ok luacheck: {} files clean", summary.files);
        }
    }

    if issues.is_empty() {
        return crate::core::utils::fallback_tail(&clean, "luacheck", 8);
    }

    let mut result = match summary {
        Some(s) => format!(
            "luacheck: {} warnings, {} errors in {} files\n",
            s.warnings, s.errors, s.files
        ),
        None => format!("luacheck: {} issues\n", issues.len()),
    };

    for issue in issues.iter().take(10) {
        let code = issue
            .code
            .as_ref()
            .map(|c| format!(" ({})", c))
            .unwrap_or_default();
        result.push_str(&format!(
            "{}:{}:{}{} {}\n",
            compact_path(&issue.path),
            issue.line,
            issue.column,
            code,
            truncate(&issue.message, 120)
        ));
    }

    if issues.len() > 10 {
        result.push_str(&format!("... +{} more issues\n", issues.len() - 10));
    }

    result.trim().to_string()
}

fn parse_summary(output: &str) -> Option<LuacheckSummary> {
    lazy_static! {
        static ref SUMMARY_RE: Regex = Regex::new(
            r"Total:\s+(\d+)\s+warnings?\s*/\s*(\d+)\s+errors?\s+in\s+(\d+)\s+files?"
        )
        .unwrap();
    }

    SUMMARY_RE.captures(output).map(|caps| LuacheckSummary {
        warnings: caps[1].parse().unwrap_or(0),
        errors: caps[2].parse().unwrap_or(0),
        files: caps[3].parse().unwrap_or(0),
    })
}

fn parse_issues(output: &str) -> Vec<LuacheckIssue> {
    lazy_static! {
        static ref ISSUE_RE: Regex =
            Regex::new(r"^\s*(.+?):(\d+):(\d+):\s+(?:(\([A-Z]\d+\))\s+)?(.+)$").unwrap();
    }

    output
        .lines()
        .filter_map(|line| {
            let caps = ISSUE_RE.captures(line)?;
            let path = caps.get(1)?.as_str().trim().to_string();
            if path == "Total" || path.starts_with("Checking ") {
                return None;
            }
            Some(LuacheckIssue {
                path,
                line: caps.get(2)?.as_str().parse().ok()?,
                column: caps.get(3)?.as_str().parse().ok()?,
                code: caps
                    .get(4)
                    .map(|m| m.as_str().trim_matches(['(', ')']).to_string()),
                message: caps.get(5)?.as_str().trim().to_string(),
            })
        })
        .collect()
}

fn compact_path(path: &str) -> String {
    let path = path.replace('\\', "/");

    for prefix in &["src/", "spec/", "tests/", "lua/", "plugins/"] {
        if path.starts_with(prefix) {
            return path;
        }
    }

    for marker in &["/src/", "/spec/", "/tests/", "/lua/", "/plugins/"] {
        if let Some(pos) = path.rfind(marker) {
            return path[pos + 1..].to_string();
        }
    }

    if let Some(pos) = path.rfind('/') {
        path[pos + 1..].to_string()
    } else {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::utils::count_tokens;

    #[test]
    fn test_filter_luacheck_clean_summary() {
        let output = "Checking main.lua OK\n\nTotal: 0 warnings / 0 errors in 1 file";
        let result = filter_luacheck_output(output);
        assert_eq!(result, "ok luacheck: 1 files clean");
    }

    #[test]
    fn test_filter_luacheck_groups_diagnostics() {
        let output = r#"Checking main.lua                       1 warning
Checking spec/foo_spec.lua              1 warning

    main.lua:10:5: (W211) unused variable 'tmp'
    spec/foo_spec.lua:22:1: (E011) expected '=' near 'end'

Total: 1 warning / 1 error in 2 files"#;

        let result = filter_luacheck_output(output);
        assert!(result.contains("1 warnings, 1 errors in 2 files"));
        assert!(result.contains("main.lua:10:5 (W211) unused variable"));
        assert!(result.contains("spec/foo_spec.lua:22:1 (E011) expected"));
        assert!(!result.contains("Checking main.lua"));
    }

    #[test]
    fn test_filter_luacheck_caps_issues() {
        let mut output = String::new();
        for i in 1..=12 {
            output.push_str(&format!("file{}.lua:{}:1: (W211) unused variable\n", i, i));
        }
        output.push_str("Total: 12 warnings / 0 errors in 12 files");

        let result = filter_luacheck_output(&output);
        assert!(result.contains("file10.lua:10:1"));
        assert!(!result.contains("file11.lua:11:1"));
        assert!(result.contains("+2 more issues"));
    }

    #[test]
    fn test_luacheck_token_savings() {
        let mut output = String::new();
        for i in 1..=40 {
            output.push_str(&format!("Checking src/file{}.lua OK\n", i));
        }
        output.push_str("\n    src/file1.lua:10:5: (W211) unused variable 'tmp'\n");
        output.push_str("Total: 1 warning / 0 errors in 40 files");

        let filtered = filter_luacheck_output(&output);
        let savings =
            100.0 - (count_tokens(&filtered) as f64 / count_tokens(&output) as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "expected >=60% savings, got {:.1}%\n{}",
            savings,
            filtered
        );
    }
}
