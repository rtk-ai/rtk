//! Stop/resume contracts, independent of a particular command's option grammar.

use super::{
    Dialect, ParameterParser, ParseBoundary, Token, TokenAction, TokenKind, Tokenizer, ValueSpec,
    syntax_callback, tokenize_grammar,
};

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
    let prefix = tokenizer.tokenize_while(&mut syntax_callback(|token| {
        visited.push(token.source_index);
        if token.is_free_positional() {
            (TokenAction::Stop, None)
        } else {
            (
                TokenAction::Continue,
                (token.text == "config").then(ValueSpec::value),
            )
        }
    }));
    assert_eq!(prefix.len(), 3);
    assert_eq!(prefix[0].value(&prefix), Some("check"));
    assert_eq!(prefix[2].text, "install");
    assert_eq!(prefix[2].source_index, 2);

    let command = tokenizer.tokenize_while(&mut syntax_callback(|token| {
        visited.push(token.source_index);
        if token.kind == TokenKind::DashDash {
            (TokenAction::Stop, None)
        } else {
            (
                TokenAction::Continue,
                (token.text == "version").then(ValueSpec::value),
            )
        }
    }));
    assert_eq!(command.len(), 3);
    assert_eq!(command[0].value(&command), Some("1.2.3"));
    assert_eq!(command[0].linked, Some(1));
    assert_eq!(command[1].linked, Some(0));
    assert_eq!(command[0].source_index, 3);
    assert_eq!(command[2].kind, TokenKind::DashDash);

    tokenizer.begin_region(Dialect::Clap);
    let harness = tokenizer.tokenize_while(&mut syntax_callback(|token| {
        visited.push(token.source_index);
        if token.kind == TokenKind::DashDash {
            (TokenAction::Stop, None)
        } else {
            (
                TokenAction::Continue,
                (token.text == "format").then(ValueSpec::value),
            )
        }
    }));
    assert_eq!(harness.len(), 3);
    assert_eq!(harness[0].source_index, 6);
    assert_eq!(harness[0].value(&harness), Some("json"));
    // An earlier batch's links remain valid after subsequent calls.
    assert_eq!(prefix[0].value(&prefix), Some("check"));

    let tail = tokenizer.tokenize_while(&mut syntax_callback(|token| {
        visited.push(token.source_index);
        (TokenAction::Continue, None)
    }));
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0].kind, TokenKind::Positional);
    assert_eq!(tail[0].text, "--help");
    assert_eq!(visited, [0, 2, 3, 5, 6, 8, 9]);
    assert!(
        tokenizer
            .tokenize_while(&mut syntax_callback(|_| panic!("must not rescan")))
            .is_empty()
    );
}

#[test]
fn stop_finishes_the_cluster_and_its_separate_value() {
    let args = ["-vn", "value", "tail"];
    let mut tokenizer = Tokenizer::new(&args, Dialect::Clap);
    let mut visited = Vec::new();
    let batch = tokenizer.tokenize_while(&mut syntax_callback(|token| {
        visited.push(token.text);
        match token.text {
            "v" => (TokenAction::Stop, None),
            "n" => (TokenAction::Continue, Some(ValueSpec::value())),
            _ => panic!("must stop before the tail"),
        }
    }));
    assert_eq!(visited, ["v", "n"]);
    assert_eq!(batch.len(), 3);
    assert_eq!(batch[1].value(&batch), Some("value"));
    assert_eq!(batch[2].source_index, 1);
    let tail = tokenizer.tokenize_while(&mut syntax_callback(|_| (TokenAction::Continue, None)));
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
        let batch = tokenizer.tokenize_while(&mut syntax_callback(|_| {
            (TokenAction::Stop, Some(ValueSpec::value()))
        }));
        assert_eq!(batch[0].value(&batch), Some("data"), "{args:?}");
        let tail =
            tokenizer.tokenize_while(&mut syntax_callback(|_| (TokenAction::Continue, None)));
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
    let batch = tokenizer.tokenize_while(&mut syntax_callback(|token| {
        visited.push(token.source_index);
        if token.kind == TokenKind::DashDash {
            (TokenAction::Stop, None)
        } else {
            (
                TokenAction::Continue,
                Some(ValueSpec::value().claiming_dash_dash()),
            )
        }
    }));
    assert_eq!(visited, [0, 2]);
    assert_eq!(batch[0].value(&batch), Some("--"));
    assert_eq!(batch[2].kind, TokenKind::DashDash);
    let tail = tokenizer.tokenize_while(&mut syntax_callback(|_| (TokenAction::Continue, None)));
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
        let batch = tokenizer.tokenize_while(&mut syntax_callback(|_| (TokenAction::Stop, None)));
        assert_eq!(batch.len(), 1, "{first}");
        assert_eq!(batch[0].source_index, 0);
        let tail =
            tokenizer.tokenize_while(&mut syntax_callback(|_| (TokenAction::Continue, None)));
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
                    let mut batch = tokenizer.tokenize_while(&mut syntax_callback(|token| {
                        (TokenAction::Stop, takes_value(token.kind, token.text))
                    }));
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

#[test]
fn parser_objects_read_resolved_values_in_the_token_loop() {
    #[derive(Clone, Default)]
    struct Parameters<'a> {
        flags: usize,
        values: Vec<&'a str>,
    }

    impl<'a> ParameterParser<'a> for Parameters<'a> {
        fn parse_parameter(
            &mut self,
            token: &Token<'a>,
            value: Option<&'a str>,
        ) -> Option<(TokenAction, Option<ValueSpec>)> {
            match (token.kind, token.text) {
                (TokenKind::Long, "value") | (TokenKind::Short, "n") => {
                    if let Some(value) = value {
                        self.values.push(value);
                    }
                    Some((TokenAction::Continue, Some(ValueSpec::value())))
                }
                (TokenKind::Long, "flag") | (TokenKind::Short, "v") => {
                    self.flags += 1;
                    Some((TokenAction::Continue, None))
                }
                (TokenKind::DashDash, _) => Some((TokenAction::Stop, None)),
                _ => None,
            }
        }
    }

    let args = [
        "--unknown",
        "operand",
        "--value=long",
        "-vn=short",
        "--value",
        "separate",
        "--",
        "--flag",
    ];
    let mut tokenizer = Tokenizer::new(&args, Dialect::Clap);
    let mut destination = Parameters::default();
    let mut scratch = destination.clone();
    let tokens = tokenizer.tokenize_while(&mut scratch);
    assert_eq!(scratch.flags, 1);
    assert!(
        scratch.values.is_empty(),
        "tokenization always supplies None"
    );
    assert_eq!(destination.flags, 0);
    assert!(
        tokens[1].is_free_positional(),
        "unrecognized options consume no value"
    );
    assert_eq!(tokens.last().unwrap().kind, TokenKind::DashDash);

    for token in &tokens {
        if token.kind == TokenKind::Positional && !token.is_free_positional() {
            continue;
        }
        let _ = destination.parse_parameter(token, token.value(&tokens));
    }
    assert_eq!(destination.flags, 1);
    assert_eq!(destination.values, ["long", "short", "separate"]);

    // A trait object can also resume the same tokenizer with its -- state intact.
    let parser: &mut dyn ParameterParser<'_> = &mut ();
    let tail = tokenizer.tokenize_while(parser);
    assert_eq!(tail.len(), 1);
    assert!(tail[0].is_free_positional());
    assert_eq!(tail[0].text, "--flag");
    assert_eq!(tail[0].source_index, 7);
}

#[derive(Clone, Default)]
struct ParsedArguments<'a> {
    flags: Vec<&'a str>,
    values: Vec<&'a str>,
    positionals: Vec<&'a str>,
}

impl<'a> ParameterParser<'a> for ParsedArguments<'a> {
    fn parse_parameter(
        &mut self,
        token: &Token<'a>,
        value: Option<&'a str>,
    ) -> Option<(TokenAction, Option<ValueSpec>)> {
        match (token.kind, token.text) {
            (TokenKind::Long, "value") | (TokenKind::Short, "n") => {
                if let Some(value) = value {
                    self.values.push(value);
                }
                let action = if value == Some("stop") {
                    TokenAction::Stop
                } else {
                    TokenAction::Continue
                };
                Some((action, Some(ValueSpec::value())))
            }
            (TokenKind::Long, "flag") | (TokenKind::Short, "v") => {
                self.flags.push(token.text);
                let action = if token.text == "v" {
                    TokenAction::Stop
                } else {
                    TokenAction::Continue
                };
                Some((action, None))
            }
            (TokenKind::Positional, _) => {
                self.positionals.push(token.text);
                Some((TokenAction::Stop, None))
            }
            (TokenKind::DashDash, _) => Some((TokenAction::Stop, None)),
            _ => None,
        }
    }
}

#[test]
fn parse_while_resolves_values_once_and_preserves_boundaries() {
    let args = ["--value", "operand", "command", "--flag", "--", "--flag"];
    let mut tokenizer = Tokenizer::new(&args, Dialect::Clap);
    let mut parsed = ParsedArguments::default();
    assert_eq!(
        tokenizer.parse_while(&mut parsed),
        ParseBoundary::Positional("command")
    );
    assert_eq!(parsed.values, ["operand"]);
    assert_eq!(parsed.positionals, ["command"]);
    assert!(parsed.flags.is_empty());
    assert_eq!(tokenizer.parse_while(&mut parsed), ParseBoundary::Stopped);
    assert_eq!(parsed.flags, ["flag"]);
    assert_eq!(
        tokenizer.parse_while(&mut parsed),
        ParseBoundary::Positional("--flag")
    );
    assert_eq!(parsed.positionals, ["command", "--flag"]);
    assert_eq!(parsed.flags, ["flag"]);
    assert_eq!(
        tokenizer.parse_while(&mut parsed),
        ParseBoundary::EndOfInput
    );
}

#[test]
fn parse_while_finishes_semantic_updates_for_the_cluster_and_its_value() {
    let args = ["-vn", "data", "tail"];
    let mut tokenizer = Tokenizer::new(&args, Dialect::Clap);
    let mut parsed = ParsedArguments::default();
    assert_eq!(tokenizer.parse_while(&mut parsed), ParseBoundary::Stopped);
    assert_eq!(parsed.flags, ["v"]);
    assert_eq!(parsed.values, ["data"]);
    assert_eq!(
        tokenizer.parse_while(&mut parsed),
        ParseBoundary::Positional("tail")
    );
}

#[test]
fn parse_while_value_dependent_stops_leave_the_next_separator_unread() {
    for args in [
        vec!["--value", "stop", "--", "--flag"],
        vec!["--value=stop", "--", "--flag"],
        vec!["-n", "stop", "--", "--flag"],
        vec!["-nstop", "--", "--flag"],
    ] {
        let mut tokenizer = Tokenizer::new(&args, Dialect::Clap);
        let mut parsed = ParsedArguments::default();
        assert_eq!(
            tokenizer.parse_while(&mut parsed),
            ParseBoundary::Stopped,
            "{args:?}"
        );
        assert_eq!(parsed.values, ["stop"]);
        assert_eq!(
            tokenizer.parse_while(&mut parsed),
            ParseBoundary::Stopped,
            "{args:?}"
        );
        assert_eq!(
            tokenizer.parse_while(&mut parsed),
            ParseBoundary::Positional("--flag"),
            "{args:?}"
        );
        assert!(parsed.flags.is_empty());
    }
}

#[test]
fn parse_while_uses_resolved_state_to_tokenize_subsequent_arguments() {
    #[derive(Clone, Default)]
    struct ValueMode<'a> {
        takes_value: bool,
        captured: Option<&'a str>,
    }
    impl<'a> ParameterParser<'a> for ValueMode<'a> {
        fn parse_parameter(
            &mut self,
            token: &Token<'a>,
            value: Option<&'a str>,
        ) -> Option<(TokenAction, Option<ValueSpec>)> {
            match (token.kind, token.text) {
                (TokenKind::Long, "mode") => {
                    self.takes_value = value == Some("value");
                    Some((TokenAction::Continue, Some(ValueSpec::value())))
                }
                (TokenKind::Long, "option") => {
                    self.captured = value;
                    Some((
                        TokenAction::Continue,
                        self.takes_value.then(ValueSpec::value),
                    ))
                }
                (TokenKind::Positional, _) => Some((TokenAction::Stop, None)),
                _ => None,
            }
        }
    }
    let args = ["--mode", "value", "--option", "operand", "command"];
    let mut tokenizer = Tokenizer::new(&args, Dialect::Clap);
    let mut parsed = ValueMode::default();
    assert_eq!(
        tokenizer.parse_while(&mut parsed),
        ParseBoundary::Positional("command")
    );
    assert_eq!(parsed.captured, Some("operand"));
}
