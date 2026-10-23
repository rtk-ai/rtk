//! OpenAI-dialect navigation for [`super::normalize`].
//!
//! `normalise_tool_result` / `block_chars` operate on a `content` field that is
//! either a plain string or an array of `{ type: "text", text }` parts. The
//! OpenAI `role: "tool"` message carries the same field with the same shapes,
//! so the rewrite runs in place on the message object. Tool names for the
//! Read/NotebookRead exemption come from assistant `tool_calls[]` entries
//! (`id` → `function.name`) rather than Anthropic's `tool_use` blocks (same
//! history-based resolution, different wire shape).

use std::collections::HashMap;

use super::normalize::{Stats, block_chars, is_read_tool, normalise_tool_result};

/// Map `tool_call_id` → function name from assistant `tool_calls[]` entries,
/// so the Read/NotebookRead exemption can be resolved for a tool message.
/// Deliberately NOT `ingest_openai::collect_calls`: the name-only walk skips
/// the `function.arguments` JSON parse, which would be wasted work (O(body) on
/// huge Write/Edit arguments) to build a label this stage never prints.
fn tool_name_by_call_id(messages: &[serde_json::Value]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for msg in messages {
        if msg.get("role").and_then(|r| r.as_str()) != Some("assistant") {
            continue;
        }
        let Some(tool_calls) = msg.get("tool_calls").and_then(|t| t.as_array()) else {
            continue;
        };
        for tc in tool_calls {
            if let (Some(id), Some(name)) = (
                tc.get("id").and_then(|v| v.as_str()),
                tc.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|v| v.as_str()),
            ) {
                map.insert(id.to_string(), name.to_string());
            }
        }
    }
    map
}

/// Normalise every `role: "tool"` message in the history, in place. The
/// rewrite is a pure function of each message's own content, so the
/// provider-visible prefix stays byte-identical across turns.
pub(crate) fn run(messages: &mut [serde_json::Value]) -> Stats {
    let tools = tool_name_by_call_id(messages);
    let mut stats = Stats::default();
    for m in messages.iter_mut() {
        if m.get("role").and_then(|r| r.as_str()) != Some("tool") {
            continue;
        }
        let is_read = is_read_tool(
            m.get("tool_call_id")
                .and_then(|v| v.as_str())
                .and_then(|id| tools.get(id).map(|t| t.as_str())),
        );
        let pre = block_chars(m);
        let removed = normalise_tool_result(m, is_read);
        if removed > 0 {
            stats.blocks += 1;
            stats.chars_removed += removed;
            stats.before += pre;
            stats.after += block_chars(m);
        }
    }
    stats
}

#[cfg(test)]
mod tests {
    use crate::model::{InterceptedRequest, Provider};
    use crate::pipeline::Transform;
    use crate::pipeline::normalize::NormalizeTransform;

    fn req(body: serde_json::Value) -> InterceptedRequest {
        InterceptedRequest {
            provider: Provider::Openai,
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

    fn ansi_output() -> String {
        // Real terminal noise: color codes + progress redraws + trailing spaces.
        let mut s = String::new();
        for i in 0..50 {
            s.push_str(&format!("\x1b[32mcompiling crate-{i}\x1b[0m   \r"));
            s.push_str(&format!("compiling crate-{i}\n"));
        }
        s.push_str("   \n");
        s
    }

    fn call_msg(id: &str, name: &str) -> serde_json::Value {
        serde_json::json!({
            "role": "assistant",
            "tool_calls": [{
                "id": id,
                "type": "function",
                "function": { "name": name, "arguments": "{}" }
            }]
        })
    }

    fn tool_msg(id: &str, content: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "role": "tool", "tool_call_id": id, "content": content })
    }

    #[tokio::test]
    async fn openai_ansi_and_whitespace_are_stripped() {
        let normalize = NormalizeTransform::new();
        let mut r = req(serde_json::json!({
            "messages": [
                call_msg("call_1", "Bash"),
                tool_msg("call_1", serde_json::json!(ansi_output())),
            ]
        }));
        let before = r.body["messages"][1]["content"]
            .as_str()
            .unwrap()
            .to_string();
        normalize.on_request(&mut r).await.unwrap();

        let out = r.body["messages"][1]["content"].as_str().unwrap();
        assert!(!out.contains('\x1b'), "ANSI escapes removed");
        assert!(out.len() < before.len(), "whitespace collapsed");
        assert!(r.tokens_saved > 0, "saving recorded");
    }

    #[tokio::test]
    async fn openai_parts_content_is_normalised() {
        let normalize = NormalizeTransform::new();
        let mut r = req(serde_json::json!({
            "messages": [
                call_msg("call_1", "Bash"),
                tool_msg("call_1", serde_json::json!([{ "type": "text", "text": ansi_output() }])),
            ]
        }));
        normalize.on_request(&mut r).await.unwrap();

        let out = r.body["messages"][1]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert!(!out.contains('\x1b'), "ANSI escapes removed from parts");
        assert!(r.tokens_saved > 0);
    }

    #[tokio::test]
    async fn openai_read_results_stay_verbatim() {
        let normalize = NormalizeTransform::new();
        // A Read result is the model's view of bytes on disk: a failed later
        // Edit costs more than the compression saved, so it must come through
        // untouched even with ANSI-ish bytes inside.
        let file_bytes = "    1  let x = 1;\r\n    2  \x1b[1mlet y = 2;\x1b[0m\r\n";
        let mut r = req(serde_json::json!({
            "messages": [
                call_msg("call_1", "Read"),
                tool_msg("call_1", serde_json::json!(file_bytes)),
            ]
        }));
        normalize.on_request(&mut r).await.unwrap();

        let out = r.body["messages"][1]["content"].as_str().unwrap();
        assert_eq!(out, file_bytes, "Read results pass through verbatim");
        assert_eq!(r.tokens_saved, 0);
    }

    #[tokio::test]
    async fn openai_null_and_junk_content_is_preserved() {
        let normalize = NormalizeTransform::new();
        let body = serde_json::json!({
            "messages": [
                tool_msg("junk_null", serde_json::Value::Null),
                tool_msg("junk_num", serde_json::json!(42)),
                tool_msg(
                    "junk_parts",
                    serde_json::json!([{ "type": "text" }, { "text": 7 }]),
                ),
                tool_msg("valid", serde_json::json!(ansi_output())),
            ]
        });
        let mut r = req(body);
        normalize.on_request(&mut r).await.unwrap();

        assert_eq!(r.body["messages"][0]["content"], serde_json::Value::Null);
        assert_eq!(r.body["messages"][1]["content"], serde_json::json!(42));
        let junk_parts = r.body["messages"][2]["content"].as_array().unwrap();
        assert_eq!(junk_parts.len(), 2);
        let valid = r.body["messages"][3]["content"].as_str().unwrap();
        assert!(
            !valid.contains('\x1b'),
            "well-formed content still normalised"
        );
    }

    #[tokio::test]
    async fn openai_non_tool_messages_are_untouched() {
        let normalize = NormalizeTransform::new();
        let body = serde_json::json!({
            "messages": [
                { "role": "user", "content": "run \x1b[31mit\x1b[0m   please" },
                call_msg("call_1", "Bash"),
                tool_msg("call_1", serde_json::json!(ansi_output())),
            ]
        });
        let user_before = body["messages"][0]["content"].clone();
        let mut r = req(body);
        normalize.on_request(&mut r).await.unwrap();

        assert_eq!(
            r.body["messages"][0]["content"], user_before,
            "plain user text is not normalised"
        );
    }
}
