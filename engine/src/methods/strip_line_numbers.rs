use crate::block::{Block, Layer};
use crate::method::{Method, MethodCtx, MethodInfo, Outcome, Recovery};
use crate::shape;

pub struct StripLineNumbers;

impl Method for StripLineNumbers {
    fn name(&self) -> &'static str {
        "strip-line-numbers"
    }

    fn info(&self) -> MethodInfo {
        MethodInfo {
            lossy: true,
            recovery: Recovery::None,
            idempotent: false,
            saves_on: &[Layer::Read],
            ..MethodInfo::LOSSLESS_TRANSFORM
        }
    }

    fn apply(&self, block: &Block, _ctx: &MethodCtx) -> Outcome {
        let normalized = shape::normalize(block.text);
        if !normalized.had_numbering || !normalized.file_coords {
            return Outcome::Skip;
        }
        let stripped: Vec<&str> = normalized
            .lines
            .iter()
            .map(|line| line.content.as_str())
            .collect();
        Outcome::Continue(stripped.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::ContentFormat;
    use crate::testutil;

    fn apply(input: &str) -> Outcome {
        let block = testutil::block(input, ContentFormat::Term);
        StripLineNumbers.apply(&block, &MethodCtx { source: "test" })
    }

    #[test]
    fn strips_arrow_numbered_file_reads() {
        let input = (1..=8)
            .map(|i| format!("{i}\u{2192}line {i} content"))
            .collect::<Vec<_>>()
            .join("\n");
        match apply(&input) {
            Outcome::Continue(out) => {
                assert!(out.starts_with("line 1 content"));
                assert!(!out.contains('\u{2192}'));
            }
            other => panic!("expected Continue, got {other:?}"),
        }
    }

    #[test]
    fn skips_grep_style_colon_numbers() {
        let input = (1..=8)
            .map(|i| format!("{i}:match {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(apply(&input), Outcome::Skip);
    }

    #[test]
    fn skips_unnumbered_text() {
        assert_eq!(apply("plain line\nanother line\n"), Outcome::Skip);
    }
}
