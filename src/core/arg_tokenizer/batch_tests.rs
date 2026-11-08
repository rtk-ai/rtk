//! Stop/resume contracts, independent of a particular command's option grammar.

use super::{Dialect, TokenAction, TokenKind, Tokenizer, ValueSpec, tokenize_grammar};

#[test]
fn batches_change_grammar_without_restarting_or_mistaking_values_for_commands() {
    let args = [
        "--config",
        "check",
        "install",
        "--version",
        "1.2.3",
        "--",
        "--format",
        "json",
        "--",
        "--help",
    ];
    let mut tokenizer = Tokenizer::new(&args, Dialect::Clap);
    let mut visited = Vec::new();
    let prefix = tokenizer.tokenize_until(&mut |token| {
        visited.push(token.source_index);
        if token.is_free_positional() {
            TokenAction::StopAfter(None)
        } else {
            TokenAction::Continue((token.text == "config").then(ValueSpec::value))
        }
    });
    assert_eq!(prefix.len(), 3);
    assert_eq!(prefix[0].value(&prefix), Some("check"));
    assert_eq!(prefix[2].text, "install");
    assert_eq!(prefix[2].source_index, 2);

    let command = tokenizer.tokenize_until(&mut |token| {
        visited.push(token.source_index);
        if token.kind == TokenKind::DashDash {
            TokenAction::StopAfter(None)
        } else {
            TokenAction::Continue((token.text == "version").then(ValueSpec::value))
        }
    });
    assert_eq!(command.len(), 3);
    assert_eq!(command[0].value(&command), Some("1.2.3"));
    assert_eq!(command[0].linked, Some(1));
    assert_eq!(command[1].linked, Some(0));
    assert_eq!(command[0].source_index, 3);
    assert_eq!(command[2].kind, TokenKind::DashDash);

    tokenizer.begin_region(Dialect::Clap);
    let harness = tokenizer.tokenize_until(&mut |token| {
        visited.push(token.source_index);
        if token.kind == TokenKind::DashDash {
            TokenAction::StopAfter(None)
        } else {
            TokenAction::Continue((token.text == "format").then(ValueSpec::value))
        }
    });
    assert_eq!(harness.len(), 3);
    assert_eq!(harness[0].source_index, 6);
    assert_eq!(harness[0].value(&harness), Some("json"));
    // An earlier batch's links remain valid after subsequent calls.
    assert_eq!(prefix[0].value(&prefix), Some("check"));

    let tail = tokenizer.tokenize_until(&mut |token| {
        visited.push(token.source_index);
        TokenAction::Continue(None)
    });
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0].kind, TokenKind::Positional);
    assert_eq!(tail[0].text, "--help");
    assert_eq!(visited, [0, 2, 3, 5, 6, 8, 9]);
    assert!(
        tokenizer
            .tokenize_until(&mut |_| panic!("must not rescan"))
            .is_empty()
    );
}

#[test]
fn stop_finishes_the_cluster_and_its_separate_value() {
    let args = ["-vn", "value", "tail"];
    let mut tokenizer = Tokenizer::new(&args, Dialect::Clap);
    let mut visited = Vec::new();
    let batch = tokenizer.tokenize_until(&mut |token| {
        visited.push(token.text);
        match token.text {
            "v" => TokenAction::StopAfter(None),
            "n" => TokenAction::Continue(Some(ValueSpec::value())),
            _ => panic!("must stop before the tail"),
        }
    });
    assert_eq!(visited, ["v", "n"]);
    assert_eq!(batch.len(), 3);
    assert_eq!(batch[1].value(&batch), Some("value"));
    assert_eq!(batch[2].source_index, 1);
    let tail = tokenizer.tokenize_until(&mut |_| TokenAction::Continue(None));
    assert_eq!(tail[0].text, "tail");
    assert_eq!(tail[0].source_index, 2);
}

#[test]
fn stop_on_an_option_consumes_its_attached_or_separate_value() {
    for args in [
        vec!["--value", "data", "tail"],
        vec!["--value=data", "tail"],
        vec!["-n", "data", "tail"],
        vec!["-n=data", "tail"],
    ] {
        let mut tokenizer = Tokenizer::new(&args, Dialect::Clap);
        let batch =
            tokenizer.tokenize_until(&mut |_| TokenAction::StopAfter(Some(ValueSpec::value())));
        assert_eq!(batch[0].value(&batch), Some("data"), "{args:?}");
        let tail = tokenizer.tokenize_until(&mut |_| TokenAction::Continue(None));
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].text, "tail");
        assert_eq!(tail[0].source_index, args.len() - 1);
    }
}

#[test]
fn a_consumed_double_dash_is_not_a_stop_event() {
    let args = ["--value", "--", "--", "-v"];
    let mut tokenizer = Tokenizer::new(&args, Dialect::Clap);
    let mut visited = Vec::new();
    let batch = tokenizer.tokenize_until(&mut |token| {
        visited.push(token.source_index);
        if token.kind == TokenKind::DashDash {
            TokenAction::StopAfter(None)
        } else {
            TokenAction::Continue(Some(ValueSpec::value().claiming_dash_dash()))
        }
    });
    assert_eq!(visited, [0, 2]);
    assert_eq!(batch[0].value(&batch), Some("--"));
    assert_eq!(batch[2].kind, TokenKind::DashDash);
    let tail = tokenizer.tokenize_until(&mut |_| TokenAction::Continue(None));
    assert_eq!(tail[0].kind, TokenKind::Positional);
    assert_eq!(tail[0].text, "-v");
}

#[test]
fn attached_atomic_flags_numeric_shortcuts_and_empty_operands_can_stop() {
    for (dialect, first) in [
        (Dialect::Clap, "--value=data"),
        (Dialect::Msbuild, "/bl:data"),
        (Dialect::Posix, "-20"),
        (Dialect::Clap, ""),
    ] {
        let args = [first, "tail"];
        let mut tokenizer = Tokenizer::new(&args, dialect);
        let batch = tokenizer.tokenize_until(&mut |_| TokenAction::StopAfter(None));
        assert_eq!(batch.len(), 1, "{first}");
        assert_eq!(batch[0].source_index, 0);
        let tail = tokenizer.tokenize_until(&mut |_| TokenAction::Continue(None));
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].text, "tail");
        assert_eq!(tail[0].source_index, 1);
    }
}

#[test]
fn pauses_preserve_tokenization_under_every_dialect() {
    let cases: &[&[&str]] = &[
        &["--value", "x", "-vn", "y", "tail"],
        &["-vn=x", "--value=", "-20", "-é", ""],
        &["/bl:out.binlog", "-n", "--", "--value", "x", "--"],
        &["--value", "--", "--", "-n", "value"],
    ];
    for dialect in [
        Dialect::Posix,
        Dialect::Clap,
        Dialect::Msbuild,
        Dialect::CommonsCli,
        Dialect::GoFlag,
    ] {
        for args in cases {
            for spec in [
                ValueSpec::value(),
                ValueSpec::attached_only(),
                ValueSpec::solo_only().claiming_dash_dash(),
            ] {
                let takes_value = |kind, name: &str| {
                    (matches!(kind, TokenKind::Long | TokenKind::Short)
                        && matches!(name, "value" | "n" | "vn"))
                    .then_some(spec)
                };
                let expected = tokenize_grammar(args, &takes_value, dialect);
                let mut tokenizer = Tokenizer::new(args, dialect);
                let mut actual = Vec::new();
                loop {
                    let mut batch = tokenizer.tokenize_until(&mut |token| {
                        TokenAction::StopAfter(takes_value(token.kind, token.text))
                    });
                    if batch.is_empty() {
                        break;
                    }
                    let offset = actual.len();
                    for token in &mut batch {
                        token.linked = token.linked.map(|index| index + offset);
                    }
                    actual.extend(batch);
                }
                assert_eq!(actual, expected, "{dialect:?}: {args:?}: {spec:?}");
            }
        }
    }
}
