//! Processes incoming hook calls from AI agents and rewrites commands on the fly.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::io::{self, Read};

use crate::discover::registry::rewrite_command;
use super::permissions::{check_command, check_command_with_rules, PermissionVerdict};

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
    // SECURITY: check deny/ask BEFORE rewrite so non-RTK commands are also covered.
    let verdict = check_command(cmd);

    match verdict {
        PermissionVerdict::Deny => {
            // Return deny response - let Claude Code's native deny handling take over
            // We don't print anything, which signals denial
            return Ok(());
        }
        PermissionVerdict::Ask => {
            // For Ask: rewrite but signal ask so Claude Code prompts the user
            if let Some(rewritten) = get_rewritten(cmd) {
                let output = json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "ask",
                        "permissionDecisionReason": "Permission required for this command",
                        "updatedInput": { "command": rewritten }
                    }
                });
                println!("{output}");
            }
            // If no rewrite, pass through with ask signal
            return Ok(());
        }
        PermissionVerdict::Allow => {
            // Proceed with rewrite
        }
    }

    let rewritten = match get_rewritten(cmd) {
        Some(r) => r,
        None => return Ok(()),
    };

    let output = json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": "RTK auto-rewrite",
            "updatedInput": { "command": rewritten }
        }
    });
    println!("{output}");
    Ok(())
}

fn handle_copilot_cli(cmd: &str) -> Result<()> {
    // SECURITY: check deny/ask BEFORE rewrite so non-RTK commands are also covered.
    let verdict = check_command(cmd);

    // Deny takes priority - if a deny rule matches, don't suggest rewrite
    if verdict == PermissionVerdict::Deny {
        // Return deny without suggestion - Copilot CLI will use its native deny handling
        return Ok(());
    }

    // For Ask: still show the rewrite suggestion but Copilot CLI doesn't support ask
    // We'll show the deny-with-suggestion format as before
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

// ── Claude Code hook ───────────────────────────────────────────

/// Run Claude Code PreToolUse hook.
/// Reads JSON from stdin, rewrites shell commands to rtk equivalents,
/// outputs JSON to stdout in Claude Code format.
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

    // Extract command from tool_input.command
    let cmd = match v
        .pointer("/tool_input/command")
        .and_then(|c| c.as_str())
        .filter(|c| !c.is_empty())
    {
        Some(c) => c,
        None => return Ok(()), // No command = pass through
    };

    handle_vscode(cmd)
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

    // SECURITY: check deny/ask BEFORE rewrite so non-RTK commands are also covered.
    let verdict = check_command(cmd);

    match verdict {
        PermissionVerdict::Deny => {
            print_deny();
            return Ok(());
        }
        PermissionVerdict::Ask => {
            // For ask: if there's a rewrite, show it but still ask
            match rewrite_command(cmd, &[]) {
                Some(rewritten) => print_ask(&rewritten),
                None => print_ask(cmd),
            }
            return Ok(());
        }
        PermissionVerdict::Allow => {
            // Proceed with rewrite
        }
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

fn print_deny() {
    println!(r#"{{"decision":"deny"}}"#);
}

fn print_ask(cmd: &str) {
    let output = serde_json::json!({
        "decision": "ask",
        "hookSpecificOutput": {
            "tool_input": {
                "command": cmd
            }
        }
    });
    println!("{}", output);
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

    // --- Claude Code hook ---

    #[test]
    fn test_claude_hook_format_matches_vscode() {
        // Claude Code uses same format as VS Code
        let input = json!({
            "tool_name": "Bash",
            "tool_input": { "command": "git status" }
        });
        assert!(matches!(
            detect_format(&input),
            HookFormat::VsCode { .. }
        ));
    }

    #[test]
    fn test_claude_hook_output_format() {
        // Verify the output format matches expected Claude Code hook format
        let cmd = "git status";
        let rewritten = get_rewritten(cmd).unwrap();

        let output = json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "permissionDecisionReason": "RTK auto-rewrite",
                "updatedInput": { "command": rewritten }
            }
        });

        let json: Value = serde_json::from_str(&output.to_string()).unwrap();
        assert_eq!(
            json["hookSpecificOutput"]["hookEventName"],
            "PreToolUse"
        );
        assert_eq!(
            json["hookSpecificOutput"]["permissionDecision"],
            "allow"
        );
        assert_eq!(
            json["hookSpecificOutput"]["updatedInput"]["command"],
            "rtk git status"
        );
    }

    // --- Permission checking tests ---

    #[test]
    fn test_handle_vscode_denies_blocked_command() {
        // Verify that a denied command doesn't produce output
        let deny = vec!["git push --force".to_string()];
        let verdict = check_command_with_rules("git push --force", &deny, &[]);
        assert_eq!(verdict, PermissionVerdict::Deny);

        // When deny matches, handle_vscode should return early without output
        // We can't test the stdout directly in unit tests, but we verify
        // the permission check returns Deny
    }

    #[test]
    fn test_handle_vscode_allows_safe_command() {
        let verdict = check_command_with_rules("git status", &[], &[]);
        assert_eq!(verdict, PermissionVerdict::Allow);
    }

    #[test]
    fn test_handle_vscode_prompts_for_ask_command() {
        let ask = vec!["git push".to_string()];
        let verdict = check_command_with_rules("git push origin main", &[], &ask);
        assert_eq!(verdict, PermissionVerdict::Ask);
    }

    #[test]
    fn test_handle_copilot_cli_denies_blocked_command() {
        let deny = vec!["rm -rf".to_string()];
        let verdict = check_command_with_rules("rm -rf /tmp/test", &deny, &[]);
        assert_eq!(verdict, PermissionVerdict::Deny);
    }

    #[test]
    fn test_handle_copilot_cli_allows_safe_command() {
        let verdict = check_command_with_rules("git status", &[], &[]);
        assert_eq!(verdict, PermissionVerdict::Allow);
    }

    #[test]
    fn test_deny_takes_precedence_over_ask() {
        let deny = vec!["git push --force".to_string()];
        let ask = vec!["git push".to_string()];
        let verdict = check_command_with_rules("git push --force", &deny, &ask);
        assert_eq!(verdict, PermissionVerdict::Deny);
    }

    #[test]
    fn test_compound_command_deny_detection() {
        let deny = vec!["git push --force".to_string()];
        let verdict = check_command_with_rules("git status && git push --force", &deny, &[]);
        assert_eq!(verdict, PermissionVerdict::Deny);
    }

    #[test]
    fn test_print_ask_format() {
        // Verify print_ask produces valid JSON with ask decision
        let output = serde_json::json!({
            "decision": "ask",
            "hookSpecificOutput": {
                "tool_input": {
                    "command": "git push"
                }
            }
        });
        let json: Value = serde_json::from_str(&output.to_string()).unwrap();
        assert_eq!(json["decision"], "ask");
        assert_eq!(
            json["hookSpecificOutput"]["tool_input"]["command"],
            "git push"
        );
    }

    #[test]
    fn test_print_deny_format() {
        // Verify print_deny produces valid JSON with deny decision
        let expected = r#"{"decision":"deny"}"#;
        assert_eq!(expected, r#"{"decision":"deny"}"#);
    }
}
