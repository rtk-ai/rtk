//! `stdbuf` and a bare `xargs`: wrappers a deny or ask rule looks past
//! ([`super::MATCHING`]) but the rewrite never goes through, so they are not
//! in [`super::ALL`].
//!
//! `stdbuf` exists to make the wrapped command emit output incrementally, and
//! routing through rtk buffers that output until the child exits; but
//! `stdbuf -oL git push` is still `git push` to a deny rule. A bare `xargs`
//! runs the command it is given, once per input item, which is why
//! `xargs rtk git status` would be wrong; the option-less form is the one
//! Claude Code matches past. Its grammar declares no option, so any option
//! (`xargs -I{} …`) stops the match.

use super::{Family, LayerKind};
use crate::core::arg_tokenizer::{Dialect, Flag, Grammar, ValueSpec};
use crate::core::cmdline::family::{Form, WrapperSpec};

const fn match_only(head: &'static str, grammar: Grammar) -> Family {
    Family {
        head,
        kind: LayerKind::ProcessWrapper,
        form: Form::Wrapper(WrapperSpec {
            operands: 0,
            grammar,
        }),
    }
}

/// GNU `stdbuf OPTION... COMMAND`.
const STDBUF_GRAMMAR: Grammar = Grammar::new(
    Dialect::Posix,
    &[&[
        Flag::pair("i", "input").takes(ValueSpec::value()),
        Flag::pair("o", "output").takes(ValueSpec::value()),
        Flag::pair("e", "error").takes(ValueSpec::value()),
    ]],
);

pub(crate) const STDBUF: Family = match_only("stdbuf", STDBUF_GRAMMAR);

pub(crate) const XARGS: Family = match_only("xargs", Grammar::new(Dialect::Posix, &[]));

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::arg_tokenizer::{TokenKind, assert_takes_value_table};
    use crate::core::cmdline::walker::Position;
    use crate::discover::families::{ALL, MATCHING, peel_text};

    fn accepts(cmd: &str, prefix: &str, rest: &str) {
        let peel = peel_text(cmd, MATCHING, &[], Position::START)
            .unwrap_or_else(|| panic!("expected a peel for {cmd:?}"));
        assert_eq!((peel.prefix, peel.rest), (prefix, rest), "{cmd:?}");
        assert!(
            peel_text(cmd, ALL, &[], Position::START).is_none(),
            "the rewrite must not peel {cmd:?}"
        );
    }

    fn rejects(cmd: &str) {
        assert!(
            peel_text(cmd, MATCHING, &[], Position::START).is_none(),
            "expected no peel for {cmd:?}"
        );
    }

    #[test]
    fn grammars() {
        let value = Some(ValueSpec::value());
        assert_takes_value_table(
            &STDBUF_GRAMMAR,
            &[
                (TokenKind::Short, &["i", "o", "e"], value),
                (TokenKind::Long, &["input", "output", "error"], value),
            ],
        );
        assert_takes_value_table(&Grammar::new(Dialect::Posix, &[]), &[]);
    }

    #[test]
    fn accept_table() {
        accepts("stdbuf -oL git push", "stdbuf -oL", "git push");
        accepts("stdbuf -o L git push", "stdbuf -o L", "git push");
        accepts(
            "stdbuf --output=L -e0 git push",
            "stdbuf --output=L -e0",
            "git push",
        );
        accepts(
            "/usr/bin/stdbuf -i0 git push",
            "/usr/bin/stdbuf -i0",
            "git push",
        );
        accepts("stdbuf git push", "stdbuf", "git push");
        accepts("xargs git push", "xargs", "git push");
    }

    #[test]
    fn reject_table() {
        rejects("stdbuf --unknown git push");
        rejects("stdbuf -oL");
        rejects("xargs -I{} git push {}");
        rejects("xargs -0 rm");
        rejects("xargs");
    }
}
