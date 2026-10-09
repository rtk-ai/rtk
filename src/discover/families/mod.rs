//! One file per wrapper family the rewrite peels, each a plain data table
//! with its own accept/reject table test. [`ALL`] is the list the walker
//! ([`walker`]) is handed. A family's `kind` is the label the rewrite reads
//! back ([`LayerKind`]); its `Form` decides which of the walker's fixed tiers
//! it is tried in (a `Form::RunWord` in the assignment run, a
//! `Form::Keyword` among the keywords, a `Form::Wrapper` among the wrappers),
//! never this list's order. Within a tier the order matters: a tier tries its
//! families as listed and takes the first that matches.
//!
//! To add a family, add a file here declaring its `Family` values with an
//! accept/reject table test (calling [`peel_text`] with [`ALL`]), and list
//! them in [`ALL`].
//!
//! Bash's reserved words are no family: the walker reads them from
//! `core/cmdline/bash_grammar.rs`. A user's `transparent_prefixes` are no
//! family either: they are configuration text the walker matches as literal
//! words ([`user`]).

pub(crate) mod env;
pub(crate) mod precommand;
pub(crate) mod process;
pub(crate) mod user;
pub(crate) mod uv_run;

use crate::core::cmdline::family;
#[cfg(test)]
use crate::core::cmdline::walker;
use crate::core::cmdline::walker::Position;

/// What the rewrite does with a layer: it labels every family of the tables,
/// and the layers the walker finds without one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LayerKind {
    /// A run of `NAME=value` words and `env`, in any mix: one environment for
    /// one command. Read by the walker itself.
    Assignments,
    /// A reserved word bash runs a pipeline after (`time`, `then`, `do`, ...),
    /// with its options. Read by the walker from bash's grammar.
    ReservedWord,
    /// A word that runs its argument as the command it names (`command`,
    /// `exec`, `noglob`, ...). It is no program on its own (`rtk exec date`
    /// fails), so a command behind it that has no rewrite of its own leaves
    /// the whole command as written.
    Precommand,
    /// A wrapper the rules also match whole (`uv run`): a command behind it
    /// that has no rewrite of its own falls through to the wrapper's rule.
    RoutableWrapper,
    /// `timeout`, `time`, `nice`, `nohup`: the command behind it keeps its
    /// process identity, so `rtk` runs inside the wrapper.
    ProcessWrapper,
    /// One of the user's `transparent_prefixes` ([`user`]).
    UserPrefix,
}

/// A family of the tables, labelled with what the rewrite does with it.
pub(crate) type Family = family::Family<LayerKind>;

/// Where the walk stands behind a user prefix: what the prefix runs is not
/// known, so an assignment may follow it (as it does when the prefix only
/// wraps, and when it takes a command of its own), and no pipeline starts.
pub(crate) const AFTER_USER_PREFIX: Position = Position::STAGE;

/// The first layer of `text` the walker finds, for the accept/reject tables of
/// the family files.
#[cfg(test)]
pub(crate) fn peel_text<'a>(
    text: &'a str,
    families: &[Family],
    prefixes: &[String],
    pos: Position,
) -> Option<walker::TextPeel<'a, LayerKind>> {
    walker::peel_text(
        text,
        families,
        prefixes,
        pos,
        (
            LayerKind::Assignments,
            LayerKind::ReservedWord,
            LayerKind::UserPrefix,
        ),
    )
}

/// Every built-in family.
pub(crate) static ALL: &[Family] = &[
    env::ENV,
    uv_run::UV_RUN,
    precommand::NOGLOB,
    precommand::COMMAND,
    precommand::BUILTIN,
    precommand::EXEC,
    precommand::NOCORRECT,
    process::TIMEOUT,
    process::TIME,
    process::NICE,
    process::NOHUP,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Bash's reserved words the walker steps over are read from its grammar
    /// where a pipeline starts, and a pipeline starts again behind them.
    #[test]
    fn a_reserved_word_is_a_layer_of_its_own_where_a_pipeline_starts() {
        for (cmd, prefix, rest) in [
            ("time git status", "time", "git status"),
            ("time -p git status", "time -p", "git status"),
            ("time -- git status", "time --", "git status"),
            ("then git status", "then", "git status"),
            ("do cargo build", "do", "cargo build"),
            ("if git status", "if", "git status"),
            ("else git status", "else", "git status"),
        ] {
            let peel = peel_text(cmd, ALL, &[], Position::START).expect(cmd);
            assert_eq!(peel.kind, LayerKind::ReservedWord, "{cmd}");
            assert_eq!((peel.prefix, peel.rest), (prefix, rest), "{cmd}");
            assert_eq!(peel.next, Position::START, "{cmd}");
        }
    }

    /// Where no pipeline starts, bare `time` is the program: GNU `time` and
    /// its own options. Where one does, `time` takes only `-p` and `--`, and
    /// the next word is the command, whatever it looks like.
    #[test]
    fn time_is_the_reserved_word_where_a_pipeline_starts_and_gnu_time_elsewhere() {
        let at =
            |cmd: &str, pos| peel_text(cmd, ALL, &[], pos).map(|p| (p.kind, p.prefix.to_string()));
        assert_eq!(
            at("time -v cargo build", Position::START),
            Some((LayerKind::ReservedWord, "time".into()))
        );
        assert_eq!(
            at("time -p -p cargo build", Position::START),
            Some((LayerKind::ReservedWord, "time -p".into()))
        );
        assert_eq!(
            at("time -v cargo build", Position::INNER),
            Some((LayerKind::ProcessWrapper, "time -v".into()))
        );
        assert_eq!(
            at("time -f %e cargo build", Position::STAGE),
            Some((LayerKind::ProcessWrapper, "time -f %e".into()))
        );
        assert_eq!(
            at("/usr/bin/time -v cargo build", Position::START),
            Some((LayerKind::ProcessWrapper, "/usr/bin/time -v".into()))
        );
    }

    /// `coproc` runs one command and no pipeline, and the words after which
    /// no command starts open or close a compound command: none is a layer.
    #[test]
    fn other_reserved_words_are_no_layer() {
        for cmd in [
            "coproc git status",
            "for x in a",
            "case x in",
            "fi",
            "done",
            "esac",
            "function f",
        ] {
            assert!(peel_text(cmd, ALL, &[], Position::START).is_none(), "{cmd}");
        }
    }

    /// Every family's head is one or more words joined by single spaces, and
    /// no two families of one tier answer to the same head.
    #[test]
    fn heads_are_words_and_each_is_declared_once_per_tier() {
        for (i, a) in ALL.iter().enumerate() {
            assert!(
                !a.head.is_empty() && a.head.split(' ').all(|w| !w.is_empty()),
                "{}",
                a.head
            );
            for b in &ALL[i + 1..] {
                let same_tier = std::mem::discriminant(&a.form) == std::mem::discriminant(&b.form);
                assert!(!(same_tier && a.head == b.head), "{} twice", a.head);
            }
        }
    }
}
