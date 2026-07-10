/// Compact filter for `wc` — strips redundant paths and alignment padding.
///
/// Compression examples:
/// - `wc file.py`     → `30L 96W 978B`
/// - `wc -l file.py`  → `30`
/// - `wc -w file.py`  → `96`
/// - `wc -c file.py`  → `978`
/// - `wc -l *.py`     → table with common path prefix stripped
#[cfg(any(target_os = "windows", test))]
use anyhow::anyhow;
use anyhow::Result;
#[cfg(not(target_os = "windows"))]
use crate::core::runner::{self, RunOptions};
#[cfg(not(target_os = "windows"))]
use crate::core::utils::resolved_command;
#[cfg(target_os = "windows")]
use std::io::Read;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    #[cfg(target_os = "windows")]
    {
        run_native(args, verbose)
    }

    #[cfg(not(target_os = "windows"))]
    {
        run_external(args, verbose)
    }
}

#[cfg(not(target_os = "windows"))]
fn run_external(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("wc");
    for arg in args {
        cmd.arg(arg);
    }

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
        &args.join(" "),
        |stdout| filter_wc_output(stdout, &mode),
        opts,
    )
}

#[cfg(target_os = "windows")]
fn run_native(args: &[String], verbose: u8) -> Result<i32> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_native_help();
        return Ok(0);
    }
    if args.iter().any(|a| a == "--version") {
        println!("rtk wc {}", env!("CARGO_PKG_VERSION"));
        return Ok(0);
    }

    if verbose > 0 {
        eprintln!("Running native wc {}", args.join(" "));
    }

    let mode = detect_mode(args);
    let operands = match file_operands(args) {
        Ok(operands) => operands,
        Err(err) => {
            eprintln!("rtk wc: {err}");
            return Ok(2);
        }
    };
    let columns = requested_columns(args);
    let needs_chars = columns.contains(&WcColumn::Chars);

    let raw = match build_native_output(&operands, &mode, &columns, needs_chars) {
        Ok(raw) => raw,
        Err(err) => {
            eprintln!("rtk wc: {err}");
            return Ok(2);
        }
    };

    let filtered = filter_wc_output(&raw, &mode);
    if !filtered.is_empty() {
        println!("{filtered}");
    }
    Ok(0)
}

#[cfg(target_os = "windows")]
fn build_native_output(
    operands: &[String],
    mode: &WcMode,
    columns: &[WcColumn],
    needs_chars: bool,
) -> Result<String> {
    if operands.is_empty() {
        let mut bytes = Vec::new();
        std::io::stdin().read_to_end(&mut bytes)?;
        let stats = count_bytes(&bytes);
        format_native_line(&stats, None, mode, columns)
    } else {
        let mut lines = Vec::new();
        let mut total = WcTotals::default();

        for path in operands {
            let bytes = std::fs::read(path).map_err(|err| anyhow!("{}: {}", path, err))?;
            let stats = count_bytes(&bytes);
            total.add(&stats, needs_chars)?;
            lines.push(format_native_line(&stats, Some(path), mode, columns)?);
        }

        if operands.len() > 1 {
            lines.push(format_native_line(
                &total.into_stats(),
                Some("total"),
                mode,
                columns,
            )?);
        }
        Ok(lines.join("\n"))
    }
}

#[cfg(target_os = "windows")]
fn print_native_help() {
    println!(
        "Word/line/byte count with compact output (native Windows)\n\n\
Usage: rtk wc [OPTIONS] [FILES]...\n\n\
Options:\n  -l        count lines\n  -w        count words\n  -c        count bytes\n  -m        count UTF-8 characters\n  -h, --help     Print help\n      --version  Print version"
    );
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

#[cfg(any(target_os = "windows", test))]
#[derive(Debug, PartialEq)]
struct WcStats {
    lines: usize,
    words: usize,
    bytes: usize,
    chars: std::result::Result<usize, String>,
}

#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq)]
enum WcColumn {
    Lines,
    Words,
    Bytes,
    Chars,
}

#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Default)]
struct WcTotals {
    lines: usize,
    words: usize,
    bytes: usize,
    chars: usize,
}

#[cfg(any(target_os = "windows", test))]
impl WcTotals {
    fn add(&mut self, stats: &WcStats, needs_chars: bool) -> Result<()> {
        self.lines += stats.lines;
        self.words += stats.words;
        self.bytes += stats.bytes;
        if needs_chars {
            self.chars += stats.chars.as_ref().map_err(|err| anyhow!(err.clone()))?;
        }
        Ok(())
    }

    fn into_stats(self) -> WcStats {
        WcStats {
            lines: self.lines,
            words: self.words,
            bytes: self.bytes,
            chars: Ok(self.chars),
        }
    }
}

#[cfg(any(target_os = "windows", test))]
fn count_bytes(bytes: &[u8]) -> WcStats {
    let lines = bytes.iter().filter(|b| **b == b'\n').count();
    let words = match std::str::from_utf8(bytes) {
        Ok(text) => text.split_whitespace().count(),
        Err(_) => bytes
            .split(|b| b.is_ascii_whitespace())
            .filter(|part| !part.is_empty())
            .count(),
    };
    let chars = std::str::from_utf8(bytes)
        .map(|text| text.chars().count())
        .map_err(|err| format!("invalid UTF-8 for -m: {err}"));

    WcStats {
        lines,
        words,
        bytes: bytes.len(),
        chars,
    }
}

#[cfg(any(target_os = "windows", test))]
fn file_operands(args: &[String]) -> Result<Vec<String>> {
    let mut operands = Vec::new();
    for arg in args {
        if arg.starts_with('-') {
            validate_native_flag(arg)?;
        } else {
            operands.push(arg.clone());
        }
    }
    Ok(operands)
}

#[cfg(any(target_os = "windows", test))]
fn validate_native_flag(flag: &str) -> Result<()> {
    if flag == "--" {
        return Ok(());
    }
    if !flag.starts_with('-') || flag == "-" {
        return Ok(());
    }
    if flag.starts_with("--") {
        return Err(anyhow!(
            "unsupported wc flag '{flag}' on Windows native path; use rtk proxy wc ..."
        ));
    }
    if flag.chars().skip(1).all(|ch| matches!(ch, 'l' | 'w' | 'c' | 'm')) {
        Ok(())
    } else {
        Err(anyhow!(
            "unsupported wc flag '{flag}' on Windows native path; use rtk proxy wc ..."
        ))
    }
}

#[cfg(any(target_os = "windows", test))]
fn format_native_line(
    stats: &WcStats,
    path: Option<&str>,
    mode: &WcMode,
    columns: &[WcColumn],
) -> Result<String> {
    let mut fields: Vec<String> = match mode {
        WcMode::Lines => vec![stats.lines.to_string()],
        WcMode::Words => vec![stats.words.to_string()],
        WcMode::Bytes => vec![stats.bytes.to_string()],
        WcMode::Chars => vec![stats
            .chars
            .as_ref()
            .map_err(|err| anyhow!(err.clone()))?
            .to_string()],
        WcMode::Full => vec![
            stats.lines.to_string(),
            stats.words.to_string(),
            stats.bytes.to_string(),
        ],
        WcMode::Mixed => columns
            .iter()
            .map(|column| match column {
                WcColumn::Lines => Ok(stats.lines.to_string()),
                WcColumn::Words => Ok(stats.words.to_string()),
                WcColumn::Bytes => Ok(stats.bytes.to_string()),
                WcColumn::Chars => Ok(stats
                    .chars
                    .as_ref()
                    .map_err(|err| anyhow!(err.clone()))?
                    .to_string()),
            })
            .collect::<Result<Vec<_>>>()?,
    };
    if let Some(path) = path {
        fields.push(path.to_string());
    }
    Ok(fields.join(" "))
}

#[cfg(any(target_os = "windows", test))]
fn requested_columns(args: &[String]) -> Vec<WcColumn> {
    let mut has_lines = false;
    let mut has_words = false;
    let mut has_bytes = false;
    let mut has_chars = false;
    for arg in args
        .iter()
        .filter(|arg| arg.starts_with('-') && !arg.starts_with("--"))
    {
        for ch in arg.chars().skip(1) {
            match ch {
                'l' => has_lines = true,
                'w' => has_words = true,
                'c' => has_bytes = true,
                'm' => has_chars = true,
                _ => {}
            }
        }
    }

    let mut columns = Vec::new();
    if has_lines {
        columns.push(WcColumn::Lines);
    }
    if has_words {
        columns.push(WcColumn::Words);
    }
    if has_chars {
        columns.push(WcColumn::Chars);
    }
    if has_bytes {
        columns.push(WcColumn::Bytes);
    }
    if columns.is_empty() {
        vec![WcColumn::Lines, WcColumn::Words, WcColumn::Bytes]
    } else {
        columns
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
    fn test_native_count_full_uses_byte_count() {
        let stats = count_bytes(b"hello world\nsecond line\n");
        assert_eq!(stats.lines, 2);
        assert_eq!(stats.words, 4);
        assert_eq!(stats.bytes, 24);
        assert_eq!(stats.chars, Ok(24));
    }

    #[test]
    fn test_native_count_words_invalid_utf8_uses_ascii_whitespace() {
        let stats = count_bytes(b"alpha\xff beta\tgamma\n");
        assert_eq!(stats.words, 3);
        assert_eq!(stats.bytes, 18);
    }

    #[test]
    fn test_native_count_chars_invalid_utf8_errors() {
        let err = count_bytes(b"alpha\xff beta").chars.unwrap_err();
        assert!(err.to_string().contains("invalid UTF-8"));
    }

    #[test]
    fn test_native_wc_rejects_unknown_flags_with_exit_two() {
        let code = run_native(&["-z".to_string()], 0).unwrap();
        assert_eq!(code, 2);
    }

    #[test]
    fn test_native_mixed_output_respects_requested_columns() {
        let args = vec!["-cw".to_string(), "file.txt".to_string()];
        let stats = count_bytes(b"one two\n");
        let line = format_native_line(
            &stats,
            Some("file.txt"),
            &WcMode::Mixed,
            &requested_columns(&args),
        )
        .unwrap();
        assert_eq!(line, "2 8 file.txt");
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
