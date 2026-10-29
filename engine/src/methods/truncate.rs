//! `truncate` layer: per-line char cap (`utils::truncate`) + line windowing
//! (`filter::smart_truncate`, `[N more lines]` trailer).
//!
//! Reserved: not in any built-in chain — the posthook path is conversion-only,
//! content is reformatted, never dropped.

use crate::block::Block;
use crate::method::{Method, MethodCtx, MethodInfo, Outcome};
use crate::text::smart_truncate;
use crate::text::truncate as truncate_line;

/// Line window for posthook payloads. Grep content arrives host-capped at
/// 250 lines; Read/WebFetch payloads beyond this are tails the recall file
/// preserves.
pub const MAX_LINES: usize = 300;
/// Per-line char cap — protects against minified one-liner blobs.
pub const MAX_LINE_CHARS: usize = 500;

pub struct Truncate;

impl Method for Truncate {
    fn name(&self) -> &'static str {
        "truncate"
    }

    fn info(&self) -> MethodInfo {
        MethodInfo {
            lossy: true,
            recovery: crate::method::Recovery::Recall,
            idempotent: false,
            ..MethodInfo::LOSSLESS_TRANSFORM
        }
    }

    fn apply(&self, block: &Block, _ctx: &MethodCtx) -> Outcome {
        let input = block.text;
        let needs_line_cap = input.lines().any(|l| l.chars().count() > MAX_LINE_CHARS);
        let needs_window = input.lines().count() > MAX_LINES;

        if !needs_line_cap && !needs_window {
            return Outcome::Continue(input.to_string());
        }

        let capped = if needs_line_cap {
            input
                .lines()
                .map(|l| truncate_line(l, MAX_LINE_CHARS))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            input.to_string()
        };

        if needs_window {
            Outcome::Continue(smart_truncate(&capped, MAX_LINES))
        } else {
            Outcome::Continue(capped)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::ContentFormat;

    fn ctx() -> MethodCtx<'static> {
        MethodCtx { source: "test" }
    }

    fn blk(text: &str) -> Block<'_> {
        crate::testutil::block(text, ContentFormat::Web)
    }

    fn apply(input: &str) -> String {
        match Truncate.apply(&blk(input), &ctx()) {
            Outcome::Continue(out) | Outcome::ShortCircuit(out) => out,
            Outcome::Skip => input.to_string(),
        }
    }

    #[test]
    fn test_short_content_unchanged() {
        let input = "line one\nline two";
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_long_line_capped() {
        let long = "x".repeat(2 * MAX_LINE_CHARS);
        let out = apply(&long);
        assert!(out.chars().count() <= MAX_LINE_CHARS);
        assert!(out.ends_with("..."));
    }

    #[test]
    fn test_many_lines_windowed_with_trailer() {
        let input = (0..2 * MAX_LINES)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = apply(&input);
        assert!(out.lines().count() <= MAX_LINES + 1);
        assert!(out.contains("more lines]"), "trailer must be present");
    }
}
