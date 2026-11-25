//! Compact filter for `wc` — strips redundant paths and alignment padding.
//!
//! Compression examples:
//! - `wc file.py`     → `30L 96W 978B`
//! - `wc -l file.py`  → `30`
//! - `wc -w file.py`  → `96`
//! - `wc -c file.py`  → `978`
//! - `wc -l *.py`     → table with common path prefix stripped

use super::ls::{Flavor, HOST_FLAVOR};
use crate::core::arg_tokenizer::{self, Abbreviation, Dialect, Flag, Grammar, Token, ValueSpec};
use crate::core::args_utils;
use crate::core::operand_glob;
use crate::core::runner::{self, RunOptions};
use crate::core::shell::display_args;
use crate::core::utils::resolved_command;
use anyhow::Result;
use std::collections::HashSet;

/// A required value. getopt hands it the next argument whatever that is, `--` included, and
/// libxo reads `--libxo`'s the same way.
const MANDATORY: ValueSpec = ValueSpec::value().claiming_dash_dash();

const BYTES: Flag = Flag::pair("c", "bytes");
const CHARS: Flag = Flag::pair("m", "chars");
const LINES: Flag = Flag::pair("l", "lines");
const WORDS: Flag = Flag::pair("w", "words");
const MAX_LINE_LENGTH: Flag = Flag::pair("L", "max-line-length");
/// The operands are the NUL-separated names in this file instead, or on stdin when it is `-`.
const FILES0_FROM: Flag = Flag::long("files0-from").takes(MANDATORY);

/// Every option GNU wc accepts, from its `--help` (coreutils 8.32, Git for Windows', and 9.4,
/// which adds `--total`). Booleans too: `getopt_long` resolves an unambiguous prefix
/// (`--t=never`, `--f list`), and a prefix is only unambiguous against the whole set.
const GNU_FLAGS: &[Flag] = &[
    BYTES,
    CHARS,
    FILES0_FROM,
    Flag::long("help"),
    LINES,
    MAX_LINE_LENGTH,
    Flag::long("total").takes(MANDATORY),
    Flag::long("version"),
    WORDS,
];

const BSD_BYTES: Flag = Flag::short("c");
const BSD_CHARS: Flag = Flag::short("m");
const BSD_LINES: Flag = Flag::short("l");
const BSD_WORDS: Flag = Flag::short("w");
const BSD_MAX_LINE_LENGTH: Flag = Flag::short("L");

/// BSD wc (macOS, FreeBSD): `wc [--libxo] [-Lclmw] [file ...]`, short flags only, plus libxo's
/// `--libxo` taking its options as the next argument or after `=`.
const BSD_FLAGS: &[Flag] = &[
    BSD_MAX_LINE_LENGTH,
    BSD_BYTES,
    BSD_LINES,
    BSD_CHARS,
    BSD_WORDS,
    Flag::long("libxo").takes(MANDATORY),
];

/// GNU wc reads its options with `getopt_long`: short flags cluster, options and operands
/// interleave, and a long option may be abbreviated to any prefix that names it alone.
const GNU_DIALECT: Dialect = Dialect {
    abbreviation: Abbreviation::UniquePrefix,
    ..Dialect::Posix
};

const GNU_GRAMMAR: Grammar = Grammar::new(GNU_DIALECT, &[GNU_FLAGS]);
/// BSD's plain `getopt` also stops at the first operand, which the dialect cannot say:
/// [`detect_mode`] applies it.
const BSD_GRAMMAR: Grammar = Grammar::new(Dialect::Posix, &[BSD_FLAGS]);

/// The grammar of the `wc` the child process will be.
fn grammar(flavor: Flavor) -> &'static Grammar {
    match flavor {
        Flavor::Gnu => &GNU_GRAMMAR,
        Flavor::Bsd => &BSD_GRAMMAR,
    }
}

/// What one `wc` invocation asks for, read with the child's grammar.
struct WcPlan {
    mode: WcMode,
    reads_stdin: bool,
    /// The indices of the file operands, which get glob expanded (see `operand_glob`): the free
    /// positionals, never a flag's value such as `--files0-from`'s.
    operands: HashSet<usize>,
}

fn plan_for(args: &[String], flavor: Flavor) -> WcPlan {
    let tokens = arg_tokenizer::tokenize_grammar(args, grammar(flavor));
    WcPlan {
        mode: detect_mode(&tokens, flavor),
        reads_stdin: reads_stdin(&tokens),
        operands: tokens
            .iter()
            .filter(|t| t.is_free_positional())
            .map(|t| t.source_index)
            .collect(),
    }
}

/// Whether the child reads rtk's stdin, which is then forwarded so `cat file | rtk wc` counts
/// the piped data instead of reporting zero. It does with no file operand, or with `-` as one;
/// under `--files0-from` (which GNU wc refuses to combine with operands) only when the last
/// one given names `-`, the names then coming from stdin.
fn reads_stdin(tokens: &[Token<'_>]) -> bool {
    if let Some(list) = tokens.iter().rev().find(|t| t.flag() == Some(&FILES0_FROM)) {
        return list.value(tokens) == Some("-");
    }
    let mut operands = tokens.iter().filter(|t| t.is_free_positional()).peekable();
    operands.peek().is_none() || operands.any(|t| t.text == "-")
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    // Restored first: the grammar reads it, and clap ate the boundary.
    let typed = &args_utils::restore_double_dash(args);
    // The child gets the expanded operands; the tracked command stays what the user typed.
    let WcPlan {
        mode,
        reads_stdin,
        operands,
    } = plan_for(typed, HOST_FLAVOR);
    let (child_args, child_globs) = operand_glob::for_child(
        typed,
        &operands,
        |path| operand_glob::expand_operands([path]),
        |expanded| operand_glob::fits(resolved_command("wc").args(expanded)),
    );

    let mut cmd = resolved_command("wc");
    child_globs.push_all(&mut cmd, &child_args);

    if verbose > 0 {
        eprintln!("Running: wc {}", child_args.join(" "));
    }

    let opts = if reads_stdin {
        RunOptions::stdout_only().inherit_stdin()
    } else {
        RunOptions::stdout_only()
    };

    runner::run_filtered(
        cmd,
        "wc",
        &display_args(typed),
        |stdout| filter_wc_output(stdout, &mode),
        opts,
    )
}

/// Which columns the user requested
#[derive(Debug, PartialEq)]
enum WcMode {
    /// Default: lines, words, bytes (3 columns)
    Full,
    /// Lines only (-l)
    Lines,
    /// Words only (-w)
    Words,
    /// Bytes only (-c)
    Bytes,
    /// Chars only (-m)
    Chars,
    /// Several columns, or `-L` alone: this many numbers, with no fixed label
    Mixed(usize),
}

impl WcMode {
    /// How many count columns `wc` prints before the name on each line.
    fn columns(&self) -> usize {
        match self {
            WcMode::Full => 3,
            WcMode::Lines | WcMode::Words | WcMode::Bytes | WcMode::Chars => 1,
            WcMode::Mixed(columns) => *columns,
        }
    }
}

/// One count column a flag asks `wc` for.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Column {
    Lines,
    Words,
    Bytes,
    Chars,
    MaxLineLength,
}

fn column(token: &Token<'_>) -> Option<Column> {
    match token.flag()? {
        &LINES | &BSD_LINES => Some(Column::Lines),
        &WORDS | &BSD_WORDS => Some(Column::Words),
        &BYTES | &BSD_BYTES => Some(Column::Bytes),
        &CHARS | &BSD_CHARS => Some(Column::Chars),
        &MAX_LINE_LENGTH | &BSD_MAX_LINE_LENGTH => Some(Column::MaxLineLength),
        _ => None,
    }
}

/// The columns `tokens` ask for, as `flavor`'s wc reads them. GNU prints every column asked
/// for, wherever the flag stands. BSD stops reading options at the first operand (a later `-l`
/// is a file name there), and its `-c` and `-m` cancel each other, the last one winning.
fn detect_mode(tokens: &[Token<'_>], flavor: Flavor) -> WcMode {
    let options = match flavor {
        Flavor::Gnu => tokens,
        Flavor::Bsd => {
            let first_operand = tokens.iter().position(|t| t.is_free_positional());
            &tokens[..first_operand.unwrap_or(tokens.len())]
        }
    };
    let mut columns: Vec<Column> = Vec::new();
    for column in options.iter().filter_map(column) {
        if flavor == Flavor::Bsd && matches!(column, Column::Bytes | Column::Chars) {
            columns.retain(|c| !matches!(c, Column::Bytes | Column::Chars));
        }
        if !columns.contains(&column) {
            columns.push(column);
        }
    }
    match columns.as_slice() {
        [] => WcMode::Full,
        [Column::Lines] => WcMode::Lines,
        [Column::Words] => WcMode::Words,
        [Column::Bytes] => WcMode::Bytes,
        [Column::Chars] => WcMode::Chars,
        // Several columns, or `-L` alone: numbers with no fixed label.
        columns => WcMode::Mixed(columns.len()),
    }
}

fn filter_wc_output(raw: &str, mode: &WcMode) -> String {
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();

    if lines.is_empty() {
        return String::new();
    }

    // Single file (one output line, no "total")
    if lines.len() == 1 {
        return format_single_line(lines[0], mode);
    }

    // Multiple files — compact table
    format_multi_line(&lines, mode)
}

/// One `wc` output line: its counts, and the name after them unless it is stdin's.
type Row<'a> = (Vec<&'a str>, Option<&'a str>);

/// One `wc` output line split into its `columns` counts and the name after them, `None` for
/// stdin's nameless line. Both GNU and BSD right-align each count and put one space before the
/// name, so the name is everything after that space, spaces included. `None` when the line does
/// not start with that many counts.
fn split_line(line: &str, columns: usize) -> Option<Row<'_>> {
    let mut counts = Vec::with_capacity(columns);
    let mut rest = line;
    for _ in 0..columns {
        rest = rest.trim_start_matches([' ', '\t']);
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if digits == 0 {
            return None;
        }
        counts.push(&rest[..digits]);
        rest = &rest[digits..];
    }
    if rest.is_empty() {
        return Some((counts, None));
    }
    let name = rest.strip_prefix([' ', '\t'])?;
    Some((counts, (!name.is_empty()).then_some(name)))
}

/// The counts of one line in `mode`'s compact form: `30L 96W 978B` for the default columns, the
/// numbers alone otherwise.
fn format_counts(counts: &[&str], mode: &WcMode) -> String {
    match (mode, counts) {
        (WcMode::Full, [lines, words, bytes]) => format!("{lines}L {words}W {bytes}B"),
        _ => counts.join(" "),
    }
}

/// Format a single wc output line (one file or stdin)
fn format_single_line(line: &str, mode: &WcMode) -> String {
    match split_line(line, mode.columns()) {
        Some((counts, _)) => format_counts(&counts, mode),
        None => line.trim().to_string(),
    }
}

/// Format multiple files as a compact table
fn format_multi_line(lines: &[&str], mode: &WcMode) -> String {
    let rows: Vec<(&str, Option<Row<'_>>)> = lines
        .iter()
        .map(|line| (*line, split_line(line, mode.columns())))
        .collect();

    // Find common directory prefix to shorten paths
    let paths: Vec<&str> = rows
        .iter()
        .filter_map(|(_, row)| row.as_ref()?.1)
        .filter(|name| *name != "total")
        .collect();

    let common_prefix = find_common_prefix(&paths);

    rows.iter()
        .map(|(line, row)| match row {
            Some((counts, Some("total"))) => format!("Σ {}", format_counts(counts, mode)),
            Some((counts, Some(name))) => format!(
                "{} {}",
                format_counts(counts, mode),
                strip_prefix(name, &common_prefix)
            ),
            Some((counts, None)) => format_counts(counts, mode),
            None => line.trim().to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Find common directory prefix among paths
fn find_common_prefix(paths: &[&str]) -> String {
    if paths.len() <= 1 {
        return String::new();
    }

    let first = paths[0];
    let prefix = if let Some(pos) = first.rfind('/') {
        &first[..=pos]
    } else {
        return String::new();
    };

    if paths.iter().all(|p| p.starts_with(prefix)) {
        return prefix.to_string();
    }

    // Try shorter prefixes by removing right-most segments
    let mut candidate = prefix.to_string();
    while !candidate.is_empty() {
        if paths.iter().all(|p| p.starts_with(&candidate)) {
            return candidate;
        }
        if let Some(pos) = candidate[..candidate.len() - 1].rfind('/') {
            candidate.truncate(pos + 1);
        } else {
            return String::new();
        }
    }
    String::new()
}

/// Strip common prefix from a path
fn strip_prefix<'a>(path: &'a str, prefix: &str) -> &'a str {
    if prefix.is_empty() {
        return path;
    }
    path.strip_prefix(prefix).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_file_full() {
        let raw = "      30      96     978 scripts/find_duplicate_attrs.py\n";
        let result = filter_wc_output(raw, &WcMode::Full);
        assert_eq!(result, "30L 96W 978B");
    }

    #[test]
    fn test_single_file_lines_only() {
        let raw = "      30 scripts/find_duplicate_attrs.py\n";
        let result = filter_wc_output(raw, &WcMode::Lines);
        assert_eq!(result, "30");
    }

    #[test]
    fn test_single_file_words_only() {
        let raw = "      96 scripts/find_duplicate_attrs.py\n";
        let result = filter_wc_output(raw, &WcMode::Words);
        assert_eq!(result, "96");
    }

    #[test]
    fn test_stdin_full() {
        let raw = "      30      96     978\n";
        let result = filter_wc_output(raw, &WcMode::Full);
        assert_eq!(result, "30L 96W 978B");
    }

    #[test]
    fn test_stdin_lines() {
        let raw = "      30\n";
        let result = filter_wc_output(raw, &WcMode::Lines);
        assert_eq!(result, "30");
    }

    #[test]
    fn test_multi_file_lines() {
        let raw = "      30 src/main.rs\n      50 src/lib.rs\n      80 total\n";
        let result = filter_wc_output(raw, &WcMode::Lines);
        assert_eq!(result, "30 main.rs\n50 lib.rs\nΣ 80");
    }

    #[test]
    fn test_multi_file_full() {
        let raw = "      30      96     978 src/main.rs\n      50     120    1500 src/lib.rs\n      80     216    2478 total\n";
        let result = filter_wc_output(raw, &WcMode::Full);
        assert_eq!(
            result,
            "30L 96W 978B main.rs\n50L 120W 1500B lib.rs\nΣ 80L 216W 2478B"
        );
    }

    /// Git for Windows' wc (coreutils 8.32) over `my file.txt` (`ab\n`) and `a.txt`
    /// (`abcde\n`): the name is everything after the counts, not the last word.
    #[test]
    fn test_multi_file_name_with_a_space_is_kept_whole() {
        let bytes = "3 my file.txt\n6 a.txt\n9 total\n";
        assert_eq!(
            filter_wc_output(bytes, &WcMode::Bytes),
            "3 my file.txt\n6 a.txt\nΣ 9"
        );
        let lines = "1 my file.txt\n1 a.txt\n2 total\n";
        assert_eq!(
            filter_wc_output(lines, &WcMode::Lines),
            "1 my file.txt\n1 a.txt\nΣ 2"
        );
        let full = "1 1 3 my file.txt\n1 1 6 a.txt\n2 2 9 total\n";
        assert_eq!(
            filter_wc_output(full, &WcMode::Full),
            "1L 1W 3B my file.txt\n1L 1W 6B a.txt\nΣ 2L 2W 9B"
        );
        let mixed = "1 2 my file.txt\n1 5 a.txt\n2 5 total\n";
        assert_eq!(
            filter_wc_output(mixed, &WcMode::Mixed(2)),
            "1 2 my file.txt\n1 5 a.txt\nΣ 2 5"
        );
    }

    #[test]
    fn test_padded_columns_and_a_leading_space_in_a_name() {
        // GNU pads every count to the widest; the one space before the name is the separator.
        let raw = "  1 my file.txt\n 10  lead.txt\n 11 total\n";
        assert_eq!(
            filter_wc_output(raw, &WcMode::Lines),
            "1 my file.txt\n10  lead.txt\nΣ 11"
        );
    }

    #[test]
    fn test_a_name_made_of_digits_is_not_read_as_a_count() {
        // `-l` prints one count, so `2024 01.log` is the name, not two more columns.
        let raw = "5 2024 01.log\n7 b\n12 total\n";
        assert_eq!(
            filter_wc_output(raw, &WcMode::Lines),
            "5 2024 01.log\n7 b\nΣ 12"
        );
    }

    use crate::core::arg_tokenizer::{TakesValueRow, TokenKind, assert_takes_value_table};

    /// Plans `args` for `flavor`'s wc, so the expectations describe one wc rather than
    /// whichever one the test machine ships.
    fn plan(args: &[&str], flavor: Flavor) -> WcPlan {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        plan_for(&args, flavor)
    }

    /// `args` as `flavor`'s wc gets them, each file operand marked as the expander saw it.
    fn child_args(args: &[&str], flavor: Flavor) -> Vec<String> {
        let operands = plan(args, flavor).operands;
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        operand_glob::expand_at(&args, &operands, |path| vec![format!("<{path}>")])
    }

    fn mode_gnu(args: &[&str]) -> WcMode {
        plan(args, Flavor::Gnu).mode
    }

    #[test]
    fn test_grammars_match_their_tables() {
        let gnu: [TakesValueRow; 3] = [
            (TokenKind::Short, &["L", "c", "l", "m", "w"], None),
            (
                TokenKind::Long,
                &[
                    "bytes",
                    "chars",
                    "help",
                    "lines",
                    "max-line-length",
                    "version",
                    "words",
                ],
                None,
            ),
            (TokenKind::Long, &["files0-from", "total"], Some(MANDATORY)),
        ];
        assert_takes_value_table(&GNU_GRAMMAR, &gnu);
        let bsd: [TakesValueRow; 2] = [
            (TokenKind::Short, &["L", "c", "l", "m", "w"], None),
            (TokenKind::Long, &["libxo"], Some(MANDATORY)),
        ];
        assert_takes_value_table(&BSD_GRAMMAR, &bsd);
    }

    #[test]
    fn test_detect_mode_full() {
        assert_eq!(mode_gnu(&["file.py"]), WcMode::Full);
    }

    #[test]
    fn test_detect_mode_lines() {
        assert_eq!(mode_gnu(&["-l", "file.py"]), WcMode::Lines);
    }

    #[test]
    fn test_detect_mode_mixed() {
        assert_eq!(mode_gnu(&["-lw", "file.py"]), WcMode::Mixed(2));
    }

    #[test]
    fn test_detect_mode_separate_flags() {
        assert_eq!(mode_gnu(&["-l", "-w", "file.py"]), WcMode::Mixed(2));
    }

    #[test]
    fn test_detect_mode_reads_long_flags_by_name() {
        // A first-letter scan read `--chars` as `-c` (bytes) and `--bytes` as no flag at all.
        assert_eq!(mode_gnu(&["--chars", "f"]), WcMode::Chars);
        assert_eq!(mode_gnu(&["--bytes", "f"]), WcMode::Bytes);
        assert_eq!(mode_gnu(&["--lin", "f"]), WcMode::Lines);
        assert_eq!(mode_gnu(&["--words", "--lines", "f"]), WcMode::Mixed(2));
    }

    #[test]
    fn test_detect_mode_ignores_flag_values_and_operands() {
        // `--total`'s value and a `--`-protected `-l` are not count flags.
        assert_eq!(mode_gnu(&["--total", "only", "-w", "f"]), WcMode::Words);
        assert_eq!(mode_gnu(&["--", "-l"]), WcMode::Full);
        // A repeated flag is still one column.
        assert_eq!(mode_gnu(&["-l", "-l", "f"]), WcMode::Lines);
    }

    #[test]
    fn test_detect_mode_max_line_length_is_a_column() {
        assert_eq!(mode_gnu(&["-L", "f"]), WcMode::Mixed(1));
        assert_eq!(mode_gnu(&["-lL", "f"]), WcMode::Mixed(2));
    }

    #[test]
    fn test_detect_mode_gnu_permutes_bsd_stops_at_the_first_operand() {
        assert_eq!(mode_gnu(&["f", "-l"]), WcMode::Lines);
        assert_eq!(plan(&["f", "-l"], Flavor::Bsd).mode, WcMode::Full);
        assert_eq!(plan(&["-l", "f"], Flavor::Bsd).mode, WcMode::Lines);
    }

    #[test]
    fn test_detect_mode_bsd_bytes_and_chars_cancel() {
        assert_eq!(plan(&["-cm", "f"], Flavor::Bsd).mode, WcMode::Chars);
        assert_eq!(plan(&["-m", "-c", "f"], Flavor::Bsd).mode, WcMode::Bytes);
        assert_eq!(mode_gnu(&["-cm", "f"]), WcMode::Mixed(2));
    }

    #[test]
    fn test_reads_stdin_only_without_a_file_operand() {
        let reads = |args: &[&str]| plan(args, Flavor::Gnu).reads_stdin;
        assert!(reads(&[]));
        assert!(reads(&["-l"]));
        assert!(reads(&["-l", "-"]));
        assert!(!reads(&["-l", "f"]));
        // A dash-led operand after `--` is still a file.
        assert!(!reads(&["--", "-l"]));
        // `--total`'s value is not a file operand.
        assert!(reads(&["--total", "always"]));
        // `--files0-from` reads names from its file, or from stdin for `-`; the last one wins.
        assert!(!reads(&["--files0-from", "list"]));
        assert!(reads(&["--files0-from=-"]));
        assert!(reads(&["--f", "list", "--files0-from", "-"]));
        assert!(!reads(&["--files0-from", "-", "--files0-from=list"]));
    }

    #[test]
    fn test_only_file_operands_are_expanded() {
        assert_eq!(
            child_args(&["-l", "*.rs", "-"], Flavor::Gnu),
            ["-l", "<*.rs>", "<->"]
        );
        assert_eq!(
            child_args(&["--files0-from", "list*", "--total=a*"], Flavor::Gnu),
            ["--files0-from", "list*", "--total=a*"]
        );
        assert_eq!(
            child_args(&["--f", "l*", "--", "-w*"], Flavor::Gnu),
            ["--f", "l*", "--", "<-w*>"]
        );
        assert_eq!(
            child_args(&["--libxo", "j*", "a*"], Flavor::Bsd),
            ["--libxo", "j*", "<a*>"]
        );
    }

    #[test]
    fn test_common_prefix() {
        let paths = vec!["src/main.rs", "src/lib.rs", "src/utils.rs"];
        assert_eq!(find_common_prefix(&paths), "src/");
    }

    #[test]
    fn test_no_common_prefix() {
        let paths = vec!["main.rs", "lib.rs"];
        assert_eq!(find_common_prefix(&paths), "");
    }

    #[test]
    fn test_deep_common_prefix() {
        let paths = vec!["src/cmd/wc.rs", "src/cmd/ls.rs"];
        assert_eq!(find_common_prefix(&paths), "src/cmd/");
    }

    #[test]
    fn test_empty() {
        let raw = "";
        let result = filter_wc_output(raw, &WcMode::Full);
        assert_eq!(result, "");
    }
}
