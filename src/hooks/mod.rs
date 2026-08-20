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

/// Directory holding the hook audit log.
///
/// `RTK_AUDIT_DIR` overrides everything. Otherwise Windows uses
/// `%LOCALAPPDATA%/rtk`, and Unix keeps the historic `$XDG_DATA_HOME/rtk`
/// or `$HOME/.local/share/rtk` -- deliberately not `dirs::data_local_dir()`,
/// which resolves to `~/Library/Application Support` on macOS and would
/// silently move an existing user's log.
pub fn audit_dir() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    if let Ok(dir) = std::env::var("RTK_AUDIT_DIR") {
        return Some(PathBuf::from(dir));
    }

    #[cfg(windows)]
    {
        std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok()
            .or_else(dirs::data_local_dir)
            .map(|d| d.join("rtk"))
    }

    #[cfg(not(windows))]
    {
        std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .ok()
            .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("share")))
            .map(|d| d.join("rtk"))
    }
}

pub fn is_claude_hook_command(command: &str) -> bool {
    let parts = crate::discover::lexer::shell_split(command);
    let [_binary, hook, claude] = parts.as_slice() else {
        return false;
    };

    // Take the binary's name from the raw text, not from `binary`.
    // `shell_split` applies POSIX unquoting, which consumes the separators in a
    // Windows path -- `C:\...\rtk.exe` arrives here as `C:...rtk.exe`, with
    // nothing left for `rsplit` to find.
    let raw = leading_token(command);
    let binary_name = raw.rsplit(['/', '\\']).next().unwrap_or(raw);

    // `rtk.exe` is what the command reads on Windows.
    (binary_name == "rtk" || binary_name.eq_ignore_ascii_case("rtk.exe"))
        && hook == "hook"
        && claude == "claude"
}

/// The command's first token, with one layer of surrounding quotes removed.
fn leading_token(command: &str) -> &str {
    let command = command.trim_start();
    if let Some(rest) = command.strip_prefix(['"', '\'']) {
        let quote = command.as_bytes()[0] as char;
        return rest.split(quote).next().unwrap_or(rest);
    }
    command.split_whitespace().next().unwrap_or(command)
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
    fn claude_hook_command_matches_windows_exe_and_backslash_path() {
        assert!(is_claude_hook_command("rtk.exe hook claude"));
        assert!(is_claude_hook_command("RTK.EXE hook claude"));
        assert!(is_claude_hook_command(
            r"C:\Users\me\AppData\Local\rtk\bin\rtk.exe hook claude"
        ));
    }

    #[test]
    fn claude_hook_command_rejects_other_commands() {
        assert!(!is_claude_hook_command("not-rtk hook claude"));
        assert!(!is_claude_hook_command("/opt/homebrew/bin/rtk hook cursor"));
        assert!(!is_claude_hook_command("echo rtk hook claude"));
    }
}
