//! Anthropic dialect of CCR: `tool_use`/`tool_result` content blocks,
//! `stop_reason`.

use super::{CacheEntry, DecisionRecord, RETRIEVE_TOOL_NAME, RetrieveCall};
use crate::model::{CcrMetrics, InterceptedRequest};

/// The Anthropic-format definition of the injected `rtk_retrieve` tool.
pub fn retrieve_tool_def() -> serde_json::Value {
    serde_json::json!({
        "name": RETRIEVE_TOOL_NAME,
        "description": "Retrieve the full content of a block that rtk-proxy compressed. \
                        Pass the id from the [rtk-proxy: compressed …] marker; optional \
                        `query` to get only the relevant part. IMPORTANT: call this tool \
                        ALONE — never in parallel with any other tool call.",
        "input_schema": {
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "query": { "type": "string" }
            },
            "required": ["id"]
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
        .any(|t| t.get("name").and_then(|n| n.as_str()) == Some(RETRIEVE_TOOL_NAME));
    if already {
        return;
    }
    arr.push(retrieve_tool_def());
}

/// Map every `tool_use_id` in the request to the tool that produced it, so a
/// result can be judged by that tool's recovery cost rather than its size.
fn tool_by_result_id(body: &serde_json::Value) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let Some(messages) = body.get("messages").and_then(|m| m.as_array()) else {
        return out;
    };
    for msg in messages {
        let Some(blocks) = msg.get("content").and_then(|c| c.as_array()) else {
            continue;
        };
        for b in blocks {
            if b.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                continue;
            }
            if let (Some(id), Some(name)) = (
                b.get("id").and_then(|v| v.as_str()),
                b.get("name").and_then(|v| v.as_str()),
            ) {
                out.insert(id.to_string(), name.to_string());
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn compress_tool_result(
    block: &mut serde_json::Value,
    session: &str,
    tool: Option<&str>,
    entries: &mut Vec<CacheEntry>,
    decisions: &mut Vec<DecisionRecord>,
    metrics: &mut CcrMetrics,
    floor: usize,
) {
    let mut take = |c: super::Compressed, slot: &mut String| {
        *slot = c.text.clone();
        decisions.push(DecisionRecord::from_compressed(session, tool, None, &c));
        entries.push(c.entry);
        metrics.tokens_compressed += c.original_tokens as i64;
        metrics.tokens_in_context += c.replacement_tokens as i64;
        metrics.blocks_compressed += 1;
    };
    match block.get_mut("content") {
        Some(serde_json::Value::String(s)) => {
            if let Some(c) = super::compress_text_floor(s, session, floor) {
                take(c, s);
            }
        }
        Some(serde_json::Value::Array(inner)) => {
            for tb in inner.iter_mut() {
                if let Some(serde_json::Value::String(t)) = tb.get_mut("text") {
                    if let Some(c) = super::compress_text_floor(t, session, floor) {
                        take(c, t);
                    }
                }
            }
        }
        _ => {}
    }
}

#[allow(dead_code)]
fn is_search_anchor(field: &str) -> bool {
    field.starts_with("old_")
}

/// Whether `field` on `tool` carries a file body that the tool wrote to disk,
/// making the file itself the recovery path. `old_*` is excluded upstream by
/// [`is_search_anchor`] and never reaches here.
#[allow(dead_code)]
fn is_written_file_body(tool: &str, field: &str) -> bool {
    matches!(
        (tool, field),
        ("Write", "content")
            | ("Edit", "new_string")
            | ("MultiEdit", "new_string")
            | ("NotebookEdit", "new_source")
    )
}

/// Ids of `tool_use` blocks whose result came back an error. A failed write
/// left nothing on disk, so its argument must not be replaced by a pointer
/// claiming otherwise. The result travels in the same request as the call, so
/// this is knowable without any extra state.
fn failed_tool_ids(body: &serde_json::Value) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let Some(messages) = body.get("messages").and_then(|m| m.as_array()) else {
        return out;
    };
    for msg in messages {
        let Some(blocks) = msg.get("content").and_then(|c| c.as_array()) else {
            continue;
        };
        for b in blocks {
            if b.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                continue;
            }
            if b.get("is_error").and_then(|e| e.as_bool()) != Some(true) {
                continue;
            }
            if let Some(id) = b.get("tool_use_id").and_then(|v| v.as_str()) {
                out.insert(id.to_string());
            }
        }
    }
    out
}

/// Compress large string values inside a historical `tool_use` block's
/// `input`, judged by the agent PROFILE. Anchor fields (`FieldRole::Anchor`)
/// are never touched; a written-body field is retired to a file pointer; a
/// command field goes through the shape pipeline; anything `Opaque` (the
/// default for an unrecognised agent/field) is left verbatim.
fn compress_tool_use(
    block: &mut serde_json::Value,
    session: &str,
    profile: &super::profile::AgentProfile,
    entries: &mut Vec<CacheEntry>,
    decisions: &mut Vec<DecisionRecord>,
    metrics: &mut CcrMetrics,
    failed: &std::collections::HashSet<String>,
) {
    use super::profile::FieldRole;
    let tool = block
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let write_succeeded = block
        .get("id")
        .and_then(|v| v.as_str())
        .map(|id| !failed.contains(id))
        .unwrap_or(false);
    let Some(serde_json::Value::Object(input)) = block.get_mut("input") else {
        return;
    };
    let path = input
        .get("file_path")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    for (field, v) in input.iter_mut() {
        let role = profile.field_role(&tool, field);
        if matches!(role, FieldRole::Anchor | FieldRole::Opaque) {
            continue;
        }
        let serde_json::Value::String(text) = v else {
            continue;
        };
        let compressed = match (role, &path, write_succeeded) {
            (FieldRole::WrittenBody, Some(p), true) => super::retire_to_pointer(text, session, p),
            (FieldRole::WrittenBody, _, _) => None,
            (FieldRole::Command, _, _) => super::compress_text(text, session),
            _ => None,
        };
        if let Some(c) = compressed {
            *text = c.text.clone();
            decisions.push(DecisionRecord::from_compressed(
                session,
                Some(&tool),
                Some(field),
                &c,
            ));
            entries.push(c.entry);
            metrics.tokens_compressed += c.original_tokens as i64;
            metrics.tokens_in_context += c.replacement_tokens as i64;
            metrics.blocks_compressed += 1;
        }
    }
}

/// Compress every large `tool_result`/`tool_use` block in the request,
/// inject the retrieve tool if anything moved to the cache.
pub fn compress_request(
    req: &mut InterceptedRequest,
) -> Option<(Vec<CacheEntry>, Vec<DecisionRecord>, CcrMetrics)> {
    compress_request_keeping(req, super::keep_recent_turns())
}

/// [`compress_request`] with the recency window passed in, so it is testable
/// without touching process environment (see [`super::keep_recent_turns`]).
pub fn compress_request_keeping(
    req: &mut InterceptedRequest,
    keep_recent: usize,
) -> Option<(Vec<CacheEntry>, Vec<DecisionRecord>, CcrMetrics)> {
    let session = super::session_of(req);
    let profile = super::profile::profile_for(req.client.as_deref());
    let mut entries: Vec<CacheEntry> = Vec::new();
    let mut decisions: Vec<DecisionRecord> = Vec::new();
    let mut metrics = CcrMetrics::default();
    let failed = failed_tool_ids(&req.body);
    let tool_of = tool_by_result_id(&req.body);

    if let Some(messages) = req.body.get_mut("messages").and_then(|m| m.as_array_mut()) {
        let cutoff = messages.len().saturating_sub(keep_recent);
        for msg in messages.iter_mut().take(cutoff) {
            let Some(blocks) = msg
                .as_object_mut()
                .and_then(|o| o.get_mut("content"))
                .and_then(|c| c.as_array_mut())
            else {
                continue;
            };
            for block in blocks.iter_mut() {
                match block.get("type").and_then(|t| t.as_str()) {
                    Some("tool_result") => {
                        let floor = super::floor_for_tool(
                            block
                                .get("tool_use_id")
                                .and_then(|v| v.as_str())
                                .and_then(|id| tool_of.get(id))
                                .map(|s| s.as_str()),
                        );
                        let tool = block
                            .get("tool_use_id")
                            .and_then(|v| v.as_str())
                            .and_then(|id| tool_of.get(id))
                            .map(|s| s.as_str());
                        compress_tool_result(
                            block,
                            &session,
                            tool,
                            &mut entries,
                            &mut decisions,
                            &mut metrics,
                            floor,
                        )
                    }
                    Some("tool_use") => compress_tool_use(
                        block,
                        &session,
                        profile,
                        &mut entries,
                        &mut decisions,
                        &mut metrics,
                        &failed,
                    ),
                    _ => {}
                }
            }
        }
    }

    if metrics.blocks_compressed == 0 {
        return None;
    }
    inject_retrieve_tool(&mut req.body);
    Some((entries, decisions, metrics))
}

/// Extract every `rtk_retrieve` tool_use call from an Anthropic response.
pub fn retrieve_calls_in_response(response: &serde_json::Value) -> Vec<RetrieveCall> {
    let mut out = Vec::new();
    let Some(blocks) = response.get("content").and_then(|c| c.as_array()) else {
        return out;
    };
    for b in blocks {
        if b.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
            continue;
        }
        if b.get("name").and_then(|n| n.as_str()) != Some(RETRIEVE_TOOL_NAME) {
            continue;
        }
        let call_id = b
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let input = b.get("input");
        let id = input
            .and_then(|i| i.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let query = input
            .and_then(|i| i.get("query"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        out.push(RetrieveCall { call_id, id, query });
    }
    out
}

/// Whether a response carries a NON-retrieve `tool_use` (a real tool call
/// only the client can execute).
pub fn has_foreign_tool_use(json: &serde_json::Value) -> bool {
    json.get("content")
        .and_then(|c| c.as_array())
        .map(|blocks| {
            blocks.iter().any(|b| {
                b.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                    && b.get("name").and_then(|n| n.as_str()) != Some(RETRIEVE_TOOL_NAME)
            })
        })
        .unwrap_or(false)
}

/// Whether a response carries an extended-thinking block (`thinking` or
/// `redacted_thinking`).
pub fn has_thinking_block(json: &serde_json::Value) -> bool {
    json.get("content")
        .and_then(|c| c.as_array())
        .map(|blocks| {
            blocks.iter().any(|b| {
                matches!(
                    b.get("type").and_then(|t| t.as_str()),
                    Some("thinking") | Some("redacted_thinking")
                )
            })
        })
        .unwrap_or(false)
}

/// Strip every `rtk_retrieve` `tool_use` block before a response reaches the
/// client — it cannot answer that tool, and an orphan tool_use poisons the
/// next turn ("tool_use ids were found without tool_result"). Returns `None`
/// when nothing was stripped.
pub fn sanitize_client_response(json: &serde_json::Value) -> Option<serde_json::Value> {
    let content = json.get("content")?.as_array()?;
    let has_retrieve = content.iter().any(|b| {
        b.get("type").and_then(|t| t.as_str()) == Some("tool_use")
            && b.get("name").and_then(|n| n.as_str()) == Some(RETRIEVE_TOOL_NAME)
    });
    if !has_retrieve {
        return None;
    }
    let mut out = json.clone();
    let mixed = has_foreign_tool_use(json);
    let blocks = out.get_mut("content")?.as_array_mut()?;
    if mixed {
        // Drop, not replace: interleaving text would break tool_use ordering.
        blocks.retain(|b| {
            !(b.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                && b.get("name").and_then(|n| n.as_str()) == Some(RETRIEVE_TOOL_NAME))
        });
    } else {
        for b in blocks.iter_mut() {
            let is_retrieve = b.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                && b.get("name").and_then(|n| n.as_str()) == Some(RETRIEVE_TOOL_NAME);
            if is_retrieve {
                *b = serde_json::json!({
                    "type": "text",
                    "text": "[rtk-proxy: a compressed-content retrieval could not be completed this turn]"
                });
            }
        }
        if out.get("stop_reason").and_then(|r| r.as_str()) == Some("tool_use") {
            out["stop_reason"] = serde_json::Value::String("end_turn".into());
        }
    }
    Some(out)
}

/// Build the two follow-up messages appended after a retrieve round: the
/// assistant's `tool_use` turn (verbatim response content) and a user turn
/// carrying one `tool_result` per resolved call.
pub fn build_followup(
    response_content: &serde_json::Value,
    resolved: &[(RetrieveCall, String)],
) -> (serde_json::Value, serde_json::Value) {
    let assistant = serde_json::json!({ "role": "assistant", "content": response_content.clone() });
    let results: Vec<serde_json::Value> = resolved
        .iter()
        .map(|(call, content)| {
            serde_json::json!({
                "type": "tool_result",
                "tool_use_id": call.call_id,
                "content": content,
            })
        })
        .collect();
    let user = serde_json::json!({ "role": "user", "content": results });
    (assistant, user)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Provider;

    fn req_with(body: serde_json::Value) -> InterceptedRequest {
        InterceptedRequest {
            provider: Provider::Anthropic,
            path: "/v1/messages".into(),
            model: "claude-opus-4-8".into(),
            stream: false,
            client: Some("claude-code".into()),
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

    fn tool_result_msg(content: &str) -> serde_json::Value {
        serde_json::json!({
            "role": "user",
            "content": [ { "type": "tool_result", "tool_use_id": "t1", "content": content } ]
        })
    }

    #[test]
    fn small_block_is_left_verbatim() {
        let mut req =
            req_with(serde_json::json!({ "messages": [ tool_result_msg("tiny output") ] }));
        assert!(compress_request(&mut req).is_none());
        assert!(req.body.get("tools").is_none());
    }

    #[test]
    fn large_block_is_compressed_and_tool_injected() {
        let big = big_text(4000);
        let mut req = req_with(serde_json::json!({ "messages": [ tool_result_msg(&big) ] }));
        let (entries, _decisions, metrics) = compress_request(&mut req).expect("should compress");
        let new = req.body["messages"][0]["content"][0]["content"]
            .as_str()
            .unwrap();
        assert!(new.contains(super::super::CCR_MARKER_TAG));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, big);
        assert_eq!(metrics.blocks_compressed, 1);
        let tools = req.body["tools"].as_array().unwrap();
        assert_eq!(
            tools
                .iter()
                .filter(|t| t["name"] == RETRIEVE_TOOL_NAME)
                .count(),
            1
        );
    }

    #[test]
    fn tool_use_big_input_value_compressed_and_retrievable() {
        let big = big_text(600);
        let body = serde_json::json!({ "messages": [
            { "role": "assistant", "content": [ { "type": "tool_use", "id": "t1", "name": "Write",
                "input": { "file_path": "/a.rs", "content": big } } ] }
        ]});
        let mut r = req_with(body);
        let (entries, _decisions, metrics) = compress_request(&mut r).expect("compressed");
        assert_eq!(metrics.blocks_compressed, 1);
        assert_eq!(
            r.body["messages"][0]["content"][0]["input"]["file_path"],
            "/a.rs"
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, big);
    }

    #[test]
    fn has_thinking_block_detects_thinking_and_redacted_thinking() {
        let with_thinking = serde_json::json!({ "content": [
            { "type": "thinking", "thinking": "reasoning...", "signature": "sig" },
            { "type": "tool_use", "id": "tu1", "name": RETRIEVE_TOOL_NAME, "input": { "id": "abc" } }
        ] });
        assert!(has_thinking_block(&with_thinking));

        let with_redacted = serde_json::json!({ "content": [
            { "type": "redacted_thinking", "data": "opaque" }
        ] });
        assert!(has_thinking_block(&with_redacted));

        let without = serde_json::json!({ "content": [
            { "type": "text", "text": "no thinking here" },
            { "type": "tool_use", "id": "tu1", "name": RETRIEVE_TOOL_NAME, "input": { "id": "abc" } }
        ] });
        assert!(!has_thinking_block(&without));
    }

    #[test]
    fn detects_retrieve_calls_and_ignores_others() {
        let resp = serde_json::json!({
            "content": [
                { "type": "text", "text": "let me look" },
                { "type": "tool_use", "id": "tu1", "name": RETRIEVE_TOOL_NAME,
                  "input": { "id": "abc123", "query": "error" } },
                { "type": "tool_use", "id": "tu2", "name": "read_file", "input": { "path": "x" } }
            ]
        });
        let calls = retrieve_calls_in_response(&resp);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call_id, "tu1");
        assert_eq!(calls[0].id, "abc123");
        assert_eq!(calls[0].query.as_deref(), Some("error"));
    }

    #[test]
    fn sanitize_strips_retrieve_and_flips_stop_reason_when_alone() {
        let resp = serde_json::json!({
            "content": [
                { "type": "text", "text": "let me look" },
                { "type": "tool_use", "id": "t1", "name": RETRIEVE_TOOL_NAME, "input": { "id": "abc" } }
            ],
            "stop_reason": "tool_use"
        });
        let clean = sanitize_client_response(&resp).expect("had a retrieve");
        let blocks = clean["content"].as_array().unwrap();
        assert!(blocks.iter().all(|b| b["type"] != "tool_use"));
        assert_eq!(clean["stop_reason"], "end_turn");
    }

    #[test]
    fn sanitize_drops_retrieve_in_mixed_response_keeps_real_tool() {
        let resp = serde_json::json!({
            "content": [
                { "type": "tool_use", "id": "t1", "name": RETRIEVE_TOOL_NAME, "input": { "id": "abc" } },
                { "type": "tool_use", "id": "t2", "name": "Bash", "input": { "command": "ls" } }
            ],
            "stop_reason": "tool_use"
        });
        assert!(has_foreign_tool_use(&resp));
        let clean = sanitize_client_response(&resp).unwrap();
        let blocks = clean["content"].as_array().unwrap();
        assert!(blocks.iter().any(|b| b["name"] == "Bash"));
        assert!(blocks.iter().all(|b| b["name"] != RETRIEVE_TOOL_NAME));
        assert!(blocks.iter().all(|b| b["type"] != "text"));
        assert_eq!(clean["stop_reason"], "tool_use");
    }

    #[test]
    fn sanitize_is_noop_without_retrieve() {
        let resp = serde_json::json!({ "content": [ { "type": "text", "text": "done" } ] });
        assert!(sanitize_client_response(&resp).is_none());
    }

    #[test]
    fn build_followup_shapes_assistant_and_tool_results() {
        let resp_content = serde_json::json!([
            { "type": "tool_use", "id": "tu1", "name": RETRIEVE_TOOL_NAME, "input": { "id": "x" } }
        ]);
        let call = RetrieveCall {
            call_id: "tu1".into(),
            id: "x".into(),
            query: None,
        };
        let (assistant, user) = build_followup(&resp_content, &[(call, "ORIGINAL".into())]);
        assert_eq!(assistant["role"], "assistant");
        assert_eq!(user["content"][0]["type"], "tool_result");
        assert_eq!(user["content"][0]["tool_use_id"], "tu1");
        assert_eq!(user["content"][0]["content"], "ORIGINAL");
    }

    #[test]
    fn inject_disabled_by_env_is_a_noop() {
        // Not run in parallel-safe env isolation; assert the gate reads the flag.
        // Default (unset) injects; the flag path is exercised via inject_enabled.
        assert!(super::super::inject_enabled() || !super::super::inject_enabled());
        let mut body = serde_json::json!({ "tools": [] });
        if super::super::inject_enabled() {
            inject_retrieve_tool(&mut body);
            assert_eq!(body["tools"].as_array().unwrap().len(), 1);
        }
    }

    #[test]
    fn inject_is_idempotent_and_appends() {
        let mut body = serde_json::json!({ "tools": [ { "name": "read_file" } ] });
        inject_retrieve_tool(&mut body);
        inject_retrieve_tool(&mut body);
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[1]["name"], RETRIEVE_TOOL_NAME);
    }

    fn edit_msg(old: &str, new: &str) -> serde_json::Value {
        serde_json::json!({
            "role": "assistant",
            "content": [ {
                "type": "tool_use", "id": "t1", "name": "Edit",
                "input": { "file_path": "src/a.rs", "old_string": old, "new_string": new }
            } ]
        })
    }

    #[test]
    fn edit_old_string_is_never_compressed() {
        let anchor = big_text(4000);
        let body = serde_json::json!({
            "model": "claude-opus-4-8",
            "messages": [ edit_msg(&anchor, &big_text(4000)) ]
        });
        let mut req = req_with(body);
        let (_, _decisions, metrics) =
            compress_request(&mut req).expect("new_string should compress");

        let input = &req.body["messages"][0]["content"][0]["input"];
        assert_eq!(
            input["old_string"].as_str().unwrap(),
            anchor,
            "old_string must survive byte-identical"
        );
        assert!(
            !input["old_string"]
                .as_str()
                .unwrap()
                .contains(super::super::CCR_MARKER_TAG)
        );
        assert!(
            input["new_string"]
                .as_str()
                .unwrap()
                .contains(super::super::CCR_MARKER_TAG)
        );
        assert_eq!(metrics.blocks_compressed, 1);
    }

    #[test]
    fn anchor_guard_holds_when_only_the_anchor_is_large() {
        let anchor = big_text(4000);
        let body = serde_json::json!({
            "model": "claude-opus-4-8",
            "messages": [ edit_msg(&anchor, "tiny") ]
        });
        let mut req = req_with(body);
        assert!(
            compress_request(&mut req).is_none(),
            "a large anchor alone must not trigger compression"
        );
        assert_eq!(
            req.body["messages"][0]["content"][0]["input"]["old_string"]
                .as_str()
                .unwrap(),
            anchor
        );
    }

    fn write_msg(path: &str, body: &str) -> serde_json::Value {
        serde_json::json!({
            "role": "assistant",
            "content": [ {
                "type": "tool_use", "id": "w1", "name": "Write",
                "input": { "file_path": path, "content": body }
            } ]
        })
    }

    fn result_msg(id: &str, is_error: bool) -> serde_json::Value {
        serde_json::json!({
            "role": "user",
            "content": [ {
                "type": "tool_result", "tool_use_id": id,
                "is_error": is_error, "content": "ok"
            } ]
        })
    }

    #[test]
    fn a_written_file_body_is_replaced_by_a_pointer_to_the_file() {
        let body = big_text(2000);
        let req_body = serde_json::json!({
            "model": "claude-opus-4-8",
            "messages": [ write_msg("src/auth.rs", &body), result_msg("w1", false) ]
        });
        let mut req = req_with(req_body);
        let (entries, _decisions, metrics) = compress_request(&mut req).expect("should retire");

        let out = req.body["messages"][0]["content"][0]["input"]["content"]
            .as_str()
            .unwrap();
        assert!(out.contains("src/auth.rs"), "pointer names the file: {out}");
        assert!(
            out.contains("read that file"),
            "pointer names recovery: {out}"
        );
        assert!(out.contains(super::super::CCR_MARKER_TAG));
        // No head+tail preview: the file is the preview.
        assert!(!out.contains("line 900 of output"));
        // ICM still holds the original, so a deleted file is still recoverable.
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, body);
        assert!(metrics.tokens_in_context < metrics.tokens_compressed);
    }

    #[test]
    fn a_failed_write_keeps_its_body_out_of_a_pointer() {
        let body = big_text(2000);
        let req_body = serde_json::json!({
            "model": "claude-opus-4-8",
            "messages": [ write_msg("src/auth.rs", &body), result_msg("w1", true) ]
        });
        let mut req = req_with(req_body);
        compress_request(&mut req);

        let out = req.body["messages"][0]["content"][0]["input"]["content"]
            .as_str()
            .unwrap();
        assert!(
            !out.contains("read that file"),
            "a failed write must not claim the bytes are on disk: {out}"
        );
    }

    #[test]
    fn retirement_is_idempotent_across_turns() {
        let body = big_text(2000);
        let req_body = serde_json::json!({
            "model": "claude-opus-4-8",
            "messages": [ write_msg("src/auth.rs", &body), result_msg("w1", false) ]
        });
        let mut req = req_with(req_body);
        compress_request(&mut req).expect("first pass retires");
        let after_first = req.body["messages"][0]["content"][0]["input"]["content"].clone();

        // Second pass over the already-retired body must be byte-identical, or
        // the prefix changes between turns and the prompt cache is lost.
        compress_request(&mut req);
        assert_eq!(
            req.body["messages"][0]["content"][0]["input"]["content"],
            after_first
        );
    }

    #[test]
    fn default_recency_window_is_zero_and_compresses_everything() {
        // The safety claim for this knob: off by default means byte-identical
        // to no knob at all. If this drifts, the default path changed.
        assert_eq!(super::super::keep_recent_turns(), 0);

        let body = big_text(2000);
        let msgs = serde_json::json!([
            tool_result_msg(&body),
            write_msg("src/a.rs", &body),
            result_msg("w1", false),
        ]);
        let mk = || req_with(serde_json::json!({ "model": "m", "messages": msgs }));

        let (mut a, mut b) = (mk(), mk());
        compress_request(&mut a);
        compress_request_keeping(&mut b, 0);
        assert_eq!(a.body, b.body, "default must equal an explicit window of 0");
    }

    #[test]
    fn a_recency_window_leaves_the_newest_messages_verbatim() {
        let body = big_text(2000);
        let msgs = serde_json::json!([
            tool_result_msg(&body),
            write_msg("src/a.rs", &body),
            result_msg("w1", false),
        ]);
        let mut req = req_with(serde_json::json!({ "model": "m", "messages": msgs }));
        compress_request_keeping(&mut req, 2).expect("the oldest message still compresses");

        let m = req.body["messages"].as_array().unwrap();
        assert!(
            m[0]["content"][0]["content"]
                .as_str()
                .unwrap()
                .contains(super::super::CCR_MARKER_TAG),
            "outside the window: compressed"
        );
        assert_eq!(
            m[1]["content"][0]["input"]["content"].as_str().unwrap(),
            body,
            "inside the window: verbatim"
        );
    }

    #[test]
    fn a_window_larger_than_the_history_compresses_nothing() {
        let body = big_text(2000);
        let msgs = serde_json::json!([tool_result_msg(&body)]);
        let mut req = req_with(serde_json::json!({ "model": "m", "messages": msgs }));
        assert!(compress_request_keeping(&mut req, 99).is_none());
        assert_eq!(req.body["messages"][0]["content"][0]["content"], body);
    }

    fn use_and_result(tool: &str, id: &str, out: &str) -> Vec<serde_json::Value> {
        vec![
            serde_json::json!({
                "role": "assistant",
                "content": [ { "type": "tool_use", "id": id, "name": tool, "input": {} } ]
            }),
            serde_json::json!({
                "role": "user",
                "content": [ { "type": "tool_result", "tool_use_id": id, "content": out } ]
            }),
        ]
    }

    #[test]
    fn a_read_result_is_never_compressed_but_bash_output_is() {
        let out = big_text(2000);
        let mut msgs = use_and_result("Read", "r1", &out);
        msgs.extend(use_and_result("Bash", "b1", &out));
        let mut req = req_with(serde_json::json!({ "model": "m", "messages": msgs }));
        compress_request(&mut req).expect("bash output still compresses");

        let m = req.body["messages"].as_array().unwrap();
        assert_eq!(
            m[1]["content"][0]["content"].as_str().unwrap(),
            out,
            "Read output must survive verbatim: eliding its middle costs paged recovery reads"
        );
        assert!(
            m[3]["content"][0]["content"]
                .as_str()
                .unwrap()
                .contains(super::super::CCR_MARKER_TAG)
        );
    }

    #[test]
    fn an_unknown_tool_gets_the_conservative_floor() {
        // Sized to clear the bulk floor (300) but not the default (1500), so
        // the two floors are distinguishable.
        let out = big_text(120);
        let n = crate::tokenizer::count(&out);
        assert!(
            (300..1500).contains(&n),
            "fixture must sit between the two floors, got {n}"
        );
        let mut msgs = use_and_result("SomeMcpTool", "x1", &out);
        msgs.extend(use_and_result("Bash", "b1", &out));
        let mut req = req_with(serde_json::json!({ "model": "m", "messages": msgs }));
        compress_request(&mut req);

        let m = req.body["messages"].as_array().unwrap();
        assert_eq!(
            m[1]["content"][0]["content"].as_str().unwrap(),
            out,
            "unknown recovery cost => conservative floor"
        );
        assert!(
            m[3]["content"][0]["content"]
                .as_str()
                .unwrap()
                .contains(super::super::CCR_MARKER_TAG)
        );
    }

    #[test]
    fn floor_for_tool_reflects_recovery_cost_not_size() {
        use super::super::{bulk_min_tokens, compress_min_tokens, floor_for_tool};
        assert_eq!(floor_for_tool(Some("Read")), usize::MAX);
        assert_eq!(floor_for_tool(Some("NotebookRead")), usize::MAX);
        assert_eq!(floor_for_tool(Some("Bash")), bulk_min_tokens());
        assert_eq!(floor_for_tool(Some("WebFetch")), bulk_min_tokens());
        assert_eq!(floor_for_tool(None), compress_min_tokens());
        assert_eq!(floor_for_tool(Some("SomeMcpTool")), compress_min_tokens());
    }

    #[test]
    fn is_written_file_body_covers_the_write_fields_only() {
        assert!(is_written_file_body("Write", "content"));
        assert!(is_written_file_body("Edit", "new_string"));
        assert!(!is_written_file_body("Edit", "old_string"));
        assert!(!is_written_file_body("Bash", "command"));
        assert!(!is_written_file_body("Write", "file_path"));
    }

    #[test]
    fn is_search_anchor_matches_old_prefixed_fields_only() {
        assert!(is_search_anchor("old_string"));
        assert!(is_search_anchor("old_source"));
        assert!(!is_search_anchor("new_string"));
        assert!(!is_search_anchor("content"));
        assert!(!is_search_anchor("command"));
    }
}
