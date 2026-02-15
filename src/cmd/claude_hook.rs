//! Claude Code PreToolUse hook protocol handler.
//!
//! Reads JSON from stdin, applies safety checks and rewrites,
//! outputs JSON to stdout.
//!
//! Protocol: https://docs.anthropic.com/en/docs/claude-code/hooks
//!
//! Exit codes:
//!   0 = success (allow or rewrite) — command proceeds
//!   2 = blocking error (deny) — command rejected
//!
//! Fail-open: Any parse error or unexpected input → exit 0, no output.
//! Claude Code treats no-output-exit-0 as "no opinion" and proceeds.

use super::hook::{check_for_hook, HookResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read};

// --- Wire format structs (field names must match Claude Code spec exactly) ---

#[derive(Deserialize)]
pub(crate) struct ClaudePayload {
    tool_input: Option<Value>,
    // Claude Code also sends: tool_name, session_id, session_cwd,
    // transcript_path — serde silently ignores unknown fields.
    // The settings.json matcher already filters to Bash-only events.
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClaudeResponse {
    hook_specific_output: HookOutput,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HookOutput {
    hook_event_name: &'static str,
    permission_decision: &'static str,
    permission_decision_reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_input: Option<Value>,
}

// --- Guard logic (extracted for testability) ---

/// Extract the command string from a parsed payload.
/// Returns None if payload has no tool_input or no command field.
pub(crate) fn extract_command(payload: &ClaudePayload) -> Option<&str> {
    payload
        .tool_input
        .as_ref()?
        .get("command")?
        .as_str()
        .filter(|s| !s.is_empty())
}

/// Check if this command should bypass hook processing entirely.
/// Returns true if the command should be passed through without rewriting.
pub(crate) fn should_passthrough(cmd: &str) -> bool {
    // Already routed through rtk
    cmd.starts_with("rtk ") || cmd.contains("/rtk ")
    // Heredocs need shell, not rtk
    || cmd.contains("<<")
}

/// Check if hook processing is disabled by environment.
pub(crate) fn is_disabled() -> bool {
    std::env::var("RTK_HOOK_ENABLED").as_deref() == Ok("0") || std::env::var("RTK_ACTIVE").is_ok()
}

/// Build a ClaudeResponse for an allowed/rewritten command.
pub(crate) fn allow_response(reason: String, updated_input: Option<Value>) -> ClaudeResponse {
    ClaudeResponse {
        hook_specific_output: HookOutput {
            hook_event_name: "PreToolUse",
            permission_decision: "allow",
            permission_decision_reason: reason,
            updated_input,
        },
    }
}

/// Build a ClaudeResponse for a blocked command.
pub(crate) fn deny_response(reason: String) -> ClaudeResponse {
    ClaudeResponse {
        hook_specific_output: HookOutput {
            hook_event_name: "PreToolUse",
            permission_decision: "deny",
            permission_decision_reason: reason,
            updated_input: None,
        },
    }
}

// --- Entry point ---

/// Run the Claude Code hook handler.
///
/// Reads JSON from stdin, processes safety checks via shared
/// `check_for_hook()`, outputs JSON to stdout.
///
/// Fail-open design: malformed input → exit 0, no output.
/// Claude Code interprets this as "no opinion" and proceeds normally.
pub fn run() -> anyhow::Result<()> {
    // Fail-open: wrap entire handler so ANY panic/error → exit 0 (no opinion).
    // Claude Code treats no-output-exit-0 as "hook has no opinion, proceed."
    match run_inner() {
        Ok(exit_code) => {
            if exit_code != 0 {
                std::process::exit(exit_code);
            }
        }
        Err(_) => {} // Fail-open: swallow errors, exit 0
    }
    Ok(())
}

/// Inner handler returns exit code (0 = allow, 2 = block).
/// Separated from run() so errors propagate to the fail-open wrapper.
fn run_inner() -> anyhow::Result<i32> {
    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;

    let payload: ClaudePayload = match serde_json::from_str(&buffer) {
        Ok(p) => p,
        Err(_) => return Ok(0), // Fail-open: bad JSON → no opinion
    };

    let cmd = match extract_command(&payload) {
        Some(c) => c,
        None => return Ok(0), // No command → no opinion
    };

    if is_disabled() || should_passthrough(cmd) {
        return Ok(0);
    }

    // Shared safety/rewrite logic (same function gemini_hook.rs uses)
    let result = check_for_hook(cmd, "claude");

    match result {
        HookResult::Rewrite(new_cmd) => {
            // Preserve all original tool_input fields, only replace "command"
            let mut updated = payload
                .tool_input
                .unwrap_or_else(|| Value::Object(Default::default()));
            if let Some(obj) = updated.as_object_mut() {
                obj.insert("command".into(), Value::String(new_cmd));
            }

            let response = allow_response("RTK safety rewrite applied".into(), Some(updated));
            println!("{}", serde_json::to_string(&response)?);
            Ok(0)
        }
        HookResult::Blocked(msg) => {
            let response = deny_response(msg);
            println!("{}", serde_json::to_string(&response)?);
            Ok(2) // Exit 2 = blocking error per Claude Code spec
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // CLAUDE CODE WIRE FORMAT CONFORMANCE
    // https://docs.anthropic.com/en/docs/claude-code/hooks
    //
    // These tests verify exact JSON field names per the Claude Code spec.
    // A wrong field name means Claude Code silently ignores the response.
    // =========================================================================

    // --- Output: field name conformance ---

    #[test]
    fn test_output_uses_hook_specific_output() {
        // Claude expects "hookSpecificOutput" (camelCase), NOT "hook_specific_output"
        let response = allow_response("test".into(), None);
        let json = serde_json::to_string(&response).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert!(
            parsed.get("hookSpecificOutput").is_some(),
            "must have 'hookSpecificOutput' field"
        );
        assert!(
            parsed.get("hook_specific_output").is_none(),
            "must NOT have snake_case field"
        );
    }

    #[test]
    fn test_output_uses_permission_decision() {
        // Claude expects "permissionDecision", NOT "decision"
        let response = allow_response("test".into(), None);
        let json = serde_json::to_string(&response).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();
        let output = &parsed["hookSpecificOutput"];

        assert!(
            output.get("permissionDecision").is_some(),
            "must have 'permissionDecision' field"
        );
        assert!(
            output.get("decision").is_none(),
            "must NOT have Gemini-style 'decision' field"
        );
    }

    #[test]
    fn test_output_uses_permission_decision_reason() {
        let response = deny_response("blocked".into());
        let json = serde_json::to_string(&response).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();
        let output = &parsed["hookSpecificOutput"];

        assert!(
            output.get("permissionDecisionReason").is_some(),
            "must have 'permissionDecisionReason'"
        );
    }

    #[test]
    fn test_output_uses_hook_event_name() {
        let response = allow_response("test".into(), None);
        let json = serde_json::to_string(&response).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    }

    #[test]
    fn test_output_uses_updated_input_for_rewrite() {
        let input = serde_json::json!({"command": "rtk run -c 'git status'"});
        let response = allow_response("rewrite".into(), Some(input));
        let json = serde_json::to_string(&response).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert!(
            parsed["hookSpecificOutput"].get("updatedInput").is_some(),
            "must have 'updatedInput' for rewrites"
        );
    }

    #[test]
    fn test_allow_omits_updated_input_when_none() {
        let response = allow_response("passthrough".into(), None);
        let json = serde_json::to_string(&response).unwrap();

        assert!(
            !json.contains("updatedInput"),
            "updatedInput must be omitted when None"
        );
    }

    #[test]
    fn test_rewrite_preserves_other_tool_input_fields() {
        let original = serde_json::json!({
            "command": "git status",
            "timeout": 30,
            "description": "check repo"
        });

        let mut updated = original.clone();
        if let Some(obj) = updated.as_object_mut() {
            obj.insert(
                "command".into(),
                Value::String("rtk run -c 'git status'".into()),
            );
        }

        assert_eq!(updated["timeout"], 30);
        assert_eq!(updated["description"], "check repo");
        assert_eq!(updated["command"], "rtk run -c 'git status'");
    }

    #[test]
    fn test_output_decision_values() {
        let allow = allow_response("test".into(), None);
        let deny = deny_response("blocked".into());

        let allow_json: Value =
            serde_json::from_str(&serde_json::to_string(&allow).unwrap()).unwrap();
        let deny_json: Value =
            serde_json::from_str(&serde_json::to_string(&deny).unwrap()).unwrap();

        assert_eq!(
            allow_json["hookSpecificOutput"]["permissionDecision"],
            "allow"
        );
        assert_eq!(
            deny_json["hookSpecificOutput"]["permissionDecision"],
            "deny"
        );
    }

    // --- Input: payload parsing ---

    #[test]
    fn test_input_extra_fields_ignored() {
        // Claude sends session_id, tool_name, transcript_path, etc.
        let json = r#"{"tool_input": {"command": "ls"}, "tool_name": "Bash", "session_id": "abc-123", "session_cwd": "/tmp", "transcript_path": "/path/to/transcript.jsonl"}"#;
        let payload: ClaudePayload = serde_json::from_str(json).unwrap();
        assert_eq!(extract_command(&payload), Some("ls"));
    }

    #[test]
    fn test_input_tool_input_is_object() {
        let json = r#"{"tool_input": {"command": "git status", "timeout": 30}}"#;
        let payload: ClaudePayload = serde_json::from_str(json).unwrap();
        let input = payload.tool_input.unwrap();
        assert_eq!(input["command"].as_str().unwrap(), "git status");
        assert_eq!(input["timeout"].as_i64().unwrap(), 30);
    }

    // --- Guard function tests ---

    #[test]
    fn test_extract_command_basic() {
        let payload: ClaudePayload =
            serde_json::from_str(r#"{"tool_input": {"command": "git status"}}"#).unwrap();
        assert_eq!(extract_command(&payload), Some("git status"));
    }

    #[test]
    fn test_extract_command_missing_tool_input() {
        let payload: ClaudePayload = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(extract_command(&payload), None);
    }

    #[test]
    fn test_extract_command_missing_command_field() {
        let payload: ClaudePayload =
            serde_json::from_str(r#"{"tool_input": {"cwd": "/tmp"}}"#).unwrap();
        assert_eq!(extract_command(&payload), None);
    }

    #[test]
    fn test_extract_command_empty_string() {
        let payload: ClaudePayload =
            serde_json::from_str(r#"{"tool_input": {"command": ""}}"#).unwrap();
        assert_eq!(extract_command(&payload), None);
    }

    #[test]
    fn test_should_passthrough_rtk_prefix() {
        assert!(should_passthrough("rtk run -c 'ls'"));
        assert!(should_passthrough("rtk cargo test"));
        assert!(should_passthrough("/usr/local/bin/rtk run -c 'ls'"));
    }

    #[test]
    fn test_should_passthrough_heredoc() {
        assert!(should_passthrough("cat <<EOF\nhello\nEOF"));
        assert!(should_passthrough("cat <<'EOF'\nhello\nEOF"));
    }

    #[test]
    fn test_should_passthrough_normal_commands() {
        assert!(!should_passthrough("git status"));
        assert!(!should_passthrough("ls -la"));
        assert!(!should_passthrough("echo hello"));
    }

    #[test]
    fn test_malformed_json_does_not_panic() {
        let bad_inputs = ["", "not json", "{}", r#"{"tool_input": 42}"#, "null"];
        for input in bad_inputs {
            let _ = serde_json::from_str::<ClaudePayload>(input);
        }
    }

    // --- Fail-open behavior ---

    #[test]
    fn test_run_inner_returns_zero_for_empty_payload() {
        // Simulates what happens when run_inner processes "{}" —
        // no tool_input means no command, should return exit 0
        let payload: ClaudePayload = serde_json::from_str("{}").unwrap();
        assert_eq!(extract_command(&payload), None);
        // run_inner() would return Ok(0) here
    }

    #[test]
    fn test_is_disabled_hook_enabled_zero() {
        std::env::set_var("RTK_HOOK_ENABLED", "0");
        assert!(is_disabled());
        std::env::remove_var("RTK_HOOK_ENABLED");
    }

    #[test]
    fn test_is_disabled_rtk_active() {
        std::env::set_var("RTK_ACTIVE", "1");
        assert!(is_disabled());
        std::env::remove_var("RTK_ACTIVE");
    }

    // --- Integration: safety decisions ---

    #[test]
    fn test_safe_command_produces_allow_with_rewrite() {
        let payload: ClaudePayload =
            serde_json::from_str(r#"{"tool_input": {"command": "git status"}}"#).unwrap();
        let cmd = extract_command(&payload).unwrap();
        let result = check_for_hook(cmd, "claude");

        match result {
            HookResult::Rewrite(new_cmd) => {
                assert!(
                    new_cmd.contains("rtk run"),
                    "safe command should be rewritten to use rtk run"
                );
            }
            HookResult::Blocked(_) => panic!("git status should not be blocked"),
        }
    }

    #[test]
    fn test_blocked_command_produces_deny() {
        let payload: ClaudePayload =
            serde_json::from_str(r#"{"tool_input": {"command": "cat /etc/passwd"}}"#).unwrap();
        let cmd = extract_command(&payload).unwrap();
        let result = check_for_hook(cmd, "claude");

        assert!(
            matches!(result, HookResult::Blocked(_)),
            "cat should be blocked by safety rules"
        );
    }

    #[test]
    fn test_cross_protocol_same_decision() {
        // Same command must produce same allow/block decision
        // regardless of whether it comes through Claude or Gemini protocol
        for cmd in ["git status", "ls -la", "cat file.txt"] {
            let claude = check_for_hook(cmd, "claude");
            let gemini = check_for_hook(cmd, "gemini");

            let claude_blocked = matches!(claude, HookResult::Blocked(_));
            let gemini_blocked = matches!(gemini, HookResult::Blocked(_));

            assert_eq!(
                claude_blocked, gemini_blocked,
                "command '{}': Claude blocked={} but Gemini blocked={}",
                cmd, claude_blocked, gemini_blocked
            );
        }
    }
}
