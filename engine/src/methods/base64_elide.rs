//! `base64-elide` layer: replaces long base64 runs with a size marker.
//!
//! Embedded images/fonts in JSON, data URLs in HTML/SVG/CSS are often 90%+
//! of a file's bytes with zero informational value to the model. A run is
//! only elided above a high threshold so hashes and short tokens never
//! match. Replacement happens inside string values without breaking JSON
//! validity (marker contains no quotes or backslashes).
//!
//! Not applied to the `matches` chain: bytes in a Grep match are genuine
//! file content and stay faithful.

use crate::block::Block;
use crate::method::{Method, MethodCtx, MethodInfo, Outcome};

/// Minimum contiguous base64 chars to elide (~768 decoded bytes).
const MIN_RUN: usize = 1024;

/// Byte class of a base64 run: `[A-Za-z0-9+/]`.
fn is_base64_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'+' || b == b'/'
}

/// Byte class of a data-URL mime part: `[A-Za-z0-9.+-]`.
fn is_mime_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-')
}

/// End of the maximal base64 run starting at `i`.
fn run_end(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && is_base64_byte(bytes[i]) {
        i += 1;
    }
    i
}

/// If `data:<type>/<subtype>;base64,` starts at `i`, the end of the mime
/// type and the end of the whole prefix.
fn data_url_prefix(bytes: &[u8], i: usize) -> Option<(usize, usize)> {
    let rest = bytes.get(i..)?;
    if !rest.starts_with(b"data:") {
        return None;
    }
    let mime_part = |from: usize| {
        let mut j = from;
        while j < bytes.len() && is_mime_byte(bytes[j]) {
            j += 1;
        }
        (j > from).then_some(j)
    };
    let slash = mime_part(i + 5)?;
    if bytes.get(slash) != Some(&b'/') {
        return None;
    }
    let mime_end = mime_part(slash + 1)?;
    if !bytes[mime_end..].starts_with(b";base64,") {
        return None;
    }
    Some((mime_end, mime_end + 8))
}

/// One base64 match: the whole match range, the mime range (when a data-URL
/// prefix is present) and where the run (including `=` padding) starts.
struct Base64Match {
    start: usize,
    end: usize,
    mime: Option<(usize, usize)>,
    run_start: usize,
}

/// Leftmost match at or after `pos` of
/// `(?:data:([A-Za-z0-9.+-]+/[A-Za-z0-9.+-]+);base64,)?([A-Za-z0-9+/]{MIN_RUN,}={0,2})`,
/// scanned by hand: the counted repetition makes the regex slow.
fn next_match(bytes: &[u8], mut pos: usize) -> Option<Base64Match> {
    let finish = |start: usize, mime: Option<(usize, usize)>, run_start: usize, end: usize| {
        let mut end = end;
        for _ in 0..2 {
            if bytes.get(end) == Some(&b'=') {
                end += 1;
            }
        }
        Base64Match {
            start,
            end,
            mime,
            run_start,
        }
    };
    while pos < bytes.len() {
        if !is_base64_byte(bytes[pos]) {
            pos += 1;
            continue;
        }
        let end = run_end(bytes, pos);
        if end - pos >= MIN_RUN {
            return Some(finish(pos, None, pos, end));
        }
        // A short run holds no bare match, but a data-URL prefix can start
        // inside it: `data` is base64 and the `:` after it ends the run.
        if end - pos >= 4 && &bytes[end - 4..end] == b"data" {
            if let Some((mime_end, prefix_end)) = data_url_prefix(bytes, end - 4) {
                let run = run_end(bytes, prefix_end);
                if run - prefix_end >= MIN_RUN {
                    return Some(finish(end - 4, Some((end + 1, mime_end)), prefix_end, run));
                }
            }
        }
        pos = end;
    }
    None
}

pub struct Base64Elide;

impl Method for Base64Elide {
    fn name(&self) -> &'static str {
        "base64-elide"
    }

    fn info(&self) -> MethodInfo {
        MethodInfo {
            lossy: true,
            recovery: crate::method::Recovery::None,
            idempotent: false,
            ..MethodInfo::LOSSLESS_TRANSFORM
        }
    }

    fn apply(&self, block: &Block, _ctx: &MethodCtx) -> Outcome {
        let input = block.text;
        if input.len() < MIN_RUN {
            return Outcome::Continue(input.to_string());
        }
        let bytes = input.as_bytes();
        let mut out = String::with_capacity(input.len());
        let mut last = 0;
        while let Some(m) = next_match(bytes, last) {
            out.push_str(&input[last..m.start]);
            let run = &input[m.run_start..m.end];
            // The character class also matches a long decimal number or a hex
            // dump, and eliding one corrupts the surrounding JSON (a bare
            // number turns into `[base64 …]`). Only elide a run that is
            // actually base64 by `looks_like_base64`; leave the whole match
            // untouched otherwise, data-URL prefix included.
            if looks_like_base64(run) {
                let decoded_kb = (run.len() * 3 / 4) as f64 / 1024.0;
                let kind = m.mime.map_or("data", |(from, to)| &input[from..to]);
                out.push_str(&format!("[base64 {kind}, {decoded_kb:.0} KB elided]"));
            } else {
                out.push_str(&input[m.start..m.end]);
            }
            last = m.end;
        }
        out.push_str(&input[last..]);
        Outcome::Continue(out)
    }
}

/// Whether a run is plausibly real base64 rather than a decimal number or a
/// hex dump that happens to share the alphabet. Real base64 pads to a multiple
/// of 4 and, across 1024+ characters, spans all of upper, lower, and digits;
/// a number is digits only and a hex run is missing one letter case.
fn looks_like_base64(run: &str) -> bool {
    let body = run.trim_end_matches('=');
    if body.is_empty() || !run.len().is_multiple_of(4) {
        return false;
    }
    let (mut upper, mut lower, mut digit) = (false, false, false);
    for c in body.bytes() {
        match c {
            b'A'..=b'Z' => upper = true,
            b'a'..=b'z' => lower = true,
            b'0'..=b'9' => digit = true,
            _ => {}
        }
    }
    upper && lower && digit
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::ContentFormat;

    fn ctx() -> MethodCtx<'static> {
        MethodCtx {
            source: "test.json",
        }
    }

    fn blk(text: &str) -> Block<'_> {
        crate::testutil::block(text, ContentFormat::Json)
    }

    fn apply(input: &str) -> String {
        match Base64Elide.apply(&blk(input), &ctx()) {
            Outcome::Continue(out) | Outcome::ShortCircuit(out) => out,
            Outcome::Skip => input.to_string(),
        }
    }

    // Real base64 payload (deterministic bytes, standard alphabet, no padding
    // ambiguity): 3000 chars ≈ 2.2 KB decoded.
    fn long_b64() -> String {
        "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVphYmNkZWZnaGlqa2xtbm9wcXJzdHV2d3h5ejAxMjM0NTY3ODkr"
            .repeat(36)
    }

    #[test]
    fn test_data_url_elided_with_mime() {
        let input = format!(r#"{{"icon": "data:image/png;base64,{}"}}"#, long_b64());
        let out = apply(&input);
        assert!(out.contains("[base64 image/png,"), "mime in marker: {out}");
        assert!(out.len() < 200, "run must be gone");
        serde_json::from_str::<serde_json::Value>(&out).expect("JSON stays valid");
    }

    #[test]
    fn test_bare_run_elided_generic() {
        let input = format!("prefix {} suffix", long_b64());
        let out = apply(&input);
        assert!(out.contains("[base64 data,"), "generic marker: {out}");
        assert!(out.starts_with("prefix ") && out.ends_with(" suffix"));
    }

    #[test]
    fn test_short_base64_kept() {
        let input = r#"{"hash": "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVo="}"#;
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_savings_on_blob_heavy_json() {
        let input = format!(
            r#"{{"name": "logo", "data": "data:image/png;base64,{}"}}"#,
            long_b64()
        );
        let out = apply(&input);
        let savings = 100.0 - (out.len() as f64 / input.len() as f64 * 100.0);
        assert!(savings >= 90.0, "expected ≥90% on blob, got {savings:.1}%");
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(apply(""), "");
    }
}

#[cfg(test)]
mod base64_guard {
    use super::*;
    use crate::block::ContentFormat;

    fn run(input: &str) -> String {
        let block = crate::testutil::block(input, ContentFormat::Json);
        match Base64Elide.apply(&block, &crate::method::MethodCtx::new("t")) {
            Outcome::Continue(s) | Outcome::ShortCircuit(s) => s,
            Outcome::Skip => input.to_string(),
        }
    }

    /// A long digit run is not base64 — eliding it inside a JSON
    /// number produces invalid JSON that the downstream toon then fails on.
    #[test]
    fn a_long_bare_number_is_not_mistaken_for_base64() {
        // serde_json cannot even represent a 1300-digit integer, so "does it
        // parse" is the wrong check — the property is that the number is left
        // exactly as it was, never replaced by an unquoted `[base64 …]` that
        // would corrupt the surrounding structure.
        let big_int: String = "1".repeat(1300);
        let json = format!("{{\"n\": {big_int}, \"k\": \"v\"}}");
        let out = run(&json);
        assert_eq!(out, json, "a bare number must be left untouched");
    }

    /// Repeated hex (deadbeef×N) is not base64 either — same digit
    /// alphabet subset, but low entropy and not a real encoding.
    #[test]
    fn repeated_hex_is_not_elided() {
        let hex = "deadbeef".repeat(200);
        let out = run(&hex);
        assert_eq!(out, hex, "low-entropy hex must not be treated as base64");
    }

    /// A genuine base64 blob inside a JSON string must still be elided — the
    /// guard must not over-correct.
    #[test]
    fn a_real_base64_blob_is_still_elided() {
        let blob: String =
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/".repeat(30);
        let json = format!("{{\"img\": \"{blob}\"}}");
        let out = run(&json);
        assert!(
            out.contains("elided"),
            "real base64 must still elide: {out}"
        );
        assert!(
            serde_json::from_str::<serde_json::Value>(&out).is_ok(),
            "elision must keep JSON valid: {out}"
        );
    }
}
