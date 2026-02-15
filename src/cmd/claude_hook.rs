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
//! Deny output strategy (dual-path for robustness):
//!   stdout: JSON with permissionDecision "deny" (documented main path)
//!   stderr: plain text reason (workaround for Claude Code bug #4669,
//!           where permissionDecision "deny" on stdout was ignored;
//!           exit code 2 causes Claude Code to read stderr as error)
//!
//! I/O enforcement: `run_inner()` returns `HookResponse` (no I/O).
//! Only `run()` writes to stdout/stderr via `write!`/`writeln!`.
//! The `#![deny(clippy::print_stdout, clippy::print_stderr)]` on this
//! module catches any accidental `println!`/`eprintln!` at compile time.
//!
//! Fail-open: Any parse error or unexpected input → exit 0, no output.
//! Claude Code treats no-output-exit-0 as "no opinion" and proceeds.

// Compile-time enforcement: no accidental println!/eprintln! in this module.
// All stdout/stderr output is done via write!/writeln! in run() only.
// clippy::print_stdout/print_stderr catch println!/eprintln! but NOT write!.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use super::hook::{check_for_hook, is_hook_disabled, should_passthrough, HookResponse, HookResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read, Write};

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

// Guard functions `is_hook_disabled()` and `should_passthrough()` are shared
// with gemini_hook.rs via hook.rs to avoid duplication (DRY).

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
/// This is the ONLY function that performs I/O (stdout/stderr).
/// `run_inner()` returns a `HookResponse` enum — pure logic, no I/O.
/// Combined with `#![deny(clippy::print_stdout, clippy::print_stderr)]`,
/// this ensures no stray output corrupts the JSON hook protocol.
///
/// Fail-open design: malformed input → exit 0, no output.
/// Claude Code interprets this as "no opinion" and proceeds normally.
pub fn run() -> anyhow::Result<()> {
    // Fail-open: wrap entire handler so ANY error → exit 0 (no opinion).
    let response = match run_inner() {
        Ok(r) => r,
        Err(_) => HookResponse::NoOpinion, // Fail-open: swallow errors
    };

    // Single I/O point: write!/writeln! are not caught by the clippy lint,
    // so this is the only place that can produce output in this module.
    match response {
        HookResponse::NoOpinion => {} // Exit 0, no output
        HookResponse::Allow(json) => {
            writeln!(io::stdout(), "{json}")?;
        }
        HookResponse::Deny(json, reason) => {
            // Dual-path deny for bug #4669 robustness:
            // stdout: JSON with permissionDecision "deny" (documented main path)
            // stderr: plain text reason (exit code 2 fallback path)
            writeln!(io::stdout(), "{json}")?;
            writeln!(io::stderr(), "{reason}")?;
            std::process::exit(2);
        }
    }
    Ok(())
}

/// Inner handler: pure decision logic, no I/O.
/// Returns `HookResponse` for `run()` to output.
fn run_inner() -> anyhow::Result<HookResponse> {
    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;

    let payload: ClaudePayload = match serde_json::from_str(&buffer) {
        Ok(p) => p,
        Err(_) => return Ok(HookResponse::NoOpinion),
    };

    let cmd = match extract_command(&payload) {
        Some(c) => c,
        None => return Ok(HookResponse::NoOpinion),
    };

    if is_hook_disabled() || should_passthrough(cmd) {
        return Ok(HookResponse::NoOpinion);
    }

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
            let json = serde_json::to_string(&response)?;
            Ok(HookResponse::Allow(json))
        }
        HookResult::Blocked(msg) => {
            let response = deny_response(msg.clone());
            let json = serde_json::to_string(&response)?;
            Ok(HookResponse::Deny(json, msg))
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
    fn test_shared_should_passthrough_rtk_prefix() {
        assert!(should_passthrough("rtk run -c 'ls'"));
        assert!(should_passthrough("rtk cargo test"));
        assert!(should_passthrough("/usr/local/bin/rtk run -c 'ls'"));
    }

    #[test]
    fn test_shared_should_passthrough_heredoc() {
        assert!(should_passthrough("cat <<EOF\nhello\nEOF"));
        assert!(should_passthrough("cat <<'EOF'\nhello\nEOF"));
    }

    #[test]
    fn test_shared_should_passthrough_normal_commands() {
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
    fn test_run_inner_returns_no_opinion_for_empty_payload() {
        // "{}" has no tool_input → no command → NoOpinion
        let payload: ClaudePayload = serde_json::from_str("{}").unwrap();
        assert_eq!(extract_command(&payload), None);
    }

    #[test]
    fn test_shared_is_hook_disabled_hook_enabled_zero() {
        std::env::set_var("RTK_HOOK_ENABLED", "0");
        assert!(is_hook_disabled());
        std::env::remove_var("RTK_HOOK_ENABLED");
    }

    #[test]
    fn test_shared_is_hook_disabled_rtk_active() {
        std::env::set_var("RTK_ACTIVE", "1");
        assert!(is_hook_disabled());
        std::env::remove_var("RTK_ACTIVE");
    }

    // --- Integration: Bug #4669 workaround verification ---

    #[test]
    fn test_deny_response_includes_reason_for_stderr() {
        // Bug #4669 workaround: deny must provide plain text reason
        // that can be output to stderr alongside the JSON stdout.
        // The msg is cloned for both paths in run_inner().
        let msg = "RTK: cat is blocked (use rtk read instead)";
        let response = deny_response(msg.to_string());
        let json = serde_json::to_string(&response).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        // JSON stdout path
        assert_eq!(parsed["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(
            parsed["hookSpecificOutput"]["permissionDecisionReason"],
            msg
        );
        // The same msg string is used for stderr in run() via HookResponse::Deny
    }

    // Note: Integration tests for check_for_hook() safety decisions are in
    // src/cmd/hook.rs (test_safe_commands_rewrite, test_blocked_commands, etc.)
    // to avoid duplication. This module focuses on Claude Code wire format.
}
