#![deny(clippy::print_stdout, clippy::print_stderr)]

use super::{
    check_for_hook, is_hook_disabled, should_passthrough, update_command_in_tool_input,
    HookResponse, HookResult,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read, Write};

#[derive(Deserialize)]
pub(crate) struct ClaudePayload {
    tool_input: Option<Value>,
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

#[derive(Deserialize)]
struct ManifestFallthroughEntry {
    fallthrough_command: String,
}

#[derive(Deserialize)]
struct ManifestFallthrough {
    entries: Vec<ManifestFallthroughEntry>,
}

pub(crate) fn extract_command(payload: &ClaudePayload) -> Option<&str> {
    payload
        .tool_input
        .as_ref()?
        .get("command")?
        .as_str()
        .filter(|s| !s.is_empty())
}

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

pub fn run() -> anyhow::Result<()> {
    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;

    let response = match run_inner(&buffer) {
        Ok(r) => r,
        Err(_) => HookResponse::NoOpinion,
    };

    match response {
        HookResponse::NoOpinion => match run_manifest_handlers(&buffer) {
            ManifestResult::Blocked { json, stderr_bytes } => {
                writeln!(io::stdout(), "{json}")?;
                io::stderr().write_all(&stderr_bytes)?;
                if stderr_bytes.is_empty() {
                    writeln!(io::stderr(), "Command blocked by registered handler")?;
                }
                std::process::exit(2);
            }
            ManifestResult::NoBlock => {}
        },
        HookResponse::Allow(rtk_json) => match run_manifest_handlers(&buffer) {
            ManifestResult::Blocked {
                json: handler_json,
                stderr_bytes,
            } => {
                writeln!(io::stdout(), "{handler_json}")?;
                io::stderr().write_all(&stderr_bytes)?;
                if stderr_bytes.is_empty() {
                    let reason = extract_deny_reason(&handler_json).unwrap_or_else(|| {
                        "Command blocked by registered safety handler".to_owned()
                    });
                    writeln!(io::stderr(), "{reason}")?;
                }
                std::process::exit(2);
            }
            ManifestResult::NoBlock => {
                writeln!(io::stdout(), "{rtk_json}")?;
            }
        },
        HookResponse::Deny(json, reason) => {
            // ISSUE #4669: dual-path deny workaround — stdout JSON + stderr reason + exit 2
            writeln!(io::stdout(), "{json}")?;
            writeln!(io::stderr(), "{reason}")?;
            std::process::exit(2);
        }
    }
    Ok(())
}

fn run_inner(buffer: &str) -> anyhow::Result<HookResponse> {
    let payload: ClaudePayload = match serde_json::from_str(buffer) {
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
            let updated = update_command_in_tool_input(payload.tool_input, new_cmd);

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

fn manifest_path() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    Some(
        std::path::Path::new(&home)
            .join(".claude")
            .join("hooks")
            .join("rtk-bash-manifest.json"),
    )
}

fn is_json_deny(json_str: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(json_str.trim()) else {
        return false;
    };
    let cc_deny = v
        .get("hookSpecificOutput")
        .and_then(|o| o.get("permissionDecision"))
        .and_then(|d| d.as_str())
        == Some("deny");
    let gemini_deny = v.get("decision").and_then(|d| d.as_str()) == Some("deny");
    cc_deny || gemini_deny
}

fn extract_deny_reason(json_str: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json_str.trim()).ok()?;
    if let Some(r) = v
        .get("hookSpecificOutput")
        .and_then(|o| o.get("permissionDecisionReason"))
        .and_then(|r| r.as_str())
    {
        return Some(r.to_owned());
    }
    v.get("reason").and_then(|r| r.as_str()).map(str::to_owned)
}

enum ManifestResult {
    Blocked { json: String, stderr_bytes: Vec<u8> },
    NoBlock,
}

fn load_manifest() -> Option<ManifestFallthrough> {
    let path = manifest_path()?;
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

fn run_manifest_handlers(payload: &str) -> ManifestResult {
    let manifest = match load_manifest() {
        Some(m) => m,
        None => return ManifestResult::NoBlock,
    };

    let mut block_json: Option<String> = None;
    let mut block_stderr: Vec<u8> = Vec::new();

    for entry in &manifest.entries {
        let mut child = match std::process::Command::new("sh")
            .arg("-c")
            .arg(&entry.fallthrough_command)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => continue,
        };

        // Track write success to avoid false-positive exit 2 on partial stdin
        let write_ok = if let Some(mut stdin) = child.stdin.take() {
            io::Write::write_all(&mut stdin, payload.as_bytes()).is_ok()
        } else {
            false
        };

        let output = match child.wait_with_output() {
            Ok(o) => o,
            Err(_) => continue,
        };

        let exit_code = output.status.code().unwrap_or(0);
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        let blocked = (exit_code == 2 && write_ok) || is_json_deny(&stdout_str);

        if blocked && block_json.is_none() {
            block_json = Some(stdout_str.into_owned());
            block_stderr.extend_from_slice(&output.stderr);
        }
    }

    match block_json {
        Some(json) => ManifestResult::Blocked {
            json,
            stderr_bytes: block_stderr,
        },
        None => ManifestResult::NoBlock,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::test_helpers::EnvGuard;

    #[test]
    fn test_output_uses_hook_specific_output() {
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

    #[test]
    fn test_input_extra_fields_ignored() {
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

    #[test]
    fn test_run_inner_returns_no_opinion_for_empty_payload() {
        let payload: ClaudePayload = serde_json::from_str("{}").unwrap();
        assert_eq!(extract_command(&payload), None);
    }

    #[test]
    fn test_shared_is_hook_disabled_hook_enabled_zero() {
        let _env = EnvGuard::new();
        std::env::set_var("RTK_HOOK_ENABLED", "0");
        assert!(is_hook_disabled());
    }

    #[test]
    fn test_shared_is_hook_disabled_rtk_active() {
        let _env = EnvGuard::new();
        std::env::set_var("RTK_ACTIVE", "1");
        assert!(is_hook_disabled());
    }

    #[test]
    fn test_deny_response_includes_reason_for_stderr() {
        // ISSUE #4669: deny must provide plain text reason for stderr dual-path workaround
        let msg = "RTK: cat is blocked (use rtk read instead)";
        let response = deny_response(msg.to_string());
        let json = serde_json::to_string(&response).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(
            parsed["hookSpecificOutput"]["permissionDecisionReason"],
            msg
        );
    }

    #[test]
    fn test_is_json_deny_claude_code_format() {
        let json = r#"{"hookSpecificOutput":{"permissionDecision":"deny","permissionDecisionReason":"blocked"}}"#;
        assert!(is_json_deny(json));
    }

    #[test]
    fn test_is_json_deny_gemini_format() {
        let json = r#"{"decision":"deny","reason":"blocked"}"#;
        assert!(is_json_deny(json));
    }

    #[test]
    fn test_is_json_deny_allow_not_matched() {
        assert!(!is_json_deny(
            r#"{"hookSpecificOutput":{"permissionDecision":"allow"}}"#
        ));
        assert!(!is_json_deny(r#"{"decision":"allow"}"#));
        assert!(!is_json_deny(""));
        assert!(!is_json_deny("not json"));
    }

    #[test]
    fn test_extract_deny_reason_cc_format() {
        let json = r#"{"hookSpecificOutput":{"permissionDecision":"deny","permissionDecisionReason":"Use Grep tool"}}"#;
        assert_eq!(extract_deny_reason(json), Some("Use Grep tool".to_owned()));
    }

    #[test]
    fn test_extract_deny_reason_gemini_format() {
        let json = r#"{"decision":"deny","reason":"command blocked"}"#;
        assert_eq!(
            extract_deny_reason(json),
            Some("command blocked".to_owned())
        );
    }

    #[test]
    fn test_extract_deny_reason_missing() {
        assert_eq!(extract_deny_reason("{}"), None);
        assert_eq!(extract_deny_reason("not json"), None);
    }

    #[test]
    fn test_load_manifest_returns_none_when_missing() {
        let result = load_manifest();
        drop(result);
    }
}
