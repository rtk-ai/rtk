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
    timer.track(&super::read::cat_label(file), "rtk log", &content, shown);
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

        // Most frequent first. `unique_errors` is in first-seen order and the
        // sort is stable, so equal counts keep input order instead of the
        // HashMap's random one (which also decided which errors the cap kept).
        let mut error_list: Vec<(&str, usize)> = unique_errors
            .iter()
            .map(|e| {
                let normalized =
                    normalize_log_line(e, &TIMESTAMP_RE, &UUID_RE, &HEX_RE, &NUM_RE, &PATH_RE);
                (e.as_str(), error_counts[&normalized])
            })
            .collect();
        error_list.sort_by_key(|e| std::cmp::Reverse(e.1));

        const MAX_LOG_ERRORS: usize = CAP_WARNINGS;
        for (original, count) in error_list.iter().take(MAX_LOG_ERRORS) {
            let truncated = if original.len() > 100 {
                let t: String = original.chars().take(97).collect();
                format!("{}...", t)
            } else {
                original.to_string()
            };

            if *count > 1 {
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

        let mut warn_list: Vec<(&str, usize)> = unique_warnings
            .iter()
            .map(|w| {
                let normalized =
                    normalize_log_line(w, &TIMESTAMP_RE, &UUID_RE, &HEX_RE, &NUM_RE, &PATH_RE);
                (w.as_str(), warn_counts[&normalized])
            })
            .collect();
        warn_list.sort_by_key(|e| std::cmp::Reverse(e.1));

        // warnings are lower severity than errors — show fewer.
        const MAX_LOG_WARNS: usize = reduced(CAP_WARNINGS, 5);
        for (original, count) in warn_list.iter().take(MAX_LOG_WARNS) {
            let truncated = if original.len() > 100 {
                let t: String = original.chars().take(97).collect();
                format!("{}...", t)
            } else {
                original.to_string()
            };

            if *count > 1 {
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

    result.join("\n")
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

    #[test]
    fn test_analyze_logs_ties_keep_input_order() {
        // 15 distinct errors and 8 distinct warnings, each seen once: every count
        // ties, so the lines shown under the caps must be the first ones in the
        // log, in order, on every run.
        let names = [
            "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india",
            "juliet", "kilo", "lima", "mike", "november", "oscar",
        ];
        let mut logs = String::new();
        for n in &names {
            logs.push_str(&format!("ERROR: {} failed\n", n));
        }
        for n in names.iter().take(8) {
            logs.push_str(&format!("WARN: {} slow\n", n));
        }
        let result = analyze_logs(&logs);
        let expected: Vec<String> = names
            .iter()
            .take(10)
            .map(|n| format!("   ERROR: {} failed", n))
            .collect();
        let shown: Vec<&str> = result
            .lines()
            .filter(|l| l.starts_with("   ERROR: "))
            .collect();
        assert_eq!(shown, expected);
        assert!(result.contains("... +5 more unique errors"));
        let warns: Vec<&str> = result
            .lines()
            .filter(|l| l.starts_with("   WARN: "))
            .collect();
        let expected: Vec<String> = names
            .iter()
            .take(5)
            .map(|n| format!("   WARN: {} slow", n))
            .collect();
        assert_eq!(warns, expected);
        assert!(result.contains("... +3 more unique warnings"));
    }
}
