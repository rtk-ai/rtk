//! Processes incoming hook calls from AI agents and rewrites commands on the fly.

use super::constants::PRE_TOOL_USE_KEY;
use crate::hooks::permissions::PermissionVerdict;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::io::{self, Read};

use crate::discover::registry::rewrite_command;

// ── Copilot hook (VS Code + Copilot CLI) ──────────────────────

/// Format detected from the preToolUse JSON input.
enum HookFormat {
    /// VS Code Copilot Chat / Claude Code: `tool_name` + `tool_input.command`, supports `updatedInput`.
    VsCode { command: String },
    /// GitHub Copilot CLI: camelCase `toolName` + `toolArgs` (JSON string), deny-with-suggestion only.
    CopilotCli { command: String },
    /// Non-bash tool, already uses rtk, or unknown format — pass through silently.
    PassThrough,
}

/// Run the Copilot preToolUse hook.
/// Auto-detects VS Code Copilot Chat vs Copilot CLI format.
pub fn run_copilot() -> Result<()> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .context("Failed to read stdin")?;

    let input = input.trim();
    if input.is_empty() {
        return Ok(());
    }

    let v: Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[rtk hook] Failed to parse JSON input: {e}");
            return Ok(());
        }
    };

    match detect_format(&v) {
        HookFormat::VsCode { command } => handle_vscode(&command),
        HookFormat::CopilotCli { command } => handle_copilot_cli(&command),
        HookFormat::PassThrough => Ok(()),
    }
}

fn detect_format(v: &Value) -> HookFormat {
    // VS Code Copilot Chat / Claude Code: snake_case keys
    if let Some(tool_name) = v.get("tool_name").and_then(|t| t.as_str()) {
        if matches!(tool_name, "runTerminalCommand" | "Bash" | "bash") {
            if let Some(cmd) = v
                .pointer("/tool_input/command")
                .and_then(|c| c.as_str())
                .filter(|c| !c.is_empty())
            {
                return HookFormat::VsCode {
                    command: cmd.to_string(),
                };
            }
        }
        return HookFormat::PassThrough;
    }

    // Copilot CLI: camelCase keys, toolArgs is a JSON-encoded string
    if let Some(tool_name) = v.get("toolName").and_then(|t| t.as_str()) {
        if tool_name == "bash" {
            if let Some(tool_args_str) = v.get("toolArgs").and_then(|t| t.as_str()) {
                if let Ok(tool_args) = serde_json::from_str::<Value>(tool_args_str) {
                    if let Some(cmd) = tool_args
                        .get("command")
                        .and_then(|c| c.as_str())
                        .filter(|c| !c.is_empty())
                    {
                        return HookFormat::CopilotCli {
                            command: cmd.to_string(),
                        };
                    }
                }
            }
        }
        return HookFormat::PassThrough;
    }

    HookFormat::PassThrough
}

fn get_rewritten(cmd: &str) -> Option<String> {
    if cmd.contains("<<") {
        return None;
    }

    let excluded = crate::core::config::Config::load()
        .map(|c| c.hooks.exclude_commands)
        .unwrap_or_default();

    let rewritten = rewrite_command(cmd, &excluded)?;

    if rewritten == cmd {
        return None;
    }

    Some(rewritten)
}

fn handle_vscode(cmd: &str) -> Result<()> {
    let rewritten = match get_rewritten(cmd) {
        Some(r) => r,
        None => return Ok(()),
    };

    let output = json!({
        "hookSpecificOutput": {
            "hookEventName": PRE_TOOL_USE_KEY,
            "permissionDecision": "allow",
            "permissionDecisionReason": "RTK auto-rewrite",
            "updatedInput": { "command": rewritten }
        }
    });
    println!("{output}");
    Ok(())
}

fn handle_copilot_cli(cmd: &str) -> Result<()> {
    let rewritten = match get_rewritten(cmd) {
        Some(r) => r,
        None => return Ok(()),
    };

    let output = json!({
        "permissionDecision": "deny",
        "permissionDecisionReason": format!(
            "Token savings: use `{}` instead (rtk saves 60-90% tokens)",
            rewritten
        )
    });
    println!("{output}");
    Ok(())
}

/// Run the Claude Code PreToolUse hook (native, no bash/jq required).
/// Reads JSON from stdin, rewrites shell commands to rtk equivalents,
/// outputs JSON to stdout in Claude Code hook format.
///
/// Unlike `run_copilot()`, this function:
/// - Only handles Claude Code's snake_case format (not Copilot CLI camelCase)
/// - Respects deny/ask permission rules before rewriting
///
/// Permission protocol (mirrors rewrite_cmd.rs exit codes):
/// - Deny rule matched → silent passthrough, Claude Code enforces natively
/// - Ask/Default rule → emit `updatedInput` but omit `permissionDecision` so Claude Code prompts
/// - Allow rule matched → emit `updatedInput` with `permissionDecision: "allow"`
pub fn run_claude() -> Result<()> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .context("Failed to read stdin")?;

    let input = input.trim();
    if input.is_empty() {
        return Ok(());
    }

    let v: Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[rtk hook] Failed to parse JSON input: {e}");
            return Ok(());
        }
    };

    // Claude Code uses snake_case keys only. Reject camelCase Copilot CLI format
    // to avoid emitting a deny response Claude Code wouldn't understand.
    let tool_name = v.get("tool_name").and_then(|t| t.as_str()).unwrap_or("");
    if !matches!(tool_name, "Bash" | "bash") {
        return Ok(());
    }

    let cmd = match v
        .pointer("/tool_input/command")
        .and_then(|c| c.as_str())
        .filter(|c| !c.is_empty())
    {
        Some(c) => c,
        None => return Ok(()),
    };

    if cmd.contains("<<") {
        return Ok(());
    }

    let excluded = crate::core::config::Config::load()
        .map(|c| c.hooks.exclude_commands)
        .unwrap_or_default();

    let verdict = crate::hooks::permissions::check_command(cmd);

    if let Some(output) = build_claude_response(cmd, verdict, &excluded) {
        println!("{output}");
    }
    Ok(())
}

/// Build the JSON response for a Claude Code hook event.
///
/// Returns `None` when no output should be emitted (deny rule, no rewrite, heredoc).
/// Visible for testing.
pub(crate) fn build_claude_response(
    cmd: &str,
    verdict: PermissionVerdict,
    excluded: &[String],
) -> Option<Value> {
    // Deny: silent passthrough — Claude Code evaluates its own deny rules.
    if verdict == PermissionVerdict::Deny {
        return None;
    }

    let rewritten = rewrite_command(cmd, excluded)?;
    if rewritten == cmd {
        return None;
    }

    Some(match verdict {
        // Allow (explicit) or Default (no rules configured) → auto-approve.
        PermissionVerdict::Allow | PermissionVerdict::Default => json!({
            "hookSpecificOutput": {
                "hookEventName": PRE_TOOL_USE_KEY,
                "permissionDecision": "allow",
                "permissionDecisionReason": "RTK auto-rewrite",
                "updatedInput": { "command": rewritten }
            }
        }),
        // Ask: rewrite but omit permissionDecision so Claude Code prompts the user.
        PermissionVerdict::Ask => json!({
            "hookSpecificOutput": {
                "hookEventName": PRE_TOOL_USE_KEY,
                "updatedInput": { "command": rewritten }
            }
        }),
        PermissionVerdict::Deny => unreachable!(),
    })
}

// ── Gemini hook ───────────────────────────────────────────────

/// Run the Gemini CLI BeforeTool hook.
/// Reads JSON from stdin, rewrites shell commands to rtk equivalents,
/// outputs JSON to stdout in Gemini CLI format.
pub fn run_gemini() -> Result<()> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .context("Failed to read hook input from stdin")?;

    let json: Value = serde_json::from_str(&input).context("Failed to parse hook input as JSON")?;

    let tool_name = json.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");

    if tool_name != "run_shell_command" {
        print_allow();
        return Ok(());
    }

    let cmd = json
        .pointer("/tool_input/command")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if cmd.is_empty() {
        print_allow();
        return Ok(());
    }

    // Delegate to the single source of truth for command rewriting
    match rewrite_command(cmd, &[]) {
        Some(rewritten) => print_rewrite(&rewritten),
        None => print_allow(),
    }

    Ok(())
}

fn print_allow() {
    println!(r#"{{"decision":"allow"}}"#);
}

fn print_rewrite(cmd: &str) {
    let output = serde_json::json!({
        "decision": "allow",
        "hookSpecificOutput": {
            "tool_input": {
                "command": cmd
            }
        }
    });
    println!("{}", output);
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Copilot format detection ---

    fn vscode_input(tool: &str, cmd: &str) -> Value {
        json!({
            "tool_name": tool,
            "tool_input": { "command": cmd }
        })
    }

    fn copilot_cli_input(cmd: &str) -> Value {
        let args = serde_json::to_string(&json!({ "command": cmd })).unwrap();
        json!({ "toolName": "bash", "toolArgs": args })
    }

    #[test]
    fn test_detect_vscode_bash() {
        assert!(matches!(
            detect_format(&vscode_input("Bash", "git status")),
            HookFormat::VsCode { .. }
        ));
    }

    #[test]
    fn test_detect_vscode_run_terminal_command() {
        assert!(matches!(
            detect_format(&vscode_input("runTerminalCommand", "cargo test")),
            HookFormat::VsCode { .. }
        ));
    }

    #[test]
    fn test_detect_copilot_cli_bash() {
        assert!(matches!(
            detect_format(&copilot_cli_input("git status")),
            HookFormat::CopilotCli { .. }
        ));
    }

    #[test]
    fn test_detect_non_bash_is_passthrough() {
        let v = json!({ "tool_name": "editFiles" });
        assert!(matches!(detect_format(&v), HookFormat::PassThrough));
    }

    #[test]
    fn test_detect_unknown_is_passthrough() {
        assert!(matches!(detect_format(&json!({})), HookFormat::PassThrough));
    }

    #[test]
    fn test_get_rewritten_supported() {
        assert!(get_rewritten("git status").is_some());
    }

    #[test]
    fn test_get_rewritten_unsupported() {
        assert!(get_rewritten("htop").is_none());
    }

    #[test]
    fn test_get_rewritten_already_rtk() {
        assert!(get_rewritten("rtk git status").is_none());
    }

    #[test]
    fn test_get_rewritten_heredoc() {
        assert!(get_rewritten("cat <<'EOF'\nhello\nEOF").is_none());
    }

    // --- Gemini format ---

    #[test]
    fn test_print_allow_format() {
        // Verify the allow JSON format matches Gemini CLI expectations
        let expected = r#"{"decision":"allow"}"#;
        assert_eq!(expected, r#"{"decision":"allow"}"#);
    }

    #[test]
    fn test_print_rewrite_format() {
        let output = serde_json::json!({
            "decision": "allow",
            "hookSpecificOutput": {
                "tool_input": {
                    "command": "rtk git status"
                }
            }
        });
        let json: Value = serde_json::from_str(&output.to_string()).unwrap();
        assert_eq!(json["decision"], "allow");
        assert_eq!(
            json["hookSpecificOutput"]["tool_input"]["command"],
            "rtk git status"
        );
    }

    #[test]
    fn test_gemini_hook_uses_rewrite_command() {
        // Verify that rewrite_command handles the cases we need for Gemini
        assert_eq!(
            rewrite_command("git status", &[]),
            Some("rtk git status".into())
        );
        assert_eq!(
            rewrite_command("cargo test", &[]),
            Some("rtk cargo test".into())
        );
        // Already rtk → returned as-is (idempotent)
        assert_eq!(
            rewrite_command("rtk git status", &[]),
            Some("rtk git status".into())
        );
        // Heredoc → no rewrite
        assert_eq!(rewrite_command("cat <<EOF", &[]), None);
    }

    #[test]
    fn test_gemini_hook_excluded_commands() {
        let excluded = vec!["curl".to_string()];
        assert_eq!(rewrite_command("curl https://example.com", &excluded), None);
        // Non-excluded still rewrites
        assert_eq!(
            rewrite_command("git status", &excluded),
            Some("rtk git status".into())
        );
    }

    #[test]
    fn test_gemini_hook_env_prefix_preserved() {
        assert_eq!(
            rewrite_command("RUST_LOG=debug cargo test", &[]),
            Some("RUST_LOG=debug rtk cargo test".into())
        );
    }

    // --- run_claude() / build_claude_response() tests ---

    #[test]
    fn test_claude_allow_verdict_emits_permission_decision() {
        let response = build_claude_response("git status", PermissionVerdict::Allow, &[]);
        let response = response.expect("should produce output");
        assert_eq!(
            response["hookSpecificOutput"]["permissionDecision"],
            "allow"
        );
        assert_eq!(
            response["hookSpecificOutput"]["updatedInput"]["command"],
            "rtk git status"
        );
    }

    #[test]
    fn test_claude_default_verdict_emits_permission_decision() {
        // Default (no rules configured) should behave like Allow — auto-approve.
        let response = build_claude_response("git status", PermissionVerdict::Default, &[]);
        let response = response.expect("should produce output");
        assert_eq!(
            response["hookSpecificOutput"]["permissionDecision"],
            "allow"
        );
    }

    #[test]
    fn test_claude_ask_verdict_omits_permission_decision() {
        // Ask rule: rewrite but omit permissionDecision so Claude Code prompts.
        let response = build_claude_response("git status", PermissionVerdict::Ask, &[]);
        let response = response.expect("should produce output");
        assert!(
            response["hookSpecificOutput"]
                .get("permissionDecision")
                .is_none()
                || response["hookSpecificOutput"]["permissionDecision"].is_null(),
            "permissionDecision must be absent for Ask verdict"
        );
        assert_eq!(
            response["hookSpecificOutput"]["updatedInput"]["command"],
            "rtk git status"
        );
    }

    #[test]
    fn test_claude_deny_verdict_returns_none() {
        // Deny: no output — Claude Code evaluates its own deny rules.
        let response = build_claude_response("git status", PermissionVerdict::Deny, &[]);
        assert!(response.is_none());
    }

    #[test]
    fn test_claude_unsupported_command_returns_none() {
        let response = build_claude_response("htop", PermissionVerdict::Default, &[]);
        assert!(response.is_none());
    }

    #[test]
    fn test_claude_already_rtk_returns_none() {
        // If the command is already an rtk command, rewrite_command returns the same string.
        let response = build_claude_response("rtk git status", PermissionVerdict::Default, &[]);
        assert!(response.is_none());
    }

    #[test]
    fn test_claude_response_has_hook_event_name() {
        let response = build_claude_response("git status", PermissionVerdict::Allow, &[])
            .expect("should produce output");
        assert_eq!(
            response["hookSpecificOutput"]["hookEventName"],
            PRE_TOOL_USE_KEY
        );
    }

    #[test]
    fn test_claude_excluded_command_returns_none() {
        // Excluded commands are not rewritten — hook should produce no output.
        let excluded = vec!["git".to_string()];
        let response = build_claude_response("git status", PermissionVerdict::Default, &excluded);
        assert!(response.is_none());
    }
}
