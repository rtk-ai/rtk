//! Filters `ast-grep run` plain-mode output by grouping matches by file and
//! capping how many are shown. `--json` is left near-passthrough (explicit
//! structured-output request — see Correctness vs Token Savings).

use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::LazyLock;

/// Matches the `path:line:` prefix ast-grep emits for every match/context line.
static MATCH_LINE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^(?P<file>[^:]+):(?P<line>\d+):(?P<content>.*)$").unwrap());

const DEFAULT_MAX_TOTAL: usize = 50;
const DEFAULT_MAX_PER_FILE: usize = 5;

/// Groups raw `ast-grep run` plain output by file, keeping at most
/// `max_per_file` lines per file and `max_total` lines overall. Files beyond
/// the cap collapse to a one-line count so nothing silently disappears.
fn filter_ast_grep(raw: &str, max_per_file: usize, max_total: usize) -> String {
    let mut by_file: HashMap<&str, Vec<(usize, &str)>> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();

    for line in raw.lines() {
        let Some(caps) = MATCH_LINE_RE.captures(line) else {
            continue;
        };
        let file = caps.name("file").unwrap().as_str();
        let line_num: usize = caps.name("line").unwrap().as_str().parse().unwrap_or(0);
        let content = caps.name("content").unwrap().as_str();
        if !by_file.contains_key(file) {
            order.push(file);
        }
        by_file.entry(file).or_default().push((line_num, content));
    }

    if order.is_empty() {
        return raw.to_string();
    }

    let mut out = String::new();
    let mut shown_total = 0;
    let mut skipped_files = 0;

    for file in &order {
        let entries = &by_file[file];
        if shown_total >= max_total {
            skipped_files += 1;
            continue;
        }
        for (line_num, content) in entries.iter().take(max_per_file) {
            if shown_total >= max_total {
                break;
            }
            out.push_str(file);
            out.push(':');
            out.push_str(&line_num.to_string());
            out.push(':');
            out.push_str(content);
            out.push('\n');
            shown_total += 1;
        }
        if entries.len() > max_per_file {
            out.push_str(&format!(
                "  … {} more matches in {}\n",
                entries.len() - max_per_file,
                file
            ));
        }
    }

    if skipped_files > 0 {
        out.push_str(&format!(
            "… {} more file(s) with matches not shown (use --json or narrow the pattern)\n",
            skipped_files
        ));
    }

    out
}

pub fn run(args: &[String]) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let real_cmd = format!("ast-grep {}", args.join(" "));

    let is_json = args.iter().any(|a| a == "--json" || a.starts_with("--json="));

    let mut cmd = resolved_command("ast-grep");
    cmd.args(args);
    let result = exec_capture(&mut cmd).context("Failed to execute ast-grep")?;

    let filtered_owned;
    let filtered: &str = if is_json {
        &result.stdout
    } else {
        filtered_owned = filter_ast_grep(&result.stdout, DEFAULT_MAX_PER_FILE, DEFAULT_MAX_TOTAL);
        &filtered_owned
    };

    let shown = crate::core::guard::never_worse(&result.stdout, filtered);
    timer.track(&real_cmd, "rtk ast-grep", &result.stdout, shown);
    print!("{}", shown);

    if !result.stderr.is_empty() {
        eprint!("{}", result.stderr);
    }

    Ok(result.exit_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    #[test]
    fn test_groups_and_caps_by_file() {
        let input = "\
src/a.rs:1:fn foo() {}
src/a.rs:2:fn bar() {}
src/a.rs:3:fn baz() {}
src/b.rs:10:fn qux() {}
";
        let out = filter_ast_grep(input, 2, 50);
        assert!(out.contains("src/a.rs:1:"));
        assert!(out.contains("src/a.rs:2:"));
        assert!(!out.contains("src/a.rs:3:"));
        assert!(out.contains("1 more matches in src/a.rs"));
        assert!(out.contains("src/b.rs:10:"));
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(filter_ast_grep("", 5, 50), "");
    }

    #[test]
    fn test_unparseable_input_falls_back_unchanged() {
        let input = "no colons here\njust plain text\n";
        assert_eq!(filter_ast_grep(input, 5, 50), input);
    }

    #[test]
    fn test_real_fixture_savings() {
        let input = include_str!("../../../tests/fixtures/ast_grep_lazylock_raw.txt");
        let output = filter_ast_grep(input, DEFAULT_MAX_PER_FILE, DEFAULT_MAX_TOTAL);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "ast-grep filter: expected >=60% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }
}
