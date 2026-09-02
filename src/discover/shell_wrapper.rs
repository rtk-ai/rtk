//! Parses the conservative subset of quoted shell -c wrappers that RTK can rewrite.

/// POSIX-family shells only. `fish` needs a fish-aware lexer to attest its
/// control keywords (`and`, `or`, `if`) at a command boundary, which the shared
/// lexer does not model, so a fish script is never treated as rewritable.
const SUPPORTED_SHELLS: &[&str] = &["sh", "bash", "zsh"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShellWrapper {
    script_start: usize,
    script_end: usize,
}

impl ShellWrapper {
    pub(crate) fn script<'a>(&self, command: &'a str) -> Option<&'a str> {
        command.get(self.script_start..self.script_end)
    }

    pub(crate) fn replace_script(&self, command: &str, rewritten: &str) -> Option<String> {
        let prefix = command.get(..self.script_start)?;
        let suffix = command.get(self.script_end..)?;
        let mut result = String::with_capacity(prefix.len() + rewritten.len() + suffix.len());
        result.push_str(prefix);
        result.push_str(rewritten);
        result.push_str(suffix);
        Some(result)
    }
}

/// Recognize a quoted shell -c script without decoding or re-quoting it.
///
/// Double-quoted scripts are accepted only when the outer shell has no active
/// expansion or escape to evaluate. Unsupported forms return None, leaving the
/// original command untouched.
pub(crate) fn parse_shell_wrapper(command: &str) -> Option<ShellWrapper> {
    let bytes = command.as_bytes();
    if bytes.contains(&b'\0') {
        return None;
    }
    let shell_end = supported_shell_end(command)?;

    let mut position = skip_horizontal_space(bytes, shell_end);
    if bytes.get(position..position + 2)? != b"-c" {
        return None;
    }
    position += 2;
    if !bytes
        .get(position)
        .is_some_and(|byte| is_horizontal_space(*byte))
    {
        return None;
    }

    position = skip_horizontal_space(bytes, position);
    let quote = *bytes.get(position)?;
    if !matches!(quote, b'\'' | b'"') {
        return None;
    }

    let script_start = position + 1;
    let script_end = find_script_end(bytes, script_start, quote)?;
    if script_start == script_end {
        return None;
    }

    let after_quote = script_end + 1;
    if bytes
        .get(after_quote)
        .is_some_and(|byte| !is_horizontal_space(*byte))
    {
        return None;
    }

    Some(ShellWrapper {
        script_start,
        script_end,
    })
}

/// Detect a supported shell whose option list requests command-string mode,
/// including forms that the strict rewrite parser intentionally rejects.
pub(crate) fn is_shell_wrapper_candidate(command: &str) -> bool {
    let Some(shell_end) = supported_shell_end(command) else {
        return false;
    };
    let rest_start = skip_horizontal_space(command.as_bytes(), shell_end);
    let Some(rest) = command.get(rest_start..) else {
        return false;
    };

    for option in rest.split_ascii_whitespace() {
        if option == "--" || !option.starts_with('-') {
            return false;
        }
        if option == "--command"
            || option
                .strip_prefix('-')
                .is_some_and(|flags| !flags.starts_with('-') && flags.contains('c'))
        {
            return true;
        }
    }
    false
}

fn supported_shell_end(command: &str) -> Option<usize> {
    let shell_end = command
        .as_bytes()
        .iter()
        .position(|byte| is_horizontal_space(*byte))?;
    let shell = command.get(..shell_end)?;
    let basename = shell.rsplit(['/', '\\']).next()?;
    SUPPORTED_SHELLS.contains(&basename).then_some(shell_end)
}

fn is_horizontal_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t')
}

fn skip_horizontal_space(bytes: &[u8], mut position: usize) -> usize {
    while bytes
        .get(position)
        .is_some_and(|byte| is_horizontal_space(*byte))
    {
        position += 1;
    }
    position
}

fn find_script_end(bytes: &[u8], start: usize, quote: u8) -> Option<usize> {
    for (offset, byte) in bytes.get(start..)?.iter().copied().enumerate() {
        if byte == quote {
            return Some(start + offset);
        }
        if matches!(byte, b'\0' | b'\n' | b'\r') {
            return None;
        }
        if quote == b'"' && matches!(byte, b'$' | b'\x60' | b'\\') {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed_script(command: &str) -> Option<&str> {
        parse_shell_wrapper(command)?.script(command)
    }

    #[test]
    fn test_parse_shell_wrapper_double_quoted_bash() {
        assert_eq!(
            parsed_script(r#"bash -c "head foo && grep -R bar .""#),
            Some("head foo && grep -R bar .")
        );
    }

    #[test]
    fn test_parse_shell_wrapper_rejects_fish() {
        assert_eq!(parsed_script("fish -c 'git status; cargo test'"), None);
    }

    #[test]
    fn test_parse_shell_wrapper_preserves_unicode_and_suffix() {
        let command = "/bin/bash -c 'printf 日本語' command-name 'a b'";
        let wrapper = parse_shell_wrapper(command).expect("wrapper should parse");
        assert_eq!(wrapper.script(command), Some("printf 日本語"));
        assert_eq!(
            wrapper.replace_script(command, "rtk printf 日本語"),
            Some("/bin/bash -c 'rtk printf 日本語' command-name 'a b'".into())
        );
    }

    #[test]
    fn test_parse_shell_wrapper_accepts_inner_expansion_in_single_quotes() {
        assert_eq!(
            parsed_script("bash -c 'printf \"%s\\n\" \"$HOME\"'"),
            Some("printf \"%s\\n\" \"$HOME\"")
        );
    }

    #[test]
    fn test_parse_shell_wrapper_rejects_unsupported_shapes() {
        for command in [
            "bash -c git status",
            "bash -c ''",
            "bash -c 'git status",
            "bash -c 'git'\" status\"",
            "bash -lc 'git status'",
            "bash -e -c 'git status'",
            "python -c 'git status'",
            "command bash -c 'git status'",
            "bash\n-c 'git status'",
            "bash -c 'git\0status'",
        ] {
            assert!(
                parse_shell_wrapper(command).is_none(),
                "unsupported wrapper must be rejected: {command:?}"
            );
        }
    }

    #[test]
    fn test_parse_shell_wrapper_spans_are_valid_for_malformed_corpus() {
        let corpus = [
            "",
            "日",
            "bash",
            "bash ",
            "bash -",
            "bash -c",
            "bash -c ",
            "bash -c '",
            "bash -c '日",
            "bash -c '日本語'",
            "bash -c \"日本語\"",
        ];

        for input in corpus {
            for end in input
                .char_indices()
                .map(|(index, _)| index)
                .chain(std::iter::once(input.len()))
            {
                let candidate = &input[..end];
                if let Some(wrapper) = parse_shell_wrapper(candidate) {
                    assert!(wrapper.script(candidate).is_some());
                    assert!(wrapper
                        .replace_script(candidate, "rtk git status")
                        .is_some());
                }
            }
        }

        let long_script = format!("bash -c '{}'", "日".repeat(32_768));
        let wrapper = parse_shell_wrapper(&long_script).expect("long script should parse");
        assert_eq!(
            wrapper
                .script(&long_script)
                .map(|script| script.chars().count()),
            Some(32_768)
        );
        assert!(wrapper
            .replace_script(&long_script, "rtk git status")
            .is_some());
    }

    #[test]
    fn test_parse_shell_wrapper_rejects_active_double_quote_expansion() {
        for command in [
            r#"bash -c "git status $HOME""#,
            r#"bash -c "git status $(whoami)""#,
            "bash -c \"git status \x60whoami\x60\"",
            r#"bash -c "printf \"escaped\"""#,
        ] {
            assert!(
                parse_shell_wrapper(command).is_none(),
                "active outer expansion must be rejected: {command:?}"
            );
        }
    }

    #[test]
    fn test_shell_wrapper_candidate_includes_unsupported_command_options() {
        for command in [
            "bash -c 'git status'",
            "bash -lc 'git status'",
            "bash -e -c 'git status'",
            "/bin/zsh -fc 'git status'",
        ] {
            assert!(
                is_shell_wrapper_candidate(command),
                "command-string wrapper must be recognized: {command:?}"
            );
        }
        for command in [
            "bash script.sh",
            "python -c 'git status'",
            "bash -- script.sh",
            "fish --command 'git status'",
        ] {
            assert!(
                !is_shell_wrapper_candidate(command),
                "non-wrapper must not be recognized: {command:?}"
            );
        }
    }
}
