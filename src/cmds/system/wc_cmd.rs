//! Compact filter for `wc` — strips redundant paths and alignment padding.
//!
//! Compression examples:
//! - `wc file.py`     → `30L 96W 978B`
//! - `wc -l file.py`  → `30`
//! - `wc -w file.py`  → `96`
//! - `wc -c file.py`  → `978`
//! - `wc -l *.py`     → table with common path prefix stripped

use crate::core::runner::{self, RunOptions};
use crate::core::shell::display_args;
use crate::core::utils::{ChildArgExt, resolved_command};
use anyhow::Result;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("wc");
    cmd.child_args(args);

    if verbose > 0 {
        eprintln!("Running: wc {}", args.join(" "));
    }

    let mode = detect_mode(args);

    // No file operands → wc reads from stdin. Forward rtk's stdin to the child
    // so `cat file | rtk wc` counts the piped data instead of reporting zero.
    let reads_stdin = !args.iter().any(|a| !a.starts_with('-'));
    let opts = if reads_stdin {
        RunOptions::stdout_only().inherit_stdin()
    } else {
        RunOptions::stdout_only()
    };

    runner::run_filtered(
        cmd,
        "wc",
        &display_args(args),
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

fn detect_mode(args: &[String]) -> WcMode {
    let flags: Vec<&str> = args
        .iter()
        .filter(|a| a.starts_with('-'))
        .map(|s| s.as_str())
        .collect();

    if flags.is_empty() {
        return WcMode::Full;
    }

    // Collect all single-char flags (handles combined flags like -lw)
    let mut has_l = false;
    let mut has_w = false;
    let mut has_c = false;
    let mut has_m = false;
    let mut flag_count = 0;

    for flag in &flags {
        for ch in flag.chars().skip(1) {
            match ch {
                'l' => {
                    has_l = true;
                    flag_count += 1;
                }
                'w' => {
                    has_w = true;
                    flag_count += 1;
                }
                'c' => {
                    has_c = true;
                    flag_count += 1;
                }
                'm' => {
                    has_m = true;
                    flag_count += 1;
                }
                _ => {}
            }
        }
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
            let (counts, _) = split_counts(line, mode);
            if counts.is_empty() {
                line.trim().to_string()
            } else {
                counts.join(" ")
            }
        }
    }
}

/// Number of count columns wc prints for a mode, when it is known.
fn count_columns(mode: &WcMode) -> Option<usize> {
    match mode {
        WcMode::Lines | WcMode::Words | WcMode::Bytes | WcMode::Chars => Some(1),
        WcMode::Full => Some(3),
        WcMode::Mixed => None,
    }
}

/// Split a wc output line into its leading counts and the file name.
///
/// wc prints the name after the counts verbatim, so a name may contain
/// spaces and must be taken as the rest of the line, not the last token.
fn split_counts<'a>(line: &'a str, mode: &WcMode) -> (Vec<&'a str>, &'a str) {
    let max = count_columns(mode).unwrap_or(usize::MAX);
    let mut counts = Vec::new();
    let mut rest = line.trim_start();
    while counts.len() < max && !rest.is_empty() {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let token = &rest[..end];
        if token.parse::<u64>().is_err() {
            break;
        }
        counts.push(token);
        rest = rest[end..].trim_start();
    }
    (counts, rest)
}

/// Format multiple files as a compact table
fn format_multi_line(lines: &[&str], mode: &WcMode) -> String {
    let mut result = Vec::new();

    let rows: Vec<(&str, Vec<&str>, &str)> = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (counts, name) = split_counts(line, mode);
            (*line, counts, name)
        })
        .collect();

    // Find common directory prefix to shorten paths
    let paths: Vec<&str> = rows
        .iter()
        .map(|(_, _, name)| *name)
        .filter(|name| !name.is_empty() && *name != "total")
        .collect();

    let common_prefix = find_common_prefix(&paths);

    for (line, counts, name) in &rows {
        let is_total = *name == "total";

        match mode {
            WcMode::Lines | WcMode::Words | WcMode::Bytes | WcMode::Chars => {
                let count = counts.first().unwrap_or(&"0");
                if is_total {
                    result.push(format!("Σ {}", count));
                } else {
                    result.push(format!("{} {}", count, strip_prefix(name, &common_prefix)));
                }
            }
            WcMode::Full => {
                if is_total {
                    result.push(format!(
                        "Σ {}L {}W {}B",
                        counts.first().unwrap_or(&"0"),
                        counts.get(1).unwrap_or(&"0"),
                        counts.get(2).unwrap_or(&"0"),
                    ));
                } else if counts.len() == 3 && !name.is_empty() {
                    let name = strip_prefix(name, &common_prefix);
                    result.push(format!(
                        "{}L {}W {}B {}",
                        counts[0], counts[1], counts[2], name
                    ));
                } else {
                    result.push(line.trim().to_string());
                }
            }
            WcMode::Mixed => {
                if is_total {
                    result.push(format!("Σ {}", counts.join(" ")));
                } else if counts.is_empty() {
                    result.push(line.trim().to_string());
                } else if name.is_empty() {
                    result.push(counts.join(" "));
                } else {
                    let name = strip_prefix(name, &common_prefix);
                    result.push(format!("{} {}", counts.join(" "), name));
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
    fn test_multi_file_lines_names_with_spaces() {
        let raw = " 1 alpha report.txt\n 5 beta report.txt\n 2 gamma report.txt\n 8 total\n";
        let result = filter_wc_output(raw, &WcMode::Lines);
        assert_eq!(
            result,
            "1 alpha report.txt\n5 beta report.txt\n2 gamma report.txt\nΣ 8"
        );
    }

    #[test]
    fn test_multi_file_full_names_with_spaces() {
        let raw = "  3  10  60 docs/my notes.md\n  4  20  90 docs/to do.md\n  7  30 150 total\n";
        let result = filter_wc_output(raw, &WcMode::Full);
        assert_eq!(
            result,
            "3L 10W 60B my notes.md\n4L 20W 90B to do.md\nΣ 7L 30W 150B"
        );
    }

    #[test]
    fn test_multi_file_mixed_names_with_spaces() {
        let raw = "  3  10 docs/my notes.md\n  4  20 docs/to do.md\n  7  30 total\n";
        let result = filter_wc_output(raw, &WcMode::Mixed);
        assert_eq!(result, "3 10 my notes.md\n4 20 to do.md\nΣ 7 30");
    }

    #[test]
    fn test_single_file_mixed_name_with_spaces() {
        let raw = "  3  10 my notes.md\n";
        let result = filter_wc_output(raw, &WcMode::Mixed);
        assert_eq!(result, "3 10");
    }

    #[test]
    fn test_multi_file_lines_numeric_name() {
        let raw = " 1 2024\n 2 notes.md\n 3 total\n";
        let result = filter_wc_output(raw, &WcMode::Lines);
        assert_eq!(result, "1 2024\n2 notes.md\nΣ 3");
    }

    #[test]
    fn test_detect_mode_full() {
        let args: Vec<String> = vec!["file.py".into()];
        assert_eq!(detect_mode(&args), WcMode::Full);
    }

    #[test]
    fn test_detect_mode_lines() {
        let args: Vec<String> = vec!["-l".into(), "file.py".into()];
        assert_eq!(detect_mode(&args), WcMode::Lines);
    }

    #[test]
    fn test_detect_mode_mixed() {
        let args: Vec<String> = vec!["-lw".into(), "file.py".into()];
        assert_eq!(detect_mode(&args), WcMode::Mixed);
    }

    #[test]
    fn test_detect_mode_separate_flags() {
        let args: Vec<String> = vec!["-l".into(), "-w".into(), "file.py".into()];
        assert_eq!(detect_mode(&args), WcMode::Mixed);
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
