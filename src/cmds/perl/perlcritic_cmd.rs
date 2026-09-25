//! perlcritic filter: one line per (file, policy, message) with every location, grouped by file.
//!
//! perlcritic's own formats either drop the file name (`--verbose 8`, and any single-file run)
//! or spend most of each line on a PBP page reference. rtk passes its own `--verbose` format,
//! tab-separated so a message containing `:` cannot shift the fields, and regroups the result.
//! A user-supplied `--verbose`, and the listing modes (`--count`, `-l`, `--list`, `--doc`, ...),
//! pass through untouched.

use super::utils::unfiltered;
use crate::core::arg_tokenizer::{self, Dialect, TokenKind, ValueSpec};
use crate::core::runner;
use crate::core::tee::force_tee_tail_hint;
use crate::core::truncate::CAP_INVENTORY;
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::Result;

/// File, line, column, severity, policy (short name), description.
const FORMAT: &str = r"%f\t%l\t%c\t%s\t%p\t%m\n";

/// Output lines, file headers included, before the rest goes to `rtk recall`.
const MAX_LINES: usize = CAP_INVENTORY;

/// perlcritic's value-taking options, from `perlcritic --help`. `--top` takes an optional
/// number, which Getopt::Long only reads when attached.
fn perlcritic_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    match kind {
        TokenKind::Long => match name {
            "severity"
            | "profile"
            | "theme"
            | "include"
            | "exclude"
            | "single-policy"
            | "profile-strictness"
            | "verbose"
            | "pager"
            | "program-extensions"
            | "doc"
            | "color-severity-highest"
            | "color-severity-high"
            | "color-severity-medium"
            | "color-severity-low"
            | "color-severity-lowest" => Some(ValueSpec::value()),
            "top" => Some(ValueSpec::attached_only()),
            _ => None,
        },
        TokenKind::Short => matches!(name, "p" | "s").then(ValueSpec::value),
        _ => None,
    }
}

/// Whether rtk should add its format and regroup the output.
fn filters_this_invocation(args: &[String]) -> bool {
    let tokens = arg_tokenizer::tokenize_grammar(args, &perlcritic_takes_value, Dialect::Posix);
    !arg_tokenizer::before_dashdash(&tokens)
        .iter()
        .any(|t| match t.kind {
            TokenKind::Long => matches!(
                t.text,
                "verbose"
                    | "count"
                    | "statistics-only"
                    | "files-with-violations"
                    | "files-without-violations"
                    | "list"
                    | "list-enabled"
                    | "list-themes"
                    | "doc"
                    | "profile-proto"
                    | "help"
                    | "options"
                    | "man"
                    | "version"
            ),
            TokenKind::Short => matches!(t.text, "C" | "l" | "L"),
            _ => false,
        })
}

struct Violation<'a> {
    file: &'a str,
    line: u32,
    column: &'a str,
    severity: &'a str,
    policy: &'a str,
    message: &'a str,
}

fn parse_violation(line: &str) -> Option<Violation<'_>> {
    let mut fields = line.splitn(6, '\t');
    let file = fields.next()?;
    let line_no = fields.next()?.parse().ok()?;
    let column = fields.next()?;
    let severity = fields.next()?;
    let policy = fields.next()?;
    let message = fields.next()?;
    Some(Violation {
        file,
        line: line_no,
        column,
        severity,
        policy,
        message,
    })
}

/// One line of the report, and for a violation line what it stands for: `(file, severity,
/// violations)`. The counts let a cut-off report say what it hid.
struct ReportLine {
    text: String,
    counts: Option<(String, String, usize)>,
}

impl ReportLine {
    fn plain(text: String) -> Self {
        Self { text, counts: None }
    }
}

/// Shortest shared start and end, in characters, that a message template must keep for the
/// template form to be worth it over one line per message.
const MIN_FRAME: usize = 12;

fn common_prefix_len(a: &str, b: &str) -> usize {
    a.char_indices()
        .zip(b.chars())
        .find(|((_, ca), cb)| ca != cb)
        .map_or(a.len().min(b.len()), |((i, _), _)| i)
}

fn common_suffix_len(a: &str, b: &str) -> usize {
    a.chars()
        .rev()
        .zip(b.chars().rev())
        .take_while(|(ca, cb)| ca == cb)
        .map(|(c, _)| c.len_utf8())
        .sum()
}

/// The words every message starts and ends with, when they are most of each message:
/// `Sub ` + ` is never called from bin/ or lib/`. Cut back to word boundaries so a value is
/// never split. `None` when the messages have too little in common.
fn shared_frame<'a>(messages: &[&'a str]) -> Option<(&'a str, &'a str)> {
    let first = *messages.first()?;
    let mut prefix_len = first.len();
    let mut suffix_len = first.len();
    for m in &messages[1..] {
        prefix_len = prefix_len.min(common_prefix_len(first, m));
        suffix_len = suffix_len.min(common_suffix_len(first, m));
    }
    let prefix = &first[..prefix_len];
    let prefix = prefix.rfind(' ').map_or("", |i| &prefix[..=i]);
    let suffix = &first[first.len() - suffix_len..];
    let suffix = suffix.find(' ').map_or("", |i| &suffix[i..]);
    let frame = prefix.len() + suffix.len();
    if frame < MIN_FRAME || messages.iter().any(|m| m.len() <= frame) {
        return None;
    }
    Some((prefix, suffix))
}

fn locations(locs: &[(u32, &str)]) -> String {
    locs.iter()
        .map(|(l, c)| format!("{}:{}", l, c))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A (severity, policy) group: each distinct message with the `(line, column)` it fires at.
type Group<'a> = ((&'a str, &'a str), Vec<(&'a str, Vec<(u32, &'a str)>)>);

/// The report lines for one group: one line when every message is the same or they share a
/// template, one line per message otherwise.
fn render_group(file: &str, group: &Group<'_>) -> Vec<ReportLine> {
    let ((severity, policy), messages) = group;
    let counted = |text: String, n: usize| ReportLine {
        text,
        counts: Some((file.to_string(), severity.to_string(), n)),
    };
    let total: usize = messages.iter().map(|(_, locs)| locs.len()).sum();
    if let [(message, locs)] = messages.as_slice() {
        let text = format!(
            "  [{}] {}: {} ({})",
            severity,
            policy,
            message,
            locations(locs)
        );
        return vec![counted(text, total)];
    }
    let texts: Vec<&str> = messages.iter().map(|(m, _)| *m).collect();
    if let Some((prefix, suffix)) = shared_frame(&texts) {
        let values: Vec<String> = messages
            .iter()
            .map(|(m, locs)| {
                let value = &m[prefix.len()..m.len() - suffix.len()];
                format!("{} ({})", value, locations(locs))
            })
            .collect();
        let text = format!(
            "  [{}] {}: {}…{}: {}",
            severity,
            policy,
            prefix,
            suffix,
            values.join(", ")
        );
        return vec![counted(text, total)];
    }
    messages
        .iter()
        .map(|(message, locs)| {
            let text = format!(
                "  [{}] {}: {} ({})",
                severity,
                policy,
                message,
                locations(locs)
            );
            counted(text, locs.len())
        })
        .collect()
}

fn report_lines(raw: &str) -> Vec<ReportLine> {
    let clean = strip_ansi(raw);
    let mut other: Vec<ReportLine> = Vec::new();
    let mut clean_files = 0usize;
    // Files in first-seen order, each with its (severity, policy) groups.
    let mut files: Vec<(&str, Vec<Group>)> = Vec::new();

    for line in clean.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        if line.ends_with(" source OK") {
            clean_files += 1;
            continue;
        }
        let Some(v) = parse_violation(line) else {
            other.push(ReportLine::plain(line.to_string()));
            continue;
        };
        let groups = match files.iter().position(|(f, _)| *f == v.file) {
            Some(i) => &mut files[i].1,
            None => {
                files.push((v.file, Vec::new()));
                &mut files.last_mut().expect("just pushed").1
            }
        };
        let key = (v.severity, v.policy);
        let messages = match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, messages)) => messages,
            None => {
                groups.push((key, Vec::new()));
                &mut groups.last_mut().expect("just pushed").1
            }
        };
        match messages.iter_mut().find(|(m, _)| *m == v.message) {
            Some((_, locs)) => locs.push((v.line, v.column)),
            None => messages.push((v.message, vec![(v.line, v.column)])),
        }
    }

    let mut out = other;
    for (file, mut groups) in files {
        let count: usize = groups
            .iter()
            .flat_map(|(_, messages)| messages.iter().map(|(_, locs)| locs.len()))
            .sum();
        out.push(ReportLine::plain(format!(
            "{}: {} violation{}",
            file,
            count,
            if count == 1 { "" } else { "s" }
        )));
        // Most severe first, then by where the policy first fires.
        let first_line = |g: &Group| g.1.iter().map(|(_, locs)| locs[0].0).min().unwrap_or(0);
        groups.sort_by(|a, b| b.0.0.cmp(a.0.0).then(first_line(a).cmp(&first_line(b))));
        for group in &groups {
            out.extend(render_group(file, group));
        }
    }
    if clean_files > 0 {
        out.push(ReportLine::plain(format!(
            "{} file{} source OK",
            clean_files,
            if clean_files == 1 { "" } else { "s" }
        )));
    }
    out
}

/// What the cut-off lines hold, per file: `hidden: 3 in lib/B.pm (severity 2: 2, severity 1:
/// 1)`. The reader can then tell whether anything severe is behind the recall hint.
fn hidden_summary(lines: &[ReportLine]) -> Vec<String> {
    let mut per_file: Vec<(String, Vec<(String, usize)>)> = Vec::new();
    for (file, severity, n) in lines.iter().filter_map(|l| l.counts.as_ref()) {
        if per_file.last().is_none_or(|(f, _)| f != file) {
            per_file.push((file.clone(), Vec::new()));
        }
        let counts = &mut per_file.last_mut().expect("just pushed").1;
        match counts.iter_mut().find(|(s, _)| s == severity) {
            Some((_, total)) => *total += n,
            None => counts.push((severity.clone(), *n)),
        }
    }
    per_file
        .into_iter()
        .map(|(file, counts)| {
            let total: usize = counts.iter().map(|(_, n)| n).sum();
            let by_severity: Vec<String> = counts
                .iter()
                .map(|(s, n)| format!("severity {}: {}", s, n))
                .collect();
            format!("hidden: {} in {} ({})", total, file, by_severity.join(", "))
        })
        .collect()
}

fn filter_perlcritic(raw: &str) -> String {
    let lines = report_lines(raw);
    let all = lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if lines.len() <= MAX_LINES {
        return all;
    }
    let mut shown: Vec<String> = lines[..MAX_LINES].iter().map(|l| l.text.clone()).collect();
    shown.extend(hidden_summary(&lines[MAX_LINES..]));
    let mut text = shown.join("\n");
    if let Some(hint) = force_tee_tail_hint(&all, "perlcritic", MAX_LINES + 1) {
        text.push('\n');
        text.push_str(&hint);
    }
    text
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("perlcritic");
    let filtered = filters_this_invocation(args);
    if filtered {
        // Getopt::Long takes the last --verbose, and the user passed none, so a profile's
        // `verbose = N` is the only thing this overrides.
        cmd.arg("--verbose").arg(FORMAT);
    }
    cmd.args(args);

    if verbose > 0 {
        eprintln!("Running: perlcritic {}", args.join(" "));
    }

    let filter: fn(&str) -> String = if filtered {
        filter_perlcritic
    } else {
        unfiltered
    };

    runner::run_filtered(
        cmd,
        "perlcritic",
        &args.join(" "),
        filter,
        runner::RunOptions::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    /// The report's text, for assertions.
    fn format_perlcritic(raw: &str) -> Vec<String> {
        report_lines(raw).into_iter().map(|l| l.text).collect()
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// The fixture was captured with `--verbose '%f:%l:%c:%s:%p:%m\n'`; the policy name never
    /// contains `:` twice in a row, so it converts cleanly to the tab-separated form rtk asks for.
    fn to_tabs(colon_format: &str) -> String {
        colon_format
            .lines()
            .map(|l| l.splitn(6, ':').collect::<Vec<_>>().join("\t"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn test_groups_by_file_and_policy() {
        let input = "lib/A.pm\t22\t5\t5\tSubroutines::ProhibitReturnSort\t\"return\" statement followed by \"sort\"\n\
                     lib/A.pm\t4\t1\t3\tValuesAndExpressions::ProhibitMagicNumbers\tNumeric literals\n\
                     lib/A.pm\t9\t7\t3\tValuesAndExpressions::ProhibitMagicNumbers\tNumeric literals\n\
                     lib/B.pm source OK\n";
        assert_eq!(
            format_perlcritic(input),
            vec![
                "lib/A.pm: 3 violations",
                "  [5] Subroutines::ProhibitReturnSort: \"return\" statement followed by \"sort\" (22:5)",
                "  [3] ValuesAndExpressions::ProhibitMagicNumbers: Numeric literals (4:1, 9:7)",
                "1 file source OK",
            ]
        );
    }

    #[test]
    fn test_fixture_keeps_every_violation() {
        let colon = include_str!("../../../tests/fixtures/perl/perlcritic_fmt_raw.txt");
        let input = to_tabs(colon);
        let located: usize = report_lines(&input)
            .iter()
            .filter_map(|l| l.counts.as_ref().map(|(_, _, n)| n))
            .sum();
        assert_eq!(located, colon.lines().count());
        // Every `line:column` of the input appears in the output.
        let output = format_perlcritic(&input);
        let joined = output.join("\n");
        for line in colon.lines() {
            let mut f = line.splitn(4, ':');
            let (_, l, c) = (f.next(), f.next().unwrap_or(""), f.next().unwrap_or(""));
            assert!(
                joined.contains(&format!("{}:{}", l, c)),
                "{}:{} missing",
                l,
                c
            );
        }
        let in_first_file = colon
            .lines()
            .filter(|l| l.starts_with("lib/Acme/RtkSample.pm:"))
            .count();
        assert!(output.contains(&format!(
            "lib/Acme/RtkSample.pm: {} violations",
            in_first_file
        )));
    }

    #[test]
    fn test_savings_against_default_verbose_output() {
        // What the user would otherwise read is perlcritic's default format, with the PBP
        // references; compare against that.
        let raw = include_str!("../../../tests/fixtures/perl/perlcritic_sev1_raw.txt");
        let colon = include_str!("../../../tests/fixtures/perl/perlcritic_fmt_raw.txt");
        let output = filter_perlcritic(&to_tabs(colon));
        let pct = 100.0 - (count_tokens(&output) as f64 / count_tokens(raw) as f64 * 100.0);
        assert!(
            pct >= 40.0,
            "expected >=40% savings, got {:.1}%\n{}",
            pct,
            output
        );
    }

    #[test]
    fn test_non_violation_lines_kept() {
        let input = "Can't parse code: Syntax error at line 3 (lib/Bad.pm)\nlib/C.pm source OK\nlib/D.pm source OK\n";
        assert_eq!(
            format_perlcritic(input),
            vec![
                "Can't parse code: Syntax error at line 3 (lib/Bad.pm)",
                "2 files source OK"
            ]
        );
    }

    #[test]
    fn test_hidden_summary_names_file_and_severities() {
        let line = |file: &str, severity: &str, n: usize| ReportLine {
            text: String::new(),
            counts: Some((file.to_string(), severity.to_string(), n)),
        };
        let hidden = vec![
            line("lib/A.pm", "2", 1),
            ReportLine::plain("lib/B.pm: 4 violations".to_string()),
            line("lib/B.pm", "3", 2),
            line("lib/B.pm", "1", 2),
        ];
        assert_eq!(
            hidden_summary(&hidden),
            vec![
                "hidden: 1 in lib/A.pm (severity 2: 1)",
                "hidden: 4 in lib/B.pm (severity 3: 2, severity 1: 2)",
            ]
        );
    }

    #[test]
    fn test_shared_frame_groups_varying_messages() {
        let input = "lib/A.pm\t9\t1\t2\tProhibitUnusedDefinitions\tSub A::new is never called from bin/ or lib/\n\
                     lib/A.pm\t16\t1\t2\tProhibitUnusedDefinitions\tSub A::add is never called from bin/ or lib/\n\
                     lib/A.pm\t38\t18\t2\tValuesAndExpressions::ProhibitMagicNumbers\t100 is not one of the allowed literal values (0, 1, 2).\n\
                     lib/A.pm\t39\t22\t2\tValuesAndExpressions::ProhibitMagicNumbers\t1000 is not one of the allowed literal values (0, 1, 2).\n\
                     lib/A.pm\t21\t1\t1\tCognitive\tscore of '1'\n\
                     lib/A.pm\t35\t1\t1\tCognitive\tscore of '8'\n";
        assert_eq!(
            format_perlcritic(input),
            vec![
                "lib/A.pm: 6 violations",
                "  [2] ProhibitUnusedDefinitions: Sub … is never called from bin/ or lib/: A::new (9:1), A::add (16:1)",
                "  [2] ValuesAndExpressions::ProhibitMagicNumbers: … is not one of the allowed literal values (0, 1, 2).: 100 (38:18), 1000 (39:22)",
                "  [1] Cognitive: score of '1' (21:1)",
                "  [1] Cognitive: score of '8' (35:1)",
            ]
        );
    }

    #[test]
    fn test_empty_input() {
        assert!(format_perlcritic("").is_empty());
    }

    #[test]
    fn test_invocation_detection() {
        assert!(filters_this_invocation(&args(&["lib"])));
        assert!(filters_this_invocation(&args(&["--severity", "1", "lib"])));
        assert!(filters_this_invocation(&args(&["-3", "lib"])));
        assert!(!filters_this_invocation(&args(&["--verbose", "8", "lib"])));
        assert!(!filters_this_invocation(&args(&["-C", "lib"])));
        assert!(!filters_this_invocation(&args(&["--list"])));
    }
}
