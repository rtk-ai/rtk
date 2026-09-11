//! Never-worse output guard: RTK never emits more tokens than the raw command.
//!
//! One caller is allowed past it. `rtk diff` prints a one-line message for a
//! difference `str::lines()` cannot render — CRLF against LF, or a missing
//! final newline — where the raw fallback is two blobs that look identical and
//! answer the question worse at any size. The exception is bounded by the case
//! rather than by a token count: it fires only when the bytes differ and the
//! line vectors do not, so the message never competes with a change list. A
//! fixed allowance above raw was tried and could not be met by construction:
//! the shortest form of the message is ~20 tokens and a one-line pair is ~2, so
//! the ceiling sat under the message's own floor and dropped it on 90% of
//! one-line pairs.

use crate::core::tracking::estimate_tokens;

/// Returns `filtered`, or `raw` when `filtered` would emit more tokens.
pub fn never_worse<'a>(raw: &'a str, filtered: &'a str) -> &'a str {
    if estimate_tokens(filtered) > estimate_tokens(raw) {
        raw
    } else {
        filtered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_filtered_when_smaller() {
        let raw = "a".repeat(400);
        assert_eq!(never_worse(&raw, "ok"), "ok");
    }

    #[test]
    fn falls_back_to_raw_when_filtered_bigger() {
        let raw = "{}";
        let filtered = "{\n  \"pretty\": true\n}";
        assert_eq!(never_worse(raw, filtered), raw);
    }

    #[test]
    fn tie_keeps_filtered() {
        assert_eq!(never_worse("abcd", "wxyz"), "wxyz");
    }

    #[test]
    fn token_boundary_follows_estimate_tokens() {
        assert_eq!(never_worse("abcd", "abcde"), "abcd");
        assert_eq!(never_worse("abcdefgh", "ijklmnop"), "ijklmnop");
    }

    #[test]
    fn empty_raw_returns_raw() {
        assert_eq!(never_worse("", "0 matches"), "");
    }

    #[test]
    fn empty_filtered_returns_filtered() {
        assert_eq!(never_worse("data", ""), "");
    }

    #[test]
    fn both_empty_returns_filtered() {
        assert_eq!(never_worse("", ""), "");
    }
}
