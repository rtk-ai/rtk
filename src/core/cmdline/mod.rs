//! Command-line parsing: a quote-aware shell lexer, span edits against the
//! text it lexed, and rtk's own command line read from its words.

pub(crate) mod edit;
pub mod lexer;
pub(crate) mod rtk;
