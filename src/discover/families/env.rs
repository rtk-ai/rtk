//! The bare `env` word. Bash's own `NAME=value` assignment has no head word
//! (the walker reads it itself, [`peel_assignments`]), but `env` with an
//! inner command runs that command in an environment its operands set: it is
//! a run word, labelled [`LayerKind::Assignments`], which folds a run of it
//! and `NAME=value` words into one layer, in any mix (`A=1 env B=2 cmd`).
//!
//! [`peel_assignments`]: crate::core::cmdline::walker::peel_assignments

use super::{Family, LayerKind};
use crate::core::cmdline::family::Form;

pub(crate) const ENV: Family = Family {
    head: "env",
    kind: LayerKind::Assignments,
    form: Form::RunWord,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::cmdline::walker::Position;
    use crate::discover::families::{ALL, peel_text};

    fn peel(cmd: &str, pos: Position) -> Option<(String, String)> {
        peel_text(cmd, ALL, &[], pos).map(|p| {
            assert_eq!(p.kind, LayerKind::Assignments, "{cmd:?}");
            (p.prefix.to_string(), p.rest.to_string())
        })
    }

    #[test]
    fn accepts_bare_env() {
        assert_eq!(
            peel("env git status", Position::START),
            Some(("env".into(), "git status".into()))
        );
    }

    #[test]
    fn merges_with_name_value_assignments_into_one_layer() {
        assert_eq!(
            peel("A=1 env B=2 git status", Position::START),
            Some(("A=1 env B=2".into(), "git status".into()))
        );
    }

    #[test]
    fn env_ignores_command_position_but_name_equals_value_does_not() {
        // `env` is a program, found wherever it sits; a shell `NAME=value`
        // word is `env`'s own operand only once `env` has started the run.
        assert_eq!(
            peel("env A=1 git status", Position::INNER),
            Some(("env A=1".into(), "git status".into()))
        );
        assert_eq!(peel("A=1 git status", Position::INNER), None);
    }

    #[test]
    fn a_later_pipeline_stage_starts_a_simple_command() {
        assert_eq!(
            peel("A=1 git status", Position::STAGE),
            Some(("A=1".into(), "git status".into()))
        );
    }

    #[test]
    fn rejects_glued_word() {
        assert_eq!(peel("envy git status", Position::START), None);
    }

    #[test]
    fn rejects_unrelated_command() {
        assert_eq!(peel("git status", Position::START), None);
    }

    #[test]
    fn a_name_is_letters_digits_and_underscores_not_starting_with_a_digit() {
        for word in ["A=1", "_a1=x", "PATH+=:/x", "A=", "a=\"x y\""] {
            assert!(
                peel(&format!("{word} git status"), Position::START).is_some(),
                "{word}"
            );
        }
        for word in ["1a=b", "foo-bar=x", "\"foo\"=x", "=x", "A+b=1"] {
            assert!(
                peel(&format!("{word} git status"), Position::START).is_none(),
                "{word}"
            );
        }
    }

    #[test]
    fn a_quoted_value_is_one_word() {
        assert_eq!(
            peel("D='shellcheck disable=SC2034' git status", Position::START),
            Some(("D='shellcheck disable=SC2034'".into(), "git status".into()))
        );
    }

    #[test]
    fn an_escaped_trailing_blank_stays_in_the_word() {
        assert_eq!(
            peel("A=x\\  git status", Position::START),
            Some(("A=x\\ ".into(), "git status".into()))
        );
    }

    /// The assignment run bash reads at the start of a simple command, and
    /// the command behind it. A quoted value is one word however many blanks
    /// it holds. Reading it with a pattern let the value alternation
    /// backtrack into the quotes whenever the quoted form was not followed by
    /// a blank — at the end of a line, or before a `;` — so `D='# shellcheck
    /// disable=SC2034'` was read as the assignment `D='# ` and a command
    /// `shellcheck`, and the rewrite edited inside the literal (#3262).
    #[test]
    fn an_assignment_run_ends_where_bash_reads_the_command() {
        for (cmd, expected) in [
            // The three shapes of the same backtrack: before a `;`, at the end
            // of the line, either quote.
            (
                "D='# shellcheck disable=SC2034'",
                Some(("D='# shellcheck disable=SC2034'", "")),
            ),
            ("D=\"x y\"", Some(("D=\"x y\"", ""))),
            ("D='no trailing space'", Some(("D='no trailing space'", ""))),
            // And the forms that always worked, which must keep working.
            ("FOO=bar git status", Some(("FOO=bar", "git status"))),
            (
                "GIT_SSH_COMMAND='ssh -o X=no' git push",
                Some(("GIT_SSH_COMMAND='ssh -o X=no'", "git push")),
            ),
            ("env FOO=1 cargo test", Some(("env FOO=1", "cargo test"))),
            ("FOO=a\\ b git status", Some(("FOO=a\\ b", "git status"))),
            // A backslash escapes inside double quotes, not inside single ones.
            (
                "FOO=\"he said \\\"hi\\\"\" git status",
                Some(("FOO=\"he said \\\"hi\\\"\"", "git status")),
            ),
            // An assignment is bash's: any case, `+=`, and a quoted value
            // that ends the line is still one word.
            ("foo=bar git status", Some(("foo=bar", "git status"))),
            ("Foo_1=bar git status", Some(("Foo_1=bar", "git status"))),
            ("_x=1 git status", Some(("_x=1", "git status"))),
            ("foo+=x git status", Some(("foo+=x", "git status"))),
            ("a=1 B=2 git status", Some(("a=1 B=2", "git status"))),
            ("env foo=1 cargo test", Some(("env foo=1", "cargo test"))),
            (
                "d='# shellcheck disable=SC2034'",
                Some(("d='# shellcheck disable=SC2034'", "")),
            ),
            // And what bash runs as a command rather than assigning.
            ("1a=b git status", None),
            ("=x git status", None),
            ("foo-bar=x git status", None),
            ("\"foo\"=x git status", None),
            // A run with nothing after it is all run, and no command.
            ("env", Some(("env", ""))),
            ("FOO=bar", Some(("FOO=bar", ""))),
            // A word of nothing but escapes ends the run like any other word
            // that is not an assignment. Lose it and the assignment behind it
            // is swallowed too, and `BAZ=1` stops being the env var it is.
            (
                "FOO=bar \\' BAZ=1 git status",
                Some(("FOO=bar", "\\' BAZ=1 git status")),
            ),
            // `sudo` is never peeled (#146).
            ("sudo docker ps", None),
            // The run ends where its last word does, and an escaped blank
            // stays with its word.
            (" \tFOO=1  ls \t", Some((" \tFOO=1", "ls \t"))),
            ("FOO=1 cat f\\ ", Some(("FOO=1", "cat f\\ "))),
            ("FOO=a\\ ", Some(("FOO=a\\ ", ""))),
            (" \t", None),
        ] {
            let expected = expected.map(|(run, rest)| (run.to_string(), rest.to_string()));
            assert_eq!(peel(cmd, Position::START), expected, "{cmd:?}");
        }
    }

    /// `RTK_DISABLED=` is a word of the assignment run like any other, so the
    /// command behind it is the one the run leaves.
    #[test]
    fn the_bypass_marker_is_a_word_of_the_assignment_run() {
        assert_eq!(
            peel("RTK_DISABLED=1 git status", Position::START),
            Some(("RTK_DISABLED=1".into(), "git status".into()))
        );
        assert_eq!(
            peel("FOO=1 RTK_DISABLED=1 cargo test", Position::START),
            Some(("FOO=1 RTK_DISABLED=1".into(), "cargo test".into()))
        );
        assert_eq!(peel("git status", Position::START), None);
    }
}
