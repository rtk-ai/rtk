//! OpenAI-dialect navigation for [`super::ingest_filter`].
//!
//! Wire-shape mapping (same mapping the reversible CCR compressor uses in
//! `ccr/openai.rs`):
//!
//! - tool output lives in `role: "tool"` messages carrying `tool_call_id`,
//!   with `content` as a plain string or an array of `{ type: "text", text }`
//!   parts. Both are exactly the shapes `filter_tool_result_block` already
//!   handles, so the message object is filtered in place;
//! - the originating call lives on the preceding assistant message as
//!   `tool_calls[]` entries (`id` + `function.name`); `function.arguments` is
//!   a JSON-encoded string, parsed best-effort only to label the dedup marker.
//!
//! The [`Filter`] machinery is shared verbatim with the Anthropic path: every
//! rewrite stays a pure function of the block's own text (plus earlier blocks
//! for backward-only dedup), so the provider-visible prefix stays
//! byte-identical across turns and the implicit prompt cache keeps hitting.
//! `tool_calls[].function.arguments` values are never touched — the same
//! idempotency rule the CCR OpenAI dialect applies.
//!
//! Scope parity: only `role: "tool"` bodies are walked here. DedupOnly's
//! plain-text-block pass and the `tool_use` input-value filter remain
//! Anthropic-only, matching the scope note in `ccr/openai.rs`.

use super::ingest_filter::{CallMap, Filter, filter_tool_result_block, origin_label, salient_arg};

/// Map `tool_call_id` → (function name, salient argument) from assistant
/// `tool_calls[]` entries, so a later `role: "tool"` message can name its
/// originating call in the dedup marker. Mirrors the Anthropic `collect_calls`.
/// `want_labels` is false in modes that never print the label, skipping the
/// `function.arguments` JSON parse entirely (O(body) on huge Write/Edit
/// arguments — the same cost note `normalize_openai` carries).
pub(crate) fn collect_calls(
    messages: &[serde_json::Value],
    calls: &mut CallMap,
    want_labels: bool,
) {
    for msg in messages {
        if msg.get("role").and_then(|r| r.as_str()) != Some("assistant") {
            continue;
        }
        let Some(tool_calls) = msg.get("tool_calls").and_then(|t| t.as_array()) else {
            continue;
        };
        for tc in tool_calls {
            let (Some(id), Some(name)) = (
                tc.get("id").and_then(|v| v.as_str()),
                tc.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            // `function.arguments` is a JSON-encoded string. Parsing is
            // best-effort and cosmetic (marker label only): a non-JSON body
            // leaves the marker name-only, exactly like an Anthropic call
            // without a salient input key.
            let arg = if want_labels {
                tc.get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                    .and_then(|input| salient_arg(&input))
            } else {
                None
            };
            calls.insert(id.to_string(), (name.to_string(), arg));
        }
    }
}

/// Walk one message, filtering it when it is an OpenAI `role: "tool"` tool
/// result. `tool_calls[].function.arguments` values are never touched, and
/// non-tool messages are left alone.
pub(crate) fn filter_message(message: &mut serde_json::Value, f: &mut Filter, calls: &CallMap) {
    if message.get("role").and_then(|r| r.as_str()) != Some("tool") {
        return;
    }
    let origin = message
        .get("tool_call_id")
        .and_then(|v| v.as_str())
        .and_then(|id| calls.get(id))
        .and_then(|c| origin_label(Some(c)));
    // `role: "tool"` content is string-or-text-parts, the same shapes the
    // shared filter already handles for Anthropic `tool_result` blocks.
    filter_tool_result_block(message, f, origin.as_deref());
}

#[cfg(test)]
mod tests {
    use crate::model::{InterceptedRequest, Provider};
    use crate::pipeline::Transform;
    use crate::pipeline::ingest_filter::IngestFilterTransform;
    use crate::pipeline::level::CompressionLevel;

    fn req(provider: Provider, body: serde_json::Value) -> InterceptedRequest {
        InterceptedRequest {
            provider,
            path: "/v1/chat/completions".into(),
            model: "m".into(),
            stream: false,
            client: None,
            project: None,
            session: None,
            api_key_hash: None,
            tokens_saved: 0,
            saved_by_stage: Vec::new(),
            routed_from: None,
            request_class: None,
            user_id: None,
            ccr: Default::default(),
            ccr_retrieve_calls: 0,
            pipeline_ms: 0,
            budget_reserved: 0.0,
            team_budget_reserved: 0.0,
            team_budget_reserved_at: None,
            body,
        }
    }

    fn lines(n: usize) -> String {
        (0..n)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// An assistant message carrying one OpenAI tool call.
    fn call_msg(id: &str, name: &str, arguments: &str) -> serde_json::Value {
        serde_json::json!({
            "role": "assistant",
            "tool_calls": [{
                "id": id,
                "type": "function",
                "function": { "name": name, "arguments": arguments }
            }]
        })
    }

    fn tool_msg(id: &str, content: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "role": "tool", "tool_call_id": id, "content": content })
    }

    const BIG_ARGUMENTS: &str =
        r#"{"command":"cargo test --all-features -- --nocapture","extra":"x"}"#;

    // ---- size filter -----------------------------------------------------

    #[tokio::test]
    async fn openai_tool_string_content_is_truncated() {
        let filter = IngestFilterTransform::fixed(CompressionLevel::Light);
        let original = lines(6000); // ≈ 15k tokens, far above the Light floor
        let mut r = req(
            Provider::Openai,
            serde_json::json!({
                "messages": [
                    call_msg("call_1", "Bash", BIG_ARGUMENTS),
                    tool_msg("call_1", serde_json::json!(original)),
                ]
            }),
        );
        let before = crate::tokenizer::count(&original);
        filter.on_request(&mut r).await.unwrap();

        let out = r.body["messages"][1]["content"].as_str().unwrap();
        assert!(
            out.contains("lines omitted by rtk-proxy"),
            "middle was marked: {out}"
        );
        assert!(out.len() < original.len() / 10, "truncated hard");
        assert!(r.tokens_saved > 0);
        // Savings assertion: a big tool dump must clear the ≥20% target.
        assert!(
            r.tokens_saved as f64 / before as f64 > 0.2,
            "saved {} of {} tokens",
            r.tokens_saved,
            before
        );
    }

    #[tokio::test]
    async fn openai_parts_content_is_truncated() {
        let filter = IngestFilterTransform::fixed(CompressionLevel::Light);
        let original = lines(6000);
        let mut r = req(
            Provider::Openai,
            serde_json::json!({
                "messages": [
                    call_msg("call_1", "Read", r#"{ "path": "big.log" }"#),
                    tool_msg("call_1", serde_json::json!([{ "type": "text", "text": original }])),
                ]
            }),
        );
        filter.on_request(&mut r).await.unwrap();

        let part = &r.body["messages"][1]["content"][0]["text"];
        let out = part.as_str().unwrap();
        assert!(out.contains("lines omitted by rtk-proxy"));
        assert!(r.tokens_saved > 0);
    }

    // ---- dedup + markers ---------------------------------------------------

    #[tokio::test]
    async fn openai_dedup_marker_names_originating_call() {
        let filter = IngestFilterTransform::dedup_only();
        let big = lines(400);
        let mut r = req(
            Provider::Openai,
            serde_json::json!({
                "messages": [
                    call_msg("call_1", "Bash", r#"{ "command": "cargo test" }"#),
                    tool_msg("call_1", serde_json::json!(big)),
                    call_msg("call_2", "Bash", r#"{ "command": "cargo test" }"#),
                    tool_msg("call_2", serde_json::json!(big)),
                ]
            }),
        );
        filter.on_request(&mut r).await.unwrap();

        let second = r.body["messages"][3]["content"].as_str().unwrap();
        assert!(second.contains("Bash"), "marker names the call: {second}");
        assert!(second.len() < big.len() / 2, "repeat collapsed");
        assert!(r.tokens_saved > 0);
    }

    #[tokio::test]
    async fn openai_function_arguments_are_untouched() {
        let filter = IngestFilterTransform::fixed(CompressionLevel::Light);
        let mut r = req(
            Provider::Openai,
            serde_json::json!({
                "messages": [
                    call_msg("call_1", "Write", &format!(r#"{{ "content": "{}" }}"#, lines(6000))),
                    tool_msg("call_1", serde_json::json!(lines(6000))),
                ]
            }),
        );
        filter.on_request(&mut r).await.unwrap();

        let args = r.body["messages"][0]["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .unwrap();
        // Idempotency parity with ccr/openai.rs: a tool_calls value is never
        // touched, however big.
        assert!(
            args.contains(&lines(6000)[..200]),
            "arguments stayed verbatim"
        );
    }

    // ---- cache safety ------------------------------------------------------

    #[tokio::test]
    async fn openai_history_prefix_stays_byte_identical_as_conversation_grows() {
        let filter = IngestFilterTransform::dedup_only();
        let big = lines(400);
        // Base already carries one dedup'd repeat (so turn N saves); the next
        // turn appends a third copy of the same content.
        let base = vec![
            call_msg("call_1", "Bash", r#"{ "command": "cargo test" }"#),
            tool_msg("call_1", serde_json::json!(big)),
            call_msg("call_2", "Bash", r#"{ "command": "cargo test" }"#),
            tool_msg("call_2", serde_json::json!(big)),
        ];
        let mut grown = base.clone();
        grown.push(call_msg("call_3", "Bash", r#"{ "command": "cargo test" }"#));
        grown.push(tool_msg("call_3", serde_json::json!(big)));

        let mut r_n = req(Provider::Openai, serde_json::json!({ "messages": base }));
        let mut r_n1 = req(Provider::Openai, serde_json::json!({ "messages": grown }));
        filter.on_request(&mut r_n).await.unwrap();
        filter.on_request(&mut r_n1).await.unwrap();

        let a = r_n.body["messages"].as_array().unwrap();
        let b = r_n1.body["messages"].as_array().unwrap();
        assert!(r_n.tokens_saved > 0, "the repeated block was dedup'd");
        assert!(r_n1.tokens_saved > 0, "the third copy was dedup'd too");
        for (i, msg) in a.iter().enumerate() {
            assert_eq!(
                msg, &b[i],
                "turn-N message {i} must be byte-identical in turn N+1"
            );
        }
    }

    #[tokio::test]
    async fn openai_refilter_is_a_noop() {
        let filter = IngestFilterTransform::fixed(CompressionLevel::Light);
        let mut r = req(
            Provider::Openai,
            serde_json::json!({
                "messages": [
                    call_msg("call_1", "Bash", BIG_ARGUMENTS),
                    tool_msg("call_1", serde_json::json!(lines(6000))),
                ]
            }),
        );
        filter.on_request(&mut r).await.unwrap();
        let once = r.body.clone();
        filter.on_request(&mut r).await.unwrap();
        assert_eq!(once, r.body, "re-filtering filtered text is a no-op");
    }

    #[tokio::test]
    async fn openai_invalid_json_arguments_fall_back_to_name_only_marker() {
        let filter = IngestFilterTransform::dedup_only();
        let big = lines(400);
        // Truncated / non-JSON arguments: the marker falls back to the name
        // only, and the raw string is never touched.
        let bad_args = r#"{"command": "cargo te"#;
        let mut r = req(
            Provider::Openai,
            serde_json::json!({
                "messages": [
                    call_msg("call_1", "Bash", bad_args),
                    tool_msg("call_1", serde_json::json!(big)),
                    call_msg("call_2", "Bash", bad_args),
                    tool_msg("call_2", serde_json::json!(big)),
                ]
            }),
        );
        filter.on_request(&mut r).await.unwrap();

        let second = r.body["messages"][3]["content"].as_str().unwrap();
        assert!(second.contains("Bash"), "name-only marker: {second}");
        assert!(
            r.body["messages"][0]["tool_calls"][0]["function"]["arguments"]
                .as_str()
                .unwrap()
                == bad_args,
            "arguments stayed verbatim"
        );
        assert!(r.tokens_saved > 0);
    }

    #[tokio::test]
    async fn openai_junk_content_shapes_are_preserved() {
        let filter = IngestFilterTransform::fixed(CompressionLevel::Light);
        let big = lines(6000);
        let mut r = req(
            Provider::Openai,
            serde_json::json!({
                "messages": [
                    // Junk shapes a hostile peer could send: forwarded verbatim.
                    tool_msg("junk_null", serde_json::Value::Null),
                    tool_msg("junk_num", serde_json::json!(42)),
                    tool_msg(
                        "junk_parts",
                        serde_json::json!([{ "type": "text" }, { "text": 7 }, "bare"]),
                    ),
                    // The one well-formed big message IS truncated.
                    tool_msg("valid", serde_json::json!(big)),
                ]
            }),
        );
        filter.on_request(&mut r).await.unwrap();

        assert_eq!(r.body["messages"][0]["content"], serde_json::Value::Null);
        assert_eq!(r.body["messages"][1]["content"], serde_json::json!(42));
        let junk_parts = r.body["messages"][2]["content"].as_array().unwrap();
        assert_eq!(junk_parts.len(), 3, "junk parts kept as-is");
        let valid = r.body["messages"][3]["content"].as_str().unwrap();
        assert!(
            valid.contains("lines omitted by rtk-proxy"),
            "valid content truncated"
        );
        assert!(r.tokens_saved > 0);
    }

    // ---- scope ---------------------------------------------------------------

    #[tokio::test]
    async fn non_standard_fields_are_forwarded_verbatim() {
        let filter = IngestFilterTransform::fixed(CompressionLevel::Light);
        let thinking = serde_json::json!({ "steps": lines(300) });
        let mut r = req(
            Provider::Openai,
            serde_json::json!({
                "messages": [
                    { "role": "assistant", "content": "answer", "thinking": thinking },
                    call_msg("call_1", "Bash", BIG_ARGUMENTS),
                    tool_msg("call_1", serde_json::json!(lines(6000))),
                ]
            }),
        );
        filter.on_request(&mut r).await.unwrap();
        assert_eq!(
            r.body["messages"][0]["thinking"], thinking,
            "non-standard extension fields are forwarded verbatim"
        );
    }

    #[tokio::test]
    async fn non_openai_non_anthropic_providers_stay_untouched() {
        let filter = IngestFilterTransform::dedup_only();
        let body = serde_json::json!({
            "messages": [
                call_msg("call_1", "Bash", BIG_ARGUMENTS),
                tool_msg("call_1", serde_json::json!(lines(400))),
            ]
        });
        let mut r = req(Provider::Google, body.clone());
        filter.on_request(&mut r).await.unwrap();
        assert_eq!(r.body, body, "unknown dialect is forwarded unchanged");
    }
}
