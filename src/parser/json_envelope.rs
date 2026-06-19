//! JSON envelope for `--json` global flag.
//!
//! Wraps a [`ParseResult<T>`] in a stable, machine-readable shape so orchestrators
//! and CI parsers can consume RTK output without dealing with formatter truncation.

use serde::{Deserialize, Serialize};

use super::ParseResult;

/// Tier marker for the JSON envelope.
///
/// Mirrors [`ParseResult`] variants but serialises as lowercase strings:
/// `"full"`, `"degraded"`, `"passthrough"`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JsonTier {
    Full,
    Degraded,
    Passthrough,
}

/// Stable JSON envelope shape for `--json` output.
///
/// Field semantics (see spec 007-rkt-json):
/// - `data` is present iff `tier ∈ {full, degraded}`.
/// - `warnings` is present iff `tier == degraded` (signals degraded tier explicitly).
/// - `raw` is present iff `tier == passthrough` (truncated per `passthrough_max_chars`).
///
/// Also derives [`Deserialize`] so consumers (and our own tests) can round-trip
/// the envelope via `serde_json::from_str::<JsonEnvelope<T>>`.
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonEnvelope<T> {
    pub tool: String,
    pub tier: JsonTier,
    pub exit: i32,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<String>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

/// Construct a [`JsonEnvelope`] from a [`ParseResult`] and the underlying tool's exit code.
///
/// The envelope handles tier branching so command modules only need a single
/// `if json { ... }` block in their `run` function.
pub fn build_json_envelope<T>(
    tool: &'static str,
    result: ParseResult<T>,
    exit_code: i32,
) -> JsonEnvelope<T> {
    match result {
        ParseResult::Full(data) => JsonEnvelope {
            tool: tool.to_string(),
            tier: JsonTier::Full,
            exit: exit_code,
            data: Some(data),
            warnings: None,
            raw: None,
        },
        ParseResult::Degraded(data, warnings) => JsonEnvelope {
            tool: tool.to_string(),
            tier: JsonTier::Degraded,
            exit: exit_code,
            data: Some(data),
            warnings: Some(warnings),
            raw: None,
        },
        ParseResult::Passthrough(raw) => JsonEnvelope {
            tool: tool.to_string(),
            tier: JsonTier::Passthrough,
            exit: exit_code,
            data: None,
            warnings: None,
            raw: Some(raw),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn full_envelope_has_data_no_warnings_no_raw() {
        let env = build_json_envelope("vitest", ParseResult::Full(42_i32), 0);
        let json: Value = serde_json::from_str(&serde_json::to_string(&env).unwrap()).unwrap();

        assert_eq!(json["tool"], "vitest");
        assert_eq!(json["tier"], "full");
        assert_eq!(json["exit"], 0);
        assert_eq!(json["data"], 42);
        assert!(json.get("warnings").is_none(), "warnings must be omitted on full tier");
        assert!(json.get("raw").is_none(), "raw must be omitted on full tier");
    }

    #[test]
    fn degraded_envelope_has_data_and_warnings() {
        let env = build_json_envelope(
            "vitest",
            ParseResult::Degraded(7_i32, vec!["regex fallback".to_string()]),
            1,
        );
        let json: Value = serde_json::from_str(&serde_json::to_string(&env).unwrap()).unwrap();

        assert_eq!(json["tier"], "degraded");
        assert_eq!(json["data"], 7);
        assert_eq!(json["warnings"][0], "regex fallback");
        assert!(json.get("raw").is_none());
    }

    #[test]
    fn degraded_envelope_keeps_warnings_field_when_empty() {
        // R6: warnings field is always present for degraded, even if empty
        let env = build_json_envelope("vitest", ParseResult::Degraded(0_i32, vec![]), 0);
        let json: Value = serde_json::from_str(&serde_json::to_string(&env).unwrap()).unwrap();

        assert_eq!(json["tier"], "degraded");
        assert!(
            json.get("warnings").is_some(),
            "warnings must be present (as empty array) on degraded tier"
        );
        assert!(json["warnings"].as_array().unwrap().is_empty());
    }

    #[test]
    fn passthrough_envelope_has_raw_no_data_no_warnings() {
        let env = build_json_envelope::<i32>(
            "vitest",
            ParseResult::Passthrough("oops".to_string()),
            2,
        );
        let json: Value = serde_json::from_str(&serde_json::to_string(&env).unwrap()).unwrap();

        assert_eq!(json["tier"], "passthrough");
        assert_eq!(json["exit"], 2);
        assert_eq!(json["raw"], "oops");
        assert!(json.get("data").is_none());
        assert!(json.get("warnings").is_none());
    }

    #[test]
    fn exit_code_preserved() {
        let env = build_json_envelope("vitest", ParseResult::Full(0_i32), -1);
        let json: Value = serde_json::from_str(&serde_json::to_string(&env).unwrap()).unwrap();
        assert_eq!(json["exit"], -1);
    }
}
