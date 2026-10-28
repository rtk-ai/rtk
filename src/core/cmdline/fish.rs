//! What RTK knows of fish: the words fish reads as its own where a command
//! starts, and one reading of a command's tokens against them.
//!
//! The tokens are the shared lexer's, which reads every string as bash
//! (`lexer::ShellDialect`), so fish's view of a string is told by where it
//! parts from bash's: a word one of the two shells reserves and the other does
//! not, and the syntax only one of them has.

use super::bash_grammar::reserved_word;
use super::lexer::{Token, TokenKind};

/// fish's keywords where a command starts, as fish 3.7 and 4 both read them,
/// each with whether it is control flow: a word of one of fish's blocks, or
/// `and`, `or` and `not`, which join and negate commands. `!` and `time`
/// prefix a command as they do in bash. fish 4 also groups commands in
/// `{ … }`, which fish 3.7 refuses, so neither brace is listed.
const KEYWORDS: &[(&str, bool)] = &[
    ("and", true),
    ("begin", true),
    ("case", true),
    ("else", true),
    ("end", true),
    ("for", true),
    ("function", true),
    ("if", true),
    ("not", true),
    ("or", true),
    ("switch", true),
    ("while", true),
    ("!", false),
    ("time", false),
];

/// Whether fish reads `word` as a control keyword where a command starts, or
/// `None` when fish reads it as no keyword there.
fn keyword(word: &str) -> Option<bool> {
    KEYWORDS
        .iter()
        .find(|(name, _)| *name == word)
        .map(|&(_, control)| control)
}

/// What fish makes of a command string RTK lexed as bash.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Reading {
    /// A control keyword of fish where a command starts: syntax the rewrite,
    /// which reads the string as bash, does not model.
    pub(crate) control: bool,
    /// A keyword only fish reserves where a command starts (`end`, `and`,
    /// `begin`): bash would run it as a program.
    pub(crate) fish_only: bool,
    /// Syntax only bash has: a word bash reserves and fish does not where a
    /// command starts (`then`, `done`, `until`), `[[` anywhere, an unquoted
    /// backquote, or a heredoc.
    pub(crate) bash_only: bool,
    /// An unquoted `(` or `)`: fish's command substitution, which the bash
    /// reading takes for a subshell.
    pub(crate) substitution: bool,
}

/// Read `tokens` as fish reads the string they tile.
///
/// A command starts at the first token and after each `;`, `&&`, `||`, pipe,
/// newline and background `&`; the first word there is the one that may be a
/// keyword.
pub(crate) fn read(tokens: &[Token<'_>]) -> Reading {
    let mut reading = Reading::default();
    let mut command_position = true;
    for token in tokens {
        match token.kind {
            TokenKind::Operator | TokenKind::Pipe(_) | TokenKind::Newline => {
                command_position = true
            }
            TokenKind::Shellism => match token.value {
                "&" => command_position = true,
                "(" | ")" => reading.substitution = true,
                // fish has no backquote substitution; a quoted one stays in
                // its word.
                "`" => reading.bash_only = true,
                _ => {}
            },
            // `<<`, `<<-` and `<<<` all lex with a `<<` prefix, and fish has
            // none of them.
            TokenKind::Redirect if token.value.starts_with("<<") => reading.bash_only = true,
            TokenKind::Arg => {
                if token.value == "[[" {
                    reading.bash_only = true;
                }
                if command_position {
                    let bash_reserved = reserved_word(token.value).is_some();
                    match keyword(token.value) {
                        Some(control) => {
                            reading.control |= control;
                            reading.fish_only |= !bash_reserved;
                        }
                        None => reading.bash_only |= bash_reserved,
                    }
                    command_position = false;
                }
            }
            _ => {}
        }
    }
    reading
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::cmdline::lexer::tokenize;

    fn reading(cmd: &str) -> Reading {
        read(&tokenize(cmd))
    }

    #[test]
    fn test_fish_only_keywords_are_the_ones_bash_does_not_reserve() {
        let fish_only: Vec<&str> = KEYWORDS
            .iter()
            .map(|&(name, _)| name)
            .filter(|name| reserved_word(name).is_none())
            .collect();
        assert_eq!(fish_only, ["and", "begin", "end", "not", "or", "switch"]);
    }

    #[test]
    fn test_a_keyword_counts_only_where_a_command_starts() {
        for cmd in [
            "end",
            "true; and echo",
            "true && or echo",
            "true | not grep x",
            "true &\nbegin",
            "true & end",
        ] {
            let read = reading(cmd);
            assert!(read.control && read.fish_only, "{cmd:?}");
        }
        for cmd in ["echo end", "git log --and", "'end'", "x=end"] {
            assert_eq!(reading(cmd), Reading::default(), "{cmd:?}");
        }
        // Both shells reserve these: control flow to fish, never fish's alone.
        for cmd in ["if true", "while x", "for x in a", "function f", "case x"] {
            let read = reading(cmd);
            assert!(
                read.control && !read.fish_only && !read.bash_only,
                "{cmd:?}"
            );
        }
        for cmd in ["! true", "time true"] {
            assert_eq!(reading(cmd), Reading::default(), "{cmd:?}");
        }
    }

    #[test]
    fn test_bash_only_syntax() {
        for cmd in [
            "if x; then y; fi",
            "for x in a; do y; done",
            "until x; do y; done",
            "select x in a; do y; done",
            "coproc x",
            "echo [[ x ]]",
            "echo `date`",
            "cat <<EOF",
            "cat <<< x",
        ] {
            assert!(reading(cmd).bash_only, "{cmd:?}");
        }
        for cmd in ["echo then", "echo '`date`'", "echo \"`date`\"", "'then'"] {
            assert!(!reading(cmd).bash_only, "{cmd:?}");
        }
    }

    #[test]
    fn test_a_bare_paren_is_fish_substitution() {
        assert!(reading("echo (date)").substitution);
        assert!(reading("(cd x)").substitution);
        assert!(!reading("echo '(x)' \\(y\\)").substitution);
    }
}
