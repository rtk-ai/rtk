use super::*;

fn rewritten(input: &str) -> Option<String> {
    rewrite(input, "")
}

#[test]
fn normalizes_supported_grep_flags_before_the_pattern() {
    let cases = [
        (
            "grep -n -A 40 begin{abstract} paper.tex",
            "rtk grep 'begin{abstract}' -n -A 40 paper.tex",
        ),
        (
            "grep -inA3 TODO src tests",
            "rtk grep TODO -i -n -A 3 src tests",
        ),
        (
            "grep -Er --max-count=2 TODO src",
            "rtk grep TODO -E -r --max-count=2 src",
        ),
    ];

    for (input, expected) in cases {
        assert_eq!(rewritten(input), Some(expected.to_string()), "{input}");
    }
}

#[test]
fn keeps_rg_flags_valid_before_between_and_after_operands() {
    let cases = [
        ("rg -n TODO src", "rtk rg TODO -n src"),
        ("rg TODO -n src", "rtk rg TODO -n src"),
        ("rg TODO src -n", "rtk rg TODO -n src"),
        ("rg TODO src --glob '*.rs'", "rtk rg TODO --glob '*.rs' src"),
    ];

    for (input, expected) in cases {
        assert_eq!(rewritten(input), Some(expected.to_string()), "{input}");
    }
}

#[test]
fn leaves_non_portable_grep_operand_then_flag_forms_raw() {
    for input in ["grep TODO -n src", "grep TODO src -n"] {
        assert_eq!(rewritten(input), None, "{input}");
    }
}

#[test]
fn distinguishes_engine_specific_short_flags() {
    assert_eq!(
        rewritten("rg -L -E utf-8 -r '$1' TODO src"),
        Some("rtk rg TODO --follow --encoding utf-8 --replace '$1' src".into())
    );
    assert_eq!(
        rewritten("grep -ER TODO src"),
        Some("rtk grep TODO -E -R src".into())
    );

    // grep -L/-h and other shape flags make rtk grep use native passthrough;
    // moving them after the pattern is not safe in POSIXLY_CORRECT mode.
    for input in ["grep -L TODO src", "grep -h TODO src", "grep -c TODO src"] {
        assert_eq!(rewritten(input), None, "{input}");
    }

    for input in ["rg -h", "rg -R TODO src"] {
        assert_eq!(rewritten(input), None, "{input}");
    }
}

#[test]
fn handles_attached_short_and_long_values() {
    let cases = [
        ("grep -m2 TODO src", "rtk grep TODO --max-count 2 src"),
        ("grep --context=2 TODO src", "rtk grep TODO --context=2 src"),
        ("rg -Eutf-8 TODO src", "rtk rg TODO --encoding utf-8 src"),
        ("rg TODO src -g'*.rs'", "rtk rg TODO --glob '*.rs' src"),
        ("rg --glob='*.rs' TODO src", "rtk rg TODO '--glob=*.rs' src"),
    ];

    for (input, expected) in cases {
        assert_eq!(rewritten(input), Some(expected.to_string()), "{input}");
    }
}

#[test]
fn skips_grep_context_form_that_differs_between_bsd_and_gnu() {
    assert_eq!(rewritten("grep --context 2 TODO src"), None);
    assert_eq!(
        rewritten("rg --context 2 TODO src"),
        Some("rtk rg TODO --context 2 src".into())
    );
}

#[test]
fn preserves_stdin_instead_of_inventing_a_path() {
    assert_eq!(
        rewrite("grep -in warning", " 2>&1"),
        Some("rtk grep warning -i -n 2>&1".into())
    );
    assert_eq!(
        rewritten("rg warning -g '*.log'"),
        Some("rtk rg warning --glob '*.log'".into())
    );
}

#[test]
fn preserves_multiple_paths_but_not_explicit_engine_paths() {
    assert_eq!(
        rewritten("rg TODO src tests"),
        Some("rtk rg TODO src tests".into())
    );
    assert_eq!(rewritten("/usr/bin/grep -n TODO src"), None);
    assert_eq!(rewritten("/opt/homebrew/bin/rg TODO src"), None);
}

#[test]
fn requotes_literal_shell_words_without_freezing_expansions() {
    let supported = [
        ("rg '*.rs' src", "rtk rg '*.rs' src"),
        ("rg \\*.rs src", "rtk rg '*.rs' src"),
        ("rg '$PATTERN' src", "rtk rg '$PATTERN' src"),
        (r#"rg TODO "~/literal""#, "rtk rg TODO '~/literal'"),
        (r#"rg "can't fail" src"#, r#"rtk rg 'can'\''t fail' src"#),
    ];
    for (input, expected) in supported {
        assert_eq!(rewritten(input), Some(expected.to_string()), "{input}");
    }

    for input in [
        "rg *.rs src",
        "rg {src,tests} .",
        "rg TODO ~/expanded",
        "rg \"$PATTERN\" src",
        "rg foo # comment",
        "grep foo <(generate)",
        r#"rg "a\q" src"#,
        "rg '' src",
    ] {
        assert_eq!(rewritten(input), None, "{input}");
    }
}

#[test]
fn keeps_non_expanding_regex_braces_and_trailing_redirects() {
    assert_eq!(
        rewrite("grep -n begin{abstract} paper.tex", " < input"),
        Some("rtk grep 'begin{abstract}' -n paper.tex < input".into())
    );
    assert_eq!(rewritten("grep < input TODO"), None);
}

#[test]
fn skips_metadata_and_dash_prefixed_operands() {
    for input in [
        "grep --version",
        "grep --help",
        "rg -V",
        "rg --help",
        "rg -- -pattern src",
        "grep TODO -- -path",
    ] {
        assert_eq!(rewritten(input), None, "{input}");
    }
}
