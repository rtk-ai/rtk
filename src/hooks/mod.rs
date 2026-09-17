//! Hook installation and lifecycle management for AI coding agents.

pub mod constants;
// Shares `hook_cmd`'s constraint: it runs inside the hook, where stray output
// corrupts the JSON protocol. `from_agent` is the one exception and carries its
// own allow -- it is reached only from the `rtk hook check` CLI.
#[deny(clippy::print_stdout, clippy::print_stderr)]
pub mod decision;
pub mod hook_audit_cmd;
pub mod hook_check;
#[deny(clippy::print_stdout, clippy::print_stderr)]
pub mod hook_cmd;
pub mod init;
pub mod integrity;
pub mod permissions;
pub mod rewrite_cmd;
pub mod trust;
pub mod verify_cmd;

fn is_rtk_binary(binary: &str) -> bool {
    let binary_name = binary.rsplit(['/', '\\']).next().unwrap_or(binary);
    binary_name == constants::CLAUDE_HOOK_BINARY || binary_name.eq_ignore_ascii_case("rtk.exe")
}

fn is_rtk_hook_command(command: &str, agent: &str) -> bool {
    let parts = crate::discover::lexer::shell_split(command);
    let [parsed_binary, hook, target] = parts.as_slice() else {
        return false;
    };

    is_rtk_binary(parsed_binary) && hook == "hook" && target == agent
}

pub fn is_claude_hook_command(command: &str) -> bool {
    is_rtk_hook_command(command, "claude")
}

pub fn is_codex_hook_command(command: &str) -> bool {
    is_rtk_hook_command(command, "codex")
}

pub fn is_claude_hook_entry(hook: &serde_json::Value) -> bool {
    if hook.get("type").and_then(serde_json::Value::as_str) != Some("command") {
        return false;
    }
    let Some(command) = hook.get("command").and_then(serde_json::Value::as_str) else {
        return false;
    };
    match hook.get("args") {
        None => is_claude_hook_command(command),
        Some(serde_json::Value::Array(args)) => {
            let [hook_arg, claude_arg] = args.as_slice() else {
                return false;
            };
            is_rtk_binary(command)
                && hook_arg.as_str() == Some(constants::CLAUDE_HOOK_ARGS[0])
                && claude_arg.as_str() == Some(constants::CLAUDE_HOOK_ARGS[1])
        }
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_hook_command_matches_bare_and_absolute_rtk() {
        assert!(is_claude_hook_command("rtk hook claude"));
        assert!(is_claude_hook_command("rtk.exe hook claude"));
        assert!(is_claude_hook_command("/opt/homebrew/bin/rtk hook claude"));
        assert!(is_claude_hook_command(
            "\"/opt/homebrew/bin/rtk\" hook claude"
        ));
        assert!(is_claude_hook_command(
            "/Users/jane/My\\ Apps/rtk hook claude"
        ));
    }

    #[test]
    fn claude_hook_command_rejects_other_commands() {
        assert!(!is_claude_hook_command("not-rtk hook claude"));
        assert!(!is_claude_hook_command("/opt/homebrew/bin/rtk hook cursor"));
        assert!(!is_claude_hook_command("echo rtk hook claude"));
    }

    #[test]
    fn codex_hook_command_matches_bare_absolute_and_windows_rtk() {
        assert!(is_codex_hook_command("rtk hook codex"));
        assert!(is_codex_hook_command("/opt/homebrew/bin/rtk hook codex"));
        assert!(is_codex_hook_command(
            "\"C:\\Program Files\\rtk.exe\" hook codex"
        ));
    }

    #[test]
    fn codex_hook_command_rejects_other_commands() {
        assert!(!is_codex_hook_command("rtk hook claude"));
        assert!(!is_codex_hook_command("echo rtk hook codex"));
        assert!(!is_codex_hook_command("\"rtk\"evil hook codex"));
    }

    #[test]
    fn claude_hook_entry_matches_windows_exec_form() {
        let hook = serde_json::json!({
            "type": "command",
            "command": r"C:\Users\me\.local\bin\rtk.exe",
            "args": ["hook", "claude"]
        });

        assert!(is_claude_hook_entry(&hook));
    }

    #[test]
    fn claude_hook_entry_does_not_treat_empty_args_as_shell_form() {
        let empty_args = serde_json::json!({
            "type": "command",
            "command": "rtk hook claude",
            "args": []
        });
        let wrong_args = serde_json::json!({
            "type": "command",
            "command": "rtk.exe",
            "args": ["hook", "cursor"]
        });
        let prompt_hook = serde_json::json!({
            "type": "prompt",
            "command": "rtk.exe",
            "args": ["hook", "claude"]
        });

        assert!(!is_claude_hook_entry(&empty_args));
        assert!(!is_claude_hook_entry(&wrong_args));
        assert!(!is_claude_hook_entry(&prompt_hook));
    }
}
