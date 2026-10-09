//! Calls the TypeSafe API (https://typesafe.ai) and prints a compact answer.
//!
//! Wraps `POST https://api.typesafe.ai/v1/systemone`. The full response
//! (model name, token usage, full probability maps) is ~250 tokens; the
//! compact form (just the answer and confidence per question) is ~25 tokens.
//!
//! Auth: reads `TYPESAFE_API_KEY` from the environment. Returns exit 2
//! if missing.
//!
//! Two output modes:
//! - default: one line per question id — `<qid>: <answer> (conf <n>)`
//! - `--raw`: passthrough of the full JSON response, for callers that need
//!   the probability distribution or the usage block.
//!
//! State source: `--state` argument, or `-` / `--state -` to read stdin,
//! or `@path` / `--state @path` to read a file. Stdin is the right choice
//! for long inputs (PR bodies, support tickets, log dumps).

use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::io::Read;

const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
const MODEL: &str = "jev-latest";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Primitive {
    Noul,
    Choice,
    Score,
}

impl Primitive {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "noul" => Some(Self::Noul),
            "choice" => Some(Self::Choice),
            "score" => Some(Self::Score),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct Criteria {
    /// For Choice: ordered map of option → description.
    /// For Score: ordered list of level descriptions.
    /// For Noul: optional (true_label, false_label).
    raw: CriteriaKind,
}

#[derive(Debug)]
enum CriteriaKind {
    Map(Vec<(String, String)>),
    Levels(Vec<String>),
    Noul { t: String, f: String },
}

pub fn run(
    primitive: &str,
    state: &str,
    instructions: &str,
    criteria: &[String],
    raw_output: bool,
    verbose: u8,
) -> Result<i32> {
    let prim = Primitive::parse(primitive)
        .with_context(|| format!("unknown primitive '{}' (expected noul|choice|score)", primitive))?;

    let api_key = std::env::var("TYPESAFE_API_KEY")
        .context("TYPESAFE_API_KEY not set — export it before running rtk typesafe")?;
    if api_key.is_empty() {
        bail!("TYPESAFE_API_KEY is empty");
    }

    let state_text = resolve_state(state).context("failed to read --state")?;
    let crit = build_criteria(prim, criteria).context("invalid --criteria")?;
    let body = build_request(&state_text, instructions, prim, &crit)?;

    if verbose > 0 {
        eprintln!("typesafe: {} question(s), {} state bytes", 1, state_text.len());
    }

    let raw_response = call_api(&api_key, &body).context("TypeSafe API call failed")?;

    let shown = if raw_output {
        raw_response.clone()
    } else {
        compact(&raw_response)
    };

    println!("{}", shown);

    Ok(0)
}

fn resolve_state(arg: &str) -> Result<String> {
    if arg == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        return Ok(buf);
    }
    if let Some(path) = arg.strip_prefix('@') {
        return Ok(std::fs::read_to_string(path)
            .with_context(|| format!("read state file {}", path))?);
    }
    Ok(arg.to_string())
}

fn build_criteria(prim: Primitive, args: &[String]) -> Result<Criteria> {
    // Empty / missing criteria is OK for Noul (defaults to yes/no).
    let entries: Vec<&str> = args
        .iter()
        .flat_map(|s| s.split(','))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    match prim {
        Primitive::Noul => {
            let (t, f) = match entries.len() {
                0 => ("yes".to_string(), "no".to_string()),
                1 => bail!("noul needs two criteria: true and false"),
                _ => (entries[0].to_string(), entries[1].to_string()),
            };
            Ok(Criteria { raw: CriteriaKind::Noul { t, f } })
        }
        Primitive::Choice => {
            if entries.is_empty() {
                bail!("choice needs --criteria 'k:desc,k:desc,...'");
            }
            let mut map = Vec::new();
            for entry in entries {
                let (k, d) = entry
                    .split_once(':')
                    .with_context(|| format!("choice entry '{}' must be 'key:description'", entry))?;
                map.push((k.to_string(), d.to_string()));
            }
            Ok(Criteria { raw: CriteriaKind::Map(map) })
        }
        Primitive::Score => {
            if entries.len() < 2 {
                bail!("score needs --criteria 'level1,level2,...'");
            }
            let levels: Vec<String> = entries.iter().map(|s| s.to_string()).collect();
            Ok(Criteria { raw: CriteriaKind::Levels(levels) })
        }
    }
}

fn build_request(state: &str, instructions: &str, prim: Primitive, c: &Criteria) -> Result<Value> {
    let qid = "q";
    let criteria_json = match &c.raw {
        CriteriaKind::Map(m) => {
            let obj: serde_json::Map<String, Value> = m
                .iter()
                .map(|(k, d)| (k.clone(), Value::String(d.clone())))
                .collect();
            Value::Object(obj)
        }
        CriteriaKind::Levels(l) => Value::Array(l.iter().map(|s| Value::String(s.clone())).collect()),
        CriteriaKind::Noul { t, f } => json!({ "true": t, "false": f }),
    };
    let type_str = match prim {
        Primitive::Noul => "noul",
        Primitive::Choice => "choice",
        Primitive::Score => "score",
    };
    let question = json!({
        "type": type_str,
        "instructions": instructions,
        "criteria": criteria_json,
    });
    Ok(json!({
        "state": state,
        "model": MODEL,
        "questions": { qid: question },
    }))
}

fn call_api(api_key: &str, body: &Value) -> Result<String> {
    let resp = ureq::post(ENDPOINT)
        .set("Authorization", &format!("Bearer {}", api_key))
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .context("request to typesafe failed")?;
    let mut buf = String::new();
    resp.into_reader().read_to_string(&mut buf)?;
    Ok(buf)
}

#[derive(Serialize)]
struct CompactRow<'a> {
    qid: &'a str,
    kind: &'a str,
    answer: String,
    confidence: Option<f64>,
}

fn compact(raw_json: &str) -> String {
    let v: Value = match serde_json::from_str(raw_json) {
        Ok(v) => v,
        Err(_) => return raw_json.to_string(),
    };
    let answers = match v.get("answers").and_then(|a| a.as_object()) {
        Some(a) => a,
        None => return raw_json.to_string(),
    };
    let mut rows: Vec<CompactRow> = Vec::new();
    for (qid, ans) in answers {
        let kind = ans.get("type").and_then(|t| t.as_str()).unwrap_or("?");
        let (answer, confidence) = match kind {
            "choice" => (
                ans.get("choice")
                    .and_then(|c| c.as_str())
                    .unwrap_or("?")
                    .to_string(),
                ans.get("confidence").and_then(|c| c.as_f64()),
            ),
            "noul" => (
                format!(
                    "{:.3}",
                    ans.get("noul").and_then(|n| n.as_f64()).unwrap_or(0.0)
                ),
                None,
            ),
            "score" => (
                format!(
                    "{:.2}",
                    ans.get("score").and_then(|s| s.as_f64()).unwrap_or(0.0)
                ),
                ans.get("confidence").and_then(|c| c.as_f64()),
            ),
            _ => ("?".to_string(), None),
        };
        rows.push(CompactRow { qid, kind, answer, confidence });
    }
    let mut out = String::new();
    for r in &rows {
        out.push_str(&format!("{}: {} ({})", r.qid, r.answer, r.kind));
        if let Some(c) = r.confidence {
            out.push_str(&format!(" conf={:.2}", c));
        }
        out.push('\n');
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitive_parse() {
        assert_eq!(Primitive::parse("noul"), Some(Primitive::Noul));
        assert_eq!(Primitive::parse("choice"), Some(Primitive::Choice));
        assert_eq!(Primitive::parse("score"), Some(Primitive::Score));
        assert_eq!(Primitive::parse("bogus"), None);
    }

    #[test]
    fn criteria_choice_parses_key_value_pairs() {
        let c = build_criteria(
            Primitive::Choice,
            &["billing:Payments".into(), "tech:Bugs".into()],
        )
        .unwrap();
        match c.raw {
            CriteriaKind::Map(m) => {
                assert_eq!(m.len(), 2);
                assert_eq!(m[0].0, "billing");
                assert_eq!(m[1].1, "Bugs");
            }
            _ => panic!("expected Map"),
        }
    }

    #[test]
    fn criteria_choice_rejects_missing_colon() {
        let err = build_criteria(Primitive::Choice, &["nocolon".into()]);
        assert!(err.is_err());
    }

    #[test]
    fn criteria_score_parses_levels() {
        let c = build_criteria(
            Primitive::Score,
            &["Calm, Frustrated, Very angry".into()],
        )
        .unwrap();
        match c.raw {
            CriteriaKind::Levels(l) => assert_eq!(l, vec!["Calm", "Frustrated", "Very angry"]),
            _ => panic!("expected Levels"),
        }
    }

    #[test]
    fn criteria_score_requires_two_levels() {
        let err = build_criteria(Primitive::Score, &["only-one".into()]);
        assert!(err.is_err());
    }

    #[test]
    fn criteria_noul_defaults_yes_no() {
        let c = build_criteria(Primitive::Noul, &[]).unwrap();
        match c.raw {
            CriteriaKind::Noul { t, f } => {
                assert_eq!(t, "yes");
                assert_eq!(f, "no");
            }
            _ => panic!("expected Noul"),
        }
    }

    #[test]
    fn request_body_uses_jev_latest_and_question_map() {
        let c = Criteria { raw: CriteriaKind::Noul { t: "yes".into(), f: "no".into() } };
        let body = build_request("hello", "is urgent?", Primitive::Noul, &c).unwrap();
        assert_eq!(body["model"], "jev-latest");
        assert_eq!(body["state"], "hello");
        assert_eq!(body["questions"]["q"]["type"], "noul");
        assert_eq!(body["questions"]["q"]["instructions"], "is urgent?");
        assert_eq!(body["questions"]["q"]["criteria"]["true"], "yes");
    }

    #[test]
    fn compact_choice_extracts_label_and_confidence() {
        let raw = r#"{"answers":{"dept":{"type":"choice","choice":"billing","confidence":0.91,"probabilities":{"billing":0.91,"tech":0.09}}},"model":"jev-latest","usage":{"input_tokens":42,"output_tokens":3}}"#;
        let out = compact(raw);
        assert!(out.contains("dept: billing (choice)"));
        assert!(out.contains("conf=0.91"));
        // raw_response is ~180 chars; compact should be much smaller
        assert!(out.len() < raw.len() / 3, "compact should reduce bytes by >66% (raw={}, compact={})", raw.len(), out.len());
    }

    #[test]
    fn compact_noul_extracts_probability() {
        let raw = r#"{"answers":{"urgent":{"type":"noul","noul":0.97}},"model":"jev-latest","usage":{}}"#;
        let out = compact(raw);
        assert!(out.contains("urgent: 0.970 (noul)"));
    }

    #[test]
    fn compact_score_extracts_position() {
        let raw = r#"{"answers":{"rel":{"type":"score","score":1.79,"confidence":0.4,"legend":{"0":"Irrelevant","1":"Tangential"}}},"model":"jev-latest","usage":{}}"#;
        let out = compact(raw);
        assert!(out.contains("rel: 1.79 (score)"));
        assert!(out.contains("conf=0.40"));
    }

    #[test]
    fn compact_invalid_json_passes_through() {
        let raw = "not json at all";
        assert_eq!(compact(raw), raw);
    }

    #[test]
    fn real_fixture_reduces_bytes_by_at_least_60_percent() {
        // Captured live from POST /v1/systemone with a real API key.
        let raw = include_str!("../../../tests/fixtures/typesafe_response_raw.json");
        let out = compact(raw);
        let input_bytes = raw.len();
        let output_bytes = out.len();
        let savings = 100.0 * (1.0 - output_bytes as f64 / input_bytes as f64);
        assert!(
            savings >= 60.0,
            "Expected ≥60% byte savings on real fixture, got {:.1}% ({} → {} bytes)",
            savings, input_bytes, output_bytes
        );
    }

    #[test]
    fn real_fixture_preserves_answer_values() {
        let raw = include_str!("../../../tests/fixtures/typesafe_response_raw.json");
        let out = compact(raw);
        // Real fixture is a Choice + Noul pair: billing with high confidence,
        // noul near 1.0. Check the model survives the compaction.
        assert!(out.contains("department: billing (choice)"));
        assert!(out.contains("conf=0.98"));
        assert!(out.contains("urgent: 0.960 (noul)"));
    }
}