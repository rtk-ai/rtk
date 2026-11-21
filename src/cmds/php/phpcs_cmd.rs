//! PHP_CodeSniffer (`phpcs`) and Code Beautifier and Fixer (`phpcbf`) output filter.
//!
//! The text reports (`full`, `summary`, `source`) are framed by rows of dashes, blank lines,
//! a progress line under `-p`, and a `Time: ...; Memory: ...` footer. This filter drops that
//! frame and keeps every `FILE:` header, finding row, total and "can fix" line verbatim.
//! Anything that is not a plain text report (`--report=json`, `-v`, `-i`, `-e`, ...) is passed
//! through untouched, since the user asked for that exact shape.

use super::utils::{php_tool_command, strip_ansi_and_controls};
use crate::core::arg_tokenizer::{self, Dialect, TokenKind, ValueSpec};
use crate::core::runner::{self, RunMode, RunOptions};
use crate::core::shell::display_args;
use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

/// Reports whose screen layout this filter understands. Every other report format is a
/// machine-readable or tool-specific shape that must reach the caller unchanged.
const FILTERABLE_REPORTS: [&str; 3] = ["full", "summary", "source"];

static SEPARATOR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^-{20,}$").unwrap());
static PROGRESS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[.EWSF]+\s+\d+ / \d+ \(\d+%\)$").unwrap());
static FOOTER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Time: [^;]+; Memory: \S+$").unwrap());

pub fn run_phpcs(args: &[String], verbose: u8) -> Result<i32> {
    run_tool("phpcs", args, verbose)
}

pub fn run_phpcbf(args: &[String], verbose: u8) -> Result<i32> {
    run_tool("phpcbf", args, verbose)
}

fn run_tool(tool: &str, args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = php_tool_command(tool);
    cmd.args(args);

    if verbose > 0 {
        eprintln!("Running: {} {}", tool, args.join(" "));
    }

    let label = display_args(args);
    let Some(filter) = filter_for(args) else {
        return runner::run(
            cmd,
            tool,
            &label,
            RunMode::Passthrough,
            RunOptions::default(),
        );
    };

    runner::run_filtered(cmd, tool, &label, filter, RunOptions::default().tee(tool))
}

/// The output filter for this invocation, or `None` when it must run unfiltered.
fn filter_for(args: &[String]) -> Option<fn(&str) -> String> {
    wants_text_report(args).then_some(filter_phpcs_output as fn(&str) -> String)
}

/// phpcs/phpcbf option grammar, transcribed from `phpcs --help`.
fn takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    match (kind, name) {
        (TokenKind::Short, "d") => Some(ValueSpec::solo_only()),
        (TokenKind::Long, "cache") => Some(ValueSpec::attached_only()),
        (
            TokenKind::Long,
            "stdin-path" | "file-list" | "filter" | "ignore" | "extensions" | "standard" | "sniffs"
            | "exclude" | "generator" | "bootstrap" | "parallel" | "report" | "report-file"
            | "report-width" | "basepath" | "severity" | "error-severity" | "warning-severity"
            | "encoding" | "tab-width" | "config-set" | "config-delete" | "runtime-set",
        ) => Some(ValueSpec::value()),
        (TokenKind::Long, n) if n.starts_with("report-") => Some(ValueSpec::value()),
        _ => None,
    }
}

/// True when the invocation prints a text report whose frame this filter may strip.
fn wants_text_report(args: &[String]) -> bool {
    let tokens = arg_tokenizer::tokenize_grammar(args, &takes_value, Dialect::Posix);
    let own = arg_tokenizer::before_dashdash(&tokens);

    for token in own {
        match (token.kind, token.text) {
            // `-` reads the file from stdin and phpcbf then prints the fixed source.
            (TokenKind::Positional, "-") if token.linked.is_none() => return false,
            // Verbose levels, installed-standard listings, interactive mode, config edits,
            // doc generators: the caller asked for detail or for something that is not a scan.
            (TokenKind::Short, "v" | "i" | "e" | "a") => return false,
            (
                TokenKind::Long,
                "generator" | "config-show" | "config-set" | "config-delete" | "version",
            ) => return false,
            (TokenKind::Long, "report") => {
                let value = token.value(&tokens).unwrap_or("");
                if !value.split(',').all(|r| FILTERABLE_REPORTS.contains(&r)) {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

fn filter_phpcs_output(output: &str) -> String {
    let cleaned = strip_ansi_and_controls(output);
    let kept: Vec<&str> = cleaned
        .lines()
        .map(str::trim_end)
        .filter(|line| !is_frame(line))
        .collect();
    kept.join("\n")
}

fn is_frame(line: &str) -> bool {
    line.is_empty()
        || SEPARATOR_RE.is_match(line)
        || PROGRESS_RE.is_match(line)
        || FOOTER_RE.is_match(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tracking::estimate_tokens;

    const PHPCS_FULL: &str = include_str!("../../../tests/fixtures/phpcs_full_raw.txt");
    const PHPCS_SUMMARY: &str = include_str!("../../../tests/fixtures/phpcs_summary_raw.txt");
    const PHPCBF: &str = include_str!("../../../tests/fixtures/phpcbf_raw.txt");

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn savings(raw: &str, filtered: &str) -> f64 {
        100.0 - (estimate_tokens(filtered) as f64 / estimate_tokens(raw) as f64 * 100.0)
    }

    #[test]
    fn full_report_drops_frame_and_keeps_every_finding() {
        let out = filter_phpcs_output(PHPCS_FULL);
        assert!(!out.contains("-----"));
        assert!(!out.contains("5 / 5"));
        for line in PHPCS_FULL.lines().filter(|l| l.contains("| ERROR")) {
            assert!(out.contains(line.trim_end()), "lost finding: {line}");
        }
        assert!(out.contains("FILE: /app/src/Calc.php"));
        assert!(out.contains("FOUND 13 ERRORS AFFECTING 6 LINES"));
        assert!(out.contains("FOUND 8 ERRORS AND 1 WARNING AFFECTING 7 LINES"));
        assert!(out.contains("PHPCBF CAN FIX THE 11 MARKED SNIFF VIOLATIONS AUTOMATICALLY"));
        assert!(out.contains("| WARNING | [ ] Visibility must be declared"));
    }

    #[test]
    fn full_report_meets_savings_floor() {
        let s = savings(PHPCS_FULL, &filter_phpcs_output(PHPCS_FULL));
        assert!(s >= 20.0, "phpcs full savings {s:.1}%");
    }

    #[test]
    fn summary_report_keeps_table_rows_and_totals() {
        let out = filter_phpcs_output(PHPCS_SUMMARY);
        assert!(out.contains("PHP CODE SNIFFER REPORT SUMMARY"));
        assert!(
            out.contains("FILE                                                  ERRORS  WARNINGS")
        );
        assert!(out.contains("/app/src/Calc.php                                     13      0"));
        assert!(out.contains("A TOTAL OF 13 ERRORS AND 0 WARNINGS WERE FOUND IN 1 FILE"));
        assert!(!out.contains("Time:"));
        let s = savings(PHPCS_SUMMARY, &out);
        assert!(s >= 20.0, "phpcs summary savings {s:.1}%");
    }

    #[test]
    fn phpcbf_result_summary_keeps_counts() {
        let out = filter_phpcs_output(PHPCBF);
        assert!(out.contains("PHPCBF RESULT SUMMARY"));
        assert!(out.contains("/app/src/Util/Strings.php                             7      2"));
        assert!(out.contains("A TOTAL OF 22 ERRORS WERE FIXED IN 3 FILES"));
        assert!(!out.contains("-----"));
        assert!(!out.contains("Time:"));
        let s = savings(PHPCBF, &out);
        assert!(s >= 20.0, "phpcbf savings {s:.1}%");
    }

    #[test]
    fn clean_run_stays_empty() {
        assert_eq!(filter_phpcs_output(""), "");
        assert_eq!(filter_phpcs_output("\n\n"), "");
    }

    #[test]
    fn progress_lines_are_dropped() {
        let raw = "..........E.....W...   20 / 20 (100%)\n\nFILE: a.php\n";
        assert_eq!(filter_phpcs_output(raw), "FILE: a.php");
    }

    #[test]
    fn ansi_colors_are_stripped_before_matching() {
        let raw = "\u{1b}[1mFILE: a.php\u{1b}[0m\n--------------------------------------------------------------------------------\n";
        assert_eq!(filter_phpcs_output(raw), "FILE: a.php");
    }

    #[test]
    fn a_dash_row_shorter_than_a_report_frame_is_kept() {
        assert_eq!(filter_phpcs_output("-----\n"), "-----");
    }

    #[test]
    fn default_and_text_reports_are_filtered() {
        assert!(wants_text_report(&args(&["src"])));
        assert!(wants_text_report(&args(&[
            "--standard=PSR12",
            "-s",
            "-p",
            "src"
        ])));
        assert!(wants_text_report(&args(&["--report=summary", "src"])));
        assert!(wants_text_report(&args(&["--report", "source", "src"])));
        assert!(wants_text_report(&args(&["--report=full,summary"])));
        assert!(wants_text_report(&args(&["--report-json=out.json", "src"])));
        assert!(wants_text_report(&args(&["--report-file=out.txt", "src"])));
    }

    #[test]
    fn machine_readable_reports_pass_through() {
        for report in [
            "json",
            "xml",
            "checkstyle",
            "csv",
            "junit",
            "emacs",
            "diff",
            "gitblame",
        ] {
            assert!(
                !wants_text_report(&args(&[&format!("--report={report}"), "src"])),
                "{report}"
            );
        }
        assert!(!wants_text_report(&args(&["--report", "json", "src"])));
        assert!(!wants_text_report(&args(&["--report=full,json", "src"])));
    }

    #[test]
    fn verbose_listing_and_stdin_modes_pass_through() {
        for a in [
            &["-v", "src"][..],
            &["-vvv", "src"],
            &["-i"],
            &["-e", "--standard=PSR12"],
            &["-a", "src"],
            &["--generator=text", "--standard=PSR12"],
            &["--config-show"],
            &["--version"],
            &["--stdin-path=a.php", "-"],
        ] {
            assert!(!wants_text_report(&args(a)), "{a:?}");
        }
    }

    #[test]
    fn a_flag_value_that_looks_like_a_flag_is_not_one() {
        // `--ignore -v` ignores the pattern "-v"; it is not a verbose request.
        assert!(wants_text_report(&args(&["--ignore", "-v", "src"])));
        // Past `--` everything is a path.
        assert!(wants_text_report(&args(&["src", "--", "-v"])));
    }
}
