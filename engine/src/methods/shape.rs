use crate::block::Block;
use crate::method::{CostClass, Method, MethodCtx, MethodInfo, Outcome, Recovery};
use crate::strategy;
use crate::tokens::estimate_tokens;

pub struct ShapeMethod;

impl Method for ShapeMethod {
    fn name(&self) -> &'static str {
        "shape"
    }

    fn info(&self) -> MethodInfo {
        MethodInfo {
            lossy: true,
            recovery: Recovery::Recall,
            idempotent: false,
            cost: CostClass::Fast,
            ..MethodInfo::LOSSLESS_TRANSFORM
        }
    }

    fn apply(&self, block: &Block, _ctx: &MethodCtx) -> Outcome {
        match strategy::render(block.text, 0, estimate_tokens) {
            Some(rendered) if estimate_tokens(&rendered.body) < estimate_tokens(block.text) => {
                Outcome::Continue(rendered.body)
            }
            _ => Outcome::Skip,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::ContentFormat;

    fn apply(input: &str) -> Option<String> {
        let ctx = MethodCtx::new("test");
        match ShapeMethod.apply(&crate::testutil::block(input, ContentFormat::Term), &ctx) {
            Outcome::Continue(out) | Outcome::ShortCircuit(out) => Some(out),
            Outcome::Skip => None,
        }
    }

    #[test]
    fn renders_a_smaller_body_for_large_flat_output() {
        let big: String = (0..500).map(|i| format!("line {i} of output\n")).collect();
        let out = apply(&big).expect("renders");
        assert!(estimate_tokens(&out) < estimate_tokens(&big));
    }

    #[test]
    fn small_or_unshapeable_input_is_skipped() {
        assert!(apply("short").is_none());
    }
}
