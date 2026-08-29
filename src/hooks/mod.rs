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
/// Accepts either interpretation of the command string: POSIX shell form
/// (quotes and backslash escapes unwrapped, e.g. `/opt/RTK\ Tools/rtk hook
/// claude`) or raw Windows form (backslashes are path separators and the
/// binary may be quoted to contain spaces, e.g. `C:\rtk\rtk.exe hook
/// copilot`). A trailing `.exe` on the binary is accepted in both.
fn is_rtk_hook_command(command: &str, agent: &str) -> bool {
    matches_posix_form(command, agent) || matches_raw_form(command, agent)
}

/// POSIX shell interpretation via `shell_split`.
fn matches_posix_form(command: &str, agent: &str) -> bool {
    let parts = crate::discover::lexer::shell_split(command);
    let [binary, hook, target] = parts.as_slice() else {
        return false;
    };
    hook == "hook" && target == agent && binary_is_rtk(binary)
}

/// Raw interpretation for Windows-style registrations: no unescaping, so
/// backslash paths survive; a quoted binary may contain spaces and the
/// closing quote must end the token.
fn matches_raw_form(command: &str, agent: &str) -> bool {
    let trimmed = command.trim();
    let (binary, rest) = match trimmed.chars().next() {
        Some(quote @ ('"' | '\'')) => {
            let inner = &trimmed[1..];
            let Some(end) = inner.find(quote) else {
                return false;
            };
            let rest = &inner[end + 1..];
            if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
                return false;
            }
            (&inner[..end], rest)
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

    hook == "hook" && target == agent && binary_is_rtk(binary)
}

fn binary_is_rtk(binary: &str) -> bool {
    let name = binary.rsplit(['/', '\\']).next().unwrap_or(binary);
    let name = name.to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    name == "rtk"
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

    #[test]
    fn hook_command_rejects_quote_glued_to_next_token() {
        assert!(!is_copilot_hook_command(r#""rtk"hook copilot"#));
        assert!(!is_copilot_hook_command("'rtk'hook copilot"));
        assert!(!is_copilot_hook_command(r#""rtk"x hook copilot"#));
    }

    #[test]
    fn hook_command_matches_posix_escaped_paths() {
        assert!(is_claude_hook_command(r"/opt/RTK\ Tools/rtk hook claude"));
        assert!(is_copilot_hook_command(r"/opt/RTK\ Tools/rtk hook copilot"));
        assert!(is_copilot_hook_command(
            r#""/opt/rtk tools/rtk" hook copilot"#
        ));
        assert!(!is_copilot_hook_command(
            r"/opt/RTK\ Tools/not-rtk hook copilot"
        ));
    }
}
