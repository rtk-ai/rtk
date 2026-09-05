//! Native Codex hook protocol and safety policy.
//!
//! Codex runs `PreToolUse` hooks before it evaluates the command's execution
//! approval.  RTK therefore only changes input when Codex has explicitly
//! selected its no-approval mode.  In the normal approval mode we cannot see
//! Codex's per-command decision, so returning a rewritten command could turn a
//! denied command into a different command that no longer matches the user's
//! approval rule.

use crate::core::config::hook_rewrite_params;
use crate::discover::lexer::contains_unattestable_construct;
use crate::discover::registry::{has_heredoc, rewrite_command};
use serde_json::{json, Value};

pub const PRE_TOOL_USE_EVENT: &str = "PreToolUse";
pub const SUBAGENT_START_EVENT: &str = "subagent-start";
pub const BYPASS_PERMISSIONS_MODE: &str = "bypassPermissions";

/// Parse one Codex hook event and return a protocol response when RTK can
/// safely rewrite it.  A malformed event is a no-op to keep the host fail-open.
pub fn response_from_input(input: &str) -> serde_json::Result<Option<Value>> {
    let value = serde_json::from_str::<Value>(input)?;
    Ok(response_from_value(&value))
}

/// Build a Codex `PreToolUse` response from a parsed event.
pub fn response_from_value(value: &Value) -> Option<Value> {
    if value.get("hook_event_name").and_then(Value::as_str) != Some(PRE_TOOL_USE_EVENT) {
        return None;
    }
    if value.get("tool_name").and_then(Value::as_str) != Some("Bash") {
        return None;
    }
    if value.get("permission_mode").and_then(Value::as_str) != Some(BYPASS_PERMISSIONS_MODE) {
        return None;
    }

    let command = value
        .pointer("/tool_input/command")
        .and_then(Value::as_str)?;
    let rewritten = safe_rewrite(command)?;

    let mut updated_input = value.get("tool_input")?.clone();
    let input_object = updated_input.as_object_mut()?;
    input_object.insert("command".to_string(), Value::String(rewritten));

    Some(json!({
        "hookSpecificOutput": {
            "hookEventName": PRE_TOOL_USE_EVENT,
            "updatedInput": updated_input
        }
    }))
}

/// Return an RTK rewrite only for shell expressions whose execution contract
/// remains intact.  Redirects, substitutions, heredocs, and similar forms
/// must be executed exactly as authored by the user.
pub fn safe_rewrite(command: &str) -> Option<String> {
    if command.is_empty() || has_heredoc(command) || contains_unattestable_construct(command) {
        return None;
    }

    let (excluded, transparent_prefixes) = hook_rewrite_params();
    let rewritten = rewrite_command(command, &excluded, &transparent_prefixes)?;
    (rewritten != command).then_some(rewritten)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(command: &str, permission_mode: &str) -> Value {
        json!({
            "hook_event_name": PRE_TOOL_USE_EVENT,
            "permission_mode": permission_mode,
            "tool_name": "Bash",
            "tool_input": {
                "command": command,
                "description": "keep me"
            }
        })
    }

    #[test]
    fn rewrites_only_in_explicit_bypass_mode_and_preserves_input() {
        let output = response_from_value(&event("git status", BYPASS_PERMISSIONS_MODE))
            .expect("rewrite expected");
        assert_eq!(
            output["hookSpecificOutput"]["updatedInput"]["command"],
            "rtk git status"
        );
        assert_eq!(
            output["hookSpecificOutput"]["updatedInput"]["description"],
            "keep me"
        );
        assert!(output["hookSpecificOutput"]
            .get("permissionDecision")
            .is_none());
    }

    #[test]
    fn approval_mode_and_missing_mode_are_noops() {
        assert!(response_from_value(&event("git status", "default")).is_none());
        let mut missing = event("git status", BYPASS_PERMISSIONS_MODE);
        missing.as_object_mut().unwrap().remove("permission_mode");
        assert!(response_from_value(&missing).is_none());
    }

    #[test]
    fn unsupported_tools_and_shell_forms_are_noops() {
        let mut non_bash = event("git status", BYPASS_PERMISSIONS_MODE);
        non_bash["tool_name"] = json!("apply_patch");
        assert!(response_from_value(&non_bash).is_none());

        for command in [
            "git status > result.txt",
            "git status $(printf unexpected)",
            "cat <<'EOF'\ngit status\nEOF",
        ] {
            assert!(
                response_from_value(&event(command, BYPASS_PERMISSIONS_MODE)).is_none(),
                "unsupported shell form must remain native: {command}"
            );
        }
    }
}
