//! Processes incoming hook calls from AI agents and rewrites commands on the fly.

use super::constants::PRE_TOOL_USE_KEY;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::io::{self, Read};

use crate::discover::registry::rewrite_command;
use crate::hooks::permissions::{check_command, PermissionVerdict};

// ── Copilot hook (VS Code + Copilot CLI) ──────────────────────

/// Format detected from the preToolUse JSON input.
enum HookFormat {
    /// VS Code Copilot Chat / Claude Code: `tool_name` + `tool_input.command`.
    VsCode { command: String },
    /// GitHub Copilot CLI: camelCase `toolName` + `toolArgs` (JSON-encoded string).
    /// Supports `updatedInput` / `modifiedArgs` since v1.0.24 (copilot-cli#2013).
    /// `tool_args` carries the full decoded toolArgs object so we can preserve
    /// all original fields (e.g. `description`) when building `updatedInput`.
    CopilotCli { command: String, tool_args: Value },
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
        HookFormat::CopilotCli { command, tool_args } => handle_copilot_cli(&command, &tool_args),
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
                            tool_args,
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

    let verdict = check_command(cmd);

    // Deny: pass through without rewrite — let the host tool handle it.
    if verdict == PermissionVerdict::Deny {
        return Ok(());
    }

    // Allow (explicit rule matched): auto-allow the rewritten command.
    // Ask/Default (no allow rule matched): rewrite but let the host tool prompt.
    let decision = match verdict {
        PermissionVerdict::Allow => "allow",
        _ => "ask",
    };

    let output = json!({
        "hookSpecificOutput": {
            "hookEventName": PRE_TOOL_USE_KEY,
            "permissionDecision": decision,
            "permissionDecisionReason": "RTK auto-rewrite",
            "updatedInput": { "command": rewritten }
        }
    });
    println!("{output}");
    Ok(())
}

fn handle_copilot_cli(cmd: &str, tool_args: &Value) -> Result<()> {
    let rewritten = match get_rewritten(cmd) {
        Some(r) => r,
        None => return Ok(()),
    };

    let verdict = check_command(cmd);

    // Deny: pass through without rewrite — let the host tool handle it.
    if verdict == PermissionVerdict::Deny {
        return Ok(());
    }

    // Default (no rule matched): auto-allow — just prepending `rtk` to an already-approved cmd.
    // Only an explicit `ask` rule triggers a prompt.
    let decision = match verdict {
        PermissionVerdict::Ask => "ask",
        _ => "allow",
    };

    // Preserve all original toolArgs fields (e.g. `description`) — Copilot CLI validates
    // that the rewritten args contain every field present in the original schema.
    let mut updated = tool_args.clone();
    updated["command"] = json!(rewritten);

    // Use the same hookSpecificOutput envelope as the VS Code path.
    // Include both updatedInput and modifiedArgs for maximum v1.0.24 compatibility.
    let output = json!({
        "hookSpecificOutput": {
            "hookEventName": PRE_TOOL_USE_KEY,
            "permissionDecision": decision,
            "permissionDecisionReason": "RTK auto-rewrite",
            "updatedInput": updated,
            "modifiedArgs": updated
        }
    });
    println!("{output}");
    Ok(())
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

    // Check deny rules — Gemini CLI only supports allow/deny (no ask mode).
    if check_command(cmd) == PermissionVerdict::Deny {
        println!(r#"{{"decision":"deny","reason":"Blocked by RTK permission rule"}}"#);
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
        let args =
            serde_json::to_string(&json!({ "command": cmd, "description": "test cmd" })).unwrap();
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

    // --- Copilot CLI output ---

    fn capture_copilot_cli_output(cmd: &str) -> Option<Value> {
        let tool_args = json!({ "command": cmd, "description": "run cmd" });
        let rewritten = get_rewritten(cmd)?;
        let verdict = check_command(cmd);
        if verdict == PermissionVerdict::Deny {
            return None;
        }
        let decision = match verdict {
            PermissionVerdict::Ask => "ask",
            _ => "allow",
        };
        let mut updated = tool_args.clone();
        updated["command"] = json!(rewritten);
        Some(json!({
            "hookSpecificOutput": {
                "hookEventName": PRE_TOOL_USE_KEY,
                "permissionDecision": decision,
                "permissionDecisionReason": "RTK auto-rewrite",
                "updatedInput": updated,
                "modifiedArgs": updated
            }
        }))
    }

    #[test]
    fn test_copilot_cli_output_has_updated_input() {
        let out = capture_copilot_cli_output("git status").unwrap();
        assert_eq!(
            out["hookSpecificOutput"]["updatedInput"]["command"],
            "rtk git status"
        );
        assert_eq!(
            out["hookSpecificOutput"]["modifiedArgs"]["command"],
            "rtk git status"
        );
    }

    #[test]
    fn test_copilot_cli_output_preserves_tool_args_fields() {
        let out = capture_copilot_cli_output("git status").unwrap();
        // description field from original toolArgs must survive in updatedInput
        assert_eq!(
            out["hookSpecificOutput"]["updatedInput"]["description"],
            "run cmd"
        );
        assert_eq!(
            out["hookSpecificOutput"]["modifiedArgs"]["description"],
            "run cmd"
        );
    }

    #[test]
    fn test_copilot_cli_default_verdict_is_allow() {
        let out = capture_copilot_cli_output("git status").unwrap();
        assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "allow");
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
}
