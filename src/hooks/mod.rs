//! Hook installation and lifecycle management for AI coding agents.

pub mod constants;
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

fn is_rtk_hook_command(command: &str, agent: &str) -> bool {
    let parts = crate::discover::lexer::shell_split(command);
    let [binary, hook, target] = parts.as_slice() else {
        return false;
    };

    let binary_name = binary.rsplit(['/', '\\']).next().unwrap_or(binary);

    binary_name == "rtk" && hook == "hook" && target == agent
}

pub fn is_claude_hook_command(command: &str) -> bool {
    is_rtk_hook_command(command, "claude")
}

pub fn is_copilot_hook_command(command: &str) -> bool {
    is_rtk_hook_command(command, "copilot")
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
    }

    #[test]
    fn claude_hook_command_rejects_other_commands() {
        assert!(!is_claude_hook_command("not-rtk hook claude"));
        assert!(!is_claude_hook_command("/opt/homebrew/bin/rtk hook cursor"));
        assert!(!is_claude_hook_command("echo rtk hook claude"));
    }

    #[test]
    fn copilot_hook_command_matches_bare_and_absolute_rtk() {
        assert!(is_copilot_hook_command("rtk hook copilot"));
        assert!(is_copilot_hook_command(
            "/opt/homebrew/bin/rtk hook copilot"
        ));
        assert!(is_copilot_hook_command(
            "\"/opt/homebrew/bin/rtk\" hook copilot"
        ));
    }

    #[test]
    fn copilot_hook_command_rejects_other_commands() {
        assert!(!is_copilot_hook_command("not-rtk hook copilot"));
        assert!(!is_copilot_hook_command(
            "/opt/homebrew/bin/rtk hook claude"
        ));
        assert!(!is_copilot_hook_command("echo rtk hook copilot"));
        assert!(!is_copilot_hook_command("rtk hook copilot extra"));
    }
}
