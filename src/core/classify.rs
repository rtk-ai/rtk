//! Host side of `[classifier.*]`: asks the bound upstream the typed
//! questions the resolved rules reference, memoizes answers by content, and
//! turns them into the facts (`class.*`, `score.*`, `noul.*`) the matcher
//! tests. The engine never learns what HTTP is; this module never decides
//! what a label means.
//!
//! Contract, in order of importance:
//! - **Fail-open.** No key, endpoint down, timeout, malformed answer, low
//!   confidence: the fact is simply absent, and "missing fact never matches"
//!   turns absence into passthrough. Nothing here may error a hook.
//! - **Lazy.** Nothing runs unless an enabled rule at this exact position
//!   conditions on a classifier — the no-classifier hot path pays nothing.
//! - **Batched.** All questions bound to one upstream go out as a single
//!   request (the endpoint evaluates them in parallel).
//! - **Memoized.** Answers are cached by (classifier config, content head)
//!   in the tracking DB, so repeated outputs classify once.

use crate::core::tracking::Tracker;
use rtk_engine::pipeline::{ClassifierDef, Fact, Facts, Position, Resolved, UpstreamDef};
use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Duration;

pub const DEFAULT_INPUT_CHARS: usize = 2000;
pub const DEFAULT_MIN_CONFIDENCE: f64 = 0.8;
const DEFAULT_TIMEOUT_SECS: u64 = 8;

#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    Choice { label: String, confidence: f64 },
    Score { score: f64, confidence: f64 },
    Noul { value: f64 },
}

/// Ask every classifier the enabled rules at `position` reference, and insert
/// the resulting facts. The only public entry point, and the only fallible
/// path a hook touches — which is why it returns nothing.
pub fn insert_classifier_facts(
    resolved: &Resolved,
    position: Position,
    content: &str,
    call: Option<&str>,
    facts: &mut Facts,
) {
    let referenced = resolved.classifiers_referenced(position);
    if referenced.is_empty() {
        return;
    }
    let mut by_upstream: BTreeMap<(&str, bool), Vec<(&str, &ClassifierDef)>> = BTreeMap::new();
    for name in &referenced {
        let Some((def, _)) = resolved.classifiers.get(name) else {
            continue;
        };
        let Some(upstream) = def.upstream.as_deref() else {
            continue;
        };
        let with_call = def.context && call.is_some();
        by_upstream
            .entry((upstream, with_call))
            .or_default()
            .push((name, def));
    }
    let tracker = Tracker::new().ok();
    for ((upstream_name, with_call), questions) in by_upstream {
        let Some((upstream, _)) = resolved.upstreams.get(upstream_name) else {
            continue;
        };
        let framed;
        let (content, clip) = match call {
            Some(call) if with_call => {
                framed = with_call_context(call, content, &questions);
                (framed.as_str(), false)
            }
            _ => (content, true),
        };
        let mut misses: Vec<(&str, &ClassifierDef)> = Vec::new();
        for (name, def) in &questions {
            let key = memo_key(name, def, content, clip);
            match tracker
                .as_ref()
                .and_then(|t| t.classifier_cache_get(&key).ok().flatten())
            {
                Some(json) => {
                    if let Some(answer) = answer_from_json(&json) {
                        apply_answer(name, def, &answer, facts);
                    }
                }
                None => misses.push((name, def)),
            }
        }
        if misses.is_empty() {
            continue;
        }
        let Some((answers, (usage_in, usage_out))) =
            classify_batch(upstream, content, clip, &misses)
        else {
            continue;
        };
        let spent = (usage_in + usage_out).max(0) as usize;
        if spent > 0
            && let Some(t) = tracker.as_ref()
        {
            let _ = t.record_attribution("hook", "llm-spend", upstream_name, 0, spent);
        }
        for (name, def) in &misses {
            let Some(answer) = answers.get(*name) else {
                continue;
            };
            if let Some(t) = tracker.as_ref() {
                let _ = t.classifier_cache_put(
                    &memo_key(name, def, content, clip),
                    &answer_to_json(answer),
                );
            }
            apply_answer(name, def, answer, facts);
        }
    }
}

/// One request per upstream, all questions batched. `None` on any transport
/// or shape problem — the caller inserts nothing and the rules pass through.
fn classify_batch(
    upstream: &UpstreamDef,
    content: &str,
    clip: bool,
    questions: &[(&str, &ClassifierDef)],
) -> Option<(BTreeMap<String, Answer>, (i64, i64))> {
    let env = upstream.api_key_env.as_deref()?;
    let key = std::env::var(env).ok()?;
    let url = upstream.base_url.as_deref()?;
    let head = questions
        .iter()
        .map(|(_, d)| d.input_chars.unwrap_or(DEFAULT_INPUT_CHARS))
        .max()
        .unwrap_or(DEFAULT_INPUT_CHARS);
    let content = if clip {
        head_chars(content, head)
    } else {
        content
    };
    let body = build_request(upstream, content, questions);
    let timeout = Duration::from_secs(upstream.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
    let response = ureq::post(url)
        .timeout(timeout)
        .set("authorization", &format!("Bearer {key}"))
        .set("content-type", "application/json")
        .send_string(&body.to_string())
        .ok()?;
    let body = response.into_string().ok()?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    Some((parse_answers(&json), usage_of(&json)))
}

fn usage_of(response: &serde_json::Value) -> (i64, i64) {
    let read = |key: &str| {
        response
            .pointer(&format!("/usage/{key}"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
    };
    (read("input_tokens"), read("output_tokens"))
}

/// The wire format of `POST /v1/systemone`: state + named typed questions.
fn build_request(
    upstream: &UpstreamDef,
    state: &str,
    questions: &[(&str, &ClassifierDef)],
) -> serde_json::Value {
    let mut q = serde_json::Map::new();
    for (name, def) in questions {
        let mut entry = serde_json::Map::new();
        entry.insert("type".into(), def.question.clone().into());
        if let Some(instructions) = &def.instructions {
            entry.insert("instructions".into(), instructions.clone().into());
        }
        if def.question == "choice" {
            let criteria: serde_json::Map<String, serde_json::Value> = def
                .labels
                .iter()
                .map(|(label, criteria)| (label.clone(), criteria.clone().into()))
                .collect();
            entry.insert("criteria".into(), criteria.into());
        }
        if def.question == "score" {
            let rubric: Vec<serde_json::Value> = def
                .rubric
                .iter()
                .map(|level| level.clone().into())
                .collect();
            entry.insert("criteria".into(), rubric.into());
        }
        q.insert((*name).to_string(), entry.into());
    }
    serde_json::json!({
        "state": state,
        "model": upstream.model,
        "questions": q,
    })
}

/// Typed answers out of the response's `answers` map; unknown shapes are
/// skipped, never errors.
fn parse_answers(response: &serde_json::Value) -> BTreeMap<String, Answer> {
    let mut out = BTreeMap::new();
    let Some(answers) = response.get("answers").and_then(|a| a.as_object()) else {
        return out;
    };
    for (name, answer) in answers {
        let confidence = answer
            .get("confidence")
            .and_then(|c| c.as_f64())
            .unwrap_or(0.0);
        let parsed = match answer.get("type").and_then(|t| t.as_str()) {
            Some("choice") => {
                answer
                    .get("choice")
                    .and_then(|c| c.as_str())
                    .map(|label| Answer::Choice {
                        label: label.to_string(),
                        confidence,
                    })
            }
            Some("score") => answer
                .get("score")
                .and_then(|v| v.as_f64())
                .map(|score| Answer::Score { score, confidence }),
            Some("noul") => answer
                .get("noul")
                .and_then(|v| v.as_f64())
                .map(|value| Answer::Noul { value }),
            _ => None,
        };
        if let Some(parsed) = parsed {
            out.insert(name.clone(), parsed);
        }
    }
    out
}

/// Answers below the classifier's confidence floor insert nothing: a rule
/// that cannot trust the label must not fire on it.
fn apply_answer(name: &str, def: &ClassifierDef, answer: &Answer, facts: &mut Facts) {
    let floor = def.min_confidence.unwrap_or(DEFAULT_MIN_CONFIDENCE);
    match answer {
        Answer::Choice { label, confidence } => {
            if *confidence >= floor {
                facts.insert(format!("class.{name}"), Fact::Str(label.clone()));
                facts.insert(format!("class.{name}.confidence"), Fact::Num(*confidence));
            }
        }
        Answer::Score { score, confidence } => {
            if *confidence >= def.min_confidence.unwrap_or(0.0) {
                facts.insert(format!("score.{name}"), Fact::Num(*score));
            }
        }
        Answer::Noul { value } => {
            facts.insert(format!("noul.{name}"), Fact::Num(*value));
        }
    }
}

fn with_call_context(call: &str, output: &str, questions: &[(&str, &ClassifierDef)]) -> String {
    let budget = questions
        .iter()
        .map(|(_, d)| d.input_chars.unwrap_or(DEFAULT_INPUT_CHARS))
        .max()
        .unwrap_or(DEFAULT_INPUT_CHARS);
    let call = head_chars(call, budget);
    let output = head_chars(output, budget);
    format!("--- call ---\n{call}\n--- output ---\n{output}")
}

fn head_chars(content: &str, chars: usize) -> &str {
    match content.char_indices().nth(chars) {
        Some((idx, _)) => &content[..idx],
        None => content,
    }
}

fn memo_key(name: &str, def: &ClassifierDef, content: &str, clip: bool) -> String {
    let mut h = DefaultHasher::new();
    name.hash(&mut h);
    def.question.hash(&mut h);
    def.instructions.hash(&mut h);
    def.upstream.hash(&mut h);
    for (label, criteria) in &def.labels {
        label.hash(&mut h);
        criteria.hash(&mut h);
    }
    if clip {
        head_chars(content, def.input_chars.unwrap_or(DEFAULT_INPUT_CHARS)).hash(&mut h);
    } else {
        content.hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

fn answer_to_json(answer: &Answer) -> String {
    match answer {
        Answer::Choice { label, confidence } => {
            serde_json::json!({"type": "choice", "choice": label, "confidence": confidence})
        }
        Answer::Score { score, confidence } => {
            serde_json::json!({"type": "score", "score": score, "confidence": confidence})
        }
        Answer::Noul { value } => serde_json::json!({"type": "noul", "noul": value}),
    }
    .to_string()
}

fn answer_from_json(json: &str) -> Option<Answer> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let confidence = value
        .get("confidence")
        .and_then(|c| c.as_f64())
        .unwrap_or(0.0);
    match value.get("type").and_then(|t| t.as_str())? {
        "choice" => Some(Answer::Choice {
            label: value.get("choice")?.as_str()?.to_string(),
            confidence,
        }),
        "score" => Some(Answer::Score {
            score: value.get("score")?.as_f64()?,
            confidence,
        }),
        "noul" => Some(Answer::Noul {
            value: value.get("noul")?.as_f64()?,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtk_engine::pipeline::ClassifierDef;

    fn intent() -> ClassifierDef {
        let mut def = ClassifierDef {
            question: "choice".to_string(),
            instructions: Some("What kind of tool output is this?".to_string()),
            upstream: Some("judge".to_string()),
            ..ClassifierDef::default()
        };
        def.labels
            .insert("test_failure".into(), "Failing test output".into());
        def.labels
            .insert("build_log".into(), "Compiler output".into());
        def
    }

    #[test]
    fn score_requests_send_the_rubric_as_an_ordered_list() {
        let upstream = UpstreamDef {
            model: "judge-v1".to_string(),
            ..UpstreamDef::default()
        };
        let def = ClassifierDef {
            question: "score".to_string(),
            rubric: vec!["noise".to_string(), "actionable".to_string()],
            ..ClassifierDef::default()
        };
        let body = build_request(&upstream, "x", &[("quality", &def)]);
        assert_eq!(
            body["questions"]["quality"]["criteria"],
            serde_json::json!(["noise", "actionable"]),
            "the live API requires score criteria as a list"
        );
    }

    #[test]
    fn request_matches_the_documented_wire_format() {
        let upstream = UpstreamDef {
            model: "judge-v1".to_string(),
            ..UpstreamDef::default()
        };
        let def = intent();
        let body = build_request(&upstream, "some output", &[("intent", &def)]);
        assert_eq!(body["model"], "judge-v1");
        assert_eq!(body["state"], "some output");
        assert_eq!(body["questions"]["intent"]["type"], "choice");
        assert_eq!(
            body["questions"]["intent"]["criteria"]["test_failure"],
            "Failing test output"
        );
    }

    #[test]
    fn documented_response_parses_into_typed_answers() {
        let response: serde_json::Value = serde_json::from_str(
            r#"{
              "model": "judge-1.0.0",
              "answers": {
                "intent": {
                  "type": "choice",
                  "choice": "test_failure",
                  "probabilities": { "test_failure": 0.88, "build_log": 0.12 },
                  "confidence": 0.81
                },
                "quality": { "type": "score", "score": 0.3, "confidence": 0.9 },
                "is_error": { "type": "noul", "noul": 0.97 }
              },
              "usage": { "input_tokens": 318, "output_tokens": 34 }
            }"#,
        )
        .expect("valid json");
        let answers = parse_answers(&response);
        assert_eq!(
            answers["intent"],
            Answer::Choice {
                label: "test_failure".to_string(),
                confidence: 0.81
            }
        );
        assert_eq!(
            answers["quality"],
            Answer::Score {
                score: 0.3,
                confidence: 0.9
            }
        );
        assert_eq!(answers["is_error"], Answer::Noul { value: 0.97 });
    }

    #[test]
    fn low_confidence_inserts_no_fact() {
        let def = intent();
        let mut facts = Facts::new();
        apply_answer(
            "intent",
            &def,
            &Answer::Choice {
                label: "test_failure".to_string(),
                confidence: 0.4,
            },
            &mut facts,
        );
        assert!(facts.is_empty(), "below the floor nothing may match");
        apply_answer(
            "intent",
            &def,
            &Answer::Choice {
                label: "test_failure".to_string(),
                confidence: 0.93,
            },
            &mut facts,
        );
        assert_eq!(
            facts.get("class.intent"),
            Some(&Fact::Str("test_failure".to_string()))
        );
        assert_eq!(facts.get("class.intent.confidence"), Some(&Fact::Num(0.93)));
    }

    #[test]
    fn memo_key_tracks_config_and_content() {
        let def = intent();
        let base = memo_key("intent", &def, "output A", true);
        assert_eq!(base, memo_key("intent", &def, "output A", true));
        assert_ne!(base, memo_key("intent", &def, "output B", true));
        let mut renamed = def.clone();
        renamed.labels.insert("prose".into(), "Docs".into());
        assert_ne!(base, memo_key("intent", &renamed, "output A", true));
        let framed_a = with_call_context("intent: x", "output A", &[("intent", &def)]);
        let framed_b = with_call_context("intent: x", "output B", &[("intent", &def)]);
        assert_ne!(
            memo_key("intent", &def, &framed_a, false),
            memo_key("intent", &def, &framed_b, false)
        );
    }

    #[test]
    fn hostile_responses_never_panic_and_never_fabricate() {
        let hostile = [
            "".to_string(),
            "not json".to_string(),
            "null".to_string(),
            "[]".to_string(),
            "{}".to_string(),
            r#"{"answers": null}"#.to_string(),
            r#"{"answers": []}"#.to_string(),
            r#"{"answers": {"intent": null}}"#.to_string(),
            r#"{"answers": {"intent": {"type": "choice"}}}"#.to_string(),
            r#"{"answers": {"intent": {"type": "choice", "choice": 42}}}"#.to_string(),
            r#"{"answers": {"intent": {"type": "score", "score": "high"}}}"#.to_string(),
            r#"{"answers": {"intent": {"type": "wat", "choice": "x"}}}"#.to_string(),
            r#"{"answers": {"intent": {"type": "choice", "choice": "x", "confidence": "NaN"}}}"#
                .to_string(),
            format!(
                r#"{{"answers": {{"intent": {{"type": "choice", "choice": "{}", "confidence": 0.99}}}}}}"#,
                "A".repeat(100_000)
            ),
            format!("{}{}", "[".repeat(200), "]".repeat(200)),
        ];
        for body in &hostile {
            let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
                continue;
            };
            let answers = parse_answers(&json);
            for answer in answers.values() {
                match answer {
                    Answer::Choice { confidence, .. } | Answer::Score { confidence, .. } => {
                        assert!(confidence.is_finite())
                    }
                    Answer::Noul { value } => assert!(value.is_finite() || *value == 0.0),
                }
            }
        }
        for garbage in ["", "x", "{}", r#"{"type":"choice"}"#, "[1,2,3]", "\u{0000}"] {
            let _ = answer_from_json(garbage);
        }
    }

    #[test]
    fn hostile_content_builds_a_bounded_request() {
        let upstream = UpstreamDef {
            model: "judge-v1".to_string(),
            ..UpstreamDef::default()
        };
        let def = intent();
        for content in [
            "\u{0}\u{1b}[31m\u{7f}".repeat(5_000),
            "\u{1F600}".repeat(100_000),
            "a".repeat(5_000_000),
            String::new(),
        ] {
            let body = build_request(
                &upstream,
                head_chars(&content, DEFAULT_INPUT_CHARS),
                &[("intent", &def)],
            );
            let state = body["state"].as_str().expect("state stays a string");
            assert!(
                state.chars().count() <= DEFAULT_INPUT_CHARS,
                "head cap must bound what leaves the machine (and the bill)"
            );
            assert!(serde_json::to_string(&body).is_ok());
        }
    }

    #[test]
    fn answers_round_trip_through_the_cache_encoding() {
        for answer in [
            Answer::Choice {
                label: "build_log".to_string(),
                confidence: 0.88,
            },
            Answer::Score {
                score: 0.5,
                confidence: 0.7,
            },
            Answer::Noul { value: 0.2 },
        ] {
            assert_eq!(answer_from_json(&answer_to_json(&answer)), Some(answer));
        }
    }

    #[test]
    fn call_context_comes_first_and_keeps_the_output_budget() {
        let def = ClassifierDef {
            input_chars: Some(10),
            ..ClassifierDef::default()
        };
        let framed = with_call_context(
            "intent: understand the parser",
            "0123456789ABCDEF",
            &[("c", &def)],
        );
        assert!(framed.starts_with("--- call ---\nintent: un"));
        assert!(framed.ends_with("--- output ---\n0123456789"));
    }
}
