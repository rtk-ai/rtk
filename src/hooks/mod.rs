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
    let parts = crate::discover::lexer::shell_split(command);
    let [binary, hook, claude] = parts.as_slice() else {
        return false;
    };

    let binary_name = binary.rsplit(['/', '\\']).next().unwrap_or(binary);

    binary_name == "rtk" && hook == "hook" && claude == "claude"
}

/// Broader match used only by wrapper-script detection (`hook_check`).
/// Matches `rtk hook claude` after skipping any wrapper prefix (interpreter,
/// `env`, flags, `KEY=VALUE` assignments — see [`is_wrapper_prefix_token`]),
/// or — for the inline `sh -c "rtk hook claude"` form — a single leftover
/// token that itself splits into that invocation.
fn is_claude_hook_invocation(command: &str) -> bool {
    is_claude_hook_invocation_parts(&crate::discover::lexer::shell_split(command))
}

fn is_claude_hook_invocation_parts(parts: &[String]) -> bool {
    match skip_wrapper_prefix(parts) {
        [binary, hook, claude] => {
            hook == "hook"
                && claude == "claude"
                && binary.rsplit(['/', '\\']).next().unwrap_or(binary) == "rtk"
        }
        [single] => {
            let inner = crate::discover::lexer::shell_split(single);
            inner.len() > 1 && is_claude_hook_invocation_parts(&inner)
        }
        _ => false,
    }
}

/// Shell interpreters skipped when locating the real command/script inside a
/// wrapper invocation (`bash script.sh`, `env KEY=val rtk hook claude`, ...).
const WRAPPER_INTERPRETERS: &[&str] = &["bash", "sh", "zsh"];

/// `KEY=VALUE` env-assignment shape (bare identifier on the left of `=`).
static ENV_ASSIGNMENT_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*=").unwrap());

/// True for tokens that only route to the real command and are never
/// themselves the thing being located: shell flags, `KEY=VALUE` env
/// assignments, unresolvable `$VAR`-prefixed paths, and interpreter/`env`
/// binaries (matched by file name so `/bin/bash` and `bash` both skip).
fn is_wrapper_prefix_token(token: &str) -> bool {
    if token.starts_with('-') || token.starts_with('$') || ENV_ASSIGNMENT_RE.is_match(token) {
        return true;
    }
    let name = std::path::Path::new(token)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(token);
    name == "env" || WRAPPER_INTERPRETERS.contains(&name)
}

/// Strip a leading run of wrapper-prefix tokens (see [`is_wrapper_prefix_token`]).
fn skip_wrapper_prefix(parts: &[String]) -> &[String] {
    let skip = parts
        .iter()
        .take_while(|t| is_wrapper_prefix_token(t))
        .count();
    &parts[skip..]
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
    fn claude_hook_command_rejects_wrapper_forms() {
        // Wrapper forms are matched by `is_claude_hook_invocation` only —
        // `is_claude_hook_command` stays exact-3-token so uninstall/detection
        // callers (init.rs, integrity.rs) never treat a wrapper entry as the
        // direct hook and delete or mis-detect it.
        assert!(!is_claude_hook_command(r#"bash -c "rtk hook claude""#));
        assert!(!is_claude_hook_command("bash rtk hook claude"));
        assert!(!is_claude_hook_command("env RTK_LOG=1 rtk hook claude"));
    }

    #[test]
    fn hook_invocation_matches_inline_wrapper_form() {
        // `sh -c "rtk hook claude"` — the quoted command is a single token
        // after shell_split; must be re-split and matched.
        assert!(is_claude_hook_invocation(r#"bash -c "rtk hook claude""#));
        assert!(is_claude_hook_invocation("sh -c 'rtk hook claude'"));
    }

    #[test]
    fn hook_invocation_matches_env_and_interpreter_prefix() {
        assert!(is_claude_hook_invocation("bash rtk hook claude"));
        assert!(is_claude_hook_invocation("/bin/bash rtk hook claude"));
        assert!(is_claude_hook_invocation("env RTK_LOG=1 rtk hook claude"));
    }

    #[test]
    fn hook_invocation_rejects_malformed_env_assignment_prefix() {
        // "9INVALID=1" isn't a valid `KEY=VALUE` shape (identifiers can't
        // start with a digit) — must not be skipped as an env-assignment
        // wrapper-prefix token.
        assert!(!is_claude_hook_invocation("9INVALID=1 rtk hook claude"));
        assert!(is_claude_hook_invocation("RTK_LOG=1 rtk hook claude"));
    }
}
