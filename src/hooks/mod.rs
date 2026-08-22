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

/// Match a hook-config command string against `rtk hook <agent>`.
///
/// Hook configs store plain command strings, not shell scripts, so this
/// parses without shell unescaping: Windows backslash paths survive, a
/// quoted binary path may contain spaces, and a trailing `.exe` on the
/// binary is accepted.
fn is_rtk_hook_command(command: &str, agent: &str) -> bool {
    let trimmed = command.trim();
    let (binary, rest) = match trimmed.chars().next() {
        Some(quote @ ('"' | '\'')) => {
            let inner = &trimmed[1..];
            let Some(end) = inner.find(quote) else {
                return false;
            };
            (&inner[..end], &inner[end + 1..])
        }
        Some(_) => {
            let end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
            trimmed.split_at(end)
        }
        None => return false,
    };

    let mut args = rest.split_whitespace();
    let (Some(hook), Some(target), None) = (args.next(), args.next(), args.next()) else {
        return false;
    };

    let binary_name = binary.rsplit(['/', '\\']).next().unwrap_or(binary);
    let binary_name = binary_name.to_ascii_lowercase();
    let binary_name = binary_name.strip_suffix(".exe").unwrap_or(&binary_name);

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

    #[test]
    fn hook_command_matches_windows_paths() {
        assert!(is_copilot_hook_command(r"C:\rtk\rtk.exe hook copilot"));
        assert!(is_copilot_hook_command(
            r#""C:\Program Files\rtk\rtk.exe" hook copilot"#
        ));
        assert!(is_copilot_hook_command("rtk.exe hook copilot"));
        assert!(is_claude_hook_command(r"C:\rtk\rtk.exe hook claude"));
    }

    #[test]
    fn hook_command_rejects_lookalike_windows_binaries() {
        assert!(!is_copilot_hook_command(r"C:\rtk\not-rtk.exe hook copilot"));
        assert!(!is_copilot_hook_command(r"C:\rtk\rtk.exe.bak hook copilot"));
        assert!(!is_copilot_hook_command(
            r#""C:\Program Files\rtk\rtk.exe hook copilot"#
        ));
    }
}
