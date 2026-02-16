//! State-machine lexer that respects quotes and escapes.
//! Critical: `git commit -m "Fix && Bug"` must NOT split on &&

#[derive(Debug, PartialEq, Clone)]
pub enum TokenKind {
    Arg,      // Regular argument
    Operator, // &&, ||, ;
    Pipe,     // |
    Redirect, // >, >>, <, 2>
    Shellism, // *, $, `, (, ), {, } - forces passthrough
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedToken {
    pub kind: TokenKind,
    pub value: String, // The actual string value
}

/// Tokenize input with quote awareness.
/// Returns Vec of parsed tokens.
pub fn tokenize(input: &str) -> Vec<ParsedToken> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();

    let mut quote: Option<char> = None; // None, Some('\''), Some('"')
    let mut escaped = false;

    while let Some(c) = chars.next() {
        // Handle escape sequences (but NOT inside single quotes)
        if escaped {
            current.push(c);
            escaped = false;
            continue;
        }
        if c == '\\' && quote != Some('\'') {
            escaped = true;
            current.push(c);
            continue;
        }

        // Handle quotes
        if let Some(q) = quote {
            if c == q {
                quote = None; // Close quote
            }
            current.push(c);
            continue;
        }
        if c == '\'' || c == '"' {
            quote = Some(c);
            current.push(c);
            continue;
        }

        // Outside quotes - handle operators and shellisms
        match c {
            // Shellisms force passthrough (includes ! for history expansion/negation)
            '*' | '?' | '$' | '`' | '(' | ')' | '{' | '}' | '!' => {
                flush_arg(&mut tokens, &mut current);
                tokens.push(ParsedToken {
                    kind: TokenKind::Shellism,
                    value: c.to_string(),
                });
            }
            // Operators
            '&' | '|' | ';' | '>' | '<' => {
                flush_arg(&mut tokens, &mut current);

                let mut op = c.to_string();
                // Lookahead for double-char operators
                if let Some(&next) = chars.peek() {
                    if (next == c && c != ';' && c != '<') || (c == '>' && next == '>') {
                        op.push(chars.next().unwrap());
                    }
                }

                let kind = match op.as_str() {
                    "&&" | "||" | ";" => TokenKind::Operator,
                    "|" => TokenKind::Pipe,
                    "&" => TokenKind::Shellism, // Background job needs real shell
                    _ => TokenKind::Redirect,
                };
                tokens.push(ParsedToken { kind, value: op });
            }
            // Whitespace delimits arguments
            c if c.is_whitespace() => {
                flush_arg(&mut tokens, &mut current);
            }
            // Regular character
            _ => current.push(c),
        }
    }

    // Handle unclosed quote (treat remaining as arg, don't panic)
    flush_arg(&mut tokens, &mut current);
    tokens
}

fn flush_arg(tokens: &mut Vec<ParsedToken>, current: &mut String) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        tokens.push(ParsedToken {
            kind: TokenKind::Arg,
            value: trimmed.to_string(),
        });
    }
    current.clear();
}

/// Strip quotes from a token value
pub fn strip_quotes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() >= 2
        && ((chars[0] == '"' && chars[chars.len() - 1] == '"')
            || (chars[0] == '\'' && chars[chars.len() - 1] == '\''))
    {
        return chars[1..chars.len() - 1].iter().collect();
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // === BASIC FUNCTIONALITY TESTS ===

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

    // === QUOTE HANDLING TESTS ===

    #[test]
    fn test_quoted_operator_not_split() {
        let tokens = tokenize(r#"git commit -m "Fix && Bug""#);
        // && inside quotes should NOT be an Operator token
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
        let tokens = tokenize("echo \"hello world\"");
        assert!(tokens.iter().any(|t| t.value == "\"hello world\""));
    }

    #[test]
    fn test_empty_quoted_string() {
        let tokens = tokenize("echo \"\"");
        // Should have echo and ""
        assert!(tokens.iter().any(|t| t.value == "\"\""));
    }

    #[test]
    fn test_nested_quotes() {
        let tokens = tokenize(r#"echo "outer 'inner' outer""#);
        assert!(tokens.iter().any(|t| t.value.contains("'inner'")));
    }

    #[test]
    fn test_strip_quotes_double() {
        assert_eq!(strip_quotes("\"hello\""), "hello");
    }

    #[test]
    fn test_strip_quotes_single() {
        assert_eq!(strip_quotes("'hello'"), "hello");
    }

    #[test]
    fn test_strip_quotes_none() {
        assert_eq!(strip_quotes("hello"), "hello");
    }

    #[test]
    fn test_strip_quotes_mismatched() {
        assert_eq!(strip_quotes("\"hello'"), "\"hello'");
    }

    // === ESCAPE HANDLING TESTS ===

    #[test]
    fn test_escaped_space() {
        let tokens = tokenize("echo hello\\ world");
        // Escaped space should be part of the arg
        assert!(tokens.iter().any(|t| t.value.contains("hello")));
    }

    #[test]
    fn test_backslash_in_single_quotes() {
        // In single quotes, backslash is literal
        let tokens = tokenize(r#"echo 'hello\nworld'"#);
        assert!(tokens.iter().any(|t| t.value.contains(r#"\n"#)));
    }

    #[test]
    fn test_escaped_quote_in_double() {
        let tokens = tokenize(r#"echo "hello\"world""#);
        assert!(tokens.iter().any(|t| t.value.contains("hello")));
    }

    // === EDGE CASE TESTS ===

    #[test]
    fn test_empty_input() {
        let tokens = tokenize("");
        assert!(tokens.is_empty());
    }

    #[test]
    fn test_whitespace_only() {
        let tokens = tokenize("   ");
        assert!(tokens.is_empty());
    }

    #[test]
    fn test_unclosed_single_quote() {
        // Should not panic, treat remaining as part of arg
        let tokens = tokenize("'unclosed");
        assert!(!tokens.is_empty());
    }

    #[test]
    fn test_unclosed_double_quote() {
        // Should not panic, treat remaining as part of arg
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

    // === OPERATOR TESTS ===

    #[test]
    fn test_and_operator() {
        let tokens = tokenize("cmd1 && cmd2");
        assert!(tokens
            .iter()
            .any(|t| matches!(t.kind, TokenKind::Operator) && t.value == "&&"));
    }

    #[test]
    fn test_or_operator() {
        let tokens = tokenize("cmd1 || cmd2");
        assert!(tokens
            .iter()
            .any(|t| matches!(t.kind, TokenKind::Operator) && t.value == "||"));
    }

    #[test]
    fn test_semicolon() {
        let tokens = tokenize("cmd1 ; cmd2");
        assert!(tokens
            .iter()
            .any(|t| matches!(t.kind, TokenKind::Operator) && t.value == ";"));
    }

    #[test]
    fn test_multiple_and() {
        let tokens = tokenize("a && b && c");
        let ops: Vec<_> = tokens
            .iter()
            .filter(|t| matches!(t.kind, TokenKind::Operator))
            .collect();
        assert_eq!(ops.len(), 2);
    }

    #[test]
    fn test_mixed_operators() {
        let tokens = tokenize("a && b || c");
        let ops: Vec<_> = tokens
            .iter()
            .filter(|t| matches!(t.kind, TokenKind::Operator))
            .collect();
        assert_eq!(ops.len(), 2);
    }

    #[test]
    fn test_operator_at_start() {
        let tokens = tokenize("&& cmd");
        // Should still parse, just with operator first
        assert!(tokens.iter().any(|t| t.value == "&&"));
    }

    #[test]
    fn test_operator_at_end() {
        let tokens = tokenize("cmd &&");
        assert!(tokens.iter().any(|t| t.value == "&&"));
    }

    // === PIPE TESTS ===

    #[test]
    fn test_pipe_detection() {
        let tokens = tokenize("cat file | grep pattern");
        assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Pipe)));
    }

    #[test]
    fn test_quoted_pipe_not_pipe() {
        let tokens = tokenize("\"a|b\"");
        // Pipe inside quotes is not a Pipe token
        assert!(!tokens.iter().any(|t| matches!(t.kind, TokenKind::Pipe)));
    }

    #[test]
    fn test_multiple_pipes() {
        let tokens = tokenize("a | b | c");
        let pipes: Vec<_> = tokens
            .iter()
            .filter(|t| matches!(t.kind, TokenKind::Pipe))
            .collect();
        assert_eq!(pipes.len(), 2);
    }

    // === SHELLISM TESTS ===

    #[test]
    fn test_glob_detection() {
        let tokens = tokenize("ls *.rs");
        assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Shellism)));
    }

    #[test]
    fn test_quoted_glob_not_shellism() {
        let tokens = tokenize("echo \"*.txt\"");
        // Glob inside quotes is not a Shellism token
        assert!(!tokens.iter().any(|t| matches!(t.kind, TokenKind::Shellism)));
    }

    #[test]
    fn test_variable_detection() {
        let tokens = tokenize("echo $HOME");
        assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Shellism)));
    }

    #[test]
    fn test_quoted_variable_not_shellism() {
        let tokens = tokenize("echo \"$HOME\"");
        // $ inside double quotes is NOT detected as a Shellism token
        // because the lexer respects quotes
        // This is correct - the variable can't be expanded by us anyway
        // so the whole command will need to passthrough to shell
        // But at the tokenization level, it's not a Shellism
        assert!(!tokens.iter().any(|t| matches!(t.kind, TokenKind::Shellism)));
    }

    #[test]
    fn test_backtick_substitution() {
        let tokens = tokenize("echo `date`");
        assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Shellism)));
    }

    #[test]
    fn test_subshell_detection() {
        let tokens = tokenize("echo $(date)");
        // Both $ and ( should be shellisms
        let shellisms: Vec<_> = tokens
            .iter()
            .filter(|t| matches!(t.kind, TokenKind::Shellism))
            .collect();
        assert!(!shellisms.is_empty());
    }

    #[test]
    fn test_brace_expansion() {
        let tokens = tokenize("echo {a,b}.txt");
        assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Shellism)));
    }

    #[test]
    fn test_escaped_glob() {
        let tokens = tokenize("echo \\*.txt");
        // Escaped glob should not be a shellism
        assert!(!tokens
            .iter()
            .any(|t| matches!(t.kind, TokenKind::Shellism) && t.value == "*"));
    }

    // === REDIRECT TESTS ===

    #[test]
    fn test_redirect_out() {
        let tokens = tokenize("cmd > file");
        assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Redirect)));
    }

    #[test]
    fn test_redirect_append() {
        let tokens = tokenize("cmd >> file");
        assert!(tokens
            .iter()
            .any(|t| matches!(t.kind, TokenKind::Redirect) && t.value == ">>"));
    }

    #[test]
    fn test_redirect_in() {
        let tokens = tokenize("cmd < file");
        assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Redirect)));
    }

    #[test]
    fn test_redirect_stderr() {
        let tokens = tokenize("cmd 2> file");
        assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Redirect)));
    }

    // === EXCLAMATION / NEGATION TESTS ===

    #[test]
    fn test_exclamation_is_shellism() {
        let tokens = tokenize("if ! grep -q pattern file; then echo missing; fi");
        assert!(
            tokens
                .iter()
                .any(|t| matches!(t.kind, TokenKind::Shellism) && t.value == "!"),
            "! (negation) must be Shellism"
        );
    }

    // === BACKGROUND JOB TESTS ===

    #[test]
    fn test_background_job_is_shellism() {
        let tokens = tokenize("sleep 10 &");
        assert!(
            tokens
                .iter()
                .any(|t| matches!(t.kind, TokenKind::Shellism) && t.value == "&"),
            "Single & (background job) must be Shellism, not Redirect"
        );
    }
}
