//! Command-line parsing: bash's reserved words, a quote-aware shell lexer,
//! span edits against the text it lexed, rtk's own command line read from
//! its words, and the walker that peels transparent wrappers off a command
//! along the tables a caller declares their families in.

pub mod bash_grammar;
pub(crate) mod edit;
pub(crate) mod family;
pub mod lexer;
pub(crate) mod rtk;
pub(crate) mod walker;
