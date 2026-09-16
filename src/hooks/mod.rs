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
    matches!(binary_name, "rtk" | "rtk.exe")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_hook_command_matches_bare_and_absolute_rtk() {
        assert!(is_claude_hook_command("rtk hook claude"));
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
}
