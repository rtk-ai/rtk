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
/// `shell_split`, and [`QuoteScan`] so they can't drift.
pub(crate) fn advance_quote_state(quote: Option<char>, c: char) -> Option<char> {
    match (quote, c) {
        (None, '\'' | '"') => Some(c),
        (Some(q), c) if c == q => None,
        (q, _) => q,
    }
}

/// Byte walker yielding `(offset, byte, in_single_before, in_double_before)`.
///
/// Quote state comes from the shared [`advance_quote_state`] rather than an
/// independently-maintained pair of bools, so it cannot drift from the lexer.
pub(crate) struct QuoteScan<'a> {
    bytes: &'a [u8],
    i: usize,
    quote: Option<char>,
}

impl<'a> QuoteScan<'a> {
    pub(crate) fn new(s: &'a str) -> Self {
        Self {
            bytes: s.as_bytes(),
            i: 0,
            quote: None,
        }
    }

    pub(crate) fn balanced(&self) -> bool {
        self.quote.is_none()
    }
}

impl Iterator for QuoteScan<'_> {
    type Item = (usize, u8, bool, bool);

    fn next(&mut self) -> Option<Self::Item> {
        while self.i < self.bytes.len() {
            let i = self.i;
            let b = self.bytes[i];
            if b == b'\\' && self.quote != Some('\'') {
                self.i += 2;
                continue;
            }
            let item = (i, b, self.quote == Some('\''), self.quote == Some('"'));
            if b == b'\'' || b == b'"' {
                self.quote = advance_quote_state(self.quote, b as char);
            }
            self.i += 1;
            return Some(item);
        }
        None
    }
}

/// Only `\'` inside `$'…'` diverges: bash keeps the string open, the lexer
/// closes it (#3188). Every operator past that point yields no token, so the
/// whole line reads as one segment and whatever follows rides along unchecked.
pub(crate) fn ansi_c_quote_defeats_lexer(cmd: &str) -> bool {
    let mut ansi_span = false;
    let mut backslash_run = 0u32;
    // The `$` must be one QuoteScan yielded: in `\$'...'` it was consumed as an
    // escape, so indexing the raw bytes would read a `$` that never opened a span.
    let mut prev_yielded = None;
    for (_, b, in_single, in_double) in QuoteScan::new(cmd) {
        if b == b'\'' && !in_double {
            if !in_single {
                ansi_span = prev_yielded == Some(b'$');
                backslash_run = 0;
            } else if ansi_span && backslash_run % 2 == 1 {
                return true;
            }
        } else if in_single {
            if b == b'\\' {
                backslash_run += 1;
            } else {
                backslash_run = 0;
            }
        }
        prev_yielded = Some(b);
    }
    false
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
                let start = byte_pos;
                let mut val = String::from(";");
                byte_pos += char_len;
                // `;;`, `;&` and `;;&` are single `case` terminators. Split
                // apart, the second half reads as an empty command, and the
                // rewrite emits `; ;` or `; &` in its place — a syntax error,
                // or a background job where a fall-through was written.
                if chars.peek() == Some(&';') {
                    chars.next();
                    byte_pos += 1;
                    val.push(';');
                }
                if chars.peek() == Some(&'&') {
                    chars.next();
                    byte_pos += 1;
                    val.push('&');
                }
                tokens.push(ParsedToken {
                    kind: TokenKind::Operator,
                    value: val,
                    offset: start,
                });
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
                // A leading fd number belongs to the redirect, as in the `>`
                // arm: left as its own word it reads as command text, and the
                // redirect behind it then looks like a trailing one.
                let fd_prefix =
                    if !current.is_empty() && current.chars().all(|ch| ch.is_ascii_digit()) {
                        Some(std::mem::take(&mut current))
                    } else {
                        flush_arg(&mut tokens, &mut current, current_start);
                        None
                    };
                let start = if fd_prefix.is_some() {
                    current_start
                } else {
                    byte_pos
                };
                let mut val = fd_prefix.unwrap_or_default();
                val.push('<');
                byte_pos += char_len;
                if chars.peek() == Some(&'<') {
                    chars.next();
                    byte_pos += 1;
                    val.push('<');
                } else if chars.peek() == Some(&'&') {
                    // `<&N` is one redirection operator, as `>&N` is in the
                    // `>` arm: split apart, its `&` reads as a background
                    // operator and its `N` as a command.
                    chars.next();
                    byte_pos += 1;
                    val.push('&');
                    while let Some(&nc) = chars.peek() {
                        if !nc.is_ascii_digit() && nc != '-' {
                            break;
                        }
                        chars.next();
                        byte_pos += 1;
                        val.push(nc);
                    }
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
/// be auto-allowed: command/process substitution, quoting the lexer reads
/// differently from bash, or a real file-target redirect (fd-dup like `2>&1`
/// and `/dev/null` are exempt). Separators and subshells are handled by
/// [`split_for_permissions`], not flagged here.
pub fn contains_unattestable_construct(cmd: &str) -> bool {
    if contains_substitution(cmd) {
        return true;
    }
    // Segments are not evidence about what will run once quoting diverges.
    if ansi_c_quote_defeats_lexer(cmd) {
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

/// Grammar that can open a command without being part of it, so it does not
/// count as command text: a redirect behind one is still a leading redirect.
fn is_grammar_word(value: &str) -> bool {
    matches!(value, "{" | "}" | "!")
}

// `>&N`/`>&-` (and `N>&M`) is fd-dup/close; bare `>&` before a word is
// `>word 2>&1` — a file target.
pub(crate) fn redirect_has_file_target(tokens: &[ParsedToken], i: usize) -> bool {
    let value = &tokens[i].value;
    // `<&` duplicates a descriptor exactly as `>&` does, and the tokenizer
    // only folds digits or `-` after either, so neither can name a file.
    if let Some(pos) = value.find(">&").or_else(|| value.find("<&")) {
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

/// How a policy treats redirect tokens.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RedirectPolicy {
    /// Ordinary text: the segment keeps the redirect and its operand.
    Keep,
    /// The command is what matters, not its plumbing: a redirect before any
    /// command text is stepped over, and one after it ends the segment.
    Excise,
}

/// What counts as the end of a segment, and what the segment keeps.
///
/// The permission gate, analytics classification and the rewrite each need a
/// different answer, and the gate's must be the most conservative — a segment
/// it never sees is a command its rules never check. Naming the differences
/// here keeps them chosen rather than emergent.
#[derive(Clone, Copy)]
pub(crate) struct Policy {
    newline: NewlineMode,
    /// `&`, `(` and `)` end a segment.
    group_boundaries: bool,
    redirects: RedirectPolicy,
    /// Whether the body of a `$( )` is read as commands in its own right.
    ///
    /// The gate descends, because a command hidden in a substitution still runs
    /// and still has to meet the deny rules. Every other caller stays out: the
    /// text around a substitution is not a command of its own — descending
    /// turns `git log $(git rev-parse HEAD)` into a `git log $` nobody ran —
    /// and what a substitution captures is a string the outer command is built
    /// from, so filtering it would change that string rather than change what
    /// reaches anyone.
    descend_into_substitution: bool,
}

impl Policy {
    /// The permission gate: breaks on everything a command could hide behind.
    pub(crate) const PERMISSIONS: Self = Self {
        newline: NewlineMode::Conservative,
        group_boundaries: true,
        redirects: RedirectPolicy::Excise,
        descend_into_substitution: true,
    };

    /// Classification only, never a security decision.
    pub(crate) const CLASSIFY: Self = Self {
        newline: NewlineMode::None,
        group_boundaries: true,
        redirects: RedirectPolicy::Keep,
        descend_into_substitution: false,
    };
}

/// One command's text within a compound command, with the byte range it came
/// from so a caller can splice around it rather than rebuild it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Segment<'a> {
    pub(crate) text: &'a str,
    pub(crate) start: usize,
    pub(crate) end: usize,
    /// Whether a `|` follows this command, so what it writes goes to another
    /// command instead of to the person. Its output is produced in full and
    /// then consumed, which is why filtering it can save tokens that never
    /// reach anyone.
    pub(crate) feeds_pipe: bool,
}

fn push_segment<'a>(
    out: &mut Vec<Segment<'a>>,
    input: &'a str,
    start: usize,
    end: usize,
    feeds_pipe: bool,
) {
    let raw = &input[start..end];
    let text = raw.trim();
    if text.is_empty() {
        return;
    }
    let lead = raw.len() - raw.trim_start().len();
    out.push(Segment {
        text,
        start: start + lead,
        end: start + lead + text.len(),
        feeds_pipe,
    });
}

/// Whether the `(` at `offset` opens a substitution rather than a subshell.
///
/// `$( )` captures a command's output as text the outer command is built from;
/// `<( )` and `>( )` hand it over as a file the outer command reads or writes.
/// In all three the inner command serves the outer one, so none of them is a
/// place to end a command or to filter output somebody else is about to parse.
pub(crate) fn opens_substitution(input: &str, offset: usize) -> bool {
    matches!(input[..offset].chars().next_back(), Some('$' | '<' | '>'))
}

/// Where a `case` is, between `case` and its `esac`.
///
/// Only `AwaitingPattern` changes how a token reads: there, `(` is the
/// optional opening bracket of a pattern rather than a subshell.
#[derive(PartialEq)]
enum CaseState {
    /// After `case`, before its `in`.
    AwaitingIn,
    /// A pattern starts here — after `in`, and after each arm terminator.
    AwaitingPattern,
    /// Inside an arm's commands, after the `)` that ended its pattern.
    InBody,
}

/// Follows `case … in pattern) … ;; … esac` so a pattern is not read as code.
///
/// Both segmenters consult this, because a pattern's optional opening bracket
/// is the one place a `(` does not start a command.
#[derive(Default)]
pub(crate) struct CaseTracker(Vec<CaseState>);

impl CaseTracker {
    /// Whether a pattern starts here, so a `(` belongs to it.
    pub(crate) fn in_pattern(&self) -> bool {
        self.0.last() == Some(&CaseState::AwaitingPattern)
    }

    /// Reads one token. `case` is the keyword only in command position, so
    /// that the `case` in `echo case` stays an ordinary word.
    pub(crate) fn observe(&mut self, tok: &ParsedToken, at_command_position: bool) {
        match (&tok.kind, tok.value.as_str()) {
            (TokenKind::Arg, "case") if at_command_position => {
                self.0.push(CaseState::AwaitingIn);
            }
            (TokenKind::Arg, "esac") => {
                self.0.pop();
            }
            (TokenKind::Arg, "in") => {
                self.advance(CaseState::AwaitingIn, CaseState::AwaitingPattern);
            }
            (TokenKind::Shellism, ")") => {
                self.advance(CaseState::AwaitingPattern, CaseState::InBody);
            }
            (TokenKind::Operator, ";;" | ";&" | ";;&") => {
                self.advance(CaseState::InBody, CaseState::AwaitingPattern);
            }
            _ => {}
        }
    }

    /// Moves the innermost `case` from `from` to `to`, leaving any other be.
    ///
    /// Only the innermost one advances, which is what keeps the `in` of a
    /// `for f in …` written inside an arm from being read as that arm's own.
    fn advance(&mut self, from: CaseState, to: CaseState) {
        if let Some(top) = self.0.last_mut()
            && *top == from
        {
            *top = to;
        }
    }
}

/// Split a compound command into the commands it runs, under `policy`.
///
/// Offsets are relative to `cmd.trim()`, which is what every caller matches
/// against.
pub(crate) fn segment(cmd: &str, policy: Policy) -> Vec<Segment<'_>> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return vec![];
    }

    let tokens = tokenize_inner(trimmed, policy.newline);
    let mut out = Vec::new();
    let mut seg_start: usize = 0;
    let mut seg_end: Option<usize> = None;
    let mut seg_has_text = false;
    let mut substitution_depth: usize = 0;
    let mut cases = CaseTracker::default();

    let mut i = 0;
    while let Some(tok) = tokens.get(i) {
        let at_command_position = !seg_has_text;
        let in_case_pattern = cases.in_pattern();
        let is_boundary = match tok.kind {
            TokenKind::Operator | TokenKind::Pipe(_) => substitution_depth == 0,
            TokenKind::Shellism => match tok.value.as_str() {
                // A `(` that closes a `$` opens a substitution, not a subshell.
                // Its body is tracked so the `)` that ends it is not read as a
                // boundary either, and so an operator inside it does not end a
                // command out here.
                "(" if !policy.descend_into_substitution
                    && opens_substitution(trimmed, tok.offset) =>
                {
                    substitution_depth += 1;
                    false
                }
                // A bracket nested inside a substitution counts too, or its `)`
                // closes the substitution one bracket early and the real
                // closing `)` is read as a boundary out here.
                "(" if substitution_depth > 0 => {
                    substitution_depth += 1;
                    false
                }
                ")" if substitution_depth > 0 => {
                    substitution_depth -= 1;
                    false
                }
                // `case x in (ls) …` is the same statement as `case x in ls) …`.
                // Ending a command here would make the pattern a command
                // position, and a rewrite there turns one word into two, which
                // bash rejects. The `)` still ends the pattern.
                "(" if in_case_pattern => false,
                "&" | "(" | ")" => policy.group_boundaries && substitution_depth == 0,
                _ => false,
            },
            _ => false,
        };

        if is_boundary {
            let end = seg_end.take().unwrap_or(tok.offset);
            push_segment(
                &mut out,
                trimmed,
                seg_start,
                end,
                matches!(tok.kind, TokenKind::Pipe(_)),
            );
            seg_start = tok.offset + tok.value.len();
            seg_has_text = false;
        } else if tok.kind == TokenKind::Redirect && policy.redirects == RedirectPolicy::Excise {
            if !seg_has_text {
                // A redirect may precede the command it applies to, and that
                // command still has to reach the caller, so step over the
                // redirect instead of ending the segment at it.
                //
                // The operand runs to the first gap or boundary. `>$HOME/x` is
                // several tokens but one word, and a boundary ends the operand
                // even with no gap — in `>a|rm -rf /` the `|` starts the next
                // command rather than continuing the filename.
                let mut end = tok.offset + tok.value.len();
                let mut next = i + 1;
                while let Some(part) = tokens.get(next) {
                    let ends_operand = matches!(
                        part.kind,
                        TokenKind::Operator | TokenKind::Pipe(_) | TokenKind::Shellism
                    );
                    if part.offset != end || ends_operand {
                        break;
                    }
                    end = part.offset + part.value.len();
                    next += 1;
                }
                seg_start = end;
                i = next;
                continue;
            } else if seg_end.is_none() {
                seg_end = Some(tok.offset);
            }
        } else if tok.kind == TokenKind::Arg
            || (tok.kind == TokenKind::Shellism && !is_grammar_word(&tok.value))
        {
            seg_has_text = true;
        }

        cases.observe(tok, at_command_position);
        i += 1;
    }

    let end = seg_end.unwrap_or(trimmed.len());
    push_segment(&mut out, trimmed, seg_start, end, false);
    out
}

/// Segments `cmd` for the **permission gate** (`permissions.rs::check_command_with_rules`):
/// every segment this returns is independently checked against deny/ask/allow
/// rules, so this is the most paranoid of the three compound-command segmenters
/// in this codebase — see [`split_for_classify`] (analytics/discovery
/// classification) and `registry.rs::rewrite_compound`'s inline token walk
/// (actual rewrite) for the other two.
///
/// All three agree on where a command begins and ends. Where a row below still
/// differs, the difference is the consumer's purpose, not an accident, and
/// `registry.rs`'s `segmenter_agreement` tests hold each one to a stated reason:
///
/// | | here (permission gate) | [`split_for_classify`] (analytics) | `rewrite_compound` (rewrite) |
/// |---|---|---|---|
/// | `&&` / `\|\|` / `;` | splits | splits | splits |
/// | `\|` | splits | splits | splits, then `PipelineSafety` decides per rule |
/// | background `&` | splits | splits | splits |
/// | `( ... )` grouping | splits | splits | splits |
/// | `$( ... )` substitution | descends: a command that runs must meet the rules | stays out: the text around it is not a command | stays out: it captures a string, not output |
/// | `{ ... }` grouping | strips the bracket before matching | not a boundary: `{` is brace expansion off a command position | not a boundary, same reason |
/// | trailing redirect | truncates the segment, so nothing rides in behind one | kept | kept, so the rewrite reproduces the real shape |
/// | leading redirect | stepped over, command kept | kept | kept |
/// | lone `\r` (no following `\n`) | splits | does not split | does not split |
///
/// Like [`split_for_classify`] but also breaks on newline and on a lone `\r`
/// (`NewlineMode::Conservative`), descends into `$( )`, and truncates each
/// segment at the first redirect that follows command text — deliberately
/// conservative so a hidden command can't evade the gate by hiding behind a
/// construct another segmenter would leave intact.
/// Callers must still gate on [`contains_unattestable_construct`] first.
pub fn split_for_permissions(cmd: &str) -> Vec<&str> {
    segment(cmd, Policy::PERMISSIONS)
        .into_iter()
        .map(|s| s.text)
        .collect()
}

/// Split a shell command on operators (`&&`, `||`, `;`) and pipes (`|`),
/// quote-aware.
///
/// For classification only — unlike [`split_for_permissions`] this never
/// splits on a newline or a lone `\r`, stays out of `$( )`, and keeps rather
/// than truncates at a redirect (see that function's comparison table), so it
/// must not be repurposed for permission/security decisions.
pub(crate) fn split_for_classify(cmd: &str) -> Vec<Segment<'_>> {
    segment(cmd, Policy::CLASSIFY)
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

    fn classify_texts(cmd: &str) -> Vec<&str> {
        split_for_classify(cmd)
            .into_iter()
            .map(|s| s.text)
            .collect()
    }

    #[test]
    fn test_split_for_classify_through_pipes() {
        assert_eq!(classify_texts("a | b | c"), vec!["a", "b", "c"]);
        assert_eq!(classify_texts("a && b | c ; d"), vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn test_split_for_classify_quoted() {
        assert_eq!(
            classify_texts(r#"echo "a && b" && cargo test"#),
            vec![r#"echo "a && b""#, "cargo test"]
        );
    }

    #[test]
    fn test_split_for_classify_empty() {
        assert!(classify_texts("").is_empty());
        assert!(classify_texts("  ").is_empty());
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

    /// `segment()` replaced two hand-rolled walkers. The gate's contract is
    /// still exactly that: over a generated corpus of compound commands it must
    /// return what the walker it replaced returned, because a gate that segments
    /// differently is a gate with different holes.
    ///
    /// The corpus is built from the constructs each walker treats specially,
    /// crossed rather than listed, because the shapes that broke the gate in
    /// practice were spacing variants nobody thought to write out.
    #[test]
    fn test_segment_matches_the_walkers_it_replaced() {
        const COMMANDS: &[&str] = &["ls", "rm -rf /", "git status", "echo a b", ""];
        const JOINERS: &[&str] = &[
            " && ", "&&", " || ", "||", "; ", ";", " | ", "|", " |& ", "|&", " & ", "&", " ;; ",
            ";;", ";&", ";;&", "\n", "\r\n", "\r", " ",
        ];
        const WRAPPERS: &[&str] = &["", "( ", "(", "{ ", "! ", ") ", "} "];
        const REDIRECTS: &[&str] = &[
            "",
            "2>&1 ",
            ">out ",
            "<in ",
            "0<&1 ",
            ">a|",
            "2>/dev/null ",
            ">$HOME/x ",
            "2>&1",
        ];

        let mut corpus: Vec<String> = Vec::new();
        for left in COMMANDS {
            for joiner in JOINERS {
                for right in COMMANDS {
                    corpus.push(format!("{left}{joiner}{right}"));
                    for wrapper in WRAPPERS {
                        corpus.push(format!("{wrapper}{left}{joiner}{right}"));
                    }
                    for redirect in REDIRECTS {
                        corpus.push(format!("{redirect}{left}{joiner}{right}"));
                        corpus.push(format!("{left}{joiner}{redirect}{right}"));
                    }
                }
            }
        }
        // Quoting, escapes and trailing plumbing, which change tokenization
        // rather than segmentation.
        for extra in [
            "echo 'a; b'",
            "echo \"a && b\"",
            "echo a\\;b",
            "echo $'\\''; rm -rf /",
            "cat <<EOF\nls\nEOF",
            "ls 2>&1 | grep x",
            "ls \\\n rm -rf /",
            "case x in a) ls;; esac",
            "  ls  ;  ls  ",
            "café;;fin",
            // Substitutions, which the boundary rules deliberately treat unlike
            // the subshells they look like.
            "echo $(ls)",
            "echo $(ls && rm -rf /)",
            "echo $(cd /tmp && (ls; pwd))",
            "echo $(ls) && rm -rf /",
            "ls $(",
            "ls )",
            "ls $(()) x",
        ] {
            corpus.push(extra.to_string());
        }

        assert!(corpus.len() > 5000, "corpus collapsed to {}", corpus.len());

        for cmd in &corpus {
            assert_eq!(
                split_for_permissions(cmd),
                legacy_segmenters::split_for_permissions_legacy(cmd),
                "permission segmentation changed for {cmd:?}"
            );
            // Classify and the gate hold the same policy once the three things
            // they deliberately differ on are out of the picture: newlines,
            // redirects, and substitutions. On everything else they must agree
            // exactly. That is what anchors classify to the frozen walker —
            // the gate is still compared against it line by line above, so an
            // unintended drift in `segment()` has to show up as classify and
            // the gate disagreeing here.
            // `<(` and `>(` are covered by excluding `<` and `>` for redirects.
            if !cmd.contains(['\n', '\r', '>', '<']) && !cmd.contains("$(") {
                assert_eq!(
                    classify_texts(cmd),
                    split_for_permissions(cmd),
                    "classify and the gate disagree on {cmd:?}, which shares their policy"
                );
            }

            // Whatever the policy, a segment is text that was really there, at
            // the span it claims, so a caller splicing by span can never write
            // over a neighbour or quote something nobody typed. This says
            // nothing about *which* text became a segment — gaps are legal,
            // that is where separators live — so the placement cases live in
            // registry.rs's `segmenter_agreement`.
            let trimmed = cmd.trim();
            let mut furthest = 0;
            for seg in split_for_classify(cmd) {
                assert!(
                    seg.start <= seg.end && seg.end <= trimmed.len(),
                    "span out of range for {cmd:?}"
                );
                assert_eq!(
                    &trimmed[seg.start..seg.end],
                    seg.text,
                    "span does not address its own text for {cmd:?}"
                );
                assert!(!seg.text.is_empty(), "empty segment for {cmd:?}");
                assert!(
                    seg.start >= furthest,
                    "segments overlap or run backwards for {cmd:?}"
                );
                furthest = seg.end;
            }
        }
    }

    /// The spans exist so a caller can splice around a segment instead of
    /// rebuilding it, which is only sound if they address the exact text.
    #[test]
    fn test_segment_spans_address_their_own_text() {
        for cmd in [
            "ls && rm -rf /",
            "  ls  ;  ls  ",
            "café;;fin",
            "(ls; cargo build)",
            "2>&1 ls | grep x",
            "ls\r\nls -la",
        ] {
            let trimmed = cmd.trim();
            for policy in [Policy::PERMISSIONS, Policy::CLASSIFY] {
                for seg in segment(cmd, policy) {
                    assert_eq!(
                        &trimmed[seg.start..seg.end],
                        seg.text,
                        "span {}..{} does not address {:?} in {cmd:?}",
                        seg.start,
                        seg.end,
                        seg.text
                    );
                    assert_eq!(seg.text.trim(), seg.text, "segment text was not trimmed");
                }
            }
        }
    }

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

#[cfg(test)]
mod legacy_segmenters {
    //! The hand-rolled segmenters `segment()` replaced, kept so the
    //! differential test can prove the replacement changed nothing.
    //!
    //! **Do not edit this module.** It is an oracle, not code: its value is
    //! that it is what shipped before the refactor. Tidying it, or "keeping it
    //! in sync" with `segment()`, leaves the test passing while it stops
    //! proving anything. Delete it outright when the refactor is old enough
    //! that the comparison no longer earns its keep.
    use super::*;

    #[allow(dead_code)]
    pub fn split_for_permissions_legacy(cmd: &str) -> Vec<&str> {
        let trimmed = cmd.trim();
        if trimmed.is_empty() {
            return vec![];
        }

        let tokens = tokenize_inner(trimmed, NewlineMode::Conservative);
        let mut results = Vec::new();
        let mut seg_start: usize = 0;
        let mut seg_end: Option<usize> = None;
        let mut seg_has_text = false;

        let mut i = 0;
        while let Some(tok) = tokens.get(i) {
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
                seg_has_text = false;
            } else if tok.kind == TokenKind::Redirect {
                if !seg_has_text {
                    // A redirect may precede the command it applies to, and that
                    // command still has to reach the deny rules, so step over the
                    // redirect instead of truncating the segment at it.
                    //
                    // The operand runs to the first gap or boundary. `>$HOME/x` is
                    // several tokens but one word, and a boundary ends the operand
                    // even with no gap — in `>a|rm -rf /` the `|` starts the next
                    // command rather than continuing the filename.
                    let mut end = tok.offset + tok.value.len();
                    let mut next = i + 1;
                    while let Some(part) = tokens.get(next) {
                        let ends_operand = matches!(
                            part.kind,
                            TokenKind::Operator | TokenKind::Pipe(_) | TokenKind::Shellism
                        );
                        if part.offset != end || ends_operand {
                            break;
                        }
                        end = part.offset + part.value.len();
                        next += 1;
                    }
                    seg_start = end;
                    i = next;
                    continue;
                } else if seg_end.is_none() {
                    seg_end = Some(tok.offset);
                }
            } else if tok.kind == TokenKind::Arg
                || (tok.kind == TokenKind::Shellism && !is_grammar_word(&tok.value))
            {
                seg_has_text = true;
            }

            i += 1;
        }

        let end = seg_end.unwrap_or(trimmed.len());
        let tail = trimmed[seg_start..end].trim();
        if !tail.is_empty() {
            results.push(tail);
        }

        results
    }

    #[allow(dead_code)]
    pub fn split_on_operators_legacy(cmd: &str, stop_at_pipe: bool) -> Vec<&str> {
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
}
