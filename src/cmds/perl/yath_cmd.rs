//! yath (Test2::Harness) filter: groups the per-job event stream and keeps only failing jobs.
//!
//! yath prints one tagged line per event, `( FAILED )  job  3    t/foo.t`, interleaved across
//! jobs. This filter groups lines by job, prints each failed job once with its failed assertions,
//! diagnostics, STDERR and the `REASON` lines yath gives for failing it, and drops passing jobs,
//! `PASS`/`PLAN`/`MEMORY`/`TIME`/`LAUNCH` events and the CPU figures. The job tables at the end
//! become one line per row, without the job UUIDs; the "jobs failed" table is dropped because
//! the failed jobs are already listed above it.

use regex::Regex;
use std::sync::LazyLock;

use super::utils::{
    current_dir, merge_failure_locations, passed_files_line, relative_to, unfiltered,
};
use crate::core::arg_tokenizer::{self, Dialect};
use crate::core::runner;
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::Result;

/// `[  FAIL  ]  job  3  + name`, `(  DIAG  )  job  3      text`, `< REASON >  job  3    text`.
static EVENT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[\[\(<]\s*([A-Z]+)\s*[\]\)>]\s+job\s+(\S+)(?:\s{2}\+ |\s{4}|\s+)(.*)$").unwrap()
});
/// `(INTERNAL)     1313696 yath-runner Aborting the test run...`
static INTERNAL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\(INTERNAL\)\s+\d+\s+(.*)$").unwrap());
static TABLE_HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^The following jobs (.+):$").unwrap());
static RESULT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*-->\s+Result: (\S+)\s+<--$").unwrap());
static SUMMARY_COUNT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(Fail Count|File Count|Assertion Count|Wall Time): (.*)$").unwrap()
});
static EXIT_REASON_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Test script returned error \(Err: (\d+)\)$").unwrap());
static ASSERT_REASON_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^Assertion failures were encountered \(Count: (\d+)\)$").unwrap()
});
/// Repeats the `failed assertions` count in the job's header.
static LOOKS_LIKE_FAILED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*Looks like you failed \d+ tests? of \d+").unwrap());

/// Commands other than `test`/`run` print something this filter does not parse.
const OTHER_COMMANDS: &[&str] = &[
    "help",
    "init",
    "start",
    "stop",
    "watch",
    "which",
    "reload",
    "replay",
    "failed",
    "times",
    "spec",
    "status",
    "kill",
    "abort",
    "speedtag",
    "auditor",
    "resources",
    "collector",
];

#[derive(Default)]
struct Job {
    file: Option<String>,
    failed: bool,
    lines: Vec<String>,
    stderr: Vec<String>,
    /// yath's reasons for failing the job, shortened for the header.
    reasons: Vec<String>,
}

/// `Test script returned error (Err: 3)` reads as `exit 3`, and the assertion count as
/// `3 failed assertions`; other reasons are kept as yath wrote them.
fn short_reason(reason: &str) -> String {
    if let Some(caps) = EXIT_REASON_RE.captures(reason) {
        return format!("exit {}", &caps[1]);
    }
    if let Some(caps) = ASSERT_REASON_RE.captures(reason) {
        let n = &caps[1];
        return format!("{} failed assertion{}", n, if n == "1" { "" } else { "s" });
    }
    reason.to_string()
}

fn job_line(tag: &str, text: &str) -> Option<String> {
    // Diagnostics keep their own indentation: it aligns `got:` with `expected:`.
    let text = text.trim_end();
    match tag {
        "PASS" | "PLAN" | "MEMORY" | "TIME" | "LAUNCH" | "RETRY" | "SKIP" | "TODO" | "PASSED"
        | "FAILED" => None,
        "REASON" => None,
        _ if text.trim_start().starts_with("Seeded srand with seed") => None,
        _ if LOOKS_LIKE_FAILED_RE.is_match(text) => None,
        "FAIL" => Some(format!("not ok - {}", text.trim_start())),
        "HALT" => Some(format!("halt: {}", text.trim_start())),
        _ => Some(text.to_string()),
    }
}

/// The cells of a `| a | b |` table row, trimmed.
fn table_cells(line: &str) -> Vec<&str> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .collect()
}

pub fn filter_yath(raw: &str) -> String {
    filter_yath_in(raw, current_dir().as_deref())
}

/// [`filter_yath`] with paths under `cwd` shown relative to it.
pub fn filter_yath_in(raw: &str, cwd: Option<&str>) -> String {
    let clean = strip_ansi(raw);
    // Summary counts, printed as one line at the end.
    let mut counts: Vec<(String, String)> = Vec::new();
    let mut result: Option<String> = None;
    let mut jobs: Vec<(String, Job)> = Vec::new();
    // Jobs in the order they finished, which is the order yath reported them.
    let mut finished: Vec<String> = Vec::new();
    let mut tail: Vec<String> = Vec::new();
    // The current `The following jobs <kind>:` table and its column names.
    let mut table: Option<(String, Vec<String>)> = None;

    for line in clean.lines() {
        let line = line.trim_end();

        if let Some(caps) = EVENT_RE.captures(line) {
            let (tag, id, text) = (&caps[1], &caps[2], &caps[3]);
            let idx = match jobs.iter().position(|(j, _)| j == id) {
                Some(i) => i,
                None => {
                    jobs.push((id.to_string(), Job::default()));
                    jobs.len() - 1
                }
            };
            let job = &mut jobs[idx].1;
            match tag {
                "FAILED" | "PASSED" => {
                    job.file = Some(text.trim().to_string());
                    job.failed = tag == "FAILED";
                    finished.push(id.to_string());
                }
                "STDERR" => job.stderr.push(text.trim_start().to_string()),
                "REASON" => job.reasons.push(short_reason(text.trim())),
                // Test2 names the failing assertion's location on its own line: attach it.
                "DEBUG"
                    if job.lines.last().is_some_and(|l| l.starts_with("not ok - "))
                        && text.trim().ends_with(|c: char| c.is_ascii_digit()) =>
                {
                    if let Some(last) = job.lines.last_mut() {
                        last.push_str(&format!(" ({})", text.trim()));
                    }
                    continue;
                }
                _ => {}
            }
            if let Some(kept) = job_line(tag, text) {
                job.lines.push(kept);
            }
            continue;
        }

        if let Some(caps) = TABLE_HEADING_RE.captures(line) {
            table = Some((caps[1].to_string(), Vec::new()));
            continue;
        }
        if let Some((kind, columns)) = table.as_mut() {
            if line.starts_with('+') {
                continue;
            }
            if line.starts_with('|') {
                let cells = table_cells(line);
                if columns.is_empty() {
                    columns.extend(cells.iter().map(|c| c.to_string()));
                } else if kind != "failed" && kind != "requested all testing be halted" {
                    // Failed jobs are listed above, and a halting job's reason is its
                    // `halt:` line.
                    let row: Vec<&str> = cells
                        .iter()
                        .zip(columns.iter())
                        .filter(|(cell, col)| col.as_str() != "Job ID" && !cell.is_empty())
                        .map(|(cell, _)| *cell)
                        .collect();
                    tail.push(format!("{}: {}", kind, row.join(" | ")));
                }
                continue;
            }
            table = None;
        }

        if let Some(caps) = INTERNAL_RE.captures(line) {
            // The bailout reason is already the halting job's `halt:` line.
            if !caps[1].contains("BAIL-OUT detected:") {
                tail.push(caps[1].to_string());
            }
        } else if let Some(caps) = RESULT_RE.captures(line) {
            result = Some(caps[1].to_string());
        } else if let Some(caps) = SUMMARY_COUNT_RE.captures(line) {
            counts.push((caps[1].to_string(), caps[2].to_string()));
        } else if !line.is_empty()
            && !line.contains("Yath Result Summary")
            && !line.trim_start().starts_with("CPU ")
            && !line.bytes().all(|b| b == b'-')
        {
            tail.push(line.to_string());
        }
    }

    let mut out: Vec<String> = Vec::new();
    let mut passed: Vec<String> = Vec::new();
    let mut any_failed = false;
    for id in &finished {
        let Some((_, job)) = jobs.iter().find(|(j, _)| j == id) else {
            continue;
        };
        let file = job.file.as_deref().unwrap_or(id);
        if job.failed {
            any_failed = true;
            if job.reasons.is_empty() {
                out.push(format!("{}: FAILED", file));
            } else {
                out.push(format!("{}: FAILED ({})", file, job.reasons.join(", ")));
            }
            out.extend(merge_failure_locations(job.lines.clone()));
        } else if !job.stderr.is_empty() {
            // A passing job that wrote to STDERR (a warning, say) is worth a look.
            out.push(format!("{}: PASSED", file));
            out.extend(job.stderr.iter().cloned());
        } else {
            passed.push(file.to_string());
        }
    }
    // On a clean run every file passed, so naming them says nothing new.
    if any_failed || result.as_deref() == Some("FAILED") {
        out.extend(passed_files_line(&passed));
    }
    out.extend(tail);

    let count = |name: &str| {
        counts
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    let mut summary: Vec<String> = Vec::new();
    if let Some(n) = count("File Count") {
        summary.push(format!("Files: {}", n));
    }
    if let Some(n) = count("Fail Count") {
        summary.push(format!("Failed: {}", n));
    }
    if let Some(n) = count("Assertion Count") {
        summary.push(format!("Assertions: {}", n));
    }
    if let Some(r) = &result {
        summary.push(format!("Result: {}", r));
    }
    if !summary.is_empty() {
        out.push(summary.join(", "));
    }
    out.iter()
        .map(|l| relative_to(l, cwd))
        .collect::<Vec<_>>()
        .join("\n")
}

fn filters_this_invocation(args: &[String]) -> bool {
    // yath's grammar is large and plugin-extended; only the command word matters here, and it
    // comes first. An option value mistaken for it can only cost compression: a path or a
    // value is never one of the other command names.
    let tokens = arg_tokenizer::tokenize_grammar(args, &|_, _| None, Dialect::Posix);
    match arg_tokenizer::before_dashdash(&tokens)
        .iter()
        .find(|t| t.is_free_positional())
    {
        Some(first) => !OTHER_COMMANDS.contains(&first.text),
        None => true,
    }
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("yath");
    cmd.args(args);
    // Term::Table wraps rows to the terminal width, which splits a file name across lines.
    if std::env::var_os("TABLE_TERM_SIZE").is_none() {
        cmd.env("TABLE_TERM_SIZE", "500");
    }

    if verbose > 0 {
        eprintln!("Running: yath {}", args.join(" "));
    }

    let filter: fn(&str) -> String = if filters_this_invocation(args) {
        filter_yath
    } else {
        unfiltered
    };

    runner::run_filtered(
        cmd,
        "yath",
        &args.join(" "),
        filter,
        runner::RunOptions::with_tee("yath"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    fn yath(raw: &str) -> String {
        filter_yath_in(raw, Some("/home/user/Acme-RtkSample"))
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_pass_run() {
        let input = include_str!("../../../tests/fixtures/perl/yath_pass_raw.txt");
        assert_eq!(yath(input), "Files: 3, Assertions: 19, Result: PASSED");
    }

    #[test]
    fn test_fail_run() {
        let input = include_str!("../../../tests/fixtures/perl/yath_fail_raw.txt");
        let output = yath(input);
        let expected_start = r#"t/02-fail-more.t: FAILED (exit 3, 3 failed assertions)
not ok - two plus two is five (t/02-fail-more.t line 10)
         got: '4'
    expected: '5'
not ok - sorted names structure (t/02-fail-more.t line 11)"#;
        assert!(output.starts_with(expected_start), "{}", output);
        assert!(output.contains(
            "t/04-die.t: FAILED (exit 2, Planned for 3 assertions, but saw 1)\n\
             cannot open /nonexistent/rtk-sample.txt at lib/Acme/RtkSample.pm line 28.\n\
             Looks like your test exited with 2 just after 1.\n"
        ));
        assert!(output.contains(
            "t/03-fail-t2.t: FAILED (exit 1, 1 failed assertion)\n\
             not ok - pairs round-trips (t/03-fail-t2.t line 7)\n\
             +------+-----+----+-------+\n| PATH | GOT | OP | CHECK |"
        ));
        assert!(!output.contains("Looks like you failed"));
        assert!(output.contains("Passed: t/00-load.t, t/01-basic.t, t/06-todo-skip.t\n"));
        assert!(!output.contains("419A8A68"));
        assert!(!output.contains("Seeded srand"));
        assert!(output.ends_with("Files: 7, Failed: 4, Assertions: 27, Result: FAILED"));
    }

    #[test]
    fn test_verbose_fail_drops_pass_and_resource_events() {
        let input = include_str!("../../../tests/fixtures/perl/yath_verbose_fail_raw.txt");
        let output = yath(input);
        assert!(!output.contains("object builds"));
        assert!(!output.contains("rss:"));
        assert!(!output.contains("Startup:"));
        assert!(!output.contains("Expected assertions"));
        assert!(output.contains(
            "t/02-fail-more.t: FAILED (exit 3, 3 failed assertions)\nnot ok - two plus two is five (t/02-fail-more.t line 10)\n"
        ));
    }

    #[test]
    fn test_bailout_keeps_halt_and_never_ran() {
        let input = include_str!("../../../tests/fixtures/perl/yath_bailout_raw.txt");
        let output = yath(input);
        assert!(output.starts_with(
            "t-bail/01-bail.t: FAILED (exit 255, Errors were encountered (Count: 1), No plan was declared)\n\
             halt: database not reachable at localhost:5432\n\
             yath-runner Aborting the test run...\n"
        ));
        // The reason is said once, in the halt line.
        assert_eq!(output.matches("database not reachable").count(), 1);
        assert!(output.contains("never ran: "));
    }

    #[test]
    fn test_passing_job_with_stderr_is_shown() {
        let input = "( STDERR )  job  1    Use of uninitialized value at lib/W.pm line 3.\n\
                     ( PASSED )  job  1    t/w.t\n";
        assert_eq!(
            yath(input),
            "t/w.t: PASSED\nUse of uninitialized value at lib/W.pm line 3."
        );
    }

    #[test]
    fn test_savings() {
        for input in [
            include_str!("../../../tests/fixtures/perl/yath_pass_raw.txt"),
            include_str!("../../../tests/fixtures/perl/yath_verbose_fail_raw.txt"),
        ] {
            let output = yath(input);
            let pct = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
            assert!(
                pct >= 50.0,
                "expected >=50% savings, got {:.1}%\n{}",
                pct,
                output
            );
        }
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(yath(""), "");
    }

    #[test]
    fn test_invocation_detection() {
        assert!(filters_this_invocation(&args(&[])));
        assert!(filters_this_invocation(&args(&["test", "-j4", "t"])));
        assert!(filters_this_invocation(&args(&["t/foo.t"])));
        assert!(!filters_this_invocation(&args(&["help"])));
        assert!(!filters_this_invocation(&args(&["start"])));
    }
}
