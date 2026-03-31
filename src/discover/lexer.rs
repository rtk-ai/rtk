#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Arg,
    Operator,
    Pipe,
    Redirect,
    Shellism,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedToken {
    pub kind: TokenKind,
    pub value: String,
    pub offset: usize,
}

pub fn tokenize(input: &str) -> Vec<ParsedToken> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut current_start: usize = 0;
    let mut byte_pos: usize = 0;
    let mut chars = input.chars().peekable();
    let mut quote: Option<char> = None;
    let mut escaped = false;

    while let Some(c) = chars.next() {
        let char_len = c.len_utf8();

        if escaped {
            current.push('\\');
            current.push(c);
            byte_pos += char_len;
            escaped = false;
            continue;
        }
        if c == '\\' && quote != Some('\'') {
            escaped = true;
            if current.is_empty() {
                current_start = byte_pos;
            }
            byte_pos += char_len;
            continue;
        }

        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            current.push(c);
            byte_pos += char_len;
            continue;
        }
        if c == '\'' || c == '"' {
            quote = Some(c);
            if current.is_empty() {
                current_start = byte_pos;
            }
            current.push(c);
            byte_pos += char_len;
            continue;
        }

        match c {
            '$' => {
                flush_arg(&mut tokens, &mut current, current_start);
                let start = byte_pos;
                byte_pos += char_len;
                if chars
                    .peek()
                    .is_some_and(|&nc| nc.is_ascii_alphabetic() || nc == '_')
                {
                    let mut name = String::from("$");
                    while let Some(&nc) = chars.peek() {
                        if !nc.is_ascii_alphanumeric() && nc != '_' {
                            break;
                        }
                        chars.next();
                        byte_pos += nc.len_utf8();
                        name.push(nc);
                    }
                    tokens.push(ParsedToken {
                        kind: TokenKind::Arg,
                        value: name,
                        offset: start,
                    });
                } else {
                    tokens.push(ParsedToken {
                        kind: TokenKind::Shellism,
                        value: "$".into(),
                        offset: start,
                    });
                }
                current_start = byte_pos;
            }
            '*' | '?' | '`' | '(' | ')' | '{' | '}' | '!' => {
                flush_arg(&mut tokens, &mut current, current_start);
                tokens.push(ParsedToken {
                    kind: TokenKind::Shellism,
                    value: c.to_string(),
                    offset: byte_pos,
                });
                byte_pos += char_len;
                current_start = byte_pos;
            }
            '|' => {
                flush_arg(&mut tokens, &mut current, current_start);
                let start = byte_pos;
                byte_pos += char_len;
                if chars.peek() == Some(&'|') {
                    chars.next();
                    byte_pos += 1;
                    tokens.push(ParsedToken {
                        kind: TokenKind::Operator,
                        value: "||".into(),
                        offset: start,
                    });
                } else {
                    tokens.push(ParsedToken {
                        kind: TokenKind::Pipe,
                        value: "|".into(),
                        offset: start,
                    });
                }
                current_start = byte_pos;
            }
            ';' => {
                flush_arg(&mut tokens, &mut current, current_start);
                tokens.push(ParsedToken {
                    kind: TokenKind::Operator,
                    value: ";".into(),
                    offset: byte_pos,
                });
                byte_pos += char_len;
                current_start = byte_pos;
            }
            '&' => {
                flush_arg(&mut tokens, &mut current, current_start);
                let start = byte_pos;
                byte_pos += char_len;
                if chars.peek() == Some(&'&') {
                    chars.next();
                    byte_pos += 1;
                    tokens.push(ParsedToken {
                        kind: TokenKind::Operator,
                        value: "&&".into(),
                        offset: start,
                    });
                } else if chars.peek() == Some(&'>') {
                    chars.next();
                    byte_pos += 1;
                    let mut val = String::from("&>");
                    if chars.peek() == Some(&'>') {
                        chars.next();
                        byte_pos += 1;
                        val.push('>');
                    }
                    tokens.push(ParsedToken {
                        kind: TokenKind::Redirect,
                        value: val,
                        offset: start,
                    });
                } else {
                    tokens.push(ParsedToken {
                        kind: TokenKind::Shellism,
                        value: "&".into(),
                        offset: start,
                    });
                }
                current_start = byte_pos;
            }
            '>' => {
                let fd_prefix =
                    if !current.is_empty() && current.chars().all(|ch| ch.is_ascii_digit()) {
                        Some(std::mem::take(&mut current))
                    } else {
                        flush_arg(&mut tokens, &mut current, current_start);
                        None
                    };
                let redir_start = if fd_prefix.is_some() {
                    current_start
                } else {
                    byte_pos
                };
                let mut val = fd_prefix.unwrap_or_default();
                val.push('>');
                byte_pos += char_len;
                if chars.peek() == Some(&'>') {
                    chars.next();
                    byte_pos += 1;
                    val.push('>');
                }
                if chars.peek() == Some(&'&') {
                    chars.next();
                    byte_pos += 1;
                    val.push('&');
                    while let Some(&nc) = chars.peek() {
                        if !nc.is_ascii_digit() && nc != '-' {
                            break;
                        }
                        chars.next();
                        val.push(nc);
                        byte_pos += nc.len_utf8();
                    }
                }
                tokens.push(ParsedToken {
                    kind: TokenKind::Redirect,
                    value: val,
                    offset: redir_start,
                });
                current_start = byte_pos;
            }
            '<' => {
                flush_arg(&mut tokens, &mut current, current_start);
                let start = byte_pos;
                let mut val = String::from("<");
                byte_pos += char_len;
                if chars.peek() == Some(&'<') {
                    chars.next();
                    byte_pos += 1;
                    val.push('<');
                }
                tokens.push(ParsedToken {
                    kind: TokenKind::Redirect,
                    value: val,
                    offset: start,
                });
                current_start = byte_pos;
            }
            c if c.is_whitespace() => {
                flush_arg(&mut tokens, &mut current, current_start);
                byte_pos += c.len_utf8();
                current_start = byte_pos;
            }
            _ => {
                if current.is_empty() {
                    current_start = byte_pos;
                }
                current.push(c);
                byte_pos += char_len;
            }
        }
    }

    if escaped {
        current.push('\\');
    }
    flush_arg(&mut tokens, &mut current, current_start);
    tokens
}

fn flush_arg(tokens: &mut Vec<ParsedToken>, current: &mut String, offset: usize) {
    if !current.is_empty() {
        tokens.push(ParsedToken {
            kind: TokenKind::Arg,
            value: std::mem::take(current),
            offset,
        });
    }
}

pub fn shell_split(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;

    while let Some(c) = chars.next() {
        match c {
            '\\' if !in_single => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            ' ' | '\t' if !in_single && !in_double => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => {
                current.push(c);
            }
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_command() {
        let tokens = tokenize("git status");
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].kind, TokenKind::Arg);
        assert_eq!(tokens[0].value, "git");
        assert_eq!(tokens[1].value, "status");
    }

    #[test]
    fn test_command_with_args() {
        let tokens = tokenize("git commit -m message");
        assert_eq!(tokens.len(), 4);
        assert_eq!(tokens[0].value, "git");
        assert_eq!(tokens[1].value, "commit");
        assert_eq!(tokens[2].value, "-m");
        assert_eq!(tokens[3].value, "message");
    }

    #[test]
    fn test_quoted_operator_not_split() {
        let tokens = tokenize(r#"git commit -m "Fix && Bug""#);
        assert!(!tokens
            .iter()
            .any(|t| matches!(t.kind, TokenKind::Operator) && t.value == "&&"));
        assert!(tokens.iter().any(|t| t.value.contains("Fix && Bug")));
    }

    #[test]
    fn test_single_quoted_string() {
        let tokens = tokenize("echo 'hello world'");
        assert!(tokens.iter().any(|t| t.value == "'hello world'"));
    }

    #[test]
    fn test_double_quoted_string() {
        let tokens = tokenize(r#"echo "hello world""#);
        assert!(tokens.iter().any(|t| t.value == "\"hello world\""));
    }

    #[test]
    fn test_empty_quoted_string() {
        let tokens = tokenize("echo \"\"");
        assert!(tokens.iter().any(|t| t.value == "\"\""));
    }

    #[test]
    fn test_nested_quotes() {
        let tokens = tokenize(r#"echo "outer 'inner' outer""#);
        assert!(tokens.iter().any(|t| t.value.contains("'inner'")));
    }

    #[test]
    fn test_escaped_space() {
        let tokens = tokenize("echo hello\\ world");
        assert!(tokens.iter().any(|t| t.value.contains("hello")));
    }

    #[test]
    fn test_backslash_in_single_quotes() {
        let tokens = tokenize(r#"echo 'hello\nworld'"#);
        assert!(tokens.iter().any(|t| t.value.contains(r"\n")));
    }

    #[test]
    fn test_escaped_quote_in_double() {
        let tokens = tokenize(r#"echo "hello\"world""#);
        assert!(tokens.iter().any(|t| t.value.contains("hello")));
    }

    #[test]
    fn test_empty_input() {
        assert!(tokenize("").is_empty());
    }

    #[test]
    fn test_whitespace_only() {
        assert!(tokenize("   ").is_empty());
    }

    #[test]
    fn test_unclosed_single_quote() {
        let tokens = tokenize("'unclosed");
        assert!(!tokens.is_empty());
    }

    #[test]
    fn test_unclosed_double_quote() {
        let tokens = tokenize("\"unclosed");
        assert!(!tokens.is_empty());
    }

    #[test]
    fn test_unicode_preservation() {
        let tokens = tokenize("echo \"héllo wörld\"");
        assert!(tokens.iter().any(|t| t.value.contains("héllo")));
    }

    #[test]
    fn test_multiple_spaces() {
        let tokens = tokenize("git   status");
        assert_eq!(tokens.len(), 2);
    }

    #[test]
    fn test_leading_trailing_spaces() {
        let tokens = tokenize("  git status  ");
        assert_eq!(tokens.len(), 2);
    }

    #[test]
    fn test_and_operator() {
        let tokens = tokenize("cmd1 && cmd2");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Operator && t.value == "&&"));
    }

    #[test]
    fn test_or_operator() {
        let tokens = tokenize("cmd1 || cmd2");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Operator && t.value == "||"));
    }

    #[test]
    fn test_semicolon() {
        let tokens = tokenize("cmd1 ; cmd2");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Operator && t.value == ";"));
    }

    #[test]
    fn test_multiple_and() {
        let tokens = tokenize("a && b && c");
        let ops: Vec<_> = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Operator)
            .collect();
        assert_eq!(ops.len(), 2);
    }

    #[test]
    fn test_mixed_operators() {
        let tokens = tokenize("a && b || c");
        let ops: Vec<_> = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Operator)
            .collect();
        assert_eq!(ops.len(), 2);
    }

    #[test]
    fn test_operator_at_start() {
        let tokens = tokenize("&& cmd");
        assert!(tokens.iter().any(|t| t.value == "&&"));
    }

    #[test]
    fn test_operator_at_end() {
        let tokens = tokenize("cmd &&");
        assert!(tokens.iter().any(|t| t.value == "&&"));
    }

    #[test]
    fn test_pipe_detection() {
        let tokens = tokenize("cat file | grep pattern");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Pipe));
    }

    #[test]
    fn test_quoted_pipe_not_pipe() {
        let tokens = tokenize("\"a|b\"");
        assert!(!tokens.iter().any(|t| t.kind == TokenKind::Pipe));
    }

    #[test]
    fn test_multiple_pipes() {
        let tokens = tokenize("a | b | c");
        let pipes: Vec<_> = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Pipe)
            .collect();
        assert_eq!(pipes.len(), 2);
    }

    #[test]
    fn test_glob_detection() {
        let tokens = tokenize("ls *.rs");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_quoted_glob_not_shellism() {
        let tokens = tokenize("echo \"*.txt\"");
        assert!(!tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_simple_var_is_arg() {
        let tokens = tokenize("echo $HOME");
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Arg && t.value == "$HOME"),
            "Simple $VAR must be Arg — shell expands at execution time"
        );
        assert!(
            !tokens.iter().any(|t| t.kind == TokenKind::Shellism),
            "No Shellism expected for simple $VAR"
        );
    }

    #[test]
    fn test_simple_var_enables_native_routing() {
        let tokens = tokenize("git log $BRANCH");
        assert!(
            !tokens.iter().any(|t| t.kind == TokenKind::Shellism),
            "git log $BRANCH must have no Shellism"
        );
    }

    #[test]
    fn test_dollar_subshell_stays_shellism() {
        let tokens = tokenize("echo $(date)");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_dollar_brace_stays_shellism() {
        let tokens = tokenize("echo ${HOME}");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_dollar_special_vars_stay_shellism() {
        for s in &["echo $?", "echo $$", "echo $!"] {
            let tokens = tokenize(s);
            assert!(
                tokens.iter().any(|t| t.kind == TokenKind::Shellism),
                "{} should produce Shellism",
                s
            );
        }
    }

    #[test]
    fn test_dollar_digit_stays_shellism() {
        let tokens = tokenize("echo $1");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_quoted_variable_not_shellism() {
        let tokens = tokenize("echo \"$HOME\"");
        assert!(!tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_backtick_substitution() {
        let tokens = tokenize("echo `date`");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_subshell_detection() {
        let tokens = tokenize("echo $(date)");
        let shellisms: Vec<_> = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Shellism)
            .collect();
        assert!(!shellisms.is_empty());
    }

    #[test]
    fn test_brace_expansion() {
        let tokens = tokenize("echo {a,b}.txt");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_escaped_glob() {
        let tokens = tokenize("echo \\*.txt");
        assert!(!tokens
            .iter()
            .any(|t| t.kind == TokenKind::Shellism && t.value == "*"));
    }

    #[test]
    fn test_redirect_out() {
        let tokens = tokenize("cmd > file");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Redirect));
    }

    #[test]
    fn test_redirect_append() {
        let tokens = tokenize("cmd >> file");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == ">>"));
    }

    #[test]
    fn test_redirect_in() {
        let tokens = tokenize("cmd < file");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Redirect));
    }

    #[test]
    fn test_redirect_stderr() {
        let tokens = tokenize("cmd 2> file");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value.starts_with("2>")));
    }

    #[test]
    fn test_redirect_stderr_no_space() {
        let tokens = tokenize("cmd 2>/dev/null");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "2>"));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Arg && t.value == "/dev/null"));
    }

    #[test]
    fn test_redirect_dev_null() {
        let tokens = tokenize("cmd > /dev/null");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == ">"));
    }

    #[test]
    fn test_redirect_2_to_1_single_token() {
        let tokens = tokenize("cmd 2>&1");
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[1].kind, TokenKind::Redirect);
        assert_eq!(tokens[1].value, "2>&1");
        assert!(!tokens
            .iter()
            .any(|t| t.kind == TokenKind::Shellism && t.value == "&"));
    }

    #[test]
    fn test_redirect_1_to_2_single_token() {
        let tokens = tokenize("cmd 1>&2");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "1>&2"));
    }

    #[test]
    fn test_redirect_fd_close() {
        let tokens = tokenize("cmd 2>&-");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "2>&-"));
    }

    #[test]
    fn test_redirect_shorthand_dup() {
        let tokens = tokenize("cmd >&2");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == ">&2"));
    }

    #[test]
    fn test_redirect_amp_gt() {
        let tokens = tokenize("cmd &>/dev/null");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "&>"));
    }

    #[test]
    fn test_redirect_amp_gt_gt() {
        let tokens = tokenize("cmd &>>/dev/null");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "&>>"));
    }

    #[test]
    fn test_combined_redirect_chain() {
        let tokens = tokenize("cmd > /dev/null 2>&1");
        let redirects: Vec<_> = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Redirect)
            .collect();
        assert_eq!(redirects.len(), 2);
        assert_eq!(redirects[0].value, ">");
        assert_eq!(redirects[1].value, "2>&1");
    }

    #[test]
    fn test_redirect_append_to_file() {
        let tokens = tokenize("echo hello >> /tmp/output.txt");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == ">>"));
    }

    #[test]
    fn test_redirect_heredoc_marker() {
        let tokens = tokenize("cat <<EOF");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "<<"));
    }

    #[test]
    fn test_redirect_2_to_1_with_pipe() {
        let tokens = tokenize("cargo test 2>&1 | head");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "2>&1"));
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Pipe));
    }

    #[test]
    fn test_redirect_2_to_1_with_and() {
        let tokens = tokenize("cargo test 2>&1 && echo done");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "2>&1"));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Operator && t.value == "&&"));
    }

    #[test]
    fn test_exclamation_is_shellism() {
        let tokens = tokenize("if ! grep -q pattern file; then echo missing; fi");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Shellism && t.value == "!"));
    }

    #[test]
    fn test_background_job_is_shellism() {
        let tokens = tokenize("sleep 10 &");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Shellism && t.value == "&"));
    }

    #[test]
    fn test_background_not_confused_with_amp_redirect() {
        let tokens = tokenize("cargo test &>/dev/null");
        assert!(!tokens
            .iter()
            .any(|t| t.kind == TokenKind::Shellism && t.value == "&"));
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Redirect));
    }

    #[test]
    fn test_semicolon_no_space() {
        let tokens = tokenize("git status;cargo test");
        assert_eq!(
            tokens
                .iter()
                .filter(|t| t.kind == TokenKind::Operator)
                .count(),
            1
        );
        assert_eq!(
            tokens.iter().filter(|t| t.kind == TokenKind::Arg).count(),
            4
        );
    }

    #[test]
    fn test_offset_tracking() {
        let tokens = tokenize("a && b");
        assert_eq!(tokens[0].offset, 0);
        assert_eq!(tokens[1].offset, 2);
        assert_eq!(tokens[2].offset, 5);
    }

    #[test]
    fn test_offset_segment_extraction() {
        let cmd = "git add . && cargo test";
        let tokens = tokenize(cmd);
        let op = tokens
            .iter()
            .find(|t| t.kind == TokenKind::Operator)
            .unwrap();
        let left = cmd[..op.offset].trim();
        let right_start = op.offset + op.value.len();
        let right = cmd[right_start..].trim();
        assert_eq!(left, "git add .");
        assert_eq!(right, "cargo test");
    }

    #[test]
    fn test_env_prefix_is_arg() {
        let tokens = tokenize("GIT_SSH_COMMAND=ssh git push");
        assert_eq!(tokens[0].kind, TokenKind::Arg);
        assert_eq!(tokens[0].value, "GIT_SSH_COMMAND=ssh");
    }

    #[test]
    fn test_complex_compound() {
        let tokens = tokenize("cargo fmt --all && cargo clippy --all-targets && cargo test");
        let operators: Vec<_> = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Operator)
            .collect();
        assert_eq!(operators.len(), 2);
        assert!(operators.iter().all(|t| t.value == "&&"));
    }

    #[test]
    fn test_find_pipe_xargs() {
        let tokens = tokenize("find . -name '*.rs' | xargs grep 'fn run'");
        let pipe_idx = tokens
            .iter()
            .position(|t| t.kind == TokenKind::Pipe)
            .unwrap();
        assert!(pipe_idx > 0);
        let before_pipe: Vec<_> = tokens[..pipe_idx]
            .iter()
            .filter(|t| t.kind == TokenKind::Arg)
            .collect();
        assert!(before_pipe.iter().any(|t| t.value == "find"));
    }

    #[test]
    fn test_fd_redirect_needs_adjacent_digit() {
        let tokens = tokenize("echo 2 > file");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Arg && t.value == "2"));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == ">"));
    }

    #[test]
    fn test_fd_redirect_no_space() {
        let tokens = tokenize("echo 2>file");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "2>"));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Arg && t.value == "file"));
    }

    #[test]
    fn test_shell_split_simple() {
        assert_eq!(
            shell_split("head -50 file.php"),
            vec!["head", "-50", "file.php"]
        );
    }

    #[test]
    fn test_shell_split_double_quotes() {
        assert_eq!(
            shell_split(r#"git log --format="%H %s""#),
            vec!["git", "log", "--format=%H %s"]
        );
    }

    #[test]
    fn test_shell_split_single_quotes() {
        assert_eq!(
            shell_split("grep -r 'hello world' ."),
            vec!["grep", "-r", "hello world", "."]
        );
    }

    #[test]
    fn test_shell_split_single_word() {
        assert_eq!(shell_split("ls"), vec!["ls"]);
    }

    #[test]
    fn test_shell_split_empty() {
        let result: Vec<String> = shell_split("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_shell_split_backslash_escape() {
        assert_eq!(
            shell_split(r"echo hello\ world"),
            vec!["echo", "hello world"]
        );
    }

    #[test]
    fn test_shell_split_unclosed_quote() {
        let result = shell_split("echo 'hello");
        assert_eq!(result, vec!["echo", "hello"]);
    }

    #[test]
    fn test_shell_split_mixed_quotes() {
        assert_eq!(
            shell_split(r#"echo "it's" 'a "test"'"#),
            vec!["echo", "it's", "a \"test\""]
        );
    }

    #[test]
    fn test_shell_split_tabs() {
        assert_eq!(shell_split("a\tb\tc"), vec!["a", "b", "c"]);
    }

    #[test]
    fn test_shell_split_multiple_spaces() {
        assert_eq!(shell_split("a   b   c"), vec!["a", "b", "c"]);
    }
}
