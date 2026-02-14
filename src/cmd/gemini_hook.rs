//! Gemini Hook Protocol Handler
//! Handles JSON payloads for 'BeforeTool' events.

use serde::{Deserialize, Serialize};
use std::io::{self, Read};
use super::hook::{HookResult, check_for_hook};

#[derive(Deserialize)]
struct GeminiPayload {
    #[serde(rename = "type")]
    event_type: String,
    tool_input: Option<ToolInput>,
}

#[derive(Deserialize)]
struct ToolInput {
    command: String,
}

#[derive(Serialize)]
struct GeminiResponse {
    result: String, // "allow" or "deny"
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    modified_input: Option<ModifiedInput>,
}

#[derive(Serialize)]
struct ModifiedInput {
    command: String,
}

/// Run the Gemini hook handler
/// Reads JSON from stdin, processes it, outputs JSON to stdout
pub fn run() -> anyhow::Result<()> {
    // 1. Read JSON from stdin
    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;

    let payload: GeminiPayload = match serde_json::from_str(&buffer) {
        Ok(p) => p,
        Err(_) => {
            // Not a tool event we care about, or malformed. Allow.
            println!(r#"{{"result": "allow"}}"#);
            return Ok(());
        }
    };

    // 2. Only handle shell execution events
    // (Adjust event name based on specific Gemini CLI implementation)
    if payload.event_type != "BeforeTool" {
        println!(r#"{{"result": "allow"}}"#);
        return Ok(());
    }

    let cmd = match payload.tool_input {
        Some(input) => input.command,
        None => {
            println!(r#"{{"result": "allow"}}"#);
            return Ok(());
        }
    };

    // 3. Run RTK Logic
    let decision = check_for_hook(&cmd, "gemini");

    // 4. Output JSON Decision
    let response = match decision {
        HookResult::Rewrite(new_cmd) => {
            if new_cmd == cmd {
                // No change
                GeminiResponse {
                    result: "allow".into(),
                    message: None,
                    modified_input: None,
                }
            } else {
                // Rewrite (e.g. wrapping in rtk run, or swapping rm->trash)
                GeminiResponse {
                    result: "allow".into(),
                    message: Some("RTK applied safety optimizations.".into()),
                    modified_input: Some(ModifiedInput { command: new_cmd }),
                }
            }
        }
        HookResult::Blocked(msg) => {
            GeminiResponse {
                result: "deny".into(),
                message: Some(msg),
                modified_input: None,
            }
        }
    };

    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gemini_payload_deserialize() {
        let json = r#"{"type": "BeforeTool", "tool_input": {"command": "git status"}}"#;
        let payload: GeminiPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.event_type, "BeforeTool");
        assert_eq!(payload.tool_input.unwrap().command, "git status");
    }

    #[test]
    fn test_gemini_response_serialize_allow() {
        let response = GeminiResponse {
            result: "allow".into(),
            message: None,
            modified_input: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains(r#""result":"allow""#));
    }

    #[test]
    fn test_gemini_response_serialize_deny() {
        let response = GeminiResponse {
            result: "deny".into(),
            message: Some("Blocked for safety".into()),
            modified_input: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains(r#""result":"deny""#));
        assert!(json.contains("Blocked for safety"));
    }

    #[test]
    fn test_gemini_response_with_modified_input() {
        let response = GeminiResponse {
            result: "allow".into(),
            message: Some("RTK applied safety optimizations.".into()),
            modified_input: Some(ModifiedInput {
                command: "rtk run -c 'git status'".into(),
            }),
        };
        let json = serde_json::to_string(&response).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["result"], "allow");
        assert_eq!(parsed["modified_input"]["command"], "rtk run -c 'git status'");
    }

    #[test]
    fn test_gemini_payload_unknown_type() {
        let json = r#"{"type": "Unknown", "tool_input": {"command": "git status"}}"#;
        let payload: GeminiPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.event_type, "Unknown");
    }

    #[test]
    fn test_gemini_payload_no_tool_input() {
        let json = r#"{"type": "BeforeTool"}"#;
        let payload: GeminiPayload = serde_json::from_str(json).unwrap();
        assert!(payload.tool_input.is_none());
    }
}
