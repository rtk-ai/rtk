//! State-machine lexer that respects quotes and escapes.
//! Critical: `git commit -m "Fix && Bug"` must NOT split on &&

#[derive(Debug, PartialEq, Clone)]
pub enum TokenKind {
    Arg,      // Regular argument
    Operator, // &&, ||, ;
    Pipe,     // |
    Redirect, // >, >>, <, 2>
    Shellism, // *, ?, `, (, ), {, }, !, & — forces passthrough; $ only for complex forms
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedToken {
    pub kind: TokenKind,
    pub value: String, // The actual string value
    pub offset: usize, // Byte offset in the original input
}

/// Tokenize input with quote awareness.
/// Returns Vec of parsed tokens with byte offsets into the original input.
pub fn tokenize(input: &str) -> Vec<ParsedToken> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut current_start: usize = 0; // byte offset where current token started
    let mut byte_pos: usize = 0; // current byte position in input
    let mut chars = input.chars().peekable();

    let mut quote: Option<char> = None; // None, Some('\''), Some('"')
    let mut escaped = false;

    while let Some(c) = chars.next() {
        let char_len = c.len_utf8();

        // Handle escape sequences (but NOT inside single quotes)
        if escaped {
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
            current.push(c);
            byte_pos += char_len;
            continue;
        }

        // Handle quotes
        if let Some(q) = quote {
            if c == q {
                quote = None; // Close quote
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

        // Outside quotes - handle operators and shellisms
        match c {
            // '$' handling: simple $IDENT forms become Arg tokens.
            // The shell expands them when executing the rewritten "rtk cmd $VAR" —
            // RTK itself never needs to expand variables.
            // Complex forms ($(), ${}, $?, $$, $!, $0–$9) remain Shellism.
            '$' => {
                flush_arg(&mut tokens, &mut current, current_start);
                let dollar_pos = byte_pos;
                byte_pos += char_len;
                // Peek at the next char: alphabetic or '_' → consume a $IDENT as Arg.
                // Digits and special chars → Shellism (positional/special variables).
                if chars
                    .peek()
                    .is_some_and(|&nc| nc.is_ascii_alphabetic() || nc == '_')
                {
                    let mut name = String::from("$");
                    while chars
                        .peek()
                        .is_some_and(|&nc| nc.is_ascii_alphanumeric() || nc == '_')
                    {
                        let nc = chars.next().unwrap();
                        byte_pos += nc.len_utf8();
                        name.push(nc);
                    }
                    tokens.push(ParsedToken {
                        kind: TokenKind::Arg,
                        value: name,
                        offset: dollar_pos,
                    });
                } else {
                    // $(), ${}, $?, $$, $!, $1, bare $ — all need real shell
                    tokens.push(ParsedToken {
                        kind: TokenKind::Shellism,
                        value: "$".to_string(),
                        offset: dollar_pos,
                    });
                }
                current_start = byte_pos;
            }
            // Remaining shellisms force passthrough (includes ! for history expansion/negation)
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
            // Operators
            '&' | '|' | ';' | '>' | '<' => {
                flush_arg(&mut tokens, &mut current, current_start);
                let op_start = byte_pos;
                byte_pos += char_len;

                let mut op = c.to_string();
                // Lookahead for double-char operators
                if let Some(&next) = chars.peek() {
                    if (next == c && c != ';' && c != '<') || (c == '>' && next == '>') {
                        byte_pos += next.len_utf8();
                        op.push(chars.next().unwrap());
                    }
                }

                let kind = match op.as_str() {
                    "&&" | "||" | ";" => TokenKind::Operator,
                    "|" => TokenKind::Pipe,
                    "&" => TokenKind::Shellism, // Background job needs real shell
                    _ => TokenKind::Redirect,
                };
                tokens.push(ParsedToken {
                    kind,
                    value: op,
                    offset: op_start,
                });
                current_start = byte_pos;
            }
            // Whitespace delimits arguments
            c if c.is_whitespace() => {
                flush_arg(&mut tokens, &mut current, current_start);
                byte_pos += c.len_utf8();
                current_start = byte_pos;
            }
            // Regular character
            _ => {
                if current.is_empty() {
                    current_start = byte_pos;
                }
                current.push(c);
                byte_pos += char_len;
            }
        }
    }

    // Handle unclosed quote (treat remaining as arg, don't panic)
    flush_arg(&mut tokens, &mut current, current_start);
    tokens
}

fn flush_arg(tokens: &mut Vec<ParsedToken>, current: &mut String, offset: usize) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        // Adjust offset for leading whitespace that was trimmed
        let leading_ws = current.len() - current.trim_start().len();
        tokens.push(ParsedToken {
            kind: TokenKind::Arg,
            value: trimmed.to_string(),
            offset: offset + leading_ws,
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
