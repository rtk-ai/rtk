//! Compare the supported Clap grammar with the actual library, not a second implementation.

use super::{Dialect, TokenKind, ValueSpec, tokenize_grammar};
use clap::{Arg, ArgAction, Command};

#[test]
fn incremental_grammars_keep_links_and_source_indices_without_restarting() {
    use super::Tokenizer;
    let args = [
        "--config",
        "check",
        "install",
        "--version",
        "1.2.3",
        "--",
        "--format=json",
    ];
    let mut tokenizer = Tokenizer::new(&args, Dialect::Clap);
    let prefix = tokenizer
        .next_argument(&|_, name| (name == "config").then(ValueSpec::value))
        .unwrap();
    assert_eq!(prefix, 0..2);
    assert_eq!(
        tokenizer.tokens()[0].value(tokenizer.tokens()),
        Some("check")
    );
    let command = tokenizer.next_argument(&|_, _| None).unwrap();
    assert_eq!(tokenizer.tokens()[command.start].text, "install");
    let option = tokenizer
        .next_argument(&|_, name| (name == "version").then(ValueSpec::value))
        .unwrap();
    assert_eq!(
        tokenizer.tokens()[option.start].value(tokenizer.tokens()),
        Some("1.2.3")
    );
    assert_eq!(tokenizer.tokens()[option.start].source_index, 3);
    let boundary = tokenizer.next_argument(&|_, _| None).unwrap();
    assert_eq!(tokenizer.tokens()[boundary.start].kind, TokenKind::DashDash);
    tokenizer.begin_region(Dialect::Clap);
    let inner = tokenizer
        .next_argument(&|_, name| (name == "format").then(ValueSpec::value))
        .unwrap();
    assert_eq!(tokenizer.tokens()[inner.start].source_index, 6);
    assert_eq!(
        tokenizer.tokens()[inner.start].value(tokenizer.tokens()),
        Some("json")
    );
    assert!(
        tokenizer
            .next_argument(&|_, _| panic!("must not rescan any argument"))
            .is_none()
    );
}

fn assert_like_clap(args: &[&str], spec: ValueSpec, optional: bool) {
    let tokens = tokenize_grammar(
        args,
        &|kind, name| {
            matches!(
                (kind, name),
                (TokenKind::Long, "value") | (TokenKind::Short, "n")
            )
            .then_some(spec)
        },
        Dialect::Clap,
    );
    let mut command = Command::new("reference")
        .no_binary_name(true)
        .disable_help_flag(true)
        .disable_version_flag(true)
        .arg(
            Arg::new("value")
                .long("value")
                .short('n')
                .action(ArgAction::Append)
                .num_args(if optional { 0..=1 } else { 1..=1 })
                .allow_hyphen_values(spec.allow_hyphen_values),
        )
        .arg(Arg::new("rest").num_args(0..));
    for name in ["v", "0", "1", "2", "3", "é"] {
        command = command.arg(
            Arg::new(name)
                .short(name.chars().next().unwrap())
                .action(ArgAction::Count),
        );
    }
    let matched = command
        .try_get_matches_from(args)
        .unwrap_or_else(|error| panic!("reference rejected {args:?}: {error}"));
    let values: Vec<_> = tokens
        .iter()
        .filter(|t| matches!(t.kind, TokenKind::Long | TokenKind::Short))
        .filter(|t| matches!(t.text, "value" | "n"))
        .filter_map(|t| t.value(&tokens))
        .collect();
    let reference_values: Vec<_> = matched
        .get_many::<String>("value")
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect();
    assert_eq!(values, reference_values, "values in {args:?}");
    let rest: Vec<_> = tokens
        .iter()
        .filter(|t| t.is_free_positional())
        .map(|t| t.text)
        .collect();
    let reference_rest: Vec<_> = matched
        .get_many::<String>("rest")
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect();
    assert_eq!(rest, reference_rest, "positionals in {args:?}");
    for name in ["v", "0", "1", "2", "3", "é"] {
        let count = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Short && t.text == name)
            .count();
        assert_eq!(
            count,
            usize::from(matched.get_count(name)),
            "{name} in {args:?}"
        );
    }
}

#[test]
fn single_values_clusters_and_boundaries_match_clap() {
    let cases: &[&[&str]] = &[
        &[],
        &["plain", "", "-"],
        &["--value", "plain"],
        &["--value=plain"],
        &["--value="],
        &["--value==plain"],
        &["-nplain"],
        &["-n=plain"],
        &["-n="],
        &["-n==plain"],
        &["-vn=plain"],
        &["-vvnplain"],
        &["--value", "-"],
        &["--value=-v"],
        &["-n-v"],
        &["--value=--"],
        &["-n=--"],
        &["--", "--value", "-v", "--"],
        &["-123", "-20"],
        &["-v", "--value", "a", "tail"],
        &["--value", "a", "-n", "b"],
        &["-évn=中"],
        &["--value=é", "tail", "-v"],
    ];
    for args in cases {
        assert_like_clap(args, ValueSpec::value(), false);
    }
}

#[test]
fn optional_single_values_match_clap() {
    for args in [
        vec!["--value"],
        vec!["--value", "-v"],
        vec!["-n", "-v"],
        vec!["--value", "--", "tail"],
        vec!["--value", "plain"],
        vec!["-n="],
    ] {
        // Required and optional single values have the same token-consumption rule.
        // The tokenizer leaves missing-value validation and defaults to the caller.
        assert_like_clap(&args, ValueSpec::value(), true);
    }
}

#[test]
fn hyphen_value_override_matches_clap_including_known_flags_and_escape() {
    for args in [
        vec!["--value", "-v"],
        vec!["-n", "--value"],
        vec!["--value", "-20"],
        vec!["--value", "--", "tail"],
        vec!["--value", "--", "--", "-v"],
    ] {
        assert_like_clap(&args, ValueSpec::value().allow_hyphen_values(), false);
    }
}

#[test]
fn separate_hyphen_values_are_not_consumed_by_default() {
    for value in ["-v", "--other", "-20", "--"] {
        for flag in ["--value", "-n", "-vn"] {
            let args = [flag, value];
            let tokens = tokenize_grammar(
                &args,
                &|_, name| matches!(name, "value" | "n").then(ValueSpec::value),
                Dialect::Clap,
            );
            let option = tokens
                .iter()
                .find(|t| matches!(t.text, "value" | "n"))
                .unwrap();
            assert_eq!(option.value(&tokens), None, "{args:?}");
            assert_eq!(tokens.last().unwrap().source_index, 1, "{args:?}");
            let result = Command::new("reference")
                .no_binary_name(true)
                .arg(Arg::new("value").long("value").short('n'))
                .arg(Arg::new("verbose").short('v').action(ArgAction::Count))
                .try_get_matches_from(args);
            assert!(
                result.is_err(),
                "Clap must reject the missing value: {args:?}"
            );
        }
    }
}

#[test]
fn optional_long_values_requiring_equals_leave_following_positionals() {
    for args in [vec!["--value", "tail"], vec!["--value=given", "tail"]] {
        let tokens = tokenize_grammar(
            &args,
            &|_, name| (name == "value").then(ValueSpec::attached_only),
            Dialect::Clap,
        );
        let matched = Command::new("reference")
            .no_binary_name(true)
            .arg(
                Arg::new("value")
                    .long("value")
                    .num_args(0..=1)
                    .require_equals(true),
            )
            .arg(Arg::new("rest"))
            .try_get_matches_from(&args)
            .unwrap();
        assert_eq!(
            tokens[0].value(&tokens),
            matched.get_one::<String>("value").map(String::as_str)
        );
        assert_eq!(tokens[1].text, matched.get_one::<String>("rest").unwrap());
        assert!(tokens[1].is_free_positional());
    }
}
