//! What one family of command-line wrappers is: the head word or words that
//! introduce it, how it is shaped, and the flag grammar of its own when it
//! has one. [`super::walker`] reads these tables and names no family itself,
//! so adding a wrapper is a table entry in the caller's own tables
//! (`discover/families/`), not a change to the walker.

use crate::core::arg_tokenizer::Grammar;

/// How a family's words sit in front of the command it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Form {
    /// A bare word, or a fixed run of words matched word by word, with no
    /// options of its own: whatever follows it is the command it runs.
    Keyword,
    /// A bare word that belongs to the run of `NAME=value` words the walker
    /// reads where a simple command starts, in any mix with them. It is a
    /// program found wherever it sits, so no position gates it.
    RunWord,
    /// A program with options of its own and a fixed number of operands
    /// before the command it runs.
    ///
    /// An option its grammar does not declare stops the match: guessing
    /// whether that option takes a value could shift the command by a word.
    Wrapper(WrapperSpec),
}

/// A [`Form::Wrapper`]'s shape: its flags, and how many operands come before
/// the command it runs. The operands describe the wrapper, not its flags, so
/// they are not part of the [`Grammar`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WrapperSpec {
    pub(crate) operands: usize,
    pub(crate) grammar: Grammar,
}

/// One wrapper family: its head, the label a caller reads back for a layer of
/// it (`K`, which the walker never reads), and its [`Form`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct Family<K> {
    /// One word, or several joined by single spaces, each matched whole for
    /// a [`Form::Keyword`]. A [`Form::Wrapper`]'s head is matched by the
    /// basename of the word, so a path spelling matches too.
    pub(crate) head: &'static str,
    pub(crate) kind: K,
    pub(crate) form: Form,
}
