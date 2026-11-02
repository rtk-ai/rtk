//! OpenAI dialect of CCR: `role: "tool"` messages (the `tool_result`
//! equivalent) and `tool_calls[]` on an assistant message (the `tool_use`
//! equivalent), `finish_reason`.
//!
//! Scope note: only `role: "tool"` message bodies are compressed here —
//! that's where the large agentic tool output actually lives (command
//! output, file reads), matching the primary Anthropic mechanism. Anthropic
//! ALSO compresses big string values inside a historical `tool_use.input`
//! object (Write/Edit file bodies); the OpenAI equivalent would require
//! parsing each `tool_calls[].function.arguments` JSON-string, walking its
//! fields, and re-serializing, which this dialect does not do. A
//! `tool_calls` value is never touched (idempotency).

use super::{CacheEntry, DecisionRecord, RETRIEVE_TOOL_NAME, RetrieveCall};
use crate::model::{CcrMetrics, InterceptedRequest};

/// The OpenAI function-calling definition of the injected `rtk_retrieve` tool.
pub fn retrieve_tool_def() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": RETRIEVE_TOOL_NAME,
            "description": "Retrieve the full content of a block that rtk-proxy compressed. \
                            Pass the id from the [rtk-proxy: compressed …] marker; optional \
                            `query` to get only the relevant part. IMPORTANT: call this tool \
                            ALONE — never in parallel with any other tool call.",
            "parameters": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "query": { "type": "string" }
                },
                "required": ["id"]
            }
        }
    })
}

/// Inject the `rtk_retrieve` tool into `body`'s `tools[]`, idempotently.
pub fn inject_retrieve_tool(body: &mut serde_json::Value) {
    if !super::inject_enabled() {
        return;
    }
    let Some(obj) = body.as_object_mut() else {
        return;
    };
    let tools = obj
        .entry("tools")
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    let serde_json::Value::Array(arr) = tools else {
        return;
    };
    let already = arr
        .iter()
        .any(|t| t.pointer("/function/name").and_then(|n| n.as_str()) == Some(RETRIEVE_TOOL_NAME));
    if already {
        return;
    }
    arr.push(retrieve_tool_def());
}

/// Compress every large `role: "tool"` message body, inject the retrieve
/// tool if anything moved to the cache.
pub fn compress_request(
    req: &mut InterceptedRequest,
) -> Option<(Vec<CacheEntry>, Vec<DecisionRecord>, CcrMetrics)> {
    let session = super::session_of(req);
    let mut entries: Vec<CacheEntry> = Vec::new();
    let mut decisions: Vec<DecisionRecord> = Vec::new();
    let mut metrics = CcrMetrics::default();

    if let Some(messages) = req.body.get_mut("messages").and_then(|m| m.as_array_mut()) {
        for msg in messages.iter_mut() {
            let Some(obj) = msg.as_object_mut() else {
                continue;
            };
            if obj.get("role").and_then(|r| r.as_str()) != Some("tool") {
                continue;
            }
            let Some(serde_json::Value::String(text)) = obj.get_mut("content") else {
                continue;
            };
            if let Some(c) = super::compress_text(text, &session) {
                *text = c.text.clone();
                decisions.push(DecisionRecord::from_compressed(
                    &session,
                    Some("tool"),
                    None,
                    &c,
                ));
                entries.push(c.entry);
                metrics.tokens_compressed += c.original_tokens as i64;
                metrics.tokens_in_context += c.replacement_tokens as i64;
                metrics.blocks_compressed += 1;
            }
        }
    }

    if metrics.blocks_compressed == 0 {
        return None;
    }
    inject_retrieve_tool(&mut req.body);
    Some((entries, decisions, metrics))
}

/// Extract every `rtk_retrieve` call from an OpenAI response's first choice.
pub fn retrieve_calls_in_response(response: &serde_json::Value) -> Vec<RetrieveCall> {
    let mut out = Vec::new();
    let Some(calls) = response
        .pointer("/choices/0/message/tool_calls")
        .and_then(|c| c.as_array())
    else {
        return out;
    };
    for c in calls {
        if c.pointer("/function/name").and_then(|n| n.as_str()) != Some(RETRIEVE_TOOL_NAME) {
            continue;
        }
        let call_id = c
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let args: serde_json::Value = c
            .pointer("/function/arguments")
            .and_then(|a| a.as_str())
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(serde_json::Value::Null);
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        out.push(RetrieveCall { call_id, id, query });
    }
    out
}

/// Whether the response's first choice carries a NON-retrieve tool call.
pub fn has_foreign_tool_use(json: &serde_json::Value) -> bool {
    json.pointer("/choices/0/message/tool_calls")
        .and_then(|c| c.as_array())
        .map(|calls| {
            calls.iter().any(|c| {
                c.pointer("/function/name").and_then(|n| n.as_str()) != Some(RETRIEVE_TOOL_NAME)
            })
        })
        .unwrap_or(false)
}

/// Strip every `rtk_retrieve` tool call from the response's first choice
/// before it reaches the client. Returns `None` when nothing was stripped.
pub fn sanitize_client_response(json: &serde_json::Value) -> Option<serde_json::Value> {
    let calls = json.pointer("/choices/0/message/tool_calls")?.as_array()?;
    let has_retrieve = calls
        .iter()
        .any(|c| c.pointer("/function/name").and_then(|n| n.as_str()) == Some(RETRIEVE_TOOL_NAME));
    if !has_retrieve {
        return None;
    }
    let mut out = json.clone();
    let mixed = has_foreign_tool_use(json);
    let message = out.pointer_mut("/choices/0/message")?.as_object_mut()?;
    if mixed {
        // Flat array, no interleaving-order concern: just drop the retrieve
        // entries and keep the real ones.
        if let Some(serde_json::Value::Array(arr)) = message.get_mut("tool_calls") {
            arr.retain(|c| {
                c.pointer("/function/name").and_then(|n| n.as_str()) != Some(RETRIEVE_TOOL_NAME)
            });
        }
    } else {
        message.remove("tool_calls");
        message.insert(
            "content".to_string(),
            serde_json::Value::String(
                "[rtk-proxy: a compressed-content retrieval could not be completed this turn]"
                    .to_string(),
            ),
        );
        if out
            .pointer("/choices/0/finish_reason")
            .and_then(|r| r.as_str())
            == Some("tool_calls")
        {
            if let Some(choice) = out
                .pointer_mut("/choices/0")
                .and_then(|c| c.as_object_mut())
            {
                choice.insert(
                    "finish_reason".to_string(),
                    serde_json::Value::String("stop".to_string()),
                );
            }
        }
    }
    Some(out)
}

/// Build the follow-up messages appended after a retrieve round: the
/// assistant message exactly as returned (its `tool_calls` acknowledged),
/// then one `role: "tool"` message per resolved call.
pub fn build_followup(
    response: &serde_json::Value,
    resolved: &[(RetrieveCall, String)],
) -> Vec<serde_json::Value> {
    let mut out = Vec::with_capacity(1 + resolved.len());
    let assistant = response
        .pointer("/choices/0/message")
        .cloned()
        .unwrap_or(serde_json::json!({ "role": "assistant", "content": null }));
    out.push(assistant);
    for (call, content) in resolved {
        out.push(serde_json::json!({
            "role": "tool",
            "tool_call_id": call.call_id,
            "content": content,
        }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Provider;

    fn req_with(body: serde_json::Value) -> InterceptedRequest {
        InterceptedRequest {
            provider: Provider::Openai,
            path: "/v1/chat/completions".into(),
            model: "gpt-5".into(),
            stream: false,
            client: None,
            project: None,
            session: Some("sess1".into()),
            api_key_hash: Some("tenantA".into()),
            tokens_saved: 0,
            saved_by_stage: Vec::new(),
            routed_from: None,
            request_class: None,
            user_id: None,
            ccr: Default::default(),
            ccr_retrieve_calls: 0,
            pipeline_ms: 0,
            body,
            budget_reserved: 0.0,
            team_budget_reserved: 0.0,
            team_budget_reserved_at: None,
        }
    }

    fn big_text(n: usize) -> String {
        (0..n)
            .map(|i| format!("line {i} of output"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn tool_msg(content: &str) -> serde_json::Value {
        serde_json::json!({ "role": "tool", "tool_call_id": "call_1", "content": content })
    }

    #[test]
    fn small_block_is_left_verbatim() {
        let mut req = req_with(serde_json::json!({ "messages": [ tool_msg("tiny output") ] }));
        assert!(compress_request(&mut req).is_none());
        assert!(req.body.get("tools").is_none());
    }

    #[test]
    fn large_block_is_compressed_and_tool_injected() {
        let big = big_text(4000);
        let mut req = req_with(serde_json::json!({ "messages": [ tool_msg(&big) ] }));
        let (entries, _decisions, metrics) = compress_request(&mut req).expect("should compress");
        let new = req.body["messages"][0]["content"].as_str().unwrap();
        assert!(new.contains(super::super::CCR_MARKER_TAG));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, big);
        assert_eq!(metrics.blocks_compressed, 1);
        let tools = req.body["tools"].as_array().unwrap();
        assert_eq!(
            tools
                .iter()
                .filter(|t| t.pointer("/function/name").and_then(|n| n.as_str())
                    == Some(RETRIEVE_TOOL_NAME))
                .count(),
            1
        );
    }

    #[test]
    fn non_tool_role_messages_are_left_alone() {
        let big = big_text(4000);
        let mut req = req_with(serde_json::json!({ "messages": [
            { "role": "user", "content": big.clone() }
        ]}));
        assert!(
            compress_request(&mut req).is_none(),
            "only role:tool is compressed"
        );
        assert_eq!(req.body["messages"][0]["content"].as_str().unwrap(), big);
    }

    #[test]
    fn detects_retrieve_calls_and_ignores_others() {
        let resp = serde_json::json!({ "choices": [ { "message": { "role": "assistant", "tool_calls": [
            { "id": "call_1", "type": "function", "function": { "name": RETRIEVE_TOOL_NAME,
                "arguments": "{\"id\":\"abc123\",\"query\":\"error\"}" } },
            { "id": "call_2", "type": "function", "function": { "name": "read_file",
                "arguments": "{\"path\":\"x\"}" } }
        ] } } ] });
        let calls = retrieve_calls_in_response(&resp);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call_id, "call_1");
        assert_eq!(calls[0].id, "abc123");
        assert_eq!(calls[0].query.as_deref(), Some("error"));
    }

    #[test]
    fn sanitize_strips_retrieve_and_flips_finish_reason_when_alone() {
        let resp = serde_json::json!({ "choices": [ { "message": { "role": "assistant", "tool_calls": [
            { "id": "call_1", "type": "function", "function": { "name": RETRIEVE_TOOL_NAME,
                "arguments": "{\"id\":\"abc\"}" } }
        ] }, "finish_reason": "tool_calls" } ] });
        let clean = sanitize_client_response(&resp).expect("had a retrieve");
        assert!(clean.pointer("/choices/0/message/tool_calls").is_none());
        assert_eq!(clean["choices"][0]["finish_reason"], "stop");
        assert!(
            clean["choices"][0]["message"]["content"]
                .as_str()
                .unwrap()
                .contains("rtk-proxy")
        );
    }

    #[test]
    fn sanitize_drops_retrieve_in_mixed_response_keeps_real_tool() {
        let resp = serde_json::json!({ "choices": [ { "message": { "role": "assistant", "tool_calls": [
            { "id": "call_1", "type": "function", "function": { "name": RETRIEVE_TOOL_NAME,
                "arguments": "{\"id\":\"abc\"}" } },
            { "id": "call_2", "type": "function", "function": { "name": "Bash",
                "arguments": "{\"command\":\"ls\"}" } }
        ] }, "finish_reason": "tool_calls" } ] });
        assert!(has_foreign_tool_use(&resp));
        let clean = sanitize_client_response(&resp).unwrap();
        let calls = clean["choices"][0]["message"]["tool_calls"]
            .as_array()
            .unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["function"]["name"], "Bash");
        assert_eq!(clean["choices"][0]["finish_reason"], "tool_calls");
    }

    #[test]
    fn sanitize_is_noop_without_retrieve() {
        let resp = serde_json::json!({ "choices": [ { "message": { "role": "assistant", "content": "done" } } ] });
        assert!(sanitize_client_response(&resp).is_none());
    }

    #[test]
    fn build_followup_shapes_assistant_and_tool_messages() {
        let resp = serde_json::json!({ "choices": [ { "message": { "role": "assistant", "tool_calls": [
            { "id": "call_1", "type": "function", "function": { "name": RETRIEVE_TOOL_NAME,
                "arguments": "{\"id\":\"x\"}" } }
        ] } } ] });
        let call = RetrieveCall {
            call_id: "call_1".into(),
            id: "x".into(),
            query: None,
        };
        let msgs = build_followup(&resp, &[(call, "ORIGINAL".into())]);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "assistant");
        assert_eq!(msgs[1]["role"], "tool");
        assert_eq!(msgs[1]["tool_call_id"], "call_1");
        assert_eq!(msgs[1]["content"], "ORIGINAL");
    }

    #[test]
    fn inject_is_idempotent_and_appends() {
        let mut body = serde_json::json!({ "tools": [
            { "type": "function", "function": { "name": "read_file" } }
        ] });
        inject_retrieve_tool(&mut body);
        inject_retrieve_tool(&mut body);
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[1]["function"]["name"], RETRIEVE_TOOL_NAME);
    }
}
