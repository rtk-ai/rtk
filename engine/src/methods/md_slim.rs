//! `md-slim` layer: conservative markdown slimming for web content.
//!
//! Strips CI/badge images, HTML comments, and collapses blank runs. Link
//! URLs and reference blocks are deliberately KEPT — agents follow them with
//! the next WebFetch; that's live signal, not noise. Web chain only: Read of
//! `.md` files is edit-anchored content and stays out of scope.

use crate::block::Block;
use crate::method::{Method, MethodCtx, MethodInfo, Outcome};
use regex::Regex;
use std::sync::LazyLock;

const BADGE_HOSTS: &str = r"img\.shields\.io|badge\.fury\.io|badgen\.net|travis-ci\.|circleci\.com|codecov\.io|coveralls\.io";

/// `[![alt](badge-url)](target)` — inline linked badge, removed whole.
static LINKED_BADGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"\[!\[[^\]]*\]\(https?://[^)]*(?:{BADGE_HOSTS}|badge\.svg)[^)]*\)\]\([^)]*\)"
    ))
    .expect("valid regex")
});

/// `![alt](badge-url)` — bare inline badge image.
static BARE_BADGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"!\[[^\]]*\]\(https?://[^)]*(?:{BADGE_HOSTS}|badge\.svg)[^)]*\)"
    ))
    .expect("valid regex")
});

/// `[![label]][target]` — reference-style image link (README badge idiom).
/// Trailing spaces only — never newlines, or the next line loses its `^` anchor.
static REF_BADGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[!\[[^\]]*\]\]\[[^\]]*\][ \t]*").expect("valid regex"));

/// `[label]: https://img.shields.io/...` — badge link-reference definition.
/// Non-badge reference definitions are KEPT (URLs are live signal).
static BADGE_DEF_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?m)^\[[^\]]+\]:\s+\S*(?:{BADGE_HOSTS})\S*\s*\n?"
    ))
    .expect("valid regex")
});

static HTML_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").expect("valid regex"));

pub struct MdSlim;

impl Method for MdSlim {
    fn name(&self) -> &'static str {
        "md-slim"
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
        let out = LINKED_BADGE.replace_all(input, "");
        let out = BARE_BADGE.replace_all(&out, "");
        let out = REF_BADGE.replace_all(&out, "");
        let out = BADGE_DEF_LINE.replace_all(&out, "");
        let out = HTML_COMMENT.replace_all(&out, "");
        let out = collapse_blank_runs(&out);
        Outcome::Continue(out)
    }
}

/// Collapse 3+ consecutive blank lines to 2 (matches CLAUDE.md-cleanup
/// convention elsewhere in the repo). Byte-identical on clean input —
/// trailing newline preserved so a no-op stays a no-op.
fn collapse_blank_runs(content: &str) -> String {
    let mut result: Vec<&str> = Vec::new();
    let mut blanks = 0;
    for line in content.lines() {
        if line.trim().is_empty() {
            blanks += 1;
            if blanks <= 2 {
                result.push("");
            }
        } else {
            blanks = 0;
            result.push(line);
        }
    }
    let mut out = result.join("\n");
    if content.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::ContentFormat;

    fn ctx() -> MethodCtx<'static> {
        MethodCtx {
            source: "https://example.com/readme",
        }
    }

    fn blk(text: &str) -> Block<'_> {
        crate::testutil::block(text, ContentFormat::Web)
    }

    fn apply(input: &str) -> String {
        match MdSlim.apply(&blk(input), &ctx()) {
            Outcome::Continue(out) | Outcome::ShortCircuit(out) => out,
            Outcome::Skip => input.to_string(),
        }
    }

    #[test]
    fn test_real_readme_badges_stripped() {
        // Real captured README (serde) with shields badges.
        let input = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/posthook/readme_serde_raw.md"
        ));
        let out = apply(input);
        assert!(!out.contains("img.shields.io"), "badges gone");
        assert!(out.len() < input.len(), "smaller");
        assert!(out.contains("Serde"), "prose preserved");
        assert!(out.contains("https://serde.rs"), "plain links preserved");
    }

    #[test]
    fn test_html_comments_stripped() {
        let input = "# Title\n<!-- internal\nnote -->\nBody text.";
        let out = apply(input);
        assert!(!out.contains("internal"));
        assert!(out.contains("Body text."));
    }

    #[test]
    fn test_blank_runs_collapsed() {
        let input = "a\n\n\n\n\nb";
        assert_eq!(apply(input), "a\n\n\nb");
    }

    #[test]
    fn test_plain_markdown_untouched() {
        let input = "# Title\n\nSome [link](https://example.com) and *emphasis*.";
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_non_badge_image_kept() {
        let input = "![diagram](https://example.com/architecture.png)";
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(apply(""), "");
    }
}
