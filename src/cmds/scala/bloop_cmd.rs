//! Bloop (Scala build server) command filters.
//!
//! Bloop's CLI output differs from sbt: no `[info]`/`[error]`/`[success]`
//! prefixes, no ScalaTest summary line. Instead:
//!
//! * `compile` → `Compiling <p> (N Scala sources)` / `Compiled <p> (Nms)`, and
//!   on failure `[E] ...` diagnostics ending in `[E] Failed to compile '<p>'`.
//! * `test` → per-suite blocks ending in `N tests, N passed[, N failed]`, then
//!   a `Total duration` / `All N test suites passed.` footer.
//! * `run` → the program's own output wrapped in the compile-server banner.

use crate::core::args_utils;
use crate::core::display_helpers::format_duration;
use crate::core::runner::{self, RunOptions};
use crate::core::truncate::{CAP_ERRORS, CAP_LIST, CAP_WARNINGS};
use crate::core::utils::{resolved_command, strip_ansi, truncate};
use anyhow::Result;
use regex::Regex;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::sync::LazyLock;

/// `Compiling <project> (N Scala sources)` — N captured. A mixed module also
/// lists Java sources (`(67 Scala sources and 8 Java sources)`); the optional
/// second group captures that count so both are summed into the total.
static COMPILING_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^Compiling \S+ \((\d+) Scala sources?(?: and (\d+) Java sources?)?\)").unwrap()
});

/// `Compiled <project> (Nms)` — value and unit captured. Bloop reports ms
/// today; the seconds form (`(1.5s)`) is accepted too so a format change
/// can't silently drop a project from the source/time tally.
static COMPILED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Compiled \S+ \((\d+(?:\.\d+)?)(ms|s)\)").unwrap());

/// Error diagnostic header: `[E] [E2] path/File.scala:LINE:COL` — loc captured.
static DIAG_HEADER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[E\] \[E\d+\] (.+:\d+:\d+)\s*$").unwrap());

/// Warning diagnostic header: `[W]  [E1] path/File.scala:LINE:COL` — loc
/// captured. Same shape as the error header but `[W]`-marked (bloop pads it
/// with an extra space); body lines are `[W]`-prefixed.
static WARN_HEADER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[W\]\s+\[E\d+\] (.+:\d+:\d+)\s*$").unwrap());

/// Source-snippet line inside a diagnostic: `[E]      L42:   def y = ...`.
/// Reaching one ends the human-readable message for the current diagnostic.
static DIAG_SOURCE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^L\d+:").unwrap());

/// Runs of 2+ spaces inside a diagnostic message. bloop right-pads labels
/// (`Found:    (...)`); collapse them so the joined one-liner reads cleanly.
static MULTISPACE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" {2,}").unwrap());

/// Per-suite tally line, e.g. `4 tests, 2 passed, 2 failed`. Only the leading
/// `N tests,` shape matches here; individual counts are pulled out with the
/// `N_*_RE` regexes so the order and presence of trailing terms don't matter.
static TALLY_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+ tests?,").unwrap());
static N_PASSED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d+) passed").unwrap());
static N_FAILED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d+) failed").unwrap());
static N_IGNORED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d+) ignored").unwrap());

/// specs2 counts *errored* examples (uncaught exceptions) separately from
/// *failed* (assertion) ones: `4 tests, 3 passed, 1 errors`. Without this an
/// error-only suite filters to all-green — a false negative.
static N_ERRORS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d+) errors?").unwrap());

/// A Java stack frame's source location: `pkg.Class.method(File.scala:line)`
/// — captures the inner `File.scala:line`. Used to attach a throw-site
/// location to an uncaught-exception failure (ScalaTest / zio-test `error`).
static PAREN_LOC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(([^()]+:\d+)\)").unwrap());

/// A line that looks like a JVM throwable: a dotted FQ class name optionally
/// followed by `: message`. The zio-test *defect* body is exactly this shape
/// following a `- <name>` bullet; requiring it stops ScalaTest FunSpec scope
/// headers (`#withName`, `* <summary>`) — which also follow `- <name>` —
/// from being mis-read as defect failures.
static EXCEPTION_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\w$]+(?:\.[\w$]+)+\s*(?::|$)").unwrap());

/// A specs2 `[E]` reason often trails junk after the first `(File:line)`
/// (e.g. `… / by zero (Calculator.scala:8)example.Calculator$.div(…)`);
/// keep everything up to and including that first location.
static SPECS2_REASON_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.*?\([^()]+:\d+\))").unwrap());

/// munit failure marker (bloop test runner):
/// `==> X <test name> <dur> <ExceptionType>: <location-or-message>`
static MUNIT_FAIL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^==> X (.+?) \d+(?:\.\d+)?m?s (\S+): (.*)$").unwrap());

/// A bare `path:line[:col]` location (no whitespace) — used to shorten an
/// absolute source path to its basename for compact failure details.
static LOCATION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\S+:\d+(?::\d+)?$").unwrap());

/// Footer success line: `All N test suites passed.` — N captured.
static ALL_SUITES_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^All (\d+) test suites? passed\.").unwrap());

/// Footer timing line: `Total duration: 0.41s` — value captured.
static TOTAL_DURATION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Total duration: (\S+)").unwrap());

/// Unit shapes bloop prints for `Total duration:`, normalized via
/// `parse_duration_ms`: plain ms, fractional seconds, minutes+seconds.
static DUR_MS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d+)ms$").unwrap());
static DUR_SEC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+(?:\.\d+)?)s$").unwrap());
static DUR_MIN_SEC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+)m(\d+(?:\.\d+)?)s$").unwrap());

/// ziotest run summary: `2 tests passed. ...`. Marks the end of the per-test
/// tree; everything after is a redundant re-dump, so detail capture stops here.
static ZIO_SUMMARY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+ tests? passed\.").unwrap());

/// Compile-server banner / footer noise to drop from `run` output.
static NOISE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(Starting compilation server|Bloop server started\.|The test execution was successfully closed\.|=+)$",
    )
    .unwrap()
});

pub fn run_test(args: &[String], verbose: u8) -> Result<i32> {
    run_bloop_filtered("test", args, verbose, filter_bloop_test)
}

pub fn run_compile(args: &[String], verbose: u8) -> Result<i32> {
    run_bloop_filtered("compile", args, verbose, filter_bloop_compile)
}

pub fn run_run(args: &[String], verbose: u8) -> Result<i32> {
    run_bloop_filtered("run", args, verbose, filter_bloop_run)
}

/// Build and run `bloop <subcommand> <args>`. The `--` separator clap strips
/// from `trailing_var_arg` args is restored first so test-runner flags after it
/// (`bloop test proj -- -o file`) reach bloop.
fn run_bloop_filtered<F>(subcommand: &str, args: &[String], verbose: u8, filter: F) -> Result<i32>
where
    F: Fn(&str) -> String,
{
    let args = args_utils::restore_double_dash(args);

    let mut cmd = resolved_command("bloop");
    cmd.arg(subcommand);
    for arg in &args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: bloop {} {}", subcommand, args.join(" "));
    }

    runner::run_filtered(
        cmd,
        &format!("bloop {}", subcommand),
        &args.join(" "),
        filter,
        RunOptions::with_tee(&format!("bloop_{}", subcommand)),
    )
}

/// Passthrough for unsupported bloop subcommands (projects, clean, about, …):
/// run unfiltered, track usage. `args[0]` is the subcommand.
pub fn run_other(args: &[OsString], verbose: u8) -> Result<i32> {
    runner::run_passthrough("bloop", args, verbose)
}

/// A compiler diagnostic: its `file:line:col` location and a flattened message.
struct Diagnostic {
    location: String,
    message: String,
}

/// Caps on inline compile diagnostics before the rest collapse to `+N more`.
/// Errors get a higher cap than warnings — compiler errors cascade, so a strong
/// signal beats dumping the whole list, and warnings are rarely acted on in
/// bulk. Full output stays in the teed log.
const MAX_ERRORS: usize = CAP_ERRORS;
const MAX_WARNINGS: usize = CAP_WARNINGS;

/// Per-diagnostic message length cap. Generous because the message is the signal
/// (the diagnosis plus actionable detail, e.g. the missing-case list of a
/// non-exhaustive `match`), but bounded so a giant Scala 3 type mismatch can't
/// blow up the line. Full text stays in the teed log.
const MAX_DIAG_MSG: usize = 200;

/// Filter `bloop compile` output.
///
/// Success → `bloop compile: N sources (M projects, Ts)`; with warnings,
/// `bloop compile: N sources, W warnings (Ts)` plus an indented warning list.
/// No work to do (already up-to-date) → `bloop compile: up-to-date`.
/// Failure → indented list of `file:line:col message` error diagnostics.
fn filter_bloop_compile(output: &str) -> String {
    let output = strip_ansi(output);
    let output = output.as_str();

    let mut total_sources: u32 = 0;
    let mut projects: u32 = 0;
    let mut total_ms: u64 = 0;
    let mut failed = false;

    let mut errors: Vec<Diagnostic> = Vec::new();
    let mut warnings: Vec<Diagnostic> = Vec::new();
    // The diagnostic currently being parsed: its location, accumulated message
    // lines, and whether it's a warning (`[W]`) rather than an error (`[E]`).
    let mut cur_loc: Option<String> = None;
    let mut cur_msg: Vec<String> = Vec::new();
    let mut cur_is_warn = false;

    // Close the in-progress diagnostic, routing it to errors or warnings.
    macro_rules! flush_diag {
        () => {
            if let Some(location) = cur_loc.take() {
                let diag = Diagnostic {
                    location,
                    message: MULTISPACE_RE.replace_all(&cur_msg.join(" "), " ").into_owned(),
                };
                if cur_is_warn {
                    warnings.push(diag);
                } else {
                    errors.push(diag);
                }
            }
            cur_msg.clear();
        };
    }

    for line in output.lines() {
        if let Some(caps) = COMPILING_RE.captures(line) {
            total_sources += caps[1].parse::<u32>().unwrap_or(0);
            if let Some(java) = caps.get(2) {
                total_sources += java.as_str().parse::<u32>().unwrap_or(0);
            }
            continue;
        }
        if let Some(caps) = COMPILED_RE.captures(line) {
            projects += 1;
            let value: f64 = caps[1].parse().unwrap_or(0.0);
            let ms = if &caps[2] == "s" { value * 1000.0 } else { value };
            total_ms += ms as u64;
            continue;
        }

        if let Some(caps) = DIAG_HEADER_RE.captures(line) {
            flush_diag!();
            cur_loc = Some(caps[1].to_string());
            cur_is_warn = false;
            continue;
        }
        if let Some(caps) = WARN_HEADER_RE.captures(line) {
            flush_diag!();
            cur_loc = Some(caps[1].to_string());
            cur_is_warn = true;
            continue;
        }

        // Any other line is only meaningful while parsing a diagnostic body.
        if cur_loc.is_some() {
            let prefix = if cur_is_warn { "[W]" } else { "[E]" };
            let body = line.strip_prefix(prefix).map(str::trim);
            match body {
                // A blank diagnostic line is interior spacing, not a terminator:
                // exhaustiveness warnings separate the headline (`match may not
                // be exhaustive.`) from the actionable line (`It would fail on
                // <case>`) with blank `[W]` lines — keep parsing through it.
                Some("") => {}
                // Source snippet / caret / per-file summary end the message.
                Some(text)
                    if DIAG_SOURCE_RE.is_match(text)
                        || text.starts_with('^')
                        || text.starts_with("Failed to compile")
                        || text.contains(" [E") =>
                {
                    flush_diag!();
                }
                Some(text) => cur_msg.push(text.to_string()),
                // A line without the diagnostic's prefix (e.g. next `Compiled`)
                // ends the diagnostic.
                None => {
                    flush_diag!();
                }
            }
        }

        if line.starts_with("[E] Failed to compile") {
            failed = true;
        }
    }
    flush_diag!();

    // Nothing recompiled and no diagnostics: nothing needed recompiling.
    if projects == 0 && total_sources == 0 && errors.is_empty() && warnings.is_empty() && !failed {
        return "bloop compile: up-to-date".to_string();
    }

    // All outcomes build the same header shape: `N sources[, M errors|warnings]
    // (duration)`.
    let is_error = failed || !errors.is_empty();
    let mut summary = format!("bloop compile: {} sources", total_sources);
    if is_error {
        let n = errors.len().max(1);
        summary.push_str(&format!(", {n} error{}", if n == 1 { "" } else { "s" }));
    }
    // Warnings are always counted in the header — even alongside errors — so a
    // failing build doesn't silently hide them. (Only error locations are listed
    // below; warnings often clear once the errors are fixed.)
    if !warnings.is_empty() {
        let n = warnings.len();
        summary.push_str(&format!(", {n} warning{}", if n == 1 { "" } else { "s" }));
    }
    if projects > 1 {
        summary.push_str(&format!(
            " ({} projects, {})",
            projects,
            format_duration(total_ms)
        ));
    } else if projects == 1 || total_ms > 0 {
        // Omit the time entirely if no `Compiled` line was emitted (e.g. a build
        // that failed before finishing) rather than printing a bogus `0ms`.
        summary.push_str(&format!(" ({})", format_duration(total_ms)));
    }

    // Keep the diagnoses (the diagnosis is the signal; drop only the framing).
    // Errors take precedence over warnings.
    let (diags, label, cap) = if is_error {
        (&errors, "errors", MAX_ERRORS)
    } else {
        (&warnings, "warnings", MAX_WARNINGS)
    };
    if !diags.is_empty() {
        summary.push('\n');
        for d in diags.iter().take(cap) {
            summary.push_str(&format!(
                "  {} {}\n",
                shorten_diag_location(&d.location),
                truncate(&d.message, MAX_DIAG_MSG)
            ));
        }
        if diags.len() > cap {
            summary.push_str(&format!("\n… +{} more {label}\n", diags.len() - cap));
            // A capped warning list still needs a recovery path on a successful
            // build (exit 0), where the failure tee never fires.
            if let Some(hint) =
                crate::core::tee::force_tee_hint(output, &format!("bloop-compile-{label}"))
            {
                summary.push_str(&format!("{hint}\n"));
            }
        }
    }

    summary.trim().to_string()
}

/// A failed test with its name and a few detail lines (assertion diff, the
/// exception + location, or the first user stack frame).
struct TestFailure {
    name: String,
    details: Vec<String>,
}

/// Which test framework's detail block we are currently inside.
#[derive(PartialEq, Default)]
enum DetailMode {
    #[default]
    None,
    /// ScalaTest: indented `[info]`-style lines under a `*** FAILED ***` header.
    ScalaTest,
    /// munit: the body following a `==> X` marker (source snippet, `=> Diff`,
    /// stack frames).
    Munit,
    /// ziotest: the body following a `✗ <reason>` marker; the throw-site
    /// location is the next `at <path>:line` line.
    Ziotest,
}

const MAX_DETAIL_LINES: usize = 6;

/// Width cap for a rendered failure name / detail line in the summary, beyond
/// which it is truncated (the full text stays in the teed log).
const MAX_FAILURE_WIDTH: usize = 120;

/// Cap on the number of failed tests rendered inline; the rest are summarized as
/// `+N more` (full bodies remain in the teed log).
const MAX_FAILURES: usize = CAP_LIST;

/// Filter `bloop test` output. Handles ScalaTest (`*** FAILED ***`), munit
/// (`==> X ... => Diff`), specs2 (`x <name>` + `[E] <reason>`) and ziotest
/// (`- <name>` + `✗ <reason>`) failure formats, which bloop streams inline
/// before the per-suite tally.
///
/// Test counts are summed across the per-suite `N tests, N passed[, ...]`
/// tallies — more useful than bloop's suite-level footer count.
///
/// Pass → `bloop test: N passed (M suites, Ts)`.
/// Fail → `bloop test: P passed, F failed (Ts)` + each failing test with its
/// assertion diff / exception and source location inline.
fn filter_bloop_test(output: &str) -> String {
    let output = strip_ansi(output);
    let output = output.as_str();

    let mut run = TestRun::default();
    for line in output.lines() {
        run.feed_line(line);
    }
    run.finish(output)
}

/// Accumulated state for parsing one `bloop test` stream. The four frameworks
/// (munit / ScalaTest / specs2 / ziotest) interleave on one stream and share the
/// framework-agnostic tally/footer lines, so the parse is one ordered pass:
/// `feed_line` dispatches each line through the framework stages, `finish`
/// renders the summary. Each framework's logic lives in its own `*_marker` /
/// `*_detail` method.
#[derive(Default)]
struct TestRun {
    // Summed tallies + footer values (framework-agnostic).
    passed: u32,
    failed: u32,
    ignored: u32,
    suites: Option<u32>,
    duration: Option<String>,
    saw_tally: bool,
    no_tests: bool,

    failures: Vec<TestFailure>,
    /// Which framework's detail block the parser is currently inside.
    mode: DetailMode,

    // munit detail sub-state for the failure currently being parsed.
    capturing_diff: bool,
    /// Inside a munit `Clues { }` block (the `clue(...)` diagnostic values).
    capturing_clues: bool,
    /// A structured body (diff, clue pairs, or ScalaCheck counterexample) was
    /// kept for this failure; suppresses the framework-internal `at …` fallback.
    body_detail_seen: bool,
    at_frames: u8,

    // ScalaTest property/table-check sub-state: while inside the `Occurred at
    // table row … (` block, collect the `name = value` rows so the failing
    // inputs (`[a=10, b=7, sum=99]`) can be appended to the assertion.
    in_table_row: bool,
    row_values: Vec<String>,

    // ziotest sub-state: the pending failed-test name (most recent `- <name>`
    // tree bullet) and a flag set once the run summary ends the tree, after
    // which the redundant failure re-dump must not be captured again.
    last_dash_name: Option<String>,
    zio_done: bool,

    // specs2 sub-state: indices of `x <name>` failures still awaiting their
    // reason. specs2 prints reasons on stderr, which rtk appends after all stdout
    // — detached from the marker — so reasons match names FIFO rather than by
    // adjacency (works interleaved or concatenated).
    specs2_pending: VecDeque<usize>,
}

impl TestRun {
    /// Route one raw line through the framework stages. Each `_marker` stage
    /// returns `true` when it consumes the line. The order is significant and
    /// must not be reshuffled — e.g. the specs2 marker peels its `[E] ` tag
    /// before the specs2 reason handler runs, or a marker is eaten as a reason.
    /// A line nothing claims falls through to the dash-name tracker and the
    /// active detail block.
    fn feed_line(&mut self, line: &str) {
        let trimmed = line.trim();

        if self.handle_tally_or_footer(trimmed)
            || self.handle_munit_marker(trimmed)
            || self.handle_scalatest_header(trimmed)
            || self.handle_zio_summary(trimmed)
            || self.handle_specs2_marker(line, trimmed)
            || self.handle_specs2_reason(trimmed)
            || self.handle_zio_assertion(trimmed)
            || self.handle_zio_defect(line, trimmed)
        {
            return;
        }

        self.track_dash_name(line, trimmed);
        self.handle_detail(line, trimmed);
    }

    /// Tally / footer lines (`N tests, …`, `All N test suites passed.`, `Total
    /// duration:`, `No test suites were run.`). All are framework-agnostic and
    /// always end any in-progress detail block.
    fn handle_tally_or_footer(&mut self, trimmed: &str) -> bool {
        if TALLY_RE.is_match(trimmed) {
            self.passed += first_capture(&N_PASSED_RE, trimmed);
            // specs2 reports errored examples as `N errors`, not `N failed` —
            // both are test failures for our purposes.
            self.failed +=
                first_capture(&N_FAILED_RE, trimmed) + first_capture(&N_ERRORS_RE, trimmed);
            self.ignored += first_capture(&N_IGNORED_RE, trimmed);
            self.saw_tally = true;
            self.mode = DetailMode::None;
            return true;
        }
        if let Some(caps) = ALL_SUITES_RE.captures(trimmed) {
            self.suites = caps[1].parse().ok();
            self.mode = DetailMode::None;
            return true;
        }
        if let Some(caps) = TOTAL_DURATION_RE.captures(trimmed) {
            self.duration = Some(caps[1].to_string());
            self.mode = DetailMode::None;
            return true;
        }
        if trimmed == "No test suites were run." {
            self.no_tests = true;
            self.mode = DetailMode::None;
            return true;
        }
        false
    }

    /// munit failure marker `==> X <name> <dur> <Exc>: <loc>` (opens a Munit
    /// detail block), or any other `==>` marker (ignored/success — just ends
    /// the current detail block).
    fn handle_munit_marker(&mut self, trimmed: &str) -> bool {
        if let Some(caps) = MUNIT_FAIL_RE.captures(trimmed) {
            let name = caps[1].trim().to_string();
            let detail = format!("{}: {}", &caps[2], munit_marker_detail(caps[3].trim()));
            self.failures.push(TestFailure { name, details: vec![detail] });
            self.mode = DetailMode::Munit;
            self.capturing_diff = false;
            self.capturing_clues = false;
            self.body_detail_seen = false;
            self.at_frames = 0;
            return true;
        }
        if trimmed.starts_with("==>") {
            self.mode = DetailMode::None;
            return true;
        }
        false
    }

    /// ScalaTest failure header: `- <name> *** FAILED ***` (opens a ScalaTest
    /// detail block).
    fn handle_scalatest_header(&mut self, trimmed: &str) -> bool {
        if trimmed.contains("*** FAILED ***") {
            let name = trimmed
                .strip_suffix("*** FAILED ***")
                .unwrap_or(trimmed)
                .trim()
                .trim_start_matches('-')
                .trim()
                .to_string();
            self.failures.push(TestFailure { name, details: Vec::new() });
            self.mode = DetailMode::ScalaTest;
            self.in_table_row = false;
            self.row_values.clear();
            return true;
        }
        false
    }

    /// ziotest run summary: ends the per-test tree (a redundant re-dump of the
    /// same failures follows, which must not be captured again).
    fn handle_zio_summary(&mut self, trimmed: &str) -> bool {
        if ZIO_SUMMARY_RE.is_match(trimmed) {
            self.zio_done = true;
            self.mode = DetailMode::None;
            return true;
        }
        false
    }

    /// specs2 failure marker: `  x <name>` (assertion) or `  ! <name>` (errored
    /// example). Both await a `[E]` reason that arrives later.
    ///
    /// When bloop routes the whole specs2 report to stderr it tags every line
    /// `[E] `; strip that tag before matching so `[E] x <name>` is still
    /// recognized. The tag must be peeled here, ahead of the `[E]` *reason*
    /// handler, or the marker is eaten as a reason.
    fn handle_specs2_marker(&mut self, line: &str, trimmed: &str) -> bool {
        let marker_src = trimmed
            .strip_prefix("[E]")
            .map(str::trim_start)
            .unwrap_or(trimmed);
        let tagged = line.starts_with(' ') || trimmed.starts_with("[E]");
        if tagged {
            if let Some(name) = marker_src
                .strip_prefix("x ")
                .or_else(|| marker_src.strip_prefix("! "))
            {
                self.specs2_pending.push_back(self.failures.len());
                self.failures.push(TestFailure {
                    name: name.trim().to_string(),
                    details: Vec::new(),
                });
                return true;
            }
        }
        false
    }

    /// specs2 reason: `[E]   N != M (file:line)` — attach to the next `x`/`!`
    /// failure still missing one (the `[E]  ` blank padding is kept pending).
    /// See `specs2_pending` for why this isn't adjacency-based.
    fn handle_specs2_reason(&mut self, trimmed: &str) -> bool {
        if self.specs2_pending.is_empty() {
            return false;
        }
        if let Some(rest) = trimmed.strip_prefix("[E]") {
            let reason = rest.trim();
            // An errored example trails its exception with bare stack frames
            // (`pkg.Class.method(File.scala:line)`, no spaces). Only the first
            // human-readable reason line (which has spaces) attaches; skipping
            // frames keeps the FIFO aligned with one reason per failure even
            // when several errors stack up (live concat order).
            if !reason.is_empty() && reason.contains(' ') {
                if let Some(idx) = self.specs2_pending.pop_front() {
                    self.failures[idx].details.push(clean_specs2_reason(reason));
                }
            }
            return true;
        }
        false
    }

    /// ziotest assertion marker: `✗ <reason>` (name = the preceding `- <name>`
    /// bullet).
    fn handle_zio_assertion(&mut self, trimmed: &str) -> bool {
        if self.zio_done {
            return false;
        }
        if let Some(reason) = trimmed.strip_prefix('✗') {
            let name = self.last_dash_name.take().unwrap_or_else(|| "test".to_string());
            self.failures.push(TestFailure {
                name,
                details: vec![reason.trim().to_string()],
            });
            self.mode = DetailMode::Ziotest;
            return true;
        }
        false
    }

    /// ziotest defect: a `- <name>` bullet followed directly by an exception
    /// line (`pkg.Exception: msg`) instead of a `✗` assertion. The throw site is
    /// the next `at …(File.scala:line)` frame.
    fn handle_zio_defect(&mut self, line: &str, trimmed: &str) -> bool {
        if self.zio_done || self.mode == DetailMode::Ziotest {
            return false;
        }
        if let Some(name) = self.last_dash_name.clone() {
            if line.starts_with(' ')
                && !trimmed.is_empty()
                && !trimmed.starts_with('-')
                && !trimmed.starts_with('+')
                && !trimmed.starts_with("at ")
                // The body must look like a throwable (`pkg.Class: msg`), or
                // ScalaTest FunSpec scope headers (`#withName`) that also follow
                // a `- <name>` bullet get mis-read as defects.
                && EXCEPTION_LINE_RE.is_match(trimmed)
            {
                self.last_dash_name = None;
                self.failures.push(TestFailure {
                    name,
                    details: vec![trimmed.to_string()],
                });
                self.mode = DetailMode::Ziotest;
                return true;
            }
        }
        false
    }

    /// Remember the most recent ziotest tree bullet (`  - <name>`) as the
    /// pending failure name; consumed when the following `✗` line is seen.
    /// Harmless for other frameworks — no `✗` follows their `- ` bullets.
    fn track_dash_name(&mut self, line: &str, trimmed: &str) {
        if !self.zio_done && line.starts_with(' ') {
            if let Some(name) = trimmed.strip_prefix("- ") {
                self.last_dash_name = Some(name.trim().to_string());
            }
        }
    }

    /// Dispatch a non-marker line to the active framework's detail handler.
    fn handle_detail(&mut self, line: &str, trimmed: &str) {
        match self.mode {
            DetailMode::Munit => self.munit_detail(trimmed),
            DetailMode::ScalaTest => self.scalatest_detail(line, trimmed),
            DetailMode::Ziotest => self.ziotest_detail(trimmed),
            DetailMode::None => {}
        }
    }

    /// munit detail body: the `Clues { }` block, the `=> Diff` hunk, ScalaCheck
    /// counterexample lines, or — absent any structured body — the first
    /// throw-site frame. Source snippets / framework frames are dropped.
    fn munit_detail(&mut self, trimmed: &str) {
        // End the block when the suite/run footer is reached.
        if trimmed.starts_with("Test run ")
            || trimmed.starts_with("Execution took")
            || trimmed.ends_with("finished")
        {
            self.mode = DetailMode::None;
            return;
        }
        if self.capturing_clues {
            // Inside a munit `Clues { }` block: keep each `expr: Type = value`
            // pair (the whole reason `clue` exists) until the closing brace.
            if trimmed == "}" {
                self.capturing_clues = false;
            } else if !trimmed.is_empty() {
                push_detail(&mut self.failures, trimmed);
                self.body_detail_seen = true;
            }
        } else if trimmed == "Clues {" {
            self.capturing_clues = true;
        } else if trimmed.starts_with("=> Diff") {
            push_detail(&mut self.failures, trimmed);
            self.capturing_diff = true;
            self.body_detail_seen = true;
        } else if self.capturing_diff {
            // Keep the whole diff hunk — context lines as well as `-`/`+` lines —
            // so a collection diff retains its full obtained-vs-expected body
            // (a leading context line like `JArray(` must not end capture early).
            // The hunk ends at the trailing stack frame or a blank line.
            if trimmed.is_empty() || trimmed.starts_with("at ") {
                self.capturing_diff = false;
            } else {
                push_detail(&mut self.failures, trimmed);
            }
        } else if trimmed.starts_with("Falsified after")
            || trimmed.starts_with("> ARG_")
            || trimmed.starts_with("Failing seed:")
        {
            // ScalaCheck property failure: the counterexample (`> ARG_0: …`),
            // the falsification tally, and the failing seed are the only
            // actionable parts — keep them.
            push_detail(&mut self.failures, trimmed);
            self.body_detail_seen = true;
        } else if !self.body_detail_seen && self.at_frames < 1 && trimmed.starts_with("at ") {
            // No structured body (a plain exception): keep the throw site.
            push_detail(&mut self.failures, trimmed);
            self.at_frames += 1;
        }
        // Everything else (source snippet, `values are not the same`,
        // `=> Obtained`, framework stack frames) is dropped.
    }

    /// ScalaTest detail body: indented lines under a `*** FAILED ***` header —
    /// the `Occurred at table row` inputs, the `Message:`/`Location:` overrides,
    /// the first reason line, or an uncaught-exception throw site.
    fn scalatest_detail(&mut self, line: &str, trimmed: &str) {
        // Detail lines are indented and not a new test bullet.
        if !(line.starts_with(' ') && !trimmed.is_empty() && !trimmed.starts_with('-')) {
            self.mode = DetailMode::None;
            return;
        }
        let Some(last) = self.failures.last_mut() else {
            return;
        };
        if self.in_table_row {
            // Inside the `Occurred at table row … (` block: each `name = value`
            // row is a failing input. Collect them until the closing `)`, then
            // fold a compact `[a=10, b=7, sum=99]` onto the assertion line.
            if trimmed == ")" {
                self.in_table_row = false;
                if !self.row_values.is_empty() {
                    let joined = format!("[{}]", self.row_values.join(", "));
                    if let Some(first) = last.details.first_mut() {
                        first.push_str(&format!(" {}", joined));
                    } else {
                        last.details.push(joined);
                    }
                    self.row_values.clear();
                }
            } else if trimmed != "(" {
                self.row_values
                    .push(trimmed.trim_end_matches(',').split_whitespace().collect());
            }
        } else if let Some(msg) = trimmed.strip_prefix("Message:") {
            // Property-check failures (propspec / TableDriven) wrap the real
            // assertion in a generic "TestFailedException ... during property
            // evaluation" line; the `Message:` line carries the actual reason,
            // so prefer it over that boilerplate.
            let reason = msg.trim().to_string();
            if last.details.is_empty() {
                last.details.push(reason);
            } else {
                last.details[0] = reason;
            }
        } else if let Some(loc) = trimmed.strip_prefix("Location:") {
            // The precise failing location (e.g. the failing table row), more
            // specific than the wrapper's.
            let loc = loc.trim().trim_start_matches('(').trim_end_matches(')');
            if let Some(first) = last.details.first_mut() {
                if !loc.is_empty() && !first.contains(loc) {
                    first.push_str(&format!(" ({})", loc));
                }
            }
        } else if trimmed.starts_with("Occurred at table row") {
            self.in_table_row = true;
        } else if last.details.is_empty() {
            last.details.push(trimmed.to_string());
        } else if trimmed.starts_with("at ") {
            // ScalaTest *error* (uncaught exception): the exception was kept as
            // the reason but carries no `Location:`; attach the first throw-site
            // frame's `(File.scala:line)`, then ignore the rest of the trace (it
            // stays as `… (loc)`, won't re-append).
            if let Some(first) = last.details.first_mut() {
                if !first.ends_with(')') {
                    if let Some(caps) = PAREN_LOC_RE.captures(trimmed) {
                        first.push_str(&format!(" ({})", &caps[1]));
                    }
                }
            }
        }
    }

    /// ziotest detail body: append the throw-site location to the reason;
    /// `result == ...` lines are dropped, a blank line / next bullet ends it.
    fn ziotest_detail(&mut self, trimmed: &str) {
        if let Some(rest) = trimmed.strip_prefix("at ") {
            let rest = rest.trim();
            // Assertion failures point at a bare `path:line`; a defect's first
            // frame is a Java `pkg.Class.method(File.scala:line)`.
            let loc = PAREN_LOC_RE
                .captures(rest)
                .map(|c| c[1].to_string())
                .unwrap_or_else(|| shorten_location(rest));
            if !loc.is_empty() {
                if let Some(reason) = self.failures.last_mut().and_then(|f| f.details.first_mut()) {
                    reason.push_str(&format!(" ({})", loc));
                }
            }
            self.mode = DetailMode::None;
        } else if trimmed.is_empty() {
            self.mode = DetailMode::None;
        }
    }

    /// Render the accumulated state into the final filtered summary.
    fn finish(mut self, output: &str) -> String {
        // Collapse scalar munit `=> Diff` blocks to a one-liner (collections
        // keep the full unified diff).
        for f in self.failures.iter_mut() {
            compact_munit_diff(&mut f.details);
        }

        // Collapse identical failures (same name + details). ScalaTest / specs2
        // / zio-test `error` repeats the same uncaught exception once per suite
        // with no suite name to tell them apart; distinct failures (munit's
        // suite-qualified names, different assertions) are left alone.
        dedup_failures(&mut self.failures);

        if self.no_tests {
            return "bloop test: no tests run".to_string();
        }
        if !self.saw_tally {
            if output.trim().is_empty() {
                return "bloop test: No test output".to_string();
            }
            return output.trim().to_string();
        }

        if self.failed == 0 {
            let mut summary = format!("bloop test: {} passed", self.passed);
            if self.ignored > 0 {
                summary.push_str(&format!(", {} ignored", self.ignored));
            }
            let mut parts: Vec<String> = Vec::new();
            if let Some(s) = self.suites {
                parts.push(format!("{} suites", s));
            }
            if let Some(d) = &self.duration {
                parts.push(format_total_duration(d));
            }
            if !parts.is_empty() {
                summary.push_str(&format!(" ({})", parts.join(", ")));
            }
            return summary;
        }

        let mut result = format!("bloop test: {} passed, {} failed", self.passed, self.failed);
        if self.ignored > 0 {
            result.push_str(&format!(", {} ignored", self.ignored));
        }
        if let Some(d) = &self.duration {
            result.push_str(&format!(" ({})", format_total_duration(d)));
        }
        result.push('\n');
        for f in self.failures.iter().take(MAX_FAILURES) {
            result.push_str(&format!("  [FAIL] {}\n", truncate(&f.name, MAX_FAILURE_WIDTH)));
            for detail in &f.details {
                result.push_str(&format!("     {}\n", truncate(detail, MAX_FAILURE_WIDTH)));
            }
        }
        if self.failures.len() > MAX_FAILURES {
            result.push_str(&format!(
                "\n… +{} more failed\n",
                self.failures.len() - MAX_FAILURES
            ));
            if let Some(hint) = crate::core::tee::force_tee_hint(output, "bloop-test-failures") {
                result.push_str(&format!("{hint}\n"));
            }
        }
        result.trim().to_string()
    }
}

/// Collapse a scalar munit assertion diff to a single `expected X, obtained Y`
/// line. munit renders even a one-value mismatch as a three-line `=> Diff`; that
/// framing only earns its keep for multi-line/collection values, which are left
/// untouched.
///
/// The collapse **must** read the `=> Diff` legend to label the sides: munit's
/// `-`/`+` polarity is not fixed (usually `(- expected, + obtained)` but
/// sometimes the reverse), so a hardcoded assumption would swap the labels.
fn compact_munit_diff(details: &mut Vec<String>) {
    let Some(diff_idx) = details.iter().position(|d| d.starts_with("=> Diff")) else {
        return;
    };
    let minus_is_obtained = details[diff_idx].contains("- obtained");
    let after = &details[diff_idx + 1..];
    // Orphaned header: when suites run in parallel, another suite's output can
    // interleave between the `=> Diff` header and its `-`/`+` body, so the body
    // is never captured. A bare `=> Diff` with no diff lines is useless — drop
    // the header rather than emit a dangling legend.
    if !after.iter().any(|l| l.starts_with('-') || l.starts_with('+')) {
        details.truncate(diff_idx);
        return;
    }
    // Scalar diff: exactly one `-` line then one `+` line, nothing after.
    if after.len() == 2 && after[0].starts_with('-') && after[1].starts_with('+') {
        let minus = after[0].strip_prefix('-').unwrap_or(&after[0]).trim();
        let plus = after[1].strip_prefix('+').unwrap_or(&after[1]).trim();
        let (expected, obtained) = if minus_is_obtained {
            (plus, minus)
        } else {
            (minus, plus)
        };
        let one_liner = format!("expected {}, obtained {}", expected, obtained);
        details.truncate(diff_idx);
        details.push(one_liner);
    }
}

/// Append a detail line to the most recent failure, capped at `MAX_DETAIL_LINES`.
fn push_detail(failures: &mut [TestFailure], line: &str) {
    if let Some(last) = failures.last_mut() {
        if last.details.len() < MAX_DETAIL_LINES {
            last.details.push(line.to_string());
        } else if last.details.len() == MAX_DETAIL_LINES {
            // Signal that further detail lines were dropped rather than cutting
            // silently; the full block stays recoverable via the failure tee
            // (a shown failure means the run exited non-zero).
            last.details.push("… (more detail lines omitted)".to_string());
        }
    }
}

/// Parse the first capture group of `re` in `text` as a `u32` (0 if absent).
fn first_capture(re: &Regex, text: &str) -> u32 {
    re.captures(text)
        .and_then(|c| c[1].parse().ok())
        .unwrap_or(0)
}

/// Compact a munit `==> X` marker's trailing text into a failure detail.
/// A bare `path:line[:col]` shortens to its basename; a `path:line <prose>`
/// keeps only the shortened location (munit's trailing `assertion failed` is
/// generic framing — the real signal lives in the `Clues`/diff body); a pure
/// message (e.g. `/ by zero`) is kept verbatim.
fn munit_marker_detail(rest: &str) -> String {
    if LOCATION_RE.is_match(rest) {
        return shorten_location(rest);
    }
    if let Some((first, _)) = rest.split_once(char::is_whitespace) {
        if LOCATION_RE.is_match(first) {
            return shorten_location(first);
        }
    }
    rest.to_string()
}

/// Trim a specs2 `[E]` reason at the first `(File:line)` — the runner often
/// concatenates the next stack frame straight onto the exception message
/// (`… / by zero (Calculator.scala:8)example.Calculator$.div(…)`).
fn clean_specs2_reason(reason: &str) -> String {
    SPECS2_REASON_RE
        .captures(reason)
        .map(|c| c[1].to_string())
        .unwrap_or_else(|| reason.to_string())
}

/// Collapse runs of identical failures (same name *and* details) into one
/// entry tagged `(×N)`, preserving first-seen order. Frameworks that report an
/// uncaught exception once per suite without a distinguishing suite name
/// (ScalaTest / specs2 / zio-test `error`) would otherwise repeat verbatim.
fn dedup_failures(failures: &mut Vec<TestFailure>) {
    let mut out: Vec<TestFailure> = Vec::new();
    let mut counts: Vec<usize> = Vec::new();
    for f in failures.drain(..) {
        if let Some(pos) = out
            .iter()
            .position(|o| o.name == f.name && o.details == f.details)
        {
            counts[pos] += 1;
        } else {
            out.push(f);
            counts.push(1);
        }
    }
    for (o, n) in out.iter_mut().zip(counts.iter()) {
        if *n > 1 {
            o.name.push_str(&format!(" (×{})", n));
        }
    }
    *failures = out;
}

/// Drop a compile diagnostic's build-layout prefix, keeping the
/// package-qualified path. The standard layout buries the real path under
/// `<module>/.../src/{main,test}/<source-root>/`, where `<source-root>` is
/// `scala`, `java`, or a cross-compile dialect dir (`scala-2.13`, `scala-3`).
/// Everything up to and including a *plain* `scala`/`java` root is dropped:
///   `modules/core/shared/src/main/scala/io/circe/Decoder.scala:1063:91`
///   → `io/circe/Decoder.scala:1063:91`
/// A *dialect* root is kept as a prefix so cross-compiled sources stay
/// distinguishable:
///   `zio-json/shared/src/main/scala-2.x/zio/json/Foo.scala:9:20`
///   → `scala-2.x/zio/json/Foo.scala:9:20`
/// Paths with no recognizable source root are left unchanged.
fn shorten_diag_location(loc: &str) -> String {
    let segments: Vec<&str> = loc.split('/').collect();
    let Some(idx) = segments.iter().rposition(|s| is_scala_source_root(s)) else {
        return loc.to_string();
    };
    let pkg = &segments[idx + 1..];
    if pkg.is_empty() {
        return loc.to_string();
    }
    let root = segments[idx];
    if root == "scala" || root == "java" {
        pkg.join("/")
    } else {
        // A dialect dir (scala-2.13, scala-3, …) — keep it as a discriminator.
        format!("{}/{}", root, pkg.join("/"))
    }
}

/// Whether a path segment is a Scala/Java source root: `scala`, `java`, or a
/// cross-compile dialect dir (`scala-2.13`, `scala-3`, `scala-2.x`).
fn is_scala_source_root(seg: &str) -> bool {
    seg == "scala" || seg == "java" || seg.starts_with("scala-")
}

/// Normalize bloop's `Total duration:` value (`14ms`, `0.12s`, `1m5s`), falling
/// back to the raw string for an unrecognized unit shape.
fn format_total_duration(raw: &str) -> String {
    parse_duration_ms(raw)
        .map(format_duration)
        .unwrap_or_else(|| raw.to_string())
}

/// Parse a bloop duration string into milliseconds: `Nms`, fractional seconds
/// (`0.12s`, `38s`), or a minutes+seconds combo (`1m5s`).
fn parse_duration_ms(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    if let Some(c) = DUR_MS_RE.captures(raw) {
        return c[1].parse::<u64>().ok();
    }
    if let Some(c) = DUR_MIN_SEC_RE.captures(raw) {
        let m: u64 = c[1].parse().ok()?;
        let s: f64 = c[2].parse().ok()?;
        return Some(m * 60_000 + (s * 1000.0).round() as u64);
    }
    if let Some(c) = DUR_SEC_RE.captures(raw) {
        let s: f64 = c[1].parse().ok()?;
        return Some((s * 1000.0).round() as u64);
    }
    None
}

/// Shorten an absolute `path:line[:col]` location to just `file:line[:col]`.
/// Leaves non-path messages (e.g. `/ by zero`) untouched.
fn shorten_location(s: &str) -> String {
    if s.contains('/') && LOCATION_RE.is_match(s) {
        s.rsplit('/').next().unwrap_or(s).to_string()
    } else {
        s.to_string()
    }
}

/// Filter `bloop run` output — strip the compile-server banner and the
/// `Compiling`/`Compiled` progress lines, keep the program's own output.
fn filter_bloop_run(output: &str) -> String {
    let output = strip_ansi(output);
    let output = output.as_str();

    let mut lines: Vec<&str> = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() && lines.is_empty() {
            continue;
        }
        if NOISE_RE.is_match(trimmed)
            || COMPILING_RE.is_match(trimmed)
            || COMPILED_RE.is_match(trimmed)
        {
            continue;
        }
        lines.push(line);
    }
    lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    /// Shorthand for loading a bloop fixture from the standardized corpus.
    macro_rules! fixture {
        ($name:literal) => {
            include_str!(concat!("../../../tests/fixtures/bloop/", $name))
        };
    }

    fn savings(input: &str, output: &str) -> f64 {
        100.0 - (count_tokens(output) as f64 / count_tokens(input) as f64 * 100.0)
    }

    // --- compile: success ---

    #[test]
    fn test_filter_bloop_compile_success() {
        let input = fixture!("bloop_compile_compile_pass_3.txt");
        let output = filter_bloop_compile(input);

        assert!(output.starts_with("bloop compile:"), "got: {output}");
        assert!(output.contains("1 sources"), "got: {output}");
        assert!(!output.contains("error"), "got: {output}");
    }

    #[test]
    fn test_filter_bloop_compile_noop_is_up_to_date() {
        let input = fixture!("bloop_compile_incremental_3.txt");
        assert_eq!(filter_bloop_compile(input), "bloop compile: up-to-date");
        assert_eq!(filter_bloop_compile("\n"), "bloop compile: up-to-date");
    }

    // --- compile: warnings only (no errors) ---

    #[test]
    fn test_filter_bloop_compile_warn_small() {
        // Warnings (`[W]`) are not errors: the compile still succeeds, but the
        // warning must be surfaced (count + diagnosis), not silently dropped.
        let input = fixture!("bloop_compile_warn_small_3.txt");
        let output = filter_bloop_compile(input);
        assert!(output.contains("1 sources"), "got: {output}");
        assert!(output.contains("1 warning"), "warning count missing: {output}");
        assert!(!output.contains("2 warning"), "singular expected: {output}");
        assert!(!output.contains("error"), "warnings counted as errors: {output}");
        // The diagnosis (location + message) is kept, like the error path.
        assert!(output.contains("Defect1.scala:6:39"), "got: {output}");
        assert!(output.contains("match may not be exhaustive"), "got: {output}");
        // The actionable continuation (which case is unhandled) must survive —
        // it follows the headline after a blank `[W]` line.
        assert!(
            output.contains("It would fail on pattern case: None"),
            "actionable continuation dropped: {output}"
        );
        // Source-snippet / caret framing is dropped.
        assert!(!output.contains("def label"), "snippet leaked: {output}");
        assert!(!output.contains('^'), "caret leaked: {output}");
    }

    #[test]
    fn test_filter_bloop_compile_warn_large() {
        let input = fixture!("bloop_compile_warn_large_3.txt");
        let output = filter_bloop_compile(input);
        assert!(output.contains("6 sources"), "got: {output}");
        assert!(output.contains("6 warnings"), "got: {output}");
        assert!(!output.contains("error"), "got: {output}");
        // Each warning keeps its actionable `It would fail on …` continuation;
        // on this worst-case fixture (6 near-identical exhaustiveness warnings)
        // that repeated content holds savings to ~55% — still substantial.
        assert!(savings(input, &output) >= 50.0, "savings: {output}");
    }

    #[test]
    fn test_filter_bloop_compile_warn_message_not_truncated_midword() {
        // A real exhaustiveness warning (~145 chars) carries the actionable
        // missing-case list in its tail; the full message must survive (a tight
        // length cap would cut it mid-type).
        let input = "Compiling core (1 Scala sources)\n\
[W]  [E1] src/io/circe/Decoder.scala:1063:91\n\
[W]        match may not be exhaustive.\n\
[W]        It would fail on the following input: (x: io.circe.ACursor forSome x not in (io.circe.FailedCursor, io.circe.HCursor))\n\
[W]        L1063:     def f(c: ACursor) = c match {\n\
Compiled core (100ms)\n";
        let output = filter_bloop_compile(input);
        assert!(output.contains("1 warning"), "got: {output}");
        // The tail (the missing cases) is the point — it must not be truncated.
        assert!(output.contains("io.circe.FailedCursor, io.circe.HCursor)"), "tail cut: {output}");
        assert!(!output.contains("io.circe.Fa..."), "truncated mid-word: {output}");
    }

    #[test]
    fn test_duration_helpers() {
        assert_eq!(format_duration(226), "226ms");
        assert_eq!(format_duration(12886), "12.9s");
        assert_eq!(format_duration(125_000), "2m5s");
        assert_eq!(format_total_duration("0.12s"), "120ms");
        assert_eq!(format_total_duration("0.1s"), "100ms");
        assert_eq!(format_total_duration("38s"), "38.0s");
        assert_eq!(format_total_duration("14ms"), "14ms");
        assert_eq!(format_total_duration("0ms"), "0ms");
        assert_eq!(format_total_duration("1m5s"), "1m5s");
        assert_eq!(format_total_duration("90s"), "1m30s");
        assert_eq!(format_total_duration("ages"), "ages");
    }

    #[test]
    fn test_filter_bloop_test_duration_normalized() {
        // A fractional-second `Total duration` must render as ms in the summary.
        let input = "example.S\n4 tests, 4 passed\n\
                     All 1 test suites passed.\nTotal duration: 0.41s\n";
        let output = filter_bloop_test(input);
        assert!(output.contains("410ms"), "duration not normalized: {output}");
        assert!(!output.contains("0.41s"), "raw duration echoed: {output}");
    }

    #[test]
    fn test_shorten_diag_location() {
        // Plain `scala`/`java` source root: drop the build-layout + module prefix.
        assert_eq!(
            shorten_diag_location("modules/core/shared/src/main/scala/io/circe/Decoder.scala:1063:91"),
            "io/circe/Decoder.scala:1063:91"
        );
        assert_eq!(
            shorten_diag_location("app/src/test/java/com/x/Foo.java:5:9"),
            "com/x/Foo.java:5:9"
        );
        // Cross-compile dialect dir is kept as a discriminator.
        assert_eq!(
            shorten_diag_location("zio-json/shared/src/main/scala-2.x/zio/json/Foo.scala:9:20"),
            "scala-2.x/zio/json/Foo.scala:9:20"
        );
        assert_eq!(
            shorten_diag_location("m/src/main/scala-3/p/Bar.scala:1:1"),
            "scala-3/p/Bar.scala:1:1"
        );
        // No recognizable source root: left unchanged.
        assert_eq!(shorten_diag_location("src/Bad.scala:5:21"), "src/Bad.scala:5:21");
        assert_eq!(shorten_diag_location("Bare.scala:1:1"), "Bare.scala:1:1");
    }

    #[test]
    fn test_filter_bloop_compile_shortens_paths() {
        // The build-layout + module prefix is dropped from the listed location.
        let input = "Compiling root (1 Scala sources)\n\
[E] [E1] modules/core/shared/src/main/scala/io/circe/Decoder.scala:5:21\n\
[E]      Found:    (\"x\" : String)\n\
[E]      Required: Int\n\
Compiled root (100ms)\n\
[E] Failed to compile 'root'\n";
        let output = filter_bloop_compile(input);
        assert!(output.contains("io/circe/Decoder.scala:5:21"), "got: {output}");
        assert!(!output.contains("src/main/scala"), "layout prefix leaked: {output}");
        assert!(!output.contains("modules/core"), "module prefix leaked: {output}");
    }

    // --- compile: errors ---

    #[test]
    fn test_filter_bloop_compile_errors_small() {
        let input = fixture!("bloop_compile_compile_error_small_3.txt");
        let output = filter_bloop_compile(input);

        assert!(output.contains("1 error"), "got: {output}");
        assert!(output.contains("1 sources"), "source count missing: {output}");
        assert!(output.contains("Defect1.scala:5:21"), "got: {output}");
        assert!(output.contains("Required: Int"), "got: {output}");
        // Caret / source-snippet noise must be gone.
        assert!(!output.contains("^^^"), "caret leaked: {output}");
        assert!(!output.contains("L5:"), "source snippet leaked: {output}");
        assert!(!output.contains("Failed to compile"));
    }

    #[test]
    fn test_filter_bloop_compile_errors_small_colored() {
        // Real `bloop compile` output on a host that did not disable color: every
        // `[E]` header, the file path, and the snippet are wrapped in SGR codes.
        // The diagnostic regexes are line-anchored, so without ANSI stripping the
        // filter would miss every line and fall through to raw passthrough. Parsed
        // result must match the nocolor corpus (and carry no escape bytes).
        let input = fixture!("bloop_compile_compile_error_small_color_3.txt");
        assert!(input.contains('\x1b'), "fixture should be colored");
        let output = filter_bloop_compile(input);

        assert!(!output.contains('\x1b'), "escape byte leaked: {output:?}");
        assert!(output.contains("1 error"), "got: {output}");
        assert!(output.contains("1 sources"), "source count missing: {output}");
        assert!(output.contains("Defect1.scala:5:21"), "got: {output}");
        assert!(output.contains("Required: Int"), "got: {output}");
        assert!(!output.contains("^^^"), "caret leaked: {output}");
        assert!(!output.contains("L5:"), "source snippet leaked: {output}");
    }

    #[test]
    fn test_filter_bloop_compile_errors_large() {
        let input = fixture!("bloop_compile_compile_error_large_3.txt");
        let output = filter_bloop_compile(input);

        assert!(output.contains("6 errors"), "got: {output}");
        // Errors report the source count + build time, like the pass/warn paths.
        assert!(output.contains("6 sources"), "source count missing: {output}");
        assert!(output.contains("2.1s"), "build time missing: {output}");
        assert!(output.contains("Defect6.scala:5:21"), "got: {output}");
        assert!(output.contains("Defect1.scala:5:21"), "got: {output}");
        assert!(!output.contains("^^^"), "caret leaked: {output}");
        // Padded labels are collapsed: `Found:    (` → `Found: (`.
        assert!(output.contains("Found: ("), "whitespace not collapsed: {output}");
        assert!(!output.contains("Found:  "), "padding leaked: {output}");
        // Compile output is mostly diagnostics, which we keep (the diagnosis is
        // the signal) — only source-snippet framing is dropped. On this small
        // fixture (~180 tokens) that holds savings to ~57%; compressing further
        // would discard the actual errors.
        assert!(savings(input, &output) >= 50.0, "savings: {output}");
    }

    #[test]
    fn test_filter_bloop_compile_errors_and_warnings() {
        // A failing build that also emitted warnings: the header counts both, but
        // only error locations are listed (warnings often clear once errors do).
        let input = "Compiling root (6 Scala sources)\n\
[E] [E1] src/Bad.scala:5:21\n\
[E]      Found:    (\"x\" : String)\n\
[E]      Required: Int\n\
[E]      L5:   val broken: Int = \"x\"\n\
[W] [E2] src/Warn.scala:6:39\n\
[W]      match may not be exhaustive.\n\
[W]      L6:   def f(o: Option[Int]) = o match {\n\
Compiled root (2058ms)\n\
[E] Failed to compile 'root'\n";
        let output = filter_bloop_compile(input);

        assert!(output.contains("6 sources"), "got: {output}");
        assert!(output.contains("1 error"), "error count missing: {output}");
        assert!(output.contains("1 warning"), "warning count dropped: {output}");
        // Error location is listed; the warning location is not.
        assert!(output.contains("src/Bad.scala:5:21"), "got: {output}");
        assert!(!output.contains("src/Warn.scala"), "warning listed: {output}");
        assert!(!output.contains("exhaustive"), "warning body listed: {output}");
    }

    #[test]
    fn test_filter_bloop_compile_caps_errors() {
        // 25 errors: only MAX_ERRORS (20) render inline, rest collapse to `+N more`.
        let mut input = String::from("Compiling root (25 Scala sources)\n");
        for i in 1..=25 {
            input.push_str(&format!("[E] [E{i}] src/F{i}.scala:5:21\n[E]      Required: Int\n"));
        }
        input.push_str("Compiled root (100ms)\n[E] Failed to compile 'root'\n");
        let output = filter_bloop_compile(&input);

        assert!(output.contains("25 errors"), "header counts all: {output}");
        assert_eq!(
            output.lines().filter(|l| l.contains(".scala:")).count(),
            MAX_ERRORS
        );
        assert!(output.contains("+5 more errors"), "overflow missing: {output}");
    }

    #[test]
    fn test_filter_bloop_compile_caps_warnings() {
        // 15 warnings: only MAX_WARNINGS (10) render inline, rest `+N more`.
        let mut input = String::from("Compiling root (15 Scala sources)\n");
        for i in 1..=15 {
            input.push_str(&format!(
                "[W]  [E{i}] src/F{i}.scala:6:39\n[W]      match may not be exhaustive.\n"
            ));
        }
        input.push_str("Compiled root (100ms)\n");
        let output = filter_bloop_compile(&input);

        assert!(output.contains("15 warnings"), "header counts all: {output}");
        assert_eq!(
            output.lines().filter(|l| l.contains(".scala:")).count(),
            MAX_WARNINGS
        );
        assert!(output.contains("+5 more warnings"), "overflow missing: {output}");
    }

    // --- test: munit pass ---

    #[test]
    fn test_filter_bloop_test_munit_pass_small() {
        let input = fixture!("bloop_test_munit_funsuite_pass_small_3.txt");
        let output = filter_bloop_test(input);

        assert!(output.starts_with("bloop test:"), "got: {output}");
        assert!(output.contains("4 passed"), "got: {output}");
        assert!(output.contains("1 suites"), "got: {output}");
        assert!(!output.contains('\n'), "pass output should be one line: {output}");
        assert!(savings(input, &output) >= 60.0, "savings: {output}");
    }

    #[test]
    fn test_filter_bloop_test_munit_pass_large() {
        let input = fixture!("bloop_test_munit_funsuite_pass_large_3.txt");
        let output = filter_bloop_test(input);

        assert!(output.contains("24 passed"), "got: {output}");
        assert!(output.contains("6 suites"), "got: {output}");
        assert!(savings(input, &output) >= 60.0, "savings: {output}");
    }

    // --- test: munit (assertion failures with diffs) ---

    #[test]
    fn test_filter_bloop_test_munit_fail_small() {
        let input = fixture!("bloop_test_munit_funsuite_fail_small_3.txt");
        let output = filter_bloop_test(input);

        assert!(output.contains("2 passed, 2 failed"), "got: {output}");
        // Each munit failure must keep its name, location, and the diff body.
        assert!(
            output.contains("[FAIL] example.CalculatorSuite.sub returns the difference"),
            "got: {output}"
        );
        assert!(output.contains("CalculatorSuite.scala:10"), "got: {output}");
        // Scalar assertEquals diffs collapse to a one-liner (no 3-line `=> Diff`).
        assert!(output.contains("expected 7, obtained 6"), "got: {output}");
        assert!(!output.contains("=> Diff"), "scalar diff not collapsed: {output}");
        assert!(output.contains("expected 4, obtained 5"), "got: {output}");
        assert!(output.contains("CalculatorSuite.scala:18"), "got: {output}");
        // Absolute /tmp/workspace path must be shortened to the basename.
        assert!(!output.contains("/tmp/workspace"), "leaked abs path: {output}");
        // Framework stack frames must be dropped.
        assert!(!output.contains("munit.FunSuite.assertEquals"), "stack leaked: {output}");
        assert!(savings(input, &output) >= 60.0, "savings: {output}");
    }

    #[test]
    fn test_filter_bloop_test_munit_fail_small_colored() {
        // Real colored `bloop test` output: the `==> X` marker, the `=> Diff`
        // header, and the `-`/`+` diff lines are all SGR-wrapped, so without ANSI
        // stripping the marker/diff matchers miss and no failures are detected.
        let input = fixture!("bloop_test_munit_funsuite_fail_small_color_3.txt");
        assert!(input.contains('\x1b'), "fixture should be colored");
        let output = filter_bloop_test(input);

        assert!(!output.contains('\x1b'), "escape byte leaked: {output:?}");
        assert!(output.contains("2 passed, 2 failed"), "got: {output}");
        assert!(
            output.contains("[FAIL] example.CalculatorSuite.sub returns the difference"),
            "got: {output}"
        );
        assert!(output.contains("CalculatorSuite.scala:10"), "got: {output}");
        assert!(output.contains("expected 7, obtained 6"), "got: {output}");
        assert!(!output.contains("=> Diff"), "scalar diff not collapsed: {output}");
        assert!(output.contains("expected 4, obtained 5"), "got: {output}");
        assert!(!output.contains("/tmp/workspace"), "leaked abs path: {output}");
    }

    #[test]
    fn test_filter_bloop_test_munit_fail_large() {
        let input = fixture!("bloop_test_munit_funsuite_fail_large_3.txt");
        let output = filter_bloop_test(input);

        assert!(output.contains("12 passed, 12 failed"), "got: {output}");
        assert!(savings(input, &output) >= 60.0, "savings: {output}");
    }

    #[test]
    fn test_filter_bloop_test_munit_multiline_diff_kept() {
        // A multi-line/collection diff must keep the full unified `=> Diff` block
        // — only scalar (one `-` / one `+`) diffs collapse to a one-liner.
        let input = "Test run example.S started\n\
                     ==> X example.S.list 0.0s munit.ComparisonFailException: /tmp/ws/S.scala:5\n\
                     => Diff (- expected, + obtained)\n\
                     -List(1, 2, 3)\n\
                     -extra\n\
                     +List(1, 2)\n\
                     Execution took 1ms\n\
                     1 tests, 0 passed, 1 failed\n";
        let output = filter_bloop_test(input);
        assert!(output.contains("=> Diff (- expected, + obtained)"), "got: {output}");
        assert!(output.contains("-List(1, 2, 3)"), "got: {output}");
        assert!(!output.contains("expected List"), "should not collapse: {output}");
    }

    #[test]
    fn test_filter_bloop_test_munit_orphan_diff_header_dropped() {
        // Parallel suites can interleave on the shared stream so another suite's
        // output lands between a `=> Diff` header and its `-`/`+` body, leaving
        // the body uncaptured. A bare `=> Diff` legend with no diff is useless —
        // it must be dropped, not emitted dangling.
        let input = "Test run example.S started\n\
                     ==> X example.S.div 0.0s munit.ComparisonFailException: /tmp/ws/S.scala:18\n\
                     => Diff (- expected, + obtained)\n\
                     Test run example.Other started\n\
                     1 tests, 0 passed, 1 failed\n";
        let output = filter_bloop_test(input);
        assert!(output.contains("0 passed, 1 failed"), "got: {output}");
        assert!(output.contains("[FAIL] example.S.div"), "got: {output}");
        // The orphaned legend must not survive (neither as a dangling header nor
        // a bogus collapse).
        assert!(!output.contains("=> Diff"), "dangling diff header: {output}");
    }

    // --- test: munit warnings interleaved with failures ---

    #[test]
    fn test_filter_bloop_test_munit_warn_fail_large() {
        let input = fixture!("bloop_test_munit_funsuite_warn_fail_large_3.txt");
        let output = filter_bloop_test(input);

        // Compiler `[W]` warnings must not corrupt the pass/fail tally.
        assert!(output.contains("12 passed, 12 failed"), "got: {output}");
    }

    // --- test: munit (uncaught exception, no diff) ---

    #[test]
    fn test_filter_bloop_test_munit_error_small() {
        let input = fixture!("bloop_test_munit_funsuite_error_small_3.txt");
        let output = filter_bloop_test(input);

        assert!(output.contains("3 passed, 1 failed"), "got: {output}");
        assert!(
            output.contains("[FAIL] example.CalculatorSuite.div returns the quotient"),
            "got: {output}"
        );
        assert!(output.contains("java.lang.ArithmeticException: / by zero"), "got: {output}");
        // For an exception (no diff) the user-code throw site is kept.
        assert!(output.contains("at example.Calculator$.div(Calculator.scala:8)"), "got: {output}");
    }

    #[test]
    fn test_filter_bloop_test_munit_error_large() {
        let input = fixture!("bloop_test_munit_funsuite_error_large_3.txt");
        let output = filter_bloop_test(input);

        assert!(output.contains("18 passed, 6 failed"), "got: {output}");
        assert_eq!(output.matches("[FAIL]").count(), 6, "got: {output}");
    }

    // --- test: munit multimodule (two suites) ---

    #[test]
    fn test_filter_bloop_test_munit_multimodule_fail() {
        let input = fixture!("bloop_test_mm_munit_funsuite_fail_small_3.txt");
        let output = filter_bloop_test(input);

        // Two suites × (2 passed, 2 failed) summed.
        assert!(output.contains("4 passed, 4 failed"), "got: {output}");
        assert!(
            output.contains("example.util.CalculatorSuite.sub returns the difference"),
            "got: {output}"
        );
        assert!(
            output.contains("example.core.CalculatorSuite.div returns the quotient"),
            "got: {output}"
        );
        // 4 failures, each with its diff — but no duplicate from the trailing
        // `Failed:` summary block.
        assert_eq!(output.matches("[FAIL]").count(), 4, "got: {output}");
    }

    // --- test: ScalaTest variants (all use `*** FAILED ***`) ---

    #[test]
    fn test_filter_bloop_test_scalatest_spec_styles() {
        // Every ScalaTest spec style collapses to the same compact shape:
        // `N passed, M failed` + `[FAIL] <name>` + `<reason> (file:line)`. One
        // case per style; `name = ""` where the style's wording isn't asserted.
        let cases: &[(&str, &str, &str)] = &[
            (
                fixture!("bloop_test_scalatest_flatspec_fail_small_3.txt"),
                "should sub returns the difference",
                "6 did not equal 7 (CalculatorSuite.scala:12)",
            ),
            (
                fixture!("bloop_test_scalatest_wordspec_fail_small_3.txt"),
                "",
                "6 did not equal 7 (CalculatorSuite.scala:11)",
            ),
            (
                fixture!("bloop_test_scalatest_freespec_fail_small_3.txt"),
                "",
                "6 did not equal 7 (CalculatorSuite.scala:11)",
            ),
            (
                fixture!("bloop_test_scalatest_funspec_fail_small_3.txt"),
                "",
                "6 did not equal 7 (CalculatorSuite.scala:11)",
            ),
            (
                fixture!("bloop_test_scalatest_featurespec_fail_small_3.txt"),
                "Scenario: sub returns the difference",
                "6 did not equal 7 (CalculatorSuite.scala:18)",
            ),
            (
                fixture!("bloop_test_scalatest_refspec_fail_small_3.txt"),
                "div returns the quotient",
                "5 did not equal 4 (CalculatorSuite.scala:9)",
            ),
            (
                fixture!("bloop_test_scalatest_matchers_fail_small_3.txt"),
                "sub returns the difference",
                "6 was not equal to 7 (CalculatorSuite.scala:13)",
            ),
        ];
        for &(input, name, reason) in cases {
            let output = filter_bloop_test(input);
            assert!(output.contains("2 passed, 2 failed"), "got: {output}");
            if !name.is_empty() {
                assert!(output.contains(name), "name `{name}` missing: {output}");
            }
            assert!(output.contains(reason), "reason `{reason}` missing: {output}");
            assert!(savings(input, &output) >= 60.0, "savings ({reason}): {output}");
        }
    }

    #[test]
    fn test_filter_bloop_test_scalatest_propspec() {
        let input = fixture!("bloop_test_scalatest_propspec_fail_small_3.txt");
        let output = filter_bloop_test(input);
        assert!(output.contains("0 passed, 1 failed"), "got: {output}");
        assert!(output.contains("[FAIL] Calculator.add matches the table"), "got: {output}");
        // The real assertion (`Message:`) + precise row (`Location:`) replace the
        // generic "thrown during property evaluation" boilerplate.
        assert!(output.contains("17 did not equal 99"), "lost assertion: {output}");
        assert!(
            output.contains("(CalculatorSuite.scala:16)"),
            "lost failing-row location: {output}"
        );
        assert!(
            !output.contains("property evaluation"),
            "boilerplate not replaced: {output}"
        );
        // The failing table row's input values are surfaced compactly so the
        // failure is reproducible without the raw multi-line `Occurred at` block.
        assert!(output.contains("[a=10, b=7, sum=99]"), "lost table inputs: {output}");
        assert!(!output.contains("Occurred at table row"), "raw block leaked: {output}");
    }

    // --- test: munit clue (`assert(clue(x) == clue(y))`) ---

    #[test]
    fn test_filter_bloop_test_munit_clue() {
        // A `clue` failure's whole point is the captured `Clues { }` values; the
        // filter must surface those.
        let input = fixture!("bloop_test_munit_clue_fail_small_3.txt");
        let output = filter_bloop_test(input);

        assert!(output.contains("2 passed, 2 failed"), "got: {output}");
        assert!(
            output.contains("[FAIL] example.CalculatorSuite.sub returns the difference"),
            "got: {output}"
        );
        // The clue pairs (expression + captured value) are kept.
        assert!(output.contains("Calculator.sub(10, 4): Int = 6"), "lost clue: {output}");
        assert!(output.contains("7: Int = 7"), "lost clue: {output}");
        assert!(output.contains("Calculator.div(20, 4): Int = 5"), "lost clue: {output}");
        // The marker location is shortened, not the leaked /tmp path; munit's
        // generic `assertion failed` framing is dropped.
        assert!(output.contains("CalculatorSuite.scala:10"), "got: {output}");
        assert!(!output.contains("/tmp/workspace"), "leaked abs path: {output}");
        assert!(!output.contains("assertion failed"), "framing leaked: {output}");
        // Framework-internal stack frames are dropped.
        assert!(!output.contains("munit.FunSuite.assert"), "stack leaked: {output}");
        assert_eq!(output.matches("[FAIL]").count(), 2, "got: {output}");
        assert!(savings(input, &output) >= 60.0, "savings: {output}");
    }

    // --- test: munit scalacheck (property failure) ---

    #[test]
    fn test_filter_bloop_test_munit_scalacheck() {
        // A ScalaCheck property failure's actionable parts are the falsification
        // tally, the shrunk counterexample (`> ARG_0`), and the failing seed —
        // the filter must keep all three.
        let input = fixture!("bloop_test_munit_scalacheck_fail_small_3.txt");
        let output = filter_bloop_test(input);

        assert!(output.contains("1 passed, 1 failed"), "got: {output}");
        assert!(
            output.contains("[FAIL] example.CalculatorSuite.add zero is identity"),
            "got: {output}"
        );
        assert!(output.contains("Falsified after 0 passed tests"), "lost tally: {output}");
        assert!(output.contains("> ARG_0: 0"), "lost counterexample: {output}");
        assert!(
            output.contains("Failing seed: TO7zgUWdv-WPJuetViT8r85tY3TprSjGhdyoESCTTkI="),
            "lost seed: {output}"
        );
        // The framework-internal `at munit.Assertions.fail` frame is dropped.
        assert!(!output.contains("munit.Assertions.fail"), "stack leaked: {output}");
        assert!(savings(input, &output) >= 60.0, "savings: {output}");
    }

    // --- test: specs2 (`x <name>` + `[E] <reason>`) ---

    #[test]
    fn test_filter_bloop_test_specs2() {
        let input = fixture!("bloop_test_specs2_mutable_fail_small_3.txt");
        let output = filter_bloop_test(input);

        assert!(output.contains("2 passed, 2 failed"), "got: {output}");
        assert!(output.contains("[FAIL] sub returns the difference"), "got: {output}");
        assert!(output.contains("6 != 7 (CalculatorSpec.scala:11)"), "got: {output}");
        assert!(output.contains("[FAIL] div returns the quotient"), "got: {output}");
        assert!(output.contains("5 != 4 (CalculatorSpec.scala:17)"), "got: {output}");
        // Exactly the two failures — no duplicate from the `Failed:` block.
        assert_eq!(output.matches("[FAIL]").count(), 2, "got: {output}");
        assert!(savings(input, &output) >= 60.0, "savings: {output}");
    }

    #[test]
    fn test_filter_bloop_test_specs2_split_streams() {
        // Real `rtk bloop test` concatenates all stdout, then all stderr, so the
        // specs2 `x <name>` markers (stdout) arrive separated from their `[E]`
        // reasons (stderr). Reasons must still attach to names FIFO. This input
        // reproduces that ordering (markers first, reasons last).
        let input = "CalculatorSpec\n\
                     Calculator should\n\
                     \x20 + add returns the sum\n\
                     \x20 x sub returns the difference\n\
                     \x20 + mul returns the product\n\
                     \x20 x div returns the quotient\n\
                     4 tests, 2 passed, 2 failed\n\
                     [E]    6 != 7 (CalculatorSpec.scala:11)\n\
                     [E]  \n\
                     [E]    5 != 4 (CalculatorSpec.scala:17)\n\
                     [E]  \n";
        let output = filter_bloop_test(input);

        assert!(output.contains("2 passed, 2 failed"), "got: {output}");
        assert!(output.contains("[FAIL] sub returns the difference"), "got: {output}");
        assert!(output.contains("6 != 7 (CalculatorSpec.scala:11)"), "got: {output}");
        assert!(output.contains("[FAIL] div returns the quotient"), "got: {output}");
        assert!(output.contains("5 != 4 (CalculatorSpec.scala:17)"), "got: {output}");
        assert_eq!(output.matches("[FAIL]").count(), 2, "got: {output}");
    }

    // --- test: ziotest (`- <name>` + `✗ <reason>` + `at <path>:line`) ---

    #[test]
    fn test_filter_bloop_test_ziotest() {
        let input = fixture!("bloop_test_ziotest_asserttrue_fail_small_3.txt");
        let output = filter_bloop_test(input);

        assert!(output.contains("2 passed, 2 failed"), "got: {output}");
        assert!(output.contains("[FAIL] sub returns the difference"), "got: {output}");
        assert!(output.contains("6 was not equal to 7 (CalculatorSpec.scala:13)"), "got: {output}");
        assert!(output.contains("[FAIL] div returns the quotient"), "got: {output}");
        assert!(output.contains("5 was not equal to 4 (CalculatorSpec.scala:21)"), "got: {output}");
        // The redundant post-summary re-dump must not double-count failures.
        assert_eq!(output.matches("[FAIL]").count(), 2, "got: {output}");
        // The absolute /tmp/workspace path must be shortened to the basename.
        assert!(!output.contains("/tmp/workspace"), "leaked abs path: {output}");
        assert!(savings(input, &output) >= 60.0, "savings: {output}");
    }

    // --- test: additional framework styles (regression anchors) ---
    //
    // These styles filter cleanly with no per-style special-casing, but nothing
    // pinned them: a future change to marker detection / `last_dash_name` bullet
    // tracking / the `✗` handler could silently break one. Each must still yield
    // `[FAIL] <name>` + reason + `(file:line)` and exactly two failures.

    #[test]
    fn test_filter_bloop_test_specs2_acceptance() {
        // specs2 acceptance style (`s2"..."` spec text).
        let input = fixture!("bloop_test_specs2_acceptance_fail_small_3.txt");
        let output = filter_bloop_test(input);
        assert!(output.contains("2 passed, 2 failed"), "got: {output}");
        assert!(output.contains("[FAIL] sub returns the difference"), "got: {output}");
        assert!(output.contains("6 != 7 (CalculatorSpec.scala:14)"), "got: {output}");
        assert!(output.contains("[FAIL] div returns the quotient"), "got: {output}");
        assert!(output.contains("5 != 4 (CalculatorSpec.scala:16)"), "got: {output}");
        assert_eq!(output.matches("[FAIL]").count(), 2, "got: {output}");
        assert!(savings(input, &output) >= 60.0, "savings: {output}");
    }

    #[test]
    fn test_filter_bloop_test_specs2_nested() {
        // specs2 nested style (`>>` blocks).
        let input = fixture!("bloop_test_specs2_nested_fail_small_3.txt");
        let output = filter_bloop_test(input);
        assert!(output.contains("2 passed, 2 failed"), "got: {output}");
        assert!(output.contains("6 != 7 (CalculatorSpec.scala:11)"), "got: {output}");
        assert!(output.contains("5 != 4 (CalculatorSpec.scala:17)"), "got: {output}");
        assert_eq!(output.matches("[FAIL]").count(), 2, "got: {output}");
        assert!(savings(input, &output) >= 60.0, "savings: {output}");
    }

    #[test]
    fn test_filter_bloop_test_ziotest_assertion() {
        // zio-test `assert(x)(Assertion)` style (vs the `assertTrue` baseline).
        let input = fixture!("bloop_test_ziotest_assertion_fail_small_3.txt");
        let output = filter_bloop_test(input);
        assert!(output.contains("2 passed, 2 failed"), "got: {output}");
        assert!(output.contains("[FAIL] sub returns the difference"), "got: {output}");
        assert!(output.contains("6 was not equal to 7 (CalculatorSpec.scala:11)"), "got: {output}");
        assert!(output.contains("5 was not equal to 4 (CalculatorSpec.scala:17)"), "got: {output}");
        // The post-summary re-dump must not double-count.
        assert_eq!(output.matches("[FAIL]").count(), 2, "got: {output}");
        assert!(savings(input, &output) >= 60.0, "savings: {output}");
    }

    #[test]
    fn test_filter_bloop_test_ziotest_effectful() {
        // zio-test for-comprehension (effectful) style.
        let input = fixture!("bloop_test_ziotest_effectful_fail_small_3.txt");
        let output = filter_bloop_test(input);
        assert!(output.contains("2 passed, 2 failed"), "got: {output}");
        assert!(output.contains("6 was not equal to 7 (CalculatorSpec.scala:16)"), "got: {output}");
        assert!(output.contains("5 was not equal to 4 (CalculatorSpec.scala:26)"), "got: {output}");
        assert_eq!(output.matches("[FAIL]").count(), 2, "got: {output}");
        assert!(savings(input, &output) >= 60.0, "savings: {output}");
    }

    #[test]
    fn test_filter_bloop_test_ziotest_nested() {
        // zio-test suite-of-suites (nested) style: the `- <name>` bullets are
        // doubly nested, and the post-summary re-dump repeats them — neither must
        // inflate the failure count.
        let input = fixture!("bloop_test_ziotest_nested_fail_small_3.txt");
        let output = filter_bloop_test(input);
        assert!(output.contains("2 passed, 2 failed"), "got: {output}");
        assert!(output.contains("6 was not equal to 7 (CalculatorSpec.scala:14)"), "got: {output}");
        assert!(output.contains("5 was not equal to 4 (CalculatorSpec.scala:24)"), "got: {output}");
        assert_eq!(output.matches("[FAIL]").count(), 2, "got: {output}");
        assert!(savings(input, &output) >= 60.0, "savings: {output}");
    }

    #[test]
    fn test_filter_bloop_test_specs2_warning_concat() {
        // The riskiest untested interaction: a compiler `[W]` warning block and
        // the specs2 `[E]`-reason FIFO both land on stderr, which `run_test`
        // appends after all stdout. In that concat order the warning block sits
        // *between* the `x` markers (stdout) and their `[E]` reasons (stderr). A
        // stray `[W]` line must not be consumed by the FIFO, mis-attached as a
        // reason, or counted as a failure — the two reasons must still map to
        // their two markers in order. (Hand-built: the raw `.txt` fixtures are
        // interleaved `2>&1` and don't reproduce this ordering.)
        let input = "CalculatorSpec\n\
                     Calculator should\n\
                     \x20 + add returns the sum\n\
                     \x20 x sub returns the difference\n\
                     \x20 + mul returns the product\n\
                     \x20 x div returns the quotient\n\
                     4 tests, 2 passed, 2 failed\n\
                     [W]  [E1] src/main/scala/example/Calculator.scala:12:39\n\
                     [W]       match may not be exhaustive.\n\
                     [W]       It would fail on pattern case: None\n\
                     [W]       L12:   def label(o: Option[Int]): String = o match {\n\
                     [W] src/main/scala/example/Calculator.scala: L12 [E1]\n\
                     [E]    6 != 7 (CalculatorSpec.scala:11)\n\
                     [E]  \n\
                     [E]    5 != 4 (CalculatorSpec.scala:17)\n\
                     [E]  \n";
        let output = filter_bloop_test(input);

        assert!(output.contains("2 passed, 2 failed"), "warning corrupted tally: {output}");
        assert!(output.contains("[FAIL] sub returns the difference"), "got: {output}");
        assert!(output.contains("6 != 7 (CalculatorSpec.scala:11)"), "reason mis-attached: {output}");
        assert!(output.contains("[FAIL] div returns the quotient"), "got: {output}");
        assert!(output.contains("5 != 4 (CalculatorSpec.scala:17)"), "reason mis-attached: {output}");
        assert_eq!(output.matches("[FAIL]").count(), 2, "warning inflated failures: {output}");
        // No warning content leaks into the test output.
        assert!(!output.contains("exhaustive"), "warning body leaked: {output}");
        assert!(!output.contains("[W]"), "warning marker leaked: {output}");
    }

    // --- test: `error` shape (uncaught exception, not an assertion) ---

    #[test]
    fn test_filter_bloop_test_specs2_error() {
        // An *errored* specs2 example is marked `!` (not `x`) and tallied as
        // `N errors` (not `N failed`). Both must count as failures — otherwise an
        // error-only suite filters to all-green (a dangerous false negative).
        let input = fixture!("bloop_test_specs2_mutable_error_small_3.txt");
        let output = filter_bloop_test(input);

        assert!(output.contains("3 passed, 1 failed"), "false green: {output}");
        assert!(output.contains("[FAIL] div returns the quotient"), "got: {output}");
        assert!(
            output.contains("java.lang.ArithmeticException: / by zero (Calculator.scala:8)"),
            "got: {output}"
        );
        // The trailing stack frame concatenated onto the reason is trimmed off.
        assert!(!output.contains("$$anonfun"), "stack junk leaked: {output}");
        assert_eq!(output.matches("[FAIL]").count(), 1, "got: {output}");
        assert!(savings(input, &output) >= 60.0, "savings: {output}");
    }

    #[test]
    fn test_filter_bloop_test_specs2_error_large() {
        // Six suites each errored identically: the count is summed, the verbatim
        // repeat is collapsed to a single `(×6)` entry.
        let input = fixture!("bloop_test_specs2_mutable_error_large_3.txt");
        let output = filter_bloop_test(input);

        assert!(output.contains("18 passed, 6 failed"), "got: {output}");
        assert_eq!(output.matches("[FAIL]").count(), 1, "not deduped: {output}");
        assert!(output.contains("(×6)"), "missing dedup count: {output}");
        assert!(savings(input, &output) >= 60.0, "savings: {output}");
    }

    #[test]
    fn test_filter_bloop_test_specs2_error_concat() {
        // Live `rtk bloop test` concatenates all stdout then all stderr, so the
        // `!` markers (stdout) arrive before every `[E]` reason+frame (stderr).
        // Each errored example emits one reason line (has spaces) + bare stack
        // frames (no spaces); the FIFO must attach one reason per failure, not
        // mis-assign a frame. Two suites reproduce the ordering.
        let input = "CalculatorSpec1\n\
                     Calculator should\n\
                     \x20 + add returns the sum\n\
                     \x20 ! div returns the quotient\n\
                     4 tests, 3 passed, 1 errors\n\
                     CalculatorSpec2\n\
                     Calculator should\n\
                     \x20 + add returns the sum\n\
                     \x20 ! div returns the quotient\n\
                     4 tests, 3 passed, 1 errors\n\
                     [E]    java.lang.ArithmeticException: / by zero (Calculator.scala:8)example.Calculator$.div(Calculator.scala:8)\n\
                     [E] example.CalculatorSpec1.$init$$$anonfun$1(CalculatorSpec1.scala:17)\n\
                     [E]  \n\
                     [E]    java.lang.ArithmeticException: / by zero (Calculator.scala:8)example.Calculator$.div(Calculator.scala:8)\n\
                     [E] example.CalculatorSpec2.$init$$$anonfun$1(CalculatorSpec2.scala:17)\n\
                     [E]  \n";
        let output = filter_bloop_test(input);

        assert!(output.contains("6 passed, 2 failed"), "got: {output}");
        // Both failures get the exception reason (not a stack frame), deduped.
        assert!(
            output.contains("java.lang.ArithmeticException: / by zero (Calculator.scala:8)"),
            "got: {output}"
        );
        assert!(!output.contains("$$anonfun"), "frame mis-attached: {output}");
        assert!(output.contains("(×2)"), "identical errors not deduped: {output}");
    }

    #[test]
    fn test_filter_bloop_test_ziotest_error() {
        // A zio-test effect that dies with a defect uses a `- <name>` bullet (no
        // `✗`) followed by the exception + stack. The body (which test, the
        // exception, the throw site) must be kept, not dropped.
        let input = fixture!("bloop_test_ziotest_asserttrue_error_small_3.txt");
        let output = filter_bloop_test(input);

        assert!(output.contains("3 passed, 1 failed"), "got: {output}");
        assert!(output.contains("[FAIL] div returns the quotient"), "empty body: {output}");
        assert!(
            output.contains("java.lang.ArithmeticException: / by zero (Calculator.scala:8)"),
            "got: {output}"
        );
        assert_eq!(output.matches("[FAIL]").count(), 1, "got: {output}");
        assert!(savings(input, &output) >= 60.0, "savings: {output}");
    }

    #[test]
    fn test_filter_bloop_test_ziotest_error_large() {
        let input = fixture!("bloop_test_ziotest_asserttrue_error_large_3.txt");
        let output = filter_bloop_test(input);

        assert!(output.contains("18 passed, 6 failed"), "got: {output}");
        assert_eq!(output.matches("[FAIL]").count(), 1, "not deduped: {output}");
        assert!(output.contains("(×6)"), "missing dedup count: {output}");
        assert!(savings(input, &output) >= 60.0, "savings: {output}");
    }

    #[test]
    fn test_filter_bloop_test_scalatest_error() {
        // A ScalaTest `*** FAILED ***` from an uncaught exception keeps the
        // exception as the reason but carries no `Location:`; the throw site is
        // recovered from the first `at …(File.scala:line)` stack frame.
        let input = fixture!("bloop_test_scalatest_funsuite_error_small_3.txt");
        let output = filter_bloop_test(input);

        assert!(output.contains("3 passed, 1 failed"), "got: {output}");
        assert!(output.contains("[FAIL] div returns the quotient"), "got: {output}");
        assert!(
            output.contains("java.lang.ArithmeticException: / by zero (Calculator.scala:8)"),
            "lost throw-site location: {output}"
        );
        // The rest of the (long) stack trace is dropped.
        assert!(!output.contains("OutcomeOf"), "stack trace leaked: {output}");
        assert!(savings(input, &output) >= 60.0, "savings: {output}");
    }

    #[test]
    fn test_filter_bloop_test_scalatest_error_large() {
        // ScalaTest's `*** FAILED ***` line has no suite name, so six suites
        // produce six identical entries — collapsed to one `(×6)`.
        let input = fixture!("bloop_test_scalatest_funsuite_error_large_3.txt");
        let output = filter_bloop_test(input);

        assert!(output.contains("18 passed, 6 failed"), "got: {output}");
        assert_eq!(output.matches("[FAIL]").count(), 1, "not deduped: {output}");
        assert!(output.contains("(×6)"), "missing dedup count: {output}");
        assert!(output.contains("(Calculator.scala:8)"), "lost location: {output}");
        assert!(savings(input, &output) >= 60.0, "savings: {output}");
    }

    #[test]
    fn test_filter_bloop_test_caps_failure_list() {
        // Synthesize a run with 25 munit failures; only MAX_FAILURES render inline.
        let mut input = String::from("Test run example.BigSuite started\n");
        for i in 0..25 {
            input.push_str(&format!(
                "==> X example.BigSuite.case {i} 0.0s munit.ComparisonFailException: /tmp/ws/BigSuite.scala:{i}\n=> Diff (- expected, + obtained)\n-1\n+2\n"
            ));
        }
        input.push_str("Execution took 1ms\n25 tests, 0 passed, 25 failed\n");
        let output = filter_bloop_test(&input);

        assert!(output.contains("0 passed, 25 failed"), "got: {output}");
        assert_eq!(output.matches("[FAIL]").count(), MAX_FAILURES, "got: {output}");
        assert!(output.contains("+5 more failed"), "missing cap note: {output}");
    }

    #[test]
    fn test_push_detail_caps_and_signals_omission() {
        // A single failure with more detail lines than the cap keeps the first
        // MAX_DETAIL_LINES verbatim and appends exactly one omission marker —
        // never silently dropping the tail.
        let mut failures = vec![TestFailure {
            name: "Suite.case".to_string(),
            details: Vec::new(),
        }];
        for i in 0..10 {
            push_detail(&mut failures, &format!("at frame {i}"));
        }
        let details = &failures[0].details;
        assert_eq!(details.len(), MAX_DETAIL_LINES + 1, "got: {details:?}");
        assert_eq!(details[MAX_DETAIL_LINES], "… (more detail lines omitted)");
        assert!(
            details[..MAX_DETAIL_LINES]
                .iter()
                .all(|l| l.starts_with("at frame")),
            "kept lines should be verbatim: {details:?}"
        );
    }

    // --- test: munit skipped / no-tests ---

    #[test]
    fn test_filter_bloop_test_munit_skipped() {
        let input = fixture!("bloop_test_munit_funsuite_skipped_small_3.txt");
        let output = filter_bloop_test(input);

        assert!(output.contains("2 passed"), "got: {output}");
        assert!(output.contains("2 ignored"), "got: {output}");
        assert!(!output.contains("[FAIL]"), "ignored is not a failure: {output}");
    }

    #[test]
    fn test_filter_bloop_test_no_tests() {
        let input = fixture!("bloop_test_no_tests_3.txt");
        let output = filter_bloop_test(input);
        assert_eq!(output, "bloop test: no tests run");
    }

    #[test]
    fn test_filter_bloop_test_munit_diff_legend_reversed() {
        // munit's `-`/`+` polarity is not fixed: scalar collapse must read the
        // `=> Diff` legend, not assume `-` is expected. With `(- obtained,
        // + expected)` the labels would otherwise be swapped (misleading).
        let input = "Test run example.S started\n\
                     ==> X example.S.t 0.0s munit.ComparisonFailException: /tmp/ws/S.scala:5\n\
                     => Diff (- obtained, + expected)\n\
                     -6\n\
                     +7\n\
                     1 tests, 0 passed, 1 failed\n";
        let output = filter_bloop_test(input);
        // `-` is obtained (6), `+` is expected (7) — labels follow the legend.
        assert!(output.contains("expected 7, obtained 6"), "legend ignored: {output}");
        assert!(!output.contains("expected 6"), "labels swapped: {output}");
    }

    // --- test: real-world repos (gaps the generated corpus never exposed) ---

    #[test]
    fn test_filter_bloop_test_real_circe_munit_multiline_diff() {
        // A real `assertEquals` mismatch on a nested ADT raises a
        // `ComparisonFailException` whose `=> Diff` body leads with a *context*
        // line (`JString(`), not a `-`/`+` line. The body must be kept — it
        // carries the only obtained-vs-expected content the agent sees.
        let input = fixture!("real_circe_munit_diff_fail_3.txt");
        let output = filter_bloop_test(input);

        assert!(output.contains("2 passed, 1 failed"), "got: {output}");
        assert!(
            output.contains(
                "[FAIL] io.circe.JsonSuite.deepDropNullValues should remove null value for JsonArray"
            ),
            "got: {output}"
        );
        assert!(output.contains("=> Diff (- obtained, + expected)"), "lost diff header: {output}");
        // The diff body (context + the `-` obtained / `+` expected lines) is kept.
        assert!(output.contains("JString("), "lost diff context: {output}");
        assert!(output.contains(r#"-      value = "a""#), "lost obtained line: {output}");
        assert!(
            output.contains(r#"+      value = "REGRESSION""#),
            "lost expected line: {output}"
        );
        // Multi-line diff is not collapsed to the scalar `expected X, obtained Y`.
        assert!(!output.contains("expected JString"), "should not collapse: {output}");
        // The framework-internal stack frame is dropped.
        assert!(!output.contains("munit.Assertions.failComparison"), "stack leaked: {output}");
    }

    #[test]
    fn test_filter_bloop_test_real_enumeratum_funspec_no_false_positives() {
        // A real run with exactly one failure (nested `describe { it }` FunSpec).
        // The passing `-`-prefixed describe/it bullets and the trailing `Failed:`
        // summary must not be mis-detected as defect failures.
        let input = fixture!("real_enumeratum_scalatest_funspec_fail_3.txt");
        let output = filter_bloop_test(input);

        assert!(output.contains("181 passed, 1 failed"), "got: {output}");
        // Exactly one failure — the real one, with its assertion + location.
        assert_eq!(output.matches("[FAIL]").count(), 1, "spurious failures: {output}");
        assert!(
            output.contains("[FAIL] should result in findValues finding nothing"),
            "got: {output}"
        );
        assert!(output.contains("0 was not equal to 1 (EnumSpec.scala:11)"), "got: {output}");
        // None of the scope headers / passing bullets leaked as failures.
        assert!(!output.contains("#withName"), "scope header leaked: {output}");
        assert!(!output.contains("instance of subclass"), "passing test leaked: {output}");
        assert!(!output.contains("fail to compile"), "passing test leaked: {output}");
    }

    #[test]
    fn test_filter_bloop_test_real_cats_effect_specs2_stderr() {
        // Real cats-effect (specs2 mutable) routes the *entire* failure report to
        // stderr, so bloop tags both the `x <name>` marker and the `[E] <reason>`
        // with `[E] `. The marker must still be recognized (else it is eaten as a
        // reason, producing a false-green empty failure block).
        let input = fixture!("real_cats_effect_specs2_stderr_fail_3.txt");
        let output = filter_bloop_test(input);

        assert!(output.contains("0 passed, 1 failed"), "got: {output}");
        assert_eq!(output.matches("[FAIL]").count(), 1, "lost failure: {output}");
        assert!(output.contains("[FAIL] ExitCode.unapply is exhaustive"), "got: {output}");
        assert!(
            output.contains("0 is less than 100 (ExitCodeSpec.scala:23)"),
            "lost reason: {output}"
        );
    }

    // --- run ---

    #[test]
    fn test_filter_bloop_run_strips_noise() {
        let input = "Starting compilation server\n\
                     Bloop server started.\n\
                     Compiling app (3 Scala sources)\n\
                     Compiled app (812ms)\n\
                     Hello, World!\n\
                     Server started on port 8080\n";
        let output = filter_bloop_run(input);

        assert!(output.contains("Hello, World!"));
        assert!(output.contains("Server started on port 8080"));
        assert!(!output.contains("Bloop server started"));
        assert!(!output.contains("Compiling app"));
        assert!(!output.contains("Compiled app"));
    }

    // --- edge cases ---

    #[test]
    fn test_filter_empty_input() {
        assert!(!filter_bloop_test("").is_empty());
        assert_eq!(filter_bloop_compile(""), "bloop compile: up-to-date");
        assert!(filter_bloop_run("").is_empty());
    }

    #[test]
    fn test_filter_bloop_compile_duration_units() {
        // `COMPILED_RE` must parse both reported time forms and render them
        // correctly. Seconds form (`(2.5s)`): the project/time must be counted,
        // not silently skipped.
        let secs = filter_bloop_compile("Compiling app (3 Scala sources)\nCompiled app (2.5s)\n");
        assert!(secs.contains("3 sources"), "got: {secs}");
        assert!(secs.contains("2.5s"), "got: {secs}");

        // Millisecond form summed across a slow multi-project build must read as
        // minutes (`2m15s`), never a giant `Ns`.
        let mins = filter_bloop_compile(
            "Compiling a (10 Scala sources)\nCompiled a (90000ms)\n\
             Compiling b (5 Scala sources)\nCompiled b (45000ms)\n",
        );
        assert!(mins.contains("15 sources"), "got: {mins}");
        assert!(mins.contains("2 projects"), "got: {mins}");
        assert!(mins.contains("2m15s"), "got: {mins}"); // 90s + 45s
    }

    #[test]
    fn test_filter_bloop_compile_mixed_scala_java_sources() {
        // A mixed module lists Java sources alongside Scala
        // (`(67 Scala sources and 8 Java sources)`); both counts must be summed
        // (the closing `)` does not sit right after `Scala sources`).
        let input = "Compiling coreJVM (67 Scala sources and 8 Java sources)\n\
[E] [E1] cats/effect/ExitCode.scala:43:16\n\
[E]      method thisOverridesNothing overrides nothing\n\
Compiled coreJVM (1100ms)\n\
[E] Failed to compile 'coreJVM'\n";
        let output = filter_bloop_compile(input);
        assert!(output.contains("75 sources"), "scala+java not summed: {output}");
        assert!(!output.contains("0 sources"), "sources dropped: {output}");
        assert!(output.contains("1 error"), "got: {output}");
    }

    // --- argument handling ---

    #[test]
    fn test_bloop_restores_double_dash() {
        // clap's `trailing_var_arg` strips the `--` separator, so test-runner
        // flags after it would be lost before reaching bloop. `run_bloop_filtered`
        // restores it via `restore_double_dash`; verify the contract for a
        // representative `rtk bloop test proj -- -o junit.xml` invocation.
        let raw = vec![
            "rtk".to_string(),
            "bloop".to_string(),
            "test".to_string(),
            "proj".to_string(),
            "--".to_string(),
            "-o".to_string(),
            "junit.xml".to_string(),
        ];
        // What clap hands `run_test` (the `--` swallowed).
        let parsed = vec!["proj".to_string(), "-o".to_string(), "junit.xml".to_string()];
        let restored = args_utils::restore_double_dash_with_raw(&parsed, &raw);
        assert_eq!(
            restored,
            vec!["proj", "--", "-o", "junit.xml"],
            "the `--` separator must be restored ahead of the test-runner flags"
        );
    }
}
