/// Compact filter for `wc` — strips redundant paths and alignment padding.
///
/// Compression examples:
/// - `wc file.py`     → `30L 96W 978B`
/// - `wc -l file.py`  → `30`
/// - `wc -w file.py`  → `96`
/// - `wc -c file.py`  → `978`
/// - `wc -l *.py`     → table with common path prefix stripped
use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;
#[cfg(windows)]
use crate::core::utils::{resolve_host_command, HostCommand};
use anyhow::Result;
#[cfg(windows)]
use std::io::Read;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    #[cfg(windows)]
    if matches!(resolve_host_command("wc"), HostCommand::Missing)
        && windows_wc_args_supported(args)
    {
        return run_windows_native(args, verbose);
    }

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

    runner::run_ai_from_filter(
        cmd,
        "wc",
        &args.join(" "),
        crate::core::ai_output::BudgetClass::Collection,
        |stdout| filter_wc_output(stdout, &mode),
        opts,
    )
}

#[cfg(windows)]
fn windows_wc_args_supported(args: &[String]) -> bool {
    args.iter().filter(|arg| arg.starts_with('-')).all(|arg| {
        matches!(
            arg.as_str(),
            "-l" | "-w" | "-c" | "-m" | "--lines" | "--words" | "--bytes" | "--chars"
        ) || (arg.starts_with('-')
            && !arg.starts_with("--")
            && arg[1..].chars().all(|flag| matches!(flag, 'l' | 'w' | 'c' | 'm'))
            && !arg[1..].is_empty())
    })
}

#[cfg(windows)]
fn run_windows_native(args: &[String], verbose: u8) -> Result<i32> {
    let mode = detect_mode(args);
    let files = args
        .iter()
        .filter(|arg| !arg.starts_with('-'))
        .map(String::as_str)
        .collect::<Vec<_>>();
    let file_count = files.len();
    let mut rows = Vec::new();
    let mut total = (0usize, 0usize, 0usize, 0usize);

    if files.is_empty() {
        let mut bytes = Vec::new();
        std::io::stdin()
            .read_to_end(&mut bytes)
            .map_err(|error| anyhow::anyhow!("wc: failed to read stdin: {error}"))?;
        let counts = windows_counts(&bytes);
        rows.push(format_windows_counts(counts, &mode, None));
    } else {
        for file in files {
            let bytes = std::fs::read(file)
                .map_err(|error| anyhow::anyhow!("wc: {}: {}", file, error))?;
            let counts = windows_counts(&bytes);
            rows.push(format_windows_counts(counts, &mode, Some(file)));
            total.0 += counts.0;
            total.1 += counts.1;
            total.2 += counts.2;
            total.3 += counts.3;
        }
        if file_count > 1 {
            rows.push(format_windows_counts(total, &mode, Some("total")));
        }
    }

    let rendered = rows.join("\n");
    if verbose > 0 {
        eprintln!(
            "[rtk-debug] windows-wc backend=native files={} mode={:?}",
            file_count,
            mode
        );
    }
    println!("{}", rendered);
    let timer = crate::core::tracking::TimedExecution::start();
    timer.track(
        &format!("wc {}", args.join(" ")),
        &format!("rtk wc {}", args.join(" ")),
        &rendered,
        &rendered,
    );
    Ok(0)
}

#[cfg(windows)]
fn windows_counts(bytes: &[u8]) -> (usize, usize, usize, usize) {
    let text = String::from_utf8_lossy(bytes);
    (
        bytes.iter().filter(|byte| **byte == b'\n').count(),
        text.split_whitespace().count(),
        bytes.len(),
        text.chars().count(),
    )
}

#[cfg(windows)]
fn format_windows_counts(
    counts: (usize, usize, usize, usize),
    mode: &WcMode,
    name: Option<&str>,
) -> String {
    let values = match mode {
        WcMode::Lines => format!("{}", counts.0),
        WcMode::Words => format!("{}", counts.1),
        WcMode::Bytes => format!("{}", counts.2),
        WcMode::Chars => format!("{}", counts.3),
        WcMode::Full | WcMode::Mixed => {
            if matches!(mode, WcMode::Mixed) {
                format!("{} {} {} {}", counts.0, counts.1, counts.2, counts.3)
            } else {
                format!("{}L {}W {}B", counts.0, counts.1, counts.2)
            }
        }
    };
    match name {
        Some(name) => format!("{values} {name}"),
        None => values,
    }
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

    // Match long options as complete tokens. Scanning the letters in
    // `--bytes`/`--chars` would classify those names as unrelated short flags.
    let mut has_l = false;
    let mut has_w = false;
    let mut has_c = false;
    let mut has_m = false;

    for flag in &flags {
        match *flag {
            "--lines" => {
                has_l = true;
                continue;
            }
            "--words" => {
                has_w = true;
                continue;
            }
            "--bytes" => {
                has_c = true;
                continue;
            }
            "--chars" => {
                has_m = true;
                continue;
            }
            _ if flag.starts_with("--") => continue,
            _ => {}
        }
        for ch in flag.chars().skip(1) {
            match ch {
                'l' => has_l = true,
                'w' => has_w = true,
                'c' => has_c = true,
                'm' => has_m = true,
                _ => {}
            }
        }
    }

    let flag_count = [has_l, has_w, has_c, has_m]
        .into_iter()
        .filter(|present| *present)
        .count();
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
    fn test_detect_mode_long_names() {
        assert_eq!(detect_mode(&["--bytes".into()]), WcMode::Bytes);
        assert_eq!(detect_mode(&["--chars".into()]), WcMode::Chars);
        assert_eq!(detect_mode(&["--lines".into()]), WcMode::Lines);
        assert_eq!(detect_mode(&["--words".into()]), WcMode::Words);
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
