//! Quote-, escape- and operator-aware shell lexer: tokens, words, and the
//! segmenter that decides where one command ends and the next begins. Who uses
//! each entry point is listed in `src/core/README.md`.

use std::iter::{Peekable, once};

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
    /// A run of unquoted spaces and tabs between two words.
    Sep,
    /// One unquoted `\n`. Bash ends a command line there and nowhere else: a
    /// `\r` is an ordinary word byte, so `\r\n` is the end of a word followed
    /// by a `Newline`.
    Newline,
}

/// One token of a command line: what it is, the exact bytes it covers, and
/// where they start.
///
/// Tokens tile their input: every byte belongs to exactly one token, in order,
/// so `value` is always `input[offset..offset + value.len()]` and the values
/// concatenated give the input back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token<'a> {
    pub kind: TokenKind,
    pub value: &'a str,
    pub offset: usize,
}

impl Token<'_> {
    /// The offset just past this token.
    pub fn end(&self) -> usize {
        self.offset + self.value.len()
    }

    /// Whether this token only separates words: a `Sep` or a `Newline`.
    pub fn is_blank(&self) -> bool {
        matches!(self.kind, TokenKind::Sep | TokenKind::Newline)
    }
}

/// Bash's default `$IFS`: space, tab and newline. Vertical tab, form feed,
/// carriage return and non-breaking space are word bytes, which is why neither
/// `char::is_whitespace` nor `str::trim` may stand in for this.
pub(crate) fn is_ifs(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n')
}

/// `s` without the `$IFS` bytes at either end.
pub(crate) fn trim_ifs(s: &str) -> &str {
    s.trim_matches(is_ifs)
}

/// `s` without the `$IFS` bytes at its start.
pub(crate) fn trim_ifs_start(s: &str) -> &str {
    s.trim_start_matches(is_ifs)
}

/// `s` without the `$IFS` bytes at its end.
pub(crate) fn trim_ifs_end(s: &str) -> &str {
    s.trim_end_matches(is_ifs)
}

/// The non-empty runs of `s` between `$IFS` bytes. Blind to quoting, so a
/// quoted blank splits a word like any other.
pub(crate) fn split_ifs(s: &str) -> impl Iterator<Item = &str> {
    s.split(is_ifs).filter(|part| !part.is_empty())
}

/// Byte walker yielding each byte with the quoting it was reached in.
///
/// The one flat quote-state machine of this module: the tokenizer,
/// [`resolve_word_text`] and every other reader of quoting walk it, so none of
/// them can drift from the others. Only the quote char that opened a span
/// closes it, as in bash.
pub(crate) struct QuoteScan<'a> {
    bytes: &'a [u8],
    i: usize,
    quote: Option<u8>,
    /// The next byte is the one a `\` escapes.
    pending_escape: bool,
}

impl<'a> QuoteScan<'a> {
    pub(crate) fn new(s: &'a str) -> Self {
        Self {
            bytes: s.as_bytes(),
            i: 0,
            quote: None,
            pending_escape: false,
        }
    }

    /// Only the bytes that are shell syntax: an escaped byte, and the backslash
    /// escaping it, are dropped. What almost every caller wants — the exception
    /// being one asking where words start and end, for which an escaped byte is
    /// text like any other.
    pub(crate) fn significant(self) -> impl Iterator<Item = Scanned> {
        self.filter(|s| !s.escaped)
    }

    pub(crate) fn balanced(&self) -> bool {
        self.quote.is_none()
    }
}

/// One byte of a command, with what the quoting was when it was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Scanned {
    pub(crate) index: usize,
    pub(crate) byte: u8,
    pub(crate) in_single: bool,
    pub(crate) in_double: bool,
    /// A `\` outside single quotes, or the byte it escapes. Present in the
    /// text but not shell syntax: `\(` is not a bracket and `\'` opens
    /// nothing. Ask [`QuoteScan::significant`] to leave these out.
    pub(crate) escaped: bool,
}

impl Scanned {
    /// Neither quoted nor escaped: a byte the shell reads as syntax.
    fn is_bare(&self) -> bool {
        !self.escaped && !self.in_single && !self.in_double
    }
}

impl Iterator for QuoteScan<'_> {
    type Item = Scanned;

    fn next(&mut self) -> Option<Self::Item> {
        let i = self.i;
        let b = *self.bytes.get(i)?;
        let in_single = self.quote == Some(b'\'');
        let in_double = self.quote == Some(b'"');

        if self.pending_escape {
            self.pending_escape = false;
            self.i += 1;
            return Some(Scanned {
                index: i,
                byte: b,
                in_single,
                in_double,
                escaped: true,
            });
        }

        // A backslash outside single quotes escapes what follows, and neither
        // byte is syntax — including a quote, which must not flip the state.
        if b == b'\\' && !in_single {
            self.pending_escape = self.i + 1 < self.bytes.len();
            self.i += 1;
            return Some(Scanned {
                index: i,
                byte: b,
                in_single,
                in_double,
                escaped: true,
            });
        }

        match (self.quote, b) {
            (None, b'\'' | b'"') => self.quote = Some(b),
            (Some(q), b) if b == q => self.quote = None,
            _ => {}
        }
        self.i += 1;
        Some(Scanned {
            index: i,
            byte: b,
            in_single,
            in_double,
            escaped: false,
        })
    }
}

/// One shell word: a run of adjacent tokens that are not blanks, with its span.
///
/// A quoted span is one word however many blanks it holds, and so is `a\ b`,
/// because the tokenizer keeps both inside a single token. Operators are not
/// word boundaries here: `a;b` is one run of tokens with no blank in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Word<'a> {
    pub(crate) text: &'a str,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// The words of `input`, from its `tokens` (as [`tokenize`] returned them for
/// that same `input`).
pub(crate) fn words<'a>(input: &'a str, tokens: &[Token<'a>]) -> Vec<Word<'a>> {
    let mut found = Vec::new();
    let mut start: Option<usize> = None;
    let mut push = |start: usize, end: usize| {
        found.push(Word {
            text: &input[start..end],
            start,
            end,
        });
    };
    for tok in tokens {
        if tok.is_blank() {
            if let Some(from) = start.take() {
                push(from, tok.offset);
            }
        } else if start.is_none() {
            start = Some(tok.offset);
        }
    }
    if let Some(from) = start {
        push(from, tokens.last().map_or(input.len(), Token::end));
    }
    found
}

/// Where the text between `start` and `end` begins and ends once the blanks
/// around it are left out: from the first of `tokens` in that range that is
/// not a `Sep` or a `Newline` to the end of the last one. `None` when every
/// token there is a blank.
///
/// A command ends where its last word does, and an escaped or quoted blank is
/// part of a word: `head\ ` names the program `head␠`, so its span ends after
/// the space. Trimming `$IFS` bytes off the text would drop that space and
/// name `head`. `start` and `end` must fall on token boundaries.
pub(crate) fn content_span(
    tokens: &[Token<'_>],
    start: usize,
    end: usize,
) -> Option<(usize, usize)> {
    let from = tokens.partition_point(|t| t.offset < start);
    let to = tokens.partition_point(|t| t.offset < end);
    let inside = &tokens[from..to];
    let first = inside.iter().find(|t| !t.is_blank())?;
    let last = inside.iter().rev().find(|t| !t.is_blank())?;
    Some((first.offset, last.end()))
}

/// `input` without the blanks at either end, as [`content_span`] reads them,
/// with its tokens, their offsets counted from the trimmed text. One lex for
/// a caller that needs both.
pub(crate) fn tokenize_trimmed(input: &str) -> (&str, Vec<Token<'_>>) {
    let mut tokens = tokenize(input);
    let Some((from, to)) = content_span(&tokens, 0, input.len()) else {
        return ("", Vec::new());
    };
    tokens.retain(|t| t.offset >= from && t.end() <= to);
    for tok in &mut tokens {
        tok.offset -= from;
    }
    (&input[from..to], tokens)
}

/// The first `end` bytes' share of `tokens`: every token that ends by `end`.
/// When `end` falls on a token boundary, these are the tokens a lex of those
/// bytes alone returns.
pub(crate) fn tokens_until<'t, 'a>(tokens: &'t [Token<'a>], end: usize) -> &'t [Token<'a>] {
    &tokens[..tokens.partition_point(|t| t.end() <= end)]
}

/// `text`'s runs between `$IFS` bytes joined by one space each, with nothing at
/// either end: the spelling a pattern written with single spaces matches.
/// Blind to quoting, as [`split_ifs`] is, so these are `text`'s words only when
/// it carries no quoting, as a regex capture of plain words does.
pub(crate) fn squeeze_blanks(text: &str) -> String {
    split_ifs(text).collect::<Vec<_>>().join(" ")
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
    for Scanned {
        byte: b,
        in_single,
        in_double,
        ..
    } in QuoteScan::new(cmd).significant()
    {
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

/// The tokens of `input`, every one a slice of it: words (`Arg`), operators,
/// pipes, redirects, shellisms, the `Sep` runs between words and each unquoted
/// `Newline`.
pub fn tokenize(input: &str) -> Vec<Token<'_>> {
    lex(input, 0)
}

/// [`tokenize`] for a piece that starts at byte `base` of a larger input: every
/// offset is into that larger input.
pub(crate) fn tokenize_at(input: &str, base: usize) -> Vec<Token<'_>> {
    lex(input, base)
}

fn lex(input: &str, base: usize) -> Vec<Token<'_>> {
    let mut lexer = Lexer {
        input,
        base,
        tokens: Vec::new(),
        word_start: None,
        scan: QuoteScan::new(input).peekable(),
    };
    lexer.run();
    lexer.tokens
}

struct Lexer<'a> {
    input: &'a str,
    /// Added to every offset pushed.
    base: usize,
    tokens: Vec<Token<'a>>,
    /// Where the word being read began, if one is.
    word_start: Option<usize>,
    scan: Peekable<QuoteScan<'a>>,
}

impl<'a> Lexer<'a> {
    fn run(&mut self) {
        while let Some(c) = self.scan.next() {
            let i = c.index;
            // Quoted and escaped bytes are text, and so is the quote opening a
            // span: none of them ends or starts anything.
            if !c.is_bare() || c.byte == b'\'' || c.byte == b'"' {
                self.word_start.get_or_insert(i);
                continue;
            }
            match c.byte {
                b'$' => {
                    self.flush(i);
                    if self.next_is(|b| b.is_ascii_alphabetic() || b == b'_') {
                        let end =
                            self.take_while(i + 1, |b| b.is_ascii_alphanumeric() || b == b'_');
                        self.push(TokenKind::Arg, i, end);
                    } else {
                        self.push(TokenKind::Shellism, i, i + 1);
                    }
                }
                b'*' | b'?' | b'`' | b'(' | b')' | b'{' | b'}' | b'!' => {
                    self.flush(i);
                    self.push(TokenKind::Shellism, i, i + 1);
                }
                b'|' => {
                    self.flush(i);
                    if self.eat(b'|') {
                        self.push(TokenKind::Operator, i, i + 2);
                    } else if self.eat(b'&') {
                        self.push(TokenKind::Pipe(PipeKind::StdoutAndStderr), i, i + 2);
                    } else {
                        self.push(TokenKind::Pipe(PipeKind::Stdout), i, i + 1);
                    }
                }
                b';' => {
                    self.flush(i);
                    // `;;`, `;&` and `;;&` are single `case` terminators. Split
                    // apart, the second half reads as an empty command, and the
                    // rewrite emits `; ;` or `; &` in its place — a syntax error,
                    // or a background job where a fall-through was written.
                    let mut end = i + 1;
                    if self.eat(b';') {
                        end += 1;
                    }
                    if self.eat(b'&') {
                        end += 1;
                    }
                    self.push(TokenKind::Operator, i, end);
                }
                b'&' => {
                    self.flush(i);
                    if self.eat(b'&') {
                        self.push(TokenKind::Operator, i, i + 2);
                    } else if self.eat(b'>') {
                        let end = if self.eat(b'>') { i + 3 } else { i + 2 };
                        self.push(TokenKind::Redirect, i, end);
                    } else {
                        self.push(TokenKind::Shellism, i, i + 1);
                    }
                }
                b'>' => {
                    let start = self.fd_prefix_start(i);
                    let mut end = i + 1;
                    if self.eat(b'>') {
                        end += 1;
                    }
                    if self.eat(b'&') {
                        end = self.take_while(end + 1, |b| b.is_ascii_digit() || b == b'-');
                    }
                    self.push(TokenKind::Redirect, start, end);
                }
                b'<' => {
                    // A leading fd number belongs to the redirect, as in the `>`
                    // arm: left as its own word it reads as command text, and the
                    // redirect behind it then looks like a trailing one.
                    let start = self.fd_prefix_start(i);
                    let mut end = i + 1;
                    if self.eat(b'<') {
                        end += 1;
                    } else if self.eat(b'&') {
                        // `<&N` is one redirection operator, as `>&N` is in the
                        // `>` arm: split apart, its `&` reads as a background
                        // operator and its `N` as a command.
                        end = self.take_while(end + 1, |b| b.is_ascii_digit() || b == b'-');
                    }
                    self.push(TokenKind::Redirect, start, end);
                }
                b'\n' => {
                    self.flush(i);
                    self.push(TokenKind::Newline, i, i + 1);
                }
                b' ' | b'\t' => {
                    self.flush(i);
                    let end = self.take_while(i + 1, |b| b == b' ' || b == b'\t');
                    self.push(TokenKind::Sep, i, end);
                }
                _ => {
                    self.word_start.get_or_insert(i);
                }
            }
        }
        self.flush(self.input.len());
    }

    fn push(&mut self, kind: TokenKind, start: usize, end: usize) {
        self.tokens.push(Token {
            kind,
            value: &self.input[start..end],
            offset: self.base + start,
        });
    }

    /// Ends the word being read, if any, at `at`.
    fn flush(&mut self, at: usize) {
        if let Some(start) = self.word_start.take() {
            self.push(TokenKind::Arg, start, at);
        }
    }

    /// Where a redirect operator at `at` starts: at the word before it when
    /// that word is all digits (`2>`), else at the operator itself.
    fn fd_prefix_start(&mut self, at: usize) -> usize {
        match self.word_start {
            Some(start)
                if self.input.as_bytes()[start..at]
                    .iter()
                    .all(u8::is_ascii_digit) =>
            {
                self.word_start = None;
                start
            }
            _ => {
                self.flush(at);
                at
            }
        }
    }

    fn next_is(&mut self, accept: impl Fn(u8) -> bool) -> bool {
        self.scan
            .peek()
            .is_some_and(|c| c.is_bare() && accept(c.byte))
    }

    /// Consumes the next byte if it is a bare `b`.
    fn eat(&mut self, b: u8) -> bool {
        let found = self.next_is(|n| n == b);
        if found {
            self.scan.next();
        }
        found
    }

    /// Consumes bare bytes `accept` takes, and returns the offset past them,
    /// `from` when there are none.
    fn take_while(&mut self, from: usize, accept: impl Fn(u8) -> bool) -> usize {
        let mut end = from;
        while self.next_is(&accept) {
            if let Some(c) = self.scan.next() {
                end = c.index + 1;
            }
        }
        end
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
    QuoteScan::new(cmd).significant().any(|c| {
        let opens_paren = bytes.get(c.index + 1) == Some(&b'(');
        match c.byte {
            b'`' => !c.in_single,
            b'$' => !c.in_single && opens_paren,
            b'<' | b'>' => !c.in_single && !c.in_double && opens_paren,
            _ => false,
        }
    })
}

/// Grammar that can open a command without being part of it, so it does not
/// count as command text: a redirect behind one is still a leading redirect.
fn is_grammar_word(value: &str) -> bool {
    matches!(value, "{" | "}" | "!")
}

// `>&N`/`>&-` (and `N>&M`) is fd-dup/close; bare `>&` before a word is
// `>word 2>&1` — a file target. The operand is the next token that is not a
// blank.
pub(crate) fn redirect_has_file_target(tokens: &[Token<'_>], i: usize) -> bool {
    let value = tokens[i].value;
    // `<&` duplicates a descriptor exactly as `>&` does, and the tokenizer
    // only folds digits or `-` after either, so neither can name a file.
    if let Some(pos) = value.find(">&").or_else(|| value.find("<&")) {
        let tail = &value[pos + 2..];
        if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit() || c == '-') {
            return false;
        }
    }
    match tokens[i + 1..].iter().find(|t| !t.is_blank()) {
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

/// How a policy reads the end of a line.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum NewlinePolicy {
    /// A `Newline` separates words like a `Sep`.
    Blank,
    /// A `Newline` ends a segment, and so does an unquoted `\r`, although bash
    /// reads that as a word byte: the line is cut there and each piece
    /// segmented on its own, so that no command can hide behind one.
    EndsSegmentAndCrCuts,
}

/// What counts as the end of a segment, and what the segment keeps.
///
/// The permission gate, analytics classification and the rewrite each need a
/// different answer, and the gate's must be the most conservative — a segment
/// it never sees is a command its rules never check. Naming the differences
/// here keeps them chosen rather than emergent.
#[derive(Clone, Copy)]
pub(crate) struct Policy {
    newline: NewlinePolicy,
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
        newline: NewlinePolicy::EndsSegmentAndCrCuts,
        group_boundaries: true,
        redirects: RedirectPolicy::Excise,
        descend_into_substitution: true,
    };

    /// Classification only, never a security decision.
    pub(crate) const CLASSIFY: Self = Self {
        newline: NewlinePolicy::Blank,
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
    let text = trim_ifs(raw);
    if text.is_empty() {
        return;
    }
    let lead = raw.len() - trim_ifs_start(raw).len();
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
    pub(crate) fn observe(&mut self, tok: &Token<'_>, at_command_position: bool) {
        match (&tok.kind, tok.value) {
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

/// How deep a walk currently is inside `$( )`, `<( )` or `>( )`.
///
/// The segmenter and the emitter walk the same tokens and must agree about
/// where a substitution starts and stops — one counting a bracket the other
/// does not is a command appearing or vanishing. They share this rather than
/// each keeping their own count.
#[derive(Default)]
pub(crate) struct SubstitutionDepth(usize);

impl SubstitutionDepth {
    /// Whether the token being looked at sits inside a substitution, where
    /// nothing ends a command out here.
    pub(crate) fn is_inside(&self) -> bool {
        self.0 > 0
    }

    /// Take `tok` into account, and say whether it was the bracketing of a
    /// substitution — the `(` that opens one or the `)` that closes it — which
    /// is never a boundary in its own right.
    pub(crate) fn absorbs(&mut self, cmd: &str, tok: &Token<'_>) -> bool {
        if tok.kind != TokenKind::Shellism {
            return false;
        }
        match tok.value {
            // Brackets nest, so one inside a substitution counts too: without
            // that its `)` closes the substitution a bracket early and the real
            // closing `)` reads as a boundary.
            "(" if self.0 > 0 => {
                self.0 += 1;
                true
            }
            "(" if opens_substitution(cmd, tok.offset) => {
                self.0 += 1;
                true
            }
            ")" if self.0 > 0 => {
                self.0 -= 1;
                true
            }
            _ => false,
        }
    }
}

/// Split a compound command into the commands it runs, under `policy`.
///
/// Offsets are relative to `trim_ifs(cmd)`, which is what every caller matches
/// against.
pub(crate) fn segment(cmd: &str, policy: Policy) -> Vec<Segment<'_>> {
    let trimmed = trim_ifs(cmd);
    let mut walk = SegmentWalk {
        input: trimmed,
        policy,
        out: Vec::new(),
        seg_start: 0,
        seg_end: None,
        seg_has_text: false,
        substitution: SubstitutionDepth::default(),
        cases: CaseTracker::default(),
    };
    let cuts: Vec<usize> = match policy.newline {
        NewlinePolicy::Blank => Vec::new(),
        NewlinePolicy::EndsSegmentAndCrCuts => QuoteScan::new(trimmed)
            .filter(|c| c.byte == b'\r' && c.is_bare())
            .map(|c| c.index)
            .collect(),
    };
    let mut from = 0;
    for cut in cuts.into_iter().chain(once(trimmed.len())) {
        walk.read(from, cut);
        walk.end_segment(cut, false);
        walk.seg_start = cut + 1;
        from = cut + 1;
    }
    walk.out
}

/// The state [`segment`] carries from token to token, and across the pieces a
/// policy cuts the line into.
struct SegmentWalk<'a> {
    input: &'a str,
    policy: Policy,
    out: Vec<Segment<'a>>,
    seg_start: usize,
    seg_end: Option<usize>,
    seg_has_text: bool,
    substitution: SubstitutionDepth,
    cases: CaseTracker,
}

impl SegmentWalk<'_> {
    /// Ends the current segment at `at`, or where a trailing redirect cut it.
    fn end_segment(&mut self, at: usize, feeds_pipe: bool) {
        let end = self.seg_end.take().unwrap_or(at);
        push_segment(&mut self.out, self.input, self.seg_start, end, feeds_pipe);
        self.seg_has_text = false;
    }

    /// Reads `input[from..to]`, lexed on its own.
    fn read(&mut self, from: usize, to: usize) {
        let tokens = tokenize_at(&self.input[from..to], from);
        let policy = self.policy;

        let mut i = 0;
        while let Some(tok) = tokens.get(i) {
            let at_command_position = !self.seg_has_text;
            // The gate descends into a substitution, so for it the tracker never
            // engages and everything inside is read as ordinary commands.
            let bracketing =
                !policy.descend_into_substitution && self.substitution.absorbs(self.input, tok);
            let is_boundary = !bracketing
                && !self.substitution.is_inside()
                && match tok.kind {
                    TokenKind::Operator | TokenKind::Pipe(_) => true,
                    TokenKind::Newline => policy.newline == NewlinePolicy::EndsSegmentAndCrCuts,
                    // `case x in (ls) …` is the same statement as `case x in ls) …`,
                    // so that `(` opens nothing. Ending a command there would make
                    // the pattern a command position, and a rewrite turns its one
                    // word into two, which bash rejects. The `)` still ends it.
                    TokenKind::Shellism if tok.value == "(" && self.cases.in_pattern() => false,
                    TokenKind::Shellism => {
                        policy.group_boundaries && matches!(tok.value, "&" | "(" | ")")
                    }
                    TokenKind::Arg | TokenKind::Redirect | TokenKind::Sep => false,
                };

            if is_boundary {
                self.end_segment(tok.offset, matches!(tok.kind, TokenKind::Pipe(_)));
                self.seg_start = tok.end();
            } else if tok.kind == TokenKind::Redirect && policy.redirects == RedirectPolicy::Excise
            {
                if !self.seg_has_text {
                    // A redirect may precede the command it applies to, and that
                    // command still has to reach the caller, so step over the
                    // redirect instead of ending the segment at it.
                    //
                    // The operand runs to the first blank or boundary. `>$HOME/x`
                    // is several tokens but one word, and a boundary ends the
                    // operand even with no blank — in `>a|rm -rf /` the `|` starts
                    // the next command rather than continuing the filename.
                    let mut end = tok.end();
                    let mut next = i + 1;
                    while let Some(part) = tokens.get(next) {
                        match part.kind {
                            TokenKind::Operator
                            | TokenKind::Pipe(_)
                            | TokenKind::Shellism
                            | TokenKind::Sep
                            | TokenKind::Newline => break,
                            TokenKind::Arg | TokenKind::Redirect => {}
                        }
                        end = part.end();
                        next += 1;
                    }
                    self.seg_start = end;
                    i = next;
                    continue;
                } else if self.seg_end.is_none() {
                    self.seg_end = Some(tok.offset);
                }
            } else if tok.kind == TokenKind::Arg
                || (tok.kind == TokenKind::Shellism && !is_grammar_word(tok.value))
            {
                self.seg_has_text = true;
            }

            self.cases.observe(tok, at_command_position);
            i += 1;
        }
    }
}

/// Segments `cmd` for the **permission gate** (`hooks/permissions.rs::check_command_with_rules`):
/// every segment this returns is independently checked against deny/ask/allow
/// rules, so this is the most paranoid of the three compound-command segmenters
/// in this codebase — see [`split_for_classify`] (analytics/discovery
/// classification) and `discover/registry.rs::rewrite_compound`'s inline token walk
/// (actual rewrite) for the other two.
///
/// All three agree on where a command begins and ends. Where a row below still
/// differs, the difference is the consumer's purpose, not an accident, and
/// `discover/registry.rs`'s `segmenter_agreement` tests hold each one to a stated reason:
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
/// (`NewlinePolicy::EndsSegmentAndCrCuts`), descends into `$( )`, and truncates each
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

/// Turns a word's raw text (quotes/escapes still literal, as `tokenize()`
/// preserves them) into argv-ready text: quote chars that open/close a span are
/// stripped, backslash escapes resolved.
pub(crate) fn resolve_word_text(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut result = String::with_capacity(raw.len());
    // Bytes are dropped one at a time and all of them are ASCII, so every run
    // kept between two dropped bytes is whole UTF-8.
    let mut kept_from = 0;
    let mut skip = |result: &mut String, at: usize| {
        result.push_str(&raw[kept_from..at]);
        kept_from = at + 1;
    };
    let mut escaping = false;

    for c in QuoteScan::new(raw) {
        if escaping {
            // The escaped byte itself, which is always text.
            escaping = false;
            continue;
        }
        if c.escaped {
            escaping = true;
            // Inside double quotes bash only lets `\` escape `$`, `` ` ``, `"`,
            // `\` or a newline; before anything else it is a literal character.
            // That is what keeps a quoted Windows path (`"C:\Program Files"`)
            // intact instead of eating its separators.
            let escapes = !c.in_double
                || matches!(
                    bytes.get(c.index + 1),
                    Some(b'$' | b'`' | b'"' | b'\\' | b'\n')
                );
            if escapes {
                skip(&mut result, c.index);
            }
            continue;
        }
        // Only a quote that opens or closes a span is syntax: a `'` inside
        // `"..."`, or a `"` inside `'...'`, is literal text.
        let toggles = match c.byte {
            b'\'' => !c.in_double,
            b'"' => !c.in_single,
            _ => false,
        };
        if toggles {
            skip(&mut result, c.index);
        }
    }
    result.push_str(&raw[kept_from..]);
    result
}

/// Quote-aware split of a single shell command into argv-ready words: quotes
/// stripped, backslash escapes resolved — for callers that hand the result
/// straight to `Command::new`/exec or compare it against literal words
/// (`hooks/mod.rs::is_claude_hook_command`, `rtk proxy` arg-splitting).
pub fn shell_split(input: &str) -> Vec<String> {
    words(input, &tokenize(input))
        .into_iter()
        .map(|w| resolve_word_text(w.text))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scan reports every byte and says which are escaped, so a caller
    /// asking about syntax and a caller asking about text get different
    /// answers from one place rather than one answer and a workaround.
    #[test]
    fn test_quote_scan_reports_escaped_bytes_without_letting_them_act() {
        let all: Vec<u8> = QuoteScan::new(r"a\'b").map(|c| c.byte).collect();
        assert_eq!(all, b"a\\'b", "every byte is reported");

        let significant: Vec<u8> = QuoteScan::new(r"a\'b")
            .significant()
            .map(|c| c.byte)
            .collect();
        assert_eq!(significant, b"ab", "the escape and its byte are not syntax");

        // An escaped quote opens nothing, so what follows is not quoted — and
        // a real one after it still is.
        let quoted: Vec<(u8, bool)> = QuoteScan::new(r"\'a'b")
            .significant()
            .map(|c| (c.byte, c.in_single))
            .collect();
        assert_eq!(quoted, vec![(b'a', false), (b'\'', false), (b'b', true)]);

        assert!(
            quotes_balanced_for_test(r"echo \'"),
            "an escaped quote leaves nothing open"
        );
        assert!(!quotes_balanced_for_test("echo '"), "a real quote does");
    }

    /// A pending escape is consumed before a fresh backslash is looked for.
    /// Reverse those two and a run of backslashes never finishes escaping, so
    /// the real quote after it is swallowed as an escaped byte and never opens
    /// the string — `\\\\'x` would report no quote and `x` as unquoted.
    #[test]
    fn test_quote_scan_finishes_one_escape_before_starting_another() {
        let seen: Vec<(u8, bool)> = QuoteScan::new(r"\\\\'x")
            .significant()
            .map(|c| (c.byte, c.in_single))
            .collect();
        assert_eq!(
            seen,
            vec![(b'\'', false), (b'x', true)],
            "two escaped backslashes, then a quote that really opens"
        );
    }

    fn quotes_balanced_for_test(cmd: &str) -> bool {
        let mut scan = QuoteScan::new(cmd);
        scan.by_ref().for_each(drop);
        scan.balanced()
    }

    fn word_texts(cmd: &str) -> Vec<&str> {
        words(cmd, &tokenize(cmd))
            .into_iter()
            .map(|w| w.text)
            .collect()
    }

    /// `words` decides where a word ends for anyone who needs to know —
    /// `split_env_prefix` among them — so it is worth asserting on its own
    /// rather than only through a caller that happens to exercise it.
    #[test]
    fn test_words_reads_a_quoted_span_as_one_word() {
        assert_eq!(word_texts("git status"), vec!["git", "status"]);
        assert_eq!(
            word_texts("D='# shellcheck disable=SC2034' git status"),
            vec!["D='# shellcheck disable=SC2034'", "git", "status"]
        );
        assert_eq!(word_texts("FOO=\"a b\" ls"), vec!["FOO=\"a b\"", "ls"]);
        assert_eq!(
            word_texts("a\\ b c"),
            vec!["a\\ b", "c"],
            "an escaped blank joins"
        );
        assert_eq!(word_texts("a\tb"), vec!["a", "b"], "a tab separates");
        assert_eq!(word_texts("a\nb"), vec!["a", "b"], "a newline separates");
        assert_eq!(
            word_texts("echo \"a'b'c\" x"),
            vec!["echo", "\"a'b'c\"", "x"]
        );
        assert_eq!(word_texts("echo '' x"), vec!["echo", "''", "x"]);
        assert_eq!(word_texts("café日本語 x"), vec!["café日本語", "x"]);
        // A word of nothing but escapes is still a word.
        assert_eq!(word_texts(r"git status \'"), vec!["git", "status", r"\'"]);
        assert_eq!(word_texts(r"\' foo"), vec![r"\'", "foo"]);
        assert_eq!(word_texts(r"a \' b"), vec!["a", r"\'", "b"]);
        // Adjacent tokens are one word: a glob, and an operator with no blank.
        assert_eq!(
            word_texts("golangci-lint --config *.yml run"),
            vec!["golangci-lint", "--config", "*.yml", "run"]
        );
        assert_eq!(word_texts("a;b c"), vec!["a;b", "c"]);

        assert!(word_texts("").is_empty());
        assert!(word_texts("   \t  ").is_empty());
        assert_eq!(
            word_texts("  ls  "),
            vec!["ls"],
            "outer blanks are not words"
        );
    }

    /// Only space, tab and newline separate words, as in bash: a vertical tab,
    /// a form feed, a carriage return or a non-breaking space is a word byte.
    #[test]
    fn test_words_split_on_ifs_only() {
        for byte in ["\x0b", "\x0c", "\r", "\u{a0}"] {
            let cmd = format!("git{byte}status");
            assert_eq!(word_texts(&cmd), vec![cmd.as_str()], "{cmd:?}");
            let cmd = format!("git status{byte}");
            assert_eq!(
                word_texts(&cmd),
                vec!["git", &cmd[4..]],
                "trailing {byte:?} stays in the word"
            );
        }
    }

    /// Each word carries its span and the text that span names.
    #[test]
    fn test_words_carry_their_span_and_text() {
        let cmd = "FOO='a b' git  *.rs";
        let found = words(cmd, &tokenize(cmd));
        assert_eq!(
            found.iter().map(|w| (w.start, w.end)).collect::<Vec<_>>(),
            vec![(0, 9), (10, 13), (15, 19)]
        );
        for word in &found {
            assert_eq!(word.text, &cmd[word.start..word.end]);
        }
    }

    /// Nothing here should panic or lose a byte, however the quoting ends.
    #[test]
    fn test_words_survive_unfinished_quoting() {
        for cmd in ["echo 'abc", "echo \"abc", "echo abc\\", "'", "\\"] {
            for word in words(cmd, &tokenize(cmd)) {
                assert!(
                    word.start < word.end && word.end <= cmd.len(),
                    "bad span in {cmd:?}"
                );
                assert!(cmd.is_char_boundary(word.start) && cmd.is_char_boundary(word.end));
            }
        }
    }

    /// `$'…'` is read as a plain `'…'`, so the backslash inside it closes the
    /// string where bash would keep it open (#3188). That belongs to the shared
    /// scanner and is why `ansi_c_quote_defeats_lexer` exists as a separate
    /// guard; pinned here so the limitation is visible where the splitting is.
    #[test]
    fn test_word_spans_reads_ansi_c_quoting_as_ordinary_quoting() {
        let cmd = "D=$'ansi\\'c' git status";
        assert_ne!(
            word_texts(cmd).len(),
            3,
            "if this ever splits into three words the limitation is gone and \
             the comment above should go with it"
        );
        assert!(
            ansi_c_quote_defeats_lexer(cmd),
            "the guard that does catch it"
        );
    }

    /// Every token is a slice of the input, and together they are all of it.
    #[test]
    fn test_tokens_tile_their_input() {
        for cmd in [
            "git status",
            "  a  &&\tb || c ;; d |& e\n",
            "FOO=1 2>&1 >>out <&3 &>/dev/null $HOME ${X} *.rs",
            "echo 'a b' \"c\\\"d\" e\\ f \\",
            "git status\r\ngit log\rx\x0b\u{a0}y",
            "echo 'unterminated",
            "café;;&fin",
        ] {
            let tokens = tokenize(cmd);
            let mut at = 0;
            for tok in &tokens {
                assert_eq!(tok.offset, at, "gap before {tok:?} in {cmd:?}");
                assert_eq!(tok.value, &cmd[tok.offset..tok.end()]);
                at = tok.end();
            }
            assert_eq!(at, cmd.len(), "tokens stop short in {cmd:?}");
        }
    }

    #[test]
    fn test_sep_carries_the_exact_blank_bytes() {
        let seps: Vec<&str> = tokenize("a \t b\tc")
            .into_iter()
            .filter(|t| t.kind == TokenKind::Sep)
            .map(|t| t.value)
            .collect();
        assert_eq!(seps, vec![" \t ", "\t"]);
    }

    /// `tokenize_at` is `tokenize` with its offsets moved, and nothing else.
    #[test]
    fn test_tokenize_variants_agree_with_tokenize() {
        for input in [
            "git status && ls -la\n  cargo test 2>&1 | grep x",
            "a\r\n\tb\x0bc \u{a0}d",
            "",
            "echo 'a  b' \"c\td\" $(x)",
        ] {
            let full = tokenize(input);
            let shifted: Vec<Token<'_>> = full
                .iter()
                .map(|t| Token {
                    offset: t.offset + 7,
                    ..*t
                })
                .collect();
            assert_eq!(tokenize_at(input, 7), shifted, "{input:?}");
        }
        assert_eq!(
            split_ifs(" a\t\tb\r \x0bc\n").collect::<Vec<_>>(),
            vec!["a", "b\r", "\x0bc"]
        );
        assert_eq!(squeeze_blanks("pm \t\n ls "), "pm ls");
    }

    /// A command ends where its last word does. An escaped or quoted blank is
    /// part of that word, so only a bare blank is trimmed.
    #[test]
    fn test_trimmed_text_ends_at_the_last_word() {
        for (input, expected) in [
            ("  git status \t\n", "git status"),
            ("head\\ ", "head\\ "),
            ("cat f\\ \t", "cat f\\ "),
            ("cat f\\\t", "cat f\\\t"),
            ("echo 'a ", "echo 'a "),
            ("echo \"a\t\"  ", "echo \"a\t\""),
            ("ls \x0b", "ls \x0b"),
            (" \t\n", ""),
            ("", ""),
        ] {
            let (trimmed, tokens) = tokenize_trimmed(input);
            assert_eq!(trimmed, expected, "{input:?}");
            assert_eq!(tokens, tokenize(trimmed), "{input:?}");
        }

        let cmd = "a | head\\  ; b";
        let tokens = tokenize(cmd);
        let pipe_end = cmd.find('|').expect("pipe") + 1;
        let semi = cmd.find(';').expect("semicolon");
        assert_eq!(
            content_span(&tokens, pipe_end, semi).map(|(from, to)| &cmd[from..to]),
            Some("head\\ ")
        );
        assert_eq!(content_span(&tokens, semi + 1, semi + 1), None);
        assert_eq!(tokens_until(&tokens, semi), &tokens[..tokens.len() - 3]);
    }

    /// The tokens that are not blanks, as `(kind, value)`.
    fn non_blank(cmd: &str) -> Vec<(TokenKind, &str)> {
        tokenize(cmd)
            .into_iter()
            .filter(|t| !t.is_blank())
            .map(|t| (t.kind, t.value))
            .collect()
    }

    #[test]
    fn test_simple_command() {
        let tokens = tokenize("git status");
        assert_eq!(
            tokens.iter().map(|t| (t.kind, t.value)).collect::<Vec<_>>(),
            vec![
                (TokenKind::Arg, "git"),
                (TokenKind::Sep, " "),
                (TokenKind::Arg, "status")
            ]
        );
    }

    #[test]
    fn test_command_with_args() {
        let values: Vec<&str> = non_blank("git commit -m message")
            .into_iter()
            .map(|(_, v)| v)
            .collect();
        assert_eq!(values, vec!["git", "commit", "-m", "message"]);
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
        assert!(non_blank("   ").is_empty());
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
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[1].value, "   ");
    }

    #[test]
    fn test_leading_trailing_spaces() {
        assert_eq!(non_blank("  git status  ").len(), 2);
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
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[2].kind, TokenKind::Redirect);
        assert_eq!(tokens[2].value, "2>&1");
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
        let offsets: Vec<usize> = tokenize("a && b").iter().map(|t| t.offset).collect();
        assert_eq!(offsets, vec![0, 1, 2, 4, 5]);
    }

    #[test]
    fn test_offset_segment_extraction() {
        let cmd = "git add . && cargo test";
        let tokens = tokenize(cmd);
        let op = tokens
            .iter()
            .find(|t| t.kind == TokenKind::Operator)
            .unwrap();
        let left = trim_ifs(&cmd[..op.offset]);
        let right = trim_ifs(&cmd[op.end()..]);
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
            // Process substitution, which the boundary rules treat like `$( )`
            // and which nothing else in this corpus produces.
            "diff <(ls) <(rm -rf /)",
            "tee >(rm -rf /) < in",
            "diff <(ls) && rm -rf /",
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
            // About a third of the corpus reaches this: the redirect variants
            // are eight of nine and every one of them carries a `<` or `>`.
            // What is left still crosses every joiner with every wrapper, which
            // is where this commit's boundaries live. `<(`/`>(` fall out with
            // the redirects, and `$(` is named because it has no angle bracket.
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
            // discover/registry.rs's `segmenter_agreement`.
            let trimmed = trim_ifs(cmd);
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
            let trimmed = trim_ifs(cmd);
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
                    assert_eq!(trim_ifs(seg.text), seg.text, "segment text was not trimmed");
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
    fn test_newline_token_outside_quotes_only() {
        fn newlines(input: &str) -> Vec<(usize, &str)> {
            tokenize(input)
                .into_iter()
                .filter(|t| t.kind == TokenKind::Newline)
                .map(|t| (t.offset, t.value))
                .collect()
        }
        assert_eq!(newlines("git status\ngit log"), vec![(10, "\n")]);
        assert!(newlines("echo 'line1\nline2'").is_empty());
        // The `\r` of a CRLF is the last byte of the word before the newline.
        assert_eq!(newlines("git status\r\ngit log"), vec![(11, "\n")]);
        // A lone `\r` is never a newline.
        assert!(newlines("git status\rgit log").is_empty());
    }

    #[test]
    fn test_lone_cr_is_not_a_word_boundary() {
        // Bash's default $IFS is space/tab/newline, never CR: a bare `\r` with no
        // following `\n` stays glued into its surrounding word instead of splitting
        // it, matching how real bash tokenizes `git status<CR>git log`.
        assert_eq!(
            word_texts("git status\rgit log"),
            vec!["git", "status\rgit", "log"]
        );
    }

    #[test]
    fn test_crlf_keeps_cr_glued_to_word() {
        assert_eq!(
            word_texts("git status\r\ngit log"),
            vec!["git", "status\r", "git", "log"]
        );
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

    // The tokenizer the walkers above read, as it shipped with them: frozen for
    // the same reason, so the oracle does not move with the lexer it checks.

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct LegacyToken {
        pub kind: TokenKind,
        pub value: String,
        pub offset: usize,
    }

    #[allow(dead_code)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum NewlineMode {
        None,
        Bash,
        Conservative,
    }

    fn tokenize(input: &str) -> Vec<LegacyToken> {
        tokenize_inner(input, NewlineMode::None)
    }

    fn advance_quote_state(quote: Option<char>, c: char) -> Option<char> {
        match (quote, c) {
            (None, '\'' | '"') => Some(c),
            (Some(q), c) if c == q => None,
            (q, _) => q,
        }
    }

    fn is_word_boundary_whitespace(c: char) -> bool {
        matches!(c, ' ' | '\t' | '\n')
    }

    fn is_crlf_at(bytes: &[u8], i: usize) -> bool {
        bytes.get(i) == Some(&b'\r') && bytes.get(i + 1) == Some(&b'\n')
    }

    fn tokenize_inner(input: &str, newline_mode: NewlineMode) -> Vec<LegacyToken> {
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
                        tokens.push(LegacyToken {
                            kind: TokenKind::Arg,
                            value: name,
                            offset: start,
                        });
                    } else {
                        tokens.push(LegacyToken {
                            kind: TokenKind::Shellism,
                            value: "$".into(),
                            offset: start,
                        });
                    }
                    current_start = byte_pos;
                }
                '*' | '?' | '`' | '(' | ')' | '{' | '}' | '!' => {
                    flush_arg(&mut tokens, &mut current, current_start);
                    tokens.push(LegacyToken {
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
                        tokens.push(LegacyToken {
                            kind: TokenKind::Operator,
                            value: "||".into(),
                            offset: start,
                        });
                    } else if chars.peek() == Some(&'&') {
                        chars.next();
                        byte_pos += 1;
                        tokens.push(LegacyToken {
                            kind: TokenKind::Pipe(PipeKind::StdoutAndStderr),
                            value: "|&".into(),
                            offset: start,
                        });
                    } else {
                        tokens.push(LegacyToken {
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
                    tokens.push(LegacyToken {
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
                        tokens.push(LegacyToken {
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
                        tokens.push(LegacyToken {
                            kind: TokenKind::Redirect,
                            value: val,
                            offset: start,
                        });
                    } else {
                        tokens.push(LegacyToken {
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
                    tokens.push(LegacyToken {
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
                    tokens.push(LegacyToken {
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
                    tokens.push(LegacyToken {
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

    fn flush_arg(tokens: &mut Vec<LegacyToken>, current: &mut String, offset: usize) {
        if !current.is_empty() {
            tokens.push(LegacyToken {
                kind: TokenKind::Arg,
                value: std::mem::take(current),
                offset,
            });
        }
    }
}
