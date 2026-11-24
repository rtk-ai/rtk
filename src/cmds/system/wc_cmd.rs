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
    LINES,
    WORDS,
    MAX_LINE_LENGTH,
    FILES0_FROM,
    Flag::long("total").takes(MANDATORY),
    Flag::long("help"),
    Flag::long("version"),
];

const BSD_BYTES: Flag = Flag::short("c");
const BSD_CHARS: Flag = Flag::short("m");
const BSD_LINES: Flag = Flag::short("l");
const BSD_WORDS: Flag = Flag::short("w");
const BSD_MAX_LINE_LENGTH: Flag = Flag::short("L");

/// BSD wc (macOS, FreeBSD): `wc [--libxo] [-Lclmw] [file ...]`, short flags only, plus libxo's
/// `--libxo` taking its options as the next argument or after `=`.
const BSD_FLAGS: &[Flag] = &[
    BSD_BYTES,
    BSD_CHARS,
    BSD_LINES,
    BSD_WORDS,
    BSD_MAX_LINE_LENGTH,
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
    /// `args` with every file operand glob expanded (see `operand_glob`): the free positionals,
    /// never a flag's value such as `--files0-from`'s.
    child_args: Vec<String>,
}

fn plan_for(args: &[String], flavor: Flavor, expand: impl Fn(&str) -> Vec<String>) -> WcPlan {
    let tokens = arg_tokenizer::tokenize_grammar(args, grammar(flavor));
    let operands = tokens
        .iter()
        .filter(|t| t.is_free_positional())
        .map(|t| t.source_index)
        .collect();
    WcPlan {
        mode: detect_mode(&tokens, flavor),
        reads_stdin: reads_stdin(&tokens),
        child_args: operand_glob::expand_at(args, &operands, expand),
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
        child_args,
    } = plan_for(typed, HOST_FLAVOR, |path| {
        operand_glob::expand_operands([path])
    });

    let mut cmd = resolved_command("wc");
    cmd.args(&child_args);

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
    /// Multiple flags combined — keep compact format
    Mixed,
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
        _ => WcMode::Mixed,
    }
}

fn filter_wc_output(raw: &str, mode: &WcMode) -> String {
    let lines: Vec<&str> = raw.trim().lines().collect();

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

/// Format a single wc output line (one file or stdin)
fn format_single_line(line: &str, mode: &WcMode) -> String {
    let parts: Vec<&str> = line.split_whitespace().collect();

    match mode {
        WcMode::Lines | WcMode::Words | WcMode::Bytes | WcMode::Chars => {
            // First number is the only requested column
            parts.first().map(|s| s.to_string()).unwrap_or_default()
        }
        WcMode::Full => {
            if parts.len() >= 3 {
                format!("{}L {}W {}B", parts[0], parts[1], parts[2])
            } else {
                line.trim().to_string()
            }
        }
        WcMode::Mixed => {
            // Strip file path, keep numbers only
            if parts.len() >= 2 {
                let last_is_path = parts.last().is_some_and(|p| p.parse::<u64>().is_err());
                if last_is_path {
                    parts[..parts.len() - 1].join(" ")
                } else {
                    parts.join(" ")
                }
            } else {
                line.trim().to_string()
            }
        }
    }
}

/// Format multiple files as a compact table
fn format_multi_line(lines: &[&str], mode: &WcMode) -> String {
    let mut result = Vec::new();

    // Find common directory prefix to shorten paths
    let paths: Vec<&str> = lines
        .iter()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            parts.last().copied()
        })
        .filter(|p| *p != "total")
        .collect();

    let common_prefix = find_common_prefix(&paths);

    for line in lines {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        let is_total = parts.last().is_some_and(|p| *p == "total");

        match mode {
            WcMode::Lines | WcMode::Words | WcMode::Bytes | WcMode::Chars => {
                if is_total {
                    result.push(format!("Σ {}", parts.first().unwrap_or(&"0")));
                } else {
                    let name = strip_prefix(parts.last().unwrap_or(&""), &common_prefix);
                    result.push(format!("{} {}", parts.first().unwrap_or(&"0"), name));
                }
            }
            WcMode::Full => {
                if is_total {
                    result.push(format!(
                        "Σ {}L {}W {}B",
                        parts.first().unwrap_or(&"0"),
                        parts.get(1).unwrap_or(&"0"),
                        parts.get(2).unwrap_or(&"0"),
                    ));
                } else if parts.len() >= 4 {
                    let name = strip_prefix(parts[3], &common_prefix);
                    result.push(format!(
                        "{}L {}W {}B {}",
                        parts[0], parts[1], parts[2], name
                    ));
                } else {
                    result.push(line.trim().to_string());
                }
            }
            WcMode::Mixed => {
                if is_total {
                    let nums: Vec<&str> = parts[..parts.len() - 1].to_vec();
                    result.push(format!("Σ {}", nums.join(" ")));
                } else if parts.len() >= 2 {
                    let last_is_path = parts.last().is_some_and(|p| p.parse::<u64>().is_err());
                    if last_is_path {
                        let name = strip_prefix(parts.last().unwrap_or(&""), &common_prefix);
                        let nums: Vec<&str> = parts[..parts.len() - 1].to_vec();
                        result.push(format!("{} {}", nums.join(" "), name));
                    } else {
                        result.push(parts.join(" "));
                    }
                } else {
                    result.push(line.trim().to_string());
                }
            }
        }
    }

    result.join("\n")
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

    use crate::core::arg_tokenizer::{TakesValueRow, TokenKind, assert_takes_value_table};

    /// Plans `args` for `flavor`'s wc, marking each argument handed to the expander, so the
    /// expectations describe one wc rather than whichever one the test machine ships.
    fn plan(args: &[&str], flavor: Flavor) -> WcPlan {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        plan_for(&args, flavor, |path| vec![format!("<{path}>")])
    }

    fn mode_gnu(args: &[&str]) -> WcMode {
        plan(args, Flavor::Gnu).mode
    }

    #[test]
    fn test_grammars_match_their_tables() {
        let gnu: [TakesValueRow; 3] = [
            (TokenKind::Short, &["c", "m", "l", "w", "L"], None),
            (
                TokenKind::Long,
                &[
                    "bytes",
                    "chars",
                    "lines",
                    "words",
                    "max-line-length",
                    "help",
                    "version",
                ],
                None,
            ),
            (TokenKind::Long, &["files0-from", "total"], Some(MANDATORY)),
        ];
        assert_takes_value_table(&GNU_GRAMMAR, &gnu);
        let bsd: [TakesValueRow; 2] = [
            (TokenKind::Short, &["c", "m", "l", "w", "L"], None),
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
        assert_eq!(mode_gnu(&["-lw", "file.py"]), WcMode::Mixed);
    }

    #[test]
    fn test_detect_mode_separate_flags() {
        assert_eq!(mode_gnu(&["-l", "-w", "file.py"]), WcMode::Mixed);
    }

    #[test]
    fn test_detect_mode_reads_long_flags_by_name() {
        // A first-letter scan read `--chars` as `-c` (bytes) and `--bytes` as no flag at all.
        assert_eq!(mode_gnu(&["--chars", "f"]), WcMode::Chars);
        assert_eq!(mode_gnu(&["--bytes", "f"]), WcMode::Bytes);
        assert_eq!(mode_gnu(&["--lin", "f"]), WcMode::Lines);
        assert_eq!(mode_gnu(&["--words", "--lines", "f"]), WcMode::Mixed);
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
        assert_eq!(mode_gnu(&["-L", "f"]), WcMode::Mixed);
        assert_eq!(mode_gnu(&["-lL", "f"]), WcMode::Mixed);
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
        assert_eq!(mode_gnu(&["-cm", "f"]), WcMode::Mixed);
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
            plan(&["-l", "*.rs", "-"], Flavor::Gnu).child_args,
            ["-l", "<*.rs>", "<->"]
        );
        assert_eq!(
            plan(&["--files0-from", "list*", "--total=a*"], Flavor::Gnu).child_args,
            ["--files0-from", "list*", "--total=a*"]
        );
        assert_eq!(
            plan(&["--f", "l*", "--", "-w*"], Flavor::Gnu).child_args,
            ["--f", "l*", "--", "<-w*>"]
        );
        assert_eq!(
            plan(&["--libxo", "j*", "a*"], Flavor::Bsd).child_args,
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
