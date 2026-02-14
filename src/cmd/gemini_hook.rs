//! Gemini Hook Protocol Handler
//!
//! Implements the Gemini CLI BeforeTool hook protocol.
//! See: https://geminicli.com/docs/hooks/reference/
//!
//! Input: JSON on stdin with hook_event_name, tool_name, tool_input
//! Output: JSON on stdout with decision, reason, hookSpecificOutput

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read};
use super::hook::{HookResult, check_for_hook};

#[derive(Deserialize)]
struct GeminiPayload {
    hook_event_name: Option<String>,
    tool_name: Option<String>,
    tool_input: Option<Value>,
}

#[derive(Serialize)]
struct GeminiResponse {
    decision: String, // "allow" or "deny"
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(rename = "hookSpecificOutput")]
    #[serde(skip_serializing_if = "Option::is_none")]
    hook_specific_output: Option<HookSpecificOutput>,
}

#[derive(Serialize)]
struct HookSpecificOutput {
    tool_input: Value,
}

/// Tool names that represent shell command execution in Gemini CLI
fn is_shell_tool(name: &str) -> bool {
    // Gemini CLI built-in shell tool, plus common MCP patterns
    name == "run_shell_command"
        || name == "shell"
        || name.ends_with("__run_shell_command")
}

/// Run the Gemini hook handler
/// Reads JSON from stdin, processes it, outputs JSON to stdout
pub fn run() -> anyhow::Result<()> {
    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;

    let payload: GeminiPayload = match serde_json::from_str(&buffer) {
        Ok(p) => p,
        Err(_) => {
            // Malformed or unrecognized payload — allow
            println!(r#"{{"decision": "allow"}}"#);
            return Ok(());
        }
    };

    // Only handle BeforeTool events
    if payload.hook_event_name.as_deref() != Some("BeforeTool") {
        println!(r#"{{"decision": "allow"}}"#);
        return Ok(());
    }

    // Only intercept shell execution tools
    let _tool_name = match &payload.tool_name {
        Some(name) if is_shell_tool(name) => name.clone(),
        _ => {
            println!(r#"{{"decision": "allow"}}"#);
            return Ok(());
        }
    };

    // Extract the command string from tool_input
    let cmd = match &payload.tool_input {
        Some(input) => {
            input.get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        }
        None => {
            println!(r#"{{"decision": "allow"}}"#);
            return Ok(());
        }
    };

    if cmd.is_empty() {
        println!(r#"{{"decision": "allow"}}"#);
        return Ok(());
    }

    // Run RTK safety logic
    let decision = check_for_hook(&cmd, "gemini");

    let response = match decision {
        HookResult::Rewrite(new_cmd) => {
            if new_cmd == cmd {
                GeminiResponse {
                    decision: "allow".into(),
                    reason: None,
                    hook_specific_output: None,
                }
            } else {
                // Build modified tool_input, preserving other fields
                let mut new_input = payload.tool_input.unwrap_or(Value::Object(Default::default()));
                if let Some(obj) = new_input.as_object_mut() {
                    obj.insert("command".into(), Value::String(new_cmd));
                }
                GeminiResponse {
                    decision: "allow".into(),
                    reason: Some("RTK applied safety optimizations.".into()),
                    hook_specific_output: Some(HookSpecificOutput {
                        tool_input: new_input,
                    }),
                }
            }
        }
        HookResult::Blocked(msg) => {
            GeminiResponse {
                decision: "deny".into(),
                reason: Some(msg),
                hook_specific_output: None,
            }
        }
    };

    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // GEMINI WIRE FORMAT CONFORMANCE
    // https://geminicli.com/docs/hooks/reference/
    //
    // These tests verify exact JSON field names per the Gemini CLI spec.
    // A wrong field name means Gemini silently ignores the response.
    // =========================================================================

    // --- Input: field name conformance ---

    #[test]
    fn test_input_uses_hook_event_name_not_type() {
        // Gemini sends "hook_event_name", NOT "type"
        let json = r#"{"hook_event_name": "BeforeTool", "tool_name": "run_shell_command", "tool_input": {"command": "git status"}}"#;
        let payload: GeminiPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.hook_event_name.as_deref(), Some("BeforeTool"));

        // Verify the old wrong field name does NOT populate our struct
        let wrong_json = r#"{"type": "BeforeTool", "tool_name": "run_shell_command"}"#;
        let payload: GeminiPayload = serde_json::from_str(wrong_json).unwrap();
        assert_eq!(payload.hook_event_name, None, "\"type\" must not be accepted as event name");
    }

    #[test]
    fn test_input_includes_tool_name() {
        let json = r#"{"hook_event_name": "BeforeTool", "tool_name": "run_shell_command", "tool_input": {"command": "ls"}}"#;
        let payload: GeminiPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.tool_name.as_deref(), Some("run_shell_command"));
    }

    #[test]
    fn test_input_tool_input_is_object() {
        let json = r#"{"hook_event_name": "BeforeTool", "tool_name": "run_shell_command", "tool_input": {"command": "git status", "timeout": 30}}"#;
        let payload: GeminiPayload = serde_json::from_str(json).unwrap();
        let input = payload.tool_input.unwrap();
        assert_eq!(input["command"].as_str().unwrap(), "git status");
        assert_eq!(input["timeout"].as_i64().unwrap(), 30);
    }

    #[test]
    fn test_input_extra_fields_ignored() {
        // Gemini sends session_id, cwd, timestamp, transcript_path etc.
        let json = r#"{"hook_event_name": "BeforeTool", "tool_name": "run_shell_command", "tool_input": {"command": "ls"}, "session_id": "abc123", "cwd": "/tmp", "timestamp": "2026-01-01T00:00:00Z", "transcript_path": "/path/to/transcript"}"#;
        let payload: GeminiPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.hook_event_name.as_deref(), Some("BeforeTool"));
    }

    // --- Output: field name conformance ---

    #[test]
    fn test_output_uses_decision_not_result() {
        // Gemini expects "decision", NOT "result"
        let response = GeminiResponse {
            decision: "allow".into(),
            reason: None,
            hook_specific_output: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert!(parsed.get("decision").is_some(), "must have 'decision' field");
        assert!(parsed.get("result").is_none(), "must NOT have 'result' field");
    }

    #[test]
    fn test_output_uses_reason_not_message() {
        // Gemini expects "reason", NOT "message"
        let response = GeminiResponse {
            decision: "deny".into(),
            reason: Some("Blocked for safety".into()),
            hook_specific_output: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert!(parsed.get("reason").is_some(), "must have 'reason' field");
        assert!(parsed.get("message").is_none(), "must NOT have 'message' field");
    }

    #[test]
    fn test_output_uses_hook_specific_output_not_modified_input() {
        // Gemini expects "hookSpecificOutput", NOT "modified_input"
        let response = GeminiResponse {
            decision: "allow".into(),
            reason: None,
            hook_specific_output: Some(HookSpecificOutput {
                tool_input: serde_json::json!({"command": "rtk run -c 'ls'"}),
            }),
        };
        let json = serde_json::to_string(&response).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert!(parsed.get("hookSpecificOutput").is_some(), "must have 'hookSpecificOutput' field");
        assert!(parsed.get("modified_input").is_none(), "must NOT have 'modified_input' field");
    }

    #[test]
    fn test_output_rewrite_nests_under_tool_input() {
        // Gemini merges hookSpecificOutput.tool_input into the original
        let response = GeminiResponse {
            decision: "allow".into(),
            reason: Some("RTK applied safety optimizations.".into()),
            hook_specific_output: Some(HookSpecificOutput {
                tool_input: serde_json::json!({"command": "rtk run -c 'git status'"}),
            }),
        };
        let json = serde_json::to_string(&response).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["hookSpecificOutput"]["tool_input"]["command"],
            "rtk run -c 'git status'");
    }

    #[test]
    fn test_output_allow_omits_optional_fields() {
        let response = GeminiResponse {
            decision: "allow".into(),
            reason: None,
            hook_specific_output: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(!json.contains("reason"), "reason must be omitted when None");
        assert!(!json.contains("hookSpecificOutput"), "hookSpecificOutput must be omitted when None");
    }

    #[test]
    fn test_output_decision_values() {
        // Only "allow" and "deny" are valid
        for val in ["allow", "deny"] {
            let response = GeminiResponse {
                decision: val.into(),
                reason: Some("test".into()),
                hook_specific_output: None,
            };
            let json = serde_json::to_string(&response).unwrap();
            let parsed: Value = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed["decision"].as_str().unwrap(), val);
        }
    }

    // --- Tool filtering ---

    #[test]
    fn test_is_shell_tool() {
        assert!(is_shell_tool("run_shell_command"));
        assert!(is_shell_tool("shell"));
        assert!(is_shell_tool("mcp__server__run_shell_command"));
        assert!(!is_shell_tool("read_file"));
        assert!(!is_shell_tool("write_file"));
        assert!(!is_shell_tool("search_code"));
        assert!(!is_shell_tool("list_directory"));
    }

    #[test]
    fn test_non_shell_tools_always_allowed() {
        // read_file, write_file, etc. must never be intercepted
        for tool in ["read_file", "write_file", "search_code", "list_directory"] {
            let json = format!(
                r#"{{"hook_event_name": "BeforeTool", "tool_name": "{}", "tool_input": {{"path": "/etc/passwd"}}}}"#,
                tool
            );
            let payload: GeminiPayload = serde_json::from_str(&json).unwrap();
            assert!(!is_shell_tool(payload.tool_name.as_deref().unwrap()),
                "tool '{}' must not be treated as shell tool", tool);
        }
    }

    // --- Event filtering ---

    #[test]
    fn test_non_before_tool_events_ignored() {
        for event in ["AfterTool", "BeforeAgent", "AfterAgent", "SessionStart"] {
            let json = format!(
                r#"{{"hook_event_name": "{}", "tool_name": "run_shell_command", "tool_input": {{"command": "rm -rf /"}}}}"#,
                event
            );
            let payload: GeminiPayload = serde_json::from_str(&json).unwrap();
            assert_ne!(payload.hook_event_name.as_deref(), Some("BeforeTool"));
        }
    }

    // --- Rewrite preserves other tool_input fields ---

    #[test]
    fn test_rewrite_preserves_other_tool_input_fields() {
        // If tool_input has { "command": "git status", "timeout": 30 },
        // after rewrite it should be { "command": "rtk run -c '...'", "timeout": 30 }
        let original_input = serde_json::json!({
            "command": "git status",
            "timeout": 30,
            "cwd": "/project"
        });

        let mut new_input = original_input.clone();
        if let Some(obj) = new_input.as_object_mut() {
            obj.insert("command".into(), Value::String("rtk run -c 'git status'".into()));
        }

        assert_eq!(new_input["timeout"], 30);
        assert_eq!(new_input["cwd"], "/project");
        assert_eq!(new_input["command"], "rtk run -c 'git status'");
    }

    // --- Edge cases ---

    #[test]
    fn test_missing_tool_input() {
        let json = r#"{"hook_event_name": "BeforeTool", "tool_name": "run_shell_command"}"#;
        let payload: GeminiPayload = serde_json::from_str(json).unwrap();
        assert!(payload.tool_input.is_none());
    }

    #[test]
    fn test_missing_command_in_tool_input() {
        let json = r#"{"hook_event_name": "BeforeTool", "tool_name": "run_shell_command", "tool_input": {"cwd": "/tmp"}}"#;
        let payload: GeminiPayload = serde_json::from_str(json).unwrap();
        let input = payload.tool_input.unwrap();
        assert!(input.get("command").is_none());
    }

    #[test]
    fn test_malformed_json_does_not_panic() {
        let bad_inputs = [
            "",
            "not json",
            "{}",
            r#"{"hook_event_name": 42}"#,
            "null",
        ];
        for input in bad_inputs {
            // Should not panic, just return Err or deserialize to defaults
            let _ = serde_json::from_str::<GeminiPayload>(input);
        }
    }
}
