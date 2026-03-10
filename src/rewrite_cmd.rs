use crate::config;
use crate::discover::registry;

/// Run the `rtk rewrite` command.
///
/// **Default mode** (no flags):
/// Prints the RTK-rewritten command to stdout and exits 0.
/// Exits 1 (without output) if the command has no RTK equivalent.
///
/// **`--hook-json` mode**:
/// Outputs the full Claude Code PreToolUse hook JSON response including
/// the rewritten command and permission decision (controlled by
/// `hooks.auto_approve` in config.toml / RTK_HOOK_AUTO_APPROVE env var).
/// Exits 0 with no output if the command has no RTK equivalent.
pub fn run(cmd: &str, hook_json: bool) -> anyhow::Result<()> {
    let config = config::Config::load().unwrap_or_default();
    let excluded = &config.hooks.exclude_commands;

    if hook_json {
        run_hook_json(cmd, &config)
    } else {
        run_plain(cmd, excluded)
    }
}

/// Original behavior: print rewritten command or exit 1.
fn run_plain(cmd: &str, excluded: &[String]) -> anyhow::Result<()> {
    match registry::rewrite_command(cmd, excluded) {
        Some(rewritten) => {
            print!("{}", rewritten);
            Ok(())
        }
        None => {
            std::process::exit(1);
        }
    }
}

/// Build the PreToolUse hook JSON response.
/// Extracted as a pure function for testability.
fn build_hook_response(rewritten: &str, auto_approve: bool) -> serde_json::Value {
    if auto_approve {
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "permissionDecisionReason": "RTK auto-rewrite",
                "updatedInput": {
                    "command": rewritten
                }
            }
        })
    } else {
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "updatedInput": {
                    "command": rewritten
                }
            }
        })
    }
}

/// Hook-json mode: output full Claude Code PreToolUse response JSON.
/// The command string comes from the CLI arg (extracted by the hook script
/// via jq before invoking `rtk rewrite --hook-json "$CMD"`).
fn run_hook_json(cmd: &str, config: &config::Config) -> anyhow::Result<()> {
    let excluded = &config.hooks.exclude_commands;

    let rewritten = match registry::rewrite_command(cmd, excluded) {
        Some(r) => r,
        None => return Ok(()), // no rewrite — silent exit 0, hook passes through
    };

    // Already rtk-prefixed or compound where all segments matched — nothing to do
    if cmd == rewritten {
        return Ok(());
    }

    let auto_approve = config::resolve_auto_approve_with(&config.hooks);

    let response = build_hook_response(&rewritten, auto_approve);
    println!("{}", serde_json::to_string(&response)?);
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
    fn test_build_hook_response_auto_approve() {
        let response = build_hook_response("rtk git status", true);
        let output = response["hookSpecificOutput"].as_object().unwrap();
        assert_eq!(output["permissionDecision"], "allow");
        assert_eq!(output["updatedInput"]["command"], "rtk git status");
    }

    #[test]
    fn test_build_hook_response_no_auto_approve() {
        let response = build_hook_response("rtk git status", false);
        let output = response["hookSpecificOutput"].as_object().unwrap();
        assert!(!output.contains_key("permissionDecision"));
        assert_eq!(output["updatedInput"]["command"], "rtk git status");
    }
}
