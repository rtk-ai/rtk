/// Compact filter for `wc` — strips redundant paths and alignment padding.
///
/// Compression examples:
/// - `wc file.py`     → `30L 96W 978B`
/// - `wc -l file.py`  → `30`
/// - `wc -w file.py`  → `96`
/// - `wc -c file.py`  → `978`
/// - `wc -l *.py`     → table with common path prefix stripped
use crate::core::runner::{self, RunOptions};
use crate::core::tracking::TimedExecution;
use crate::core::utils::{resolved_command, tool_exists};
use anyhow::Result;
use std::io::Read;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mode = detect_mode(args);

    // On Windows (and any host lacking the Unix `wc` binary) fall back to a
    // native Rust implementation so `rtk wc` works without coreutils installed.
    if !tool_exists("wc") {
        return run_native(args, &mode, verbose);
    }

    let mut cmd = resolved_command("wc");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: wc {}", args.join(" "));
    }

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

/// Byte/word/line/char counts for a chunk of input.
struct Counts {
    lines: u64,
    words: u64,
    bytes: u64,
    chars: u64,
}

impl Counts {
    fn from_bytes(data: &[u8]) -> Self {
        let bytes = data.len() as u64;
        let lines = data.iter().filter(|&&b| b == b'\n').count() as u64;
        let text = String::from_utf8_lossy(data);
        let words = text.split_whitespace().count() as u64;
        let chars = text.chars().count() as u64;
        Counts {
            lines,
            words,
            bytes,
            chars,
        }
    }

    fn add(&mut self, other: &Counts) {
        self.lines += other.lines;
        self.words += other.words;
        self.bytes += other.bytes;
        self.chars += other.chars;
    }
}

/// Which numeric columns to emit, in `wc`'s fixed order (lines, words, chars, bytes).
struct WcCols {
    lines: bool,
    words: bool,
    chars: bool,
    bytes: bool,
}

impl WcCols {
    fn from_args(args: &[String]) -> Self {
        let mut l = false;
        let mut w = false;
        let mut c = false;
        let mut m = false;
        let mut any = false;
        for flag in args.iter().filter(|a| a.starts_with('-')) {
            for ch in flag.chars().skip(1) {
                match ch {
                    'l' => {
                        l = true;
                        any = true;
                    }
                    'w' => {
                        w = true;
                        any = true;
                    }
                    'c' => {
                        c = true;
                        any = true;
                    }
                    'm' => {
                        m = true;
                        any = true;
                    }
                    _ => {}
                }
            }
        }
        // No selecting flag → default columns: lines, words, bytes.
        if !any {
            return WcCols {
                lines: true,
                words: true,
                chars: false,
                bytes: true,
            };
        }
        WcCols {
            lines: l,
            words: w,
            chars: m,
            bytes: c,
        }
    }

    /// Render the requested counts as a space-separated number string.
    fn render(&self, c: &Counts) -> String {
        let mut nums: Vec<String> = Vec::new();
        if self.lines {
            nums.push(c.lines.to_string());
        }
        if self.words {
            nums.push(c.words.to_string());
        }
        if self.chars {
            nums.push(c.chars.to_string());
        }
        if self.bytes {
            nums.push(c.bytes.to_string());
        }
        nums.join(" ")
    }
}

/// Native `wc` implementation used when the Unix binary is unavailable
/// (e.g. on stock Windows). Produces output in the same shape the spawn
/// path emits, then reuses [`filter_wc_output`].
fn run_native(args: &[String], mode: &WcMode, verbose: u8) -> Result<i32> {
    let timer = TimedExecution::start();
    let cols = WcCols::from_args(args);

    let files: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();

    let mut raw = String::new();
    let mut exit_code = 0;

    if files.is_empty() {
        // No file operands → read from stdin.
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .map_err(|e| anyhow::anyhow!("wc: failed to read stdin: {e}"))?;
        let counts = Counts::from_bytes(&buf);
        raw.push_str(&cols.render(&counts));
        raw.push('\n');
    } else {
        let mut total = Counts {
            lines: 0,
            words: 0,
            bytes: 0,
            chars: 0,
        };
        for file in &files {
            match std::fs::read(file) {
                Ok(data) => {
                    let counts = Counts::from_bytes(&data);
                    total.add(&counts);
                    raw.push_str(&cols.render(&counts));
                    raw.push(' ');
                    raw.push_str(file);
                    raw.push('\n');
                }
                Err(e) => {
                    eprintln!("wc: {file}: {e}");
                    exit_code = 1;
                }
            }
        }
        if files.len() > 1 {
            raw.push_str(&cols.render(&total));
            raw.push_str(" total\n");
        }
    }

    let filtered = filter_wc_output(&raw, mode);

    if verbose > 0 {
        eprintln!("wc (native): {} files", files.len());
    }

    println!("{filtered}");
    timer.track(&format!("wc {}", args.join(" ")), "rtk wc", &raw, &filtered);
    Ok(exit_code)
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

    #[test]
    fn test_counts_from_bytes() {
        let c = Counts::from_bytes(b"hello world\nsecond line\n");
        assert_eq!(c.lines, 2);
        assert_eq!(c.words, 4);
        assert_eq!(c.bytes, 24);
        assert_eq!(c.chars, 24);
    }

    #[test]
    fn test_counts_no_trailing_newline() {
        let c = Counts::from_bytes(b"one two three");
        assert_eq!(c.lines, 0);
        assert_eq!(c.words, 3);
        assert_eq!(c.bytes, 13);
    }

    #[test]
    fn test_wccols_default_full() {
        let cols = WcCols::from_args(&["file.txt".into()]);
        let c = Counts {
            lines: 10,
            words: 20,
            bytes: 100,
            chars: 95,
        };
        // Default → lines words bytes (no chars).
        assert_eq!(cols.render(&c), "10 20 100");
    }

    #[test]
    fn test_wccols_lines_only() {
        let cols = WcCols::from_args(&["-l".into(), "f".into()]);
        let c = Counts {
            lines: 42,
            words: 0,
            bytes: 0,
            chars: 0,
        };
        assert_eq!(cols.render(&c), "42");
    }

    #[test]
    fn test_wccols_chars_with_m() {
        let cols = WcCols::from_args(&["-m".into(), "f".into()]);
        let c = Counts {
            lines: 0,
            words: 0,
            bytes: 100,
            chars: 95,
        };
        assert_eq!(cols.render(&c), "95");
    }
}
