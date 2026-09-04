#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipeKind {
    /// Standard stdout pipeline (`|`).
    Stdout,
    /// Combined stdout-and-stderr pipeline (`|&`).
    StdoutAndStderr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Arg,
    Operator,
    Pipe(PipeKind),
    Redirect,
    Shellism,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedToken {
    pub kind: TokenKind,
    pub value: String,
    pub offset: usize,
}

/// How `tokenize_inner` treats `\n`/`\r`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NewlineMode {
    /// Ordinary characters, no Operator tokens.
    None,
    /// `\n`, and the `\r` of a CRLF pair, are Operator boundaries; a lone
    /// `\r` stays glued to its word — real bash's behavior.
    Bash,
    /// Like `Bash`, but a lone `\r` is a boundary too. Only
    /// `split_for_permissions` uses this, to stay maximally conservative.
    Conservative,
}

pub fn tokenize(input: &str) -> Vec<ParsedToken> {
    tokenize_inner(input, NewlineMode::None)
}

/// Like [`tokenize`] but emits a `\n` operator token for each newline that
/// sits outside quotes. Newlines inside quoted strings stay part of their
/// argument, so callers can use the emitted offsets as safe line-split points.
pub fn tokenize_with_newlines(input: &str) -> Vec<ParsedToken> {
    tokenize_inner(input, NewlineMode::Bash)
}

/// Applies one character's effect on quote state, mirroring bash: only the
/// quote char that opened a span closes it. Shared by `tokenize_inner`,
/// `shell_split`, and `registry.rs::QuoteScan` so they can't drift.
pub(crate) fn advance_quote_state(quote: Option<char>, c: char) -> Option<char> {
    match (quote, c) {
        (None, '\'' | '"') => Some(c),
        (Some(q), c) if c == q => None,
        (q, _) => q,
    }
}

/// Bash's default `$IFS` is exactly space/tab/newline — not Rust's
/// `char::is_whitespace()`, which wrongly includes non-IFS Unicode
/// whitespace like NBSP. Shared with `permissions.rs::command_matches_pattern`.
pub(crate) fn is_word_boundary_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n')
}

/// True if `bytes[i..]` starts a CRLF pair. Shared by `tokenize_inner` and
/// `registry.rs::rewrite_multiline_block`'s raw-newline parity check.
pub(crate) fn is_crlf_at(bytes: &[u8], i: usize) -> bool {
    bytes.get(i) == Some(&b'\r') && bytes.get(i + 1) == Some(&b'\n')
}

/// Merges `tokenize()` tokens that are directly adjacent in `cmd` (no gap)
/// into single words — e.g. `*.yml` tokenizes as `Shellism("*")` +
/// `Arg(".yml")` but is one bash word. For callers that only need "was there
/// a space here", not full shell-operator awareness.
pub(crate) fn coalesce_words<'a>(cmd: &'a str, tokens: &[ParsedToken]) -> Vec<(&'a str, usize)> {
    let mut words = Vec::new();
    let mut run_start: Option<usize> = None;
    let mut run_end: usize = 0;

    for tok in tokens {
        if let Some(start) = run_start
            && tok.offset != run_end
        {
            words.push((&cmd[start..run_end], start));
            run_start = None;
        }
        if run_start.is_none() {
            run_start = Some(tok.offset);
        }
        run_end = tok.offset + tok.value.len();
    }
    if let Some(start) = run_start {
        words.push((&cmd[start..run_end], start));
    }
    words
}

fn tokenize_inner(input: &str, newline_mode: NewlineMode) -> Vec<ParsedToken> {
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

        if quote.is_some() || c == '\'' || c == '"' {
            if quote.is_none() && current.is_empty() {
                current_start = byte_pos;
            }
            quote = advance_quote_state(quote, c);
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
                } else if chars.peek() == Some(&'&') {
                    chars.next();
                    byte_pos += 1;
                    tokens.push(ParsedToken {
                        kind: TokenKind::Pipe(PipeKind::StdoutAndStderr),
                        value: "|&".into(),
                        offset: start,
                    });
                } else {
                    tokens.push(ParsedToken {
                        kind: TokenKind::Pipe(PipeKind::Stdout),
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
            c @ ('\n' | '\r')
                if newline_mode != NewlineMode::None
                    && (c == '\n'
                        || newline_mode == NewlineMode::Conservative
                        || is_crlf_at(input.as_bytes(), byte_pos)) =>
            {
                flush_arg(&mut tokens, &mut current, current_start);
                tokens.push(ParsedToken {
                    kind: TokenKind::Operator,
                    value: "\n".into(),
                    offset: byte_pos,
                });
                byte_pos += char_len;
                current_start = byte_pos;
            }
            c if is_word_boundary_whitespace(c) => {
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

/// True for constructs the permission gate can't decompose, so they must never
/// be auto-allowed: command/process substitution, or a real file-target redirect
/// (fd-dup like `2>&1` and `/dev/null` are exempt). Separators and subshells are
/// handled by [`split_for_permissions`], not flagged here.
pub fn contains_unattestable_construct(cmd: &str) -> bool {
    if contains_substitution(cmd) {
        return true;
    }
    let tokens = tokenize(cmd);
    tokens
        .iter()
        .enumerate()
        .any(|(i, tok)| tok.kind == TokenKind::Redirect && redirect_has_file_target(&tokens, i))
}

/// Quote-aware: bash runs backtick/`$(...)` unquoted and inside double quotes,
/// but treats single-quoted text literally; `<(`/`>(` is unquoted-only.
fn contains_substitution(cmd: &str) -> bool {
    let bytes = cmd.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if !in_single => {
                i += 2;
                continue;
            }
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'`' if !in_single => return true,
            b'$' if !in_single && bytes.get(i + 1) == Some(&b'(') => return true,
            b'<' | b'>' if !in_single && !in_double && bytes.get(i + 1) == Some(&b'(') => {
                return true;
            }
            _ => {}
        }
        i += 1;
    }
    false
}

// `>&N`/`>&-` (and `N>&M`) is fd-dup/close; bare `>&` before a word is
// `>word 2>&1` — a file target.
fn redirect_has_file_target(tokens: &[ParsedToken], i: usize) -> bool {
    let value = &tokens[i].value;
    if let Some(pos) = value.find(">&") {
        let tail = &value[pos + 2..];
        if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit() || c == '-') {
            return false;
        }
    }
    match tokens.get(i + 1) {
        Some(next) if next.kind == TokenKind::Arg => next.value != "/dev/null",
        _ => true,
    }
}

/// Segments `cmd` for the **permission gate** (`permissions.rs::check_command_with_rules`):
/// every segment this returns is independently checked against deny/ask/allow
/// rules, so this is deliberately the most paranoid of the three compound-command
/// segmenters in this codebase — see [`split_on_operators`] (analytics/discovery
/// classification) and `registry.rs::rewrite_compound`'s inline token walk (actual
/// rewrite) for the other two, which intentionally segment the same kind of input
/// differently:
///
/// | | here (permission gate) | [`split_on_operators`] (analytics) | `rewrite_compound` (rewrite) |
/// |---|---|---|---|
/// | `&&` / `\|\|` / `;` | splits | splits | splits |
/// | `\|` | always splits | stops at first `\|` | pipeline handled specially |
/// | background `&` | splits (Shellism boundary) | does not split | splits |
/// | `( ... )` grouping | splits (Shellism boundary) | does not split | does not split standalone |
/// | trailing redirect | truncates the segment | kept | kept (rewritten output preserves it) |
/// | lone `\r` (no following `\n`) | splits | does not split | does not split |
///
/// Like [`split_on_operators`] but also breaks on newline, background `&`,
/// subshell `( ... )`, and a lone `\r` (`NewlineMode::Conservative`), and
/// truncates each segment at its first redirect — deliberately conservative
/// so a hidden command can't evade the gate by hiding behind a construct
/// another segmenter would leave intact.
/// Callers must still gate on [`contains_unattestable_construct`] first.
pub fn split_for_permissions(cmd: &str) -> Vec<&str> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return vec![];
    }

    let tokens = tokenize_inner(trimmed, NewlineMode::Conservative);
    let mut results = Vec::new();
    let mut seg_start: usize = 0;
    let mut seg_end: Option<usize> = None;

    for tok in &tokens {
        let is_boundary = match tok.kind {
            TokenKind::Operator | TokenKind::Pipe(_) => true,
            TokenKind::Shellism => matches!(tok.value.as_str(), "&" | "(" | ")"),
            _ => false,
        };

        if is_boundary {
            let end = seg_end.take().unwrap_or(tok.offset);
            let segment = trimmed[seg_start..end].trim();
            if !segment.is_empty() {
                results.push(segment);
            }
            seg_start = tok.offset + tok.value.len();
        } else if tok.kind == TokenKind::Redirect && seg_end.is_none() {
            seg_end = Some(tok.offset);
        }
    }

    let end = seg_end.unwrap_or(trimmed.len());
    let tail = trimmed[seg_start..end].trim();
    if !tail.is_empty() {
        results.push(tail);
    }

    results
}

/// Split a shell command on operators (`&&`, `||`, `;`) and optionally pipes
/// (`|`), quote-aware. `stop_at_pipe: true` returns only segments before the
/// first `|` (rewrite's left-side-only case); `false` splits through pipes
/// too (permission checking, every segment validated).
///
/// For classification only — unlike [`split_for_permissions`] this never
/// splits on background `&`/`( ... )` or truncates at a redirect (see that
/// function's comparison table), so it must not be repurposed for
/// permission/security decisions.
pub fn split_on_operators(cmd: &str, stop_at_pipe: bool) -> Vec<&str> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return vec![];
    }

    let tokens = tokenize(trimmed);
    let mut results = Vec::new();
    let mut seg_start: usize = 0;

    for tok in &tokens {
        match tok.kind {
            TokenKind::Operator => {
                let segment = trimmed[seg_start..tok.offset].trim();
                if !segment.is_empty() {
                    results.push(segment);
                }
                seg_start = tok.offset + tok.value.len();
            }
            TokenKind::Pipe(_) => {
                let segment = trimmed[seg_start..tok.offset].trim();
                if !segment.is_empty() {
                    results.push(segment);
                }
                if stop_at_pipe {
                    return results;
                }
                seg_start = tok.offset + tok.value.len();
            }
            _ => {}
        }
    }

    let tail = trimmed[seg_start..].trim();
    if !tail.is_empty() {
        results.push(tail);
    }

    results
}

#[cfg(test)]
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

/// Turns a coalesced word's raw text (quotes/escapes still literal, as
/// `tokenize()` preserves them) into argv-ready text: quote chars that
/// open/close a span are stripped, backslash escapes resolved.
fn resolve_word_text(raw: &str) -> String {
    let mut result = String::new();
    let mut chars = raw.chars().peekable();
    let mut quote: Option<char> = None;

    while let Some(c) = chars.next() {
        match c {
            // Inside double quotes bash only lets `\` escape `$`, `` ` ``, `"`,
            // `\` or a newline; before anything else it is a literal character.
            // That is what keeps a quoted Windows path (`"C:\Program Files"`)
            // intact instead of eating its separators.
            '\\' if quote == Some('"') => match chars.peek() {
                Some('$' | '`' | '"' | '\\' | '\n') => {
                    if let Some(next) = chars.next() {
                        result.push(next);
                    }
                }
                _ => result.push('\\'),
            },
            '\\' if quote.is_none() => {
                if let Some(next) = chars.next() {
                    result.push(next);
                }
            }
            '\'' | '"' => {
                // advance_quote_state leaves `quote` unchanged when `c` is the
                // "wrong" quote char for the current span (e.g. a `'` while
                // inside `"..."`) — that's literal text, not a toggle.
                let new_quote = advance_quote_state(quote, c);
                if new_quote == quote {
                    result.push(c);
                } else {
                    quote = new_quote;
                }
            }
            _ => result.push(c),
        }
    }

    result
}

/// Quote-aware split of a single shell command into argv-ready words: quotes
/// stripped, backslash escapes resolved — for callers that hand the result
/// straight to `Command::new`/exec or compare it against literal words
/// (`hooks/mod.rs::is_claude_hook_command`, `rtk proxy` arg-splitting).
pub fn shell_split(input: &str) -> Vec<String> {
    coalesce_words(input, &tokenize(input))
        .into_iter()
        .map(|(raw, _)| resolve_word_text(raw))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coalesce_words_merges_adjacent_tokens() {
        let cmd = "golangci-lint --config *.yml run";
        let words: Vec<&str> = coalesce_words(cmd, &tokenize(cmd))
            .into_iter()
            .map(|(w, _)| w)
            .collect();
        assert_eq!(words, vec!["golangci-lint", "--config", "*.yml", "run"]);
    }

    #[test]
    fn test_coalesce_words_preserves_offsets() {
        let cmd = "a *.yml b";
        let words = coalesce_words(cmd, &tokenize(cmd));
        assert_eq!(words, vec![("a", 0), ("*.yml", 2), ("b", 8)]);
    }

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
        assert!(
            !tokens
                .iter()
                .any(|t| matches!(t.kind, TokenKind::Operator) && t.value == "&&")
        );
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
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Operator && t.value == "&&")
        );
    }

    #[test]
    fn test_or_operator() {
        let tokens = tokenize("cmd1 || cmd2");
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Operator && t.value == "||")
        );
    }

    #[test]
    fn test_semicolon() {
        let tokens = tokenize("cmd1 ; cmd2");
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Operator && t.value == ";")
        );
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
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Pipe(PipeKind::Stdout))
        );
    }

    #[test]
    fn test_stderr_pipe_is_atomic() {
        let tokens = tokenize("cargo test |& grep FAILED");
        let pipes: Vec<_> = tokens
            .iter()
            .filter(|token| matches!(token.kind, TokenKind::Pipe(_)))
            .collect();

        assert_eq!(pipes.len(), 1);
        assert_eq!(pipes[0].kind, TokenKind::Pipe(PipeKind::StdoutAndStderr));
        assert_eq!(pipes[0].value, "|&");
        assert!(
            !tokens
                .iter()
                .any(|token| token.kind == TokenKind::Shellism && token.value == "&")
        );
        assert_eq!(
            split_for_permissions("cargo test |& grep FAILED"),
            vec!["cargo test", "grep FAILED"]
        );
    }

    #[test]
    fn test_quoted_pipe_not_pipe() {
        let tokens = tokenize("\"a|b\"");
        assert!(!tokens.iter().any(|t| matches!(t.kind, TokenKind::Pipe(_))));
    }

    #[test]
    fn test_multiple_pipes() {
        let tokens = tokenize("a | b | c");
        let pipes: Vec<_> = tokens
            .iter()
            .filter(|t| matches!(t.kind, TokenKind::Pipe(_)))
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
        assert!(
            !tokens
                .iter()
                .any(|t| t.kind == TokenKind::Shellism && t.value == "*")
        );
    }

    #[test]
    fn test_redirect_out() {
        let tokens = tokenize("cmd > file");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Redirect));
    }

    #[test]
    fn test_redirect_append() {
        let tokens = tokenize("cmd >> file");
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Redirect && t.value == ">>")
        );
    }

    #[test]
    fn test_redirect_in() {
        let tokens = tokenize("cmd < file");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Redirect));
    }

    #[test]
    fn test_redirect_stderr() {
        let tokens = tokenize("cmd 2> file");
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Redirect && t.value.starts_with("2>"))
        );
    }

    #[test]
    fn test_redirect_stderr_no_space() {
        let tokens = tokenize("cmd 2>/dev/null");
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Redirect && t.value == "2>")
        );
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Arg && t.value == "/dev/null")
        );
    }

    #[test]
    fn test_redirect_dev_null() {
        let tokens = tokenize("cmd > /dev/null");
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Redirect && t.value == ">")
        );
    }

    #[test]
    fn test_redirect_2_to_1_single_token() {
        let tokens = tokenize("cmd 2>&1");
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[1].kind, TokenKind::Redirect);
        assert_eq!(tokens[1].value, "2>&1");
        assert!(
            !tokens
                .iter()
                .any(|t| t.kind == TokenKind::Shellism && t.value == "&")
        );
    }

    #[test]
    fn test_redirect_1_to_2_single_token() {
        let tokens = tokenize("cmd 1>&2");
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Redirect && t.value == "1>&2")
        );
    }

    #[test]
    fn test_redirect_fd_close() {
        let tokens = tokenize("cmd 2>&-");
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Redirect && t.value == "2>&-")
        );
    }

    #[test]
    fn test_redirect_shorthand_dup() {
        let tokens = tokenize("cmd >&2");
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Redirect && t.value == ">&2")
        );
    }

    #[test]
    fn test_redirect_amp_gt() {
        let tokens = tokenize("cmd &>/dev/null");
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Redirect && t.value == "&>")
        );
    }

    #[test]
    fn test_redirect_amp_gt_gt() {
        let tokens = tokenize("cmd &>>/dev/null");
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Redirect && t.value == "&>>")
        );
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
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Redirect && t.value == ">>")
        );
    }

    #[test]
    fn test_redirect_heredoc_marker() {
        let tokens = tokenize("cat <<EOF");
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Redirect && t.value == "<<")
        );
    }

    #[test]
    fn test_redirect_2_to_1_with_pipe() {
        let tokens = tokenize("cargo test 2>&1 | head");
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Redirect && t.value == "2>&1")
        );
        assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Pipe(_))));
    }

    #[test]
    fn test_redirect_2_to_1_with_and() {
        let tokens = tokenize("cargo test 2>&1 && echo done");
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Redirect && t.value == "2>&1")
        );
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Operator && t.value == "&&")
        );
    }

    #[test]
    fn test_exclamation_is_shellism() {
        let tokens = tokenize("if ! grep -q pattern file; then echo missing; fi");
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Shellism && t.value == "!")
        );
    }

    #[test]
    fn test_background_job_is_shellism() {
        let tokens = tokenize("sleep 10 &");
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Shellism && t.value == "&")
        );
    }

    #[test]
    fn test_background_not_confused_with_amp_redirect() {
        let tokens = tokenize("cargo test &>/dev/null");
        assert!(
            !tokens
                .iter()
                .any(|t| t.kind == TokenKind::Shellism && t.value == "&")
        );
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
            .position(|t| matches!(t.kind, TokenKind::Pipe(_)))
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
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Arg && t.value == "2")
        );
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Redirect && t.value == ">")
        );
    }

    #[test]
    fn test_fd_redirect_no_space() {
        let tokens = tokenize("echo 2>file");
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Redirect && t.value == "2>")
        );
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Arg && t.value == "file")
        );
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
    fn test_shell_split_keeps_backslash_in_double_quotes() {
        assert_eq!(
            shell_split(r#""C:\Program Files\rtk.exe" hook codex"#),
            vec![r"C:\Program Files\rtk.exe", "hook", "codex"]
        );
    }

    #[test]
    fn test_shell_split_double_quote_escapes_only_bash_specials() {
        assert_eq!(
            shell_split(r#"echo "a\$b" "a\"b" "a\\b" "a\nb""#),
            vec!["echo", "a$b", "a\"b", r"a\b", r"a\nb"]
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

    #[test]
    fn test_shell_split_coalesces_unquoted_glob_next_to_quoted_segment() {
        // An unquoted metacharacter directly adjacent to a quoted segment
        // (no space between them) must stay one word — the same
        // token-coalescing gap that split_token_spans needed for golangci-lint,
        // now exercised through shell_split's output shape (quotes stripped).
        assert_eq!(
            shell_split(r#"echo *.yml"quoted end""#),
            vec!["echo", "*.ymlquoted end"]
        );
    }

    #[test]
    fn test_shell_split_splits_on_embedded_newline() {
        // Bash's default $IFS is space/tab/newline, so an embedded unquoted
        // `\n` is a word boundary, same as space or tab.
        assert_eq!(shell_split("a\nb"), vec!["a", "b"]);
    }

    #[test]
    fn test_shell_split_does_not_split_on_nbsp() {
        // U+00A0 (NBSP) has Unicode `White_Space = Y` despite not being part
        // of bash's $IFS — char::is_whitespace() would wrongly treat it as a
        // word boundary. `a\u{a0}b` must stay one word, matching real bash.
        assert_eq!(shell_split("a\u{a0}b"), vec!["a\u{a0}b"]);
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

    #[test]
    fn test_split_on_operators_stop_at_pipe() {
        assert_eq!(split_on_operators("a | b | c", true), vec!["a"]);
        assert_eq!(split_on_operators("a && b | c", true), vec!["a", "b"]);
    }

    #[test]
    fn test_split_on_operators_through_pipes() {
        assert_eq!(split_on_operators("a | b | c", false), vec!["a", "b", "c"]);
        assert_eq!(
            split_on_operators("a && b | c ; d", false),
            vec!["a", "b", "c", "d"]
        );
    }

    #[test]
    fn test_split_on_operators_quoted() {
        assert_eq!(
            split_on_operators(r#"echo "a && b" && cargo test"#, false),
            vec![r#"echo "a && b""#, "cargo test"]
        );
    }

    #[test]
    fn test_split_on_operators_empty() {
        assert!(split_on_operators("", false).is_empty());
        assert!(split_on_operators("  ", true).is_empty());
    }

    // --- contains_unattestable_construct (security) -------------------------

    #[test]
    fn test_unattestable_backtick() {
        assert!(contains_unattestable_construct("git status `whoami`"));
    }

    #[test]
    fn test_unattestable_command_substitution() {
        assert!(contains_unattestable_construct(
            "git log --pretty=$(rm -rf ~)"
        ));
    }

    #[test]
    fn test_unattestable_process_substitution() {
        assert!(contains_unattestable_construct("diff <(secret) <(other)"));
        assert!(contains_unattestable_construct("tee >(cat)"));
    }

    #[test]
    fn test_unattestable_substitution_inside_double_quotes() {
        assert!(contains_unattestable_construct(
            r#"git log --pretty="$(rm -rf ~)""#
        ));
        assert!(contains_unattestable_construct(
            r#"git log --pretty="`rm -rf ~`""#
        ));
        assert!(contains_unattestable_construct(
            r#"git -c x="$(whoami)" status"#
        ));
    }

    #[test]
    fn test_attestable_substitution_inside_single_quotes() {
        assert!(!contains_unattestable_construct("echo '$(rm -rf ~)'"));
        assert!(!contains_unattestable_construct("echo '`whoami`'"));
        assert!(!contains_unattestable_construct(r#"echo "\$(rm -rf ~)""#));
    }

    #[test]
    fn test_unattestable_file_redirects() {
        assert!(contains_unattestable_construct("git log > /tmp/x"));
        // nosemgrep: sensitive-path-reference -- test fixture
        assert!(contains_unattestable_construct("echo evil >> ~/.bashrc"));
        assert!(contains_unattestable_construct("cmd &> /tmp/x"));
        // nosemgrep: sensitive-path-reference -- test fixture
        assert!(contains_unattestable_construct("cat < /etc/passwd"));
        assert!(contains_unattestable_construct("cat << EOF"));
    }

    #[test]
    fn test_unattestable_ampersand_file_redirect() {
        // `>&word` (word not a number) == `>word 2>&1` — a file write.
        assert!(contains_unattestable_construct("git status >& /tmp/evil"));
        // nosemgrep: sensitive-path-reference -- test fixture
        assert!(contains_unattestable_construct("cat x >&~/.bashrc"));
        assert!(contains_unattestable_construct("echo hi 2>& /tmp/evil"));
    }

    #[test]
    fn test_attestable_fd_dup_and_devnull_redirects() {
        assert!(!contains_unattestable_construct("git status 2>&1"));
        assert!(!contains_unattestable_construct("cmd >&2"));
        assert!(!contains_unattestable_construct("cmd 2>&-"));
        assert!(!contains_unattestable_construct("cmd 2>/dev/null"));
        assert!(!contains_unattestable_construct("cmd > /dev/null"));
        assert!(!contains_unattestable_construct("cmd &> /dev/null"));
        assert!(!contains_unattestable_construct("cmd >& /dev/null"));
    }

    #[test]
    fn test_attestable_subshell_and_separators() {
        assert!(!contains_unattestable_construct(
            "(git status; cargo build)"
        ));
        assert!(!contains_unattestable_construct(
            "git status && cargo build"
        ));
        assert!(!contains_unattestable_construct("git status; cargo build"));
        assert!(!contains_unattestable_construct("git log | head"));
        assert!(!contains_unattestable_construct("sleep 1 &"));
        assert!(!contains_unattestable_construct("git status\ncargo build"));
    }

    #[test]
    fn test_attestable_variable_expansion() {
        assert!(!contains_unattestable_construct("echo $HOME"));
        assert!(!contains_unattestable_construct("echo ${HOME}"));
        assert!(!contains_unattestable_construct("git status"));
        assert!(!contains_unattestable_construct(""));
    }

    // --- split_for_permissions ---------------------------------------------

    #[test]
    fn test_split_perms_operators() {
        assert_eq!(
            split_for_permissions("git status && cargo build"),
            vec!["git status", "cargo build"]
        );
        assert_eq!(
            split_for_permissions("git status; cargo build"),
            vec!["git status", "cargo build"]
        );
        assert_eq!(
            split_for_permissions("git log | head"),
            vec!["git log", "head"]
        );
    }

    #[test]
    fn test_split_perms_newline() {
        assert_eq!(
            split_for_permissions("git status\ncargo build"),
            vec!["git status", "cargo build"]
        );
    }

    #[test]
    fn test_split_perms_lone_cr_still_splits() {
        assert_eq!(
            split_for_permissions("git status\rrm -rf ~"),
            vec!["git status", "rm -rf ~"]
        );
        // A CRLF pair still splits exactly once, not twice.
        assert_eq!(
            split_for_permissions("git status\r\ncargo build"),
            vec!["git status", "cargo build"]
        );
    }

    #[test]
    fn test_split_perms_lone_cr_inside_quotes_not_split() {
        assert_eq!(
            split_for_permissions("echo 'foo\rbar'"),
            vec!["echo 'foo\rbar'"]
        );
    }

    #[test]
    fn test_split_perms_background_ampersand() {
        assert_eq!(
            split_for_permissions("git status & rm -rf ~"),
            vec!["git status", "rm -rf ~"]
        );
        assert_eq!(split_for_permissions("sleep 1 &"), vec!["sleep 1"]);
    }

    #[test]
    fn test_split_perms_subshell() {
        assert_eq!(
            split_for_permissions("(git status; cargo build)"),
            vec!["git status", "cargo build"]
        );
        assert_eq!(split_for_permissions("((a; b); c)"), vec!["a", "b", "c"]);
    }

    #[test]
    fn test_split_perms_truncates_at_redirect() {
        assert_eq!(split_for_permissions("git status 2>&1"), vec!["git status"]);
        assert_eq!(split_for_permissions("git log > /tmp/x"), vec!["git log"]);
        assert_eq!(
            split_for_permissions("git push --force 2>&1"),
            vec!["git push --force"]
        );
    }

    #[test]
    fn test_split_perms_newline_inside_quotes_not_split() {
        let segments = split_for_permissions("echo 'line1\nline2'");
        assert_eq!(segments.len(), 1);
        assert!(segments[0].starts_with("echo"));
    }

    #[test]
    fn test_split_perms_empty() {
        assert!(split_for_permissions("").is_empty());
        assert!(split_for_permissions("   ").is_empty());
    }

    #[test]
    fn test_tokenize_with_newlines_emits_operator_outside_quotes_only() {
        let newline_ops = |input: &str| {
            tokenize_with_newlines(input)
                .iter()
                .filter(|t| t.kind == TokenKind::Operator && t.value == "\n")
                .count()
        };
        assert_eq!(newline_ops("git status\ngit log"), 1);
        assert_eq!(newline_ops("echo 'line1\nline2'"), 0);
        assert_eq!(newline_ops("git status\r\ngit log"), 2);
        // A lone `\r` (no following `\n`) is not a separator → no newline operator.
        assert_eq!(newline_ops("git status\rgit log"), 0);
    }

    #[test]
    fn test_lone_cr_is_not_a_word_boundary() {
        // Bash's default $IFS is space/tab/newline, never CR: a bare `\r` with no
        // following `\n` stays glued into its surrounding word instead of splitting
        // it, matching how real bash tokenizes `git status<CR>git log`.
        let args: Vec<String> = tokenize("git status\rgit log")
            .into_iter()
            .map(|t| t.value)
            .collect();
        assert_eq!(args, vec!["git", "status\rgit", "log"]);
    }

    #[test]
    fn test_crlf_in_plain_tokenize_keeps_cr_glued_to_word() {
        let args: Vec<String> = tokenize("git status\r\ngit log")
            .into_iter()
            .map(|t| t.value)
            .collect();
        assert_eq!(args, vec!["git", "status\r", "git", "log"]);
    }
}
