//! Cross-dialect extraction helpers: pull model, prompt text, token usage and
//! client identity out of provider-specific JSON shapes.

use super::model::Provider;
use sha2::{Digest, Sha256};

/// Extract the `model` field from a request body (both dialects use `model`).
/// The requested output-token budget, if present. Anthropic/OpenAI use
/// top-level `max_tokens`; Gemini nests it as `generationConfig.maxOutputTokens`.
pub fn max_tokens(body: &serde_json::Value) -> Option<i64> {
    body.get("max_tokens")
        .and_then(|v| v.as_i64())
        .or_else(|| {
            body.pointer("/generationConfig/maxOutputTokens")
                .and_then(|v| v.as_i64())
        })
        .filter(|n| *n > 0)
}

pub fn model(body: &serde_json::Value) -> String {
    body.get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string()
}

/// Whether the client requested streaming.
pub fn stream(body: &serde_json::Value) -> bool {
    body.get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Count image blocks across the request body — Anthropic `type:"image"`,
/// OpenAI `type:"image_url"`, Gemini inline-data parts. Images are billed by the
/// provider but carry no countable text, so [`prompt_text`] misses them; the
/// estimate adds a flat per-image allowance for each one this returns.
pub fn image_count(body: &serde_json::Value) -> i64 {
    let mut n = 0i64;
    if let Some(messages) = body.get("messages").and_then(|m| m.as_array()) {
        for msg in messages {
            if let Some(blocks) = msg.get("content").and_then(|c| c.as_array()) {
                for b in blocks {
                    if matches!(
                        b.get("type").and_then(|t| t.as_str()),
                        Some("image") | Some("image_url")
                    ) {
                        n += 1;
                    }
                }
            }
        }
    }
    // Gemini inline image parts (`inlineData` / `inline_data`).
    if let Some(contents) = body.get("contents").and_then(|c| c.as_array()) {
        for c in contents {
            if let Some(parts) = c.get("parts").and_then(|p| p.as_array()) {
                for p in parts {
                    if p.get("inlineData").is_some() || p.get("inline_data").is_some() {
                        n += 1;
                    }
                }
            }
        }
    }
    n
}

/// Distinctive keywords from a piece of text, for lenient (OR-based) recall.
/// Lowercased, deduplicated, stop-ish short words dropped, capped.
pub fn keywords(text: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for raw in text.split(|c: char| !c.is_alphanumeric()) {
        let w = raw.to_lowercase();
        if w.len() < 4 || w.len() > 40 {
            continue;
        }
        if seen.insert(w.clone()) {
            out.push(w);
        }
        if out.len() >= 12 {
            break;
        }
    }
    out
}

/// Text of the most recent `user` message — the natural query to recall on and
/// to summarize, unaffected by any injected `system` context.
pub fn last_user_message(body: &serde_json::Value) -> String {
    let messages = match body.get("messages").and_then(|m| m.as_array()) {
        Some(m) => m,
        None => return String::new(),
    };
    for msg in messages.iter().rev() {
        let is_user = msg
            .get("role")
            .and_then(|r| r.as_str())
            .map(|r| r == "user")
            .unwrap_or(true); // role-less messages are treated as user input
        if is_user {
            if let Some(content) = msg.get("content") {
                let mut out = String::new();
                push_content(&mut out, content);
                let out = out.trim().to_string();
                if !out.is_empty() {
                    return out;
                }
            }
        }
    }
    String::new()
}

fn push_content(out: &mut String, content: &serde_json::Value) {
    match content {
        serde_json::Value::String(s) => {
            out.push_str(s);
            out.push('\n');
        }
        serde_json::Value::Array(blocks) => {
            for block in blocks {
                if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                    out.push_str(t);
                    out.push('\n');
                } else if let Some(t) = block.as_str() {
                    out.push_str(t);
                    out.push('\n');
                }
            }
        }
        _ => {}
    }
}

/// Extract the assistant's tool calls from a non-streaming response body.
pub fn tool_calls_from_response(
    provider: &Provider,
    json: &serde_json::Value,
) -> Vec<crate::model::ToolCall> {
    use crate::model::ToolCall;
    let mut out = Vec::new();
    match provider {
        Provider::Anthropic => {
            if let Some(blocks) = json.get("content").and_then(|c| c.as_array()) {
                for b in blocks {
                    if b.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                        if let Some(name) = b.get("name").and_then(|n| n.as_str()) {
                            out.push(ToolCall {
                                name: name.to_string(),
                                input: b.get("input").cloned().unwrap_or(serde_json::Value::Null),
                            });
                        }
                    }
                }
            }
        }
        Provider::Google => {
            // Native Gemini: tool calls are candidates[].content.parts[].functionCall
            // (name + args object), not Anthropic tool_use blocks.
            if let Some(parts) = json
                .pointer("/candidates/0/content/parts")
                .and_then(|p| p.as_array())
            {
                for p in parts {
                    if let Some(fc) = p.get("functionCall") {
                        if let Some(name) = fc.get("name").and_then(|n| n.as_str()) {
                            out.push(ToolCall {
                                name: name.to_string(),
                                input: fc.get("args").cloned().unwrap_or(serde_json::Value::Null),
                            });
                        }
                    }
                }
            }
        }
        Provider::Openai => {
            if let Some(calls) = json
                .pointer("/choices/0/message/tool_calls")
                .and_then(|c| c.as_array())
            {
                for c in calls {
                    if let Some(name) = c.pointer("/function/name").and_then(|n| n.as_str()) {
                        // OpenAI arguments are a JSON string; parse for readability.
                        let input = c
                            .pointer("/function/arguments")
                            .and_then(|a| a.as_str())
                            .and_then(|s| serde_json::from_str(s).ok())
                            .unwrap_or(serde_json::Value::Null);
                        out.push(ToolCall {
                            name: name.to_string(),
                            input,
                        });
                    }
                }
            }
        }
    }
    out
}

/// Extract tool_result outputs (command returns) from the latest user message of
/// a request body, so agentic command output is visible in the logs.
pub fn tool_results_from_request(body: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    let Some(messages) = body.get("messages").and_then(|m| m.as_array()) else {
        return out;
    };
    for msg in messages.iter().rev() {
        let is_user = msg.get("role").and_then(|r| r.as_str()) == Some("user");
        let Some(blocks) = msg.get("content").and_then(|c| c.as_array()) else {
            if is_user {
                break;
            }
            continue;
        };
        let mut found = false;
        for b in blocks {
            if b.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                found = true;
                match b.get("content") {
                    Some(serde_json::Value::String(s)) => out.push(s.clone()),
                    Some(serde_json::Value::Array(inner)) => {
                        for tb in inner {
                            if let Some(t) = tb.get("text").and_then(|v| v.as_str()) {
                                out.push(t.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if found || is_user {
            break;
        }
    }
    out
}

/// Whether `text` carries the marker the rtk CLI injects when it omits part of a
/// command's output: `+<n> more <label>` (e.g. `+12 more errors`, `+7 more in
/// src/x.rs`, `+14 more files`). rtk emits this in **ASCII** — it does NOT use a
/// Unicode ellipsis. Match the stable `+<digits> more` core: distinctive enough
/// to skip diff/`+5`-style noise, and it still catches an `… +<n> more` shape as
/// a substring.
fn has_rtk_marker(text: &str) -> bool {
    text.match_indices('+').any(|(i, _)| {
        let rest = &text[i + 1..];
        let digits = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
        digits > 0 && rest[digits..].starts_with(" more")
    })
}

/// Count rtk-filtered tool_result blocks in the latest user message. Returns
/// `(rewritten, total)` where `total` is the number of tool_result blocks in
/// the current turn and `rewritten` is how many carry an rtk truncation marker
/// (see [`has_rtk_marker`]). `rewritten / total` is rtk's command-output
/// coverage for the turn. Mirrors [`tool_results_from_request`]'s traversal.
pub fn count_rtk_rewrites(body: &serde_json::Value) -> (i64, i64) {
    let Some(messages) = body.get("messages").and_then(|m| m.as_array()) else {
        return (0, 0);
    };
    let (mut rewritten, mut total) = (0i64, 0i64);
    for msg in messages.iter().rev() {
        let is_user = msg.get("role").and_then(|r| r.as_str()) == Some("user");
        let Some(blocks) = msg.get("content").and_then(|c| c.as_array()) else {
            if is_user {
                break;
            }
            continue;
        };
        let mut found = false;
        for b in blocks {
            if b.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                found = true;
                total += 1;
                let marked = match b.get("content") {
                    Some(serde_json::Value::String(s)) => has_rtk_marker(s),
                    Some(serde_json::Value::Array(inner)) => inner.iter().any(|tb| {
                        tb.get("text")
                            .and_then(|v| v.as_str())
                            .is_some_and(has_rtk_marker)
                    }),
                    _ => false,
                };
                if marked {
                    rewritten += 1;
                }
            }
        }
        if found || is_user {
            break;
        }
    }
    (rewritten, total)
}

/// Token usage extracted from a provider response, including prompt-cache
/// counters (the dominant cost lever for agentic clients like Claude Code).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: i64,
    pub output: i64,
    /// Tokens written to the prompt cache this request (Anthropic only).
    pub cache_creation: i64,
    /// Tokens served from the prompt cache (Anthropic `cache_read_input_tokens`
    /// / OpenAI `prompt_tokens_details.cached_tokens`).
    pub cache_read: i64,
}

/// Token usage extracted from a non-streaming JSON response body.
pub fn usage_from_response(provider: &Provider, body: &serde_json::Value) -> Usage {
    // Gemini carries usage under `usageMetadata`; Anthropic/OpenAI under `usage`.
    let key = if matches!(provider, Provider::Google) {
        "usageMetadata"
    } else {
        "usage"
    };
    let usage = match body.get(key) {
        Some(u) => u,
        None => return Usage::default(),
    };
    usage_from_obj(provider, usage)
}

/// Extract usage from a provider `usage` object (Gemini: `usageMetadata`).
pub fn usage_from_obj(provider: &Provider, usage: &serde_json::Value) -> Usage {
    let get = |k: &str| usage.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
    match provider {
        Provider::Anthropic => Usage {
            input: get("input_tokens"),
            output: get("output_tokens"),
            cache_creation: get("cache_creation_input_tokens"),
            cache_read: get("cache_read_input_tokens"),
        },
        Provider::Google => {
            // Gemini `usageMetadata`: promptTokenCount INCLUDES the cached prefix;
            // keep `input` fresh/uncached like the other providers.
            let cache_read = get("cachedContentTokenCount");
            Usage {
                input: (get("promptTokenCount") - cache_read).max(0),
                // thoughtsTokenCount (Gemini 2.5 thinking output) is billed at the
                // output rate and is EXCLUDED from candidatesTokenCount, so add it
                // — otherwise thinking-enabled responses undercount output/cost.
                output: get("candidatesTokenCount") + get("thoughtsTokenCount"),
                cache_creation: 0,
                cache_read,
            }
        }
        Provider::Openai => {
            let cache_read = usage
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            // OpenAI's `prompt_tokens` INCLUDES cached tokens, but the rest of the
            // codebase treats Usage.input as fresh/uncached (cost_usd_cached bills
            // input at full price + cache_read at the cache rate). Subtract the
            // cached prefix so it isn't billed twice, matching the translation
            // paths' saturating_sub.
            Usage {
                input: (get("prompt_tokens") - cache_read).max(0),
                output: get("completion_tokens"),
                cache_creation: 0,
                cache_read,
            }
        }
    }
}

/// Detect the calling client from the `user-agent` header, best-effort.
pub fn detect_client(user_agent: Option<&str>) -> Option<String> {
    let ua = user_agent?.to_lowercase();
    // Order matters: OpenCode/Cursor embed the Anthropic SDK (UA contains
    // "anthropic"), so match the specific client BEFORE the generic SDK marker.
    if ua.contains("opencode") {
        Some("opencode".to_string())
    } else if ua.contains("cursor") {
        Some("cursor".to_string())
    } else if ua.contains("claude-code") || ua.contains("claude-cli") {
        Some("claude-code".to_string())
    } else if ua.contains("codex") {
        Some("codex".to_string())
    } else if ua.contains("windsurf") {
        Some("windsurf".to_string())
    } else if ua.contains("aider") {
        Some("aider".to_string())
    } else if ua.contains("cline") {
        Some("cline".to_string())
    } else if ua.contains("anthropic") {
        Some("anthropic-sdk".to_string())
    } else if ua.is_empty() {
        None
    } else {
        Some(ua.split('/').next().unwrap_or("unknown").to_string())
    }
}

/// Hash an API key to a short hex fingerprint for per-key statistics.
/// Never stores the raw key.
pub fn hash_api_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    let digest = hasher.finalize();
    hex_short(&digest)
}

fn hex_short(bytes: &[u8]) -> String {
    bytes.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_calls_from_anthropic_response() {
        let resp = json!({
            "content": [
                { "type": "text", "text": "let me check" },
                { "type": "tool_use", "id": "t1", "name": "run_bash", "input": { "cmd": "ls" } }
            ]
        });
        let calls = tool_calls_from_response(&Provider::Anthropic, &resp);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "run_bash");
        assert_eq!(calls[0].input["cmd"], "ls");
    }

    #[test]
    fn tool_results_from_request_latest_user_turn() {
        let body = json!({
            "messages": [
                { "role": "user", "content": "do it" },
                { "role": "assistant", "content": [ { "type": "tool_use", "id": "t1", "name": "ls", "input": {} } ] },
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "t1", "content": "file-a\nfile-b" }
                ] }
            ]
        });
        let results = tool_results_from_request(&body);
        assert_eq!(results.len(), 1);
        assert!(results[0].contains("file-a"));
    }

    #[test]
    fn rtk_marker_detection() {
        // The real marker is ASCII "+<digits> more" — with or without a leading
        // ellipsis (rtk does not emit the ellipsis).
        assert!(has_rtk_marker("error: foo\n\u{2026} +12 more errors"));
        assert!(has_rtk_marker("  \u{2026} +3 more\n"));
        assert!(has_rtk_marker("a +5 more"));
        assert!(!has_rtk_marker("loading\u{2026} please wait"));
        assert!(!has_rtk_marker("\u{2026} +many more")); // no digit after '+'
        assert!(!has_rtk_marker("plain output, nothing truncated"));
    }

    #[test]
    fn count_rtk_rewrites_mixed_blocks() {
        let body = json!({
            "messages": [
                { "role": "assistant", "content": [ { "type": "tool_use", "id": "t1", "name": "ls", "input": {} } ] },
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "t1", "content": "a\nb\n\u{2026} +9 more files" },
                    { "type": "tool_result", "tool_use_id": "t2", "content": [
                        { "type": "text", "text": "warning: x" },
                        { "type": "text", "text": "\u{2026} +2 more warnings" }
                    ] },
                    { "type": "tool_result", "tool_use_id": "t3", "content": "all good" }
                ] }
            ]
        });
        assert_eq!(count_rtk_rewrites(&body), (2, 3));
    }

    #[test]
    fn rtk_marker_matches_real_ascii_output() {
        // The real rtk CLI emits ASCII "+<n> more <label>" (no Unicode ellipsis).
        assert!(has_rtk_marker("  +14 more files [see remaining: …]"));
        assert!(has_rtk_marker("src/x.rs:1:fn a\n  +7 more in src/x.rs"));
        assert!(has_rtk_marker("warning: x\n+2 more warnings"));
        // An ellipsis-prefixed shape ("… +9 more") still matches (substring).
        assert!(has_rtk_marker("a\nb\n\u{2026} +9 more files"));
        // No false positives on diffs / plain "+n" / prose.
        assert!(!has_rtk_marker("clean output"));
        assert!(!has_rtk_marker("+5 apples and +3 oranges"));
        assert!(!has_rtk_marker("@@ -1,5 +1,7 @@\n+ added a line"));
        assert!(!has_rtk_marker("cost is +12 percent more expensive")); // digits not adjacent to " more"
    }

    #[test]
    fn count_rtk_rewrites_none_and_empty() {
        let body = json!({ "messages": [ { "role": "user", "content": "hi" } ] });
        assert_eq!(count_rtk_rewrites(&body), (0, 0));
        let body = json!({
            "messages": [
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "t1", "content": "clean output" }
                ] }
            ]
        });
        assert_eq!(count_rtk_rewrites(&body), (0, 1));
    }

    #[test]
    fn model_present_and_absent() {
        assert_eq!(
            model(&json!({ "model": "claude-opus-4-8" })),
            "claude-opus-4-8"
        );
        assert_eq!(model(&json!({})), "unknown");
        assert_eq!(model(&json!({ "model": 42 })), "unknown");
    }

    #[test]
    fn stream_flag() {
        assert!(stream(&json!({ "stream": true })));
        assert!(!stream(&json!({ "stream": false })));
        assert!(!stream(&json!({})));
        assert!(!stream(&json!({ "stream": "yes" })));
    }

    #[test]
    fn image_count_across_dialects() {
        let body = json!({ "messages": [
            { "role": "user", "content": [
                { "type": "text", "text": "look" },
                { "type": "image", "source": { "type": "base64", "data": "AAAA" } }
            ] },
            { "role": "user", "content": [
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAAA" } }
            ] }
        ] });
        assert_eq!(image_count(&body), 2);
        let g = json!({ "contents": [ { "parts": [ { "inlineData": { "data": "AAAA" } } ] } ] });
        assert_eq!(image_count(&g), 1);
        assert_eq!(
            image_count(&json!({ "messages": [{ "role": "user", "content": "hi" }] })),
            0
        );
    }

    #[test]
    fn usage_anthropic_shape_with_cache() {
        let body = json!({ "usage": {
            "input_tokens": 100, "output_tokens": 50,
            "cache_creation_input_tokens": 20, "cache_read_input_tokens": 800
        } });
        let u = usage_from_response(&Provider::Anthropic, &body);
        assert_eq!((u.input, u.output), (100, 50));
        assert_eq!((u.cache_creation, u.cache_read), (20, 800));
    }

    #[test]
    fn usage_openai_shape_with_cached() {
        let body = json!({ "usage": {
            "prompt_tokens": 30, "completion_tokens": 70,
            "prompt_tokens_details": { "cached_tokens": 12 }
        } });
        let u = usage_from_response(&Provider::Openai, &body);
        // OpenAI's prompt_tokens (30) INCLUDES the 12 cached; `input` is the
        // fresh/uncached remainder (18) so cost math doesn't bill the cached
        // prefix at both the full and the cache rate.
        assert_eq!((u.input, u.output), (18, 70));
        assert_eq!((u.cache_creation, u.cache_read), (0, 12));
    }

    #[test]
    fn usage_missing_returns_zero() {
        assert_eq!(
            usage_from_response(&Provider::Anthropic, &json!({})),
            Usage::default()
        );
        assert_eq!(
            usage_from_response(&Provider::Openai, &json!({ "usage": {} })),
            Usage::default()
        );
    }

    #[test]
    fn usage_native_gemini_shape() {
        let body = json!({ "usageMetadata": {
            "promptTokenCount": 1000, "candidatesTokenCount": 200,
            "cachedContentTokenCount": 300, "thoughtsTokenCount": 50
        }});
        let u = usage_from_response(&Provider::Google, &body);
        assert_eq!((u.input, u.output), (700, 250));
        assert_eq!((u.cache_creation, u.cache_read), (0, 300));
        // Missing usageMetadata → zero, not a silent Anthropic-field miss.
        assert_eq!(
            usage_from_response(&Provider::Google, &json!({})),
            Usage::default()
        );
    }

    #[test]
    fn tool_calls_native_gemini_shape() {
        // Native Gemini: functionCall under candidates[].content.parts[].
        let body = json!({ "candidates": [ { "content": { "parts": [
            { "text": "sure" },
            { "functionCall": { "name": "get_weather", "args": { "city": "Paris" } } }
        ] } } ] });
        let calls = tool_calls_from_response(&Provider::Google, &body);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[0].input, json!({ "city": "Paris" }));
    }

    #[test]
    fn keywords_extracts_distinctive_terms() {
        let kw = keywords("the production deploy key is zebra-quartz-42!!");
        assert!(kw.contains(&"production".to_string()));
        assert!(kw.contains(&"deploy".to_string()));
        assert!(kw.contains(&"zebra".to_string()));
        // Short tokens (<4 chars) dropped.
        assert!(!kw.contains(&"the".to_string()));
        assert!(!kw.contains(&"is".to_string()));
    }

    #[test]
    fn last_user_message_picks_latest_user() {
        let body = json!({ "messages": [
            { "role": "user", "content": "first" },
            { "role": "assistant", "content": "reply" },
            { "role": "user", "content": "second" }
        ] });
        assert_eq!(last_user_message(&body), "second");
    }

    #[test]
    fn detect_client_variants() {
        assert_eq!(
            detect_client(Some("claude-code/1.0")),
            Some("claude-code".to_string())
        );
        // OpenCode embeds the Anthropic SDK (UA mentions anthropic) but must be
        // detected as opencode, not claude-code.
        assert_eq!(
            detect_client(Some("opencode/1.0 @ai-sdk/anthropic")),
            Some("opencode".to_string())
        );
        assert_eq!(
            detect_client(Some("anthropic-sdk")),
            Some("anthropic-sdk".to_string())
        );
        assert_eq!(
            detect_client(Some("OpenCode/2.1")),
            Some("opencode".to_string())
        );
        assert_eq!(
            detect_client(Some("Cursor/0.4")),
            Some("cursor".to_string())
        );
        assert_eq!(
            detect_client(Some("codex_cli_rs/0.2 (anthropic)")),
            Some("codex".to_string())
        );
        assert_eq!(
            detect_client(Some("Windsurf/1.1 anthropic-sdk")),
            Some("windsurf".to_string())
        );
        assert_eq!(
            detect_client(Some("aider/0.86 anthropic")),
            Some("aider".to_string())
        );
        assert_eq!(
            detect_client(Some("Cline/3.0 @ai-sdk/anthropic")),
            Some("cline".to_string())
        );
        assert_eq!(detect_client(Some("curl/8.0")), Some("curl".to_string()));
        assert_eq!(detect_client(Some("")), None);
        assert_eq!(detect_client(None), None);
    }

    #[test]
    fn hash_api_key_is_deterministic_and_short() {
        let a = hash_api_key("sk-secret");
        let b = hash_api_key("sk-secret");
        let c = hash_api_key("other");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 16); // 8 bytes -> 16 hex chars
        assert!(a.chars().all(|ch| ch.is_ascii_hexdigit()));
    }
}
