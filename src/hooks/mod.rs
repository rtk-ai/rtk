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
pub mod permissions_opencode;
#[deny(clippy::print_stdout, clippy::print_stderr)]
#[cfg(feature = "engine")]
pub mod posthook;
pub mod rewrite_cmd;
#[deny(clippy::print_stdout, clippy::print_stderr)]
#[cfg(feature = "engine")]
pub mod session_start;
pub mod trust;
pub mod verify_cmd;

fn is_rtk_binary(binary: &str) -> bool {
    let binary_name = binary.rsplit(['/', '\\']).next().unwrap_or(binary);
    matches!(binary_name, "rtk" | "rtk.exe")
}

/// Claims an entry only when it is literally `<path>/rtk hook <agent>`. Words
/// may hold literal text and `$NAME` / `${NAME}`, quoted or not; any other
/// shell syntax (`(`, backticks, `$(`, `*`, `?`, `!`, ANSI-C quoting...)
/// makes it refuse.
/// Refusing costs a duplicate RTK entry on install, while a wrong claim lets
/// uninstall delete the user's own hook, so the matcher lists what it accepts
/// rather than what it rejects.
fn is_rtk_hook_command(command: &str, agent: &str) -> bool {
    if crate::discover::lexer::ansi_c_quote_defeats_lexer(command) {
        return false;
    }
    let command = strip_comments_and_line_continuations(command);
    let command = command.as_str();
    let mut tokens = crate::discover::lexer::tokenize_with_newlines(command);
    // Ignore only literal LF operator tokens at the command's boundaries.
    // The lexer also reports the CR of a CRLF pair as an LF operator; checking
    // the source byte leaves that token for the guard below, and the word check
    // after shell_split would reject the CR anyway.
    while tokens.first().is_some_and(|token| {
        matches!(token.kind, crate::discover::lexer::TokenKind::Operator)
            && token.value == "\n"
            && command.as_bytes().get(token.offset) == Some(&b'\n')
    }) {
        tokens.remove(0);
    }
    while tokens.last().is_some_and(|token| {
        matches!(token.kind, crate::discover::lexer::TokenKind::Operator)
            && token.value == "\n"
            && command.as_bytes().get(token.offset) == Some(&b'\n')
    }) {
        tokens.pop();
    }
    if has_shell_syntax(&tokens) {
        return false;
    }

    let parts = crate::discover::lexer::shell_split(command);
    let [parsed_binary, hook, target] = parts.as_slice() else {
        return false;
    };

    is_rtk_binary(parsed_binary) && hook == "hook" && target == agent
}

/// Drops what bash removes before it splits words: a backslash-newline pair
/// outside single quotes, and a comment that an unquoted `#` opens at the start
/// of a word, up to the end of its line. A backslash inside a comment does not
/// continue it.
fn strip_comments_and_line_continuations(command: &str) -> String {
    let mut stripped = String::with_capacity(command.len());
    let mut chars = command.chars().peekable();
    let mut quote = None;
    let mut word_start = true;
    while let Some(c) = chars.next() {
        if c == '\\' && quote != Some('\'') {
            match chars.next() {
                Some('\n') => {}
                Some(escaped) => {
                    stripped.push(c);
                    stripped.push(escaped);
                    word_start = false;
                }
                None => stripped.push(c),
            }
            continue;
        }
        if c == '#' && quote.is_none() && word_start {
            while chars.next_if(|&next| next != '\n').is_some() {}
            continue;
        }
        word_start = quote.is_none() && crate::discover::lexer::is_word_boundary_whitespace(c);
        quote = crate::discover::lexer::advance_quote_state(quote, c);
        stripped.push(c);
    }
    stripped
}

fn has_shell_syntax(tokens: &[crate::discover::lexer::ParsedToken]) -> bool {
    use crate::discover::lexer::TokenKind;

    let mut i = 0;
    while let Some(token) = tokens.get(i) {
        match token.kind {
            TokenKind::Arg if has_expansion_in_double_quotes(&token.value) => return true,
            TokenKind::Arg => i += 1,
            TokenKind::Shellism if is_braced_variable(&tokens[i..]) => i += 4,
            _ => return true,
        }
    }
    false
}

/// The lexer returns a double-quoted span as one word, but bash still expands
/// `$` and backticks inside it, so the same rule applies there.
fn has_expansion_in_double_quotes(word: &str) -> bool {
    let mut chars = word.chars().peekable();
    let mut quote = None;
    while let Some(c) = chars.next() {
        if c == '\\' && quote != Some('\'') {
            chars.next();
            continue;
        }
        if quote == Some('"') && (c == '`' || (c == '$' && !skip_variable_name(&mut chars))) {
            return true;
        }
        quote = crate::discover::lexer::advance_quote_state(quote, c);
    }
    false
}

/// Consumes `NAME` or `{NAME}` after a `$`; false when neither follows.
fn skip_variable_name(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> bool {
    let braced = chars.next_if_eq(&'{').is_some();
    if chars
        .next_if(|c| c.is_ascii_alphabetic() || *c == '_')
        .is_none()
    {
        return false;
    }
    while chars
        .next_if(|c| c.is_ascii_alphanumeric() || *c == '_')
        .is_some()
    {}
    !braced || chars.next_if_eq(&'}').is_some()
}

/// `${NAME}` arrives as four adjacent tokens: `$`, `{`, the name and `}`.
fn is_braced_variable(tokens: &[crate::discover::lexer::ParsedToken]) -> bool {
    use crate::discover::lexer::TokenKind;

    let [dollar, open, name, close, ..] = tokens else {
        return false;
    };
    let mut name_chars = name.value.chars();
    let is_name = name_chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name_chars.all(|c| c.is_ascii_alphanumeric() || c == '_');

    dollar.kind == TokenKind::Shellism
        && dollar.value == "$"
        && open.kind == TokenKind::Shellism
        && open.value == "{"
        && open.offset == dollar.offset + 1
        && name.kind == TokenKind::Arg
        && name.offset == open.offset + 1
        && is_name
        && close.kind == TokenKind::Shellism
        && close.value == "}"
        && close.offset == name.offset + name.value.len()
}

pub fn is_claude_hook_command(command: &str) -> bool {
    is_rtk_hook_command(command, "claude")
}

pub fn is_legacy_rewrite_hook(command: &str) -> bool {
    let parts = crate::discover::lexer::shell_split(command);
    let [word] = parts.as_slice() else {
        return false;
    };

    let filename = word.rsplit(['/', '\\']).next().unwrap_or(word);
    filename == constants::REWRITE_HOOK_FILE
}

#[cfg(feature = "engine")]
static HOOK_AGENT: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

#[cfg(feature = "engine")]
pub(crate) fn set_hook_agent(agent: &str) {
    let _ = HOOK_AGENT.set(Some(agent.to_string()));
}

#[cfg(feature = "engine")]
pub(crate) fn hook_agent() -> Option<String> {
    HOOK_AGENT.get().cloned().flatten()
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
    fn legacy_rewrite_hook_matches_direct_invocations() {
        assert!(is_legacy_rewrite_hook("rtk-rewrite.sh"));
        assert!(is_legacy_rewrite_hook("~/.claude/hooks/rtk-rewrite.sh"));
        assert!(is_legacy_rewrite_hook("/path/to/rtk-rewrite.sh"));
        assert!(is_legacy_rewrite_hook("./hooks/rtk-rewrite.sh"));
        assert!(is_legacy_rewrite_hook("\"/path/to/rtk-rewrite.sh\""));
        assert!(is_legacy_rewrite_hook(
            "\"/Users/John Doe/.claude/hooks/rtk-rewrite.sh\""
        ));
        assert!(is_legacy_rewrite_hook(
            "'C:\\Users\\test\\.claude\\hooks\\rtk-rewrite.sh'"
        ));
        assert!(is_legacy_rewrite_hook(
            "C:/Users/test/.claude/hooks/rtk-rewrite.sh"
        ));
    }

    #[test]
    fn legacy_rewrite_hook_rejects_non_invocations() {
        assert!(!is_legacy_rewrite_hook("echo rtk-rewrite.sh"));
        assert!(!is_legacy_rewrite_hook("/custom/rtk-rewrite.sh.backup"));
        assert!(!is_legacy_rewrite_hook(
            "python3 /custom/audit.py --label rtk-rewrite.sh"
        ));
        assert!(!is_legacy_rewrite_hook(
            "audit.sh --mirror ~/.claude/hooks/rtk-rewrite.sh"
        ));
        assert!(!is_legacy_rewrite_hook(
            "~/.claude/hooks/rtk-rewrite.sh && logger -t audit done"
        ));
        assert!(!is_legacy_rewrite_hook(
            "~/.claude/hooks/rtk-rewrite.sh | tee -a /tmp/audit.log"
        ));
        assert!(!is_legacy_rewrite_hook(
            "~/.claude/hooks/rtk-rewrite.sh --flag"
        ));
        assert!(!is_legacy_rewrite_hook("/custom/my-rtk-rewrite.sh"));
        assert!(!is_legacy_rewrite_hook(""));
    }
}
