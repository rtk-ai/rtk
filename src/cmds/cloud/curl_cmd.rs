//! Runs curl and condenses long output for human consumption.
//!
//! For pipes / redirects (non-TTY) and JSON bodies the full response is passed
//! through unchanged — truncating mid-stream would break downstream parsers.
//! The condensed-form-with-tee-hint path is reserved for non-JSON bodies on
//! a real terminal where a human reads the output and the tee file gives the
//! LLM a way to recover the raw response.
//!
//! Binary downloads (any non-UTF-8 byte sequence) are written through to
//! stdout as raw bytes, bypassing the UTF-8 lossy conversion that would
//! otherwise replace non-UTF-8 bytes with U+FFFD and corrupt the stream
//! (`#1087`).

use crate::core::tee::force_tee_hint;
use crate::core::tracking;
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};
use std::borrow::Cow;
use std::io::{IsTerminal, Write};

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

    // Capture stdout as raw bytes (not UTF-8 String) so binary downloads
    // survive intact. `String::from_utf8_lossy` would otherwise replace
    // every non-UTF-8 byte with U+FFFD (3 bytes), corrupting e.g. gzip
    // magic `1f 8b 08 00` into `1f ef bf bd 08 00` (#1087).
    let output = cmd.output().context("Failed to run curl")?;
    let exit_code = output.status.code().unwrap_or(1);

    // Skip filtering on failure: curl can return HTML error bodies that would
    // be misleading to summarize, and we want the real exit code surfaced.
    if !output.status.success() {
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        let msg = if stderr_str.trim().is_empty() {
            stdout_str.trim().to_string()
        } else {
            stderr_str.trim().to_string()
        };
        eprintln!("FAILED: curl {}", msg);
        return Ok(exit_code);
    }

    // Binary detection: if the body is not valid UTF-8, `from_utf8_lossy`
    // would replace every invalid byte with U+FFFD and corrupt the stream
    // (gzip, zip, png, pdf, elf, ... — any binary format). Write raw bytes
    // through and skip filtering. Tracking is recorded as passthrough
    // (0% savings) since token counts over binary content have no meaning.
    if is_binary(&output.stdout) {
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        handle
            .write_all(&output.stdout)
            .context("Failed to write binary response to stdout")?;
        timer.track_passthrough(
            &format!("curl {}", args.join(" ")),
            &format!("rtk curl {}", args.join(" ")),
        );
        return Ok(exit_code);
    }

    let raw = String::from_utf8_lossy(&output.stdout).into_owned();
    let is_tty = std::io::stdout().is_terminal();
    let filtered = filter_curl_output(&raw, is_tty);

    let shown =
        crate::core::runner::emit_guarded(&filtered.content, filtered.tee_hint.as_deref(), &raw);

    timer.track(
        &format!("curl {}", args.join(" ")),
        &format!("rtk curl {}", args.join(" ")),
        &raw,
        &shown,
    );

    Ok(exit_code)
}

/// Returns `true` if `bytes` is not valid UTF-8 — which is exactly the
/// condition under which `from_utf8_lossy` would replace invalid bytes
/// with U+FFFD and corrupt downstream consumers (`#1087`).
///
/// This is correct by construction: the only reason to passthrough raw
/// bytes is to avoid the lossy conversion, and the only bytes that suffer
/// from it are the non-UTF-8 ones.
fn is_binary(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_err()
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

    // --- is_binary tests ----------------------------------------------------

    #[test]
    fn test_is_binary_gzip_magic_is_not_utf8() {
        // gzip magic 1f 8b — 0x8b is an invalid UTF-8 continuation byte
        let bytes = [0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03];
        assert!(is_binary(&bytes));
    }

    #[test]
    fn test_is_binary_valid_utf8_text_is_not_binary() {
        assert!(!is_binary(br#"{"key": "value"}"#));
        assert!(!is_binary(b"<!DOCTYPE html>\n<html><body>Hi</body></html>"));
        assert!(!is_binary(b"Plain ASCII text"));
        assert!(!is_binary("Héllo wörld — emojis 🚀 ✓".as_bytes()));
    }

    #[test]
    fn test_is_binary_empty_is_not_binary() {
        // Empty input is technically valid UTF-8 and trivially safe to filter.
        assert!(!is_binary(&[]));
    }

    #[test]
    fn test_is_binary_text_with_nul_is_not_binary() {
        // NUL is valid UTF-8 (U+0000). Unusual in HTTP responses but the
        // function honors UTF-8 strictly — caller can still filter such
        // content safely. The bug we're fixing is only invalid UTF-8 bytes.
        assert!(!is_binary(b"text with\0embedded nul"));
    }
}
