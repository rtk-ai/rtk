//! Deduplicates repeated log lines and shows counts instead.

use crate::core::guard::never_worse;
use crate::core::tracking;
use crate::core::truncate::{CAP_WARNINGS, reduced};
use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead};
use std::path::Path;
use std::sync::LazyLock;

static TIMESTAMP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\d{4}[-/]\d{2}[-/]\d{2}[T ]\d{2}:\d{2}:\d{2}[.,]?\d*\s*").unwrap()
});
static UUID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
        .unwrap()
});
static HEX_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"0x[0-9a-fA-F]+").unwrap());
static NUM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d{4,}\b").unwrap());
static PATH_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/[\w./\-]+").unwrap());

/// Filter and deduplicate log output
pub fn run_file(file: &Path, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Analyzing log: {}", file.display());
    }

    let content = fs::read_to_string(file)?;
    let result = analyze_logs(&content);
    let shown = never_worse(&content, &result);
    println!("{}", shown);
    timer.track(
        &format!("cat {}", file.display()),
        "rtk log",
        &content,
        shown,
    );
    Ok(())
}

/// Filter logs from stdin
pub fn run_stdin(_verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut content = String::new();
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        content.push_str(&line?);
        content.push('\n');
    }

    let result = analyze_logs(&content);
    let shown = never_worse(&content, &result);
    println!("{}", shown);

    timer.track("log (stdin)", "rtk log (stdin)", &content, shown);

    Ok(())
}

/// For use by other modules
pub fn run_stdin_str(content: &str) -> String {
    analyze_logs(content)
}

fn analyze_logs(content: &str) -> String {
    let mut result = Vec::new();
    let mut error_counts: HashMap<String, usize> = HashMap::new();
    let mut warn_counts: HashMap<String, usize> = HashMap::new();
    let mut info_counts: HashMap<String, usize> = HashMap::new();
    let mut unique_errors: Vec<String> = Vec::new();
    let mut unique_warnings: Vec<String> = Vec::new();

    // Use module-level LazyLock regexes for normalization

    for line in content.lines() {
        let line_lower = line.to_lowercase();

        // Normalize for deduplication
        let normalized =
            normalize_log_line(line, &TIMESTAMP_RE, &UUID_RE, &HEX_RE, &NUM_RE, &PATH_RE);

        // Categorize. The error bucket also covers severity labels above ERROR
        // (CRITICAL, FATAL, ALERT, EMERGENCY, SEVERE, PANIC) — these are the most
        // important lines in a log and were previously dropped as noise when they
        // didn't literally contain "error".
        if line_lower.contains("error")
            || line_lower.contains("fatal")
            || line_lower.contains("panic")
            || line_lower.contains("critical")
            || line_lower.contains("alert")
            || line_lower.contains("emerg")
            || line_lower.contains("severe")
        {
            let count = error_counts.entry(normalized.clone()).or_insert(0);
            if *count == 0 {
                unique_errors.push(line.to_string());
            }
            *count += 1;
        } else if line_lower.contains("warn") || line_lower.contains("notice") {
            let count = warn_counts.entry(normalized.clone()).or_insert(0);
            if *count == 0 {
                unique_warnings.push(line.to_string());
            }
            *count += 1;
        } else if line_lower.contains("info") {
            *info_counts.entry(normalized).or_insert(0) += 1;
        }
    }

    // Summary
    let total_errors: usize = error_counts.values().sum();
    let total_warnings: usize = warn_counts.values().sum();
    let total_info: usize = info_counts.values().sum();

    result.push("Log Summary".to_string());
    result.push(format!(
        "   [error] {} errors ({} unique)",
        total_errors,
        error_counts.len()
    ));
    result.push(format!(
        "   [warn] {} warnings ({} unique)",
        total_warnings,
        warn_counts.len()
    ));
    result.push(format!("   [info] {} info messages", total_info));
    result.push(String::new());

    // Errors with counts
    if !unique_errors.is_empty() {
        result.push("[ERRORS]".to_string());

        // Sort by count
        let mut error_list: Vec<_> = error_counts.iter().collect();
        error_list.sort_by(|a, b| b.1.cmp(a.1));

        const MAX_LOG_ERRORS: usize = CAP_WARNINGS;
        for (normalized, count) in error_list.iter().take(MAX_LOG_ERRORS) {
            // Find original message
            let original = unique_errors
                .iter()
                .find(|e| {
                    &normalize_log_line(e, &TIMESTAMP_RE, &UUID_RE, &HEX_RE, &NUM_RE, &PATH_RE)
                        == *normalized
                })
                .map(|s| s.as_str())
                .unwrap_or(normalized);

            let truncated = clip_line(original);

            if **count > 1 {
                result.push(format!("   [×{}] {}", count, truncated));
            } else {
                result.push(format!("   {}", truncated));
            }
        }

        if error_list.len() > MAX_LOG_ERRORS {
            result.push(format!(
                "   ... +{} more unique errors",
                error_list.len() - MAX_LOG_ERRORS
            ));
        }
        result.push(String::new());
    }

    // Warnings with counts
    if !unique_warnings.is_empty() {
        result.push("[WARNINGS]".to_string());

        let mut warn_list: Vec<_> = warn_counts.iter().collect();
        warn_list.sort_by(|a, b| b.1.cmp(a.1));

        // warnings are lower severity than errors — show fewer.
        const MAX_LOG_WARNS: usize = reduced(CAP_WARNINGS, 5);
        for (normalized, count) in warn_list.iter().take(MAX_LOG_WARNS) {
            let original = unique_warnings
                .iter()
                .find(|w| {
                    &normalize_log_line(w, &TIMESTAMP_RE, &UUID_RE, &HEX_RE, &NUM_RE, &PATH_RE)
                        == *normalized
                })
                .map(|s| s.as_str())
                .unwrap_or(normalized);

            let truncated = clip_line(original);

            if **count > 1 {
                result.push(format!("   [×{}] {}", count, truncated));
            } else {
                result.push(format!("   {}", truncated));
            }
        }

        if warn_list.len() > MAX_LOG_WARNS {
            result.push(format!(
                "   ... +{} more unique warnings",
                warn_list.len() - MAX_LOG_WARNS
            ));
        }
    }

    // The end of a log is where a run reports its outcome — the path it
    // wrote, the final metric, a DONE marker. None of that carries a
    // severity keyword, so the summary above drops it, and the reader is
    // left with counts but not the result. Keep the last few lines verbatim.
    let tail: Vec<&str> = content
        .lines()
        .rev()
        .filter(|l| !l.trim().is_empty())
        .take(TAIL_LINES)
        .collect();
    if content.lines().filter(|l| !l.trim().is_empty()).count() > TAIL_LINES {
        result.push(format!("[TAIL] last {} lines", tail.len()));
        for line in tail.iter().rev() {
            result.push(format!("   {}", clip_line(line.trim_end())));
        }
    }
    result.join("\n")
}

/// Longest line kept in the summary. 100 chars cut `provider=nvidia_nim` to
/// `provider=nvidia_n...` in a real daemon log — the one token the reader
/// needed. 200 keeps a typical structured line whole.
const LOG_LINE_MAX: usize = 200;
/// Raw lines kept verbatim from the end of the log.
const TAIL_LINES: usize = 5;

/// Cut a line to [`LOG_LINE_MAX`] chars, at a space when one is near the
/// cut so a `key=value` token is dropped whole rather than split.
fn clip_line(line: &str) -> String {
    let n = line.chars().count();
    if n <= LOG_LINE_MAX {
        return line.to_string();
    }
    let head: String = line.chars().take(LOG_LINE_MAX - 3).collect();
    let cut = match head.rfind(' ') {
        Some(i) if i > LOG_LINE_MAX / 2 => &head[..i],
        _ => head.as_str(),
    };
    format!("{}...", cut)
}

fn normalize_log_line(
    line: &str,
    timestamp_re: &Regex,
    uuid_re: &Regex,
    hex_re: &Regex,
    num_re: &Regex,
    path_re: &Regex,
) -> String {
    let mut normalized = timestamp_re.replace_all(line, "").to_string();
    normalized = uuid_re.replace_all(&normalized, "<UUID>").to_string();
    normalized = hex_re.replace_all(&normalized, "<HEX>").to_string();
    normalized = num_re.replace_all(&normalized, "<NUM>").to_string();
    normalized = path_re.replace_all(&normalized, "<PATH>").to_string();
    normalized.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyze_logs() {
        let logs = r#"
2024-01-01 10:00:00 ERROR: Connection failed to /api/server
2024-01-01 10:00:01 ERROR: Connection failed to /api/server
2024-01-01 10:00:02 ERROR: Connection failed to /api/server
2024-01-01 10:00:03 WARN: Retrying connection
2024-01-01 10:00:04 INFO: Connected
"#;
        let result = analyze_logs(logs);
        assert!(result.contains("×3"));
        assert!(result.contains("ERRORS"));
    }

    #[test]
    fn test_analyze_logs_extended_severity_keywords() {
        let logs = "2024-01-01 10:00:00 CRITICAL: disk full\n\
                    2024-01-01 10:00:01 ALERT: memory pressure\n\
                    2024-01-01 10:00:02 emerg: system shutdown imminent\n\
                    2024-01-01 10:00:03 SEVERE: data corruption detected\n\
                    2024-01-01 10:00:04 notice: config reloaded\n";
        let result = analyze_logs(logs);
        assert!(
            result.contains("ERRORS"),
            "critical/alert/emerg/severe should count as errors"
        );
        assert!(
            result.contains("WARNINGS"),
            "notice should count as warning"
        );
    }

    #[test]
    fn test_analyze_logs_keeps_the_outcome_in_the_tail() {
        // A training log: progress noise, then the result. Nothing in the
        // result carries a severity keyword, so before the tail section the
        // summary reported "0 errors, 1 info" and lost the path and the size.
        let logs = "steps: 10%\nsteps: 50%\nsteps: 90%\nsteps: 100% avr_loss=0.109\n2026-09-13 21:21:32 INFO model saved.\nsaving checkpoint: /home/louis/lora_out/louis_sdxl.safetensors\n/home/louis/lora_out/louis_sdxl.safetensors 85423396\nKOHYA-DONE";
        let result = analyze_logs(logs);
        assert!(result.contains("[TAIL]"));
        assert!(result.contains("/home/louis/lora_out/louis_sdxl.safetensors 85423396"));
        assert!(result.contains("avr_loss=0.109"));
        assert!(result.contains("KOHYA-DONE"));
        // A short log has no tail section — everything is already shown.
        assert!(!analyze_logs("ERROR one\nERROR two").contains("[TAIL]"));
    }

    #[test]
    fn test_clip_line_keeps_a_structured_token_whole() {
        let line = format!(
            "{} WARN anvil_router: provider failed role=\"verifier\" provider=nvidia_nim error=timeout {}",
            "2026-09-02T21:08:50.356893Z",
            "x".repeat(120)
        );
        let clipped = clip_line(&line);
        assert!(
            clipped.contains("provider=nvidia_nim"),
            "the token was split at 100 chars before: {clipped}"
        );
        assert!(clipped.ends_with("..."));
        assert!(clipped.chars().count() <= LOG_LINE_MAX);
        assert_eq!(clip_line("short"), "short");
    }

    #[test]
    fn test_analyze_logs_multibyte() {
        let logs = format!(
            "2024-01-01 10:00:00 ERROR: {} connection failed\n\
             2024-01-01 10:00:01 WARN: {} retry attempt\n",
            "ข้อผิดพลาด".repeat(15),
            "คำเตือน".repeat(15)
        );
        let result = analyze_logs(&logs);
        // Should not panic even with very long multi-byte messages
        assert!(result.contains("ERRORS"));
    }
}
