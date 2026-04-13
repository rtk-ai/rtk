//! Runs curl and auto-compresses JSON responses.

use crate::core::tracking;
use crate::core::utils::{exit_code_from_output, resolved_command, truncate};
use crate::json_cmd;
use anyhow::{Context, Result};

/// Not using run_filtered: on failure, curl can return HTML error pages (404, 500)
/// that the JSON schema filter would mangle. The early exit skips filtering entirely.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let mut cmd = resolved_command("curl");
    cmd.arg("-s"); // Silent mode (no progress bar)

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: curl -s {}", args.join(" "));
    }

    let output = cmd.output().context("Failed to run curl")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Early exit: don't feed HTTP error bodies (HTML 404 etc.) through JSON schema filter
    if !output.status.success() {
        let msg = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        eprintln!("FAILED: curl {}", msg);
        return Ok(exit_code_from_output(&output, "curl"));
    }

    let raw = stdout.to_string();

    // Auto-detect JSON and pipe through filter
    let filtered = filter_curl_output(&stdout, args);
    println!("{}", filtered);

    timer.track(
        &format!("curl {}", args.join(" ")),
        &format!("rtk curl {}", args.join(" ")),
        &raw,
        &filtered,
    );

    Ok(0)
}

fn filter_curl_output(output: &str, args: &[String]) -> String {
    let trimmed = output.trim();

    // Try JSON detection: starts with { or [
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && (trimmed.ends_with('}') || trimmed.ends_with(']'))
    {
        // Skip schema conversion for internal/localhost URLs (issues #1152, #1157)
        if !is_internal_url(args) {
            if let Ok(schema) = json_cmd::filter_json_string(trimmed, 5) {
                // Only use schema if it's actually shorter than the original (#297)
                if schema.len() <= trimmed.len() {
                    return schema;
                }
            }
        }
    }

    // Not JSON: truncate long output
    let lines: Vec<&str> = trimmed.lines().collect();
    if lines.len() > 30 {
        let mut result: Vec<&str> = lines[..30].to_vec();
        result.push("");
        let msg = format!(
            "... ({} more lines, {} bytes total)",
            lines.len() - 30,
            trimmed.len()
        );
        return format!("{}\n{}", result.join("\n"), msg);
    }

    // Short output: return as-is but truncate long lines
    lines
        .iter()
        .map(|l| truncate(l, 200))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_internal_url(args: &[String]) -> bool {
    args.iter().any(|a| {
        let lower = a.to_lowercase();
        lower.starts_with("http://localhost")
            || lower.starts_with("http://127.0.0.1")
            || lower.starts_with("http://[::1]")
            || lower.starts_with("https://localhost")
            || lower.starts_with("https://127.0.0.1")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_curl_json() {
        // Large JSON where schema is shorter than original — schema should be returned
        let output = r#"{"name": "a very long user name here", "count": 42, "items": [1, 2, 3], "description": "a very long description that takes up many characters in the original JSON payload", "status": "active", "url": "https://example.com/api/v1/users/123"}"#;
        let result = filter_curl_output(output, &[]);
        assert!(result.contains("name"));
        assert!(result.contains("string"));
        assert!(result.contains("int"));
    }

    #[test]
    fn test_filter_curl_json_array() {
        let output = r#"[{"id": 1}, {"id": 2}]"#;
        let result = filter_curl_output(output, &[]);
        assert!(result.contains("id"));
    }

    #[test]
    fn test_filter_curl_non_json() {
        let output = "Hello, World!\nThis is plain text.";
        let result = filter_curl_output(output, &[]);
        assert!(result.contains("Hello, World!"));
        assert!(result.contains("plain text"));
    }

    #[test]
    fn test_filter_curl_json_small_returns_original() {
        // Small JSON where schema would be larger than original (issue #297)
        let output = r#"{"r2Ready":true,"status":"ok"}"#;
        let result = filter_curl_output(output, &[]);
        // Schema would be "{\n  r2Ready: bool,\n  status: string\n}" which is longer
        // Should return the original JSON unchanged
        assert_eq!(result.trim(), output.trim());
    }

    #[test]
    fn test_filter_curl_long_output() {
        let lines: Vec<String> = (0..50).map(|i| format!("Line {}", i)).collect();
        let output = lines.join("\n");
        let result = filter_curl_output(&output, &[]);
        assert!(result.contains("Line 0"));
        assert!(result.contains("Line 29"));
        assert!(result.contains("more lines"));
    }

    #[test]
    fn test_is_internal_url_localhost() {
        assert!(is_internal_url(&["http://localhost:9222/json/version".to_string()]));
        assert!(is_internal_url(&["http://127.0.0.1:8080/api".to_string()]));
        assert!(is_internal_url(&["-s".to_string(), "http://localhost:3000".to_string()]));
        assert!(!is_internal_url(&["https://api.example.com/data".to_string()]));
        assert!(!is_internal_url(&["https://github.com".to_string()]));
    }
}
