//! `uv run` is a wrapper the rules also match whole, so when the command it
//! wraps has no rewrite of its own, the rewrite falls through to `uv run`'s
//! rule ([`LayerKind::RoutableWrapper`]). It is matched as a bare keyword: an
//! option between `uv run` and the command stops the match.

use super::{Family, LayerKind};
use crate::core::cmdline::family::Form;

pub(crate) const UV_RUN: Family = Family {
    head: "uv run",
    kind: LayerKind::RoutableWrapper,
    form: Form::Keyword,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::cmdline::walker::Position;
    use crate::discover::families::{ALL, peel_text};

    #[test]
    fn accepts_uv_run() {
        let peel = peel_text("uv run pytest", ALL, &[], Position::START).expect("uv run");
        assert_eq!(peel.kind, LayerKind::RoutableWrapper);
        assert_eq!(peel.prefix, "uv run");
        assert_eq!(peel.rest, "pytest");
        assert_eq!(peel.next, Position::INNER);
    }

    /// `uv` alone, a glued word, and words a non-breaking space joins.
    #[test]
    fn rejects_anything_but_the_two_whole_words() {
        for cmd in [
            "uv pip list",
            "uv runner pytest",
            "uv run\u{a0}pytest",
            "uv\u{a0}run pytest",
        ] {
            assert!(
                peel_text(cmd, ALL, &[], Position::START).is_none(),
                "{cmd:?}"
            );
        }
    }

    #[test]
    fn the_two_words_are_written_with_blanks_between() {
        for text in ["uv  run pytest", "uv\trun pytest"] {
            let peel = peel_text(text, ALL, &[], Position::START).expect(text);
            assert_eq!(peel.kind, LayerKind::RoutableWrapper, "{text:?}");
            assert_eq!(peel.rest, "pytest", "{text:?}");
        }
    }
}
