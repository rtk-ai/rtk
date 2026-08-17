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

fn is_rtk_binary(binary: &str) -> bool {
    let binary_name = binary.rsplit(['/', '\\']).next().unwrap_or(binary);
    matches!(binary_name, "rtk" | "rtk.exe")
}

fn raw_first_token(command: &str) -> Option<&str> {
    let command = command.trim_start();
    let quote = match command.as_bytes().first() {
        Some(b'"') => Some('"'),
        Some(b'\'') => Some('\''),
        Some(_) => return command.split_whitespace().next(),
        None => return None,
    }?;

    let quoted = &command[1..];
    let end = quoted.find(quote)?;
    let suffix = &quoted[end + quote.len_utf8()..];
    if suffix.chars().next().is_some_and(|ch| !ch.is_whitespace()) {
        return None;
    }
    Some(&quoted[..end])
}

fn is_rtk_hook_command(command: &str, agent: &str) -> bool {
    let parts = crate::discover::lexer::shell_split(command);
    let [parsed_binary, hook, target] = parts.as_slice() else {
        return false;
    };

    // Prefer the shell-parsed token so POSIX escaped spaces are resolved.
    // Fall back to the raw token because shell_split treats Windows path
    // backslashes as escapes.
    let has_rtk_binary =
        is_rtk_binary(parsed_binary) || raw_first_token(command).is_some_and(is_rtk_binary);

    has_rtk_binary && hook == "hook" && target == agent
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
