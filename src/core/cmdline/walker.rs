//! Peels one transparent layer off the front of a command: a run of
//! assignments, a reserved word bash runs a command after, a keyword, a
//! wrapper, or a user-configured prefix. It reads the words and tokens a
//! caller has already lexed.
//!
//! The walker names no tool and holds no family table: every function that
//! needs one takes `&[Family]` from its caller (`discover::families`). It
//! reads bash's own grammar from [`super::bash_grammar`], which is shell
//! grammar and no tool's data. It owns no text either: a peel is a word index
//! into the [`Line`] it read, so the caller that lexed the line once can
//! express everything it decides as spans of that one lex.
//!
//! A peel depends on where the command sits ([`Position`]): bash reads
//! `NAME=value` as an assignment only where a simple command starts, and a
//! reserved word such as `time` only where a pipeline starts, so a caller
//! carries the position the previous layer left behind.

use super::bash_grammar::{Reserved, ReservedWord, reserved_word};
use super::family::{Family, Form, WrapperSpec};
use super::lexer::{
    self, Token, TokenKind as LexKind, Word, assignment_value, resolve_word_text,
    word_has_expansion,
};
use crate::core::arg_tokenizer::{TokenKind as ArgKind, tokenize_grammar};
use std::borrow::Cow;

/// One command as lexed once: the text the lex indexes, and the words and
/// tokens of the command, all offsets into `text`.
#[derive(Clone, Copy)]
pub(crate) struct Line<'a> {
    pub(crate) text: &'a str,
    pub(crate) words: &'a [Word<'a>],
    pub(crate) tokens: &'a [Token<'a>],
}

/// What bash's grammar allows at the place a walk has reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Position {
    /// A simple command starts here, so `NAME=value` is an assignment. Every
    /// stage of a pipeline starts its own (`cmd1 | FOO=1 cmd2` assigns `FOO`
    /// for `cmd2`).
    pub(crate) simple_command: bool,
    /// A pipeline starts here, so `time` is the reserved word. In a later
    /// stage of a pipeline the same word names a program.
    pub(crate) pipeline: bool,
}

impl Position {
    /// The front of a command, or of a pipeline's first stage.
    pub(crate) const START: Position = Position {
        simple_command: true,
        pipeline: true,
    };
    /// A later stage of a pipeline.
    pub(crate) const STAGE: Position = Position {
        simple_command: true,
        pipeline: false,
    };
    /// Behind a layer that runs its argument as a program: nothing after it
    /// starts a command of bash's own grammar.
    pub(crate) const INNER: Position = Position {
        simple_command: false,
        pipeline: false,
    };
}

/// One layer found: the label the caller gave it, the index of the first word
/// after it (`words.len()` when nothing follows), and the position the walk
/// is at from there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Peel<K> {
    pub(crate) kind: K,
    pub(crate) end: usize,
    pub(crate) next: Position,
}

/// The reserved words after which bash runs a pipeline, and which the walk
/// steps over where a pipeline starts. Each one's options follow it as
/// [`ReservedWord::options_read`] reads them (`time -p --`).
const STEPPED_OVER: &[Reserved] = &[
    Reserved::If,
    Reserved::Then,
    Reserved::Elif,
    Reserved::Else,
    Reserved::While,
    Reserved::Until,
    Reserved::Do,
    Reserved::Time,
];

/// The reserved word the walk steps over that `word` spells, as written: a
/// quoted or escaped spelling is an ordinary word to bash.
pub(crate) fn stepped_over(word: &str) -> Option<&'static ReservedWord> {
    reserved_word(word).filter(|reserved| STEPPED_OVER.contains(&reserved.word))
}

/// Whether `word` is one assignment word, in bash's own sense
/// ([`assignment_value`]): a NAME, then `=` or `+=`. The value is whatever
/// the rest of the word is, quotes included. Bash runs `1a=b`, `foo-bar=x`
/// and `"foo"=x` as commands.
pub(crate) fn is_assignment_word(word: &str) -> bool {
    assignment_value(word).is_some()
}

/// The word a wrapper head is written as, read once for every tier of the
/// walk: quotes and escapes removed, as bash reads a program name (`'env'`,
/// `\env`). A word that expands (`$D/env`) runs whatever the expansion holds
/// and has no name. A directory is part of the name: `/usr/bin/env` is the
/// program at that path, which only a wrapper family matches by its basename
/// ([`peel_wrapper`]).
fn head_name(word: &str) -> Option<Cow<'_, str>> {
    (!word_has_expansion(word)).then(|| as_bash_reads(word))
}

/// Whether `word` spells a program of a [`Form::RunWord`] or [`Form::Keyword`]
/// family by a path (`/usr/bin/env`, `'/bin/command'`). It is the program at
/// that path, not the word the family peels, so no layer takes it; but what it
/// runs is hidden behind it, so a caller leaves the command it heads as
/// written. A word that expands has no name and is none.
pub(crate) fn is_path_spelled_head<K>(line: &Line<'_>, at: usize, families: &[Family<K>]) -> bool {
    families.iter().any(|f| {
        matches!(f.form, Form::RunWord | Form::Keyword)
            && match_head(line, at, f.head, true).is_some()
    })
}

/// The number of words `head` (one or several, joined by single spaces)
/// covers when `line.words[at..]` starts with it: each word is read by its
/// [`head_name`] and matched whole, and each pair written with blanks (spaces or tabs) between,
/// however many.
fn head_len(line: &Line<'_>, at: usize, head: &str) -> Option<usize> {
    match_head(line, at, head, false)
}

/// [`head_len`], where `by_path` reads the first word as a program spelled by
/// a path, compared by its basename; the words of a multi-word head behind it
/// are read as written, so `/usr/bin/uv run` is the head `uv run` and
/// `/usr/bin/uv pip` is not.
fn match_head(line: &Line<'_>, at: usize, head: &str, by_path: bool) -> Option<usize> {
    let mut n = 0;
    for part in head.split(' ') {
        let word = line.words.get(at + n)?;
        let name = head_name(word.text)?;
        let spelled: &str = if by_path && n == 0 {
            name.contains('/').then(|| basename(&name))?
        } else {
            &name
        };
        if spelled != part {
            return None;
        }
        if n > 0 {
            let prev = &line.words[at + n - 1];
            let gap = line.text.get(prev.end..word.start)?;
            if gap.is_empty() || !gap.chars().all(|c| c == ' ' || c == '\t') {
                return None;
            }
        }
        n += 1;
    }
    Some(n)
}

/// Whether `word` is a [`Form::RunWord`] one of `families` declares, so it
/// belongs to the same run as `NAME=value`. The word is read once its quotes
/// and escapes are removed, as bash reads a program name (`'env'`, `\env`),
/// and a word that expands is none.
pub(crate) fn is_run_word<K>(word: &str, families: &[Family<K>]) -> bool {
    let Some(name) = head_name(word) else {
        return false;
    };
    families
        .iter()
        .any(|f| matches!(f.form, Form::RunWord) && f.head == name)
}

/// `word` with its quotes and escapes removed, as bash reads the name of a
/// program: `'timeout'`, `\timeout` and `"timeout"` all run `timeout`.
fn as_bash_reads(word: &str) -> Cow<'_, str> {
    if word.contains(['\'', '"', '\\']) {
        Cow::Owned(resolve_word_text(word))
    } else {
        Cow::Borrowed(word)
    }
}

/// Peels the run of `NAME=value` words and [`Form::RunWord`] words that
/// starts at `words[at]`, in any mix, as one layer labelled `kind`, the way
/// bash reads `A=1 runner B=2 cmd` as one environment for one command. It
/// takes whole words, so a quoted value is one word and `D='a note x=1'` is
/// an assignment whole.
///
/// A `NAME=value` word is an assignment only where a simple command starts
/// ([`Position::simple_command`]); anywhere else it is the name of a program.
/// A run word is a program found by `$PATH` wherever it sits, so no position
/// gates it, and once it or an assignment has started the run, a `NAME=value`
/// word right after it is the run going on.
pub(crate) fn peel_assignments<K: Copy>(
    line: &Line<'_>,
    at: usize,
    families: &[Family<K>],
    pos: Position,
    kind: K,
) -> Option<Peel<K>> {
    let mut end = at;
    while let Some(word) = line.words.get(end) {
        let member = if is_assignment_word(word.text) {
            end > at || pos.simple_command
        } else {
            is_run_word(word.text, families)
        };
        if !member {
            break;
        }
        end += 1;
    }
    (end > at).then_some(Peel {
        kind,
        end,
        next: Position::INNER,
    })
}

/// Peels a reserved word the walk steps over ([`stepped_over`]) at
/// `words[at]`, with the options bash reads after it, as the reading of the
/// line's grammar reads them ([`lexer::reserved_options_read`]), as one layer
/// labelled `kind`. Read only where a pipeline starts, and a pipeline starts
/// again behind it: `time -p time cmd`, `then time cmd`.
pub(crate) fn peel_reserved<K: Copy>(
    line: &Line<'_>,
    at: usize,
    pos: Position,
    kind: K,
) -> Option<Peel<K>> {
    if !pos.pipeline {
        return None;
    }
    let word = line.words.get(at)?;
    let reserved = stepped_over(word.text)?;
    let i = line.tokens.partition_point(|tok| tok.offset < word.start);
    let options = lexer::reserved_options_read(reserved, line.text, line.tokens, i);
    Some(Peel {
        kind,
        end: at + 1 + options,
        next: Position::START,
    })
}

/// Peels a [`Form::Keyword`] family whose head starts at `words[at]`, tried in
/// the order the table gives. What follows it is the program it runs.
pub(crate) fn peel_keyword<K: Copy>(
    line: &Line<'_>,
    at: usize,
    families: &[Family<K>],
) -> Option<Peel<K>> {
    families.iter().find_map(|family| {
        if !matches!(family.form, Form::Keyword) {
            return None;
        }
        let n = head_len(line, at, family.head)?;
        Some(Peel {
            kind: family.kind,
            end: at + n,
            next: Position::INNER,
        })
    })
}

/// The words of `line` from `at` that are plain arguments: the run up to the
/// first token that is neither an `Arg` nor a blank (an operator, a redirect,
/// a pipe or a shellism). A word glued to shell syntax (`>out.log`, `$(cmd)`,
/// `*/bin/x`) is no argument a wrapper can safely take, so the words stop
/// there. An unbraced `$NAME` glued to a word (`5$UNIT`) is an `Arg`, one
/// word with it; a braced `${T}` starts with a shellism and stops them.
fn plain_words<'a>(line: &Line<'a>, at: usize) -> &'a [Word<'a>] {
    let words = &line.words[at..];
    let Some(first) = words.first() else {
        return words;
    };
    let from = line.tokens.partition_point(|t| t.offset < first.start);
    let boundary = line.tokens[from..]
        .iter()
        .find(|t| t.kind != LexKind::Arg && !t.is_blank())
        .map(|t| t.offset);
    let n = words.partition_point(|w| boundary.is_none_or(|b| w.end <= b));
    &words[..n]
}

/// The word right after [`plain_words`], when shell syntax starts inside it:
/// `tool>out` in `wrap tool>out arg` starts as a plain word and turns into a
/// redirect. It is none of a wrapper's own flags or operands (in `wrap
/// 300>out tool` the `300>` is a redirect), but it can head the command the
/// wrapper runs, which is all that comes before the redirect.
fn straddling_word<'a>(line: &Line<'a>, at: usize, plain: &[Word<'a>]) -> Option<Word<'a>> {
    let word = *line.words.get(at + plain.len())?;
    let from = line.tokens.partition_point(|t| t.offset < word.start);
    let boundary = line.tokens[from..]
        .iter()
        .find(|t| t.kind != LexKind::Arg && !t.is_blank())?
        .offset;
    (boundary > word.start && boundary < word.end).then_some(word)
}

/// Reads a [`Form::Wrapper`]'s own flags and operands over `words` (its head
/// first) with its [`WrapperSpec::grammar`], and returns how many of `words`
/// the wrapper covers, so `words[n]` heads the command it runs. `None` when
/// the grammar does not reach one: an option it does not declare, a boolean
/// option given a value (`--verbose=1`, which getopt rejects), or no word
/// left for the command.
///
/// Every declared operand is required before the command, and no more: once
/// the last one is read the wrapper's own options are over, as GNU getopt in
/// `+` mode stops at the first argument it does not take. So in `wrap 300 -s
/// KILL cmd` the `-s` is the program the wrapper runs.
fn wrapper_boundary(words: &[Word<'_>], spec: &WrapperSpec) -> Option<usize> {
    if words.is_empty() {
        return None;
    }
    let args: Vec<&str> = words[1..].iter().map(|w| w.text).collect();
    let tokens = tokenize_grammar(&args, &spec.grammar);
    let mut operands_left = spec.operands;
    for (idx, tok) in tokens.iter().enumerate() {
        match tok.kind {
            ArgKind::DashDash => {}
            ArgKind::Long | ArgKind::Short => {
                tok.flag?;
                if tok.attached.is_some() && tok.value_spec().is_none() {
                    return None;
                }
            }
            ArgKind::Positional if tok.linked.is_some() => {}
            ArgKind::Positional if operands_left > 0 => {
                operands_left -= 1;
                if operands_left == 0 {
                    return tokens.get(idx + 1).map(|next| 1 + next.source_index);
                }
            }
            ArgKind::Positional => return Some(1 + tok.source_index),
        }
    }
    None
}

/// The last path component of `command`, split on `/` only, so the answer
/// does not depend on the platform (`std::path::Path` also splits on `\` on
/// Windows).
pub(crate) fn basename(command: &str) -> &str {
    command.rsplit('/').next().unwrap_or(command)
}

/// Peels a [`Form::Wrapper`] family whose head is `words[at]`, matched by the
/// basename of the word with its quotes and escapes removed, so a path
/// spelling and a quoted one match too. What it runs is a program, so nothing
/// behind it starts a command of bash's own grammar.
///
/// A head that carries an expansion never matches: `$D/wrap` reads as `wrap`
/// by its basename, but runs whatever `$D` holds.
///
/// The words a wrapper covers are plain arguments ([`plain_words`]) up to the
/// command it runs, which must be one of them or the word shell syntax starts
/// inside ([`straddling_word`]).
pub(crate) fn peel_wrapper<K: Copy>(
    line: &Line<'_>,
    at: usize,
    families: &[Family<K>],
) -> Option<Peel<K>> {
    let head_word = head_name(line.words.get(at)?.text)?;
    let head = basename(&head_word);
    families.iter().find_map(|family| {
        let Form::Wrapper(spec) = family.form else {
            return None;
        };
        if family.head != head {
            return None;
        }
        let plain = plain_words(line, at);
        let n = wrapper_boundary(plain, &spec).or_else(|| {
            let inner = straddling_word(line, at, plain)?;
            let mut with_inner = plain.to_vec();
            with_inner.push(inner);
            wrapper_boundary(&with_inner, &spec).filter(|&n| n == plain.len())
        })?;
        Some(Peel {
            kind: family.kind,
            end: at + n,
            next: Position::INNER,
        })
    })
}

/// Peels the first of `prefixes` (the caller sorts longer ones first) that
/// the command starts with at `words[at]`, matched as literal text that ends
/// where a word of the command ends. The caller refuses an entry that does
/// not lex into whole words, so the word it ends on is one a lex of the
/// command reads there. The layer is labelled `kind`, and the walk stands at
/// `next` behind it: what the prefix runs is not known to the walker.
pub(crate) fn peel_literal_prefix<K: Copy>(
    line: &Line<'_>,
    at: usize,
    prefixes: &[String],
    kind: K,
    next: Position,
) -> Option<Peel<K>> {
    let start = line.words.get(at)?.start;
    let end_of_line = line.words.last().map_or(start, |w| w.end);
    let rest = &line.text[start..end_of_line];
    prefixes.iter().find_map(|prefix| {
        if prefix.is_empty() || !rest.starts_with(prefix.as_str()) {
            return None;
        }
        let end = start + prefix.len();
        let last = line.words[at..].partition_point(|w| w.end < end);
        (line.words.get(at + last)?.end == end).then_some(Peel {
            kind,
            end: at + last + 1,
            next,
        })
    })
}

/// The first layer the walker finds at `words[at]`, tried in the one order
/// every walk uses: an assignment run, a reserved word, a keyword, a wrapper,
/// a literal prefix. A wrapper with a literal `rtk` word among its own
/// flags and operands (`/usr/bin/time -o rtk cmd`, whose output file is named
/// `rtk`) is no layer, and the walk does not peel the wrapper. `kinds` labels the
/// assignment run, the reserved word and the literal prefix, which no family
/// declares, and the walk stands at `after_prefix` behind a prefix.
pub(crate) fn peel_tiers<K: Copy>(
    line: &Line<'_>,
    at: usize,
    families: &[Family<K>],
    prefixes: &[String],
    pos: Position,
    kinds: (K, K, K),
    after_prefix: Position,
) -> Option<Peel<K>> {
    peel_assignments(line, at, families, pos, kinds.0)
        .or_else(|| peel_reserved(line, at, pos, kinds.1))
        .or_else(|| peel_keyword(line, at, families))
        .or_else(|| {
            peel_wrapper(line, at, families)
                .filter(|peel| !line.words[at..peel.end].iter().any(|w| w.text == "rtk"))
        })
        .or_else(|| peel_literal_prefix(line, at, prefixes, kinds.2, after_prefix))
}

#[cfg(test)]
pub(crate) use tests_support::{TextPeel, peel_text};

/// A layer read out of plain text, for the accept/reject tables of the family
/// files and the walker's own tests.
#[cfg(test)]
mod tests_support {
    use super::*;
    use crate::core::cmdline::lexer::{tokenize, words};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct TextPeel<'a, K> {
        pub(crate) kind: K,
        /// The text up to where the last word the layer took ends.
        pub(crate) prefix: &'a str,
        /// The text from the next word on, `""` when nothing follows.
        pub(crate) rest: &'a str,
        pub(crate) next: Position,
    }

    /// The first layer of `text` that [`peel_tiers`] finds.
    pub(crate) fn peel_text<'a, K: Copy>(
        text: &'a str,
        families: &[Family<K>],
        prefixes: &[String],
        pos: Position,
        kinds: (K, K, K),
    ) -> Option<TextPeel<'a, K>> {
        let toks = tokenize(text);
        let ws = words(text, &toks);
        let line = Line {
            text,
            words: &ws,
            tokens: &toks,
        };
        let peel = peel_tiers(&line, 0, families, prefixes, pos, kinds, Position::STAGE)?;
        Some(TextPeel {
            kind: peel.kind,
            prefix: &text[..ws[peel.end - 1].end],
            rest: ws.get(peel.end).map_or("", |w| &text[w.start..]),
            next: peel.next,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::arg_tokenizer::{Flag, Grammar, ValueSpec};
    use crate::core::cmdline::bash_grammar::RESERVED_WORDS;
    use crate::core::cmdline::lexer::{tokenize, words};

    /// The caller's own labels for the layers, as `discover` has its own.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Kind {
        Keyword,
        Routable,
        Wrapper,
        Assign,
        Reserved,
        Prefix,
    }
    const KINDS: (Kind, Kind, Kind) = (Kind::Assign, Kind::Reserved, Kind::Prefix);

    /// A small synthetic table for the mechanism this module provides; the
    /// real tables have their own accept/reject tests in `discover::families`.
    const KW: Family<Kind> = Family {
        head: "kw",
        kind: Kind::Keyword,
        form: Form::Keyword,
    };
    const ROUTABLE: Family<Kind> = Family {
        head: "rt run",
        kind: Kind::Routable,
        form: Form::Keyword,
    };
    const ENV_KW: Family<Kind> = Family {
        head: "run",
        kind: Kind::Assign,
        form: Form::RunWord,
    };
    const WRAP_FLAGS: &[Flag] = &[
        Flag::pair("v", "verbose"),
        Flag::pair("d", "delay").takes(ValueSpec::value()),
    ];
    const WRAP: Family<Kind> = Family {
        head: "wrap",
        kind: Kind::Wrapper,
        form: Form::Wrapper(WrapperSpec {
            operands: 1,
            grammar: Grammar::posix(&[WRAP_FLAGS]),
        }),
    };
    const NUMERIC: Family<Kind> = Family {
        head: "num",
        kind: Kind::Wrapper,
        form: Form::Wrapper(WrapperSpec {
            operands: 0,
            grammar: Grammar::posix(&[WRAP_FLAGS]).numeric(),
        }),
    };
    const TABLE: &[Family<Kind>] = &[ROUTABLE, KW, ENV_KW, WRAP, NUMERIC];

    fn peel(text: &str, pos: Position) -> Option<(Kind, String, String, Position)> {
        peel_text(text, TABLE, &[], pos, KINDS)
            .map(|p| (p.kind, p.prefix.to_string(), p.rest.to_string(), p.next))
    }

    fn kind_prefix_rest(text: &str) -> Option<(Kind, String, String)> {
        peel(text, Position::START).map(|(k, p, r, _)| (k, p, r))
    }

    #[test]
    fn a_keyword_is_peeled_and_leaves_no_position_behind() {
        assert_eq!(
            peel("kw tool arg", Position::START),
            Some((
                Kind::Keyword,
                "kw".into(),
                "tool arg".into(),
                Position::INNER
            ))
        );
    }

    #[test]
    fn a_multi_word_head_matches_whole_words_written_with_blanks_between() {
        assert_eq!(
            kind_prefix_rest("rt run pytest"),
            Some((Kind::Routable, "rt run".into(), "pytest".into()))
        );
        assert_eq!(kind_prefix_rest("rt runner pytest"), None);
        for text in ["rt  run pytest", "rt\trun pytest", "rt \t run pytest"] {
            assert_eq!(
                kind_prefix_rest(text),
                Some((
                    Kind::Routable,
                    text.split_once("run").map_or("", |(a, _)| a).to_string() + "run",
                    "pytest".into()
                )),
                "{text:?}"
            );
        }
        assert_eq!(kind_prefix_rest("rt"), None);
    }

    #[test]
    fn a_wrapper_head_is_matched_with_its_quotes_and_escapes_removed() {
        for head in [
            "wrap",
            "'wrap'",
            "\"wrap\"",
            "\\wrap",
            "w'ra'p",
            "'/usr/bin/wrap'",
        ] {
            let text = format!("{head} 300 tool");
            let peeled = kind_prefix_rest(&text);
            assert_eq!(
                peeled,
                Some((Kind::Wrapper, format!("{head} 300"), "tool".into())),
                "{text:?}"
            );
        }
        assert_eq!(kind_prefix_rest("\"$D\"/wrap 300 tool"), None);
    }

    #[test]
    fn a_keyword_with_nothing_after_it_reports_the_end_of_the_words() {
        let toks = tokenize("kw");
        let ws = words("kw", &toks);
        let line = Line {
            text: "kw",
            words: &ws,
            tokens: &toks,
        };
        let peel = peel_keyword(&line, 0, TABLE).expect("kw");
        assert_eq!(peel.end, ws.len());
    }

    /// Every word the walk steps over is one bash runs a pipeline after.
    #[test]
    fn the_stepped_over_words_are_words_a_pipeline_follows() {
        for row in RESERVED_WORDS {
            if STEPPED_OVER.contains(&row.word) {
                assert!(row.pipeline_follows(), "{}", row.spelling);
            }
        }
        for word in ["if", "then", "elif", "else", "while", "until", "do", "time"] {
            assert!(stepped_over(word).is_some(), "{word}");
        }
        for word in [
            "coproc", "for", "case", "fi", "done", "esac", "in", "\"if\"", "Time",
        ] {
            assert!(stepped_over(word).is_none(), "{word}");
        }
    }

    #[test]
    fn a_reserved_word_is_read_where_a_pipeline_starts_and_a_pipeline_starts_behind_it() {
        for (text, prefix, rest) in [
            ("then tool arg", "then", "tool arg"),
            ("do tool arg", "do", "tool arg"),
            ("if tool arg", "if", "tool arg"),
            ("time tool arg", "time", "tool arg"),
            ("time -p tool arg", "time -p", "tool arg"),
            ("time -- tool arg", "time --", "tool arg"),
            ("time -p -- tool arg", "time -p --", "tool arg"),
            ("time time tool", "time", "time tool"),
            ("time -p >out tool", "time -p", ">out tool"),
        ] {
            assert_eq!(
                peel(text, Position::START),
                Some((Kind::Reserved, prefix.into(), rest.into(), Position::START)),
                "{text}"
            );
        }
        for pos in [Position::STAGE, Position::INNER] {
            assert_eq!(peel("then tool arg", pos), None);
            assert_eq!(peel("time tool arg", pos), None);
        }
    }

    /// Bash compares `time`'s options as written, each once and in order, so
    /// any other word after them is the command.
    #[test]
    fn a_reserved_words_options_are_its_own_words_in_order() {
        for (text, rest) in [
            ("time -v tool", "-v tool"),
            ("time -p -p tool", "-p tool"),
            ("time -- -p tool", "-p tool"),
            ("time -pv tool", "-pv tool"),
            ("time -P tool", "-P tool"),
            ("time \"-p\" tool", "\"-p\" tool"),
            ("then -p tool", "-p tool"),
            // A redirect glued to an option is text of its word.
            ("time -p>out tool", "-p>out tool"),
            ("time -->x tool", "-->x tool"),
            ("time -p -->x tool", "-->x tool"),
        ] {
            assert_eq!(
                peel(text, Position::START).map(|(_, _, r, _)| r),
                Some(rest.to_string()),
                "{text}"
            );
        }
        assert_eq!(
            peel("time -p", Position::START).map(|(_, p, r, _)| (p, r)),
            Some(("time -p".into(), String::new()))
        );
    }

    #[test]
    fn a_wrapper_takes_its_flags_and_operands_then_the_inner_command() {
        for (text, prefix, rest) in [
            ("wrap 300 tool run", "wrap 300", "tool run"),
            ("wrap -v 300 tool run", "wrap -v 300", "tool run"),
            ("wrap -vd 5 300 tool run", "wrap -vd 5 300", "tool run"),
            ("wrap -d5 300 tool run", "wrap -d5 300", "tool run"),
            (
                "wrap --delay=5 300 tool run",
                "wrap --delay=5 300",
                "tool run",
            ),
            ("wrap -- 300 tool run", "wrap -- 300", "tool run"),
            ("/bin/wrap 300 tool run", "/bin/wrap 300", "tool run"),
            // The operand ends the wrapper's own grammar.
            ("wrap 300 -v tool run", "wrap 300", "-v tool run"),
            ("num -5 tool", "num -5", "tool"),
            ("num -d 5 tool", "num -d 5", "tool"),
            ("num +5 tool", "num", "+5 tool"),
        ] {
            assert_eq!(
                kind_prefix_rest(text),
                Some((Kind::Wrapper, prefix.into(), rest.into())),
                "{text}"
            );
        }
    }

    #[test]
    fn a_wrapper_stops_at_what_its_grammar_does_not_declare() {
        for text in [
            "wrap 300",
            "wrap -x 300 tool run",
            "wrap --verbose=1 300 tool run",
            "wrap --v 300 tool run",
            "wrap -d",
            "wrap -5 300 tool run",
            "num -5v tool",
            "num -v5 tool",
            "$D/wrap 300 tool run",
            "${D}/wrap 300 tool run",
            "*/wrap 300 tool run",
            "wrap 300 >out.log tool run",
            "wrap 300 $(which tool) test",
            "wrap (tool build)",
        ] {
            assert_eq!(kind_prefix_rest(text), None, "{text}");
        }
    }

    #[test]
    fn a_wrapper_peels_an_inner_head_that_a_redirect_is_glued_to() {
        for (text, prefix, rest) in [
            ("wrap 300 tool>out test", "wrap 300", "tool>out test"),
            ("wrap -v 300 tool>out", "wrap -v 300", "tool>out"),
            ("wrap 300 tool<in test", "wrap 300", "tool<in test"),
        ] {
            assert_eq!(
                kind_prefix_rest(text),
                Some((Kind::Wrapper, prefix.into(), rest.into())),
                "{text}"
            );
        }
        // Not a word that turns into a redirect where the wrapper's own
        // operand or flag value was to be, nor a redirect that leads.
        for text in [
            "wrap 300>out tool run",
            "wrap -d 5>out tool run",
            "wrap 300 >out tool run",
            "wrap 300 2>&1 tool run",
            "wrap tool>out",
        ] {
            assert_eq!(kind_prefix_rest(text), None, "{text}");
        }
    }

    #[test]
    fn plain_words_stop_at_the_first_shell_token() {
        for (text, count) in [
            ("wrap 300 >out.log tool run", 2),
            ("wrap (tool build)", 1),
            ("wrap 300 tool run", 4),
            ("wrap 5$UNIT x", 3),
            ("wrap ${T} x", 1),
        ] {
            let toks = tokenize(text);
            let ws = words(text, &toks);
            let line = Line {
                text,
                words: &ws,
                tokens: &toks,
            };
            assert_eq!(plain_words(&line, 0).len(), count, "{text}");
        }
    }

    #[test]
    fn a_run_of_assignments_and_run_words_is_one_layer() {
        assert_eq!(
            kind_prefix_rest("A=\"x y\" B=1 run C=2 tool arg"),
            Some((
                Kind::Assign,
                "A=\"x y\" B=1 run C=2".into(),
                "tool arg".into()
            ))
        );
        // Without the family declaring `run`, it is just a word.
        assert_eq!(
            peel_text("A=1 run tool arg", &[], &[], Position::START, KINDS)
                .map(|p| (p.prefix.to_string(), p.rest.to_string())),
            Some(("A=1".into(), "run tool arg".into()))
        );
    }

    #[test]
    fn an_assignment_needs_a_simple_command_start_but_a_run_word_does_not() {
        assert_eq!(peel("A=1 tool arg", Position::INNER), None);
        assert!(peel("A=1 tool arg", Position::STAGE).is_some());
        assert_eq!(
            peel("run A=1 tool arg", Position::INNER).map(|(_, p, r, _)| (p, r)),
            Some(("run A=1".into(), "tool arg".into()))
        );
    }

    #[test]
    fn a_user_prefix_matches_text_ending_on_a_word_boundary() {
        let prefixes = vec!["a b".to_string(), "a".to_string()];
        let at = |text: &str| {
            peel_text(text, &[], &prefixes, Position::START, KINDS)
                .map(|p| (p.prefix.to_string(), p.rest.to_string()))
        };
        assert_eq!(at("a b c"), Some(("a b".into(), "c".into())));
        assert_eq!(at("a c"), Some(("a".into(), "c".into())));
        assert_eq!(at("ab c"), None);
        assert_eq!(at("a"), Some(("a".into(), "".into())));
    }

    #[test]
    fn an_assignment_word_is_a_name_then_an_equals_sign() {
        for word in ["A=1", "a_1=", "_=x", "PATH+=x", "A=b=c"] {
            assert!(is_assignment_word(word), "{word}");
        }
        for word in [
            "",
            "=",
            "1A=x",
            "A-B=x",
            "A",
            "A+",
            "A +=x",
            "\"A\"=x",
            "A\u{a0}=x",
        ] {
            assert!(!is_assignment_word(word), "{word}");
        }
    }
}
