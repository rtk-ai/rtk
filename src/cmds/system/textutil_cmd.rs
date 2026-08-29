//! Proxy for macOS `textutil`. Document-to-text conversions sent to stdout get
//! the same blank-line / trailing-whitespace cleanup `rtk read` applies, `-info`
//! collapses to a single line, and every other invocation runs through untouched.

use crate::core::filter::{self, Language};
use crate::core::runner::{self, RunOptions};
use crate::core::truncate::CAP_DOCUMENT;
use crate::core::utils::resolved_command;
use anyhow::Result;
use std::ffi::OsString;

const MAX_DOC_LINES: usize = CAP_DOCUMENT;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("Running: textutil {}", args.join(" "));
    }

    match classify(args) {
        Mode::Passthrough => {
            let os_args: Vec<OsString> = args.iter().map(OsString::from).collect();
            runner::run_passthrough("textutil", &os_args, verbose)
        }
        mode => {
            let mut cmd = resolved_command("textutil");
            for arg in args {
                cmd.arg(arg);
            }
            let filter: fn(&str) -> String = match mode {
                Mode::Info => filter_info,
                _ => filter_text,
            };
            runner::run_filtered(
                cmd,
                "textutil",
                &args.join(" "),
                filter,
                RunOptions::stdout_only(),
            )
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Info,
    Text,
    Passthrough,
}

/// Only two invocations produce plain text we can safely touch: `-info` (a
/// property block) and a `txt` conversion written to stdout. Markup conversions,
/// file output and anything unrecognized fall through to raw passthrough.
fn classify(args: &[String]) -> Mode {
    if args.iter().any(|a| a == "-info") {
        return Mode::Info;
    }
    if args.iter().any(|a| a == "-stdout") && converts_to_text(args) {
        return Mode::Text;
    }
    Mode::Passthrough
}

fn converts_to_text(args: &[String]) -> bool {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "-convert" || arg == "-cat" {
            return matches!(args.next(), Some(fmt) if fmt.eq_ignore_ascii_case("txt"));
        }
    }
    false
}

/// Collapse runs of blank lines, drop trailing whitespace, and cap the body with
/// a `[N more lines]` marker — the treatment `rtk read` gives a plain text file.
fn filter_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_blank = false;
    for line in raw.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            if !prev_blank {
                out.push('\n');
            }
            prev_blank = true;
            continue;
        }
        prev_blank = false;
        out.push_str(line);
        out.push('\n');
    }
    filter::smart_truncate(out.trim_matches('\n'), MAX_DOC_LINES, &Language::Unknown)
}

/// Fold `textutil -info`'s multi-line property block onto one line, keeping the
/// file's base name rather than its full path.
fn filter_info(raw: &str) -> String {
    let mut name = String::new();
    let mut fields: Vec<String> = Vec::new();
    for line in raw.lines() {
        let (label, value) = match line.split_once(':') {
            Some(pair) => (pair.0.trim(), pair.1.trim()),
            None => continue,
        };
        if value.is_empty() {
            continue;
        }
        if label.eq_ignore_ascii_case("file") {
            name = value.rsplit(['/', '\\']).next().unwrap_or(value).to_string();
        } else {
            fields.push(format!("{}: {}", label, value));
        }
    }
    match (name.is_empty(), fields.is_empty()) {
        (true, true) => raw.trim().to_string(),
        (true, false) => fields.join(", "),
        (false, true) => name,
        (false, false) => format!("{} ({})", name, fields.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn classify_text_conversion_to_stdout() {
        assert!(classify(&args(&["-convert", "txt", "-stdout", "resume.docx"])) == Mode::Text);
        assert!(classify(&args(&["-cat", "txt", "-stdout", "a.rtf", "b.rtf"])) == Mode::Text);
    }

    #[test]
    fn classify_file_output_is_passthrough() {
        assert!(classify(&args(&["-convert", "txt", "resume.docx"])) == Mode::Passthrough);
    }

    #[test]
    fn classify_markup_conversion_is_passthrough() {
        assert!(classify(&args(&["-convert", "html", "-stdout", "resume.docx"])) == Mode::Passthrough);
    }

    #[test]
    fn classify_info() {
        assert!(classify(&args(&["-info", "resume.docx"])) == Mode::Info);
    }

    #[test]
    fn text_collapses_blanks_and_trailing_whitespace() {
        let input = "First line   \n\n\n\nSecond line\t\n";
        assert_eq!(filter_text(input), "First line\n\nSecond line");
    }

    #[test]
    fn text_saves_tokens_on_a_long_document() {
        let input = include_str!("../../../tests/fixtures/textutil_convert_txt_raw.txt");
        let output = filter_text(input);
        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "textutil text filter: expected >=60% savings, got {:.1}%",
            savings
        );
        assert!(output.contains("more lines"));
    }

    #[test]
    fn info_folds_to_one_line() {
        let input = include_str!("../../../tests/fixtures/textutil_info_raw.txt");
        let output = filter_info(input);
        assert_eq!(output.lines().count(), 1);
        assert!(output.contains("report.docx"));
        assert!(!output.contains('/'), "the directory path should be dropped");
    }
}
