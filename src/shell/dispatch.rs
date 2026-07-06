//! Command-line routing for the rtk-managed shell.
//!
//! Given a raw line of input typed at the rtk-shell prompt (or passed via
//! `-c`), decide which parts of it can be rewritten through rtk's existing
//! filter/rewrite machinery ([`Filterable`](SegmentClassification::Filterable))
//! and which parts must be executed unmodified by the backing shell
//! ([`Forward`](SegmentClassification::Forward)).
//!
//! This module makes the *decision only* — it does not spawn any processes.
//! `oneshot` and `session` are responsible for actually executing the
//! classified segments.
//!
//! ## Rules
//!
//! - A line containing a pipeline (`|`), any redirect (`>`, `>>`, `<`, `<<`,
//!   `&>`, fd-dups like `2>&1`, etc.), or command/process substitution
//!   (`` `...` ``, `$(...)`, `<(...)`, `>(...)`) is **never split** — the
//!   whole original line is classified as a single [`Forward`] segment,
//!   because rewriting only one side of such constructs would change the
//!   command's meaning (e.g. what a pipe reads, or what a redirect writes).
//! - Otherwise the line is split on **top-level** `;`, `&&`, `||` (outside
//!   quotes/parens, courtesy of [`discover::lexer`]) into segments, and each
//!   segment is independently classified as [`Filterable`] (if
//!   [`hooks::hook_cmd::get_rewritten`] produces a rewrite for it) or
//!   [`Forward`] (unchanged).
//! - De-dup guard: if the first token of the *whole* line is literally
//!   `rtk`, the line is already an rtk invocation (or is being composed by
//!   something that already routes through rtk) — classify the whole line
//!   as [`Forward`] unchanged to avoid double rewriting/tracking.
//! - Raw-output escape hatch: if the `RTK_SHELL_RAW` environment variable is
//!   set to a non-empty value, filtering is disabled for the whole
//!   invocation/session — every line is classified as a single [`Forward`]
//!   segment, unmodified, regardless of what it contains. This is the
//!   documented kill-switch for working around a filter bug without
//!   uninstalling rtk-shell.
//!
//! [`Filterable`]: SegmentClassification::Filterable
//! [`Forward`]: SegmentClassification::Forward

use crate::discover::lexer::{self, TokenKind};
use crate::hooks::hook_cmd;

/// Environment variable that force-disables rtk-shell filtering entirely
/// when set to any non-empty value: every line is forwarded to the backing
/// shell unmodified, exactly as if it contained a pipeline/redirect. See the
/// "Raw-output escape hatch" rule in the module doc comment.
pub const RTK_SHELL_RAW_ENV: &str = "RTK_SHELL_RAW";

/// True if the raw-output escape hatch is enabled for this process, i.e.
/// [`RTK_SHELL_RAW_ENV`] is set in the environment to any non-empty value.
pub fn raw_mode_enabled() -> bool {
    std::env::var(RTK_SHELL_RAW_ENV).is_ok_and(|v| !v.is_empty())
}

/// The routing decision for a single segment of a command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentClassification {
    /// This segment has an rtk rewrite available.
    Filterable {
        /// The untouched original segment text (e.g. `"git status"`), as
        /// typed/found on the line. Callers that execute against a
        /// persistent backing shell (e.g. `shell::session`) must send this
        /// — not `rewritten` — to the backing shell, so `cd`/`export`/env
        /// state changes made by the segment are applied to the real
        /// session state rather than to a disconnected `rtk` subprocess.
        original: String,
        /// The rewritten (filterable) command (e.g. `"rtk git status"") that
        /// identifies which rtk filter applies. One-shot mode executes this
        /// directly (re-invoking the `rtk` binary); session mode uses it
        /// only to select which filter to apply to the raw output captured
        /// from executing `original`.
        rewritten: String,
    },
    /// This segment (or, when a whole-line guard triggered, the entire
    /// original line) must be executed unmodified by the backing shell.
    Forward(String),
}

/// Classify a raw command line into an ordered list of segments to execute.
///
/// The returned segments, when the backing shell re-joins them with their
/// original separators, are semantically equivalent to running the original
/// line as-is — except that [`SegmentClassification::Filterable`] segments
/// carry a rewritten command in place of the original text.
///
/// Callers should preserve the original operators (`;`, `&&`, `||`) between
/// segments when reconstructing a command to hand to the backing shell;
/// this function only returns the classified segment bodies, not the
/// separators between them.
pub fn classify_line(line: &str) -> Vec<SegmentClassification> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return vec![];
    }

    // Raw-output escape hatch: RTK_SHELL_RAW disables filtering entirely for
    // this process. Forward the whole line unmodified, before any other
    // classification rule runs.
    if raw_mode_enabled() {
        return vec![SegmentClassification::Forward(trimmed.to_string())];
    }

    // De-dup guard: "rtk ..." is already an rtk invocation; never rewrite or
    // re-split it, regardless of what else is on the line.
    if first_token_is_rtk(trimmed) {
        return vec![SegmentClassification::Forward(trimmed.to_string())];
    }

    // Pipelines, redirects, and command/process substitution must never be
    // split: rewriting only part of such a construct can change what the
    // pipe reads, what the redirect writes, or what the substitution
    // expands to. Forward the whole line unchanged.
    if has_pipeline_redirect_or_substitution(trimmed) {
        return vec![SegmentClassification::Forward(trimmed.to_string())];
    }

    // Split on top-level ';', '&&', '||' only (never '|', handled above).
    let segments = lexer::split_on_operators(trimmed, /* stop_at_pipe */ false);
    if segments.is_empty() {
        return vec![SegmentClassification::Forward(trimmed.to_string())];
    }

    segments
        .into_iter()
        .map(|segment| classify_segment(segment.trim()))
        .collect()
}

/// Classify a single already-split segment (no top-level `;`/`&&`/`||`/`|`
/// of its own) as [`Filterable`](SegmentClassification::Filterable) or
/// [`Forward`](SegmentClassification::Forward).
fn classify_segment(segment: &str) -> SegmentClassification {
    if segment.is_empty() {
        return SegmentClassification::Forward(segment.to_string());
    }

    match hook_cmd::get_rewritten(segment) {
        Some(rewritten) => SegmentClassification::Filterable {
            original: segment.to_string(),
            rewritten,
        },
        None => SegmentClassification::Forward(segment.to_string()),
    }
}

/// True if the first token of `cmd` is exactly `rtk` (not a prefix like
/// `rtkx`, and not `./rtk` or a path — only the bare literal command name).
fn first_token_is_rtk(cmd: &str) -> bool {
    lexer::tokenize(cmd)
        .into_iter()
        .find(|tok| tok.kind == TokenKind::Arg)
        .is_some_and(|tok| tok.value == "rtk")
}

/// True if `cmd` contains a pipeline, any redirect, or command/process
/// substitution anywhere at the top level. Unlike
/// [`lexer::contains_unattestable_construct`], this treats *every* redirect
/// (including fd-dups like `2>&1` and `>/dev/null`) as disqualifying a
/// split, because those still change what the shell does with a segment's
/// stdout/stderr and must stay attached to their original segment.
fn has_pipeline_redirect_or_substitution(cmd: &str) -> bool {
    if lexer::contains_unattestable_construct(cmd) {
        // Covers command/process substitution and file-target redirects.
        return true;
    }
    lexer::tokenize(cmd)
        .iter()
        .any(|tok| matches!(tok.kind, TokenKind::Pipe(_) | TokenKind::Redirect))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shared lock guarding tests that mutate the process-wide RTK_SHELL_RAW
    // env var, to avoid races when tests run in parallel.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_empty_line_yields_no_segments() {
        assert_eq!(classify_line(""), vec![]);
        assert_eq!(classify_line("   "), vec![]);
    }

    #[test]
    fn test_raw_mode_disabled_by_default() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var(RTK_SHELL_RAW_ENV);
        assert!(!raw_mode_enabled());
    }

    #[test]
    fn test_raw_mode_enabled_forwards_whole_line_unmodified() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var(RTK_SHELL_RAW_ENV, "1");
        let line = "git status && echo hi";
        let result = classify_line(line);
        std::env::remove_var(RTK_SHELL_RAW_ENV);
        assert_eq!(
            result,
            vec![SegmentClassification::Forward(line.to_string())]
        );
    }

    #[test]
    fn test_raw_mode_empty_value_does_not_enable() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var(RTK_SHELL_RAW_ENV, "");
        let enabled = raw_mode_enabled();
        std::env::remove_var(RTK_SHELL_RAW_ENV);
        assert!(!enabled, "empty RTK_SHELL_RAW should not enable raw mode");
    }

    #[test]
    fn test_rtk_prefixed_line_is_always_forwarded_whole() {
        let line = "rtk git status && cargo test";
        assert_eq!(
            classify_line(line),
            vec![SegmentClassification::Forward(line.to_string())]
        );
    }

    #[test]
    fn test_pipeline_is_forwarded_whole_even_with_operators() {
        let line = "git log | grep foo && echo done";
        assert_eq!(
            classify_line(line),
            vec![SegmentClassification::Forward(line.to_string())]
        );
    }

    #[test]
    fn test_redirect_is_forwarded_whole() {
        let line = "git status > out.txt";
        assert_eq!(
            classify_line(line),
            vec![SegmentClassification::Forward(line.to_string())]
        );
    }

    #[test]
    fn test_fd_dup_redirect_is_forwarded_whole() {
        // Unlike contains_unattestable_construct, fd-dups still block splitting
        // here because they're still redirects attached to a specific segment.
        let line = "cargo build 2>&1 && echo ok";
        assert_eq!(
            classify_line(line),
            vec![SegmentClassification::Forward(line.to_string())]
        );
    }

    #[test]
    fn test_command_substitution_is_forwarded_whole() {
        let line = "echo $(git rev-parse HEAD) && echo done";
        assert_eq!(
            classify_line(line),
            vec![SegmentClassification::Forward(line.to_string())]
        );
    }

    #[test]
    fn test_quoted_operators_do_not_split() {
        let line = r#"echo "a && b""#;
        let result = classify_line(line);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_splits_on_semicolon() {
        let result = classify_line("echo one; echo two");
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_splits_on_double_ampersand() {
        let result = classify_line("echo one && echo two");
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_splits_on_double_pipe() {
        let result = classify_line("echo one || echo two");
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_compound_line_only_eligible_segment_is_filtered() {
        // "git status" has an rtk rewrite available; "echo MARKER" does not.
        // Only the eligible segment should be classified Filterable — the
        // other segment must be forwarded unchanged, proving segments are
        // classified independently rather than the whole line being
        // rewritten-or-not as a unit.
        let result = classify_line("git status ; echo MARKER");
        assert_eq!(result.len(), 2);

        match &result[0] {
            SegmentClassification::Filterable {
                original,
                rewritten,
            } => {
                assert_eq!(original, "git status");
                assert!(
                    rewritten.starts_with("rtk "),
                    "expected a rewritten rtk invocation, got: {rewritten}"
                );
            }
            SegmentClassification::Forward(_) => {
                panic!("expected \"git status\" to be classified Filterable, got Forward")
            }
        }

        assert_eq!(
            result[1],
            SegmentClassification::Forward("echo MARKER".to_string())
        );
    }
}
