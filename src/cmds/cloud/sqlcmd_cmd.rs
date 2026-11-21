//! SQL Server client (`sqlcmd`) output compression, for the ODBC client and
//! the Go client (`go-sqlcmd`).
//!
//! The default output is fixed-width and nothing in it says where a value's own
//! spaces end and the padding begins, so no cell is trimmed. The filter drops
//! each result set's dash separator row, caps long result sets behind a
//! recovery hint and leaves every other byte alone. A result set is recognised
//! only when every row is exactly as wide as its separator row; any other shape
//! (embedded newline, wrapped row, glued message) is returned unchanged.

use crate::core::arg_tokenizer::{self, Dialect, Token, TokenKind, ValueSpec};
use crate::core::runner::{self, RunMode, RunOptions};
use crate::core::shell::display_args;
use crate::core::tee::force_tee_hint;
use crate::core::truncate::CAP_LIST;
use crate::core::utils::{ChildArgExt, resolved_command};
use anyhow::Result;
use regex::Regex;
use std::io::IsTerminal;
use std::sync::LazyLock;

const MAX_TABLE_ROWS: usize = CAP_LIST;

/// Hidden rows are recovered from the stored raw output under this label.
const RECOVERY_SLUG: &str = "sqlcmd";

static SEPARATOR_ROW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^-+( -+)*$").unwrap());
// The ODBC client prints "(1 rows affected)", the Go client "(1 row affected)".
static ROWS_AFFECTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\(\d+ rows? affected\)$").unwrap());

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let shown_args = display_args(args);
    let how = plan(args, std::io::stdin().is_terminal());

    let mut cmd = resolved_command("sqlcmd");
    cmd.child_args(args);

    if verbose > 0 {
        eprintln!("Running: sqlcmd {}", shown_args);
    }

    match how {
        Plan::Passthrough(reason) => {
            if verbose > 0 {
                eprintln!("sqlcmd passthrough: {}", reason);
            }
            runner::run(
                cmd,
                "sqlcmd",
                &shown_args,
                RunMode::Passthrough,
                RunOptions::default(),
            )
        }
        // stdin stays attached so piped queries work. A failed run is printed raw.
        Plan::Filter => runner::run_filtered(
            cmd,
            "sqlcmd",
            &shown_args,
            filter_sqlcmd_output,
            RunOptions::stdout_only()
                .early_exit_on_failure()
                .inherit_stdin(),
        ),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Plan {
    Filter,
    Passthrough(&'static str),
}

/// Flags of sqlcmd's `-?` that take a value, so the value is not read as an
/// operand. `-N`, `-r`, `-k`, `-p`, `-X` and `-L` take an attached value or none.
/// A long flag missing here leaves its value as an operand, which hands the run through.
fn takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    match (kind, name) {
        (
            TokenKind::Short,
            "S" | "U" | "P" | "H" | "d" | "l" | "t" | "h" | "s" | "w" | "a" | "c" | "q" | "Q" | "m"
            | "V" | "i" | "o" | "y" | "Y" | "v" | "K" | "J" | "F" | "z" | "Z" | "f",
        )
        | (
            TokenKind::Long,
            "server" | "user-name" | "password" | "database-name" | "query" | "input-file"
            | "variables",
        ) => Some(ValueSpec::value()),
        (TokenKind::Short, "N" | "r" | "k" | "p" | "X" | "L") => Some(ValueSpec::attached_only()),
        _ => None,
    }
}

fn tokenize(args: &[String]) -> Vec<Token<'_>> {
    arg_tokenizer::tokenize_grammar(args, &takes_value, Dialect::Posix)
}

/// A flag that asks for output this filter would damage, or keeps sqlcmd interactive.
fn is_passthrough_flag(token: &Token<'_>) -> bool {
    matches!(
        (token.kind, token.text),
        (
            TokenKind::Short,
            "q" | "h" | "s" | "W" | "w" | "y" | "Y" | "o" | "L"
        ) | (
            TokenKind::Long,
            "initial-query"
                | "headers"
                | "column-separator"
                | "trim-spaces"
                | "screen-width"
                | "variable-type-width"
                | "fixed-type-width"
                | "output-file"
                | "list-servers"
                | "vertical"
        )
    )
}

fn plan(args: &[String], stdin_is_terminal: bool) -> Plan {
    let tokens = tokenize(args);
    let own = arg_tokenizer::before_dashdash(&tokens);

    // `-v var=value [var=value ...]` takes several space-separated values.
    let mut reading_variables = false;
    let mut batch = false;
    for token in own {
        if is_passthrough_flag(token) {
            return Plan::Passthrough("flag changes the output format or keeps the session open");
        }
        if token.kind == TokenKind::Positional {
            if token.is_free_positional() && !reading_variables {
                return Plan::Passthrough("not a plain sqlcmd invocation (subcommand or operand)");
            }
            continue;
        }
        batch |= matches!(
            (token.kind, token.text),
            (TokenKind::Short, "Q" | "i") | (TokenKind::Long, "query" | "input-file")
        );
        reading_variables = matches!(
            (token.kind, token.text),
            (TokenKind::Short, "v") | (TokenKind::Long, "variables")
        );
    }
    if arg_tokenizer::has_dashdash(&tokens) {
        return Plan::Passthrough("`--` is not part of sqlcmd's grammar");
    }
    if !batch && stdin_is_terminal {
        return Plan::Passthrough("interactive session");
    }
    Plan::Filter
}

fn filter_sqlcmd_output(output: &str) -> String {
    let Some(compact) = compact_result_sets(output, MAX_TABLE_ROWS) else {
        return output.to_string();
    };
    if compact.hidden_rows == 0 {
        return compact.text;
    }
    // Rows are only hidden when the agent can get them back.
    match force_tee_hint(output, RECOVERY_SLUG) {
        Some(hint) => runner::with_hint(&compact.text, Some(&hint)).into_owned(),
        None => compact_result_sets(output, usize::MAX)
            .map(|full| full.text)
            .unwrap_or_else(|| output.to_string()),
    }
}

struct Compact {
    text: String,
    hidden_rows: usize,
}

/// A line's text without its terminator.
fn content(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

fn width(line: &str) -> usize {
    content(line).chars().count()
}

/// True when `lines[at]` is a header and `lines[at + 1]` its separator row: the
/// same width, dash groups separated by single spaces, and the header leaves
/// a space at every gap between groups.
fn is_header_and_separator(lines: &[&str], at: usize) -> bool {
    let (Some(header), Some(separator)) = (lines.get(at), lines.get(at + 1)) else {
        return false;
    };
    let (header, separator) = (content(header), content(separator));
    if !SEPARATOR_ROW.is_match(separator) || header.chars().count() != separator.chars().count() {
        return false;
    }
    header
        .chars()
        .zip(separator.chars())
        .all(|(h, s)| s != ' ' || h == ' ')
}

/// Drops the separator row of every result set and keeps at most `row_cap`
/// data rows of each, counting what it hides. `None` means the output has a
/// shape the filter does not trust, and the caller prints it as it is.
fn compact_result_sets(output: &str, row_cap: usize) -> Option<Compact> {
    let lines: Vec<&str> = output.split_inclusive('\n').collect();
    let mut text = String::with_capacity(output.len());
    let mut hidden_rows = 0;
    let mut at = 0;

    while at < lines.len() {
        if !is_header_and_separator(&lines, at) {
            text.push_str(lines[at]);
            at += 1;
            continue;
        }

        let separator = lines[at + 1];
        let table_width = width(separator);
        let mut end = at + 2;
        while end < lines.len() && width(lines[end]) == table_width {
            // Another result set whose header is as wide as this one and that
            // follows with no blank line: the two cannot be told apart from rows.
            if SEPARATOR_ROW.is_match(content(lines[end])) {
                return None;
            }
            end += 1;
        }
        let rows = &lines[at + 2..end];

        // The clients end a result set with an empty line or the end of the
        // output. Any other line of a different width is a value with a
        // newline in it, a wrapped row or a glued message.
        match lines.get(end) {
            None => {}
            Some(next) if content(next).is_empty() => {}
            Some(_) => return None,
        }
        // With no rows, a "(0 rows affected)" line confirms this is a result
        // set and not two lines of PRINT text.
        if rows.is_empty() {
            let confirmed = lines.get(end).is_some()
                && lines
                    .get(end + 1)
                    .is_some_and(|footer| ROWS_AFFECTED.is_match(content(footer)));
            if !confirmed {
                text.push_str(lines[at]);
                at += 1;
                continue;
            }
        }

        text.push_str(lines[at]);
        for row in rows.iter().take(row_cap) {
            text.push_str(row);
        }
        if rows.len() > row_cap {
            let hidden = rows.len() - row_cap;
            hidden_rows += hidden;
            let eol = if lines[at].ends_with("\r\n") {
                "\r\n"
            } else {
                "\n"
            };
            // The last row may have no terminator of its own.
            if !text.ends_with('\n') {
                text.push_str(eol);
            }
            text.push_str(&format!("... +{} more rows{}", hidden, eol));
        }
        at = end;
    }

    Some(Compact { text, hidden_rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ODBC_TYPES: &str = include_str!("../../../tests/fixtures/sqlcmd/odbc_types_raw.txt");
    const ODBC_MULTI: &str = include_str!("../../../tests/fixtures/sqlcmd/odbc_multi_raw.txt");
    const GO_MULTI: &str = include_str!("../../../tests/fixtures/sqlcmd/go_multi_raw.txt");
    const ODBC_ERR_MID: &str = include_str!("../../../tests/fixtures/sqlcmd/odbc_err_mid_raw.txt");
    const ODBC_LARGE: &str = include_str!("../../../tests/fixtures/sqlcmd/odbc_large_raw.txt");

    const ONE_ROW: &str = "id         \n-----------\n          1\n\n(1 rows affected)\n";

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
    }

    fn compact(raw: &str) -> String {
        compact_result_sets(raw, usize::MAX)
            .expect("fixture has a shape the filter trusts")
            .text
    }

    /// The raw lines minus the separator rows, which is all an uncapped filter
    /// may remove.
    fn without_separator_rows(raw: &str) -> String {
        raw.split_inclusive('\n')
            .filter(|line| !SEPARATOR_ROW.is_match(content(line)))
            .collect()
    }

    fn saved(raw: &str, filtered: &str) -> f64 {
        100.0 - filtered.len() as f64 / raw.len() as f64 * 100.0
    }

    #[test]
    fn only_the_separator_rows_are_removed() {
        for raw in [ODBC_TYPES, ODBC_MULTI, GO_MULTI, ODBC_ERR_MID, ODBC_LARGE] {
            let out = compact(raw);
            assert_eq!(out, without_separator_rows(raw));
            assert!(out.len() < raw.len());
        }
    }

    #[test]
    fn cells_and_messages_stay_byte_for_byte() {
        let out = compact(ODBC_TYPES);
        for row in ODBC_TYPES.lines().skip(2).take(5) {
            assert!(out.contains(&format!("{row}\n")), "row changed: {row:?}");
        }
        assert!(out.contains("  Dana  ") && out.contains("NULL") && out.contains("Chloé Müller"));
        assert!(out.ends_with("\n(5 rows affected)\n"));
        assert!(compact(GO_MULTI).contains("(1 row affected)"));
        assert_eq!(compact(ODBC_MULTI).matches("rows affected)").count(), 5);
    }

    #[test]
    fn rows_before_an_error_and_the_error_both_survive() {
        let out = compact(ODBC_ERR_MID);
        assert!(out.contains("          1\n"));
        assert!(out.ends_with(
            "Msg 208, Level 16, State 1, Server sqlhost, Line 1\nInvalid object name 'dbo.nope'.\n"
        ));
        let error = "Msg 156, Level 15, State 1, Server s, Line 1\nIncorrect syntax near 'x'.\n";
        assert_eq!(filter_sqlcmd_output(error), error);
    }

    #[test]
    fn print_text_between_result_sets_is_untouched() {
        let raw = format!("starting\n{ONE_ROW}done\n{ONE_ROW}");
        assert_eq!(compact(&raw), without_separator_rows(&raw));
        let two_lines = "Title\n-----\n";
        assert_eq!(filter_sqlcmd_output(two_lines), two_lines);
        assert_eq!(filter_sqlcmd_output(""), "");
    }

    #[test]
    fn shapes_the_filter_does_not_trust_are_returned_unchanged() {
        let cases = [
            // A value with a newline in it.
            "id         \n-----------\n          1\nsecond\n\n(1 rows affected)\n",
            // `-W` output: rows narrower than the separator.
            "id customer\n-- --------\n1  Alice\n\n(1 rows affected)\n",
            // `SET NOCOUNT ON` in the ODBC client: the next header follows with no blank line.
            "id         \n-----------\n          1\nid         \n-----------\n          2\n",
        ];
        for raw in cases {
            assert!(compact_result_sets(raw, usize::MAX).is_none(), "{raw:?}");
            assert_eq!(filter_sqlcmd_output(raw), raw);
        }
    }

    #[test]
    fn crlf_and_a_missing_final_newline_are_kept() {
        let crlf = "id         \r\n-----------\r\n          1\r\n\r\n(1 rows affected)\r\n";
        assert_eq!(
            filter_sqlcmd_output(crlf),
            "id         \r\n          1\r\n\r\n(1 rows affected)\r\n"
        );
        let raw = "id         \n-----------\n          1\n          2\n          3";
        assert_eq!(
            compact(raw),
            "id         \n          1\n          2\n          3"
        );
        let capped = compact_result_sets(raw, 2).expect("trusted shape");
        assert_eq!(
            capped.text,
            "id         \n          1\n          2\n... +1 more rows\n"
        );
    }

    #[test]
    fn long_result_sets_are_capped_and_the_shown_rows_are_unchanged() {
        let capped = compact_result_sets(ODBC_LARGE, MAX_TABLE_ROWS).expect("trusted shape");
        assert_eq!(capped.hidden_rows, 10);
        assert!(capped.text.contains("... +10 more rows\n"));
        assert!(capped.text.ends_with("\n(30 rows affected)\n"));
        let raw_rows: Vec<&str> = ODBC_LARGE.lines().skip(2).take(MAX_TABLE_ROWS).collect();
        let shown: Vec<&str> = capped.text.lines().skip(1).take(MAX_TABLE_ROWS).collect();
        assert_eq!(raw_rows, shown);
    }

    #[test]
    fn the_cap_applies_to_each_result_set_and_keeps_what_follows() {
        let rows = |n: usize| (1..=n).map(|i| format!("{i:>11}\n")).collect::<String>();
        let header = "id         \n-----------\n";
        let raw = format!(
            "{header}{}\n(21 rows affected)\nafter\n{header}{}\n(20 rows affected)\nMsg 208, Level 16, State 1\n",
            rows(21),
            rows(20)
        );
        let capped = compact_result_sets(&raw, MAX_TABLE_ROWS).expect("trusted shape");
        assert_eq!(capped.hidden_rows, 1);
        assert!(capped.text.contains("(21 rows affected)\nafter\n"));
        assert!(
            capped
                .text
                .ends_with("(20 rows affected)\nMsg 208, Level 16, State 1\n")
        );
    }

    #[test]
    fn savings_are_measured_on_real_output_and_never_negative() {
        for raw in [ODBC_TYPES, ODBC_MULTI, GO_MULTI] {
            assert!(filter_sqlcmd_output(raw).len() < raw.len());
        }
        let capped = compact_result_sets(ODBC_LARGE, MAX_TABLE_ROWS).expect("trusted shape");
        // The caller appends the recovery hint; budget for its length.
        let shown = format!("{}[+10 hidden: rtk recall 0123456789abcdef]\n", capped.text);
        assert!(saved(ODBC_LARGE, &shown) >= 20.0);
    }

    #[test]
    fn batch_runs_and_piped_input_are_filtered() {
        for list in [
            &["-S", "srv", "-U", "sa", "-Q", "select 1"][..],
            &["-S", "srv", "-Qselect 1"],
            &["-S", "srv", "-C", "-d", "db", "-b", "-i", "q.sql"],
            &["--server", "srv", "--query", "select 1"],
            &["-S", "srv", "-v", "a=1", "b=2", "-i", "q.sql"],
            &["-S", "srv", "-N", "-C", "-Q", "select 1"],
            // The query text holds -W, -s and -h as words; they are values.
            &["-Q", "select '-W -s ,' -h", "-d", "mydb", "-S", "srv"],
        ] {
            assert_eq!(plan(&args(list), false), Plan::Filter, "{list:?}");
            assert_eq!(plan(&args(list), true), Plan::Filter, "{list:?}");
        }
        let login = args(&["-S", "srv", "-U", "sa"]);
        assert_eq!(plan(&login, false), Plan::Filter);
    }

    #[test]
    fn formats_filtering_would_damage_and_other_sessions_are_handed_through() {
        for list in [
            &["-Q", "select 1", "-h", "-1"][..],
            &["-Q", "select 1", "-h-1"],
            &["-Q", "select 1", "--headers", "-1"],
            &["-Q", "select 1", "-s", ","],
            &["-Q", "select 1", "-W"],
            &["-Q", "select 1", "-w", "200"],
            &["-Q", "select 1", "-y", "0"],
            &["-Q", "select 1", "-Y20"],
            &["-Q", "select 1", "-o", "out.txt"],
            &["-Q", "select 1", "--vertical"],
            &["-q", "select 1"],
            &["-L"],
            // The Go client's subcommands.
            &["query", "select 1"],
            &["config", "view"],
        ] {
            assert!(
                matches!(plan(&args(list), false), Plan::Passthrough(_)),
                "{list:?}"
            );
        }
        for list in [&["-S", "srv", "-U", "sa"][..], &[]] {
            assert!(matches!(plan(&args(list), true), Plan::Passthrough(_)));
        }
    }
}
