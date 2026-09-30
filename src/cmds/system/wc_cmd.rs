/// Compact filter for `wc` — strips redundant paths and alignment padding.
///
/// Compression examples:
/// - `wc file.py`     → `30L 96W 978B`
/// - `wc -l file.py`  → `30`
/// - `wc -w file.py`  → `96`
/// - `wc -c file.py`  → `978`
/// - `wc -l *.py`     → table with common path prefix stripped
use crate::core::arg_tokenizer::{self, Dialect, LongOptions, Token, TokenKind, ValueSpec};
use crate::core::child_command::{OperandGrammar, OperandSplit, SplitArgv};
use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;
use anyhow::Result;

/// Every long option GNU wc accepts (`wc --help`, coreutils 9.10). A unique prefix
/// (`--files0`, `--tot`) is read as the option it abbreviates, as `getopt_long` reads it; a bare
/// flag that is neither one of these nor such a prefix leaves the operands unbounded.
const WC_LONG_OPTIONS: LongOptions<'static> = LongOptions::new(&[
    "bytes",
    "chars",
    "debug",
    "files0-from",
    "help",
    "lines",
    "max-line-length",
    "total",
    "version",
    "words",
]);

/// wc's value-taking flags: `--files0-from F` and `--total WHEN`, both also written attached,
/// and abbreviated as wc takes them (`--tot never`). Every short flag is boolean.
fn wc_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    let long = (kind == TokenKind::Long)
        .then(|| arg_tokenizer::resolve_long(name, &WC_LONG_OPTIONS))
        .flatten();
    // A required value is whatever argument comes next, a literal `--` included, as
    // `getopt` reads it.
    matches!(long, Some("files0-from" | "total")).then(|| ValueSpec::value().claiming_dash_dash())
}

fn wc_tokens<T: AsRef<str>>(args: &[T]) -> Vec<Token<'_>> {
    arg_tokenizer::tokenize_grammar(args, &wc_takes_value, Dialect::Posix)
}

/// wc's grammar: its operands are the free positionals.
pub(crate) struct WcGrammar;

/// What rtk reads from wc's flags in the same pass.
pub(crate) struct WcArgs {
    mode: WcMode,
    reads_stdin: bool,
}

impl OperandGrammar for WcGrammar {
    type Parsed = WcArgs;

    fn split<T: AsRef<str>>(&self, args: &[T]) -> (OperandSplit, WcArgs) {
        let tokens = wc_tokens(args);
        let split = OperandSplit::free_positionals(&tokens, &WC_LONG_OPTIONS);
        // wc reads stdin with no operand, and for an operand `-`. An unbounded split may
        // have taken an operand for a flag value, and wc ignores stdin when it has a file.
        let reads_stdin = !split.bounded
            || split.indices.is_empty()
            || split
                .indices
                .iter()
                .any(|&i| args.get(i).is_some_and(|arg| arg.as_ref() == "-"));
        let parsed = WcArgs {
            mode: detect_mode(&tokens),
            reads_stdin,
        };
        (split, parsed)
    }
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let (argv, WcArgs { mode, reads_stdin }) = SplitArgv::new(&WcGrammar, args);
    let mut cmd = resolved_command("wc");
    cmd.split_args(&argv);

    if verbose > 0 {
        eprintln!("Running: wc {}", args.join(" "));
    }

    // Forward rtk's stdin to a child that reads it, so `cat file | rtk wc`
    // counts the piped data instead of reporting zero.
    let opts = if reads_stdin {
        RunOptions::stdout_only().inherit_stdin()
    } else {
        RunOptions::stdout_only()
    };

    runner::run_filtered(
        cmd,
        "wc",
        &args.join(" "),
        |stdout| filter_wc_output(stdout, &mode),
        opts,
    )
}

/// Which columns the user requested
#[derive(Debug, PartialEq)]
pub(crate) enum WcMode {
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

fn detect_mode(tokens: &[Token<'_>]) -> WcMode {
    let mut has_l = false;
    let mut has_w = false;
    let mut has_c = false;
    let mut has_m = false;
    let mut flag_count = 0;

    // Each counting flag: a short letter (clusters split), or a long option
    // resolved by prefix as wc resolves it (`--lin` is `--lines`).
    for token in tokens {
        let name = match token.kind {
            TokenKind::Short => Some(token.text),
            TokenKind::Long => arg_tokenizer::resolve_long(token.text, &WC_LONG_OPTIONS),
            _ => None,
        };
        let column = match name {
            Some("l" | "lines") => &mut has_l,
            Some("w" | "words") => &mut has_w,
            Some("c" | "bytes") => &mut has_c,
            Some("m" | "chars") => &mut has_m,
            _ => continue,
        };
        *column = true;
        flag_count += 1;
    }

    if flag_count == 0 {
        return WcMode::Full;
    }
    if flag_count > 1 {
        return WcMode::Mixed;
    }

    if has_l {
        WcMode::Lines
    } else if has_w {
        WcMode::Words
    } else if has_c {
        WcMode::Bytes
    } else if has_m {
        WcMode::Chars
    } else {
        WcMode::Full
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

    #[test]
    fn test_detect_mode_full() {
        let args: Vec<String> = vec!["file.py".into()];
        assert_eq!(detect_mode(&wc_tokens(&args)), WcMode::Full);
    }

    #[test]
    fn test_detect_mode_lines() {
        let args: Vec<String> = vec!["-l".into(), "file.py".into()];
        assert_eq!(detect_mode(&wc_tokens(&args)), WcMode::Lines);
    }

    #[test]
    fn test_detect_mode_mixed() {
        let args: Vec<String> = vec!["-lw".into(), "file.py".into()];
        assert_eq!(detect_mode(&wc_tokens(&args)), WcMode::Mixed);
    }

    #[test]
    fn test_detect_mode_separate_flags() {
        let args: Vec<String> = vec!["-l".into(), "-w".into(), "file.py".into()];
        assert_eq!(detect_mode(&wc_tokens(&args)), WcMode::Mixed);
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

    fn operands<'a>(args: &[&'a str]) -> (Vec<&'a str>, bool) {
        let (split, _) = WcGrammar.split(args);
        (
            split.indices.iter().map(|&i| args[i]).collect(),
            split.bounded,
        )
    }

    #[test]
    fn a_flag_value_is_not_an_operand() {
        // `--files0-from f*` reads its list from the file `f*`: never globbed.
        assert_eq!(operands(&["--files0-from", "f*"]), (vec![], true));
        assert_eq!(
            operands(&["--total", "never", "-l", "*.rs"]),
            (vec!["*.rs"], true)
        );
        assert_eq!(
            operands(&["-l", "a.rs", "--", "-b.rs"]),
            (vec!["a.rs", "-b.rs"], true)
        );
    }

    #[test]
    fn a_required_value_takes_a_literal_double_dash() {
        assert_eq!(operands(&["--files0-from", "--", "f*"]), (vec!["f*"], true));
        assert_eq!(
            operands(&["-l", "--total", "--", "a.txt"]),
            (vec!["a.txt"], true)
        );
    }

    #[test]
    fn an_abbreviated_value_flag_takes_its_value() {
        // GNU wc reads `--files0 f*` as `--files0-from f*` and `--tot never` as `--total`.
        assert_eq!(operands(&["--files0", "f*"]), (vec![], true));
        assert_eq!(operands(&["-l", "--tot", "never"]), (vec![], true));
        let (_, parsed) = WcGrammar.split(&["-l", "--tot", "never"]);
        assert!(parsed.reads_stdin);
        // A flag wc does not have leaves the operands unbounded.
        assert!(!operands(&["--frob", "f*"]).1);
    }

    #[test]
    fn detect_mode_reads_long_flags_and_ignores_flag_values() {
        let mode = |v: &[&str]| detect_mode(&wc_tokens(v));
        assert_eq!(mode(&["--lines", "f"]), WcMode::Lines);
        assert_eq!(mode(&["--chars", "f"]), WcMode::Chars);
        assert_eq!(mode(&["-l", "--total", "never"]), WcMode::Lines);
        assert_eq!(mode(&["--files0-from", "-lw"]), WcMode::Full);
        // GNU wc takes a unique prefix of a long option.
        assert_eq!(mode(&["--lin", "f"]), WcMode::Lines);
        assert_eq!(mode(&["--word", "f"]), WcMode::Words);
        assert_eq!(mode(&["--byt", "f"]), WcMode::Bytes);
    }

    #[test]
    fn no_operand_or_a_dash_operand_means_stdin() {
        let reads_stdin = |v: &[&str]| WcGrammar.split(v).1.reads_stdin;
        // `printf 'a\nb\n' | rtk wc -l --total never` must count the pipe.
        assert!(reads_stdin(&["-l", "--total", "never"]));
        // `printf 'x\ny\n' | rtk wc -l -`, and the same after `--`.
        assert!(reads_stdin(&["-l", "-"]));
        assert!(reads_stdin(&["-l", "--", "-"]));
        assert!(reads_stdin(&["-l", "f.txt", "-"]));
        assert!(!reads_stdin(&["-l", "f.txt"]));
        // `--frob` is no wc flag, so `f.txt` may be its value.
        assert!(reads_stdin(&["-l", "--frob", "f.txt"]));
    }
}
