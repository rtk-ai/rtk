//! Runs curl and applies a simple truncation with tee hint if the output is too long.

use crate::core::tee::force_tee_hint;
use crate::core::tracking;
use crate::core::{stream::exec_capture, utils::resolved_command};
use anyhow::{Context, Result};
use std::io::IsTerminal;

const MAX_RESPONSE_SIZE: usize = 500;

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

    let result = exec_capture(&mut cmd).context("Failed to run curl")?;

    // Early exit: don't feed HTTP error bodies (HTML 404 etc.) through JSON schema filter
    if !result.success() {
        let msg = if result.stderr.trim().is_empty() {
            result.stdout.trim().to_string()
        } else {
            result.stderr.trim().to_string()
        };
        eprintln!("FAILED: curl {}", msg);
        return Ok(result.exit_code);
    }

    let raw = result.stdout.clone();

    let is_tty = std::io::stdout().is_terminal();
    let result = filter_curl_output(&result.stdout, is_tty);

    println!("{}", result.content);
    if let Some(hint) = &result.tee_hint {
        println!("{}", hint);
    }

    timer.track(
        &format!("curl {}", args.join(" ")),
        &format!("rtk curl {}", args.join(" ")),
        &raw,
        &result.content,
    );

    Ok(0)
}

fn filter_curl_output(raw: &str, is_tty: bool) -> FilterResult {
    let trimmed = raw.trim();
    let tee_hint = force_tee_hint(raw, "curl");

    let looks_like_json = (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'));

    // Skip truncation when:
    // - body looks like a JSON document (mid-stream truncation produces invalid JSON, #1536)
    // - stdout is not a terminal (pipes/redirects need the full body for downstream parsers, #1282)
    let should_truncate = is_tty
        && !looks_like_json
        && trimmed.len() >= MAX_RESPONSE_SIZE
        && tee_hint.is_some();

    if !should_truncate {
        // Suppress the hint line so it never leaks into pipes / breaks JSON parsers.
        // The tee file itself is still written for later inspection.
        return FilterResult {
            content: trimmed.to_string(),
            tee_hint: None,
        };
    }

    let mut end = MAX_RESPONSE_SIZE;
    // Don't cut in the middle of a UTF-8 character — .len() counts bytes.
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    FilterResult {
        content: format!("{}... ({} bytes total)", &trimmed[..end], trimmed.len()),
        tee_hint,
    }
}

struct FilterResult {
    content: String,
    tee_hint: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_curl_json_small_no_tee_hint() {
        let output = r#"{"r2Ready":true,"status":"ok"}"#;
        let result = filter_curl_output(output, true);
        assert_eq!(result.content, output);
        assert!(result.tee_hint.is_none());
    }

    #[test]
    fn test_filter_curl_non_json() {
        let output = "Hello, World!\nThis is plain text.";
        let result = filter_curl_output(output, true);
        assert_eq!(result.content, output);
    }

    #[test]
    fn test_filter_curl_long_output_truncated() {
        let long: String = "x".repeat(1000);
        let result = filter_curl_output(&long, true);
        assert!(result.content.starts_with('x'));
        assert!(result.content.contains("bytes total"));
        assert!(result.content.contains("1000"));
        assert!(result.content.len() < 600);
    }

    #[test]
    fn test_filter_curl_multibyte_boundary() {
        let content = "a".repeat(499) + "é";
        let result = filter_curl_output(&content, true);
        assert!(result.content.contains("bytes total"));
        assert!(result.content.len() < 600);
    }

    #[test]
    fn test_filter_curl_exact_500_bytes() {
        let content = "a".repeat(500);
        let result = filter_curl_output(&content, true);
        assert!(result.content.contains("bytes total"));
    }

    // --- #1536: large JSON must remain parseable for downstream tools ---

    #[test]
    fn test_filter_curl_large_json_object_passthrough() {
        // JSON object > 500 bytes — must never be truncated, even on a TTY.
        let payload = "x".repeat(600);
        let json = format!(r#"{{"data":"{}"}}"#, payload);
        let result = filter_curl_output(&json, true);
        assert!(!result.content.contains("bytes total"));
        assert!(result.content.starts_with('{'));
        assert!(result.content.ends_with('}'));
        assert!(result.tee_hint.is_none());
    }

    #[test]
    fn test_filter_curl_large_json_array_passthrough() {
        // JSON array > 500 bytes — must never be truncated.
        let body = (0..50)
            .map(|i| format!(r#"{{"id":{},"name":"item-{:04}"}}"#, i, i))
            .collect::<Vec<_>>()
            .join(",");
        let json = format!("[{}]", body);
        assert!(
            json.len() >= MAX_RESPONSE_SIZE,
            "fixture must exceed cap, got {}",
            json.len()
        );
        let result = filter_curl_output(&json, true);
        assert!(!result.content.contains("bytes total"));
        assert!(result.content.starts_with('['));
        assert!(result.content.ends_with(']'));
    }

    // --- #1282: pipes / redirects (non-TTY) must receive full body ---

    #[test]
    fn test_filter_curl_pipe_no_truncation_for_non_json() {
        // Plain text > 500 bytes piped to a downstream tool — full body must reach it.
        let long: String = "x".repeat(1000);
        let result = filter_curl_output(&long, false);
        assert!(!result.content.contains("bytes total"));
        assert_eq!(result.content.len(), 1000);
        assert!(result.tee_hint.is_none());
    }

    #[test]
    fn test_filter_curl_pipe_no_truncation_for_json() {
        // JSON piped to jq / parser — full body, no hint line.
        let payload = "y".repeat(600);
        let json = format!(r#"{{"data":"{}"}}"#, payload);
        let result = filter_curl_output(&json, false);
        assert!(!result.content.contains("bytes total"));
        assert!(result.content.ends_with('}'));
        assert!(result.tee_hint.is_none());
    }
}
