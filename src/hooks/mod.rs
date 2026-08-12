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
    let [_parsed_binary, hook, target] = parts.as_slice() else {
        return false;
    };

    // shell_split treats backslashes as escapes, so use the raw first token
    // for basename detection to preserve quoted Windows paths.
    let command = command.trim_start();
    let binary = match command.as_bytes().first() {
        Some(b'"') => command[1..]
            .find('"')
            .map(|end| &command[1..end + 1])
            .unwrap_or(""),
        Some(b'\'') => command[1..]
            .find('\'')
            .map(|end| &command[1..end + 1])
            .unwrap_or(""),
        Some(_) => command.split_whitespace().next().unwrap_or(""),
        None => "",
    };
    let binary_name = binary.rsplit(['/', '\\']).next().unwrap_or(binary);

    matches!(binary_name, "rtk" | "rtk.exe") && hook == "hook" && target == agent
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
    }
}
