//! `toon` layer: shape-gated TOON encoding for JSON content.
//!
//! TOON wins on uniform arrays of flat objects
//! but loses to minified JSON on nested/non-uniform data. The shape gate is
//! therefore mandatory: gate → encode → compare vs minified → keep the
//! smaller. Parse failure → passthrough.

use crate::block::Block;
use crate::method::{Method, MethodCtx, MethodInfo, Outcome};
use serde_json::Value;

pub struct Toon;

impl Method for Toon {
    fn name(&self) -> &'static str {
        "toon"
    }

    fn info(&self) -> MethodInfo {
        MethodInfo {
            cost: crate::method::CostClass::Fast,
            terminal: true,
            // serde_json without arbitrary_precision collapses integers past
            // i64 to floats and last-wins duplicate keys, so the method is
            // labeled lossy and recall keeps the original reachable.
            lossy: true,
            recovery: crate::method::Recovery::Recall,
            ..MethodInfo::LOSSLESS_TRANSFORM
        }
    }

    fn apply(&self, block: &Block, _ctx: &MethodCtx) -> Outcome {
        let input = block.text;
        let Ok(value) = serde_json::from_str::<Value>(input) else {
            return Outcome::Continue(input.to_string());
        };
        let Ok(minified) = serde_json::to_string(&value) else {
            return Outcome::Continue(input.to_string());
        };

        // ShortCircuit on conversion: encoded/minified output is data the
        // model may need whole — a downstream truncate line-cap would chop a
        // minified one-liner to 500 chars and silently destroy it.
        if is_uniform_flat_array(&value) {
            if let Ok(toon) = toon_format::encode_default(&value) {
                if toon.len() < minified.len() {
                    return Outcome::ShortCircuit(toon);
                }
            }
        }
        Outcome::ShortCircuit(minified)
    }
}

/// TOON's winning shape: an array (≥2 items) of objects sharing one flat
/// key set (scalar values only).
fn is_uniform_flat_array(value: &Value) -> bool {
    let Value::Array(items) = value else {
        return false;
    };
    if items.len() < 2 {
        return false;
    }
    let Some(Value::Object(first)) = items.first() else {
        return false;
    };
    let keys: Vec<&String> = first.keys().collect();

    items.iter().all(|item| match item {
        Value::Object(obj) => {
            obj.len() == keys.len()
                && obj.keys().zip(keys.iter()).all(|(a, b)| a == *b)
                && obj.values().all(|v| !v.is_array() && !v.is_object())
        }
        _ => false,
    })
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

    fn apply(input: &str) -> String {
        match Toon.apply(&blk(input), &ctx()) {
            Outcome::Continue(out) | Outcome::ShortCircuit(out) => out,
            Outcome::Skip => input.to_string(),
        }
    }

    fn fixture_content(raw: &str) -> String {
        let event: serde_json::Value = serde_json::from_str(raw).expect("fixture parses");
        event["tool_response"]["file"]["content"]
            .as_str()
            .expect("fixture has file content")
            .to_string()
    }

    #[test]
    fn test_flat_uniform_array_encodes_to_toon() {
        let input = fixture_content(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/posthook/read_json_flat.json"
        )));
        let out = apply(&input);
        assert!(
            out.starts_with("[120]{id,name,email,active,score}:"),
            "expected TOON tabular header, got: {}",
            &out[..out.len().min(80)]
        );
    }

    #[test]
    fn test_flat_uniform_savings_at_least_20_percent() {
        let input = fixture_content(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/posthook/read_json_flat.json"
        )));
        let out = apply(&input);
        let savings = 100.0 - (out.len() as f64 / input.len() as f64 * 100.0);
        assert!(
            savings >= 20.0,
            "json chain: expected ≥20% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_nested_json_falls_back_to_minify() {
        let input = fixture_content(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/posthook/read_json_nested.json"
        )));
        let out = apply(&input);
        // Nested/non-uniform: TOON loses, minified JSON must win.
        assert!(out.trim_start().starts_with('{'), "expected minified JSON");
        let reparsed: serde_json::Value = serde_json::from_str(&out).expect("still valid JSON");
        let original: serde_json::Value = serde_json::from_str(&input).expect("valid input");
        assert_eq!(reparsed, original, "minify must be lossless");
        let savings = 100.0 - (out.len() as f64 / input.len() as f64 * 100.0);
        assert!(
            savings >= 20.0,
            "nested json minify: expected ≥20% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_non_uniform_array_falls_back_to_minify() {
        let input = r#"[{"a": 1}, {"b": {"nested": true}}]"#;
        let out = apply(input);
        assert_eq!(out, r#"[{"a":1},{"b":{"nested":true}}]"#);
    }

    #[test]
    fn test_invalid_json_passes_through() {
        assert_eq!(apply("not json at all"), "not json at all");
    }

    #[test]
    fn test_empty_input_passes_through() {
        assert_eq!(apply(""), "");
    }
}
