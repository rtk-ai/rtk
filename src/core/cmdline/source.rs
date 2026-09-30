//! The text of a command as bash reads it before it has words: which `#` opens a
//! comment and which `\⏎` joins two lines.
//!
//! Neither can be told from one flat quote state. Bash reads quoting afresh in a
//! `$( )` inside double quotes, a `#` is text in `${x#y}` and `$(( 16#ff ))` and a
//! comment in `$(ls # c⏎)`, and a `)` in that comment closes nothing. So this reader
//! follows the nesting, the way bash's own parser does. Past what it can follow, or
//! where bash reads one text in two ways, [`SourceReading::too_complex`] says so.
//!
//! The lexer ([`super::lexer`]) stays flat, and reads what this finds.

use std::borrow::Cow;
use std::ops::Range;

use super::bash_grammar::{Position, RESERVED_WORDS, Reserved, ReservedWord, reserved_word};
use super::lexer::name_len;

/// How many levels of nesting the reader follows.
const MAX_NESTING: usize = 16;

/// Bytes the reader may visit for each byte of text: `$((` is read twice when it
/// turns out not to be arithmetic, so a text built to nest them could cost more.
const STEPS_PER_BYTE: usize = 16;

/// What [`read_source`] finds in a text.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct SourceReading {
    /// Each comment, up to the `\n` or backquote that ends it. A comment inside
    /// double quotes is not among them: to the flat lexer it is quoted text.
    pub(super) comments: Vec<Range<usize>>,
    /// What joining the lines takes away, in order.
    pub(super) cuts: Vec<Cut>,
    /// Each `#` it read as text where bash may start a comment, to the line's end.
    pub(super) doubtful_comments: Vec<Range<usize>>,
    /// The reading above is the reader's best effort, and it may not be bash's.
    pub(super) too_complex: bool,
}

/// One or more line continuations, and the blanks around them if bash reads those
/// as separating words.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Cut {
    pub(super) range: Range<usize>,
    /// Blanks stood on either side, so one is left to separate the same words.
    pub(super) leaves_a_blank: bool,
}

/// Reads `text`, whole, from where a command starts.
pub(super) fn read_source(text: &str) -> SourceReading {
    let mut reader = Reader {
        text: text.as_bytes(),
        at: 0,
        end: text.len(),
        depth: 0,
        steps_left: text.len().saturating_mul(STEPS_PER_BYTE).saturating_add(64),
        in_double_quotes: 0,
        in_backquotes: 0,
        in_failed_arithmetic: 0,
        heredocs: Vec::new(),
        reading: SourceReading::default(),
    };
    reader.command(false, Paren::Commands);
    reader.reading
}

/// What a `(` opens.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Paren {
    /// Commands: a subshell, or the top of a substitution.
    Commands,
    /// A word stands right before it. Bash may read `@(a # b)` as a glob.
    Glued,
    /// `a=(…)`, whose words may be `[1 # 2]=x`.
    Assignment,
}

/// Whether the quoting around a nested read makes quotes in it ambiguous.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Around {
    /// Unquoted: a `$'` and a `$"` open quotes.
    Bare,
    /// Directly in double quotes: `$'` and `$"` are text.
    Double,
    /// In a `${ }` or `$[ ]` that double quotes hold. Bash closes it past a `'…'`,
    /// and yet may keep the quotes as text in the value: `"${x:-'a'}"` is `'a'` and
    /// `"${x#'a'}"` is not. What the two agree on is a quote holding nothing that
    /// means anything once it is text, its closer included.
    DoubleExpansion { closer: u8 },
}

struct Reader<'a> {
    text: &'a [u8],
    at: usize,
    /// Where the text being read ends: the backquote closing it, inside one.
    end: usize,
    depth: usize,
    steps_left: usize,
    /// Double quotes around what is being read, at any level.
    in_double_quotes: usize,
    /// Backquotes around it. Bash takes every `\⏎` out of their text before reading
    /// it, even in a quote or a comment, which then runs on.
    in_backquotes: usize,
    /// `((` that did not close as arithmetic around it. Bash read that text once
    /// already, as text, and what it does to a comment in it depends on where it
    /// stands.
    in_failed_arithmetic: usize,
    heredocs: Vec<Heredoc>,
    reading: SourceReading,
}

#[derive(Clone)]
struct Heredoc {
    delimiter: Vec<u8>,
    strips_tabs: bool,
    quoted: bool,
    /// The nesting it was named at, where the line that names it has to end.
    depth: usize,
}

/// What [`Reader::command`] knows of the command list it is reading.
#[derive(Default)]
struct CommandList {
    word_start: bool,
    command_position: bool,
    /// Where the word being read began, and whether it is plain text so far.
    word: Option<(usize, bool)>,
    /// Where the blanks that end at the byte being read began.
    blanks: Option<usize>,
    /// `case`s open here. Their pattern's `)` closes nothing.
    open_cases: usize,
    /// A `case` still has its `in` to come.
    awaiting_in: bool,
    /// A pattern is being read, up to its `)`: `case` is a word in it.
    in_pattern: bool,
    /// The word before is one that [`opens_a_command`], off a command's start.
    after_opener: bool,
    /// A `case` or `esac` stood where it may be text. Then a `)` may close a pattern or
    /// this list, and only bash can tell.
    doubtful_case: bool,
    /// Inside `[[ ]]`, where `(` is not a subshell and a `#` is text after one.
    in_condition: bool,
    /// `[` open in a word that starts with a variable's name, where no command word
    /// came before it. Bash may take it for an assignment and read to the `]` as one
    /// word, `x[1 # 2]=y`.
    open_brackets: usize,
    /// A word bash surely runs came before: `ls x[ #c` holds no subscript, and what
    /// reads `[[` as a word holds no condition. Only that much is sure, so anything
    /// that may stand ahead of a command does not count.
    ran_a_word: bool,
    /// The next word is what `for`, `select` or `function` names, and runs nothing.
    name_next: bool,
    /// Bash reads a reserved word next, as past a compound's last word and past a
    /// name: `if [[ a ]] then`, `if { a; } then`, `for x do`.
    reserved_next: bool,
    /// The next word follows `coproc`, and may name it: a reserved word may come after.
    after_coproc: bool,
    /// The next word is what a redirect takes.
    after_redirect: bool,
}

/// What a command can start after: in `function f {` the `{` stands where no
/// command starts, and its list holds commands all the same.
fn opens_a_command(word: &ReservedWord) -> bool {
    word.next == Position::Command
}

/// The reserved word that a plain word spells.
fn reserved_in(word: &[u8]) -> Option<&'static ReservedWord> {
    std::str::from_utf8(word).ok().and_then(reserved_word)
}

/// A compound's last word, where bash reads a reserved word.
fn closes_a_compound(word: &ReservedWord) -> bool {
    word.next == Position::ReservedWord && word.word != Reserved::CloseTest
}

/// What a name follows, where bash reads a reserved word: `coproc`'s word may also be
/// a command.
fn takes_a_name(word: &ReservedWord) -> bool {
    word.names && !opens_a_command(word)
}

impl CommandList {
    /// A byte of plain text.
    fn text(&mut self, at: usize) {
        if self.word_start || self.word.is_none() {
            self.word = Some((at, true));
        }
        self.word_start = false;
    }

    /// A part of the word that is not plain text: a quote, or an expansion.
    fn part(&mut self, at: usize) {
        self.word = Some((self.word.map_or(at, |(begin, _)| begin), false));
        self.word_start = false;
    }

    /// Whether the word read so far spells, as plain text, a reserved word `wanted` picks.
    fn word_is(&self, text: &[u8], at: usize, wanted: impl Fn(&ReservedWord) -> bool) -> bool {
        // Any `\⏎` in it has to be few for this to stay cheap: this runs at each `)`.
        self.word.is_some_and(|(begin, plain)| {
            plain
                && RESERVED_WORDS.iter().filter(|row| wanted(row)).any(|row| {
                    let word = row.spelling.as_bytes();
                    at - begin <= word.len() + 16 && joined(&text[begin..at]).as_ref() == word
                })
        })
    }

    /// The word being read ends: what it is says what comes next.
    fn end_word(&mut self, text: &[u8], at: usize) {
        let Some((begin, plain)) = self.word.take() else {
            return;
        };
        // What a redirect takes is no reserved word, and `fi >f then` goes on past it.
        if std::mem::take(&mut self.after_redirect) {
            self.command_position = false;
            self.after_opener = false;
            return;
        }
        let a_name = std::mem::take(&mut self.name_next);
        let reserved_here = std::mem::take(&mut self.reserved_next);
        self.reserved_next = std::mem::take(&mut self.after_coproc);
        if a_name {
            self.reserved_next = true;
            self.command_position = false;
            self.after_opener = false;
            return;
        }
        if !plain {
            self.command_position = false;
            return;
        }
        let word = joined(&text[begin..at]);
        let at_a_reserved_word = self.command_position || reserved_here;
        let row = reserved_in(&word);
        if let Some(row) =
            row.filter(|row| at_a_reserved_word && (closes_a_compound(row) || takes_a_name(row)))
        {
            if row.word == Reserved::Esac {
                self.open_cases = self.open_cases.saturating_sub(1);
                self.in_pattern = false;
            }
            self.name_next = takes_a_name(row);
            self.reserved_next = !self.name_next;
            self.command_position = false;
            self.after_opener = false;
            return;
        }
        let an_argument = self.ran_a_word;
        self.ran_a_word |= may_only_run(&word);
        match row.map(|row| row.word) {
            Some(Reserved::OpenTest) if !self.ran_a_word => self.in_condition = true,
            Some(Reserved::CloseTest) => {
                if std::mem::take(&mut self.in_condition) {
                    self.ran_a_word = false;
                    self.reserved_next = true;
                }
            }
            Some(word)
                if row.is_some_and(opens_a_command)
                    && (word == Reserved::OpenBrace || !self.ran_a_word) =>
            {
                let sure = self.command_position || reserved_here;
                self.after_opener = !sure;
                self.command_position = sure;
                self.ran_a_word = false;
                self.after_coproc = word == Reserved::Coproc;
                return;
            }
            // A pattern comes next, and it is no argument of a command.
            Some(Reserved::In) if self.awaiting_in => {
                self.awaiting_in = false;
                self.ran_a_word = false;
                self.in_pattern = true;
                self.reserved_next = true;
            }
            Some(Reserved::Case) if self.in_pattern => {}
            Some(Reserved::Case) if at_a_reserved_word => {
                self.open_cases += 1;
                self.awaiting_in = true;
            }
            Some(Reserved::Case) if self.after_opener => {
                self.open_cases += 1;
                self.awaiting_in = true;
                self.doubtful_case = true;
            }
            // Past `time -p`, say, with no command word to make it an argument: the `)`
            // it may open or close can't be told from the one that closes this list.
            Some(Reserved::Case) if !an_argument => self.doubtful_case = true,
            Some(Reserved::Esac) if !an_argument && self.open_cases > 0 => {
                self.doubtful_case = true;
            }
            _ => {}
        }
        self.command_position = false;
        self.after_opener = false;
    }

    /// A byte that ends a word and starts a command after it.
    fn separator(&mut self, text: &[u8], at: usize) {
        self.end_word(text, at);
        self.word_start = true;
        self.command_position = true;
        self.after_opener = false;
        self.ran_a_word = false;
        self.after_redirect = false;
        self.name_next = false;
    }
}

impl Reader<'_> {
    fn byte(&self) -> Option<u8> {
        (self.at < self.end).then(|| self.text[self.at])
    }

    fn byte_after(&self, n: usize) -> Option<u8> {
        (self.at + n < self.end).then(|| self.text[self.at + n])
    }

    fn at_continuation(&self) -> bool {
        self.byte() == Some(b'\\') && self.byte_after(1) == Some(b'\n')
    }

    /// Counts a byte read, or gives up on the text once it has cost too much.
    fn spend(&mut self) -> bool {
        if self.steps_left == 0 {
            self.give_up();
            return false;
        }
        self.steps_left -= 1;
        true
    }

    /// The byte being read, counted, in what has to close before the text ends. Past
    /// the text, or past what it may cost, that is too complex.
    fn open_byte(&mut self) -> Option<u8> {
        let byte = if self.spend() { self.byte() } else { None };
        self.reading.too_complex |= byte.is_none();
        byte
    }

    /// Counts bytes read in one go, which the next [`Self::spend`] answers for.
    fn charge(&mut self, bytes: usize) {
        self.steps_left = self.steps_left.saturating_sub(bytes);
    }

    fn give_up(&mut self) {
        self.reading.too_complex = true;
        self.at = self.end;
    }

    /// Steps over a `\` and what it escapes. In backquotes bash has made `\\` of it
    /// by now, which escapes what follows.
    fn escape(&mut self) {
        self.reading.too_complex |= self.in_backquotes > 0 && self.byte_after(1) == Some(b'\\');
        self.at = (self.at + 2).min(self.end);
    }

    /// Takes a `\⏎` where blanks are part of the word.
    fn take_continuation(&mut self) {
        self.reading.cuts.push(Cut {
            range: self.at..self.at + 2,
            leaves_a_blank: false,
        });
        self.at += 2;
    }

    fn take_continuations(&mut self) {
        while self.at_continuation() {
            self.take_continuation();
        }
    }

    fn nested(&mut self, read: impl FnOnce(&mut Self)) {
        if self.depth == MAX_NESTING {
            self.give_up();
            return;
        }
        self.depth += 1;
        read(self);
        self.depth -= 1;
    }

    /// Reads a list of commands, up to the `)` that closes it if it is `in_parens`.
    fn command(&mut self, in_parens: bool, paren: Paren) {
        let mut list = CommandList {
            word_start: true,
            command_position: true,
            ..CommandList::default()
        };
        while self.spend() {
            let Some(byte) = self.byte() else {
                list.end_word(self.text, self.at);
                self.reading.too_complex |= in_parens;
                return;
            };
            let at = self.at;
            if self.at_continuation() {
                if list.open_brackets > 0 || paren == Paren::Glued {
                    // Part of a subscript's one word, or what may be a glob's, blanks
                    // and all.
                    self.take_continuation();
                } else {
                    self.join_lines(&mut list);
                }
                continue;
            }
            if !matches!(byte, b' ' | b'\t') {
                list.blanks = None;
            }
            match byte {
                b'\\' => {
                    list.part(at);
                    self.escape();
                }
                b'#' if list.word_start || self.glued_to_an_operator() => {
                    self.comment(&list, paren);
                    list.word = None;
                }
                b'\'' => {
                    list.part(at);
                    self.single_quote(Around::Bare);
                }
                b'"' => {
                    list.part(at);
                    self.double_quote();
                }
                b'$' => {
                    list.part(at);
                    self.dollar(Around::Bare);
                }
                b'`' => {
                    list.part(at);
                    self.backquote();
                }
                b'(' | b')' | b'<' | b'>' if list.open_brackets > 0 => {
                    // Text in the subscript's one word, wherever that is one.
                    list.text(at);
                    self.at += 1;
                }
                b'(' if list.in_condition => {
                    list.text(at);
                    self.at += 1;
                }
                b')' if list.in_condition
                    && !list.word_is(self.text, at, |row| row.word == Reserved::CloseTest) =>
                {
                    list.text(at);
                    self.at += 1;
                }
                b'(' => self.open_paren(&mut list, paren),
                b')' => {
                    list.separator(self.text, at);
                    list.in_pattern = false;
                    self.at += 1;
                    self.reading.too_complex |= in_parens && list.doubtful_case;
                    if in_parens && list.open_cases == 0 {
                        // The line that named a heredoc ended before its body.
                        self.reading.too_complex |=
                            self.heredocs.iter().any(|doc| doc.depth >= self.depth);
                        return;
                    }
                }
                b' ' | b'\t' => {
                    // A subscript's blanks are part of its one word.
                    if list.open_brackets == 0 {
                        list.end_word(self.text, at);
                    }
                    list.word_start = true;
                    list.blanks.get_or_insert(at);
                    self.at += 1;
                }
                b'\n' => {
                    list.separator(self.text, at);
                    self.at += 1;
                    self.skip_heredoc_bodies();
                }
                // `>&` and `>|` are one operator, and what follows is its word.
                b'&' | b'|' if list.after_redirect && list.word.is_none() => {
                    self.at += 1;
                }
                // `&>` is a redirect, and the command goes on past it.
                b'&' if self.byte_after(1) == Some(b'>') => {
                    list.end_word(self.text, at);
                    self.at += 1;
                }
                b';' | b'&' | b'|' => {
                    list.separator(self.text, at);
                    // `;;`, `;&` and `;;&` end a case's arm: a pattern comes next.
                    list.in_pattern |= byte == b';'
                        && list.open_cases > 0
                        && matches!(self.byte_after(1), Some(b';' | b'&'));
                    self.at += 1;
                }
                b'<' | b'>' => {
                    list.end_word(self.text, at);
                    list.word_start = true;
                    list.after_redirect = true;
                    self.at += 1;
                    self.take_continuations();
                    if self.byte() == Some(b'(') {
                        // A process substitution is a word, or the start of one.
                        list.after_redirect = false;
                        list.part(self.at);
                        self.at += 1;
                        self.nested(|reader| reader.command(true, Paren::Commands));
                    } else if byte == b'<' {
                        list.after_redirect = !self.after_less_than();
                    }
                }
                b'[' => {
                    let names_a_variable = list.open_brackets == 0
                        && !list.ran_a_word
                        && list.word.is_some_and(|(begin, plain)| {
                            self.charge(at - begin);
                            plain && is_variable_name(&joined(&self.text[begin..at]))
                        });
                    let opens_a_key = paren == Paren::Assignment && list.word_start;
                    if names_a_variable || opens_a_key || list.open_brackets > 0 {
                        list.open_brackets += 1;
                    }
                    list.text(at);
                    self.at += 1;
                }
                b']' => {
                    list.open_brackets = list.open_brackets.saturating_sub(1);
                    list.text(at);
                    self.at += 1;
                }
                _ => {
                    list.text(at);
                    self.at += 1;
                }
            }
        }
    }

    /// A `#` that starts a word: a comment, wherever bash surely reads it as one.
    fn comment(&mut self, list: &CommandList, paren: Paren) {
        let start = self.at;
        let rest = &self.text[start..self.end];
        let mut len = rest.iter().position(|b| *b == b'\n').unwrap_or(rest.len());
        if list.in_condition
            || list.open_brackets > 0
            || paren == Paren::Glued
            || self.in_failed_arithmetic > 0
            || self.glued_to_an_operator()
        {
            // Text to bash in some of these, a comment in others. Reading on as
            // text hides nothing from the best-effort reading.
            self.reading.too_complex = true;
            self.charge(len);
            let doubtful = &mut self.reading.doubtful_comments;
            if doubtful.last().is_none_or(|last| last.end != start + len) {
                doubtful.push(start..start + len);
            }
            self.at += 1;
            return;
        }
        while self.in_backquotes > 0 && len < rest.len() && ends_in_a_lone_backslash(&rest[..len]) {
            let next = rest[len + 1..].iter().position(|b| *b == b'\n');
            len = next.map_or(rest.len(), |n| len + 1 + n);
        }
        self.at = start + len;
        self.charge(len);
        if self.in_double_quotes == 0 {
            self.reading.comments.push(start..self.at);
        } else {
            // Left out, it reads as what the flat state makes of the double quotes
            // around it, which pair off: these break that.
            let quoted = self.in_double_quotes % 2 == 1;
            self.reading.too_complex |= rest[..len]
                .iter()
                .any(|b| matches!(b, b'"' | b'\\' | b'`') || (*b == b'\'' && !quoted));
        }
    }

    /// Whether a `<`, a `>` or the `-` of `>&-` stands right before the byte being
    /// read. Bash starts a comment there, where zsh reads the likes of `<->#b` as a word.
    fn glued_to_an_operator(&self) -> bool {
        let before = &self.text[..self.at];
        match before.last() {
            Some(b'<' | b'>') => !ends_in_a_lone_backslash(&before[..before.len() - 1]),
            Some(b'-') => {
                (before.ends_with(b">&-") || before.ends_with(b"<&-"))
                    && !ends_in_a_lone_backslash(&before[..before.len() - 3])
            }
            _ => false,
        }
    }

    /// Takes the `\⏎`s here and the blanks around them, which become one blank.
    fn join_lines(&mut self, list: &mut CommandList) {
        let start = list.blanks.take().unwrap_or(self.at);
        let mut blank = start != self.at;
        while self.at_continuation() {
            self.at += 2;
            while matches!(self.byte(), Some(b' ' | b'\t')) {
                self.at += 1;
                blank = true;
            }
        }
        self.reading.cuts.push(Cut {
            range: start..self.at,
            leaves_a_blank: blank,
        });
        if blank {
            list.end_word(self.text, start);
            list.word_start = true;
        }
    }

    fn open_paren(&mut self, list: &mut CommandList, around: Paren) {
        let at = self.at;
        // `if((a))` is the reserved word, then a command.
        let after_word = !list.word_start
            && !list.word_is(self.text, at, |row| {
                opens_a_command(row) || row.word == Reserved::For
            });
        let after_equals = at > 0 && self.text[at - 1] == b'=';
        let paren = match (after_word, after_equals) {
            // All one word to bash, however deep, where it reads a glob. Past a command
            // word bash has no use for it, and zsh reads `a (b)#c` as a glob too.
            _ if around == Paren::Glued => Paren::Glued,
            (true, true) => Paren::Assignment,
            _ if list.ran_a_word => Paren::Glued,
            (true, false) => Paren::Glued,
            (false, _) => Paren::Commands,
        };
        list.end_word(self.text, at);
        let doubled = !after_word && self.byte_after(1) == Some(b'(');
        if doubled && self.arithmetic(Around::Bare) {
            // A command of its own: what follows its `))` starts a word, a reserved one.
            list.word_start = true;
            list.reserved_next = true;
            return;
        }
        self.at += 1;
        // `name()` and `name ( )` name a function, and `@()` is an empty glob.
        let inside = self.text[self.at..self.end]
            .iter()
            .find(|b| !matches!(b, b' ' | b'\t'));
        let empty = inside == Some(&b')')
            && !(at > 0 && matches!(self.text[at - 1], b'?' | b'*' | b'+' | b'@' | b'!'));
        self.in_failed_arithmetic += usize::from(doubled);
        self.nested(|reader| reader.command(true, paren));
        self.in_failed_arithmetic -= usize::from(doubled);
        if paren == Paren::Commands || empty && paren == Paren::Glued {
            // `(a)` is a whole `case` pattern, and an arm's commands follow.
            list.in_pattern = false;
            list.word = None;
            list.word_start = true;
            list.command_position = true;
            list.ran_a_word = false;
        } else {
            // An array literal, or what bash may read as a glob: the word goes on.
            list.part(at);
        }
    }

    /// At a `(`, or the `$` before one, with another after it: `((…))` if it closes
    /// so, with nothing read otherwise. Bash decides the same way, by reading on:
    /// `((a) | b)` is a subshell in a subshell.
    fn arithmetic(&mut self, around: Around) -> bool {
        let start = self.at;
        let cuts = self.reading.cuts.len();
        let comments = self.reading.comments.len();
        let doubtful_comments = self.reading.doubtful_comments.len();
        let too_complex = self.reading.too_complex;
        // A substitution in the body may read past their line and take them, so they
        // are copied, which the budget answers for.
        let heredocs = self.heredocs.clone();
        self.charge(heredocs.iter().map(|doc| doc.delimiter.len() + 1).sum());
        self.at += 2;
        let mut closes = true;
        self.nested(|reader| closes = reader.arithmetic_body(around));
        if closes || self.reading.too_complex && self.at == self.end {
            return true;
        }
        self.at = start;
        self.reading.cuts.truncate(cuts);
        self.reading.comments.truncate(comments);
        self.reading.doubtful_comments.truncate(doubtful_comments);
        self.reading.too_complex = too_complex;
        self.heredocs = heredocs;
        false
    }

    fn arithmetic_body(&mut self, around: Around) -> bool {
        let mut open = 0;
        while self.spend() {
            let Some(byte) = self.byte() else {
                return false;
            };
            match byte {
                b'(' => {
                    open += 1;
                    self.at += 1;
                }
                b')' if open > 0 => {
                    open -= 1;
                    self.at += 1;
                }
                b')' => {
                    let closes = self.byte_after(1) == Some(b')');
                    self.at += 2;
                    return closes;
                }
                _ => self.expansion_byte(byte, around),
            }
        }
        false
    }

    /// A byte inside `${ }` or an arithmetic expansion: no comment or word here.
    fn expansion_byte(&mut self, byte: u8, around: Around) {
        match byte {
            b'\\' if self.at_continuation() => self.take_continuation(),
            b'\\' => self.escape(),
            b'\'' => self.single_quote(around),
            b'"' => self.double_quote(),
            b'$' => self.dollar(around),
            b'`' => self.backquote(),
            _ => self.at += 1,
        }
    }

    /// At a `'`. A closing quote that is missing, or a quote holding anything but
    /// plain text in an expansion double quotes hold, is too complex.
    fn single_quote(&mut self, around: Around) {
        let body = self.at + 1;
        let rest = &self.text[body.min(self.end)..self.end];
        let Some(len) = rest.iter().position(|b| *b == b'\'') else {
            self.give_up();
            return;
        };
        self.charge(len);
        let holds_a_continuation = rest[..len].windows(2).any(|pair| pair == b"\\\n");
        self.reading.too_complex |= self.in_backquotes > 0 && holds_a_continuation;
        if let Around::DoubleExpansion { closer } = around {
            let body = &rest[..len];
            let closes = |b: &u8| matches!(b, b'"' | b'`' | b'\\') || *b == closer;
            let opens = |pair: &[u8]| pair[0] == b'$' && matches!(pair[1], b'(' | b'{' | b'[');
            self.reading.too_complex |= body.iter().any(closes) || body.windows(2).any(opens);
        }
        self.at = body + len + 1;
    }

    /// At the `'` of `$'`, where only a `\` keeps a `'` from closing it.
    fn ansi_c_quote(&mut self) {
        self.at += 1;
        while let Some(byte) = self.open_byte() {
            match byte {
                b'\\' => {
                    self.reading.too_complex |= self.in_backquotes > 0 && self.at_continuation();
                    self.escape();
                }
                b'\'' => {
                    self.at += 1;
                    return;
                }
                _ => self.at += 1,
            }
        }
    }

    /// At a `"`.
    fn double_quote(&mut self) {
        self.at += 1;
        self.in_double_quotes += 1;
        self.nested(|reader| {
            while let Some(byte) = reader.open_byte() {
                match byte {
                    b'"' => {
                        reader.at += 1;
                        return;
                    }
                    b'\\' if reader.at_continuation() => reader.take_continuation(),
                    b'\\' => reader.escape(),
                    b'$' => reader.dollar(Around::Double),
                    b'`' => reader.backquote(),
                    _ => reader.at += 1,
                }
            }
        });
        self.in_double_quotes -= 1;
    }

    /// At a `$`, and what it opens. Bash joins a continuation first, so `$\⏎(` opens
    /// a substitution.
    fn dollar(&mut self, around: Around) {
        self.at += 1;
        self.take_continuations();
        let within = |closer| match around {
            Around::Bare => Around::Bare,
            Around::Double | Around::DoubleExpansion { .. } => Around::DoubleExpansion { closer },
        };
        match self.byte() {
            Some(b'(') => {
                let doubled = self.byte_after(1) == Some(b'(');
                if doubled && self.arithmetic(within(b')')) {
                    return;
                }
                self.at += 1;
                self.in_failed_arithmetic += usize::from(doubled);
                self.nested(|reader| reader.command(true, Paren::Commands));
                self.in_failed_arithmetic -= usize::from(doubled);
            }
            Some(b'{') => {
                self.at += 1;
                self.nested(|reader| reader.parameter(within(b'}')));
            }
            Some(b'[') => {
                self.at += 1;
                self.nested(|reader| reader.old_arithmetic(within(b']')));
            }
            Some(b'\'') if around == Around::Bare => self.ansi_c_quote(),
            Some(b'\'') if matches!(around, Around::DoubleExpansion { .. }) => {
                self.single_quote(around);
            }
            Some(b'"') if around != Around::Double => self.double_quote(),
            // A variable: what follows it opens nothing it would after a `$`.
            Some(b'$') => self.at += 1,
            _ => {}
        }
    }

    /// Past the `${`. The first `}` closes it: a `{` in it does not nest.
    fn parameter(&mut self, around: Around) {
        // Commands since bash 5.3, and an error before it.
        self.reading.too_complex |= matches!(self.byte(), Some(b' ' | b'\t' | b'\n' | b'|'));
        while let Some(byte) = self.open_byte() {
            match byte {
                b'}' => {
                    self.at += 1;
                    return;
                }
                // Bash reads this to its `)`, even where double quotes make it text.
                b'<' | b'>' if self.byte_after(1) == Some(b'(') => {
                    self.at += 2;
                    self.nested(|reader| reader.command(true, Paren::Commands));
                }
                _ => self.expansion_byte(byte, around),
            }
        }
    }

    /// Past the `$[`.
    fn old_arithmetic(&mut self, around: Around) {
        let mut open = 0;
        while let Some(byte) = self.open_byte() {
            match byte {
                b'[' => {
                    open += 1;
                    self.at += 1;
                }
                b']' if open == 0 => {
                    self.at += 1;
                    return;
                }
                b']' => {
                    open -= 1;
                    self.at += 1;
                }
                _ => self.expansion_byte(byte, around),
            }
        }
    }

    /// At a backquote. Bash takes the text to the next one first, whatever quotes
    /// stand in it, and reads that as commands. An escaped backquote nests one more,
    /// and a `\"` inside double quotes is a quote there, neither followed here.
    fn backquote(&mut self) {
        let body = self.at + 1;
        let mut close = body;
        let mut deeper = false;
        loop {
            match self.text[..self.end].get(close) {
                None => {
                    self.give_up();
                    return;
                }
                Some(b'`') => break,
                Some(b'\\') => {
                    let escaped = self.text[..self.end].get(close + 1);
                    deeper |= escaped == Some(&b'`')
                        || (escaped == Some(&b'"') && self.in_double_quotes > 0);
                    close += 2;
                }
                Some(_) => close += 1,
            }
        }
        self.charge(close - body);
        // What the next level holds matters only if it can open or join anything.
        let body_text = &self.text[body..close.min(self.end)];
        if deeper && body_text.iter().any(|b| matches!(b, b'#' | b'\n' | b'\'')) {
            self.reading.too_complex = true;
        }
        let end = self.end;
        self.at = body;
        self.end = close;
        self.in_backquotes += 1;
        self.nested(|reader| reader.command(false, Paren::Commands));
        self.in_backquotes -= 1;
        self.end = end;
        self.at = close + 1;
    }

    /// Past a `<`: a heredoc says where its body is, and a herestring does nothing.
    /// Whether it took the word the redirect takes.
    fn after_less_than(&mut self) -> bool {
        if self.byte() != Some(b'<') {
            return false;
        }
        self.at += 1;
        self.take_continuations();
        if self.byte() == Some(b'<') {
            self.at += 1;
            return false;
        }
        let strips_tabs = self.byte() == Some(b'-');
        if strips_tabs {
            self.at += 1;
        }
        self.take_continuations();
        while matches!(self.byte(), Some(b' ' | b'\t')) {
            self.at += 1;
        }
        let word = self.heredoc_delimiter();
        match word {
            Some((delimiter, quoted)) => self.heredocs.push(Heredoc {
                delimiter,
                strips_tabs,
                quoted: quoted && self.in_backquotes == 0,
                depth: self.depth,
            }),
            None => self.reading.too_complex = true,
        }
        true
    }

    /// The word after `<<`, its quotes taken off, and whether it had any.
    fn heredoc_delimiter(&mut self) -> Option<(Vec<u8>, bool)> {
        let mut word = Vec::new();
        let mut quoted = false;
        while let Some(byte) = self.byte() {
            match byte {
                b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b'<' | b'>' | b'(' | b')' => break,
                b'\\' if self.at_continuation() => return None,
                // Blanks and brackets inside it are part of it, as this reader can't tell.
                b'`' => return None,
                b'$' if matches!(self.byte_after(1), Some(b'(' | b'{')) => return None,
                b'\\' => {
                    quoted = true;
                    word.extend(self.byte_after(1));
                    self.escape();
                }
                // Bash decodes `$'a\tb'` before it compares a line with it.
                b'$' if self.byte_after(1) == Some(b'\'') => {
                    let rest = &self.text[(self.at + 2).min(self.end)..self.end];
                    let len = rest.iter().position(|b| *b == b'\'')?;
                    if rest[..len].contains(&b'\\') {
                        return None;
                    }
                    self.at += 1;
                }
                b'$' if self.byte_after(1) == Some(b'"') => self.at += 1,
                b'\'' | b'"' => {
                    quoted = true;
                    let body = self.at + 1;
                    let rest = &self.text[body.min(self.end)..self.end];
                    let len = rest.iter().position(|b| *b == byte)?;
                    word.extend_from_slice(&rest[..len]);
                    self.at = body + len + 1;
                }
                _ => {
                    word.push(byte);
                    self.at += 1;
                }
            }
        }
        (!word.is_empty()).then_some((word, quoted))
    }

    /// Past the `\n` of a line that named heredocs: steps over their bodies.
    fn skip_heredoc_bodies(&mut self) {
        for heredoc in std::mem::take(&mut self.heredocs) {
            if heredoc.depth != self.depth {
                self.reading.too_complex = true;
            }
            // What the lines so far make, where bash joins the lines of a body it
            // expands: a delimiter is compared with that.
            let mut joined = Vec::new();
            while self.at < self.end {
                let rest = &self.text[self.at..self.end];
                let len = rest.iter().position(|b| *b == b'\n').unwrap_or(rest.len());
                let line = &rest[..len];
                self.at = (self.at + len + 1).min(self.end);
                self.charge(len);
                if !heredoc.quoted && ends_in_a_lone_backslash(line) {
                    joined.extend_from_slice(&line[..len - 1]);
                    continue;
                }
                joined.extend_from_slice(line);
                let tabs = if heredoc.strips_tabs {
                    joined.iter().take_while(|b| **b == b'\t').count()
                } else {
                    0
                };
                if joined[tabs..] == heredoc.delimiter {
                    break;
                }
                // Inside a substitution its `)` ends the body too, on this very line.
                self.reading.too_complex |= heredoc.depth > 0
                    && joined[tabs..].starts_with(&heredoc.delimiter)
                    && joined[tabs + heredoc.delimiter.len()..].first() == Some(&b')');
                joined.clear();
            }
        }
    }
}

/// Whether a plain word can only be what a command runs or takes: not a keyword, an
/// assignment, an fd before a redirect (`2>f`, `{fd}>f`), or an option of `time`.
fn may_only_run(word: &[u8]) -> bool {
    !word.contains(&b'=')
        && !(word.first() == Some(&b'{') && word.last() == Some(&b'}'))
        && !word.first().is_some_and(|b| *b == b'-')
        && !word.iter().all(u8::is_ascii_digit)
        && !reserved_in(word)
            .is_some_and(|row| row.word == Reserved::OpenTest || opens_a_command(row))
}

/// A plain word as bash sees it, past any `\⏎` in it.
fn joined(word: &[u8]) -> Cow<'_, [u8]> {
    if !word.windows(2).any(|pair| pair == b"\\\n") {
        return Cow::Borrowed(word);
    }
    let mut out = Vec::with_capacity(word.len());
    let mut rest = word;
    while let Some(at) = rest.windows(2).position(|pair| pair == b"\\\n") {
        out.extend_from_slice(&rest[..at]);
        rest = &rest[at + 2..];
    }
    out.extend_from_slice(rest);
    Cow::Owned(out)
}

/// Whether a `\` right after `text` escapes something: an odd run ends `text`.
pub(super) fn ends_in_a_lone_backslash(text: &[u8]) -> bool {
    text.iter().rev().take_while(|b| **b == b'\\').count() % 2 == 1
}

fn is_variable_name(word: &[u8]) -> bool {
    std::str::from_utf8(word).is_ok_and(|word| name_len(word) == word.len())
}

#[cfg(test)]
mod tests {
    use super::super::lexer::apply_cuts;
    use super::*;

    fn comments(text: &str) -> Vec<&str> {
        read_source(text)
            .comments
            .into_iter()
            .map(|comment| &text[comment])
            .collect()
    }

    #[test]
    fn test_each_nesting_reads_a_hash_as_bash_does() {
        // Each row as bash 5.2 runs it: the comments it skips.
        for (input, found) in [
            // Inside an expansion a `#` is text, and the comment after it is one.
            ("echo ${x:-{a} # c}", vec!["# c}"]),
            ("echo ${x:-a # b} #c", vec!["#c"]),
            ("echo ${#x} ${x#y} ${x:-\\}} #c", vec!["#c"]),
            ("echo $# #c", vec!["#c"]),
            ("echo $((16#ff)) #c", vec!["#c"]),
            ("echo $[1 #2] #c", vec!["#c"]),
            ("echo $'a\\'#b' #c", vec!["#c"]),
            ("echo \"#a\" '#b' \\#c #d", vec!["#d"]),
            // In commands it is a comment, and a `)` in it closes nothing.
            ("echo $(echo a # )\n) #c", vec!["# )", "#c"]),
            (
                "echo $(echo $(echo a # )\n) # )\n) #c",
                vec!["# )", "# )", "#c"],
            ),
            ("(echo a # )\n) #c", vec!["# )", "#c"]),
            ("echo <(echo a # )\n) #c", vec!["# )", "#c"]),
            ("echo <(echo a)#b; echo >(echo c)#d #e", vec!["#e"]),
            ("echo $(#a\necho b) #c", vec!["#a", "#c"]),
            ("echo ${x:-$(echo a # }\n)} #c", vec!["# }", "#c"]),
            ("a=(1 # )\n2) #c", vec!["# )", "#c"]),
            ("a=(1)#b #c", vec!["#c"]),
            ("f()#b", vec!["#b"]),
            ("echo ${x:-<(echo a # })\n)} #c", vec!["# })", "#c"]),
            ("echo ${x:->(echo })}#a #c", vec!["#c"]),
            ("f() { echo a # }\n} #c", vec!["# }", "#c"]),
            // A backquote ends one, and quotes do not hide its closer.
            ("echo `echo a # b` #c", vec!["# b", "#c"]),
            ("echo `echo a #b\\\nc` #d", vec!["#b\\\nc", "#d"]),
            ("echo `echo a #b\nc` #d", vec!["#b", "#d"]),
            ("echo `echo a #b\\\\\nc` #d", vec!["#b\\\\", "#d"]),
            ("echo $(echo a #b\\\nc\n) #d", vec!["#b\\", "#d"]),
            ("echo `cat <<'E'\nb\\\nE\n#x\nE\n` #d", vec!["#d"]),
            // `(( ))` is arithmetic if it closes so, and commands if not.
            ("((1 # 2\n)) #c", vec!["#c"]),
            ("((1))#c 'd", vec!["#c 'd"]),
            ("echo $((1))#c", vec![]),
            ("echo $$#a $${b #c", vec!["#c"]),
            ("echo $$[#a #c", vec!["#c"]),
            ("((echo a) | b) #c", vec!["#c"]),
            ("echo $((echo a) | b) #c", vec!["#c"]),
            ("echo $(( (1) # 2\n)) #c", vec!["#c"]),
            // A `case` pattern's `)` closes nothing, wherever it stands.
            (
                "echo $(case a in a) echo b # )\n;; esac) #c",
                vec!["# )", "#c"],
            ),
            ("(case a in (a) echo b # )\n;; esac) #c", vec!["# )", "#c"]),
            (
                "if a; then case b in c) d;; esac; fi; (echo case) #c",
                vec!["#c"],
            ),
            // A heredoc's body is its own, and what follows it is read afresh.
            ("cat <<E #a\nit's #b\nE\necho c #d", vec!["#a", "#d"]),
            ("cat <<-'E'\n\t#b\n\tE\necho c #d", vec!["#d"]),
            ("cat <<\"E\"\n#b\nE\n#d", vec!["#d"]),
            ("cat <<\\E\n#b\nE\n#d", vec!["#d"]),
            ("cat <<E; cat <<F\n#a\nE\n#b\nF\n#d", vec!["#d"]),
            ("cat <<E\na\\\nE\nE\n#d", vec!["#d"]),
            ("cat <<E\na\n\\\nE\n#d", vec!["#d"]),
            ("cat <<'E'\na\\\nE\n#d", vec!["#d"]),
            ("cat <<$'E'\n#a\nE\n#d", vec!["#d"]),
            ("cat <<$\"E\"\n#a\nE\n#d", vec!["#d"]),
            ("cat <<a'E'b\n#a\naEb\n#d", vec!["#d"]),
            ("cat <<< a #b", vec!["#b"]),
            (": <<E\nE\nls x[ #c", vec!["#c"]),
            ("a[1<<2]=3 #b\n#c", vec!["#b", "#c"]),
            ("ca\\\nse a in (a) b;; esac #c", vec!["#c"]),
            ("cat <<\\\n< a #b", vec!["#b"]),
            ("cat <<\\\nE\n#a\nE\n#d", vec!["#d"]),
            // At any depth: the shape of a commit message with an issue number.
            (
                "git commit -m \"$(cat <<'E'\nFix #1, it's\nE\n)\" #c",
                vec!["#c"],
            ),
            ("(cat <<E\na #b )\nE\n) #c", vec!["#c"]),
            ("echo $(cat <<E\n#a\nE\n) #c", vec!["#c"]),
            // In double quotes, one that holds nothing they react to is their text.
            ("echo \"$(echo a # b\n)\" #c", vec!["#c"]),
            ("echo \"$(echo a # it's\n)\" #c", vec!["#c"]),
            // A backquote nested in one is plain text if nothing in it can matter.
            ("echo `echo \\`echo a\\`` #c", vec!["#c"]),
            // Glued to a word it is no comment; a separator starts one.
            ("echo a#b \"c\"#d $(e)#f `g`#h {i,j}#k", vec![]),
            ("echo a;#b\necho c|#d\necho e &&#f", vec!["#b", "#d", "#f"]),
            ("echo a\\&#b", vec![]),
            ("echo a&#b", vec!["#b"]),
            ("echo a\\&&#b", vec!["#b"]),
            ("echo a&&#b", vec!["#b"]),
            ("echo a\r #b", vec!["#b"]),
            ("echo a\r#b", vec![]),
            // Nothing makes `[[` and `[` special off a command's start.
            ("echo [ #c", vec!["#c"]),
            ("echo a[1 # 2]", vec!["# 2]"]),
            ("ls x[\n# note\necho b", vec!["# note"]),
            ("ls [[ # ]]", vec!["# ]]"]),
            ("echo [[; b #c", vec!["#c"]),
            ("echo \"$(echo a # it's\n)\" #c", vec!["#c"]),
            ("echo $([[ a ]]) #c", vec!["#c"]),
            ("function f { [[ a =~ (#b) ]]; c; } #d", vec!["#d"]),
            ("if ([[ -f a ]]); then b; fi #c", vec!["#c"]),
            ("echo a\\>#b a\\<#c #d", vec!["#d"]),
            ("echo \"${x#'a]'}\" \"$[1'}']\" #c", vec!["#c"]),
            ("echo a[1] #c", vec!["#c"]),
            ("git log --grep do x[1 # c", vec!["# c"]),
            ("echo $(echo do case x)#b; c\nesac)", vec![]),
            ("(echo do case) #c", vec!["#c"]),
            ("git status &>/dev/null x[ # c", vec!["# c"]),
            ("echo $(ls /dev/null &> case) #c", vec!["#c"]),
            ("echo $(case x in a) :;; case) :;; esac) #c", vec!["#c"]),
            ("echo $(case x in a|case) :;; esac) #c", vec!["#c"]),
            (
                "echo $(case a in b) c;; esac; case d in e) f;; esac) #g",
                vec!["#g"],
            ),
            ("declare -a x=(a # b\n c) #d", vec!["# b", "#d"]),
            ("f() ( ls # c\n) #d", vec!["# c", "#d"]),
            ("function f() ( ls # c\n) #d", vec!["# c", "#d"]),
            ("ls; f() (\n # c\n ls\n) #d", vec!["# c", "#d"]),
            ("if((1<<2));then a;fi #c", vec!["#c"]),
            ("echo then a[k # 2]=b; c", vec!["# 2]=b; c"]),
            ("echo do a[k # 2]=b; c", vec!["# 2]=b; c"]),
            ("echo for x do a[k # 2]=b; c", vec!["# 2]=b; c"]),
            ("echo fi then a[k # 2]=b; c", vec!["# 2]=b; c"]),
            ("echo } then a[k # 2]=b; c", vec!["# 2]=b; c"]),
            ("echo ]] then a[k # 2]=b; c", vec!["# 2]=b; c"]),
            ("case y in esac #c", vec!["#c"]),
            ("echo $(case y in esac) #c", vec!["#c"]),
            (
                "echo $(if [[ a ]] then case y in y) b;; esac; fi) #c",
                vec!["#c"],
            ),
            ("a[x y]=2 #c", vec!["#c"]),
            (
                "echo $(if { a; } then case y in y) b;; esac; fi) #c",
                vec!["#c"],
            ),
            (
                "echo $(if { a; } >f then case y in y) b;; esac; fi) #c",
                vec!["#c"],
            ),
            ("echo $(case y in y) if a; then b; fi esac) #c", vec!["#c"]),
            (
                "echo $(for ((i=0;i<1;i++)) do case y in y) b;; esac; done) #c",
                vec!["#c"],
            ),
            (
                "echo $(if (( 1 )) then case y in y) b;; esac; fi) #c",
                vec!["#c"],
            ),
            ("echo $(time case y in y) b;; esac) #c", vec!["#c"]),
            ("echo $(echo esac) #c", vec!["#c"]),
            ("echo $(echo case) #c", vec!["#c"]),
            (
                "echo $(case y in y) echo esac;; x) b;; esac) #c",
                vec!["#c"],
            ),
            ("echo $(case a in a) >esac;; b) c;; esac) #c", vec!["#c"]),
            ("echo $(function f case a in a) b;; esac) #c", vec!["#c"]),
            ("echo $(f ( ) case a in a) b;; esac) #c", vec!["#c"]),
            ("echo $(coproc c case a in a) b;; esac) #c", vec!["#c"]),
            (
                "echo \"$(coproc c case a in a) echo \"x #y\";; esac)\" #c",
                vec!["#c"],
            ),
            (
                "echo \"$(f ( ) case a in a) echo \"x #y\";; esac)\" #c",
                vec!["#c"],
            ),
            ("echo $(x=1 esac) #c", vec!["#c"]),
            ("case y in for) echo x[ #c\n;; esac", vec!["#c"]),
            ("ls>f x[ #c", vec!["#c"]),
            (
                "echo $(case a in (a) case b in (b) c;; esac;; (q) d;; esac) #c",
                vec!["#c"],
            ),
            (
                "echo $(case a in (a) case b in b) c;; esac;; q) d;; esac) #c",
                vec!["#c"],
            ),
            (
                "echo $(case a in (a) case b in b) c;; esac;; q) d;; esac # c\n) #d",
                vec!["# c", "#d"],
            ),
            ("case x in a) for f in foo[ # c", vec!["# c"]),
            ("case a in (a)#b's\n c;; esac #d", vec!["#b's", "#d"]),
            ("while((1<<2));do a;done #c", vec!["#c"]),
            ("for((i=1<<2;i<3;i++));do a #b\ndone #c", vec!["#b", "#c"]),
            ("echo @()#b; c", vec![]),
            ("echo (a)#b; c", vec![]),
            ("ls () { a; } #c", vec!["#c"]),
            ("echo \"${x:-'$y'}\" #c", vec!["#c"]),
            ("(function f { case a in (b) c;; esac; }) #d", vec!["#d"]),
            ("echo $a[1] #c", vec!["#c"]),
            ("x=a[1] #c", vec!["#c"]),
        ] {
            let reading = read_source(input);
            assert!(!reading.too_complex, "{input:?}");
            assert_eq!(comments(input), found, "{input:?}");
        }
    }

    #[test]
    fn test_what_bash_reads_two_ways_is_too_complex() {
        for input in [
            // Text to bash inside a condition or a subscript, a comment outside.
            "[[ a == # b\n ]]",
            "[[ a =~ ( #b) ]]",
            "a[1 # 2]=b",
            "x=1 a[1 # 2]=b",
            "a+=1 b[1 # 2]=c",
            "a[0]=1 b[1 # 2]=c",
            ">f a[1 # 2]=b",
            "2>/dev/null a[1 # 2]=b",
            "{fd}>/dev/null a[1 # 2]=b",
            ">&2 a[1 # 2]=b",
            "if c; then a[1 # 2]=b; fi",
            "time -p a[1 # 2]=b",
            "function f { a[1 # 2]=b; }",
            "coproc c { a[1 # 2]=b; }",
            ">&f a[1 # 2]=b",
            ">|f a[1 # 2]=b",
            "\"a\" b[1 # 2]=c",
            ": <<E | a[x # 2]=b\nE",
            "echo \"$(echo \"$(echo a # it's\n)\")\"",
            "a=([1 # 2]=b)",
            "a\\\n[1 # 2]=b",
            "$(a[x) # y]=1); b",
            "(a[x) # y]=1); b",
            "a=([x) # y]=1); b",
            "[\\\n[ a == # b\n ]]",
            "echo @((a # b) ); c\n))",
            // A glob with extglob on, and a syntax error with it off.
            "echo @(a # b\n)",
            // Bash starts a comment here, and zsh a word.
            "echo a>#b",
            "echo a<#b",
            "echo a>&-#b",
            "echo a<&-#b",
            // Quoting bash reads differently by the expansion's operator.
            "echo \"${x:-'}'}\" #c",
            "echo \"$['1]']\" #c",
            "echo \"${x:-'$(y)'}\" #c",
            "echo \"${x:-'${y'}\" #c",
            "echo `echo \\\\ # a; b`",
            "(echo { case) #c",
            "echo ${ a; b; } #c",
            "echo ${| a; b; } #c",
            "echo \"${ a; b; }\" #c",
            "echo \"${x:-'a\\\nb'}\"",
            "echo \"$['1\\\n+1']\"",
            "echo \"${x:-$'a\\'b'}\" #c",
            // The flat lexer has no state to restart in.
            "echo \"$(echo a # \"\n)\"",
            "echo \"`echo a # \\`\"",
            // A level the reader does not follow.
            "echo `echo \\`echo # a\\`` #c",
            "echo `echo 'a\\\nb'` #c",
            "((echo a) # )\n) #c",
            "echo $((echo a) # )\n) #c",
            "echo `echo $'a\\\nb'` #c",
            "echo \"`echo \\\"# a\\\"`\" #c",
            // A heredoc whose line ends elsewhere than where it was named.
            "(cat <<E) #c",
            "cat <<$(a)\n$(a)\nls # c",
            "cat <<${a}\n${a}\nls # c",
            "cat <<`a`\n`a`\nls # c",
            "for x do a[k # 2]=b; c; done",
            "for ((;;)) do a[k # 2]=b; c; done",
            "select x do a[k # 2]=b; c; done",
            "if [[ a ]] then a[k # 2]=b; c; fi",
            "while [[ a ]] do a[k # 2]=b; c; done",
            "if { a; } then a[k # 2]=b; c; fi",
            "if if a; then b; fi then a[k # 2]=b; c; fi",
            "if a; then { b; } else a[k # 2]=c; d; fi",
            "if (( 1 )) then a[k # 2]=b; c; fi",
            "if a; then b; fi >f; a[k # 2]=c",
            "a[x y]=2 b[k # 2]=c; d",
            "a[x\ty]=2 b[k # 2]=c; d",
            "a[(x y)]=2 b[k # 2]=c; d",
            "a[x \\\n y]=2 b[k # 2]=c; d",
            "2>f x[1 # 2]=b; c",
            "{fd}>f x[1 # 2]=b; c",
            "echo $(case a in a) x=1 then esac;; b) c;; esac) #c",
            "echo $(time -p case a in a) b;; esac) #c",
            "echo $(time -- case a in a) b;; esac) #c",
            "echo $(case a in (a) case b in b) c;; esac;; q) d;; esac # c",
            "cat <<$'a\\tb'\nb\na\tb\n#c",
            "cat <<E $(echo a\n)\nb\nE\n#c",
            "cat <<E $(echo a\nE\n)\n#c",
            "echo $(cat <<E\nb\nE) #c's\nd\nE\n)",
            "cat <<E; (( $(a\nE\n) ) ) #c",
            // What never closes.
            "echo \"a #b",
            "echo 'a #b",
            "echo $'a #b",
            "echo ${a #b",
            "echo $(a #b",
            "echo $[a #b",
            "echo `a #b",
            "(a #b",
        ] {
            assert!(read_source(input).too_complex, "{input:?}");
        }
    }

    #[test]
    fn test_what_costs_too_much_is_too_complex() {
        let levels = |n: usize, open: &str, close: &str| {
            format!("{}a{} #c", open.repeat(n), close.repeat(n))
        };
        // Deep, but within reach.
        let fine = levels(16, "( ", ")");
        assert!(!read_source(&fine).too_complex);
        assert_eq!(comments(&fine), ["#c"]);
        for nested in [
            levels(17, "( ", ")"),
            levels(17, "$(", ")"),
            levels(17, "${x:-", "}"),
            levels(17, "$[", "]"),
            levels(10_000, "$((", ")"),
        ] {
            let reading = read_source(&nested);
            assert!(reading.too_complex, "{}", &nested[..20]);
            assert!(reading.comments.is_empty(), "{}", &nested[..20]);
        }
        // `$((` is read twice when it is not arithmetic, each level over, and a long
        // quote costs what it holds. These close, so only the cost makes them so.
        let reread = |body: String| {
            let nested = (0..6).fold(body, |inner, _| format!("$(( {inner} ) )"));
            format!("echo {nested} #c")
        };
        let plain = "x".repeat(100_000);
        assert!(!read_source(&reread("x".to_string())).too_complex);
        assert!(read_source(&reread(plain.clone())).too_complex);
        assert!(read_source(&reread(format!("'{plain}'"))).too_complex);
        assert!(read_source(&format!("a{} #c", "-[".repeat(100_000))).too_complex);
        let doubtful = read_source(&format!("a[{}", " #".repeat(100_000)));
        assert!(doubtful.too_complex);
        assert!(
            doubtful.doubtful_comments.len() <= 2,
            "one range for a line"
        );
    }

    #[test]
    fn test_only_a_continuation_bash_joins_is_cut() {
        let joined = |text: &str| apply_cuts(text, &read_source(text).cuts).into_owned();
        for (input, output) in [
            // Words, and what separates them.
            ("a\\\nb", "ab"),
            ("a \\\n b", "a b"),
            ("a \\\n \\\n b", "a b"),
            ("a\\ \\\nb", "a\\ b"),
            ("$(a \\\n b)", "$(a b)"),
            // Inside quotes and expansions the blanks belong to the word.
            ("\"a \\\n b\"", "\"a  b\""),
            ("${x:-a \\\n b}", "${x:-a  b}"),
            ("$[1 + \\\n 1]", "$[1 +  1]"),
            ("$\\\n(a)", "$(a)"),
            ("\"$\\\n(a)\"", "\"$(a)\""),
            ("\"$(a \\\n b)\"", "\"$(a b)\""),
            ("<\\\n<E\nE", "<<E\nE"),
            ("`a \\\n b`", "`a b`"),
            // Nothing joins in a single quote, an ANSI-C one or a comment.
            ("'a\\\nb'", "'a\\\nb'"),
            ("$'a\\\nb'", "$'a\\\nb'"),
            ("\"$(echo 'a\\\nb')\"", "\"$(echo 'a\\\nb')\""),
            ("x[ \\\n 1 ]=y", "x[  1 ]=y"),
            ("a @(b \\\n c)", "a @(b  c)"),
            ("#a\\\nb", "#a\\\nb"),
            ("cat <<E\na\\\nE\nE", "cat <<E\na\\\nE\nE"),
            // Nor where the newline is what the `\\` does not escape.
            ("a\\\\\nb", "a\\\\\nb"),
            ("a\\\r\nb", "a\\\r\nb"),
        ] {
            assert_eq!(joined(input), output, "{input:?}");
        }
    }

    #[test]
    fn test_a_read_that_turns_out_not_to_be_arithmetic_leaves_nothing_behind() {
        let reading = read_source("((a \\\n b) | $(c # d\n)) #e");
        assert_eq!(reading.comments.len(), 1);
        assert_eq!(reading.cuts.len(), 1);
        assert!(reading.too_complex);
    }
}
