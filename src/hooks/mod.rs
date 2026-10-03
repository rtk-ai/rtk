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

/// `rtk hook <agent>` with any of rtk's own flags that leave the hook as it is:
/// `--ultra-compact` and `--skip-env` (global, so before or after the subcommand)
/// and `-v`/`--verbose`.
fn is_rtk_hook_command(command: &str, agent: &str) -> bool {
    use crate::core::arg_tokenizer::{self, TokenKind};

    let parts = crate::discover::lexer::shell_split(command);
    let Some((parsed_binary, args)) = parts.split_first() else {
        return false;
    };
    if !is_rtk_binary(parsed_binary) {
        return false;
    }

    let mut positionals = Vec::new();
    for token in arg_tokenizer::tokenize(args) {
        match token.kind {
            TokenKind::Long
                if token.attached.is_none()
                    && matches!(token.text, "ultra-compact" | "skip-env" | "verbose") => {}
            TokenKind::Short if token.text == "v" => {}
            TokenKind::Positional => positionals.push(token.text),
            _ => return false,
        }
    }
    positionals == ["hook", agent]
}

pub fn is_claude_hook_command(command: &str) -> bool {
    is_rtk_hook_command(command, "claude")
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
        assert!(!is_claude_hook_command("rtk hook claude extra"));
        assert!(!is_claude_hook_command("rtk hook claude --unknown"));
        assert!(!is_claude_hook_command("rtk hook claude --ultra-compact=1"));
        assert!(!is_claude_hook_command("rtk hook -- claude"));
        assert!(!is_claude_hook_command("rtk hook"));
    }

    #[test]
    fn claude_hook_command_accepts_rtk_flags() {
        assert!(is_claude_hook_command("rtk hook claude --ultra-compact"));
        assert!(is_claude_hook_command("rtk hook claude --skip-env"));
        assert!(is_claude_hook_command(
            "rtk hook claude --ultra-compact --skip-env"
        ));
        assert!(is_claude_hook_command("rtk --ultra-compact hook claude"));
        assert!(is_claude_hook_command("rtk -vv hook claude"));
        assert!(is_claude_hook_command(
            "/usr/local/bin/rtk --skip-env hook claude --verbose"
        ));
    }
}
