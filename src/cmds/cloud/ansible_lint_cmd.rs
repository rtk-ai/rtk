//! Compact ansible-lint reports grouped by rule, with recoverable sample caps.

use crate::core::arg_tokenizer::{self, Dialect, TokenKind, ValueSpec};
use crate::core::runner;
use crate::core::tracking;
use crate::core::truncate::{CAP_WARNINGS, reduced};
use crate::core::utils::{ChildArgExt, resolved_command, strip_ansi};
use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;
use std::ffi::OsString;
use std::sync::LazyLock;

// A rule's examples repeat both a location and a message, unlike a flat warning list.
const SAMPLES_PER_RULE: usize = reduced(CAP_WARNINGS, 7);

// ansible-lint's rich output wraps rule IDs in OSC hyperlinks as well as CSI colors.
static OSC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)").expect("valid OSC pattern"));
static RULE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([a-z0-9][a-z0-9_.-]*(?:\[[a-z0-9_.-]+\])?)(?:\[/\])?: (.+)$")
        .expect("valid rule pattern")
});
static LOCATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(.+?:[0-9]+(?::[0-9]+)?)(?: (.*))?$").expect("valid location pattern")
});
static PEP8: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(.+?:[0-9]+(?::[0-9]+)?): ([a-z0-9][a-z0-9_.-]*(?:\[[a-z0-9_.-]+\])?)(?:\[/\])?: (.+)$",
    )
    .expect("valid pep8 pattern")
});
static TABLE_ROW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s+[0-9]+ [a-z0-9][a-z0-9_.-]* profile:\S+ tags:\S+\s*$")
        .expect("valid summary table pattern")
});
static LISTING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^WARNING\s+Listing [0-9]+ violation\(s\) that are fatal$")
        .expect("valid listing pattern")
});

pub fn run(args: &[OsString], verbose: u8) -> Result<i32> {
    let args = crate::core::args_utils::restore_double_dash_os(args);
    if passthrough(&args) {
        return runner::run_passthrough("ansible-lint", &args, verbose);
    }
    let mut cmd = resolved_command("ansible-lint");
    cmd.child_args(&args);
    if verbose > 0 {
        eprintln!("Running: ansible-lint {}", tracking::args_display(&args));
    }
    runner::run_filtered(
        cmd,
        "ansible-lint",
        &tracking::args_display(&args),
        |raw| {
            filter_output(raw, |raw| {
                crate::core::tee::force_tee_hint(raw, "ansible-lint")
            })
        },
        runner::RunOptions::default(),
    )
}

fn takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    let value = match kind {
        TokenKind::Short => matches!(name, "f" | "r" | "t" | "x" | "w" | "c" | "i"),
        TokenKind::Long => matches!(
            name,
            "format"
                | "sarif-file"
                | "profile"
                | "project-dir"
                | "rules-dir"
                | "tags"
                | "skip-list"
                | "warn-list"
                | "enable-list"
                | "exclude"
                | "config-file"
                | "ignore-file"
                | "yamllint-file"
        ),
        _ => false,
    };
    value.then(ValueSpec::value)
}

fn passthrough(args: &[OsString]) -> bool {
    let text: Vec<_> = args.iter().map(|s| s.to_string_lossy()).collect();
    let tokens = arg_tokenizer::tokenize_grammar(&text, &takes_value, Dialect::Posix);
    arg_tokenizer::before_dashdash(&tokens)
        .iter()
        .any(|t| match t.kind {
            TokenKind::Long => matches!(
                t.text,
                "help"
                    | "version"
                    | "list-rules"
                    | "list-tags"
                    | "list-profiles"
                    | "format"
                    | "fix"
                    | "generate-ignore"
            ),
            TokenKind::Short => matches!(t.text, "h" | "L" | "T" | "P" | "f" | "v" | "q"),
            _ => false,
        })
}

struct Group<'a> {
    rule: &'a str,
    count: usize,
    samples: Vec<String>,
    last_message: &'a str,
    last_details: &'a str,
}

struct Record<'a> {
    rule: &'a str,
    location: &'a str,
    details: &'a str,
    message: &'a str,
    consumed: usize,
}

fn parse_record<'a>(lines: &[&'a str]) -> Option<Record<'a>> {
    let line = *lines.first()?;
    if let Some(c) = PEP8.captures(line) {
        return Some(Record {
            rule: c.get(2)?.as_str(),
            location: c.get(1)?.as_str(),
            details: "",
            message: c.get(3)?.as_str(),
            consumed: 1,
        });
    }
    let rule = RULE.captures(line)?;
    let location = LOCATION.captures(lines.get(1)?)?;
    if lines.get(2).is_some_and(|line| !line.is_empty()) {
        return None;
    }
    Some(Record {
        rule: rule.get(1)?.as_str(),
        location: location.get(1)?.as_str(),
        details: location.get(2).map_or("", |m| m.as_str()),
        message: rule.get(2)?.as_str(),
        consumed: 2,
    })
}

impl<'a> Group<'a> {
    fn push(&mut self, record: &Record<'a>) {
        self.count += 1;
        if self.samples.len() >= SAMPLES_PER_RULE {
            return;
        }
        let mut sample = record.location.to_string();
        for (text, previous) in [
            (record.details, self.last_details),
            (record.message, self.last_message),
        ] {
            if !text.is_empty()
                && (text != previous || text.contains("(warning)") || text.contains("# ignored"))
            {
                sample.push(' ');
                sample.push_str(text);
            }
        }
        self.samples.push(sample);
        self.last_details = record.details;
        self.last_message = record.message;
    }
}

fn is_noise(line: &str) -> bool {
    line.is_empty()
        || line == "# Rule Violation Summary"
        || line == "Read documentation for instructions on how to ignore specific rule violations."
        || line.starts_with("WARNING: PATH altered to expand ~")
        || line.starts_with("AnsibleWarning: Enable tracemalloc")
        || TABLE_ROW.is_match(line)
        || LISTING.is_match(line)
}

fn filter_output(raw: &str, recover: impl FnOnce(&str) -> Option<String>) -> String {
    let clean = strip_ansi(raw);
    let clean = OSC.replace_all(&clean, "").into_owned();
    let lines: Vec<_> = clean.lines().collect();
    let mut groups: Vec<Group<'_>> = Vec::new();
    let mut indices = HashMap::new();
    let mut diagnostics = Vec::new();
    let mut summaries = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if is_noise(line) {
            i += 1;
            continue;
        }
        if line.starts_with("Passed: ") || line.starts_with("Failed: ") {
            summaries.push(line);
            i += 1;
            continue;
        }
        if let Some(record) = parse_record(&lines[i..]) {
            let index = *indices.entry(record.rule).or_insert_with(|| {
                groups.push(Group {
                    rule: record.rule,
                    count: 0,
                    samples: Vec::new(),
                    last_message: "",
                    last_details: "",
                });
                groups.len() - 1
            });
            groups[index].push(&record);
            i += record.consumed;
        } else if line.starts_with("WARNING  Directory expansion discovered ")
            || line.starts_with("Deprecated `-p` cli option")
        {
            diagnostics.push(line);
            i += 1;
        } else {
            // Unknown formats and multiline context must stay attached to their records.
            return clean;
        }
    }
    if groups.is_empty() {
        return diagnostics
            .into_iter()
            .chain(summaries)
            .collect::<Vec<_>>()
            .join("\n");
    }
    let omitted = groups.iter().any(|g| g.count > g.samples.len());
    let hint = if omitted {
        match recover(raw) {
            Some(hint) => Some(hint),
            None => return clean,
        }
    } else {
        None
    };
    let mut out: Vec<String> = diagnostics.into_iter().map(str::to_owned).collect();
    for group in groups {
        out.push(format!("{}: {}", group.rule, group.count));
        out.extend(group.samples.into_iter().map(|s| format!("  {s}")));
        if group.count > SAMPLES_PER_RULE {
            out.push(format!("  … +{} more", group.count - SAMPLES_PER_RULE));
        }
    }
    out.extend(summaries.into_iter().map(str::to_owned));
    out.extend(hint);
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_real_default_report_with_recoverable_samples() {
        let raw = include_str!("../../../tests/fixtures/ansible_lint_default.txt");
        let shown = filter_output(raw, |_| Some("[full output: test capture]".into()));
        assert!(shown.contains("command-instead-of-shell: 6"), "{shown}");
        assert!(shown.contains("fqcn[action-core]: 6"), "{shown}");
        assert!(shown.contains("name[missing]: 6"), "{shown}");
        assert!(shown.contains("no-changed-when: 6"), "{shown}");
        assert_eq!(shown.matches("… +3 more").count(), 4);
        assert!(!shown.contains("main.yml:5"));
        assert!(shown.contains("main.yml:4"));
        assert!(shown.contains("Failed: 24 failure(s)"));
        assert!(shown.contains("WARNING  Directory expansion"));
        assert!(shown.contains("[full output: test capture]"));
        assert!(!shown.contains("# Rule Violation Summary"));
        assert!(!shown.contains("Listing 24 violation(s)"));
    }

    #[test]
    fn groups_real_legacy_pep8_report() {
        let raw = include_str!("../../../tests/fixtures/ansible_lint_pep8.txt");
        let shown = filter_output(raw, |_| Some("[full output: test capture]".into()));
        assert!(shown.contains("name[missing]: 6"), "{shown}");
        assert!(!shown.contains("[/]"));
        assert!(shown.contains("main.yml:2:3"));
        assert!(shown.contains("Deprecated `-p` cli option"));
        assert_eq!(shown.matches("… +3 more").count(), 4);
        assert!(shown.contains("Failed: 24 failure(s)"));
    }

    #[test]
    fn unavailable_recovery_preserves_every_occurrence() {
        for raw in [
            include_str!("../../../tests/fixtures/ansible_lint_default.txt"),
            include_str!("../../../tests/fixtures/ansible_lint_pep8.txt"),
        ] {
            assert_eq!(filter_output(raw, |_| None), raw);
        }
    }

    #[test]
    fn multiline_and_unfamiliar_reports_stay_intact() {
        let multiline: String = (1..=5).map(|i| format!(
            "schema[tasks]: Invalid task.\nroles/r{i}/tasks/main.yml:2 First detail\n\nDistinct context for role {i}.\n\n"
        )).collect();
        for raw in [
            multiline.as_str(),
            "name[missing]: Missing name\na.yml:2 Task\n\nERROR custom plugin crashed\n",
            "Traceback (most recent call last):\n  ConfigError: invalid configuration\n",
            "{\"unexpected\": \"structured output\"}\n",
            "name[missing]: truncated record\n",
            "schema[tasks]: Invalid task\na.yml:2 Details\nWARNING context attached to this schema error\n\n",
        ] {
            assert_eq!(
                filter_output(raw, |_| panic!("must not cap unfamiliar data")),
                raw
            );
        }
    }

    #[test]
    fn clean_empty_and_known_noise() {
        let summary = "Passed: 0 failure(s), 0 warning(s) in 2 files processed of 3 encountered.";
        let raw = format!(
            "WARNING: PATH altered to expand ~ in local path\nAnsibleWarning: Enable tracemalloc to get the object allocation traceback\n\n\x1b[32m{summary}\x1b[0m\n"
        );
        assert_eq!(filter_output(&raw, |_| None), summary);
        assert_eq!(filter_output("", |_| None), "");
    }

    #[test]
    fn retains_warning_status_messages_and_windows_locations() {
        let raw = "\x1b[31mname[missing]: Missing name (warning) # ignored\x1b[0m\nC:\\roles with spaces\\tasks.yml:2 Task details\n\nname[missing]: Another message\nC:\\roles with spaces\\tasks.yml:5:3 Other details\n\nFailed: 0 failure(s), 2 warning(s)\n";
        assert_eq!(
            filter_output(raw, |_| None),
            "name[missing]: 2\n  C:\\roles with spaces\\tasks.yml:2 Task details Missing name (warning) # ignored\n  C:\\roles with spaces\\tasks.yml:5:3 Other details Another message\nFailed: 0 failure(s), 2 warning(s)"
        );
    }

    #[test]
    fn flag_routing_respects_clusters_values_and_boundaries() {
        for args in [
            vec!["--help"],
            vec!["-h"],
            vec!["--version"],
            vec!["-L"],
            vec!["-T"],
            vec!["-P"],
            vec!["--list-rules"],
            vec!["--list-tags"],
            vec!["--list-profiles"],
            vec!["--format=json"],
            vec!["--format", "sarif"],
            vec!["-fjson"],
            vec!["-f", "pep8"],
            vec!["-vp"],
            vec!["-qq"],
            vec!["--fix=all"],
            vec!["--generate-ignore"],
        ] {
            assert!(
                passthrough(&args.iter().map(OsString::from).collect::<Vec<_>>()),
                "{args:?}"
            );
        }
        for args in [
            vec!["--offline", "-p", "roles/"],
            vec!["--", "--format=json"],
            vec!["--config-file", "-v"],
            vec!["-t", "--help"],
            vec!["-r-L", "roles/"],
            vec!["--profile=production"],
        ] {
            assert!(
                !passthrough(&args.iter().map(OsString::from).collect::<Vec<_>>()),
                "{args:?}"
            );
        }
    }

    #[test]
    fn real_report_savings_meet_filter_floor() {
        let raw = include_str!("../../../tests/fixtures/ansible_lint_default.txt");
        let shown = filter_output(raw, |_| Some("[full output: test capture]".into()));
        let input = tracking::estimate_tokens(raw);
        let output = tracking::estimate_tokens(&shown);
        assert!(
            output * 100 <= input * 80,
            "expected at least 20% estimated token savings, input={input}, output={output}\n{shown}"
        );
    }

    #[test]
    fn groups_real_colored_report_without_terminal_escapes() {
        let raw = include_str!("../../../tests/fixtures/ansible_lint_color.txt");
        let shown = filter_output(raw, |_| Some("[full output: test capture]".into()));
        assert!(shown.contains("name[missing]: 6"), "{shown}");
        assert!(!shown.contains('\x1b'), "terminal escape left in output");
        assert!(shown.contains("Failed: 24 failure(s)"));
    }

    #[test]
    fn strips_bel_terminated_hyperlinks_from_unrecognized_diagnostics() {
        let raw = "\x1b]8;;https://example.com\x07ERROR\x1b]8;;\x07 plugin failure\n";
        assert_eq!(filter_output(raw, |_| None), "ERROR plugin failure\n");
    }

    #[test]
    fn repeated_messages_are_compact_but_changed_messages_stay() {
        let raw = "name[missing]: First message\na.yml:2 Task details\n\nname[missing]: First message\nb.yml:3 Task details\n\nname[missing]: Another message (warning) # ignored\nc.yml:4 New details\n\n";
        assert_eq!(
            filter_output(raw, |_| None),
            "name[missing]: 3\n  a.yml:2 Task details First message\n  b.yml:3\n  c.yml:4 New details Another message (warning) # ignored"
        );
    }
}
