use crate::block::Block;
use crate::method::{CostClass, Method, MethodCtx, MethodInfo, Outcome, Recovery};
use crate::strategy;

pub struct Rle;

impl Method for Rle {
    fn name(&self) -> &'static str {
        "rle"
    }

    fn info(&self) -> MethodInfo {
        MethodInfo {
            cost: CostClass::Fast,
            // The collapse keys on the TRIMMED line, so differently-indented
            // lines report as "xN identical" and only the first survives.
            lossy: true,
            recovery: Recovery::Recall,
            ..MethodInfo::LOSSLESS_TRANSFORM
        }
    }

    fn apply(&self, block: &Block, _ctx: &MethodCtx) -> Outcome {
        match strategy::rle_only(block.text) {
            Some(collapsed) => Outcome::Continue(collapsed),
            None => Outcome::Skip,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::ContentFormat;

    fn apply(input: &str) -> Option<String> {
        let ctx = MethodCtx::new("test");
        match Rle.apply(&crate::testutil::block(input, ContentFormat::Term), &ctx) {
            Outcome::Continue(out) | Outcome::ShortCircuit(out) => Some(out),
            Outcome::Skip => None,
        }
    }

    #[test]
    fn collapses_adjacent_repeats_and_skips_unique_content() {
        let repetitive = "warn: retry\n".repeat(30) + "done";
        let out = apply(&repetitive).expect("collapses");
        assert!(out.len() < repetitive.len());
        assert!(out.contains("done"));

        let unique: String = (0..20).map(|i| format!("line {i}\n")).collect();
        assert!(apply(&unique).is_none());
    }

    #[test]
    fn collapse_is_idempotent() {
        let repetitive = "warn: retry\n".repeat(30) + "done";
        let once = apply(&repetitive).expect("collapses");
        assert!(apply(&once).is_none(), "second pass finds nothing to do");
    }
}
