//! perldoc filter: a light trim of rendered documentation.
//!
//! Documentation is what an agent reads to learn an API, so almost all of it stays. rtk asks for
//! plain text (`-T`, no pager), drops the sections about people and legal terms (AUTHOR,
//! MAINTAINERS, COPYRIGHT, LICENSE, BUGS, SUPPORT, SOURCE, HISTORY, ACKNOWLEDGEMENTS, ...),
//! removes the four-space indent every body line carries, and squeezes runs of blank lines.
//! SEE ALSO stays: it points to where the answer might be. Modes that print something other
//! than rendered POD (`-l` path, `-m`/`-u` source, `-h`, `-V`, `-d` to a file) pass through.

use regex::Regex;
use std::sync::LazyLock;

use super::utils::unfiltered;
use crate::core::arg_tokenizer::{self, Dialect, TokenKind, ValueSpec};
use crate::core::runner;
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::Result;

/// A top-level `=head1`, which perldoc renders in column 0.
static HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][A-Za-z0-9 ,&'/-]*$").unwrap());
static DROPPED_HEADING_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^(?:AUTHORS?|MAINTAINERS?|CONTRIBUTORS|COPYRIGHT|LICEN[CS]E|BUGS|SUPPORT|SOURCE|",
        r"HISTORY|ACKNOWLEDGE?MENTS|THANKS|CREDITS|DONATIONS)\b"
    ))
    .unwrap()
});

/// perldoc's value-taking switches, from `perldoc -h`.
fn perldoc_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    match kind {
        TokenKind::Short => {
            matches!(name, "o" | "M" | "w" | "L" | "d" | "f" | "q" | "v").then(ValueSpec::value)
        }
        _ => None,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    /// Rendered documentation; `add_text` is whether rtk must add `-T`.
    Rendered {
        add_text: bool,
    },
    Raw,
}

fn mode(args: &[String]) -> Mode {
    let tokens = arg_tokenizer::tokenize_grammar(args, &perldoc_takes_value, Dialect::Posix);
    let mut add_text = true;
    for t in arg_tokenizer::before_dashdash(&tokens) {
        if t.kind != TokenKind::Short {
            continue;
        }
        match t.text {
            "l" | "m" | "u" | "h" | "V" | "d" | "o" | "M" => return Mode::Raw,
            "T" | "t" => add_text = false,
            _ => {}
        }
    }
    Mode::Rendered { add_text }
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// A document with no column-0 headings (`-f`, `-q`, `-v`) is a list of entries: the lines at
/// the shallowest indent head an entry (a function's signatures), and everything under one
/// moves left to sit two spaces below it, keeping its own relative indentation.
fn dedent_entries(lines: &[&str], top: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if !line.is_empty() && indent(line) <= top {
            out.push(line.trim_start().to_string());
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && (lines[i].is_empty() || indent(lines[i]) > top) {
            i += 1;
        }
        let body = &lines[start..i];
        let cut = body
            .iter()
            .filter(|l| !l.is_empty())
            .map(|l| indent(l))
            .min()
            .unwrap_or(0)
            .saturating_sub(2);
        out.extend(body.iter().map(|l| l.get(cut..).unwrap_or("").to_string()));
    }
    out
}

pub fn filter_perldoc(raw: &str) -> String {
    let clean = strip_ansi(raw);
    let mut kept: Vec<&str> = Vec::new();
    let mut dropping = false;

    for line in clean.lines() {
        let line = line.trim_end();
        if HEADING_RE.is_match(line) {
            dropping = DROPPED_HEADING_RE.is_match(line);
        }
        if dropping {
            continue;
        }
        if line.is_empty() && kept.last().is_none_or(|l| l.is_empty()) {
            continue;
        }
        kept.push(line);
    }
    while kept.last().is_some_and(|l| l.is_empty()) {
        kept.pop();
    }

    let top = kept
        .iter()
        .filter(|l| !l.is_empty())
        .map(|l| indent(l))
        .min()
        .unwrap_or(0);
    let out: Vec<String> = if top == 0 {
        // A module's page: head1 in column 0, head2 at 2, body at 4. Moving the body left by
        // four keeps it under its head2 and removes the most bytes.
        kept.iter()
            .map(|l| l.strip_prefix("    ").unwrap_or(l).to_string())
            .collect()
    } else {
        dedent_entries(&kept, top)
    };
    out.join("\n")
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("perldoc");
    let mode = mode(args);
    if mode == (Mode::Rendered { add_text: true }) {
        cmd.arg("-T");
    }
    cmd.args(args);

    if verbose > 0 {
        eprintln!("Running: perldoc {} ({:?})", args.join(" "), mode);
    }

    let filter: fn(&str) -> String = match mode {
        Mode::Rendered { .. } => filter_perldoc,
        Mode::Raw => unfiltered,
    };

    runner::run_filtered(
        cmd,
        "perldoc",
        &args.join(" "),
        filter,
        runner::RunOptions::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn savings(input: &str, output: &str) -> f64 {
        100.0 - (output.len() as f64 / input.len() as f64 * 100.0)
    }

    #[test]
    fn test_drops_people_and_legal_sections() {
        let input = include_str!("../../../tests/fixtures/perl/perldoc_module_raw.txt");
        let output = filter_perldoc(input);
        for gone in [
            "\nAUTHORS",
            "\nMAINTAINERS",
            "\nBUGS",
            "\nSOURCE",
            "\nCOPYRIGHT",
            "\nHISTORY",
        ] {
            assert!(!output.contains(gone), "{} still present", gone.trim());
        }
        for kept in [
            "NAME",
            "\nSYNOPSIS",
            "\nDESCRIPTION",
            "\nSEE ALSO",
            "\nCAVEATS and NOTES",
        ] {
            assert!(output.contains(kept), "{} missing", kept.trim());
        }
        assert!(output.contains("\n  use Test::More tests => 23;"));
    }

    #[test]
    fn test_body_indent_removed_relative_indent_kept() {
        let input = "NAME\n    Foo - bar\n\nSYNOPSIS\n      use Foo;\n\n\n\n    Text.\n";
        assert_eq!(
            filter_perldoc(input),
            "NAME\nFoo - bar\n\nSYNOPSIS\n  use Foo;\n\nText."
        );
    }

    #[test]
    fn test_function_doc_body_sits_two_under_its_signature() {
        let input = "    open FILEHANDLE,EXPR\n    open FILEHANDLE\n            Associates a handle.\n\n                open(my $fh, \"<\", $path);\n";
        assert_eq!(
            filter_perldoc(input),
            "open FILEHANDLE,EXPR\nopen FILEHANDLE\n  Associates a handle.\n\n      open(my $fh, \"<\", $path);"
        );
    }

    #[test]
    fn test_function_doc_keeps_everything() {
        let input = include_str!("../../../tests/fixtures/perl/perldoc_func_raw.txt");
        let output = filter_perldoc(input);
        let words = |s: &str| s.split_whitespace().count();
        assert_eq!(words(&output), words(input));
    }

    #[test]
    fn test_module_savings() {
        // perldoc is exempt from the 60% bar: the documentation is the payload.
        let input = include_str!("../../../tests/fixtures/perl/perldoc_module_raw.txt");
        let pct = savings(input, &filter_perldoc(input));
        assert!(pct >= 10.0, "expected >=10% savings, got {:.1}%", pct);
    }

    #[test]
    fn test_not_found_passes_through() {
        let input = include_str!("../../../tests/fixtures/perl/perldoc_missing_raw.txt");
        assert_eq!(filter_perldoc(input), input.trim_end());
    }

    #[test]
    fn test_mode() {
        assert_eq!(
            mode(&args(&["Test::More"])),
            Mode::Rendered { add_text: true }
        );
        assert_eq!(
            mode(&args(&["-T", "-f", "open"])),
            Mode::Rendered { add_text: false }
        );
        assert_eq!(mode(&args(&["-l", "Test::More"])), Mode::Raw);
        assert_eq!(mode(&args(&["-m", "Test::More"])), Mode::Raw);
        // `-f l` asks for the docs of a function named "l", not the -l switch.
        assert_eq!(mode(&args(&["-f", "l"])), Mode::Rendered { add_text: true });
    }
}
