//! Runs curl and condenses long output for human consumption.
//!
//! For pipes / redirects (non-TTY) and JSON bodies the full response is passed
//! through unchanged — truncating mid-stream would break downstream parsers.
//! The condensed-form-with-tee-hint path is reserved for non-JSON bodies on
//! a real terminal where a human reads the output and the tee file gives the
//! LLM a way to recover the raw response.

use crate::core::tee::force_tee_hint;
use crate::core::tracking;
use crate::core::{stream::exec_capture, utils::resolved_command};
use anyhow::{Context, Result};
use std::borrow::Cow;
use std::io::IsTerminal;

const MAX_RESPONSE_SIZE: usize = 500;

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

    // Skip filtering on failure: curl can return HTML error bodies that would
    // be misleading to summarize, and we want the real exit code surfaced.
    if !result.success() {
        let msg = if result.stderr.trim().is_empty() {
            result.stdout.trim().to_string()
        } else {
            result.stderr.trim().to_string()
        };
        eprintln!("FAILED: curl {}", msg);
        return Ok(result.exit_code);
    }

    let exit_code = result.exit_code;
    let raw = result.stdout;
    let is_tty = std::io::stdout().is_terminal();
    let filtered = filter_curl_output(&raw, is_tty);

    println!("{}", filtered.content);
    if let Some(hint) = &filtered.tee_hint {
        println!("{}", hint);
    }

    timer.track(
        &format!("curl {}", args.join(" ")),
        &format!("rtk curl {}", args.join(" ")),
        &raw,
        &filtered.content,
    );

    Ok(exit_code)
}

fn filter_curl_output(raw: &str, is_tty: bool) -> FilterResult<'_> {
    let trimmed = raw.trim();

    // Heuristic: looks like a top-level JSON document. Numbers / booleans / null
    // are always under MAX_RESPONSE_SIZE so they don't need detection here.
    let looks_like_json = (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
        || (trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2);

    // Pass through unchanged when:
    // - body looks like JSON (mid-stream truncation produces invalid JSON, #1536)
    // - stdout is not a terminal (pipes / redirects need the full body, #1282)
    // - body fits under the truncation threshold
    //
    // Critically, do NOT call `force_tee_hint` on this path — it has a side effect
    // (writes the raw body to a tee log file) and we don't need a recovery file
    // when the consumer already receives the full body.
    if !is_tty || looks_like_json || trimmed.len() < MAX_RESPONSE_SIZE {
        return FilterResult {
            content: Cow::Borrowed(trimmed),
            tee_hint: None,
        };
    }

    // We're about to truncate for a human reader. Write a tee file so they (or
    // the LLM in their stead) can recover the full body from the printed hint.
    let Some(hint) = force_tee_hint(raw, "curl") else {
        // Tee disabled (RTK_TEE=0 or below MIN_TEE_SIZE): we have nowhere to
        // point a recovery hint to, so pass through rather than emit an
        // unrecoverable truncation marker.
        return FilterResult {
            content: Cow::Borrowed(trimmed),
            tee_hint: None,
        };
    };

    let mut end = MAX_RESPONSE_SIZE;
    // Don't cut in the middle of a UTF-8 character — .len() counts bytes.
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    FilterResult {
        content: Cow::Owned(format!(
            "{}... ({} bytes total)",
            &trimmed[..end],
            trimmed.len()
        )),
        tee_hint: Some(hint),
    }
}

struct FilterResult<'a> {
    content: Cow<'a, str>,
    tee_hint: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_curl_json_small_no_tee_hint() {
        let output = r#"{"r2Ready":true,"status":"ok"}"#;
        let result = filter_curl_output(output, true);
        assert_eq!(&*result.content, output);
        assert!(result.tee_hint.is_none());
    }

    #[test]
    fn test_filter_curl_non_json() {
        let output = "Hello, World!\nThis is plain text.";
        let result = filter_curl_output(output, true);
        assert_eq!(&*result.content, output);
    }

    #[test]
    fn test_filter_curl_long_output_truncated() {
        let long: String = "x".repeat(1000);
        let result = filter_curl_output(&long, true);
        assert!(result.content.starts_with('x'));
        assert!(result.content.contains("bytes total"));
        assert!(result.content.contains("1000"));
        assert!(result.content.len() < 600);
        assert!(result.tee_hint.is_some(), "TTY truncation must emit a hint");
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

    #[test]
    fn test_filter_curl_large_json_bare_string_passthrough() {
        // Bare top-level JSON string — e.g. an /api/token endpoint returning "<long-token>".
        let token = "z".repeat(800);
        let json = format!(r#""{}""#, token);
        let result = filter_curl_output(&json, true);
        assert!(!result.content.contains("bytes total"));
        assert!(result.content.starts_with('"'));
        assert!(result.content.ends_with('"'));
    }

    // --- #1282: pipes / redirects (non-TTY) must receive full body ---

    #[test]
    fn test_filter_curl_pipe_no_truncation_for_non_json() {
        let long: String = "x".repeat(1000);
        let result = filter_curl_output(&long, false);
        assert!(!result.content.contains("bytes total"));
        assert_eq!(result.content.len(), 1000);
        assert!(result.tee_hint.is_none());
    }

    #[test]
    fn test_filter_curl_pipe_no_truncation_for_json() {
        let payload = "y".repeat(600);
        let json = format!(r#"{{"data":"{}"}}"#, payload);
        let result = filter_curl_output(&json, false);
        assert!(!result.content.contains("bytes total"));
        assert!(result.content.ends_with('}'));
        assert!(result.tee_hint.is_none());
    }

    // --- Cow optimization: passthrough must not allocate ---

    #[test]
    fn test_filter_curl_passthrough_is_borrowed() {
        // Passthrough paths return Cow::Borrowed to avoid copying multi-MB bodies.
        let pipe_payload = "x".repeat(2000);
        let pipe_result = filter_curl_output(&pipe_payload, false);
        assert!(matches!(pipe_result.content, Cow::Borrowed(_)));

        let json_payload = format!(r#"[{}]"#, "1,".repeat(300));
        let json_result = filter_curl_output(&json_payload, true);
        assert!(matches!(json_result.content, Cow::Borrowed(_)));
    }
}
