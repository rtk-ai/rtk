//! Deduplicates repeated log lines and shows counts instead.

use crate::core::tracking;
use crate::core::utils::truncate;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead};
use std::path::Path;

lazy_static! {
    static ref TIMESTAMP_RE: Regex =
        Regex::new(r"^\d{4}[-/]\d{2}[-/]\d{2}[T ]\d{2}:\d{2}:\d{2}[.,]?\d*\s*").unwrap();
    static ref UUID_RE: Regex =
        Regex::new(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
            .unwrap();
    static ref HEX_RE: Regex = Regex::new(r"0x[0-9a-fA-F]+").unwrap();
    static ref NUM_RE: Regex = Regex::new(r"\b\d{4,}\b").unwrap();
    static ref PATH_RE: Regex = Regex::new(r"/[\w./\-]+").unwrap();
}

/// Filter and deduplicate log output
pub fn run_file(file: &Path, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Analyzing log: {}", file.display());
    }

    let content = fs::read_to_string(file)
        .with_context(|| format!("Failed to read log file: {}", file.display()))?;
    let result = analyze_logs(&content);
    println!("{}", result);
    timer.track(
        &format!("cat {}", file.display()),
        "rtk log",
        &content,
        &result,
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
    println!("{}", result);

    timer.track("log (stdin)", "rtk log (stdin)", &content, &result);

    Ok(())
}

/// For use by other modules
pub fn run_stdin_str(content: &str) -> String {
    analyze_logs(content)
}

fn analyze_logs(content: &str) -> String {
    let mut result = Vec::new();
    let mut critical_counts: HashMap<String, usize> = HashMap::new();
    let mut error_counts: HashMap<String, usize> = HashMap::new();
    let mut warn_counts: HashMap<String, usize> = HashMap::new();
    let mut info_total: usize = 0;
    // DEBUG is high-volume: show count only, no per-line detail block
    let mut debug_total: usize = 0;
    let mut critical_originals: HashMap<String, String> = HashMap::new();
    let mut error_originals: HashMap<String, String> = HashMap::new();
    let mut warn_originals: HashMap<String, String> = HashMap::new();

    for line in content.lines() {
        let line_lower = line.to_lowercase();
        let normalized =
            normalize_log_line(line, &TIMESTAMP_RE, &UUID_RE, &HEX_RE, &NUM_RE, &PATH_RE);

        // CRITICAL/ALERT/EMERGENCY checked first (highest severity)
        if line_lower.contains("critical")
            || line_lower.contains("alert")
            || line_lower.contains("emergency")
        {
            let count = critical_counts.entry(normalized.clone()).or_insert(0);
            if *count == 0 {
                critical_originals.insert(normalized.clone(), line.to_string());
            }
            *count += 1;
        } else if line_lower.contains("error")
            || line_lower.contains("fatal")
            || line_lower.contains("panic")
        {
            let count = error_counts.entry(normalized.clone()).or_insert(0);
            if *count == 0 {
                error_originals.insert(normalized.clone(), line.to_string());
            }
            *count += 1;
        } else if line_lower.contains("warn") {
            let count = warn_counts.entry(normalized.clone()).or_insert(0);
            if *count == 0 {
                warn_originals.insert(normalized.clone(), line.to_string());
            }
            *count += 1;
        } else if line_lower.contains("info") {
            info_total += 1;
        } else if line_lower.contains("debug") {
            debug_total += 1;
        }
    }

    let total_criticals: usize = critical_counts.values().sum();
    let total_errors: usize = error_counts.values().sum();
    let total_warnings: usize = warn_counts.values().sum();

    result.push("Log Summary".to_string());
    if total_criticals > 0 {
        result.push(format!(
            "   [critical] {} critical ({} unique)",
            total_criticals,
            critical_counts.len()
        ));
    }
    if total_errors > 0 {
        result.push(format!(
            "   [error] {} errors ({} unique)",
            total_errors,
            error_counts.len()
        ));
    }
    if total_warnings > 0 {
        result.push(format!(
            "   [warn] {} warnings ({} unique)",
            total_warnings,
            warn_counts.len()
        ));
    }
    if info_total > 0 {
        result.push(format!("   [info] {} info messages", info_total));
    }
    if debug_total > 0 {
        result.push(format!("   [debug] {} debug messages", debug_total));
    }
    result.push(String::new());

    // Criticals with counts (shown first — highest severity)
    if !critical_originals.is_empty() {
        result.push("[CRITICALS]".to_string());
        render_entries(&critical_counts, &critical_originals, 10, &mut result, "criticals");
        result.push(String::new());
    }

    // Errors with counts
    if !error_originals.is_empty() {
        result.push("[ERRORS]".to_string());
        render_entries(&error_counts, &error_originals, 10, &mut result, "errors");
        result.push(String::new());
    }

    // Warnings with counts
    if !warn_originals.is_empty() {
        result.push("[WARNINGS]".to_string());
        render_entries(&warn_counts, &warn_originals, 5, &mut result, "warnings");
    }

    result.join("\n")
}

fn render_entries(
    counts: &HashMap<String, usize>,
    originals: &HashMap<String, String>,
    limit: usize,
    result: &mut Vec<String>,
    label: &str,
) {
    let mut list: Vec<_> = counts.iter().collect();
    // Secondary sort by key ensures deterministic output when counts are equal
    list.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));

    for (normalized, count) in list.iter().take(limit) {
        let original = originals
            .get(*normalized)
            .map(|s| s.as_str())
            .unwrap_or(normalized);
        let line = truncate(original, 100);
        if **count > 1 {
            result.push(format!("   [×{}] {}", count, line));
        } else {
            result.push(format!("   {}", line));
        }
    }

    if list.len() > limit {
        result.push(format!("   ... +{} more unique {}", list.len() - limit, label));
    }
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
    use insta::assert_snapshot;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_log_critical_snapshot() {
        let input = include_str!("../../../tests/fixtures/log_critical_raw.txt");
        let result = analyze_logs(input);
        assert_snapshot!(result);
    }

    #[test]
    fn test_log_critical_token_savings() {
        let input = include_str!("../../../tests/fixtures/log_critical_raw.txt");
        let result = analyze_logs(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "Expected >=60% token savings, got {:.1}% (input: {} tokens, output: {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

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
    fn test_critical_level_not_silently_discarded() {
        let logs = "\
[ERROR] Connection failed\n\
[CRITICAL] Payment service unreachable, 4821 pending transactions\n\
[INFO] Health check ok\n\
[ALERT] Disk space below 5%\n\
[EMERGENCY] Database corruption detected\n\
[DEBUG] Processing request id=42\n";
        let result = analyze_logs(logs);
        assert!(result.contains("CRITICALS"), "CRITICAL/ALERT/EMERGENCY must appear in output");
        assert!(result.contains("Payment service unreachable"), "CRITICAL message must not be discarded");
        assert!(result.contains("Disk space below"), "ALERT message must not be discarded");
        assert!(result.contains("Database corruption"), "EMERGENCY message must not be discarded");
        assert!(result.contains("[critical]"), "summary must count critical lines");
        assert!(result.contains("[debug]"), "summary must count debug lines");
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
