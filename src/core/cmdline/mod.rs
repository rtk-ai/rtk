//! Command-line parsing: bash's reserved words, a quote-aware shell lexer,
//! span edits against the text it lexed, and rtk's own command line read from
//! its words.

pub mod bash_grammar;
pub(crate) mod edit;
pub mod lexer;
pub(crate) mod rtk;
