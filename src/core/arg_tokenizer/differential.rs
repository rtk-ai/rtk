//! Exhaustive differential test: every arg vector up to four tokens drawn from an alphabet that
//! covers each construct the scanner branches on, tokenized by both the current scanner and the
//! [`super::frozen`] snapshot, compared token for token.
//!
//! Presets are the unit of guarantee, so this only covers `Posix` and `Msbuild` — the two that
//! existed before the axes and therefore the two that must not have moved.

use super::frozen;
use super::{
    double_dash_flag_value, has_double_dash_flag, has_flag, tokenize_grammar, Attachment, Dialect,
    Token, TokenKind, ValueSpec,
};

/// Names the lookup helpers are queried for, spanning both cases of a name in [`ALPHABET`] —
/// the only way the [`super::NameCase`] axis is observable.
const LOOKUP_NAMES: &[&str] = &["grep", "GREP", "logger", "Logger", "bl", "BL", "file", "n"];

/// Every construct `tokenize_scan` branches on, plus the shapes that have historically broken
/// one: a digit run, a bare `-`, an empty arg, a `/`-prefixed path against a `/`-prefixed
/// switch, and multi-byte chars in both a cluster and a long name.
const ALPHABET: &[&str] = &[
    "",
    "-",
    "--",
    "-n",
    "-rn",
    "-A3",
    "-20",
    "-M",
    "-file",
    "--grep",
    "--grep=x",
    "--logger:trx",
    "/bl:out.binlog",
    "/tmp/results",
    "-é",
    "--café=ü",
    "file.txt",
];

const MAX_LEN: usize = 4;

/// The three grammars worth crossing with every vector: nothing takes a value (what `tokenize`
/// does), a mixed table exercising each [`Attachment`] and `claims_dash_dash`, and everything
/// takes a value (maximum consumption, so every linking path fires).
fn specs(name: &str) -> [Option<ValueSpec>; 3] {
    let mixed = match name {
        "M" | "café" => Some(ValueSpec::attached_only()),
        "n" | "A" => Some(ValueSpec::solo_only()),
        "grep" | "logger" => Some(ValueSpec::value().claiming_dash_dash()),
        "file" | "bl" | "r" | "é" => Some(ValueSpec::value()),
        _ => None,
    };
    [None, mixed, Some(ValueSpec::value())]
}

fn frozen_spec(spec: ValueSpec) -> frozen::ValueSpec {
    frozen::ValueSpec {
        attachment: match spec.attachment {
            Attachment::AttachedOnly => frozen::Attachment::AttachedOnly,
            Attachment::AttachedOrSeparate { solo_only } => {
                frozen::Attachment::AttachedOrSeparate { solo_only }
            }
        },
        claims_dash_dash: spec.claims_dash_dash,
    }
}

fn same(new: &Token<'_>, old: &frozen::Token<'_>) -> bool {
    let kind = match old.kind {
        frozen::TokenKind::DashDash => TokenKind::DashDash,
        frozen::TokenKind::Long => TokenKind::Long,
        frozen::TokenKind::Positional => TokenKind::Positional,
        frozen::TokenKind::Short => TokenKind::Short,
    };
    new.kind == kind
        && new.text == old.text
        && new.attached == old.attached
        && new.linked == old.linked
        && new.source_index == old.source_index
        && new.double_dash == old.double_dash
        && new.slash == old.slash
}

/// Calls `body` once per arg vector of length 1..=[`MAX_LEN`] over [`ALPHABET`], and returns how
/// many there were.
fn for_each_vector(mut body: impl FnMut(&[String])) -> usize {
    let mut count = 0;
    let mut vector: Vec<String> = Vec::with_capacity(MAX_LEN);
    for len in 1..=MAX_LEN {
        let mut indices = vec![0usize; len];
        loop {
            vector.clear();
            vector.extend(indices.iter().map(|&i| ALPHABET[i].to_string()));
            body(&vector);
            count += 1;

            let mut position = len;
            while position > 0 {
                position -= 1;
                indices[position] += 1;
                if indices[position] < ALPHABET.len() {
                    break;
                }
                indices[position] = 0;
                if position == 0 {
                    break;
                }
            }
            if indices.iter().all(|&i| i == 0) {
                break;
            }
        }
    }
    count
}

fn assert_identical(dialect: Dialect, frozen_dialect: frozen::Dialect) -> usize {
    for_each_vector(|args| {
        for index in 0..3 {
            let new = tokenize_grammar(args, &|_, name| specs(name)[index], dialect);
            let old = frozen::tokenize_grammar(
                args,
                &|_, name| specs(name)[index].map(frozen_spec),
                frozen_dialect,
            );
            assert_eq!(
                new.len(),
                old.len(),
                "token count differs for {args:?} (grammar {index})\nnew: {new:?}\nold: {old:?}"
            );
            for (n, o) in new.iter().zip(old.iter()) {
                assert!(
                    same(n, o),
                    "token differs for {args:?} (grammar {index})\nnew: {n:?}\nold: {o:?}"
                );
            }

            for name in LOOKUP_NAMES {
                assert_eq!(
                    has_flag(&new, dialect, name),
                    frozen::has_flag(&old, frozen_dialect, name),
                    "has_flag({name}) differs for {args:?} (grammar {index})"
                );
                assert_eq!(
                    has_double_dash_flag(&new, dialect, name),
                    frozen::has_double_dash_flag(&old, frozen_dialect, name),
                    "has_double_dash_flag({name}) differs for {args:?} (grammar {index})"
                );
                assert_eq!(
                    double_dash_flag_value(&new, dialect, name),
                    frozen::double_dash_flag_value(&old, frozen_dialect, name),
                    "double_dash_flag_value({name}) differs for {args:?} (grammar {index})"
                );
            }
        }
    })
}

#[test]
fn posix_preset_is_token_for_token_identical_to_the_frozen_dialect() {
    let vectors = assert_identical(Dialect::Posix, frozen::Dialect::Posix);
    assert_eq!(vectors, 88_740);
}

#[test]
fn msbuild_preset_is_token_for_token_identical_to_the_frozen_dialect() {
    let vectors = assert_identical(Dialect::Msbuild, frozen::Dialect::Msbuild);
    assert_eq!(vectors, 88_740);
}

#[test]
fn tokenize_is_identical_to_the_frozen_structural_entry_point() {
    for_each_vector(|args| {
        let new = super::tokenize(args);
        let old = frozen::tokenize(args);
        assert_eq!(new.len(), old.len(), "token count differs for {args:?}");
        for (n, o) in new.iter().zip(old.iter()) {
            assert!(
                same(n, o),
                "token differs for {args:?}\nnew: {n:?}\nold: {o:?}"
            );
        }
    });
}
