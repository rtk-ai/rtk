//! Filters `ast-grep run` plain-mode output by grouping matches by file and
//! capping how many are shown. `--json` is left near-passthrough (explicit
//! structured-output request — see Correctness vs Token Savings), and so is
//! every subcommand other than `run`.

use crate::core::arg_tokenizer::{TokenKind, ValueSpec};
use crate::core::guard::never_worse;
use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::utils::resolved_command;
use crate::core::{arg_tokenizer, truncate};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::ffi::OsString;
use std::sync::LazyLock;

/// Matches the `path:line:` prefix ast-grep emits for every match/context line.
static MATCH_LINE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^(?P<file>[^:]+):(?P<line>\d+):(?P<content>.*)$").unwrap()
});

const DEFAULT_MAX_TOTAL: usize = truncate::CAP_INVENTORY;
const DEFAULT_MAX_PER_FILE: usize = 5;

/// Recovery slug for the match lines the caps hold back.
const TEE_SLUG: &str = "ast-grep";

/// Bytes the compaction must save before it is worth storing the output to point at. Covers
/// `[full output: <path>]` for any path the recovery store can hand back, since that length is
/// only known once the file has been written.
const HINT_RESERVE: usize = 256;

/// ast-grep's subcommands other than `run`. Only `run`'s plain output has the
/// `path:line:content` shape this module parses: `scan` reports diagnostics (whose
/// `--report-style short` form happens to parse, and was being capped as if it were a match
/// list), `test` reports rule results, and `lsp` speaks a protocol over stdin, which capturing
/// closes.
const OTHER_SUBCOMMANDS: &[&str] = &["scan", "test", "new", "lsp", "completions", "docs"];

/// The value-taking options ast-grep accepts *ahead of* a subcommand. Deliberately only those:
/// a per-subcommand arity table would go stale against a tool that adds options, and an entry
/// missing from it puts an option's value where a subcommand is looked for -- `--color always
/// lsp` reading as `run` hands an editor a closed stdin.
fn global_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    match kind {
        TokenKind::Long => matches!(name, "config" | "color").then(ValueSpec::value),
        TokenKind::Short => matches!(name, "c").then(ValueSpec::value),
        _ => None,
    }
}

/// `run`'s own value-taking options, consulted only once the invocation is known to be a `run`.
/// Being wrong here costs compression, never correctness: the worst an entry missing from this
/// table does is leave a pattern looking like a free positional.
fn run_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    match kind {
        TokenKind::Long => matches!(
            name,
            "pattern"
                | "rewrite"
                | "lang"
                | "globs"
                | "context"
                | "after"
                | "before"
                | "selector"
                | "strictness"
                | "threads"
        )
        .then(ValueSpec::value)
        .or_else(|| global_takes_value(kind, name)),
        TokenKind::Short => matches!(name, "p" | "r" | "l" | "C" | "A" | "B")
            .then(ValueSpec::value)
            .or_else(|| global_takes_value(kind, name)),
        _ => None,
    }
}

/// Whether this invocation is one whose output the filter understands.
///
/// Read in two passes, because which options take a value depends on the subcommand that has
/// not been identified yet. The first pass knows only the options that may precede a
/// subcommand, so the first free positional it finds is the subcommand whenever the line names
/// one. Once that positional is `run`, the rest of the line is `run`'s own grammar and a second
/// pass reads it: without that, `run -p scan src/` took its pattern for the `scan` subcommand
/// and gave up the filter entirely.
///
/// When the first positional is neither `run` nor a known subcommand, the line is not
/// identified: an option this module does not know may have claimed the real subcommand's
/// place. Any of the other names anywhere on the line then disqualifies it. That costs a
/// pattern or a path spelled like a subcommand, which is compression; reading `lsp` as a `run`
/// hands an editor a closed stdin, which is correctness, and RTK's priority order picks
/// correctness.
fn filters_this_invocation(args: &[String]) -> bool {
    let global_tokens =
        arg_tokenizer::tokenize_grammar(args, &global_takes_value, arg_tokenizer::Dialect::Posix);
    let positionals: Vec<&str> = arg_tokenizer::before_dashdash(&global_tokens)
        .iter()
        .filter(|t| t.is_free_positional())
        .map(|t| t.text)
        .collect();

    let identified_as_run = positionals.first() == Some(&"run");
    if !identified_as_run && positionals.iter().any(|n| OTHER_SUBCOMMANDS.contains(n)) {
        return false;
    }

    // A `--stdin` run reads the source from the pipe that capturing would close.
    let run_tokens =
        arg_tokenizer::tokenize_grammar(args, &run_takes_value, arg_tokenizer::Dialect::Posix);
    !arg_tokenizer::before_dashdash(&run_tokens)
        .iter()
        .any(|t| t.kind == TokenKind::Long && t.text == "stdin")
}

/// Whether the user asked for JSON, which passes through as an explicit structured-output
/// request. Read from tokens, not raw argv: past `--` a `--json` is a path, and every other
/// argument check in this module already respects that boundary.
fn requests_json(args: &[String]) -> bool {
    let tokens =
        arg_tokenizer::tokenize_grammar(args, &run_takes_value, arg_tokenizer::Dialect::Posix);
    arg_tokenizer::before_dashdash(&tokens)
        .iter()
        .any(|t| t.kind == TokenKind::Long && t.text == "json")
}

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

/// What [`filter_ast_grep`] produced: the text to print, and how many match lines the caps
/// held back. The count drives the recovery hint, which only [`run`] can emit because storing
/// the full output is I/O.
struct Filtered {
    text: String,
    hidden_lines: usize,
}

/// Groups raw `ast-grep run` plain output by file, keeping at most
/// `max_per_file` lines per file and `max_total` lines overall. Every line the
/// caps hold back is counted in a hint, so nothing disappears silently; any
/// other output shape is returned unchanged.
fn filter_ast_grep(raw: &str, max_per_file: usize, max_total: usize) -> Filtered {
    if unparsed_signal(raw) > 0 {
        return Filtered {
            text: raw.to_string(),
            hidden_lines: 0,
        };
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
        return Filtered {
            text: raw.to_string(),
            hidden_lines: 0,
        };
    }

    let mut out = String::new();
    let mut shown_total = 0;
    let mut skipped_files = 0;
    let mut skipped_lines = 0;
    let mut hidden_lines = 0;

    for file in &order {
        let entries = &by_file[file];
        if shown_total >= max_total {
            skipped_files += 1;
            skipped_lines += entries.len();
            hidden_lines += entries.len();
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
            hidden_lines += entries.len() - shown_here;
            out.push_str(&format!(
                "  … {} more match line(s) in {}\n",
                entries.len() - shown_here,
                file
            ));
        }
    }

    // Lines, not just files: a file the total cap skips outright carries match lines too, and
    // counting only the files left them out of every tally the output carried -- 255 of 387
    // lines on the module's own fixture, with nothing saying they existed.
    if skipped_files > 0 {
        out.push_str(&format!(
            "… {} more match line(s) in {} more file(s) not shown\n",
            skipped_lines, skipped_files
        ));
    }

    Filtered {
        text: out,
        hidden_lines,
    }
}

pub fn run(args: &[String]) -> Result<i32> {
    // Capturing hands the child a closed stdin, so an invocation that reads one has to be
    // streamed instead -- `ast-grep lsp` otherwise served an editor an empty document.
    if !filters_this_invocation(args) {
        let forwarded: Vec<OsString> = args.iter().map(OsString::from).collect();
        return crate::core::runner::run_passthrough("ast-grep", &forwarded, 0);
    }

    let timer = tracking::TimedExecution::start();
    let real_cmd = format!("ast-grep {}", args.join(" "));

    let is_json = requests_json(args);

    let mut cmd = resolved_command("ast-grep");
    cmd.args(args);
    let result = exec_capture(&mut cmd).context("Failed to execute ast-grep")?;

    let filtered_owned;
    let filtered: &str = if is_json {
        &result.stdout
    } else {
        let mut out = filter_ast_grep(&result.stdout, DEFAULT_MAX_PER_FILE, DEFAULT_MAX_TOTAL);
        // A count of what was cut is not a way to read it, so the whole output is stored and
        // pointed at, as every other capping filter does. Storing it is a write plus a
        // rotation slot, though, and the guard below prints the raw output whenever the
        // compacted form is not smaller -- so the decision is taken on the text the hint is
        // already part of. Deciding before appending it let a search land in the band where
        // the hint tipped the balance: the archive was written, the guard then printed raw,
        // and the file sat there with nothing pointing at it, having evicted one a caller
        // still needed.
        if out.hidden_lines > 0 {
            let fallback = "  (use --json, narrow the pattern, or rtk proxy ast-grep)\n";
            // Storing the output is a write plus a rotation slot, and the guard below prints
            // the raw output whenever the compacted form is not smaller. A hint appended after
            // that decision was taken could tip the balance back: the archive got written, the
            // guard printed raw, and the file sat there with nothing pointing at it, having
            // evicted one a caller still needed. The hint's length is not known until the
            // store has been written, so the decision is taken against a reserve wide enough
            // for any path the store can produce.
            let margin = result.stdout.len().saturating_sub(out.text.len());
            match (margin > HINT_RESERVE)
                .then(|| crate::core::tee::force_tee_hint(&result.stdout, TEE_SLUG))
                .flatten()
            {
                Some(hint) => {
                    out.text.push_str(&hint);
                    out.text.push('\n');
                }
                // Recovery is off, its directory is unwritable, or the compaction did not save
                // enough to pay for a hint. Name what still works rather than leave a count
                // with no route behind it.
                None => out.text.push_str(fallback),
            }
        }
        filtered_owned = out.text;
        &filtered_owned
    };

    let shown = never_worse(&result.stdout, filtered);
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

    fn filtered(raw: &str, max_per_file: usize, max_total: usize) -> String {
        filter_ast_grep(raw, max_per_file, max_total).text
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn test_groups_and_caps_by_file() {
        let input = "\
src/a.rs:1:fn foo() {}
src/a.rs:2:fn bar() {}
src/a.rs:3:fn baz() {}
src/b.rs:10:fn qux() {}
";
        let out = filtered(input, 2, 50);
        assert!(out.contains("src/a.rs:1:"));
        assert!(out.contains("src/a.rs:2:"));
        assert!(!out.contains("src/a.rs:3:"));
        assert!(out.contains("1 more match line(s) in src/a.rs"));
        assert!(out.contains("src/b.rs:10:"));
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(filtered("", 5, 50), "");
    }

    #[test]
    fn test_unparseable_input_falls_back_unchanged() {
        let input = "no colons here\njust plain text\n";
        assert_eq!(filtered(input, 5, 50), input);
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
        assert_eq!(filtered(input, 5, 50), input);
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
        let out = filtered(input, 5, 6);
        assert!(out.contains("b.rs:1:six"), "{out}");
        assert!(!out.contains("b.rs:2:seven"), "{out}");
        assert!(
            out.contains("2 more match line(s) in b.rs"),
            "cut lines must be hinted, got: {out}"
        );
    }

    /// A file the total cap skips outright carries match lines too. Reporting only how many
    /// FILES were dropped left those lines out of every tally the output carried.
    #[test]
    fn test_skipped_files_have_their_lines_counted() {
        let mut input = String::new();
        for file in 'a'..='d' {
            for line in 1..=10 {
                input.push_str(&format!("{file}.rs:{line}:match\n"));
            }
        }
        let out = filter_ast_grep(&input, 5, 10);

        // Two files shown (5 lines each), two skipped entirely: 5 held back per shown file
        // and 10 per skipped one.
        assert!(
            out.text
                .contains("20 more match line(s) in 2 more file(s) not shown"),
            "{}",
            out.text
        );
        assert_eq!(out.hidden_lines, 30, "{}", out.text);

        // Every one of the 40 lines is either printed or counted.
        let shown = out
            .text
            .lines()
            .filter(|l| MATCH_LINE_RE.is_match(l))
            .count();
        assert_eq!(shown + out.hidden_lines, 40);
    }

    #[test]
    fn test_capped_output_always_carries_a_route_to_the_rest() {
        // The count is not the recovery. Whether or not the tee is available, the output has
        // to name something the reader can actually run.
        let mut input = String::new();
        for file in 'a'..='d' {
            for line in 1..=10 {
                input.push_str(&format!("{file}.rs:{line}:match\n"));
            }
        }
        let out = filter_ast_grep(&input, 5, 10);
        assert!(out.hidden_lines > 0);
        // The trailer states the size of what is missing; `run` then appends the route.
        assert!(
            out.text.contains("not shown"),
            "a count of the cut lines: {}",
            out.text
        );
    }

    #[test]
    fn test_nothing_held_back_reports_no_hidden_lines() {
        let input = "a.rs:1:one\nb.rs:2:two\n";
        assert_eq!(filter_ast_grep(input, 5, 50).hidden_lines, 0);
        assert_eq!(filter_ast_grep("", 5, 50).hidden_lines, 0);
        assert_eq!(filter_ast_grep("not parseable\n", 5, 50).hidden_lines, 0);
    }

    /// Only `run` produces the `path:line:content` shape this module parses. `scan
    /// --report-style short` happens to parse as well and was being capped as if it were a
    /// match list, and `lsp` speaks a protocol over the stdin that capturing closes.
    #[test]
    fn test_a_run_flag_value_is_not_read_as_a_subcommand() {
        // `-p scan` is the pattern "scan". Classifying it as the `scan` subcommand sent a
        // perfectly ordinary search to the passthrough and gave up the whole filter.
        for pattern in OTHER_SUBCOMMANDS {
            assert!(
                filters_this_invocation(&args(&["run", "-p", pattern, "src/"])),
                "-p {pattern}"
            );
            assert!(
                filters_this_invocation(&args(&["run", "--pattern", pattern])),
                "--pattern {pattern}"
            );
        }
        assert!(filters_this_invocation(&args(&[
            "run", "--globs", "docs", "-p", "$A"
        ])));
    }

    #[test]
    fn test_a_global_option_value_does_not_hide_the_subcommand() {
        // The other direction, and the one that costs correctness: an option ahead of the
        // subcommand must not let `lsp` slide into first position unnoticed.
        assert!(!filters_this_invocation(&args(&[
            "--color", "always", "lsp"
        ])));
        assert!(!filters_this_invocation(&args(&[
            "--config", "sg.yml", "scan"
        ])));
        assert!(!filters_this_invocation(&args(&["-c", "sg.yml", "test"])));
        // And an option this table does not know keeps the conservative answer.
        assert!(!filters_this_invocation(&args(&["--future-flag", "lsp"])));
    }

    #[test]
    fn test_only_run_is_filtered() {
        assert!(filters_this_invocation(&args(&["run", "-p", "$A", "src/"])));
        assert!(filters_this_invocation(&args(&["-p", "$A", "src/"])));
        for sub in OTHER_SUBCOMMANDS {
            assert!(!filters_this_invocation(&args(&[sub])), "{sub}");
        }
        assert!(!filters_this_invocation(&args(&[
            "scan",
            "--report-style",
            "short"
        ])));
        // `run --stdin` reads the source from the pipe capturing would close.
        assert!(!filters_this_invocation(&args(&[
            "run", "-p", "$A", "--stdin"
        ])));
    }

    /// A subcommand sitting behind a flag whose arity RTK does not know would land in first
    /// position and read as `run` -- handing an editor a closed stdin again.
    #[test]
    fn test_a_subcommand_behind_any_flag_is_still_a_subcommand() {
        for flag in [
            "--color",
            "--heading",
            "--no-ignore",
            "--globs",
            "--threads",
        ] {
            assert!(
                !filters_this_invocation(&args(&[flag, "always", "lsp"])),
                "{flag}"
            );
            assert!(
                !filters_this_invocation(&args(&[flag, "always", "scan"])),
                "{flag}"
            );
        }
    }

    /// Past `--` everything is a path, including one spelled `--json`: that is not a request
    /// for structured output, and treating it as one gave up the filter.
    #[test]
    fn test_json_is_read_from_tokens_not_raw_argv() {
        assert!(requests_json(&args(&["run", "-p", "$A", "--json"])));
        assert!(requests_json(&args(&["run", "-p", "$A", "--json=stream"])));
        assert!(requests_json(&args(&["-p", "$A", "--json", "src/"])));
        assert!(!requests_json(&args(&["run", "-p", "$A", "src/"])));
        assert!(!requests_json(&args(&["run", "-p", "$A", "--", "--json"])));
        assert!(!requests_json(&args(&["-p", "--json", "src/"])));
    }

    /// Past `--` everything is a path, so a directory named after a subcommand is one.
    #[test]
    fn test_a_path_past_the_boundary_is_not_a_subcommand() {
        assert!(filters_this_invocation(&args(&[
            "run", "-p", "$A", "--", "scan"
        ])));
        assert!(filters_this_invocation(&args(&["-p", "$A", "--", "lsp"])));
        assert!(filters_this_invocation(&args(&[
            "run", "-p", "$A", "--", "--stdin"
        ])));
    }

    #[test]
    fn test_real_fixture_savings() {
        let input = include_str!("../../../tests/fixtures/ast_grep_lazylock_raw.txt");
        let output = filter_ast_grep(input, DEFAULT_MAX_PER_FILE, DEFAULT_MAX_TOTAL);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output.text);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        // Measured at ~84% on this fixture. Pinned well above the 20% release floor so a
        // change that quietly stops compacting is caught here rather than in a release review.
        assert!(
            savings >= 60.0,
            "ast-grep filter: expected >=60% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );

        // Every line the fixture holds is either printed or counted.
        let shown = output
            .text
            .lines()
            .filter(|l| MATCH_LINE_RE.is_match(l))
            .count();
        let total = input.lines().filter(|l| !l.trim().is_empty()).count();
        assert_eq!(
            shown + output.hidden_lines,
            total,
            "{} of {total} fixture lines went uncounted",
            total - shown - output.hidden_lines
        );
    }
}
