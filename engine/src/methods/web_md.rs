//! `web-md` layer: HTML → Markdown conversion via `htmd`.
//!
//! Skipped when content is already markdown-ish (Claude Code WebFetch
//! responses are usually pre-processed markdown summaries): HTML tag
//! density sniff on the first 1 KB decides.

use crate::block::Block;
use crate::method::{Method, MethodCtx, MethodInfo, Outcome};
use regex::Regex;
use std::sync::LazyLock;

static TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"</?[a-zA-Z][a-zA-Z0-9-]*(\s|>|/>)").unwrap());

/// Minimum opening/closing tags in the first 1 KB to call it HTML.
const TAG_DENSITY_THRESHOLD: usize = 4;

fn looks_like_html(content: &str) -> bool {
    let mut end = content.len().min(1024);
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    let head = &content[..end];

    let lower = head.to_lowercase();
    if lower.contains("<!doctype html") || lower.contains("<html") {
        return true;
    }
    TAG_RE.find_iter(head).take(TAG_DENSITY_THRESHOLD).count() >= TAG_DENSITY_THRESHOLD
}

pub struct WebMd;

impl Method for WebMd {
    fn name(&self) -> &'static str {
        "web-md"
    }

    fn info(&self) -> MethodInfo {
        MethodInfo {
            cost: crate::method::CostClass::Fast,
            consumes: &[crate::block::ContentFormat::Web],
            ..MethodInfo::LOSSLESS_TRANSFORM
        }
    }

    fn apply(&self, block: &Block, _ctx: &MethodCtx) -> Outcome {
        let input = block.text;
        if !looks_like_html(input) {
            return Outcome::Continue(input.to_string());
        }
        match htmd::convert(input) {
            Ok(md) => Outcome::Continue(md),
            Err(_) => Outcome::Continue(input.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::ContentFormat;

    fn ctx() -> MethodCtx<'static> {
        MethodCtx {
            source: "https://example.com",
        }
    }

    fn blk(text: &str) -> Block<'_> {
        crate::testutil::block(text, ContentFormat::Web)
    }

    fn apply(input: &str) -> String {
        match WebMd.apply(&blk(input), &ctx()) {
            Outcome::Continue(out) | Outcome::ShortCircuit(out) => out,
            Outcome::Skip => input.to_string(),
        }
    }

    #[test]
    fn test_real_html_page_converts_to_markdown() {
        let input = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/posthook/webpage_example_raw.html"
        ));
        let out = apply(input);
        assert!(!out.contains("<div"), "tags must be gone");
        assert!(!out.contains("<style"), "style must be gone");
        assert!(out.contains("Example Domain"), "text content preserved");
    }

    #[test]
    fn test_real_html_savings_at_least_20_percent() {
        let input = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/posthook/webpage_example_raw.html"
        ));
        let out = apply(input);
        let savings = 100.0 - (out.len() as f64 / input.len() as f64 * 100.0);
        assert!(
            savings >= 20.0,
            "web-md: expected ≥20% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_already_markdown_is_skipped() {
        // Real WebFetch result content (AI-processed markdown summary).
        let event: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/posthook/webfetch_markdown_url.json"
        )))
        .expect("fixture parses");
        let input = event["tool_response"]["result"]
            .as_str()
            .expect("fixture has result");
        let out = apply(input);
        assert_eq!(out, input, "markdown input must pass through untouched");
    }

    #[test]
    fn test_markdown_with_occasional_inline_tag_still_skipped() {
        let input = "# Title\n\nSome *markdown* with one <br> tag.\n\n- a\n- b\n";
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(apply(""), "");
    }
}
