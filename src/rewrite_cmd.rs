use anyhow::Context;
use crate::discover::registry;

/// Run the `rtk rewrite` command.
///
/// Prints the RTK-rewritten command to stdout and exits 0.
/// Exits 1 (without output) if the command has no RTK equivalent.
///
/// Used by shell hooks to rewrite commands transparently:
/// ```bash
/// REWRITTEN=$(rtk rewrite "$CMD") || exit 0
/// [ "$CMD" = "$REWRITTEN" ] && exit 0  # already RTK, skip
/// ```
pub fn run(cmd: &str) -> anyhow::Result<()> {
    let excluded = crate::config::Config::load()
        .map(|c| c.hooks.exclude_commands)
        .unwrap_or_default();

    match registry::rewrite_command(cmd, &excluded) {
        Some(rewritten) => {
            print!("{}", rewritten);
            Ok(())
        }
        None => {
            std::process::exit(1);
        }
    }
}

/// Hook mode: reads full JSON from stdin, emits hook JSON response to stdout.
///
/// This eliminates jq from the trust boundary -- all JSON handling happens in Rust.
/// Graceful degradation: malformed JSON, empty stdin, missing fields all pass through silently.
pub fn run_hook_mode() -> anyhow::Result<()> {
    use std::io::Read;

    let mut input = String::new();
    // Intentional: any stdin read error is treated as empty input (passthrough).
    // Broken pipe or closed stdin should not block the hook.
    let _ = std::io::stdin().read_to_string(&mut input);

    if input.trim().is_empty() {
        return Ok(());
    }

    let parsed: serde_json::Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };

    let cmd = match parsed
        .get("tool_input")
        .and_then(|ti| ti.get("command"))
        .and_then(|c| c.as_str())
    {
        Some(c) if !c.is_empty() => c,
        _ => return Ok(()),
    };

    let excluded = crate::config::Config::load()
        .map(|c| c.hooks.exclude_commands)
        .unwrap_or_default();

    let rewritten = match registry::rewrite_command(cmd, &excluded) {
        Some(r) if r != cmd => r,
        _ => return Ok(()),
    };

    // Build updatedInput: preserve all original tool_input fields, override command
    let mut updated_input = match parsed.get("tool_input").cloned() {
        Some(serde_json::Value::Object(m)) => m,
        _ => return Ok(()),
    };
    updated_input.insert(
        "command".to_string(),
        serde_json::Value::String(rewritten),
    );

    let response = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": "RTK auto-rewrite",
            "updatedInput": updated_input
        }
    });

    let serialized = serde_json::to_string(&response)
        .context("Failed to serialize hook response")?;
    println!("{}", serialized);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_supported_command_succeeds() {
        assert!(registry::rewrite_command("git status", &[]).is_some());
    }

    #[test]
    fn test_run_unsupported_returns_none() {
        assert!(registry::rewrite_command("htop", &[]).is_none());
    }

    #[test]
    fn test_run_already_rtk_returns_some() {
        assert_eq!(
            registry::rewrite_command("rtk git status", &[]),
            Some("rtk git status".into())
        );
    }

    #[test]
    fn test_hook_mode_json_structure() {
        // Simulate what run_hook_mode does internally
        let input = r#"{"tool_input":{"command":"git status"}}"#;
        let parsed: serde_json::Value = serde_json::from_str(input).unwrap();

        let cmd = parsed
            .get("tool_input")
            .and_then(|ti| ti.get("command"))
            .and_then(|c| c.as_str())
            .unwrap();

        let rewritten = registry::rewrite_command(cmd, &[]).unwrap();
        assert!(rewritten.starts_with("rtk "));

        let mut updated_input = parsed
            .get("tool_input")
            .cloned()
            .unwrap()
            .as_object()
            .unwrap()
            .clone();
        updated_input.insert(
            "command".to_string(),
            serde_json::Value::String(rewritten.clone()),
        );

        let response = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "permissionDecisionReason": "RTK auto-rewrite",
                "updatedInput": updated_input
            }
        });

        let output = response.get("hookSpecificOutput").unwrap();
        assert_eq!(output.get("permissionDecision").unwrap(), "allow");
        assert_eq!(output.get("hookEventName").unwrap(), "PreToolUse");
        assert_eq!(
            output
                .get("updatedInput")
                .unwrap()
                .get("command")
                .unwrap()
                .as_str()
                .unwrap(),
            &rewritten
        );
    }

    #[test]
    fn test_hook_mode_preserves_non_command_fields() {
        let input =
            r#"{"tool_input":{"command":"git status","workingDirectory":"/home/user/project"}}"#;
        let parsed: serde_json::Value = serde_json::from_str(input).unwrap();

        let mut updated_input = parsed
            .get("tool_input")
            .cloned()
            .unwrap()
            .as_object()
            .unwrap()
            .clone();
        updated_input.insert(
            "command".to_string(),
            serde_json::Value::String("rtk git status".to_string()),
        );

        assert_eq!(
            updated_input.get("workingDirectory").unwrap(),
            "/home/user/project"
        );
        assert_eq!(
            updated_input.get("command").unwrap(),
            "rtk git status"
        );
    }

    #[test]
    fn test_hook_mode_unsupported_produces_no_rewrite() {
        let cmd = "htop";
        let result = registry::rewrite_command(cmd, &[]);
        assert!(result.is_none());
    }

    #[test]
    fn test_hook_mode_empty_command_skips() {
        let input = r#"{"tool_input":{"command":""}}"#;
        let parsed: serde_json::Value = serde_json::from_str(input).unwrap();

        let cmd = parsed
            .get("tool_input")
            .and_then(|ti| ti.get("command"))
            .and_then(|c| c.as_str());

        // Empty string should be treated as "no command"
        match cmd {
            Some(c) if !c.is_empty() => panic!("Expected empty command to be filtered"),
            _ => {} // correct: empty or missing
        }
    }

    #[test]
    fn test_hook_mode_malformed_json_skips() {
        let input = "not json at all";
        let result: Result<serde_json::Value, _> = serde_json::from_str(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_hook_mode_missing_tool_input_skips() {
        let input = r#"{"other_field":"value"}"#;
        let parsed: serde_json::Value = serde_json::from_str(input).unwrap();

        let cmd = parsed
            .get("tool_input")
            .and_then(|ti| ti.get("command"))
            .and_then(|c| c.as_str());

        assert!(cmd.is_none());
    }
}
