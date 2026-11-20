//! `minify-json` layer: lossless JSON compaction via serde_json re-serialization.
//!
//! Not the lossy summarizers from `json_cmd`.

use crate::block::Block;
use crate::method::{Method, MethodCtx, MethodInfo, Outcome};

/// Parse and re-serialize compactly. Returns `None` when `input` is not JSON.
pub(super) fn minify(input: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(input).ok()?;
    serde_json::to_string(&value).ok()
}

pub struct MinifyJson;

impl Method for MinifyJson {
    fn name(&self) -> &'static str {
        "minify-json"
    }

    fn info(&self) -> MethodInfo {
        MethodInfo {
            cost: crate::method::CostClass::Fast,
            consumes: &[crate::block::ContentFormat::Json],
            produces: crate::method::Produces::Format(crate::block::ContentFormat::Json),
            // Same serde limits as toon: >i64 integers and duplicate keys do
            // not survive a parse-and-reserialize, so the method is labeled lossy.
            lossy: true,
            recovery: crate::method::Recovery::Recall,
            ..MethodInfo::LOSSLESS_TRANSFORM
        }
    }

    fn apply(&self, block: &Block, _ctx: &MethodCtx) -> Outcome {
        let input = block.text;
        match minify(input) {
            Some(minified) => Outcome::Continue(minified),
            None => Outcome::Continue(input.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::ContentFormat;

    fn ctx() -> MethodCtx<'static> {
        MethodCtx {
            source: "test.json",
        }
    }

    fn blk(text: &str) -> Block<'_> {
        crate::testutil::block(text, ContentFormat::Json)
    }

    #[test]
    fn test_minifies_pretty_json() {
        let input = "{\n  \"a\": 1,\n  \"b\": [\n    2,\n    3\n  ]\n}";
        let Outcome::Continue(out) = MinifyJson.apply(&blk(input), &ctx()) else {
            panic!("must continue");
        };
        assert_eq!(out, r#"{"a":1,"b":[2,3]}"#);
    }

    #[test]
    fn test_invalid_json_passes_through() {
        let Outcome::Continue(out) = MinifyJson.apply(&blk("not json {"), &ctx()) else {
            panic!("must continue");
        };
        assert_eq!(out, "not json {");
    }
}
