//! Compacts the console output of `TAP::Harness` (prove, `make test`, `dzil test`, `cover -test`).
//!
//! The harness prints one block per test file: a `t/foo.t .... ` header, then (in verbose mode)
//! the file's TAP and diagnostics, then a result. This filter keeps a block only when the file
//! failed or printed something that is not TAP, and within it keeps `not ok` lines, diagnostics
//! and anything unrecognised (die messages, compile errors, warnings). It drops passing `ok`
//! lines, plans, subtest headers, `# TODO` expected failures with their diagnostics, the wait
//! status next to the exit code, the `Non-zero exit status` lines that repeat it, the CPU
//! breakdown, and Test2's terminal-size hint and srand seed.
//!
//! Text outside a block (bailout reasons, the summary report, and in quiet mode the stderr
//! diagnostics that arrive before their file's header) passes through in order.

use regex::Regex;
use std::sync::LazyLock;

use super::utils::{current_dir, merge_failure_locations, passed_files_line, relative_to};
use crate::core::utils::strip_ansi;

/// `t/foo.t ....... ok`, `t/foo.t .. ` (a failing or verbose file), `t/foo.t .. skipped: why`.
/// `--timer` prefixes a `[hh:mm:ss]` stamp and appends `NN ms` to the status.
static FILE_STATUS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\[[\d:]+\] )?(\S.*?) \.{2,} ?(.*?)\s*$").unwrap());
/// A passing file's status, on its header line (quiet) or alone at the end of its block (verbose).
static FILE_OK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ok(?:\s+\d+ ms)?(?:\s+\(.*\))?$").unwrap());
static DUBIOUS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^Dubious, test returned (\d+) \(wstat \d+, 0x[0-9a-f]+\)$").unwrap()
});
/// The line that closes a failing file's block.
static FILE_RESULT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:Failed \d+/\d+ subtests|No subtests run|All \d+ subtests passed)\s*$").unwrap()
});
/// `t/x.t (Wstat: 768 (exited 3) Tests: 5 Failed: 3)` in the summary report.
static SUMMARY_FILE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\S.*?)\s+\(Wstat: \d+ (?:\((.*?)\) )?Tests: (\d+) Failed: (\d+)\)\s*$").unwrap()
});
static FILES_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Files=(\d+), Tests=(\d+),\s+(\d+) wallclock secs.*$").unwrap());
static TAP_OK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*ok \d+").unwrap());
static TAP_NOT_OK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*not ok \d+").unwrap());
static TAP_TODO_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)#\s*TODO\b").unwrap());
static TAP_PLAN_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*1\.\.\d+\s*$").unwrap());
static SUBTEST_HEADER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*# Subtest: ").unwrap());
static TODO_DIAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*#\s+Failed \(TODO\) test").unwrap());
/// Repeats the header's `Failed 3/5 subtests`.
static LOOKS_LIKE_FAILED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*#\s+Looks like you failed \d+ tests? of \d+").unwrap());
static BAILOUT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Bailout called\.\s+Further testing stopped:\s+(.*)$").unwrap());
static FURTHER_STOPPED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^FAILED--Further testing stopped: (.*)$").unwrap());
static DIAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*#").unwrap());
/// A diagnostic that starts a new, counted failure and so ends a TODO block.
static FAILURE_DIAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*#\s+(?:Failed test|Looks like)").unwrap());

/// Lines that carry nothing for the reader, anywhere.
fn is_noise(line: &str) -> bool {
    line.starts_with("(If this table is too small")
        || line.trim_start().starts_with("# Seeded srand with seed")
        || line.trim_start().starts_with("Non-zero exit status:")
        || (line.len() >= 3 && line.bytes().all(|b| b == b'-'))
}

/// TAP a passing test produces: nothing a reader needs.
fn is_passing_tap(line: &str) -> bool {
    TAP_OK_RE.is_match(line)
        || TAP_PLAN_RE.is_match(line)
        || SUBTEST_HEADER_RE.is_match(line)
        || (TAP_NOT_OK_RE.is_match(line) && TAP_TODO_RE.is_match(line))
}

/// One test file's block, from its header to its result line.
struct Block {
    file: String,
    lines: Vec<String>,
    exit: Option<String>,
}

impl Block {
    fn new(file: &str) -> Self {
        Self {
            file: file.to_string(),
            lines: Vec::new(),
            exit: None,
        }
    }
}

/// A file's entry in the harness's `Test Summary Report`.
struct SummaryEntry {
    file: String,
    /// `exited 3` or `Signal: KILL`, when the file did not exit cleanly.
    status: Option<String>,
    /// The indented lines under the entry: `Failed tests:  2-4`, `Parse errors: ...`, ...
    notes: Vec<String>,
}

/// A file header already written to the report, and whether its block shows `not ok` lines.
struct Header {
    file: String,
    index: usize,
    has_not_ok: bool,
}

/// The compacted report, built in one pass and assembled at the end, because the summary
/// report that comes last has lines that belong under headers written earlier.
#[derive(Default)]
struct Report {
    out: Vec<String>,
    headers: Vec<Header>,
    passed: Vec<String>,
    /// Where the summary report began, which is where its leftovers go.
    summary_at: Option<usize>,
    summaries: Vec<SummaryEntry>,
    /// `Files=7, Tests=17`, held until the `Result:` line so both print as one.
    files_line: Option<String>,
    all_successful: bool,
    result_seen: bool,
    failed: bool,
    bailout_reason: Option<String>,
}

impl Report {
    fn push_header(&mut self, file: &str, header: String, lines: Vec<String>) {
        self.headers.push(Header {
            file: file.to_string(),
            index: self.out.len(),
            has_not_ok: lines.iter().any(|l| TAP_NOT_OK_RE.is_match(l)),
        });
        self.out.push(header);
        self.out.extend(lines);
    }

    /// The file failed: its header carries the harness verdict, and every kept line follows.
    fn failed_block(&mut self, block: Block, verdict: &str) {
        self.failed = true;
        let exit = block
            .exit
            .map(|code| format!(" (exit {})", code))
            .unwrap_or_default();
        let header = format!("{}: {}{}", block.file, verdict, exit);
        self.push_header(&block.file, header, block.lines);
    }

    /// The file passed: only output that is not TAP or a comment is worth showing (a warning
    /// the test printed, say). Comments of a passing file are `note()` chatter.
    fn passed_block(&mut self, block: Block, verdict: &str) {
        self.passed.push(block.file.clone());
        let extra: Vec<String> = block
            .lines
            .into_iter()
            .filter(|l| !DIAG_RE.is_match(l))
            .collect();
        if !extra.is_empty() {
            let header = format!("{}: {}", block.file, verdict);
            self.push_header(&block.file, header, extra);
        }
    }

    /// The block ended without a verdict (a new header arrived, or the summary began): keep
    /// everything, since nothing says it passed.
    fn unfinished_block(&mut self, block: Block) {
        if block.lines.is_empty() && block.exit.is_none() {
            return;
        }
        let header = format!("{}:", block.file);
        self.push_header(&block.file, header, block.lines);
    }

    fn finish(mut self, cwd: Option<&str>) -> String {
        if !self.result_seen {
            match (self.all_successful, self.files_line.take()) {
                (true, Some(files)) => self.out.push(format!("All tests successful ({})", files)),
                (true, None) => self.out.push("All tests successful".to_string()),
                (false, Some(files)) => self.out.push(files),
                (false, None) => {}
            }
        }
        let summary_at = self.summary_at.unwrap_or(self.out.len());

        // Summary notes that add to what the file's header already says.
        let mut under_header: Vec<(usize, Vec<String>)> = Vec::new();
        let mut standalone: Vec<String> = Vec::new();
        for entry in self.summaries {
            let header = self.headers.iter().find(|h| h.file == entry.file);
            let notes: Vec<String> = entry
                .notes
                .into_iter()
                .map(|n| n.split_whitespace().collect::<Vec<_>>().join(" "))
                // `Failed tests: 2-4` repeats the `not ok` lines when the block has them.
                .filter(|n| !(n.starts_with("Failed test") && header.is_some_and(|h| h.has_not_ok)))
                .collect();
            match header {
                Some(h) => {
                    if !notes.is_empty() {
                        under_header
                            .push((h.index, notes.iter().map(|n| format!("  {}", n)).collect()));
                    }
                }
                None => {
                    let status = entry
                        .status
                        .map(|s| format!(" ({})", s))
                        .unwrap_or_default();
                    if notes.is_empty() {
                        if !status.is_empty() {
                            standalone.push(format!("{}{}", entry.file, status));
                        }
                    } else {
                        standalone.push(format!("{}{}: {}", entry.file, status, notes.join("; ")));
                    }
                }
            }
        }
        // On a clean run every file passed, so naming them says nothing new.
        let passed_line = if self.failed {
            passed_files_line(&self.passed)
        } else {
            None
        };

        // Passing files and leftover summary lines go where the summary report began.
        let mut at_summary: Vec<String> = passed_line.into_iter().chain(standalone).collect();
        let mut assembled: Vec<String> = Vec::with_capacity(self.out.len() + at_summary.len());
        for (i, line) in self.out.into_iter().enumerate() {
            if i == summary_at {
                assembled.append(&mut at_summary);
            }
            assembled.push(line);
            if let Some((_, notes)) = under_header.iter_mut().find(|(idx, _)| *idx == i) {
                assembled.append(notes);
            }
        }
        // The summary began after the last line (or never): append instead.
        assembled.append(&mut at_summary);

        merge_failure_locations(assembled)
            .into_iter()
            .map(|l| relative_to(&l, cwd))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Compact TAP::Harness console output, quiet (`prove`) or verbose (`prove -v`, `prove -v -m`).
pub fn filter_tap(raw: &str) -> String {
    filter_tap_in(raw, current_dir().as_deref())
}

/// [`filter_tap`] with paths under `cwd` shown relative to it.
pub fn filter_tap_in(raw: &str, cwd: Option<&str>) -> String {
    let clean = strip_ansi(raw);
    let mut report = Report::default();
    let mut block: Option<Block> = None;
    let mut in_todo_diag = false;
    let mut in_summary = false;

    for raw_line in clean.lines() {
        // Terminal progress rewrites (`t/foo.t .. 3/5`) arrive as `\r`-separated frames.
        let line = raw_line.rsplit('\r').next().unwrap_or(raw_line).trim_end();
        if line.is_empty() || is_noise(line) || LOOKS_LIKE_FAILED_RE.is_match(line) {
            continue;
        }

        if in_todo_diag {
            if DIAG_RE.is_match(line) && !FAILURE_DIAG_RE.is_match(line) {
                continue;
            }
            in_todo_diag = false;
        }
        if TODO_DIAG_RE.is_match(line) {
            in_todo_diag = true;
            continue;
        }
        if is_passing_tap(line) {
            continue;
        }

        if in_summary {
            if let Some(caps) = SUMMARY_FILE_RE.captures(line) {
                report.summaries.push(SummaryEntry {
                    file: caps[1].to_string(),
                    status: caps.get(2).map(|m| m.as_str().to_string()),
                    notes: Vec::new(),
                });
                continue;
            }
            if line.starts_with(char::is_whitespace) {
                if let Some(entry) = report.summaries.last_mut() {
                    entry.notes.push(line.trim().to_string());
                }
                continue;
            }
            in_summary = false;
        }

        if let Some(b) = block.as_mut() {
            if let Some(caps) = DUBIOUS_RE.captures(line) {
                b.exit = Some(caps[1].to_string());
                continue;
            }
            if FILE_RESULT_RE.is_match(line) {
                if let Some(b) = block.take() {
                    report.failed_block(b, line);
                }
                continue;
            }
            if FILE_OK_RE.is_match(line) {
                if let Some(b) = block.take() {
                    report.passed_block(b, line);
                }
                continue;
            }
            if line.starts_with("skipped:") {
                if let Some(b) = block.take() {
                    report.out.push(format!("{}: {}", b.file, line));
                }
                continue;
            }
        }

        if let Some(caps) = FILE_STATUS_RE.captures(line) {
            let (file, status) = (&caps[1], &caps[2]);
            if let Some(b) = block.take() {
                report.unfinished_block(b);
            }
            if status.is_empty() {
                block = Some(Block::new(file));
            } else if FILE_OK_RE.is_match(status) {
                report.passed.push(file.to_string());
            } else {
                // `skipped: reason`, or a status this filter does not know: keep it whole.
                report.out.push(line.to_string());
            }
            continue;
        }

        if let Some(b) = block.as_mut() {
            b.lines.push(line.to_string());
            continue;
        }

        if line == "Test Summary Report" {
            in_summary = true;
            report.summary_at.get_or_insert(report.out.len());
        } else if line == "All tests successful." {
            report.all_successful = true;
        } else if let Some(caps) = FILES_LINE_RE.captures(line) {
            report.files_line = Some(format!("Files={}, Tests={}", &caps[1], &caps[2]));
        } else if let Some(result) = line.strip_prefix("Result: ") {
            report.result_seen = true;
            report.summary_at.get_or_insert(report.out.len());
            let files = report.files_line.take();
            if result == "PASS" && report.all_successful {
                report.out.push(match files {
                    Some(f) => format!("All tests successful ({})", f),
                    None => "All tests successful".to_string(),
                });
            } else {
                report.failed |= result != "PASS";
                report.out.push(match files {
                    Some(f) => format!("{}, Result: {}", f, result),
                    None => line.to_string(),
                });
            }
        } else if let Some(caps) = BAILOUT_RE.captures(line) {
            report.failed = true;
            report.bailout_reason = Some(caps[1].trim().to_string());
            report.out.push(line.to_string());
        } else if line == "Test run interrupted!" && report.bailout_reason.is_some() {
            // The bailout line above already says so, with the reason.
        } else if let Some(caps) = FURTHER_STOPPED_RE.captures(line) {
            if report.bailout_reason.as_deref() != Some(caps[1].trim()) {
                report.out.push(line.to_string());
            }
        } else if let Some(caps) = DUBIOUS_RE.captures(line) {
            report
                .out
                .push(format!("Dubious, test returned {}", &caps[1]));
        } else {
            report.out.push(line.to_string());
        }
    }

    if let Some(b) = block.take() {
        report.unfinished_block(b);
    }
    report.finish(cwd)
}

/// Build-tool chatter that surrounds harness output under `make test`, `dzil test` and
/// `cover -test`: MakeMaker and Module::Build configure and copy steps. Errors from those tools
/// never match, so they still reach the reader.
pub fn is_build_noise(line: &str) -> bool {
    static BUILD_NOISE_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(concat!(
            r"^(?:Checking if your kit is complete\.\.\.|Looks good$|",
            r"Generating a Unix-style Makefile|Writing Makefile for |",
            r"Writing MYMETA\.yml and MYMETA\.json|Created MYMETA\.yml and MYMETA\.json|",
            r"Creating new 'Build' script for |Building \S+$|",
            r"Manifying \d+ pod documents?|Running Mkbootstrap for |chmod \d+ |",
            r#"cp \S+ blib/|PERL_DL_NONLAZY=1 |"?\S*perl"? "?-MExtUtils::Command::MM"#,
            r")"
        ))
        .unwrap()
    });
    BUILD_NOISE_RE.is_match(line)
}

/// Harness output wrapped in a build tool's own chatter (`dzil test`, `cover -test`): drop the
/// MakeMaker steps and whatever `tool_noise` names, then compact the harness output.
///
/// rtk captures these tools' stdout and stderr separately, and neither tool can be asked to
/// merge them, so the tests' diagnostics arrive after the summary. Each one still names its
/// test file and line.
pub fn filter_harness_run(raw: &str, tool_noise: impl Fn(&str) -> bool) -> String {
    let clean = strip_ansi(raw);
    let kept: Vec<&str> = clean
        .lines()
        .filter(|l| !is_build_noise(l) && !tool_noise(l))
        .collect();
    filter_tap(&kept.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    const FIXTURE_DIR: &str = "/home/user/Acme-RtkSample";

    fn tap(raw: &str) -> String {
        filter_tap_in(raw, Some(FIXTURE_DIR))
    }

    fn savings(input: &str, output: &str) -> f64 {
        100.0 - (count_tokens(output) as f64 / count_tokens(input) as f64 * 100.0)
    }

    const PASS_SUMMARY: &str =
        "t-pass/06-todo-skip.t: TODO passed: 1\nAll tests successful (Files=3, Tests=9)";

    #[test]
    fn test_quiet_pass_run() {
        let input = include_str!("../../../tests/fixtures/perl/prove_pass_raw.txt");
        assert_eq!(tap(input), PASS_SUMMARY);
    }

    #[test]
    fn test_merged_pass_run_matches_quiet_run() {
        let input = include_str!("../../../tests/fixtures/perl/prove_merged_pass_raw.txt");
        assert_eq!(tap(input), PASS_SUMMARY);
    }

    #[test]
    fn test_merged_fail_run() {
        let input = include_str!("../../../tests/fixtures/perl/prove_merged_fail_raw.txt");
        let expected = r#"t/02-fail-more.t: Failed 3/5 subtests (exit 3)
not ok 2 - two plus two is five (t/02-fail-more.t line 10)
#          got: '4'
#     expected: '5'
not ok 3 - sorted names structure (t/02-fail-more.t line 11)
#     Structures begin differing at:
#          $got->{list}[2] = 'c'
#     $expected->{list}[2] = 'd'
not ok 4 - twenty is big (t/02-fail-more.t line 16)
#                   'medium'
#     doesn't match '(?^:^big$)'
t/03-fail-t2.t: Failed 1/2 subtests (exit 1)
not ok 1 - pairs round-trips (t/03-fail-t2.t line 7)
# +------+-----+----+-------+
# | PATH | GOT | OP | CHECK |
# +------+-----+----+-------+
# | {b}  | 2   | eq | 3     |
# +------+-----+----+-------+
t/04-die.t: Failed 2/3 subtests (exit 2)
  Parse errors: Bad plan. You planned 3 tests but ran 1.
cannot open /nonexistent/rtk-sample.txt at lib/Acme/RtkSample.pm line 28.
# Looks like your test exited with 2 just after 1.
t/05-compile-error.t: No subtests run (exit 255)
  Parse errors: No plan found in TAP output
Global symbol "$undeclared_total" requires explicit package name (did you forget to declare "my $undeclared_total"?) at t/05-compile-error.t line 9.
Execution of t/05-compile-error.t aborted due to compilation errors.
Passed: t/00-load.t, t/01-basic.t, t/06-todo-skip.t
t/06-todo-skip.t: TODO passed: 1
Files=7, Tests=17, Result: FAIL"#;
        assert_eq!(tap(input), expected);
    }

    #[test]
    fn test_parallel_merged_run_keeps_blocks_whole() {
        let serial = include_str!("../../../tests/fixtures/perl/prove_merged_fail_raw.txt");
        let parallel = include_str!("../../../tests/fixtures/perl/prove_merged_fail_jobs_raw.txt");
        let mut serial_lines: Vec<&str> = Vec::new();
        let serial_out = tap(serial);
        serial_lines.extend(serial_out.lines());
        let parallel_out = tap(parallel);
        let mut parallel_lines: Vec<&str> = parallel_out.lines().collect();
        serial_lines.sort_unstable();
        parallel_lines.sort_unstable();
        assert_eq!(parallel_lines, serial_lines);
        assert!(parallel_out.contains(
            "t/04-die.t: Failed 2/3 subtests (exit 2)\n  Parse errors: Bad plan. You planned 3 tests but ran 1.\ncannot open /nonexistent/rtk-sample.txt"
        ));
    }

    #[test]
    fn test_quiet_fail_run_keeps_stderr_in_place() {
        let input = include_str!("../../../tests/fixtures/perl/prove_fail_raw.txt");
        let output = tap(input);
        assert!(output.starts_with(
            "# Failed test 'two plus two is five' (t/02-fail-more.t line 10)\n#          got: '4'\n"
        ));
        // Quiet runs print no `not ok` lines, so the failed test numbers stay.
        assert!(output.contains(
            "#     doesn't match '(?^:^big$)'\nt/02-fail-more.t: Failed 3/5 subtests (exit 3)\n  Failed tests: 2-4\n"
        ));
        assert!(output.contains(
            "# Looks like your test exited with 2 just after 1.\nt/04-die.t: Failed 2/3 subtests (exit 2)\n"
        ));
        assert!(output.ends_with(
            "Passed: t/00-load.t, t/01-basic.t, t/06-todo-skip.t\nt/06-todo-skip.t: TODO passed: 1\nFiles=7, Tests=17, Result: FAIL"
        ));
    }

    #[test]
    fn test_bailout_keeps_reason() {
        let input = include_str!("../../../tests/fixtures/perl/prove_merged_bailout_raw.txt");
        assert_eq!(
            tap(input),
            "Bailout called.  Further testing stopped:  database not reachable at localhost:5432\n\
             t-bail/01-bail.t: All 1 subtests passed (exit 255)\n\
             \x20 Parse errors: No plan found in TAP output\n\
             Files=1, Tests=1, Result: FAIL"
        );
    }

    #[test]
    fn test_todo_failure_diagnostics_dropped() {
        let input = "t/x.t .. \n\
            not ok 1 - float add # TODO rounding\n\
            #   Failed (TODO) test 'float add'\n\
            #   at t/x.t line 5.\n\
            #          got: '0.3'\n\
            not ok 2 - real one\n\
            #   Failed test 'real one'\n\
            #   at t/x.t line 9.\n\
            1..2\n\
            Dubious, test returned 1 (wstat 256, 0x100)\n\
            Failed 1/2 subtests\n";
        assert_eq!(
            tap(input),
            "t/x.t: Failed 1/2 subtests (exit 1)\nnot ok 2 - real one (t/x.t line 9)"
        );
    }

    #[test]
    fn test_passing_file_with_warning_is_shown() {
        let input = "t/w.t .. \n\
            # note: connecting\n\
            ok 1 - fine\n\
            Use of uninitialized value $x in concatenation at lib/W.pm line 3.\n\
            1..1\n\
            ok\n";
        assert_eq!(
            tap(input),
            "t/w.t: ok\nUse of uninitialized value $x in concatenation at lib/W.pm line 3."
        );
    }

    #[test]
    fn test_nested_subtest_failure_kept() {
        let input = "t/s.t .. \n\
            # Subtest: outer\n\
            \x20   ok 1 - fine\n\
            \x20   not ok 2 - inner broke\n\
            \x20   #   Failed test 'inner broke'\n\
            \x20   1..2\n\
            not ok 1 - outer\n\
            1..1\n\
            Dubious, test returned 1 (wstat 256, 0x100)\n\
            Failed 1/1 subtests\n";
        assert_eq!(
            tap(input),
            "t/s.t: Failed 1/1 subtests (exit 1)\n\
             \x20   not ok 2 - inner broke\n\
             \x20   #   Failed test 'inner broke'\n\
             not ok 1 - outer"
        );
    }

    #[test]
    fn test_skipped_file_is_kept() {
        assert_eq!(
            tap("t/net.t ..... skipped: no network\nt/a.t ....... ok\n"),
            "t/net.t ..... skipped: no network"
        );
        assert_eq!(
            tap("t/net.t .. \n1..0 # SKIP no network\nskipped: no network\n"),
            "t/net.t: skipped: no network"
        );
    }

    #[test]
    fn test_timer_and_ansi() {
        let input = "[14:32:33] t/a.t .. ok   20 ms ( 0.00 usr  0.00 sys +  0.05 cusr  0.01 csys =  0.06 CPU)\n\x1b[32mAll tests successful.\x1b[0m\n";
        assert_eq!(tap(input), "All tests successful");
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(tap(""), "");
    }

    #[test]
    fn test_unknown_lines_pass_through() {
        let input = "Can't locate Foo/Bar.pm in @INC\nsomething else\n";
        assert_eq!(tap(input), input.trim_end());
    }

    #[test]
    fn test_savings_on_merged_pass() {
        let input = include_str!("../../../tests/fixtures/perl/prove_merged_pass_raw.txt");
        let pct = savings(input, &tap(input));
        assert!(pct >= 60.0, "expected >=60% savings, got {:.1}%", pct);
    }

    #[test]
    fn test_savings_on_merged_fail() {
        // Failure output is mostly diagnostics, which are kept whole. The floor here guards
        // against a regression that starts passing the TAP stream through.
        let input = include_str!("../../../tests/fixtures/perl/prove_merged_fail_raw.txt");
        let pct = savings(input, &tap(input));
        assert!(pct >= 25.0, "expected >=25% savings, got {:.1}%", pct);
    }

    #[test]
    fn test_build_noise() {
        assert!(is_build_noise("Checking if your kit is complete..."));
        assert!(is_build_noise(
            "cp lib/Acme/RtkSample.pm blib/lib/Acme/RtkSample.pm"
        ));
        assert!(is_build_noise(
            r#"PERL_DL_NONLAZY=1 "/opt/perl5/bin/perl" "-MExtUtils::Command::MM" t/*.t"#
        ));
        assert!(!is_build_noise(
            "make: *** [Makefile:862: test_dynamic] Error 255"
        ));
        assert!(!is_build_noise("Result: FAIL"));
    }
}
