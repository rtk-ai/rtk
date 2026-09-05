//! Output-only adapters for host tool results.
//!
//! This module deliberately never executes tool_input. Its Claude hook entry
//! point emits additionalContext, which is supplemental in Claude's
//! PostToolUse contract; callers must not treat it as replacement output.

use crate::core::filter::{self, FilterLevel, Language};
use serde_json::{json, Value};
use std::io::{self, Read, Write};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptedOutput {
    pub output: Value,
    pub changed: bool,
    pub original_bytes: usize,
    pub shown_bytes: usize,
    pub omitted_lines: usize,
}

fn unchanged(output: &Value) -> AdaptedOutput {
    let bytes = serde_json::to_string(output)
        .map(|value| value.len())
        .unwrap_or_default();
    AdaptedOutput {
        output: output.clone(),
        changed: false,
        original_bytes: bytes,
        shown_bytes: bytes,
        omitted_lines: 0,
    }
}

/// Filter one already-produced host result.
///
/// Only textual native Read output is currently eligible. The returned value
/// retains original source line numbers and is suitable for a supplemental
/// context channel, not an authorization or replacement decision.
pub fn adapt_tool_output(
    tool_name: &str,
    tool_input: Option<&Value>,
    output: &Value,
) -> AdaptedOutput {
    if !matches!(tool_name, "Read" | "read") {
        return unchanged(output);
    }
    if tool_input
        .and_then(|input| input.get("rtk_execution_id"))
        .is_some()
    {
        return unchanged(output);
    }
    let Some(text) = output.as_str() else {
        return unchanged(output);
    };
    let lines = filter::get_filter(FilterLevel::Minimal).filter_lines(text, &Language::Unknown);
    if lines.is_empty() || lines.len() >= text.lines().count() {
        return unchanged(output);
    }
    let filtered = lines
        .iter()
        .map(|line| format!("{}: {}", line.original_line, line.text))
        .collect::<Vec<_>>()
        .join("\n");
    if filtered.is_empty() || filtered.len() >= text.len() {
        return unchanged(output);
    }
    AdaptedOutput {
        output: Value::String(filtered.clone()),
        changed: true,
        original_bytes: text.len(),
        shown_bytes: filtered.len(),
        omitted_lines: text.lines().count().saturating_sub(lines.len()),
    }
}

/// Build the Claude PostToolUse supplemental response, if the payload is a
/// validated and materially smaller native Read result.
pub fn post_tool_use_response(input: &Value) -> Option<Value> {
    if input.get("hook_event_name").and_then(Value::as_str) != Some("PostToolUse") {
        return None;
    }
    let tool_name = input.get("tool_name").and_then(Value::as_str)?;
    let output = input.get("tool_response")?;
    let adapted = adapt_tool_output(tool_name, input.get("tool_input"), output);
    if !adapted.changed {
        return None;
    }
    Some(json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": {
                "adapter": "rtk-output-only",
                "replacement_supported": false,
                "original_bytes": adapted.original_bytes,
                "shown_bytes": adapted.shown_bytes,
                "omitted_lines": adapted.omitted_lines,
                "output": adapted.output
            }
        }
    }))
}

/// Run the opt-in Claude PostToolUse output-only adapter.
pub fn run_claude_post_tool_use() -> anyhow::Result<()> {
    const INPUT_CAP: usize = 10 * 1024 * 1024;
    let mut input = String::new();
    io::stdin()
        .take((INPUT_CAP + 1) as u64)
        .read_to_string(&mut input)?;
    if input.len() > INPUT_CAP {
        eprintln!("rtk output adapter: input exceeds {INPUT_CAP} bytes");
        return Ok(());
    }
    let payload = match serde_json::from_str::<Value>(input.trim()) {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("rtk output adapter: invalid JSON: {error}");
            return Ok(());
        }
    };
    if let Some(response) = post_tool_use_response(&payload) {
        let mut stdout = io::stdout().lock();
        serde_json::to_writer(&mut stdout, &response)?;
        stdout.write_all(b"\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_output_is_filtered_without_running_tool_input() {
        let input = json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Read",
            "tool_input": { "file_path": "src/main.rs", "command": "do not execute" },
            "tool_response": "// comment\nfn main() {}\n"
        });
        let response = post_tool_use_response(&input).expect("supplemental response");
        assert!(
            response["hookSpecificOutput"]["additionalContext"]["output"]
                .as_str()
                .unwrap()
                .contains("2: fn main() {}")
        );
        assert_eq!(
            response["hookSpecificOutput"]["additionalContext"]["replacement_supported"],
            false
        );
    }

    #[test]
    fn errors_images_bash_and_unknown_schemas_are_unchanged() {
        for input in [
            json!({
                "hook_event_name": "PostToolUse",
                "tool_name": "Bash",
                "tool_input": { "command": "cargo test" },
                "tool_response": { "stdout": "ok", "stderr": "error", "exit_code": 1 }
            }),
            json!({
                "hook_event_name": "PostToolUse",
                "tool_name": "Read",
                "tool_response": { "type": "image", "data": "..." }
            }),
            json!({
                "hook_event_name": "PostToolUse",
                "tool_name": "Read",
                "tool_input": { "rtk_execution_id": "already-filtered" },
                "tool_response": "one\n"
            }),
            json!({
                "hook_event_name": "PostToolUse",
                "tool_name": "NewTool",
                "tool_response": "large\ntext\n"
            }),
        ] {
            assert!(post_tool_use_response(&input).is_none());
        }
    }
}
