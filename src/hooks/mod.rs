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

pub fn is_claude_hook_command(command: &str) -> bool {
    let command = command.trim();
    let Some((binary, args)) = split_hook_command(command) else {
        return false;
    };

    let binary_name = binary
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(binary)
        .to_ascii_lowercase();
    let mut args = args.split_ascii_whitespace();

    matches!(binary_name.as_str(), "rtk" | "rtk.exe")
        && args.next() == Some("hook")
        && args.next() == Some("claude")
        && args.next().is_none()
}

fn split_hook_command(command: &str) -> Option<(&str, &str)> {
    let bytes = command.as_bytes();
    if matches!(bytes.first(), Some(b'"' | b'\'')) {
        let quote = bytes[0];
        let end = bytes
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(idx, byte)| (*byte == quote).then_some(idx))?;
        return Some((&command[1..end], command[end + 1..].trim()));
    }

    let split_at = command.find(char::is_whitespace).unwrap_or(command.len());
    Some((&command[..split_at], command[split_at..].trim()))
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
    fn claude_hook_command_matches_windows_absolute_rtk_exe() {
        assert!(is_claude_hook_command(
            r"C:\Users\codex\.local\bin\rtk.exe hook claude"
        ));
        assert!(is_claude_hook_command(
            r#""C:\Program Files\RTK\rtk.exe" hook claude"#
        ));
        assert!(is_claude_hook_command(
            r#""C:\Program Files\RTK\RTK.EXE" hook claude"#
        ));
    }

    #[test]
    fn claude_hook_command_rejects_other_commands() {
        assert!(!is_claude_hook_command("not-rtk hook claude"));
        assert!(!is_claude_hook_command("/opt/homebrew/bin/rtk hook cursor"));
        assert!(!is_claude_hook_command("echo rtk hook claude"));
    }
}
