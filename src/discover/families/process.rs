//! `timeout`, `nice`, `nohup` and `time`: process wrappers that keep the
//! identity of the command they run, so `rtk` goes inside them
//! ([`LayerKind::ProcessWrapper`]).
//!
//! `time` here is GNU `time(1)` and its `-f`/`-o`/`-a`/`-v`/`-p`/`-q` grammar,
//! read through a path (`/usr/bin/time`) and wherever bash does not read the
//! bare word as its reserved word: behind an assignment run, `env`, `exec`,
//! `nice`, `timeout` or any other layer that runs a program, and in a later
//! stage of a pipeline (`nice time -v cmd` hands `nice` the word `time`, which
//! it finds by `$PATH`). Where a pipeline starts, bare `time` is bash's
//! reserved word, which the walker reads from `core/cmdline/bash_grammar.rs`.
//!
//! The `timeout` and `nice` grammars follow GNU coreutils' `--help`: each flag
//! has the short and long spellings getopt accepts, so `--v` is not `-v`, and
//! a boolean flag given `--flag=value` is refused as getopt refuses it. A
//! short cluster is read letter by letter, so `timeout -vk 5 300` and
//! `/usr/bin/time -pv` read as getopt reads them.
//!
//! GNU `getopt_long` also accepts an unambiguous prefix of a long option
//! (`--sig` for `--signal`). These grammars take exact long names only, and
//! refuse `nice --N` and `nice -+N` too: they declare the options a wrapper
//! needs, not its whole flag set, so whether a prefix is unambiguous cannot be
//! told from them, and refusing is the conservative reading.
//!
//! GNU `nice` takes `-N`, `-n N` and `--adjustment=N`, never a leading `+`, so
//! `nice +5 cmd` runs a program named `+5`, as bash runs it.

use super::{Family, LayerKind};
use crate::core::arg_tokenizer::{Flag, Grammar, ValueSpec};
use crate::core::cmdline::family::{Form, WrapperSpec};

const fn process(head: &'static str, operands: usize, grammar: Grammar) -> Family {
    Family {
        head,
        kind: LayerKind::ProcessWrapper,
        form: Form::Wrapper(WrapperSpec { operands, grammar }),
    }
}

/// `timeout [OPTION] DURATION COMMAND [ARG]...`
const TIMEOUT_GRAMMAR: Grammar = Grammar::posix(&[&[
    Flag::pair("s", "signal").takes(ValueSpec::value()),
    Flag::pair("k", "kill-after").takes(ValueSpec::value()),
    Flag::pair("p", "preserve-status"),
    Flag::pair("f", "foreground"),
    Flag::pair("v", "verbose"),
]]);

pub(crate) const TIMEOUT: Family = process("timeout", 1, TIMEOUT_GRAMMAR);

/// GNU `time [options] command [arguments...]`.
const TIME_GRAMMAR: Grammar = Grammar::posix(&[&[
    Flag::pair("f", "format").takes(ValueSpec::value()),
    Flag::pair("o", "output").takes(ValueSpec::value()),
    Flag::pair("a", "append"),
    Flag::pair("v", "verbose"),
    Flag::pair("p", "portability"),
    Flag::pair("q", "quiet"),
]]);

pub(crate) const TIME: Family = process("time", 0, TIME_GRAMMAR);

/// `nice [OPTION] [COMMAND [ARG]...]`, with its `-N` adjustment.
const NICE_GRAMMAR: Grammar =
    Grammar::posix(&[&[Flag::pair("n", "adjustment").takes(ValueSpec::value())]]).numeric();

pub(crate) const NICE: Family = process("nice", 0, NICE_GRAMMAR);

pub(crate) const NOHUP: Family = process("nohup", 0, Grammar::posix(&[]));

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::arg_tokenizer::{TokenKind, assert_takes_value_table};
    use crate::core::cmdline::walker::Position;
    use crate::discover::families::{ALL, peel_text};

    fn accepts(cmd: &str, prefix: &str, rest: &str) {
        accepts_at(cmd, Position::START, prefix, rest);
    }

    fn rejects(cmd: &str) {
        rejects_at(cmd, Position::START);
    }

    fn accepts_at(cmd: &str, pos: Position, prefix: &str, rest: &str) {
        let peel =
            peel_text(cmd, ALL, &[], pos).unwrap_or_else(|| panic!("expected a peel for {cmd:?}"));
        assert_eq!(peel.kind, LayerKind::ProcessWrapper, "{cmd:?}");
        assert_eq!(peel.prefix, prefix, "{cmd:?}");
        assert_eq!(peel.rest, rest, "{cmd:?}");
    }

    /// No process wrapper is peeled: nothing is, or bash's reserved word is.
    fn rejects_at(cmd: &str, pos: Position) {
        assert!(
            peel_text(cmd, ALL, &[], pos).is_none_or(|p| p.kind != LayerKind::ProcessWrapper),
            "expected no process wrapper for {cmd:?} at {pos:?}"
        );
    }

    #[test]
    fn grammars() {
        let value = Some(ValueSpec::value());
        assert_takes_value_table(
            &TIMEOUT_GRAMMAR,
            &[
                (TokenKind::Short, &["s", "k"], value),
                (TokenKind::Long, &["signal", "kill-after"], value),
                (TokenKind::Short, &["p", "f", "v"], None),
                (
                    TokenKind::Long,
                    &["preserve-status", "foreground", "verbose"],
                    None,
                ),
            ],
        );
        assert_takes_value_table(
            &TIME_GRAMMAR,
            &[
                (TokenKind::Short, &["f", "o"], value),
                (TokenKind::Long, &["format", "output"], value),
                (TokenKind::Short, &["a", "v", "p", "q"], None),
                (
                    TokenKind::Long,
                    &["append", "verbose", "portability", "quiet"],
                    None,
                ),
            ],
        );
        assert_takes_value_table(
            &NICE_GRAMMAR,
            &[
                (TokenKind::Short, &["n"], value),
                (TokenKind::Long, &["adjustment"], value),
            ],
        );
        assert_takes_value_table(&Grammar::posix(&[]), &[]);
    }

    #[test]
    fn timeout_accept_table() {
        accepts("timeout 300 cargo test", "timeout 300", "cargo test");
        accepts(
            "/usr/bin/timeout 300 cargo test",
            "/usr/bin/timeout 300",
            "cargo test",
        );
        accepts(
            "timeout -k 5s 300 cargo test",
            "timeout -k 5s 300",
            "cargo test",
        );
        accepts(
            "timeout -k5s 300 cargo test",
            "timeout -k5s 300",
            "cargo test",
        );
        accepts(
            "timeout --kill-after=5s 300 cargo test",
            "timeout --kill-after=5s 300",
            "cargo test",
        );
        accepts(
            "timeout --preserve-status 300 cargo test",
            "timeout --preserve-status 300",
            "cargo test",
        );
        accepts("timeout -p 300 cargo test", "timeout -p 300", "cargo test");
        accepts("timeout -f 300 cargo test", "timeout -f 300", "cargo test");
        accepts("timeout -- 300 cargo test", "timeout -- 300", "cargo test");
        accepts(
            "timeout -vk 5 300 cargo test",
            "timeout -vk 5 300",
            "cargo test",
        );
        accepts(
            "timeout -vs9 300 cargo test",
            "timeout -vs9 300",
            "cargo test",
        );
        accepts(
            "timeout -pf 300 cargo test",
            "timeout -pf 300",
            "cargo test",
        );
        // #2375 regression guard.
        accepts(
            "timeout 30 gh run view 123 --log-failed",
            "timeout 30",
            "gh run view 123 --log-failed",
        );
        // An unbraced `$NAME` glued to the operand is one word, which fills
        // the operand whole.
        accepts("timeout 5$UNIT git status", "timeout 5$UNIT", "git status");
        // After the duration, timeout's own options are over: the next word
        // is the program it runs.
        accepts(
            "timeout 300 -s KILL git status",
            "timeout 300",
            "-s KILL git status",
        );
        accepts("timeout 300 -- cargo test", "timeout 300", "-- cargo test");
    }

    #[test]
    fn timeout_reject_table() {
        rejects("timeout --unknown 300 cargo test");
        // A braced `${T}` starts with shell syntax, so no word after
        // `timeout` is a plain argument.
        rejects("timeout ${T}s git status");
        rejects("timeout 300");
        rejects("timeout -k 5s");
        rejects("stdbuf -oL cargo test");
        rejects("timeout -5 cargo test");
        // Exact long names only.
        rejects("timeout --v 300 cargo test");
        rejects("timeout --s 9 300 cargo test");
        rejects("timeout --k 5 300 cargo test");
        // A boolean long flag takes no value.
        rejects("timeout --foreground=x 300 cargo test");
        rejects("timeout --verbose=1 300 cargo test");
    }

    /// The head names the program bash runs: with an expansion in it, that is
    /// whatever the expansion holds, whatever its basename reads.
    #[test]
    fn a_head_with_an_expansion_is_no_wrapper() {
        for cmd in [
            "$D/timeout 300 git status",
            "${D}/timeout 300 git status",
            "*/timeout 300 git status",
            "$D/usr/bin/time git status",
            "$HOME/bin/nice -n 5 git status",
        ] {
            rejects(cmd);
        }
    }

    #[test]
    fn time_gnu_accept_table() {
        accepts("/usr/bin/time cargo build", "/usr/bin/time", "cargo build");
        accepts(
            "/usr/bin/time -p cargo build",
            "/usr/bin/time -p",
            "cargo build",
        );
        accepts(
            "/usr/bin/time -f %e cargo build",
            "/usr/bin/time -f %e",
            "cargo build",
        );
        accepts(
            "/usr/bin/time -a -o log git status",
            "/usr/bin/time -a -o log",
            "git status",
        );
        accepts(
            "/usr/bin/time --output=log git status",
            "/usr/bin/time --output=log",
            "git status",
        );
        accepts(
            "/usr/bin/time --format %e git status",
            "/usr/bin/time --format %e",
            "git status",
        );
        accepts(
            "/usr/bin/time -q git status",
            "/usr/bin/time -q",
            "git status",
        );
        // A short cluster reads letter by letter.
        accepts(
            "/usr/bin/time -pv cargo build",
            "/usr/bin/time -pv",
            "cargo build",
        );
        accepts(
            "/usr/bin/time -pf %e cargo build",
            "/usr/bin/time -pf %e",
            "cargo build",
        );
        accepts(
            "/usr/bin/time -vo out.txt cargo build",
            "/usr/bin/time -vo out.txt",
            "cargo build",
        );
        // Bare, where no pipeline starts.
        accepts_at(
            "time -v cargo build",
            Position::INNER,
            "time -v",
            "cargo build",
        );
        accepts_at(
            "time -f %e cargo build",
            Position::STAGE,
            "time -f %e",
            "cargo build",
        );
    }

    #[test]
    fn time_gnu_reject_table() {
        rejects("/usr/bin/time --unknown cargo build");
        rejects("/usr/bin/time --p git status");
        rejects("/usr/bin/time --o out git status");
        rejects("/usr/bin/time --f %e git status");
        rejects("/usr/bin/time --quiet=no git status");
        // Where a pipeline starts, bare `time` is bash's reserved word.
        rejects("time -f %e git status");
        rejects("time -v git status");
    }

    #[test]
    fn nice_accept_table() {
        accepts("nice -n 10 cargo test", "nice -n 10", "cargo test");
        accepts("nice -n10 cargo test", "nice -n10", "cargo test");
        accepts("nice -10 cargo test", "nice -10", "cargo test");
        accepts(
            "nice --adjustment=5 cargo test",
            "nice --adjustment=5",
            "cargo test",
        );
        // `+5` is no option of nice's: it is the program nice runs.
        accepts("nice +5 cargo test", "nice", "+5 cargo test");
        // #2375 regression guard.
        accepts("nice -n 10 ls -la", "nice -n 10", "ls -la");
    }

    #[test]
    fn nice_reject_table() {
        rejects("nice --unknown cargo test");
        rejects("nice --n 5 cargo test");
        // Exact long names only.
        rejects("nice --a 5 cargo test");
        rejects("nice --10 cargo test");
        rejects("nice -+5 cargo test");
        // A digit inside a cluster is no adjustment.
        rejects("nice -5n 10 cargo test");
    }

    #[test]
    fn nohup_accept_table() {
        accepts("nohup cargo build", "nohup", "cargo build");
    }

    #[test]
    fn nohup_reject_table() {
        rejects("nohup");
    }

    /// A wrapper with a literal `rtk` among its own words wraps a command that
    /// runs rtk: it is no layer, in the walk and in this table alike.
    #[test]
    fn a_wrapper_holding_rtk_is_no_layer() {
        rejects("timeout -k rtk 5 git status");
        accepts("timeout -k 1 5 git status", "timeout -k 1 5", "git status");
    }

    #[test]
    fn refuses_shell_syntax() {
        for cmd in [
            "nice (cargo build)",
            "timeout 300 >out.log cargo test",
            "timeout 300 $(which cargo) test",
            "timeout 300 */bin/cargo test",
        ] {
            rejects(cmd);
        }
    }
}
