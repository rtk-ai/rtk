//! Span edits against a lexed command line, and the function that applies
//! them. A caller that lexed a line once describes its rewrite as a few
//! [`Edit`]s against that same text; every byte no edit names is emitted as
//! written.

use std::ops::Range;

/// One change to a text, positioned by byte offsets into the original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Edit {
    /// Puts `text` at `at`, removing nothing.
    Insert { at: usize, text: String },
    /// Puts `text` in place of the bytes in `span`.
    Replace { span: Range<usize>, text: String },
}

impl Edit {
    /// The bytes of the original this edit takes out, empty for an insertion.
    fn span(&self) -> Range<usize> {
        match self {
            Self::Insert { at, .. } => *at..*at,
            Self::Replace { span, .. } => span.clone(),
        }
    }

    fn text(&self) -> &str {
        match self {
            Self::Insert { text, .. } | Self::Replace { text, .. } => text,
        }
    }
}

/// `original` with `edits` applied, every byte outside them kept as written.
///
/// `edits` come in position order and do not overlap; two insertions may
/// share a position, and land in the order given. Every span must lie on
/// `char` boundaries within `original`. Anything else gives `None` rather
/// than a panic: this runs on a hook's rewrite path, where the caller falls
/// back to the command unchanged.
pub(crate) fn apply_edits(original: &str, edits: &[Edit]) -> Option<String> {
    let added: usize = edits.iter().map(|edit| edit.text().len()).sum();
    let mut out = String::with_capacity(original.len() + added);
    let mut cursor = 0;
    for edit in edits {
        let span = edit.span();
        // `get` refuses a range that is reversed, out of bounds or off a
        // `char` boundary, and `cursor..start` is reversed when this edit
        // starts inside or before the previous one.
        out.push_str(original.get(cursor..span.start)?);
        original.get(span.clone())?;
        out.push_str(edit.text());
        cursor = span.end;
    }
    out.push_str(&original[cursor..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert(at: usize, text: &str) -> Edit {
        Edit::Insert {
            at,
            text: text.into(),
        }
    }

    fn replace(span: Range<usize>, text: &str) -> Edit {
        Edit::Replace {
            span,
            text: text.into(),
        }
    }

    #[test]
    fn test_apply_edits_keeps_untouched_bytes() {
        // Two replacements inside a padded, operator-joined line: the padding
        // and the operator survive exactly as written.
        let original = "  a   &&   b  ";
        let edits = [replace(2..3, "X"), replace(11..12, "Y")];
        assert_eq!(apply_edits(original, &edits), Some("  X   &&   Y  ".into()));
    }

    #[test]
    fn test_apply_edits_with_no_edits_returns_original() {
        assert_eq!(apply_edits("git status", &[]), Some("git status".into()));
    }

    #[test]
    fn test_apply_edits_inserts_without_removing() {
        let original = "FOO=1 git status";
        assert_eq!(
            apply_edits(original, &[insert(6, "rtk ")]),
            Some("FOO=1 rtk git status".into())
        );
    }

    /// An insertion right where a replacement starts lands before it.
    #[test]
    fn test_apply_edits_inserts_before_a_replacement_at_the_same_offset() {
        let original = "cargo test";
        let edits = [insert(0, "time "), replace(0..5, "rtk cargo")];
        assert_eq!(
            apply_edits(original, &edits),
            Some("time rtk cargo test".into())
        );
    }

    /// CRLF and other bytes between two edits are gaps no edit names, so they
    /// survive exactly as written.
    #[test]
    fn test_apply_edits_preserves_crlf_gap() {
        let edits = [replace(0..1, "A"), replace(3..4, "B")];
        assert_eq!(apply_edits("a\r\nb", &edits), Some("A\r\nB".into()));
    }

    #[test]
    fn test_apply_edits_rejects_overlapping_edits() {
        let edits = [replace(0..3, "a"), replace(2..5, "b")];
        assert_eq!(apply_edits("hello world", &edits), None);
    }

    #[test]
    fn test_apply_edits_rejects_identical_duplicate_edits() {
        let edits = [replace(5..6, ""), replace(5..6, "")];
        assert_eq!(apply_edits("hello world", &edits), None);
    }

    #[test]
    fn test_apply_edits_rejects_edits_out_of_order() {
        let edits = [replace(4..5, "C"), replace(0..1, "A")];
        assert_eq!(apply_edits("a b c", &edits), None);
        let edits = [insert(4, "C"), insert(0, "A")];
        assert_eq!(apply_edits("a b c", &edits), None);
    }

    #[test]
    fn test_apply_edits_rejects_a_reversed_span() {
        #[allow(clippy::reversed_empty_ranges)]
        let edits = [replace(3..1, "x")];
        assert_eq!(apply_edits("hello", &edits), None);
    }

    #[test]
    fn test_apply_edits_rejects_a_span_past_the_end() {
        assert_eq!(apply_edits("short", &[replace(0..100, "x")]), None);
        assert_eq!(apply_edits("short", &[insert(6, "x")]), None);
    }

    /// A span landing mid-character is rejected rather than panicking the way
    /// slicing `original` directly would.
    #[test]
    fn test_apply_edits_rejects_a_non_char_boundary_span() {
        let original = "café";
        // 'é' is a two-byte UTF-8 sequence; byte 4 sits between the two.
        assert!(!original.is_char_boundary(4));
        assert_eq!(apply_edits(original, &[replace(4..5, "x")]), None);
        assert_eq!(apply_edits(original, &[insert(4, "x")]), None);
    }
}
