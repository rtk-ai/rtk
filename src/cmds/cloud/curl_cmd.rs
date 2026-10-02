//! Runs curl and condenses large response bodies.
//!
//! Bodies at or under `PASSTHROUGH_MAX` pass through unchanged. Above it, a
//! JSON body is compacted to a structural view (strings truncated, long arrays
//! sampled) and any other text body is windowed to its head. Both paths persist
//! the raw body through the recovery store first and print its hint — recovery
//! is local, never a re-fetch, which an HTTP response (unlike a git object)
//! could not replay byte-exactly. How much of a huge body the store keeps
//! follows its own caps (`[retriever]` config; a capped tee file marks its
//! truncation point), and without a hint nothing is elided at all.
//!
//! Flags that change what stdout *is* — a download (`-o`/`-O`), headers mixed
//! into the body (`-i`/`-I`), a `-w` format trailer — force a byte-exact
//! passthrough, as does `RTK_CURL_RAW=1`. The hook never rewrites a piped
//! `curl … | consumer` (its rule is `PipelineSafety::None`), so a downstream
//! parser only ever meets this filter when a user wires it up by hand.
//!
//! Binary downloads (any non-UTF-8 byte sequence) are written through to
//! stdout as raw bytes, bypassing the UTF-8 lossy conversion that would
//! otherwise replace non-UTF-8 bytes with U+FFFD and corrupt the stream
//! (`#1087`).

use crate::cmds::system::json_cmd;
use crate::core::arg_tokenizer::{self, Dialect, TokenKind, ValueSpec};
use crate::core::tee::force_tee_hint;
use crate::core::tracking;
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};
use std::borrow::Cow;
use std::io::Write;

/// Bodies at or under this many bytes are never touched.
const PASSTHROUGH_MAX: usize = 4096;
/// Head window for large non-JSON text bodies.
const WINDOW_BYTES: usize = 4096;
/// Nesting depth kept in the compact JSON view.
const JSON_DEPTH: usize = 3;
/// Above this, skip JSON parsing (a serde tree of a huge body costs real
/// memory) and fall back to windowing.
const JSON_PARSE_CAP: usize = 20 * 1024 * 1024;

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
        // stderr is curl's own diagnostics, which do follow the console code
        // page. The body on stdout does not — see the note below.
        let stderr_str = crate::core::utils::decode_process_output(&output.stderr);
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        let msg = if stderr_str.trim().is_empty() {
            stdout_str.trim().to_string()
        } else {
            stderr_str.trim().to_string()
        };
        eprintln!("FAILED: curl {}", msg);
        return Ok(exit_code);
    }

    // Byte-exact passthrough for binary bodies (the lossy UTF-8 conversion
    // would corrupt them, #1087), for flags whose stdout is not a plain body
    // (see raw_output_requested), and for the RTK_CURL_RAW opt-out. Tracked
    // as passthrough (0% savings) since nothing was filtered.
    if is_binary(&output.stdout) || raw_mode_env() || raw_output_requested(args) {
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        handle
            .write_all(&output.stdout)
            .context("Failed to write raw response to stdout")?;
        timer.track_passthrough(
            &format!("curl {}", args.join(" ")),
            &format!("rtk curl {}", args.join(" ")),
        );
        return Ok(exit_code);
    }

    // Deliberately not `decode_process_output`: a response body is a network
    // payload whose encoding comes from the HTTP charset, not from the local
    // console code page, so decoding it as GBK/CP850 would only ever be right
    // by accident. Anything that is not valid UTF-8 already took the raw
    // binary passthrough above, so this conversion is lossless in practice.
    let raw = String::from_utf8_lossy(&output.stdout).into_owned();
    let filtered = filter_curl_output(&raw);

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

fn raw_mode_env() -> bool {
    crate::core::user_env::var("RTK_CURL_RAW").is_some_and(|v| v != "0")
}

/// Which curl flags take a value. Transcribed from `curl --help all`
/// (curl 8.x), limited to the commonly used options. An unknown long flag
/// defaults to "no value", which at worst makes its value look like a
/// positional — harmless here, where only flag identities are read.
fn curl_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    match kind {
        TokenKind::Short => match name {
            "A" | "b" | "c" | "C" | "d" | "D" | "e" | "E" | "F" | "H" | "K" | "m" | "o" | "P"
            | "Q" | "r" | "t" | "T" | "u" | "U" | "w" | "x" | "X" | "y" | "Y" | "z" => {
                Some(ValueSpec::value())
            }
            _ => None,
        },
        TokenKind::Long => match name {
            "user-agent" | "cookie" | "cookie-jar" | "continue-at" | "data" | "data-raw"
            | "data-binary" | "data-urlencode" | "data-ascii" | "dump-header" | "referer"
            | "cert" | "cert-type" | "form" | "form-string" | "header" | "config" | "max-time"
            | "connect-timeout" | "output" | "output-dir" | "ftp-port" | "quote" | "range"
            | "telnet-option" | "upload-file" | "user" | "proxy-user" | "write-out" | "proxy"
            | "proxy-header" | "request" | "speed-time" | "speed-limit" | "time-cond" | "url"
            | "retry" | "retry-delay" | "retry-max-time" | "max-filesize" | "limit-rate"
            | "cacert" | "capath" | "key" | "key-type" | "ciphers" | "resolve" | "interface"
            | "dns-servers" | "max-redirs" | "oauth2-bearer" | "aws-sigv4" | "unix-socket"
            | "noproxy" | "keepalive-time" | "expect100-timeout" => Some(ValueSpec::value()),
            _ => None,
        },
        _ => None,
    }
}

/// True when a flag changes what stdout *is*: a download target (`-o`/`-O`),
/// headers mixed into the body (`-i`/`-I`), or a `-w` trailer appended after
/// it. Read from tokens, not raw argv: `-H -I` is a header *value*, `-sLo`
/// is a cluster, and anything past `--` is a URL.
fn raw_output_requested(args: &[String]) -> bool {
    let tokens = arg_tokenizer::tokenize_grammar(args, &curl_takes_value, Dialect::Posix);
    arg_tokenizer::before_dashdash(&tokens)
        .iter()
        .any(|t| match t.kind {
            TokenKind::Short => matches!(t.text, "o" | "O" | "i" | "I" | "w"),
            TokenKind::Long => matches!(
                t.text,
                "output"
                    | "remote-name"
                    | "remote-name-all"
                    | "output-dir"
                    | "include"
                    | "head"
                    | "write-out"
            ),
            _ => false,
        })
}

fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn filter_curl_output(raw: &str) -> FilterResult<'_> {
    let trimmed = raw.trim();

    if trimmed.len() <= PASSTHROUGH_MAX {
        return passthrough(trimmed);
    }

    // Heuristic: looks like a top-level JSON document. Numbers / booleans / null
    // are always under PASSTHROUGH_MAX so they don't need detection here.
    let looks_like_json = (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
        || (trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2);

    // On a parse failure (truncated upstream, JSON-ish HTML, NDJSON) this falls
    // through to windowing rather than passing megabytes through.
    if looks_like_json
        && trimmed.len() <= JSON_PARSE_CAP
        && let Ok(compact) = json_cmd::filter_json_compact(trimmed, JSON_DEPTH)
    {
        // Elision must stay recoverable: no tee file, no compaction.
        if let Some(hint) = force_tee_hint(raw, "curl") {
            let banner = format!(
                "[rtk curl: {} JSON body -> compact view (strings truncated, long arrays sampled)]",
                format_size(trimmed.len())
            );
            return FilterResult {
                content: Cow::Owned(format!("{}\n{}", banner, compact)),
                tee_hint: Some(hint),
            };
        }
        return passthrough(trimmed);
    }

    // Large non-JSON text (bundles, HTML, logs): window the head and point at
    // the rest. Same recoverability rule as above: tee file or nothing.
    let Some(hint) = force_tee_hint(raw, "curl") else {
        return passthrough(trimmed);
    };
    let mut end = WINDOW_BYTES;
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

fn passthrough(trimmed: &str) -> FilterResult<'_> {
    FilterResult {
        content: Cow::Borrowed(trimmed),
        tee_hint: None,
    }
}

struct FilterResult<'a> {
    content: Cow<'a, str>,
    tee_hint: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_filter_curl_json_small_no_tee_hint() {
        let output = r#"{"r2Ready":true,"status":"ok"}"#;
        let result = filter_curl_output(output);
        assert_eq!(&*result.content, output);
        assert!(result.tee_hint.is_none());
    }

    #[test]
    fn test_filter_curl_non_json() {
        let output = "Hello, World!\nThis is plain text.";
        let result = filter_curl_output(output);
        assert_eq!(&*result.content, output);
    }

    #[test]
    fn test_filter_curl_long_output_windowed() {
        let long: String = "x".repeat(10_000);
        let result = filter_curl_output(&long);
        assert!(result.content.starts_with('x'));
        assert!(result.content.contains("bytes total"));
        assert!(result.content.contains("10000"));
        assert!(result.content.len() < WINDOW_BYTES + 100);
        assert!(result.tee_hint.is_some(), "windowing must emit a hint");
    }

    #[test]
    fn test_filter_curl_multibyte_boundary() {
        let content = "a".repeat(WINDOW_BYTES - 1) + &"é".repeat(60);
        let result = filter_curl_output(&content);
        assert!(result.content.contains("bytes total"));
        assert!(result.content.len() < WINDOW_BYTES + 100);
    }

    #[test]
    fn test_filter_curl_at_threshold_passthrough() {
        let content = "a".repeat(PASSTHROUGH_MAX);
        let result = filter_curl_output(&content);
        assert_eq!(&*result.content, content);
        assert!(result.tee_hint.is_none());
    }

    // --- Large JSON: compacted with a recovery hint (was passthrough, #1536;
    // the hook never rewrites a piped `curl … | jq`, so no parser sees this) ---

    #[test]
    fn test_filter_curl_large_json_object_compacted() {
        let body = (0..200)
            .map(|i| format!(r#""key{:03}":{{"id":{},"name":"item-{:04}"}}"#, i, i, i))
            .collect::<Vec<_>>()
            .join(",");
        let json = format!("{{{}}}", body);
        assert!(
            json.len() > PASSTHROUGH_MAX,
            "fixture must exceed threshold"
        );
        let result = filter_curl_output(&json);
        assert!(result.content.starts_with("[rtk curl:"));
        assert!(result.content.contains("compact view"));
        assert!(
            result.content.contains("more keys"),
            "elision must be marked"
        );
        assert!(result.tee_hint.is_some(), "compaction must emit a hint");
        assert!(result.content.len() < json.len() / 2);
    }

    #[test]
    fn test_filter_curl_large_json_array_compacted() {
        let body = (0..400)
            .map(|i| format!(r#"{{"id":{},"name":"item-{:04}"}}"#, i, i))
            .collect::<Vec<_>>()
            .join(",");
        let json = format!("[{}]", body);
        assert!(
            json.len() > PASSTHROUGH_MAX,
            "fixture must exceed threshold"
        );
        let result = filter_curl_output(&json);
        assert!(result.content.starts_with("[rtk curl:"));
        assert!(result.content.contains("more"), "elision must be marked");
        assert!(result.tee_hint.is_some());
    }

    #[test]
    fn test_filter_curl_json_bare_string_passthrough() {
        // Bare top-level JSON string — e.g. an /api/token endpoint returning
        // "<long-token>". Stays under the threshold, so the exact bytes
        // survive for the caller to use.
        let token = "z".repeat(800);
        let json = format!(r#""{}""#, token);
        let result = filter_curl_output(&json);
        assert_eq!(&*result.content, json);
        assert!(result.tee_hint.is_none());
    }

    #[test]
    fn test_filter_curl_invalid_json_falls_back_to_window() {
        // JSON-shaped but unparseable: `{` ... `}` around plain text.
        let body = format!("{{{}}}", "not json, ".repeat(1000));
        let result = filter_curl_output(&body);
        assert!(result.content.contains("bytes total"));
        assert!(result.tee_hint.is_some());
    }

    #[test]
    fn test_filter_curl_token_savings_real_fixture() {
        let input = include_str!("../../../tests/fixtures/glab_mr_list_raw.json");
        assert!(
            input.trim().len() > PASSTHROUGH_MAX,
            "fixture must exceed threshold"
        );
        let result = filter_curl_output(input);
        let savings =
            100.0 - (count_tokens(&result.content) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "expected >=60% savings, got {:.1}%",
            savings
        );
    }

    // --- Raw-output flags: byte-exact passthrough ---

    #[test]
    fn test_raw_output_requested_download_flags() {
        assert!(raw_output_requested(&strings(&[
            "-o",
            "/tmp/x",
            "https://e.com"
        ])));
        assert!(raw_output_requested(&strings(&[
            "-sLo",
            "/tmp/x",
            "https://e.com"
        ])));
        assert!(raw_output_requested(&strings(&["-O", "https://e.com"])));
        assert!(raw_output_requested(&strings(&[
            "--output=f.bin",
            "https://e.com"
        ])));
        assert!(raw_output_requested(&strings(&[
            "--output-dir",
            "/tmp",
            "-O",
            "https://e.com"
        ])));
    }

    #[test]
    fn test_raw_output_requested_header_and_writeout_flags() {
        assert!(raw_output_requested(&strings(&["-i", "https://e.com"])));
        assert!(raw_output_requested(&strings(&["-I", "https://e.com"])));
        assert!(raw_output_requested(&strings(&["--head", "https://e.com"])));
        assert!(raw_output_requested(&strings(&[
            "-w",
            "%{http_code}",
            "https://e.com"
        ])));
    }

    #[test]
    fn test_raw_output_not_requested_plain_fetch() {
        assert!(!raw_output_requested(&strings(&["https://e.com"])));
        assert!(!raw_output_requested(&strings(&[
            "-sL",
            "-m",
            "20",
            "https://e.com"
        ])));
        assert!(!raw_output_requested(&strings(&[
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "-d",
            "{}",
            "https://e.com",
        ])));
    }

    #[test]
    fn test_raw_output_flag_value_is_not_a_flag() {
        // `-I` here is -H's value (a header string), not --head.
        assert!(!raw_output_requested(&strings(&[
            "-H",
            "-I",
            "https://e.com"
        ])));
        // `-w` after `--` is a URL-position argument, not --write-out.
        assert!(!raw_output_requested(&strings(&["--", "-w"])));
    }

    #[test]
    fn test_raw_mode_env_opt_out() {
        use crate::core::user_env;
        user_env::with_vars(&[("RTK_CURL_RAW", Some("1"))], || assert!(raw_mode_env()));
        user_env::with_vars(&[("RTK_CURL_RAW", Some("0"))], || assert!(!raw_mode_env()));
        user_env::with_vars(&[("RTK_CURL_RAW", None)], || assert!(!raw_mode_env()));
    }

    // --- Cow optimization: passthrough must not allocate ---

    #[test]
    fn test_filter_curl_passthrough_is_borrowed() {
        let small = "x".repeat(PASSTHROUGH_MAX);
        let result = filter_curl_output(&small);
        assert!(matches!(result.content, Cow::Borrowed(_)));
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
