//! Words that run their argument as the command it names, without changing
//! which one runs: `command`, `builtin`, `exec`, and zsh's `noglob` and
//! `nocorrect`. None is a program on its own (`rtk exec date` fails), so a
//! command behind one that has no rewrite of its own leaves the whole command
//! as written ([`LayerKind::Precommand`]).
//!
//! The rewrite reads none of their options: `command -p git status` is left
//! as written, its command word `-p` being none.

use super::{Family, LayerKind};
use crate::core::cmdline::family::Form;

const fn precommand(head: &'static str) -> Family {
    Family {
        head,
        kind: LayerKind::Precommand,
        form: Form::Keyword,
    }
}

pub(crate) const NOGLOB: Family = precommand("noglob");
pub(crate) const COMMAND: Family = precommand("command");
pub(crate) const BUILTIN: Family = precommand("builtin");
pub(crate) const EXEC: Family = precommand("exec");
pub(crate) const NOCORRECT: Family = precommand("nocorrect");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::cmdline::walker::Position;
    use crate::discover::families::{ALL, peel_text};

    #[test]
    fn accepts_each_precommand_word() {
        for (cmd, head) in [
            ("noglob git status", "noglob"),
            ("command git status", "command"),
            ("builtin git status", "builtin"),
            ("exec git status", "exec"),
            ("nocorrect git status", "nocorrect"),
        ] {
            let peel = peel_text(cmd, ALL, &[], Position::START)
                .unwrap_or_else(|| panic!("no peel for {cmd}"));
            assert_eq!(peel.kind, LayerKind::Precommand);
            assert_eq!(peel.prefix, head);
            assert_eq!(peel.rest, "git status");
            assert_eq!(peel.next, Position::INNER);
        }
    }

    #[test]
    fn rejects_glued_word() {
        assert!(peel_text("noglobber git status", ALL, &[], Position::START).is_none());
    }

    #[test]
    fn rejects_unrelated_command() {
        assert!(peel_text("git status", ALL, &[], Position::START).is_none());
    }
}
