//! Runs curl and applies a simple truncation with tee hint if the output is too long.

use crate::core::tee::force_tee_hint;
use crate::core::tracking;
use crate::core::{stream::exec_capture, utils::resolved_command};
use anyhow::{Context, Result};

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

    let result = filter_curl_output(&result.stdout);

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

fn filter_curl_output(raw: &str) -> FilterResult {
    let trimmed = raw.trim();
    let tee_hint = force_tee_hint(raw, "curl");

    // If the output is too long and we have a tee hint, truncate the output.
    let content = if trimmed.len() >= MAX_RESPONSE_SIZE && tee_hint.is_some() {
        let mut end = MAX_RESPONSE_SIZE;
        // Ensure we don't cut in the middle of a UTF-8 character.
        // .len() counts bytes, not chars.
        while !trimmed.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}... ({} bytes total)", &trimmed[..end], trimmed.len())
    } else {
        trimmed.to_string()
    };

    FilterResult { content, tee_hint }
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
        let result = filter_curl_output(output);
        assert_eq!(result.content, output);
        assert!(result.tee_hint.is_none());
    }

    #[test]
    fn test_filter_curl_non_json() {
        let output = "Hello, World!\nThis is plain text.";
        let result = filter_curl_output(output);
        assert_eq!(result.content, output);
    }

    #[test]
    fn test_filter_curl_long_output_truncated() {
        let long: String = "x".repeat(1000);
        let result = filter_curl_output(&long);
        assert!(result.content.starts_with('x'));
        assert!(result.content.contains("bytes total"));
        assert!(result.content.contains("1000"));
        assert!(result.content.len() < 600);
    }

    #[test]
    fn test_filter_curl_multibyte_boundary() {
        let content = "a".repeat(499) + "é";
        let result = filter_curl_output(&content);
        assert!(result.content.contains("bytes total"));
        assert!(result.content.len() < 600);
    }

    #[test]
    fn test_filter_curl_exact_500_bytes() {
        let content = "a".repeat(500);
        let result = filter_curl_output(&content);
        assert!(result.content.contains("bytes total"));
    }
}
