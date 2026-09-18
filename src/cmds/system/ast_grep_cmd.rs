//! Filters `ast-grep run` plain-mode output by grouping matches by file and
//! capping how many are shown. Explicit JSON and context requests pass through
//! unchanged — see Correctness vs Token Savings.

use crate::core::arg_tokenizer::{Dialect, TokenKind, ValueSpec, tokenize_grammar};
use crate::core::args_utils::restore_double_dash;
use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::LazyLock;

/// Matches the `path:line:` prefix ast-grep emits for every match/context line.
static MATCH_LINE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^(?P<file>[^:]+):(?P<line>\d+):(?P<content>.*)$").unwrap()
});

const DEFAULT_MAX_TOTAL: usize = 50;
const DEFAULT_MAX_PER_FILE: usize = 5;

/// Counts non-blank lines that are not `path:line:content`.
///
/// Plain `ast-grep run` output is made up entirely of that shape. Other modes
/// are not: in an `ast-grep scan` diagnostic only the `  ┌─ a.rs:2:13` locator
/// parses, while the rule id, severity, message and source line do not, and
/// `--heading` mode and Windows drive-letter paths (`C:\src\a.rs`, which
/// `[^:]+` cannot match) fail to parse the same way. Grouping such a shape
/// would keep whichever lines happen to parse and discard the rest, so any
/// non-zero count leaves the output alone. `search.rs::unparsed_signal` guards
/// grep/rg with the same rule.
fn unparsed_signal(raw: &str) -> usize {
    raw.lines()
        .filter(|line| !line.trim().is_empty() && !MATCH_LINE_RE.is_match(line))
        .count()
}

/// Groups raw `ast-grep run` plain output by file, keeping at most
/// `max_per_file` lines per file and `max_total` lines overall. Every line the
/// caps hold back is counted in a hint, so nothing disappears silently; any
/// other output shape is returned unchanged.
fn filter_ast_grep(raw: &str, max_per_file: usize, max_total: usize) -> String {
    if unparsed_signal(raw) > 0 {
        return raw.to_string();
    }

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
        let mut shown_here = 0;
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
            shown_here += 1;
        }
        // `shown_here`, not `max_per_file`: `max_total` can cut a file short of
        // its own cap, and the hint has to cover every line this loop skipped.
        // ast-grep prints one line per matched source line and a structural
        // match spans several, so the unit is lines rather than matches.
        if entries.len() > shown_here {
            out.push_str(&format!(
                "  … {} more match line(s) in {}\n",
                entries.len() - shown_here,
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

// Value-taking options from `ast-grep run --help`. Values must be consumed so
// a pattern or glob spelled like `-C2` is not mistaken for a context request.
fn run_option_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    match (kind, name) {
        (TokenKind::Short, "p" | "k" | "r" | "l" | "c" | "j" | "A" | "B" | "C")
        | (
            TokenKind::Long,
            "pattern" | "selector" | "strictness" | "kind" | "rewrite" | "lang" | "config"
            | "no-ignore" | "globs" | "threads" | "color" | "inspect" | "heading" | "after"
            | "before" | "context",
        ) => Some(ValueSpec::value()),
        (TokenKind::Long, "json" | "debug-query") => Some(ValueSpec::attached_only()),
        _ => None,
    }
}

fn requests_context(args: &[String]) -> bool {
    tokenize_grammar(args, &run_option_value, Dialect::Posix)
        .iter()
        .any(|token| {
            matches!(
                (token.kind, token.text),
                (TokenKind::Short, "A" | "B" | "C")
                    | (TokenKind::Long, "after" | "before" | "context")
            )
        })
}

pub fn run(args: &[String]) -> Result<i32> {
    let args = restore_double_dash(args);
    let timer = tracking::TimedExecution::start();
    let real_cmd = format!("ast-grep {}", args.join(" "));

    let is_json = args
        .iter()
        .any(|a| a == "--json" || a.starts_with("--json="));

    let mut cmd = resolved_command("ast-grep");
    cmd.args(&args);
    let result = exec_capture(&mut cmd).context("Failed to execute ast-grep")?;

    let filtered_owned;
    // Context and match lines have the same prefix, so the line cap cannot
    // distinguish them. Preserve the detail the caller explicitly requested.
    let filtered: &str = if is_json || requests_context(&args) {
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

    #[test]
    fn context_options_are_recognized_in_all_spellings() {
        for args in [
            vec!["run", "-C", "2"],
            vec!["run", "-C2"],
            vec!["run", "-A2"],
            vec!["run", "-B", "2"],
            vec!["run", "--context=2"],
            vec!["run", "--after", "2"],
            vec!["run", "--before=2"],
            vec!["-p", "make($$$)", "-C2"],
            vec!["run", "-C0"],
        ] {
            let args: Vec<String> = args.into_iter().map(str::to_owned).collect();
            assert!(requests_context(&args), "{args:?}");
        }
    }

    #[test]
    fn context_like_values_and_positionals_are_not_options() {
        for args in [
            vec!["run", "-p", "-C2"],
            vec!["run", "-p-C2"],
            vec!["run", "--pattern=--context"],
            vec!["run", "--globs", "-A2"],
            vec!["run", "-r", "-B2"],
            vec!["run", "-c", "--context"],
            vec!["run", "--", "-C2"],
            vec!["run", "--", "--after=2"],
            vec!["run", "--contextual=2"],
        ] {
            let args: Vec<String> = args.into_iter().map(str::to_owned).collect();
            assert!(!requests_context(&args), "{args:?}");
        }
    }

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
        assert!(out.contains("1 more match line(s) in src/a.rs"));
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

    /// A scan diagnostic parses only on its locator line, so grouping it would
    /// keep `  ┌─ a.rs:2:13` and discard the rule id, severity, message and
    /// source line.
    #[test]
    fn test_scan_diagnostic_shape_passes_through() {
        let input = "\
warning[no-unwrap]: avoid unwrap
  ┌─ a.rs:2:13
  │
2 │     let x = foo().unwrap();
  │             ^^^^^^^^^^^^^^
";
        assert_eq!(filter_ast_grep(input, 5, 50), input);
    }

    /// `max_total` can cut a file short before its own `max_per_file` cap is
    /// reached; the per-file hint must still account for the remainder.
    #[test]
    fn test_total_cap_hints_lines_it_cut() {
        let input = "\
a.rs:1:one
a.rs:2:two
a.rs:3:three
a.rs:4:four
a.rs:5:five
b.rs:1:six
b.rs:2:seven
b.rs:3:eight
";
        let out = filter_ast_grep(input, 5, 6);
        assert!(out.contains("b.rs:1:six"), "{out}");
        assert!(!out.contains("b.rs:2:seven"), "{out}");
        assert!(
            out.contains("2 more match line(s) in b.rs"),
            "cut lines must be hinted, got: {out}"
        );
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
