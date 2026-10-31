//! Token counting via a real BPE tokenizer (tiktoken `o200k_base`).
//!
//! Used to size transform decisions (ingest filter, CCR thresholds) by
//! **tokens** rather than raw byte length. Anthropic's tokenizer is not public,
//! so this is an approximation for Claude traffic — but far closer to real
//! model cost than `String::len()`. The authoritative per-request token counts
//! in the stats still come from the provider's `usage` field.

use once_cell::sync::Lazy;
use tiktoken_rs::CoreBPE;

/// The BPE is built once (embedded vocab, no network). `None` if it failed to
/// load, in which case callers fall back to a bytes/4 estimate.
static BPE: Lazy<Option<CoreBPE>> = Lazy::new(|| tiktoken_rs::o200k_base().ok());

/// Count the tokens in `text` (≈ bytes/4 fallback if the tokenizer is absent).
pub fn count(text: &str) -> usize {
    match BPE.as_ref() {
        Some(b) => b.encode_ordinary(text).len(),
        None => text.len().div_ceil(4),
    }
}

/// Head+tail elide `text` so it fits in about `max_tokens`, splitting on token
/// boundaries. Returns `(possibly_elided, tokens_saved)`; unchanged (with 0
/// saved) when already within budget.
pub fn elide_to_tokens(text: &str, max_tokens: usize) -> (String, usize) {
    let max_tokens = max_tokens.max(16);
    let Some(bpe) = BPE.as_ref() else {
        return byte_fallback(text, max_tokens.saturating_mul(4));
    };
    let toks = bpe.encode_ordinary(text);
    if toks.len() <= max_tokens {
        return (text.to_string(), 0);
    }
    let keep = (max_tokens / 2).max(1);
    let head = bpe.decode(&toks[..keep]).unwrap_or_default();
    let tail = bpe.decode(&toks[toks.len() - keep..]).unwrap_or_default();
    let elided = toks.len().saturating_sub(2 * keep);
    let out = format!("{head}\n…[rtk-proxy elided ~{elided} tokens]…\n{tail}");
    let saved = toks.len().saturating_sub(count(&out));
    (out, saved)
}

/// Byte-based head+tail elision, used only if the BPE failed to load.
fn byte_fallback(text: &str, max_bytes: usize) -> (String, usize) {
    if text.len() <= max_bytes {
        return (text.to_string(), 0);
    }
    let keep = max_bytes / 2;
    let mut head = keep.min(text.len());
    while head > 0 && !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len().saturating_sub(keep);
    while tail < text.len() && !text.is_char_boundary(tail) {
        tail += 1;
    }
    let out = format!("{}\n…[rtk-proxy elided]…\n{}", &text[..head], &text[tail..]);
    let saved = text.len().saturating_sub(out.len());
    (out, saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_token_scale_not_byte_scale() {
        // "hello world" is ~2 tokens, far fewer than its 11 bytes.
        let n = count("hello world");
        assert!((1..6).contains(&n), "unexpected token count {n}");
    }

    #[test]
    fn small_text_is_not_elided() {
        let (out, saved) = elide_to_tokens("short text", 1000);
        assert_eq!(out, "short text");
        assert_eq!(saved, 0);
    }

    #[test]
    fn large_text_is_token_elided_and_saves() {
        let big = "lorem ipsum dolor sit amet ".repeat(400); // ~thousands of tokens
        let (out, saved) = elide_to_tokens(&big, 100);
        assert!(out.contains("rtk-proxy elided"));
        assert!(
            count(&out) <= 200,
            "elided output should be near the budget"
        );
        assert!(saved > 0);
    }
}
