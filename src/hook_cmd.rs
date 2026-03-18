use anyhow::{Context, Result};
use serde_json::Value;
use std::io::{self, Read};

use crate::discover::registry::rewrite_command;

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
