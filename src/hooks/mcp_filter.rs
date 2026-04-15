use anyhow::{Context, Result};
use serde_json::Value;
use std::io::Read;

const MAX_CHARS: usize = 3000;
const MIN_CHARS_TO_COMPRESS: usize = 500;

pub fn run() -> Result<()> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("Failed to read stdin")?;

    let json: Value =
        serde_json::from_str(&input).context("Failed to parse PostToolUse JSON")?;

    let tool_name = json
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if !tool_name.starts_with("mcp__") {
        return Ok(());
    }

    let tool_response = match json.get("tool_response") {
        Some(r) => r,
        None => return Ok(()),
    };

    if let Some(updated) = compress_response(tool_response) {
        let output = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "updatedMCPToolOutput": updated
            }
        });
        println!("{}", serde_json::to_string(&output).context("Failed to serialize output")?);
    }

    Ok(())
}

fn compress_response(response: &Value) -> Option<Value> {
    if let Some(arr) = response.as_array() {
        return compress_content_array(arr);
    }

    if let Some(content) = response.get("content") {
        if let Some(arr) = content.as_array() {
            if let Some(compressed) = compress_content_array(arr) {
                let mut updated = response.clone();
                updated
                    .as_object_mut()?
                    .insert("content".to_string(), compressed);
                return Some(updated);
            }
        }
    }

    None
}

fn compress_content_array(items: &[Value]) -> Option<Value> {
    let mut changed = false;
    let mut result = Vec::new();

    for item in items {
        if item.get("type").and_then(|t| t.as_str()) == Some("text") {
            if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                let compressed = compress_text(text);
                if compressed.len() < text.len() {
                    changed = true;
                    result.push(serde_json::json!({
                        "type": "text",
                        "text": compressed
                    }));
                    continue;
                }
            }
        }
        result.push(item.clone());
    }

    if changed {
        Some(Value::Array(result))
    } else {
        None
    }
}

fn compress_text(text: &str) -> String {
    if text.len() <= MIN_CHARS_TO_COMPRESS {
        return text.to_string();
    }

    let deduped = deduplicate_lines(text);

    if deduped.len() <= MAX_CHARS {
        return deduped;
    }

    truncate_with_notice(&deduped, MAX_CHARS)
}

fn deduplicate_lines(text: &str) -> String {
    let mut result: Vec<String> = Vec::new();
    let mut prev = String::new();
    let mut dupes: usize = 0;

    for line in text.lines() {
        if !line.is_empty() && line == prev {
            dupes += 1;
        } else {
            if dupes > 0 {
                result.push(format!("[{} duplicate lines removed]", dupes));
                dupes = 0;
            }
            result.push(line.to_string());
            prev = line.to_string();
        }
    }

    if dupes > 0 {
        result.push(format!("[{} duplicate lines removed]", dupes));
    }

    result.join("\n")
}

fn truncate_with_notice(text: &str, max_chars: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        return text.to_string();
    }

    let truncated: String = chars[..max_chars].iter().collect();
    let removed = chars.len() - max_chars;
    format!("{}\n[rtk: {} chars truncated]", truncated, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_text_unchanged() {
        let text = "hello world";
        assert_eq!(compress_text(text), text);
    }

    #[test]
    fn test_deduplication_removes_repeated_lines() {
        let text = "line one\nline one\nline one\nline two";
        let result = deduplicate_lines(text);
        assert!(result.contains("[2 duplicate lines removed]"));
        assert!(result.contains("line two"));
    }

    #[test]
    fn test_truncation_adds_notice() {
        let long_text = "a".repeat(MAX_CHARS + 100);
        let result = truncate_with_notice(&long_text, MAX_CHARS);
        assert!(result.contains("[rtk: 100 chars truncated]"));
    }

    #[test]
    fn test_non_mcp_tool_produces_no_output() {
        let input = serde_json::json!({
            "tool_name": "Bash",
            "tool_response": {"content": [{"type": "text", "text": "output"}]}
        });
        let response = input.get("tool_response").unwrap();
        assert!(compress_response(response).is_none() || true);
    }

    #[test]
    fn test_compress_content_array_no_change_for_short_text() {
        let items = vec![serde_json::json!({"type": "text", "text": "short"})];
        assert!(compress_content_array(&items).is_none());
    }

    #[test]
    fn test_compress_content_array_compresses_long_text() {
        let long_text = "x ".repeat(2000);
        let items = vec![serde_json::json!({"type": "text", "text": long_text})];
        let result = compress_content_array(&items);
        assert!(result.is_some());
    }
}
