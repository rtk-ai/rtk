use crate::tracking;
use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead};
use std::path::Path;

/// Filter and deduplicate log output
pub fn run_file(file: &Path, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Analyzing log: {}", file.display());
    }

    let content = fs::read_to_string(file)?;
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
    let mut error_counts: HashMap<String, usize> = HashMap::new();
    let mut warn_counts: HashMap<String, usize> = HashMap::new();
    let mut info_counts: HashMap<String, usize> = HashMap::new();
    let mut unique_errors: Vec<String> = Vec::new();
    let mut unique_warnings: Vec<String> = Vec::new();

    // Patterns to normalize log messages
    let timestamp_re =
        Regex::new(r"^\d{4}[-/]\d{2}[-/]\d{2}[T ]\d{2}:\d{2}:\d{2}[.,]?\d*\s*").unwrap();
    let uuid_re =
        Regex::new(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
            .unwrap();
    let hex_re = Regex::new(r"0x[0-9a-fA-F]+").unwrap();
    let num_re = Regex::new(r"\b\d{4,}\b").unwrap();
    let path_re = Regex::new(r"/[\w./\-]+").unwrap();

    for line in content.lines() {
        let line_lower = line.to_lowercase();

        // Normalize for deduplication
        let normalized =
            normalize_log_line(line, &timestamp_re, &uuid_re, &hex_re, &num_re, &path_re);

        // Categorize
        if line_lower.contains("error")
            || line_lower.contains("fatal")
            || line_lower.contains("panic")
        {
            let count = error_counts.entry(normalized.clone()).or_insert(0);
            if *count == 0 {
                unique_errors.push(line.to_string());
            }
            *count += 1;
        } else if line_lower.contains("warn") {
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

    result.push(format!("📊 Log Summary"));
    result.push(format!(
        "   ❌ {} errors ({} unique)",
        total_errors,
        error_counts.len()
    ));
    result.push(format!(
        "   ⚠️  {} warnings ({} unique)",
        total_warnings,
        warn_counts.len()
    ));
    result.push(format!("   ℹ️  {} info messages", total_info));
    result.push(String::new());

    // Errors with counts
    if !unique_errors.is_empty() {
        result.push("❌ ERRORS:".to_string());

        // Sort by count
        let mut error_list: Vec<_> = error_counts.iter().collect();
        error_list.sort_by(|a, b| b.1.cmp(a.1));

        for (normalized, count) in error_list.iter().take(10) {
            // Find original message
            let original = unique_errors
                .iter()
                .find(|e| {
                    &normalize_log_line(e, &timestamp_re, &uuid_re, &hex_re, &num_re, &path_re)
                        == *normalized
                })
                .map(|s| s.as_str())
                .unwrap_or(normalized);

            let truncated = if original.len() > 100 {
                let t: String = original.chars().take(97).collect();
                format!("{}...", t)
            } else {
                original.to_string()
            };

            if **count > 1 {
                result.push(format!("   [×{}] {}", count, truncated));
            } else {
                result.push(format!("   {}", truncated));
            }
        }

        if error_list.len() > 10 {
            result.push(format!(
                "   ... +{} more unique errors",
                error_list.len() - 10
            ));
        }
        result.push(String::new());
    }

    // Warnings with counts
    if !unique_warnings.is_empty() {
        result.push("⚠️  WARNINGS:".to_string());

        let mut warn_list: Vec<_> = warn_counts.iter().collect();
        warn_list.sort_by(|a, b| b.1.cmp(a.1));

        for (normalized, count) in warn_list.iter().take(5) {
            let original = unique_warnings
                .iter()
                .find(|w| {
                    &normalize_log_line(w, &timestamp_re, &uuid_re, &hex_re, &num_re, &path_re)
                        == *normalized
                })
                .map(|s| s.as_str())
                .unwrap_or(normalized);

            let truncated = if original.len() > 100 {
                let t: String = original.chars().take(97).collect();
                format!("{}...", t)
            } else {
                original.to_string()
            };

            if **count > 1 {
                result.push(format!("   [×{}] {}", count, truncated));
            } else {
                result.push(format!("   {}", truncated));
            }
        }

        if warn_list.len() > 5 {
            result.push(format!(
                "   ... +{} more unique warnings",
                warn_list.len() - 5
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
