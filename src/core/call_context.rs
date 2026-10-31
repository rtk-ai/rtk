use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};
use std::sync::OnceLock;

const TAIL_BYTES: u64 = 512 * 1024;
const INTENT_CHARS: usize = 600;
const GOAL_CHARS: usize = 600;
const RECENT_CALLS: usize = 5;
const CALL_CHARS: usize = 120;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct CallContext {
    pub intent: Option<String>,
    pub goal: Option<String>,
    pub recent_calls: Vec<String>,
}

#[derive(Debug, Default)]
struct Source {
    transcript: Option<String>,
    agent_message: Option<String>,
    description: Option<String>,
}

static SOURCE: OnceLock<Source> = OnceLock::new();
static CONTEXT: OnceLock<CallContext> = OnceLock::new();

pub fn set_source(event: &Value) {
    let text = |v: Option<&Value>| {
        v.and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let _ = SOURCE.set(Source {
        transcript: text(event.get("transcript_path")),
        agent_message: text(event.get("agent_message")),
        description: text(event.pointer("/tool_input/description")),
    });
}

pub fn get() -> &'static CallContext {
    CONTEXT.get_or_init(|| match SOURCE.get() {
        Some(source) => assemble(source),
        None => CallContext::default(),
    })
}

fn assemble(source: &Source) -> CallContext {
    let mut context = source
        .transcript
        .as_deref()
        .and_then(read_tail)
        .map(|tail| from_transcript(&tail))
        .unwrap_or_default();
    if let Some(message) = &source.agent_message {
        context.intent = Some(message.clone());
    }
    if context.intent.is_none() {
        context.intent = source.description.clone();
    }
    context.intent = context.intent.map(|t| tail_chars(&t, INTENT_CHARS));
    context.goal = context.goal.map(|t| head_chars(&t, GOAL_CHARS));
    context
}

fn read_tail(path: &str) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    Some(if start > 0 {
        text.split_once('\n')
            .map(|(_, rest)| rest.to_string())
            .unwrap_or_default()
    } else {
        text
    })
}

enum Piece {
    AssistantText(String),
    UserText(String),
    Call(String),
}

pub fn from_transcript(tail: &str) -> CallContext {
    let mut context = CallContext::default();
    for line in tail.lines().rev() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        for piece in pieces(&entry).into_iter().rev() {
            match piece {
                Piece::AssistantText(text) if context.intent.is_none() => {
                    context.intent = Some(text)
                }
                Piece::Call(call) if context.recent_calls.len() < RECENT_CALLS => {
                    context.recent_calls.push(call)
                }
                Piece::UserText(text) => {
                    context.goal = Some(text);
                    return context;
                }
                _ => {}
            }
        }
    }
    context
}

fn pieces(entry: &Value) -> Vec<Piece> {
    if entry.get("isMeta").and_then(Value::as_bool) == Some(true) {
        return Vec::new();
    }
    let message = entry
        .get("message")
        .or_else(|| entry.get("payload"))
        .unwrap_or(entry);
    if let Some(call) = codex_call(message) {
        return vec![Piece::Call(call)];
    }
    if message.get("type").and_then(Value::as_str) == Some("reasoning") {
        return summary_text(message)
            .map(|t| vec![Piece::AssistantText(t)])
            .unwrap_or_default();
    }
    let role = message
        .get("role")
        .or_else(|| entry.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let content = message.get("content");
    let mut out = Vec::new();
    match (role, content) {
        ("user", Some(Value::String(text))) => push_user(&mut out, text),
        ("user", Some(Value::Array(blocks))) => {
            for block in blocks {
                if matches!(block_type(block), "text" | "input_text")
                    && let Some(text) = block.get("text").and_then(Value::as_str)
                {
                    push_user(&mut out, text);
                }
            }
        }
        ("assistant", Some(Value::String(text))) => push_text(&mut out, text),
        ("assistant", Some(Value::Array(blocks))) => {
            for block in blocks {
                match block_type(block) {
                    "text" | "output_text" => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            push_text(&mut out, text);
                        }
                    }
                    "thinking" => {
                        if let Some(text) = block.get("thinking").and_then(Value::as_str) {
                            push_text(&mut out, text);
                        }
                    }
                    "tool_use" => {
                        let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                        out.push(Piece::Call(render_call(name, block.get("input"))));
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    out
}

fn block_type(block: &Value) -> &str {
    block.get("type").and_then(Value::as_str).unwrap_or("")
}

fn push_text(out: &mut Vec<Piece>, text: &str) {
    let text = text.trim();
    if !text.is_empty() {
        out.push(Piece::AssistantText(text.to_string()));
    }
}

fn push_user(out: &mut Vec<Piece>, text: &str) {
    let text = text.trim();
    if !text.is_empty() && !text.starts_with('<') {
        out.push(Piece::UserText(text.to_string()));
    }
}

fn codex_call(message: &Value) -> Option<String> {
    let kind = message.get("type").and_then(Value::as_str)?;
    if !matches!(
        kind,
        "function_call" | "custom_tool_call" | "local_shell_call"
    ) {
        return None;
    }
    let name = message.get("name").and_then(Value::as_str).unwrap_or(kind);
    let args = message
        .get("arguments")
        .or_else(|| message.get("input"))
        .or_else(|| message.get("action"))
        .map(|a| match a {
            Value::String(s) => serde_json::from_str(s).unwrap_or(Value::String(s.clone())),
            other => other.clone(),
        });
    Some(render_call(name, args.as_ref()))
}

fn summary_text(message: &Value) -> Option<String> {
    let parts: Vec<&str> = message
        .get("summary")?
        .as_array()?
        .iter()
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect();
    let joined = parts.join("\n");
    (!joined.trim().is_empty()).then(|| joined.trim().to_string())
}

fn render_call(name: &str, input: Option<&Value>) -> String {
    let detail = input
        .and_then(|i| {
            [
                "command",
                "file_path",
                "path",
                "pattern",
                "url",
                "description",
                "cmd",
            ]
            .iter()
            .find_map(|k| i.get(*k))
        })
        .map(|v| match v {
            Value::String(s) => s.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
            other => other.to_string(),
        })
        .unwrap_or_default();
    head_chars(format!("{name} {detail}").trim(), CALL_CHARS)
}

fn head_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn tail_chars(text: &str, max: usize) -> String {
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(max)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(entries: &[Value]) -> String {
        entries
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn claude_transcript_gives_goal_intent_and_recent_calls() {
        let tail = lines(&[
            serde_json::json!({"type": "user", "message": {"role": "user", "content": "old prompt"}}),
            serde_json::json!({"type": "user", "message": {"role": "user", "content": "Speed up the slow test suite"}}),
            serde_json::json!({"type": "user", "isMeta": true, "message": {"role": "user", "content": "meta noise"}}),
            serde_json::json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "thinking", "thinking": "I need the fixture setup"}]}}),
            serde_json::json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "tool_use", "name": "Read", "input": {"file_path": "test_suite.py"}}]}}),
            serde_json::json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "content": "..."}]}}),
            serde_json::json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": "I'll read the slow fixtures first"}]}}),
            serde_json::json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "tool_use", "name": "Bash", "input": {"command": "sed -n 1,200p test_suite.py"}}]}}),
        ]);
        let context = from_transcript(&tail);
        assert_eq!(
            context.goal.as_deref(),
            Some("Speed up the slow test suite")
        );
        assert_eq!(
            context.intent.as_deref(),
            Some("I'll read the slow fixtures first")
        );
        assert_eq!(
            context.recent_calls,
            vec!["Bash sed -n 1,200p test_suite.py", "Read test_suite.py"]
        );
    }

    #[test]
    fn codex_rollout_gives_the_same_facts() {
        let tail = lines(&[
            serde_json::json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "<environment_context>cwd</environment_context>"}]}}),
            serde_json::json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "fix the flaky test"}]}}),
            serde_json::json!({"type": "response_item", "payload": {"type": "reasoning", "summary": [{"type": "summary_text", "text": "Look at the test runner first"}]}}),
            serde_json::json!({"type": "response_item", "payload": {"type": "function_call", "name": "shell", "arguments": "{\"command\":[\"bash\",\"-lc\",\"cargo test\"]}"}}),
        ]);
        let context = from_transcript(&tail);
        assert_eq!(context.goal.as_deref(), Some("fix the flaky test"));
        assert_eq!(
            context.intent.as_deref(),
            Some("Look at the test runner first")
        );
        assert_eq!(context.recent_calls, vec!["shell bash -lc cargo test"]);
    }

    #[test]
    fn intent_stops_at_the_latest_prompt() {
        let tail = lines(&[
            serde_json::json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": "previous turn text"}]}}),
            serde_json::json!({"type": "user", "message": {"role": "user", "content": "new question"}}),
        ]);
        let context = from_transcript(&tail);
        assert_eq!(context.goal.as_deref(), Some("new question"));
        assert_eq!(
            context.intent, None,
            "text from an earlier turn is not this call's intent"
        );
    }

    #[test]
    fn agent_message_wins_and_description_is_the_fallback() {
        let with_message = assemble(&Source {
            transcript: None,
            agent_message: Some("Installing dependencies...".into()),
            description: Some("install".into()),
        });
        assert_eq!(
            with_message.intent.as_deref(),
            Some("Installing dependencies...")
        );
        let fallback = assemble(&Source {
            transcript: Some("/definitely/not/here.jsonl".into()),
            agent_message: None,
            description: Some("run the tests".into()),
        });
        assert_eq!(fallback.intent.as_deref(), Some("run the tests"));
        assert_eq!(fallback.goal, None);
    }

    #[test]
    fn garbage_and_partial_lines_are_skipped() {
        let context = from_transcript("{not json\n\u{0}\n{\"type\":\"user\"}\n");
        assert_eq!(context, CallContext::default());
    }

    #[test]
    fn long_texts_are_trimmed() {
        let long = "x".repeat(5000);
        let context = assemble(&Source {
            transcript: None,
            agent_message: Some(format!("start {long} end")),
            description: None,
        });
        let intent = context.intent.expect("intent");
        assert_eq!(intent.chars().count(), INTENT_CHARS);
        assert!(intent.ends_with(" end"), "the latest words are kept");
    }
}
