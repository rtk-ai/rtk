//! Stop/resume contracts, independent of a particular command's option grammar.

use super::{
    Dialect, OptionDefinition, Token, TokenAction, TokenEvent, TokenHandler, TokenKind, Tokenizer,
    ValueSpec, tokenize_grammar,
};
use std::ops::ControlFlow;

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

#[derive(Default)]
struct RecordingHandler<'a> {
    lookups: Vec<(usize, &'a str)>,
    events: Vec<TokenEvent<'a, &'static str>>,
    stop_at: Option<&'a str>,
}

impl RecordingHandler<'_> {
    fn definition(name: &str) -> Option<OptionDefinition<&'static str>> {
        Some(match name {
            "value" | "n" => OptionDefinition::value(Some("value")),
            "optional" | "o" => {
                OptionDefinition::new(Some(ValueSpec::attached_only()), Some("value"))
            }
            "flag" | "v" => OptionDefinition::flag(Some("flag")),
            "ignored" => OptionDefinition::value(None),
            _ => return None,
        })
    }
}

impl<'a> TokenHandler<'a> for RecordingHandler<'a> {
    type Meaning = &'static str;

    fn lookup(&mut self, token: &Token<'a>) -> Option<OptionDefinition<Self::Meaning>> {
        self.lookups.push((token.source_index, token.text));
        Self::definition(token.text)
    }

    fn visit(&mut self, event: TokenEvent<'a, Self::Meaning>) -> ControlFlow<()> {
        let stop = self.stop_at == Some(event.token.text);
        self.events.push(event);
        if stop {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
}

#[test]
fn visitor_receives_resolved_values_and_the_original_definition_once() {
    let args = [
        "--value=long",
        "-nshort",
        "-n=equals",
        "-n",
        "separate",
        "--ignored",
        "pause",
        "--unknown=kept",
        "free",
        "--",
        "--value",
        "forwarded",
    ];
    let mut tokenizer = Tokenizer::new(&args, Dialect::Clap);
    let mut handler = RecordingHandler {
        stop_at: Some(""), // separator
        ..Default::default()
    };
    tokenizer.visit_until(&mut handler);
    assert_eq!(
        handler.lookups,
        [
            (0, "value"),
            (1, "n"),
            (2, "n"),
            (3, "n"),
            (5, "ignored"),
            (7, "unknown")
        ]
    );
    assert_eq!(
        handler
            .events
            .iter()
            .map(TokenEvent::value)
            .collect::<Vec<_>>(),
        [
            Some("long"),
            Some("short"),
            Some("equals"),
            Some("separate"),
            Some("pause"),
            Some("kept"),
            None,
            None
        ]
    );
    let ignored = &handler.events[4];
    assert_eq!(
        ignored.definition,
        Some(OptionDefinition {
            value: Some(ValueSpec::value()),
            meaning: None,
        })
    );
    assert_eq!(ignored.separate_value.unwrap().source_index, 6);
    assert_eq!(
        handler.events[5].definition, None,
        "unknown differs from known but ignored"
    );
    for event in &handler.events {
        assert_eq!(event.token.linked, None);
        assert_eq!(event.separate_value.and_then(|token| token.linked), None);
    }

    // Resume into another grammar, retaining original argument positions.
    tokenizer.begin_region(Dialect::Clap);
    tokenizer.visit_until(&mut handler);
    assert_eq!(handler.lookups.last(), Some(&(10, "value")));
    assert_eq!(handler.events.last().unwrap().value(), Some("forwarded"));
    let event_count = handler.events.len();
    tokenizer.visit_until(&mut handler);
    assert_eq!(
        handler.events.len(),
        event_count,
        "exhaustion must not replay events"
    );
}

#[test]
fn visitor_stop_finishes_cluster_and_value_before_resuming() {
    let args = ["-vn", "data", "tail"];
    let mut tokenizer = Tokenizer::new(&args, Dialect::Clap);
    let mut handler = RecordingHandler {
        stop_at: Some("v"),
        ..Default::default()
    };
    tokenizer.visit_until(&mut handler);
    assert_eq!(handler.lookups, [(0, "v"), (0, "n")]);
    assert_eq!(handler.events.len(), 2);
    assert_eq!(handler.events[1].value(), Some("data"));
    assert_eq!(handler.events[1].separate_value.unwrap().source_index, 1);
    // The same cursor can resume through the collection API as well.
    let tail = tokenizer.tokenize_until(&mut |_| TokenAction::Continue(None));
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0].text, "tail");
    assert_eq!(tail[0].source_index, 2);
}

#[test]
fn optional_attached_values_leave_following_positionals_free() {
    let args = [
        "--optional",
        "free",
        "-o",
        "other",
        "--optional=",
        "-o=value",
    ];
    let mut tokenizer = Tokenizer::new(&args, Dialect::Clap);
    let mut handler = RecordingHandler::default();
    tokenizer.visit_until(&mut handler);
    assert_eq!(
        handler
            .events
            .iter()
            .map(|event| (event.token.text, event.value()))
            .collect::<Vec<_>>(),
        [
            ("optional", None),
            ("free", None),
            ("o", None),
            ("other", None),
            ("optional", Some("")),
            ("o", Some("value"))
        ]
    );
    assert!(handler.events[1].token.is_free_positional());
    assert!(handler.events[3].token.is_free_positional());
}

#[test]
fn completed_events_and_collected_tokens_agree_under_every_dialect() {
    let cases: &[&[&str]] = &[
        &["--value", "data", "-vn", "next", "tail"],
        &["-vn=x", "--optional=", "-20", "-é", ""],
        &["/value:out", "-n", "--", "--value", "x", "--"],
        &[
            "--ignored",
            "--flag",
            "--unknown=attached",
            "--optional",
            "free",
        ],
    ];
    for dialect in [
        Dialect::Posix,
        Dialect::Clap,
        Dialect::Msbuild,
        Dialect::CommonsCli,
        Dialect::GoFlag,
    ] {
        for args in cases {
            let tokens = tokenize_grammar(
                args,
                &|_, name| {
                    RecordingHandler::definition(name).and_then(|definition| definition.value)
                },
                dialect,
            );
            let expected: Vec<_> = tokens
                .iter()
                .filter(|token| token.kind != TokenKind::Positional || token.is_free_positional())
                .map(|token| {
                    (
                        token.kind,
                        token.text,
                        token.source_index,
                        token.value(&tokens),
                    )
                })
                .collect();
            let mut tokenizer = Tokenizer::new(args, dialect);
            let mut handler = RecordingHandler::default();
            tokenizer.visit_until(&mut handler);
            let actual: Vec<_> = handler
                .events
                .iter()
                .map(|event| {
                    (
                        event.token.kind,
                        event.token.text,
                        event.token.source_index,
                        event.value(),
                    )
                })
                .collect();
            assert_eq!(actual, expected, "{dialect:?}: {args:?}");
        }
    }
}
