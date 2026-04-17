//! SCP output compression.
//!
//! Drops carriage-return progress updates, keeps final transfer summaries, and
//! preserves warning/error lines.

use crate::core::runner::{self, RunOptions};
use crate::core::utils::{resolved_command, strip_ansi, truncate};
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;

const MAX_LINE_CHARS: usize = 200;
const HEAD_LINES: usize = 20;
const TAIL_LINES: usize = 20;
const MAX_RETAINED_LINES: usize = HEAD_LINES + TAIL_LINES;

lazy_static! {
    static ref SCP_PROGRESS: Regex =
        Regex::new(r"^(?P<name>.+?)\s+(?P<pct>\d{1,3})%\s+(?P<size>\S+)(?:\s+.*)?$").unwrap();
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("scp");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: scp {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "scp",
        &args.join(" "),
        filter_scp_output,
        RunOptions::with_tee("scp"),
    )
}

fn filter_scp_output(output: &str) -> String {
    let mut lines = Vec::new();
    let normalized = strip_ansi(output).replace('\r', "\n");

    for raw_line in normalized.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        if is_error_line(line) {
            push_dedup(&mut lines, truncate(line, MAX_LINE_CHARS));
            continue;
        }

        if let Some(summary) = summarize_progress_line(line) {
            push_dedup(&mut lines, summary);
            continue;
        }

        if is_progress_line(line) {
            continue;
        }

        push_dedup(&mut lines, truncate(line, MAX_LINE_CHARS));
    }

    if lines.is_empty() {
        return "scp: ok".to_string();
    }

    if lines.len() <= MAX_RETAINED_LINES {
        return lines.join("\n");
    }

    let mut result = Vec::new();
    let head = HEAD_LINES.min(lines.len());
    let tail = TAIL_LINES.min(lines.len().saturating_sub(head));

    result.extend(lines.iter().take(head).cloned());
    result.push(format!(
        "... +{} more lines",
        lines.len().saturating_sub(head + tail)
    ));
    let tail_lines: Vec<String> = lines.iter().rev().take(tail).cloned().collect();
    result.extend(tail_lines.into_iter().rev());

    result.join("\n")
}

fn summarize_progress_line(line: &str) -> Option<String> {
    let caps = SCP_PROGRESS.captures(line)?;
    if &caps["pct"] != "100" {
        return None;
    }

    let name = caps["name"].trim();
    if name.is_empty() {
        return None;
    }

    let size = normalize_size(&caps["size"]);
    Some(format!("{} {} transferred", name, size))
}

fn normalize_size(size: &str) -> String {
    let trimmed = size.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        return format!("{}B", trimmed);
    }

    if matches!(trimmed.chars().last(), Some('K' | 'M' | 'G' | 'T' | 'P')) {
        return format!("{}B", trimmed);
    }

    trimmed.to_string()
}

fn is_progress_line(line: &str) -> bool {
    SCP_PROGRESS.is_match(line)
}

fn is_error_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.starts_with("scp:")
        || lower.contains("permission denied")
        || lower.contains("no such file")
        || lower.contains("lost connection")
        || lower.contains("connection closed")
        || lower.contains("connection refused")
        || lower.contains("host key verification failed")
        || lower.contains("protocol error")
        || lower.contains("warning:")
}

fn push_dedup(lines: &mut Vec<String>, line: String) {
    if lines.last().map(|last| last == &line).unwrap_or(false) {
        return;
    }
    lines.push(line);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_scp_progress_updates_to_summary() {
        let output = "\
sending file modes: C0644 123 file.txt\n\
file.txt                                      10%  12KB   1.2MB/s   00:01\r\
file.txt                                      72%  12KB   1.2MB/s   00:00\r\
file.txt                                     100%  12KB   1.2MB/s   00:00\n";

        let result = filter_scp_output(output);

        assert!(result.contains("sending file modes: C0644 123 file.txt"));
        assert!(result.contains("file.txt 12KB transferred"));
        assert!(!result.contains("10%"));
        assert!(!result.contains("72%"));
        assert!(!result.contains("100%"));
    }

    #[test]
    fn test_filter_scp_preserves_errors_and_warnings() {
        let output = "\
Warning: Permanently added 'host' (ED25519) to the list of known hosts.\n\
scp: /remote/path/file.txt: No such file or directory\n\
file.txt                                     100%   1KB   1.0MB/s   00:00\n\
scp: lost connection\n";

        let result = filter_scp_output(output);

        assert!(result.contains("Warning: Permanently added"));
        assert!(result.contains("No such file or directory"));
        assert!(result.contains("lost connection"));
        assert!(result.contains("file.txt 1KB transferred"));
    }

    #[test]
    fn test_filter_scp_error_with_percent_is_not_summarized() {
        let output = "scp: remote reported 100% quota usage; Permission denied\n";

        let result = filter_scp_output(output);

        assert_eq!(
            result,
            "scp: remote reported 100% quota usage; Permission denied"
        );
    }

    #[test]
    fn test_filter_scp_returns_ok_when_only_progress_remains() {
        let output = "\
file.txt                                      10%  12KB   1.2MB/s   00:01\r\
file.txt                                      43%  12KB   1.2MB/s   00:00\r";

        let result = filter_scp_output(output);

        assert_eq!(result, "scp: ok");
    }

    #[test]
    fn test_filter_scp_caps_long_output() {
        let mut output = String::new();
        for i in 0..60 {
            output.push_str(&format!("scp: note {}\n", i));
        }

        let result = filter_scp_output(&output);

        assert!(result.contains("... +20 more lines"));
        assert!(result.contains("scp: note 0"));
        assert!(result.contains("scp: note 59"));
    }
}
