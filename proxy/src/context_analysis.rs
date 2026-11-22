//! Context-composition analysis.
//!
//! Estimates how a request body breaks down (tool output vs. tool-call
//! arguments vs. images vs. plain text/system/tool-defs), so `rtk gain
//! --watch` can show what's actually filling the context window. The logic
//! is provider-agnostic (Anthropic `tool_result`/`tool_use`, OpenAI-dialect
//! `image_url`).
//! Token counts are byte/4 estimates: cheap to compute on the body the
//! pipeline already holds in memory, and accurate enough for a share/ratio
//! display — never stores the bodies themselves.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// Flat per-image token allowance: a base64 payload's byte length has no
/// relation to its token cost (providers bill an image at roughly its
/// resized pixel footprint), so counting bytes/4 would overstate context by
/// 1-2 orders of magnitude. Presence, not payload size.
const IMAGE_TOKEN_EST: i64 = 1300;

/// Estimated breakdown of one request's context.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContextComposition {
    /// Estimated total context tokens (system + tools + all messages).
    pub total_tokens: i64,
    /// Estimated tokens inside `tool_result` blocks — command/file output
    /// that could be filtered at ingest (rtk-style), near-lossless.
    pub tool_result_tokens: i64,
    /// Estimated tokens inside `tool_use` block inputs — the assistant's
    /// call arguments (Write `content`, Edit `old_string`/`new_string`, …),
    /// resent every turn just like tool_result output.
    pub tool_use_input_tokens: i64,
    /// Estimated tokens that exactly repeat an earlier block in the same
    /// request (lossless dedup candidate).
    pub duplicate_tokens: i64,
    /// Estimated tokens for image / document blocks — a flat per-block
    /// allowance, also folded into `total_tokens`.
    pub image_tokens: i64,
}

/// Rough token estimate: ~4 chars/token (same heuristic rtk uses).
fn est_tokens(s: &str) -> i64 {
    (s.len() as f64 / 4.0).ceil() as i64
}

fn hash_str(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Which compression lever a noted text feeds.
#[derive(Clone, Copy)]
enum Kind {
    /// Plain context (system, text blocks, tool definitions): counted
    /// toward total only.
    Other,
    /// `tool_result` output — ingest-filter candidate.
    ToolResult,
    /// `tool_use` input value — ingest-filter candidate.
    ToolUseInput,
}

/// Accumulator threaded through the body walk.
struct Analyzer {
    comp: ContextComposition,
    /// Hash of every distinct block text seen so far (for duplicate detection).
    seen: HashMap<u64, i64>,
}

impl Analyzer {
    fn new() -> Self {
        Self {
            comp: ContextComposition::default(),
            seen: HashMap::new(),
        }
    }

    /// Account one block of text. A block whose exact text was already seen
    /// counts as duplicate (the repeated occurrence is what dedup would
    /// remove).
    fn note(&mut self, text: &str, kind: Kind) {
        if text.is_empty() {
            return;
        }
        let t = est_tokens(text);
        self.comp.total_tokens += t;
        match kind {
            Kind::ToolResult => self.comp.tool_result_tokens += t,
            Kind::ToolUseInput => self.comp.tool_use_input_tokens += t,
            Kind::Other => {}
        }
        let h = hash_str(text);
        match self.seen.entry(h) {
            std::collections::hash_map::Entry::Occupied(_) => self.comp.duplicate_tokens += t,
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(t);
            }
        }
    }
}

/// Pull the text out of a `tool_result` block's `content` (string, or array
/// of `{type:"text", text}` blocks), concatenated.
fn tool_result_text(block: &serde_json::Value) -> String {
    match block.get("content") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(inner)) => {
            let mut out = String::new();
            for tb in inner {
                if let Some(t) = tb.get("text").and_then(|v| v.as_str()) {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(t);
                }
            }
            out
        }
        _ => String::new(),
    }
}

/// Analyze a request body's context composition (Anthropic or OpenAI dialect).
pub fn analyze(body: &serde_json::Value) -> ContextComposition {
    let mut a = Analyzer::new();

    match body.get("system") {
        Some(serde_json::Value::String(s)) => a.note(s, Kind::Other),
        Some(serde_json::Value::Array(blocks)) => {
            for b in blocks {
                if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                    a.note(t, Kind::Other);
                }
            }
        }
        _ => {}
    }

    // Tool definitions: one stable JSON blob; counted toward total only.
    if let Some(tools) = body.get("tools") {
        if !tools.is_null() {
            a.comp.total_tokens += est_tokens(&tools.to_string());
        }
    }

    if let Some(messages) = body.get("messages").and_then(|m| m.as_array()) {
        for msg in messages {
            match msg.get("content") {
                Some(serde_json::Value::String(s)) => a.note(s, Kind::Other),
                Some(serde_json::Value::Array(blocks)) => {
                    for b in blocks {
                        match b.get("type").and_then(|t| t.as_str()) {
                            Some("text") => {
                                if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                                    a.note(t, Kind::Other);
                                }
                            }
                            Some("tool_result") => {
                                let txt = tool_result_text(b);
                                a.note(&txt, Kind::ToolResult);
                            }
                            Some("tool_use") => {
                                // The assistant's call. Account each top-level
                                // string VALUE in the input as a tool_use-input
                                // candidate; non-string fields count toward
                                // total only, via their JSON form.
                                if let Some(input) = b.get("input") {
                                    match input {
                                        serde_json::Value::Object(map) => {
                                            for v in map.values() {
                                                match v {
                                                    serde_json::Value::String(s) => {
                                                        a.note(s, Kind::ToolUseInput)
                                                    }
                                                    other => {
                                                        a.note(&other.to_string(), Kind::Other)
                                                    }
                                                }
                                            }
                                        }
                                        other => a.note(&other.to_string(), Kind::Other),
                                    }
                                }
                            }
                            Some("image") | Some("document") => {
                                if b.pointer("/source/data").and_then(|v| v.as_str()).is_some() {
                                    a.comp.total_tokens += IMAGE_TOKEN_EST;
                                    a.comp.image_tokens += IMAGE_TOKEN_EST;
                                }
                            }
                            // OpenAI-dialect image blocks carry a URL, not a
                            // base64 `source` — same flat allowance on presence.
                            Some("image_url")
                                if b.pointer("/image_url/url")
                                    .and_then(|v| v.as_str())
                                    .is_some() =>
                            {
                                a.comp.total_tokens += IMAGE_TOKEN_EST;
                                a.comp.image_tokens += IMAGE_TOKEN_EST;
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }

    a.comp
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn est_tokens_is_chars_over_four() {
        assert_eq!(est_tokens(""), 0);
        assert_eq!(est_tokens("abcd"), 1);
        assert_eq!(est_tokens("abcde"), 2); // ceil(5/4)
    }

    #[test]
    fn counts_tool_result_and_total() {
        let body = json!({
            "system": "you are helpful",                 // 15 chars -> 4 tok
            "messages": [
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "t1", "content": "12345678" } // 8 -> 2 tok
                ] }
            ]
        });
        let c = analyze(&body);
        assert_eq!(c.tool_result_tokens, 2);
        assert!(c.total_tokens >= 6); // system(4) + tool_result(2)
        assert_eq!(c.duplicate_tokens, 0);
    }

    #[test]
    fn detects_duplicate_blocks() {
        // The same file content appears in two tool_results -> the 2nd is dup.
        let big = "x".repeat(4000); // 1000 tok
        let body = json!({
            "messages": [
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "a", "content": big.clone() }
                ] },
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "b", "content": big.clone() }
                ] }
            ]
        });
        let c = analyze(&body);
        assert_eq!(c.tool_result_tokens, 2000); // both counted in tool_result total
        assert_eq!(c.duplicate_tokens, 1000); // one copy is redundant
    }

    #[test]
    fn counts_tool_use_input_string_values() {
        let body = json!({
            "messages": [
                { "role": "assistant", "content": [
                    { "type": "tool_use", "id": "u1", "name": "Write", "input": {
                        "file_path": "/x/y.rs",          // 7 chars -> 2 tok
                        "content": "abcdefgh",            // 8 chars -> 2 tok
                        "line": 12                        // number: not a string value
                    } }
                ] }
            ]
        });
        let c = analyze(&body);
        assert_eq!(c.tool_use_input_tokens, 4);
        assert_eq!(c.tool_result_tokens, 0);
        assert!(c.total_tokens >= 4);
    }

    #[test]
    fn tools_and_text_count_toward_total_not_tool_result() {
        let body = json!({
            "tools": [ { "name": "bash", "description": "run a shell command" } ],
            "messages": [
                { "role": "user", "content": "hello there" }
            ]
        });
        let c = analyze(&body);
        assert!(c.total_tokens > 0);
        assert_eq!(c.tool_result_tokens, 0);
    }

    #[test]
    fn openai_image_url_blocks_count_as_image_tokens() {
        let body = json!({
            "messages": [
                { "role": "user", "content": [
                    { "type": "image_url", "image_url": { "url": "https://x/y.png" } },
                    { "type": "text", "text": "describe this" }
                ] }
            ]
        });
        let c = analyze(&body);
        assert_eq!(c.image_tokens, IMAGE_TOKEN_EST);
        assert!(c.total_tokens >= IMAGE_TOKEN_EST);
    }
}
