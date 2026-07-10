//! Session-level output dedup: suppress re-emitting byte-identical command
//! output already seen earlier in the same Claude Code session.
//!
//! Correctness rests on two keys. The SHA-256 of the *raw* (pre-filter) bytes
//! is the guarantee — different content can never share a hash, so a changed
//! file or command output is never suppressed. The command `identity` is only
//! a cosmetic key used in the stub message; identity collisions are therefore
//! harmless. Every guard and every failure path (disabled, no session, command
//! failure, tiny output, DB error) falls back to emitting the original output,
//! so dedup can only ever *omit a repeat*, never hide new or changed output.
//!
//! Transitional: this module is wired into the print seams in Phase 5. The
//! module-level `allow(dead_code)` below is removed once that lands.
#![allow(dead_code)]

use std::borrow::Cow;

use sha2::{Digest, Sha256};

use super::config;
use super::session;
use super::tracking::{estimate_tokens, DedupRow, Tracker};

/// The knobs `maybe_suppress` needs, resolved from `[dedup]` config.
struct Params {
    enabled: bool,
    min_tokens: usize,
    suppress_on_error: bool,
}

/// SHA-256 of the raw bytes, hex-encoded. Collision-resistant, so identity
/// bucketing collisions can never cause a false suppression.
fn content_hash(raw: &str) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(raw.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for b in digest {
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

/// Format the one-line suppression stub. Machine-parseable (`rtk-dedup:`
/// prefix), carries the recovery command so the agent can force full output.
fn format_stub(prev: &DedupRow, identity: &str) -> String {
    format!(
        "rtk-dedup: identical to output at step {} · {} lines / ~{} tok suppressed \
         [{}] (force full: rtk proxy <cmd>)\n",
        prev.step_ordinal, prev.line_count, prev.output_tokens, identity
    )
}

/// Core logic, decoupled from global state for testing. Given an open tracker,
/// resolved params, and the session id, decide whether to suppress `shown`.
fn suppress_with<'a>(
    tracker: &Tracker,
    params: &Params,
    session_id: Option<&str>,
    identity: &str,
    raw: &str,
    shown: &'a str,
    exit_ok: bool,
) -> Cow<'a, str> {
    if !params.enabled {
        return Cow::Borrowed(shown);
    }
    let Some(session_id) = session_id else {
        return Cow::Borrowed(shown);
    };
    if !exit_ok && !params.suppress_on_error {
        return Cow::Borrowed(shown);
    }
    let shown_tokens = estimate_tokens(shown);
    if shown_tokens < params.min_tokens {
        return Cow::Borrowed(shown);
    }

    // The hash keys on raw (pre-filter) bytes; the ledger records the size of
    // what was actually emitted (`shown`) so the stub reports real savings.
    let hash = content_hash(raw);
    match tracker.dedup_lookup(session_id, &hash) {
        Ok(Some(prev)) => {
            let _ = tracker.dedup_bump(prev.id);
            Cow::Owned(format_stub(&prev, identity))
        }
        Ok(None) => {
            let lines = shown.lines().count();
            let _ = tracker.dedup_insert(session_id, &hash, identity, shown_tokens, lines);
            Cow::Borrowed(shown)
        }
        Err(_) => Cow::Borrowed(shown),
    }
}

/// Suppress `shown` if its raw bytes were already emitted this session; else
/// emit it and remember it. `raw` is the pre-filter output (hashed); `shown`
/// is what would otherwise be printed. Any guard failure, missing session, or
/// DB error emits `shown` unchanged.
///
/// The disabled and no-session fast paths do zero DB work, so with dedup off
/// (the default) this adds no measurable overhead.
pub fn maybe_suppress<'a>(
    identity: &str,
    raw: &str,
    shown: &'a str,
    exit_ok: bool,
) -> Cow<'a, str> {
    let cfg = config::dedup();
    if !cfg.enabled {
        return Cow::Borrowed(shown);
    }
    let Some(session_id) = session::current().id() else {
        return Cow::Borrowed(shown);
    };
    let tracker = match Tracker::new() {
        Ok(t) => t,
        Err(_) => return Cow::Borrowed(shown),
    };
    let params = Params {
        enabled: cfg.enabled,
        min_tokens: cfg.min_tokens,
        suppress_on_error: cfg.suppress_on_error,
    };
    suppress_with(
        &tracker,
        &params,
        Some(session_id),
        identity,
        raw,
        shown,
        exit_ok,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled_params() -> Params {
        Params {
            enabled: true,
            min_tokens: 200,
            suppress_on_error: false,
        }
    }

    /// A body large enough to clear the 200-token min gate (~4 chars/token).
    fn big_body() -> String {
        "line of output text\n".repeat(100) // ~2000 chars, ~500 tokens, 100 lines
    }

    #[test]
    fn hash_is_deterministic_and_content_sensitive() {
        assert_eq!(content_hash("abc"), content_hash("abc"));
        assert_ne!(content_hash("abc"), content_hash("abd"));
        assert_eq!(content_hash("abc").len(), 64); // hex SHA-256
    }

    #[test]
    fn stub_carries_recovery_and_stats() {
        let row = DedupRow {
            id: 1,
            step_ordinal: 4,
            output_tokens: 512,
            line_count: 100,
        };
        let stub = format_stub(&row, "read:foo.rs");
        assert!(stub.starts_with("rtk-dedup:"));
        assert!(stub.contains("step 4"));
        assert!(stub.contains("100 lines"));
        assert!(stub.contains("512 tok"));
        assert!(stub.contains("read:foo.rs"));
        assert!(stub.contains("rtk proxy"));
    }

    #[test]
    fn disabled_emits_full() {
        let t = Tracker::new_in_memory().expect("tracker");
        let params = Params {
            enabled: false,
            ..enabled_params()
        };
        let body = big_body();
        let out = suppress_with(&t, &params, Some("s1"), "id", &body, &body, true);
        assert_eq!(out, Cow::Borrowed(body.as_str()));
    }

    #[test]
    fn no_session_emits_full() {
        let t = Tracker::new_in_memory().expect("tracker");
        let body = big_body();
        let out = suppress_with(&t, &enabled_params(), None, "id", &body, &body, true);
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn command_failure_emits_full() {
        let t = Tracker::new_in_memory().expect("tracker");
        let body = big_body();
        // exit_ok = false, suppress_on_error = false (default)
        let out = suppress_with(&t, &enabled_params(), Some("s1"), "id", &body, &body, false);
        assert!(matches!(out, Cow::Borrowed(_)));
        // ...and nothing was recorded, so a later success still sees a miss.
        assert!(t
            .dedup_lookup("s1", &content_hash(&body))
            .unwrap()
            .is_none());
    }

    #[test]
    fn below_min_tokens_emits_full() {
        let t = Tracker::new_in_memory().expect("tracker");
        let out = suppress_with(
            &t,
            &enabled_params(),
            Some("s1"),
            "id",
            "tiny",
            "tiny",
            true,
        );
        assert_eq!(out, Cow::Borrowed("tiny"));
    }

    #[test]
    fn first_emit_full_then_identical_suppressed() {
        let t = Tracker::new_in_memory().expect("tracker");
        let body = big_body();
        // First emission: full, and recorded.
        let first = suppress_with(
            &t,
            &enabled_params(),
            Some("s1"),
            "read:x",
            &body,
            &body,
            true,
        );
        assert!(matches!(first, Cow::Borrowed(_)));
        // Second identical emission: suppressed to a stub.
        let second = suppress_with(
            &t,
            &enabled_params(),
            Some("s1"),
            "read:x",
            &body,
            &body,
            true,
        );
        match second {
            Cow::Owned(s) => {
                assert!(s.starts_with("rtk-dedup:"));
                assert!(s.contains("100 lines"));
            }
            Cow::Borrowed(_) => panic!("expected suppression on identical re-emit"),
        }
    }

    #[test]
    fn changed_content_emits_full() {
        let t = Tracker::new_in_memory().expect("tracker");
        let body = big_body();
        let changed = format!("{body}extra line\n");
        suppress_with(
            &t,
            &enabled_params(),
            Some("s1"),
            "read:x",
            &body,
            &body,
            true,
        );
        // Different raw bytes -> different hash -> not suppressed.
        let out = suppress_with(
            &t,
            &enabled_params(),
            Some("s1"),
            "read:x",
            &changed,
            &changed,
            true,
        );
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn suppress_on_error_allows_failure_dedup() {
        let t = Tracker::new_in_memory().expect("tracker");
        let params = Params {
            suppress_on_error: true,
            ..enabled_params()
        };
        let body = big_body();
        // exit_ok=false but suppress_on_error=true -> proceeds past the guard,
        // first emit records (miss -> borrowed).
        let first = suppress_with(&t, &params, Some("s1"), "id", &body, &body, false);
        assert!(matches!(first, Cow::Borrowed(_)));
        // Second identical failing emission is now suppressed.
        let second = suppress_with(&t, &params, Some("s1"), "id", &body, &body, false);
        assert!(matches!(second, Cow::Owned(_)));
    }
}
