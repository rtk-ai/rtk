//! Shared command execution skeleton for filter modules.

use anyhow::{Context, Result};
use regex::Regex;
use std::borrow::Cow;
use std::process::Command;
use std::sync::LazyLock;

use crate::core::stream::{self, FilterMode, StdinMode, StreamFilter};
use crate::core::tracking;
use crate::core::truncate::{CAP_LIST, CAP_WARNINGS};

/// Compose `filtered` with an optional recovery `hint`, cap the total at `raw`
/// (never emit more tokens than the command), print it, and return what was
/// emitted so the caller tracks exactly that.
pub fn emit_guarded(filtered: &str, hint: Option<&str>, raw: &str) -> String {
    let body = match hint {
        Some(h) => format!("{}\n{}", filtered, h),
        None => filtered.to_string(),
    };
    let shown = crate::core::guard::never_worse(raw, &body).to_string();
    println!("{}", shown);
    shown
}

pub fn print_with_hint(
    filtered: &str,
    tee_raw: &str,
    guard_raw: &str,
    tee_label: &str,
    exit_code: i32,
) -> String {
    let hint = crate::core::tee::tee_and_hint(tee_raw, tee_label, exit_code);
    emit_guarded(filtered, hint.as_deref(), guard_raw)
}

#[derive(Default)]
pub struct RunOptions<'a> {
    pub tee_label: Option<&'a str>,
    pub filter_stdout_only: bool,
    pub skip_filter_on_failure: bool,
    pub no_trailing_newline: bool,
    /// Forward rtk's own stdin to the child process. Needed for commands that
    /// can read from a pipe (e.g. `cat file | rtk wc`); without it the child
    /// gets an empty stdin and reports zero.
    pub inherit_stdin: bool,
    /// What the tool prints on a clean run in the output format rtk injected, e.g. ruff's
    /// `[]`. See [`guard_stdout`]. Not consulted when `tee_label` is set.
    pub clean_outputs: &'a [&'a str],
}

impl<'a> RunOptions<'a> {
    pub fn with_tee(label: &'a str) -> Self {
        Self {
            tee_label: Some(label),
            ..Default::default()
        }
    }

    pub fn stdout_only() -> Self {
        Self {
            filter_stdout_only: true,
            ..Default::default()
        }
    }

    pub fn tee(mut self, label: &'a str) -> Self {
        self.tee_label = Some(label);
        self
    }

    pub fn early_exit_on_failure(mut self) -> Self {
        self.skip_filter_on_failure = true;
        self
    }

    pub fn no_trailing_newline(mut self) -> Self {
        self.no_trailing_newline = true;
        self
    }

    pub fn inherit_stdin(mut self) -> Self {
        self.inherit_stdin = true;
        self
    }

    pub fn clean_outputs(mut self, outputs: &'a [&'a str]) -> Self {
        self.clean_outputs = outputs;
        self
    }
}

/// The stdout to show: `filtered`, unless it costs more tokens than `raw`.
///
/// A `raw` that is exactly one of `clean_outputs` is the empty form of a format rtk injected,
/// not output the user asked for, so the filter's summary is shown even though it is longer.
fn guard_stdout<'a>(raw: &'a str, filtered: &'a str, clean_outputs: &[&str]) -> &'a str {
    if clean_outputs.contains(&raw.trim()) {
        filtered
    } else {
        crate::core::guard::never_worse(raw, filtered)
    }
}

pub type CaptureFilter<'a> = Box<dyn Fn(&str) -> String + 'a>;
pub type ExitAwareCaptureFilter<'a> = Box<dyn Fn(&str, i32) -> String + 'a>;

pub enum RunMode<'a> {
    Filtered(CaptureFilter<'a>),
    FilteredWithExit(ExitAwareCaptureFilter<'a>),
    Streamed(Box<dyn StreamFilter + 'a>),
    Passthrough,
}

fn run_captured_filter<F>(
    mut cmd: Command,
    tool_name: &str,
    cmd_label: &str,
    filter_fn: F,
    opts: RunOptions<'_>,
    timer: tracking::TimedExecution,
) -> Result<i32>
where
    F: Fn(&str, i32) -> String,
{
    let stdin_mode = if opts.inherit_stdin {
        StdinMode::Inherit
    } else {
        StdinMode::Null
    };
    let result = stream::run_streaming(&mut cmd, stdin_mode, FilterMode::CaptureOnly)
        .with_context(|| format!("Failed to run {}", tool_name))?;

    let exit_code = result.exit_code;
    let raw = &result.raw;
    let raw_stdout = &result.raw_stdout;

    if opts.skip_filter_on_failure && exit_code != 0 {
        if !result.raw_stdout.trim().is_empty() {
            print!("{}", result.raw_stdout);
        }
        if !result.raw_stderr.trim().is_empty() {
            eprint!("{}", result.raw_stderr);
        }
        timer.track(cmd_label, &format!("rtk {}", cmd_label), raw, raw);
        return Ok(exit_code);
    }

    let text_to_filter = if opts.filter_stdout_only {
        raw_stdout
    } else {
        raw
    };
    let filtered = filter_fn(text_to_filter, exit_code);

    let raw_for_tracking = if opts.filter_stdout_only {
        raw_stdout
    } else {
        raw
    };

    // The guard governs the stdout rtk replaces. stderr is forwarded separately below, and
    // only ever shrinks there, so it cannot push the total past what the command emitted.
    let shown = if let Some(label) = opts.tee_label {
        print_with_hint(&filtered, raw, raw_for_tracking, label, exit_code)
    } else {
        let guarded = guard_stdout(raw_for_tracking, &filtered, opts.clean_outputs).to_string();
        if opts.no_trailing_newline {
            print!("{}", guarded);
        } else {
            println!("{}", guarded);
        }
        guarded
    };

    // Stdout-only filters parse structured stdout; stderr still carries diagnostics
    // (config errors, missing linters) that the user needs.
    let forwarded = (opts.filter_stdout_only && !result.raw_stderr.trim().is_empty()).then(|| {
        forwarded_stderr(&result.raw_stderr, exit_code, &shown, |stderr| {
            crate::core::tee::force_tee_hint(
                stderr,
                &format!("{}-stderr", opts.tee_label.unwrap_or(tool_name)),
            )
        })
    });
    if let Some(text) = &forwarded {
        eprint!("{}", text);
    }

    // Forwarded stderr reaches the user just as much as stdout does, so counting only
    // stdout would book a passed-through stream as if the filter had removed it.
    let emitted = match &forwarded {
        Some(text) => Cow::Owned(format!("{}{}", shown, text)),
        None => Cow::Borrowed(shown.as_str()),
    };
    timer.track(cmd_label, &format!("rtk {}", cmd_label), raw, &emitted);
    Ok(exit_code)
}

/// How many stderr lines survive the cap. Warning-shaped data, so the warnings cap.
const MAX_FORWARDED_STDERR_LINES: usize = CAP_WARNINGS;

/// The stderr a stdout-only filter forwards.
///
/// Whole, whenever stderr is the report: the command failed, or the filter had nothing to show
/// for stdout. Those are the cases stderr forwarding exists for -- a golangci-lint config
/// error leaves stdout empty, and dropping its stderr left the user with no output at all.
///
/// Capped otherwise, because on a run that succeeded and reported on stdout, stderr is as
/// often progress chatter -- `go: downloading …` once per module on a cold cache -- and
/// forwarding all of it verbatim leaves rtk emitting as much as the command it stands in
/// front of.
///
/// The cap keeps the END. A tool resolves and downloads before it builds, so its chatter comes
/// first and anything it has to say -- a deprecation warning, "matched no packages" -- comes
/// after: a cap on the head keeps the noise and drops exactly the lines worth forwarding.
///
/// Three things make it give up and forward the lot. A stderr below the recovery store's own
/// floor, which is not worth a stored file and the eviction that comes with it. `recovery_hint`
/// returning `None`, meaning there is no store to point at, and a count of lines nobody can
/// read is worse than the lines (`curl_cmd` declines the same way). And a result that is not
/// actually smaller, since the note and the hint cost bytes of their own -- rtk emits no more
/// than the command it stands in front of, on either stream.
fn forwarded_stderr<'a>(
    stderr: &'a str,
    exit_code: i32,
    shown_stdout: &str,
    recovery_hint: impl FnOnce(&str) -> Option<String>,
) -> Cow<'a, str> {
    if exit_code != 0 || shown_stdout.trim().is_empty() {
        return Cow::Borrowed(stderr);
    }
    // Below the recovery store's own floor there is nothing to gain and a file to write for
    // it: the store would hold a stderr smaller than the note and hint that point at it, and
    // every write evicts an older entry. `tsc_cmd` declines on the same threshold.
    if stderr.len() < crate::core::tee::MIN_TEE_SIZE {
        return Cow::Borrowed(stderr);
    }
    let Some((offset, held_back)) = last_lines_offset(stderr, MAX_FORWARDED_STDERR_LINES) else {
        return Cow::Borrowed(stderr);
    };
    let Some(hint) = recovery_hint(stderr) else {
        return Cow::Borrowed(stderr);
    };
    let capped = format!(
        "... (+{} earlier stderr {} not shown)\n{}{}\n",
        held_back,
        if held_back == 1 { "line" } else { "lines" },
        &stderr[offset..],
        hint
    );
    if capped.len() >= stderr.len() {
        return Cow::Borrowed(stderr);
    }
    Cow::Owned(capped)
}

/// Where `text`'s last `n` lines begin, as a byte offset, and how many lines that skips.
/// `None` when it has no more than `n`.
///
/// A byte offset rather than `lines().collect()` and a re-join: `str::lines` drops the `\r` of
/// a CRLF ending and nothing puts it back, so re-joining would rewrite the line endings of a
/// stderr that happened to be long enough to cap. It also reads none of the lines it skips.
/// `\n` is ASCII, so the offset is always a char boundary.
fn last_lines_offset(text: &str, n: usize) -> Option<(usize, usize)> {
    let skipped = text.lines().count().checked_sub(n).filter(|&s| s > 0)?;
    text.match_indices('\n')
        .nth(skipped - 1)
        .map(|(index, _)| (index + 1, skipped))
}

pub fn run(
    cmd: Command,
    tool_name: &str,
    args_display: &str,
    mode: RunMode<'_>,
    opts: RunOptions<'_>,
) -> Result<i32> {
    let result = run_inner(cmd, tool_name, args_display, mode, opts);
    // #2375
    stream::die_by_relayed_signal();
    result
}

fn run_inner(
    mut cmd: Command,
    tool_name: &str,
    args_display: &str,
    mode: RunMode<'_>,
    opts: RunOptions<'_>,
) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let cmd_label = format!("{} {}", tool_name, args_display);

    match mode {
        RunMode::Filtered(filter_fn) => run_captured_filter(
            cmd,
            tool_name,
            &cmd_label,
            move |text, _| filter_fn(text),
            opts,
            timer,
        ),
        RunMode::FilteredWithExit(filter_fn) => run_captured_filter(
            cmd,
            tool_name,
            &cmd_label,
            move |text, exit_code| filter_fn(text, exit_code),
            opts,
            timer,
        ),
        RunMode::Streamed(filter) => {
            let result =
                stream::run_streaming(&mut cmd, StdinMode::Null, FilterMode::Streaming(filter))
                    .with_context(|| format!("Failed to run {}", tool_name))?;

            if let Some(label) = opts.tee_label
                && let Some(hint) =
                    crate::core::tee::tee_and_hint(&result.raw, label, result.exit_code)
            {
                println!("{}", hint);
            }

            timer.track(
                &cmd_label,
                &format!("rtk {}", cmd_label),
                &result.raw,
                &result.filtered,
            );
            Ok(result.exit_code)
        }
        RunMode::Passthrough => {
            let result =
                stream::run_streaming(&mut cmd, StdinMode::Inherit, FilterMode::Passthrough)
                    .with_context(|| format!("Failed to run {}", tool_name))?;

            timer.track_passthrough(&cmd_label, &format!("rtk {} (passthrough)", cmd_label));
            Ok(result.exit_code)
        }
    }
}

pub fn run_filtered<F>(
    cmd: Command,
    tool_name: &str,
    args_display: &str,
    filter_fn: F,
    opts: RunOptions<'_>,
) -> Result<i32>
where
    F: Fn(&str) -> String,
{
    run(
        cmd,
        tool_name,
        args_display,
        RunMode::Filtered(Box::new(filter_fn)),
        opts,
    )
}

pub fn run_filtered_with_exit<F>(
    cmd: Command,
    tool_name: &str,
    args_display: &str,
    filter_fn: F,
    opts: RunOptions<'_>,
) -> Result<i32>
where
    F: Fn(&str, i32) -> String,
{
    run(
        cmd,
        tool_name,
        args_display,
        RunMode::FilteredWithExit(Box::new(filter_fn)),
        opts,
    )
}

pub fn run_passthrough(tool: &str, args: &[std::ffi::OsString], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("{} passthrough: {:?}", tool, args);
    }
    let mut cmd = crate::core::utils::resolved_command(tool);
    crate::core::utils::ChildArgExt::child_args(&mut cmd, args);
    let args_str = tracking::args_display(args);
    run(
        cmd,
        tool,
        &args_str,
        RunMode::Passthrough,
        RunOptions::default(),
    )
}

pub fn run_streamed(
    cmd: Command,
    tool_name: &str,
    args_display: &str,
    filter: Box<dyn StreamFilter + '_>,
    opts: RunOptions<'_>,
) -> Result<i32> {
    run(
        cmd,
        tool_name,
        args_display,
        RunMode::Streamed(filter),
        opts,
    )
}

// Ecosystem-agnostic err/test command runners. Used by cargo, bun, deno, and the
// shell-string wrappers in cmds::rust::runner.

const MAX_RUNNER_FAILURES: usize = CAP_WARNINGS;
const MAX_RUNNER_LINES: usize = CAP_LIST;

static ERROR_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // Generic errors
        Regex::new(r"(?i)^.*error[\s:\[].*$").unwrap(),
        Regex::new(r"(?i)^.*\berr\b.*$").unwrap(),
        Regex::new(r"(?i)^.*warning[\s:\[].*$").unwrap(),
        Regex::new(r"(?i)^.*\bwarn\b.*$").unwrap(),
        Regex::new(r"(?i)^.*failed.*$").unwrap(),
        Regex::new(r"(?i)^.*failure.*$").unwrap(),
        Regex::new(r"(?i)^.*exception.*$").unwrap(),
        Regex::new(r"(?i)^.*panic.*$").unwrap(),
        // Rust specific
        Regex::new(r"^error\[E\d+\]:.*$").unwrap(),
        Regex::new(r"^\s*--> .*:\d+:\d+$").unwrap(),
        // Python
        Regex::new(r"^Traceback.*$").unwrap(),
        Regex::new(r#"^\s*File ".*", line \d+.*$"#).unwrap(),
        // JavaScript/TypeScript
        Regex::new(r"^\s*at .*:\d+:\d+.*$").unwrap(),
        // Go
        Regex::new(r"^.*\.go:\d+:.*$").unwrap(),
    ]
});

struct ErrorStreamFilter {
    in_error_block: bool,
    blank_count: usize,
    emitted_any: bool,
}

impl ErrorStreamFilter {
    fn new() -> Self {
        Self {
            in_error_block: false,
            blank_count: 0,
            emitted_any: false,
        }
    }
}

impl StreamFilter for ErrorStreamFilter {
    fn feed_line(&mut self, line: &str) -> Option<String> {
        let is_error = ERROR_PATTERNS.iter().any(|p| p.is_match(line));
        if is_error {
            self.in_error_block = true;
            self.blank_count = 0;
            self.emitted_any = true;
            Some(format!("{}\n", line))
        } else if self.in_error_block {
            if line.trim().is_empty() {
                self.blank_count += 1;
                if self.blank_count >= 2 {
                    self.in_error_block = false;
                    None
                } else {
                    self.emitted_any = true;
                    Some(format!("{}\n", line))
                }
            } else if line.starts_with(' ') || line.starts_with('\t') {
                self.blank_count = 0;
                self.emitted_any = true;
                Some(format!("{}\n", line))
            } else {
                self.in_error_block = false;
                None
            }
        } else {
            None
        }
    }

    fn flush(&mut self) -> String {
        String::new()
    }

    fn on_exit(&mut self, exit_code: i32, raw: &str) -> Option<String> {
        if self.emitted_any {
            return None;
        }
        if exit_code == 0 {
            Some("[ok] Command completed successfully (no errors)".to_string())
        } else {
            let mut msg = format!("[FAIL] Command failed (exit code: {})\n", exit_code);
            let lines: Vec<&str> = raw.lines().collect();
            for line in lines.iter().rev().take(10).rev() {
                msg.push_str(&format!("  {}\n", line));
            }
            Some(msg)
        }
    }
}

/// Run a prebuilt command (no shell) and filter output to show only errors/warnings.
/// `display` is used only for logging, tee keys, and tracking, never executed.
///
/// `tool` is the command the user actually ran. It keys tracking and the tee
/// slug, so passing a real name keeps `rtk gain --history` showing invocations
/// that exist and stops recovery files colliding across ecosystems.
pub fn run_err_cmd(
    cmd: Command,
    tool: &str,
    display: &str,
    tee_label: &str,
    verbose: u8,
) -> Result<i32> {
    if verbose > 0 {
        eprintln!("Running: {}", display);
    }
    run_streamed(
        cmd,
        tool,
        display,
        Box::new(ErrorStreamFilter::new()),
        RunOptions::with_tee(tee_label),
    )
}

/// Test-output ecosystem, chosen once at the boundary. Modules that know
/// their runner statically pass the variant directly; shell-string entry
/// points convert once via `detect`. Matching on the enum makes substring
/// co-firing ("cargo test" contains "go test") unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestEcosystem {
    Cargo,
    Pytest,
    Jest,
    Go,
    Bun,
    Deno,
    Unknown,
}

impl TestEcosystem {
    /// Detect from a display or shell string. First match wins; Cargo is
    /// checked before Go because "cargo test" contains "go test".
    pub fn detect(command: &str) -> Self {
        if command.contains("cargo test") {
            Self::Cargo
        } else if command.contains("pytest") {
            Self::Pytest
        } else if command.contains("jest")
            || command.contains("npm test")
            || command.contains("yarn test")
        {
            Self::Jest
        } else if command.contains("bun test") {
            Self::Bun
        } else if command.contains("deno test") {
            Self::Deno
        } else if command.contains("go test") {
            Self::Go
        } else {
            Self::Unknown
        }
    }
}

/// Watch mode never exits, and the filtered runners buffer the whole stream
/// until the child does, so a watched run prints nothing at all and loses the
/// buffer on Ctrl-C. Callers send these through unfiltered instead.
pub fn is_watch_mode(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == "--watch" || a.starts_with("--watch="))
}

/// Run a prebuilt test command (no shell), showing only failures.
/// `display` is used only for logging and tracking, never executed.
pub fn run_test_cmd(
    cmd: Command,
    tool: &str,
    display: &str,
    tee_label: &str,
    eco: TestEcosystem,
    verbose: u8,
) -> Result<i32> {
    if verbose > 0 {
        eprintln!("Running tests: {}", display);
    }
    run_filtered(
        cmd,
        tool,
        display,
        move |raw| extract_test_summary(raw, eco),
        RunOptions::with_tee(tee_label),
    )
}

#[cfg(test)]
fn filter_errors(output: &str) -> String {
    let mut result = Vec::new();
    let mut in_error_block = false;
    let mut blank_count = 0;

    for line in output.lines() {
        let is_error_line = ERROR_PATTERNS.iter().any(|p| p.is_match(line));

        if is_error_line {
            in_error_block = true;
            blank_count = 0;
            result.push(line.to_string());
        } else if in_error_block {
            if line.trim().is_empty() {
                blank_count += 1;
                if blank_count >= 2 {
                    in_error_block = false;
                } else {
                    result.push(line.to_string());
                }
            } else if line.starts_with(' ') || line.starts_with('\t') {
                result.push(line.to_string());
                blank_count = 0;
            } else {
                in_error_block = false;
            }
        }
    }

    result.join("\n")
}

/// Whether a block's ending vouches for it. `Vouched` means the ending itself
/// attributes the block to a real failure (bun's `(fail)` marker).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Close {
    Vouched,
    Plain,
}

/// Whether the runner is currently printing its own diagnostics or relaying the
/// test's stdout.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Trusted,
    Untrusted,
}

/// Which lines belong to a real failure rather than to the test's own stdout.
///
/// Bun and deno answer the same questions (what opens a diagnostic block, what
/// closes it and whether that ending vouches for it, and where the runner's own
/// output begins and ends), so they share one engine
/// and differ only in this table. As two separate state machines they drifted:
/// a guard added on one side had to be rediscovered on the other, and deno had
/// no proof rule at all, reporting passing suites as failed.
struct BlockPolicy {
    /// Opens a diagnostic block. Opening also closes any block already open.
    opens: fn(&str) -> bool,
    /// Ends the current block. A vouched close is the runner's own marker
    /// claiming what came before it.
    closes: fn(&str) -> Option<Close>,
    /// Keeps a line inside the block. Separator rules are noise.
    keeps: fn(&str) -> bool,
    /// Moves in and out of the region where the runner prints its own
    /// diagnostics. A block opened outside it needs a vouched close to count.
    section: fn(&str) -> Option<Section>,
}

/// Buffers a diagnostic block until something decides whether the runner wrote
/// it. Kept when it opened inside a trusted section, or when the runner's own
/// marker vouches for it. Anything else is the test's own stdout and is dropped.
#[derive(Default)]
struct FailureBlocks {
    pending: Vec<String>,
    open: bool,
    opened_trusted: bool,
    trusted: bool,
    kept: Vec<String>,
}

impl FailureBlocks {
    fn feed(&mut self, line: &str, trimmed: &str, policy: &BlockPolicy) {
        if let Some(section) = (policy.section)(trimmed) {
            self.close(None);
            self.trusted = section == Section::Trusted;
        }
        if let Some(close) = (policy.closes)(trimmed) {
            self.close(Some(close));
            return;
        }
        if (policy.opens)(trimmed) {
            self.close(None);
            self.open = true;
            self.opened_trusted = self.trusted;
            self.pending.push(line.to_string());
            return;
        }
        if self.open && (policy.keeps)(trimmed) {
            self.pending.push(line.to_string());
        }
    }

    fn close(&mut self, close: Option<Close>) {
        if !self.open {
            self.pending.clear();
            return;
        }
        if self.opened_trusted || close == Some(Close::Vouched) {
            self.kept.append(&mut self.pending);
        } else {
            self.pending.clear();
        }
        self.open = false;
    }
}

/// Bun writes a diagnostic before the marker it belongs to, so a block only
/// counts once a `(fail)` marker claims it. A test logging its own `error:`,
/// even with a stack, gets claimed by `(pass)` or by the counts instead.
/// Module-level failures carry no marker at all, which is what the
/// unhandled-error banner is for.
const BUN_POLICY: BlockPolicy = BlockPolicy {
    opens: |t| t.starts_with("error:"),
    closes: |t| {
        if BUN_FAILURE_MARKER.is_match(t) {
            Some(Close::Vouched)
        } else if t.starts_with("(pass)") || is_bun_count_line(t) || BUN_RAN_FOOTER.is_match(t) {
            Some(Close::Plain)
        } else {
            None
        }
    },
    // The frame carries the file and line. Without it two failing tests that
    // share a name, or a module error that has no marker at all, cannot be
    // located. Bun prints one frame per failure, so this costs a line each.
    keeps: |t| !t.is_empty() && !t.chars().all(|c| c == '-'),
    section: |t| {
        if t == "# Unhandled error between tests" {
            Some(Section::Trusted)
        } else if is_bun_count_line(t) || BUN_RAN_FOOTER.is_match(t) {
            Some(Section::Untrusted)
        } else {
            None
        }
    },
};

/// Deno's type-check diagnostics, "TS2322 [ERROR]: Type 'string' is not
/// assignable to type 'number'." They are not introduced by an `error:` line and
/// carry no ERRORS section, so nothing else in the deno policy would keep them.
static DENO_TS_ERROR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^TS\d+ \[ERROR\]:").unwrap());

/// Deno fences output it did not write between rules: "------- output -------"
/// around a test's own stdout, "------- pre-test output -------" around what a
/// module printed while loading. Returns whether the rule opens a fence.
fn deno_output_fence(trimmed: &str) -> Option<bool> {
    if !trimmed.starts_with("-----") || !trimmed.contains("output") {
        return None;
    }
    Some(!trimmed.contains("output end"))
}

/// Bun's run footer, "Ran 4 tests across 1 file. [12.00ms]". Unanchored, the
/// prefix also matches anything a test logs starting with those characters.
static BUN_RAN_FOOTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^Ran \d+ test").unwrap());

/// Bun's failure markers carry the test's duration: "(fail) name [1.86ms]",
/// "✗ name [200.27ms]". Without that suffix a line a test merely logged
/// would read as a marker, manufacturing a failure on a green run.
static BUN_FAILURE_MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\(fail\)|✗)\s.*\[[\d.]+(?:µ|m)?s\]$").unwrap());

/// Deno's FAILURES entries are "name => file:line:col". Matching a bare
/// " => " would also match an arrow function or an arrow inside an assertion
/// message, closing the diagnostic that line belongs to.
static DENO_FAILURE_ENTRY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\S.* => \S+:\d+:\d+$").unwrap());

/// Deno groups every diagnostic it writes under an ERRORS section, and fences
/// the test's own stdout between output rules. That section is the proof: deno
/// prints no per-failure marker that could vouch for a block, so one opened
/// anywhere else is the test talking.
const DENO_POLICY: BlockPolicy = BlockPolicy {
    opens: |t| t.starts_with("error:") && t != "error: Test failed",
    closes: |t| {
        // A frame ends a block, and so does the next entry's "name => file:line"
        // header: consecutive leak failures carry no frames at all.
        if t.starts_with("at ") || DENO_FAILURE_ENTRY.is_match(t) {
            Some(Close::Plain)
        } else {
            None
        }
    },
    keeps: |t| !t.is_empty() && t != "^" && !t.starts_with("throw "),
    section: |t| {
        if t == "ERRORS" {
            Some(Section::Trusted)
        } else if t == "FAILURES"
            || t.starts_with("FAILED |")
            || t.starts_with("ok |")
            || t.contains("test result:")
            || t.starts_with("failures:")
            || deno_output_fence(t).is_some()
        {
            Some(Section::Untrusted)
        } else {
            None
        }
    },
};

fn extract_test_summary(output: &str, eco: TestEcosystem) -> String {
    // Test runners colorize even when piped (deno does), so anchor on clean text.
    let cleaned = crate::core::utils::strip_ansi(output);
    let lines: Vec<&str> = cleaned.lines().collect();

    let mut result = Vec::new();
    let mut failures = Vec::new();
    let mut failure_lines = Vec::new();
    let mut in_failure = false;
    let mut in_failures_list = false;
    let mut in_test_output = false;
    let mut in_ts_error = false;
    let mut blocks = FailureBlocks::default();

    for line in lines.iter() {
        match eco {
            TestEcosystem::Cargo => {
                if line.contains("test result:") {
                    result.push(line.to_string());
                }
                if line.contains("FAILED") && !line.contains("test result") {
                    failures.push(line.to_string());
                }
                if line.starts_with("failures:") {
                    in_failure = true;
                }
                if in_failure && line.starts_with("    ") {
                    failure_lines.push(line.to_string());
                }
            }

            TestEcosystem::Pytest => {
                if line.contains(" passed") || line.contains(" failed") || line.contains(" error") {
                    result.push(line.to_string());
                }
                if line.contains("FAILED") {
                    failures.push(line.to_string());
                }
            }

            TestEcosystem::Jest => {
                if line.contains("Tests:") || line.contains("Test Suites:") {
                    result.push(line.to_string());
                }
                if line.contains("✕") || line.contains("FAIL") {
                    failures.push(line.to_string());
                }
            }

            TestEcosystem::Go => {
                if line.starts_with("ok") || line.starts_with("FAIL") || line.starts_with("---") {
                    result.push(line.to_string());
                }
                if line.contains("FAIL") {
                    failures.push(line.to_string());
                }
            }

            TestEcosystem::Bun => {
                let trimmed = line.trim_start();
                // Anchored count lines (" 6 pass", " 4 fail") and the "Ran N tests"
                // footer. A loose `contains(" fail")` also matches bun's echoed
                // source context when a test NAME contains "fails".
                if is_bun_count_line(trimmed) || BUN_RAN_FOOTER.is_match(trimmed) {
                    result.push(line.to_string());
                }
                if BUN_FAILURE_MARKER.is_match(trimmed) {
                    failures.push(line.to_string());
                }
                blocks.feed(line, trimmed, &BUN_POLICY);
            }

            TestEcosystem::Deno => {
                // Full trim: ANSI background padding leaves " FAILURES " with
                // trailing whitespace after stripping.
                let trimmed = line.trim();
                // Current deno (2.x): "FAILED | 3 passed | 2 failed (17ms)" footer,
                // " FAILURES " section listing "name => file:line:col".
                // Legacy deno mimicked cargo ("test result:", "failures:").
                // Deno fences a test's own stdout between output rules. A
                // test is free to log "FAILURES" in there, so nothing inside
                // the fence is read as deno's own bookkeeping.
                let fence = deno_output_fence(trimmed);
                if let Some(opening) = fence {
                    in_test_output = opening;
                }
                // Inside the fence the test is talking, so neither the failures
                // list nor the block engine reads it as deno's own output. The
                // rules themselves are fed, so they close anything still open.
                if fence.is_some() || !in_test_output {
                    // A type error aborts the run before any test executes, so
                    // the diagnostic and the frame under it are the only things
                    // naming what broke and where. The echoed source line and
                    // its caret are dropped like bun's.
                    if DENO_TS_ERROR.is_match(trimmed) {
                        failures.push(line.to_string());
                        in_ts_error = true;
                        continue;
                    }
                    if in_ts_error {
                        if trimmed.starts_with("at ") {
                            failures.push(line.to_string());
                            in_ts_error = false;
                            continue;
                        }
                        if trimmed.is_empty() {
                            in_ts_error = false;
                        }
                        continue;
                    }
                    if trimmed.starts_with("FAILED |")
                        || trimmed.starts_with("ok |")
                        || line.contains("test result:")
                    {
                        result.push(line.to_string());
                        in_failures_list = false;
                    } else if trimmed == "FAILURES" || line.starts_with("failures:") {
                        in_failures_list = true;
                    } else if in_failures_list && !trimmed.is_empty() {
                        failures.push(line.to_string());
                    }
                    blocks.feed(line, trimmed, &DENO_POLICY);
                }
            }

            TestEcosystem::Unknown => {}
        }
    }

    blocks.close(None);
    failure_lines.append(&mut blocks.kept);

    let mut output = String::new();

    // failure_lines can carry the only diagnostic there is: bun prints no
    // "(fail)" marker when a test file fails to load, and the count lines it
    // still prints keep the raw-tail fallback below from firing.
    if !failures.is_empty() || !failure_lines.is_empty() {
        if failures.is_empty() {
            output.push_str("[FAIL] ERRORS:\n");
        } else {
            output.push_str("[FAIL] FAILURES:\n");
        }
        for f in failures.iter().take(MAX_RUNNER_FAILURES) {
            output.push_str(&format!("  {}\n", f.trim()));
        }
        if failures.len() > MAX_RUNNER_FAILURES {
            output.push_str(&format!(
                "  ... +{} more failures\n",
                failures.len() - MAX_RUNNER_FAILURES
            ));
        }
        for f in failure_lines.iter().take(MAX_RUNNER_LINES) {
            output.push_str(&format!("  {}\n", f.trim()));
        }
        if failure_lines.len() > MAX_RUNNER_LINES {
            output.push_str(&format!(
                "  ... +{} more\n",
                failure_lines.len() - MAX_RUNNER_LINES
            ));
        }
        output.push('\n');
    }

    if !result.is_empty() {
        output.push_str("SUMMARY:\n");
        for r in &result {
            output.push_str(&format!("  {}\n", r));
        }
    } else {
        output.push_str("OUTPUT (last 5 lines):\n");
        let start = lines.len().saturating_sub(5);
        for line in &lines[start..] {
            if !line.trim().is_empty() {
                output.push_str(&format!("  {}\n", line));
            }
        }
    }

    output
}

/// True for bun's summary count lines: " 6 pass", " 4 fail", " 2 skip", " 1 todo",
/// " 1 error".
/// Anchored on the exact two-token shape so echoed source lines never match.
fn is_bun_count_line(trimmed: &str) -> bool {
    let mut parts = trimmed.split_whitespace();
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(count), Some("pass" | "fail" | "skip" | "todo" | "error"), None)
            if count.chars().all(|c| c.is_ascii_digit())
    )
}

#[cfg(test)]
mod guard_stdout_tests {
    use super::*;

    const SUMMARY: &str = "Ruff: No issues found";

    #[test]
    fn an_injected_clean_output_shows_the_summary() {
        assert_eq!(guard_stdout("[]\n", SUMMARY, &["[]"]), SUMMARY);
        assert_eq!(guard_stdout("[]\r\n", SUMMARY, &["[]"]), SUMMARY);
    }

    #[test]
    fn any_other_short_output_still_goes_through_never_worse() {
        assert_eq!(guard_stdout("{}\n", SUMMARY, &["[]"]), "{}\n");
        assert_eq!(guard_stdout("[ ]\n", SUMMARY, &["[]"]), "[ ]\n");
        assert_eq!(guard_stdout("[]\n", SUMMARY, &[]), "[]\n");
    }
}

#[cfg(test)]
mod forwarded_stderr_tests {
    use super::*;

    const HINT: &str = "[full output: ~/.local/share/rtk/tee/1_go_test-stderr.log]";

    fn with_hint(stderr: &str, exit_code: i32, shown: &str) -> String {
        forwarded_stderr(stderr, exit_code, shown, |_| Some(HINT.to_string())).into_owned()
    }

    fn chatter(lines: usize) -> String {
        (0..lines)
            .map(|i| format!("go: downloading example.com/module-{i} v1.2.3\n"))
            .collect()
    }

    /// Long enough to clear the recovery store's floor, so a test about the cap is about the
    /// cap rather than about that threshold.
    fn over_the_floor(lines: usize) -> String {
        let raw = chatter(lines);
        assert!(raw.len() >= crate::core::tee::MIN_TEE_SIZE, "{lines} lines");
        raw
    }

    #[test]
    fn a_successful_run_with_output_keeps_the_end_of_stderr() {
        // The signal in a chatty stderr sits after the chatter: resolution and downloads come
        // first, and what the tool has to say comes last.
        let mut raw = over_the_floor(150);
        raw.push_str("go: warning: \"./...\" matched no packages\n");
        raw.push_str("go: WARNING: module example.com/legacy is deprecated\n");

        let out = with_hint(&raw, 0, "ok  example.com/pkg  0.012s");
        assert!(out.contains("matched no packages"), "{out}");
        assert!(out.contains("is deprecated"), "{out}");
        assert!(
            out.starts_with("... (+142 earlier stderr lines not shown)\n"),
            "the elision note comes before what it stands for: {out}"
        );
        assert!(
            out.contains(HINT),
            "the held-back lines must stay reachable: {out}"
        );
        assert_eq!(
            out.lines().count(),
            MAX_FORWARDED_STDERR_LINES + 2,
            "note, kept lines, hint: {out}"
        );
        assert!(out.len() < raw.len() / 2, "the cap must actually compress");
    }

    #[test]
    fn a_failing_run_forwards_stderr_whole() {
        let raw = over_the_floor(150);
        assert_eq!(with_hint(&raw, 1, "some stdout"), raw);
    }

    #[test]
    fn an_empty_filtered_stdout_forwards_stderr_whole() {
        // golangci-lint's config errors land here: stdout is empty, so stderr IS the report.
        let raw = over_the_floor(150);
        assert_eq!(with_hint(&raw, 0, ""), raw);
        assert_eq!(with_hint(&raw, 0, "   \n"), raw);
    }

    #[test]
    fn a_short_stderr_is_forwarded_whole() {
        let raw = chatter(MAX_FORWARDED_STDERR_LINES);
        assert_eq!(with_hint(&raw, 0, "stdout"), raw);
        let one = "one warning\n";
        assert_eq!(with_hint(one, 0, "stdout"), one);
        assert_eq!(with_hint("", 0, "stdout"), "");
    }

    /// Under the recovery store's floor nothing is stored and nothing is capped, however many
    /// lines it runs to.
    #[test]
    fn a_stderr_under_the_recovery_floor_is_forwarded_whole() {
        let raw: String = (0..40).map(|i| format!("w{i}\n")).collect();
        assert!(raw.len() < crate::core::tee::MIN_TEE_SIZE);
        let mut asked = false;
        let out = forwarded_stderr(&raw, 0, "stdout", |_| {
            asked = true;
            Some(HINT.to_string())
        });
        assert_eq!(out, raw);
        assert!(!asked, "nothing may be written to the store for it");
    }

    /// A note and a hint cost bytes of their own. On a stderr just over the line cap they cost
    /// more than the lines they replace, and rtk would emit more than the command it replaces.
    #[test]
    fn a_cap_that_would_not_shrink_the_output_is_not_applied() {
        for lines in (MAX_FORWARDED_STDERR_LINES + 1)..=30 {
            let raw: String = (0..lines).map(|i| format!("w{i}\n")).collect();
            let out = with_hint(&raw, 0, "stdout");
            assert!(
                out.len() <= raw.len(),
                "{lines} short lines: rtk emitted {} bytes for {} of stderr",
                out.len(),
                raw.len()
            );
        }
        // And it still caps once the lines are worth removing.
        let raw = over_the_floor(40);
        assert!(with_hint(&raw, 0, "stdout").len() < raw.len());
    }

    /// No recovery store means no way to read what the cap held back, and a count of
    /// unreachable lines is worse than the lines.
    #[test]
    fn without_a_recovery_hint_stderr_is_forwarded_whole() {
        let raw = over_the_floor(150);
        assert_eq!(
            forwarded_stderr(&raw, 0, "stdout", |_| None).into_owned(),
            raw
        );
    }

    #[test]
    fn the_note_is_singular_for_one_held_back_line() {
        let mut raw = chatter(MAX_FORWARDED_STDERR_LINES + 1);
        raw.push_str(&"go: a long trailing warning that makes the cap worth applying\n".repeat(4));
        let out = with_hint(&raw, 0, "stdout");
        assert!(
            out.contains("... (+5 earlier stderr lines not shown)"),
            "{out}"
        );

        let raw = chatter(MAX_FORWARDED_STDERR_LINES).replace("module-0 v1.2.3", &"x".repeat(400))
            + "go: one more\n";
        let out = with_hint(&raw, 0, "stdout");
        assert!(
            out.contains("... (+1 earlier stderr line not shown)"),
            "{out}"
        );
    }

    #[test]
    fn capping_does_not_rewrite_crlf_line_endings() {
        // `str::lines()` drops the `\r` and nothing puts it back, so a re-joined cap would
        // change line endings for exactly the stderr that was long enough to cap.
        let raw: String = (0..60)
            .map(|i| format!("line {i} with enough text to make capping worthwhile\r\n"))
            .collect();
        let out = with_hint(&raw, 0, "stdout");
        assert_eq!(
            out.matches('\r').count(),
            MAX_FORWARDED_STDERR_LINES,
            "every forwarded line must keep its CRLF: {out:?}"
        );
    }

    #[test]
    fn last_lines_offset_slices_on_a_line_boundary() {
        assert_eq!(last_lines_offset("a\nb\nc\n", 1), Some((4, 2)));
        assert_eq!(&"a\nb\nc\n"[4..], "c\n");
        // An unterminated last line is still a line.
        assert_eq!(last_lines_offset("a\nb\nc", 1), Some((4, 2)));
        assert_eq!(&"a\nb\nc"[4..], "c");
        // Nothing to skip.
        assert_eq!(last_lines_offset("a\nb\n", 2), None);
        assert_eq!(last_lines_offset("a\nb\n", 5), None);
        assert_eq!(last_lines_offset("", 1), None);
    }

    /// `\n` is ASCII, so the offset lands on a char boundary whatever the bytes around it.
    #[test]
    fn a_non_ascii_stderr_is_sliced_without_panicking() {
        let raw: String = (0..60)
            .map(|i| format!("警告 {i}: «déprécié» 🎌 with enough text to be worth capping\n"))
            .collect();
        let out = with_hint(&raw, 0, "stdout");
        assert!(out.contains("警告 59"), "{out}");
        assert!(out.len() < raw.len());
    }
}

#[cfg(test)]
mod err_test_runner_tests {
    use super::*;

    #[test]
    fn test_filter_errors() {
        let output = "info: compiling\nerror: something failed\n  at line 10\ninfo: done";
        let filtered = filter_errors(output);
        assert!(filtered.contains("error"));
        assert!(!filtered.contains("info"));
    }

    #[test]
    fn test_extract_bun_test_failures() {
        let raw = "bun test v1.1.0\nsrc/math.test.ts:\n✗ adds numbers [1ms]\n 3 pass\n 1 fail\nRan 4 tests across 1 file.";
        let out = extract_test_summary(raw, TestEcosystem::Bun);
        assert!(out.contains("[FAIL]"), "expected failure block, got: {out}");
        assert!(out.contains("adds numbers"));
        assert!(out.contains("1 fail"));
    }

    #[test]
    fn test_extract_deno_test_failures() {
        let raw = "running 2 tests\ntest add ... ok\ntest sub ... FAILED\nfailures:\n    sub\ntest result: FAILED. 1 passed; 1 failed; 0 ignored";
        let out = extract_test_summary(raw, TestEcosystem::Deno);
        assert!(out.contains("[FAIL]"), "expected failure block, got: {out}");
        assert!(out.contains("test result:"));
    }

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    #[test]
    fn test_bun_module_load_error_survives_with_no_fail_marker() {
        // Bun prints no "(fail)" line when a test file cannot load, so the
        // diagnostic is the only thing that says why rtk is exiting non-zero.
        let raw = include_str!("../../tests/fixtures/bun_test_load_error_raw.txt");
        let out = extract_test_summary(raw, TestEcosystem::Bun);
        assert!(out.contains("[FAIL]"), "{out}");
        assert!(out.contains("Cannot find module"), "{out}");
        assert!(out.contains("0 pass"), "{out}");
        assert!(out.contains("1 fail"), "{out}");
    }

    #[test]
    fn test_is_watch_mode_detects_the_forms_that_never_exit() {
        let watched: Vec<String> = ["--watch"].iter().map(|s| s.to_string()).collect();
        assert!(is_watch_mode(&watched));

        let valued: Vec<String> = ["--watch=src"].iter().map(|s| s.to_string()).collect();
        assert!(is_watch_mode(&valued));

        // A path or flag that merely starts with the same letters is not it.
        let plain: Vec<String> = ["--watchdog", "./watch", "-w"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(!is_watch_mode(&plain));

        assert!(!is_watch_mode(&[]));
    }

    #[test]
    fn test_deno_output_fence_matches_every_variant() {
        assert_eq!(deno_output_fence("------- output -------"), Some(true));
        assert_eq!(deno_output_fence("----- output end -----"), Some(false));
        assert_eq!(
            deno_output_fence("------- post-test output -------"),
            Some(true)
        );
        assert_eq!(
            deno_output_fence("----- post-test output end -----"),
            Some(false)
        );
        assert_eq!(
            deno_output_fence("------- pre-test output -------"),
            Some(true)
        );
        assert_eq!(deno_output_fence("error: nope"), None);
        assert_eq!(deno_output_fence("-------"), None);
    }

    /// One green run per runtime whose tests log the vocabulary the runner uses
    /// for failures: bun's cross and an "error:" line with a stack, deno's
    /// FAILURES and ERRORS headers inside its output fences. None of it is the
    /// runner speaking, so none of it may reach the summary.
    /// Exact output for each consolidated capture. The targeted assertions
    /// above say why each element matters; this catches anything else moving.
    #[test]
    fn test_golden_output_for_each_runtime() {
        let cases: [(TestEcosystem, &str, &str); 4] = [
            (
                TestEcosystem::Bun,
                include_str!("../../tests/fixtures/bun_test_green_raw.txt"),
                "SUMMARY:\n   3 pass\n   0 fail\n  Ran 3 tests across 1 file. [5.00ms]\n",
            ),
            (
                TestEcosystem::Bun,
                include_str!("../../tests/fixtures/bun_test_failures_raw.txt"),
                concat!(
                    "[FAIL] FAILURES:\n",
                    "  \u{2717} t2 fails [0.13ms]\n",
                    "  \u{2717} t4 fails too [0.03ms]\n",
                    "  \u{2717} t5 throws [0.02ms]\n",
                    "  \u{2717} t6 times out [150.10ms]\n",
                    "  error: expect(received).toBe(expected)\n",
                    "  Expected: 3\n",
                    "  Received: 2\n",
                    "  at <anonymous> (/home/user/project/fails.test.ts:3:40)\n",
                    "  error: expect(received).toBe(expected)\n",
                    "  Expected: 4\n",
                    "  Received: 3\n",
                    "  at <anonymous> (/home/user/project/fails.test.ts:8:55)\n",
                    "  error: boom: connection refused\n",
                    "  at <anonymous> (/home/user/project/fails.test.ts:9:69)\n",
                    "  error: Test \"t6 times out\" timed out after 150ms\n",
                    "\n",
                    "SUMMARY:\n",
                    "   2 pass\n",
                    "   1 skip\n",
                    "   1 todo\n",
                    "   4 fail\n",
                    "  Ran 8 tests across 1 file. [157.00ms]\n",
                ),
            ),
            (
                TestEcosystem::Deno,
                include_str!("../../tests/fixtures/deno_test_green_raw.txt"),
                "SUMMARY:\n  ok | 2 passed | 0 failed (1ms)\n",
            ),
            (
                TestEcosystem::Deno,
                include_str!("../../tests/fixtures/deno_test_failures_raw.txt"),
                concat!(
                    "[FAIL] FAILURES:\n",
                    "  plain assertion => ./fails_test.ts:2:6\n",
                    "  arrow in message => ./fails_test.ts:3:6\n",
                    "  error: AssertionError: Values are not equal.\n",
                    "  [Diff] Actual / Expected\n",
                    "  -   1\n",
                    "  +   2\n",
                    "  error: AssertionError: Values are not equal: handler (a) => b must match\n",
                    "  [Diff] Actual / Expected\n",
                    "  -   x\n",
                    "  +   y\n",
                    "\n",
                    "SUMMARY:\n",
                    "  FAILED | 3 passed | 2 failed (15ms)\n",
                ),
            ),
        ];
        for (eco, raw, expected) in cases {
            assert_eq!(extract_test_summary(raw, eco), expected, "{eco:?}");
        }
    }

    #[test]
    fn test_a_green_run_is_never_reported_red() {
        for (eco, raw, ok_marker) in [
            (
                TestEcosystem::Bun,
                include_str!("../../tests/fixtures/bun_test_green_raw.txt"),
                "0 fail",
            ),
            (
                TestEcosystem::Deno,
                include_str!("../../tests/fixtures/deno_test_green_raw.txt"),
                "0 failed",
            ),
        ] {
            let out = extract_test_summary(raw, eco);
            assert!(!out.contains("[FAIL]"), "{eco:?}: {out}");
            assert!(out.contains(ok_marker), "{eco:?}: {out}");
            // Nothing a test logged may be quoted back as a diagnostic.
            for logged in ["optional dep", "legacy fixture header", "only a log line"] {
                assert!(!out.contains(logged), "{eco:?} leaked {logged}: {out}");
            }
        }
    }

    /// One red run per runtime carrying every failure shape at once.
    #[test]
    fn test_a_red_run_keeps_each_failure_and_its_reason() {
        let bun = extract_test_summary(
            include_str!("../../tests/fixtures/bun_test_failures_raw.txt"),
            TestEcosystem::Bun,
        );
        assert!(bun.contains("[FAIL]"), "{bun}");
        // Assertion failures: marker, expected/received, and the frame locating it.
        assert!(bun.contains("t2 fails"), "{bun}");
        assert!(bun.contains("Expected: 3"), "{bun}");
        assert!(bun.contains("Received: 2"), "{bun}");
        assert!(bun.contains("fails.test.ts:3:40"), "{bun}");
        // A thrown error, and a timeout, which carries no frame at all.
        assert!(bun.contains("boom: connection refused"), "{bun}");
        assert!(bun.contains("timed out after 150ms"), "{bun}");
        // Counts, including skip and todo.
        assert!(bun.contains("4 fail"), "{bun}");
        assert!(bun.contains("1 skip"), "{bun}");
        assert!(bun.contains("1 todo"), "{bun}");
        // Echoed source context, and a line a passing test logged between two
        // real failures, both stay out.
        assert!(!bun.contains("test(\"t1 passes\""), "{bun}");
        assert!(!bun.contains("logged between real failures"), "{bun}");

        let deno = extract_test_summary(
            include_str!("../../tests/fixtures/deno_test_failures_raw.txt"),
            TestEcosystem::Deno,
        );
        assert!(deno.contains("[FAIL]"), "{deno}");
        assert!(deno.contains("AssertionError"), "{deno}");
        assert!(deno.contains("[Diff]"), "{deno}");
        // An arrow inside an assertion message must not read as an entry header
        // and end the diagnostic it belongs to.
        assert!(deno.contains("handler (a) => b must match"), "{deno}");
        assert!(deno.contains("-   x"), "{deno}");
        assert!(deno.contains("+   y"), "{deno}");
        assert!(deno.contains("2 failed"), "{deno}");
    }

    #[test]
    fn test_red_run_savings_on_either_runtime() {
        for (eco, raw) in [
            (
                TestEcosystem::Bun,
                include_str!("../../tests/fixtures/bun_test_failures_raw.txt"),
            ),
            (
                TestEcosystem::Deno,
                include_str!("../../tests/fixtures/deno_test_failures_raw.txt"),
            ),
        ] {
            let out = extract_test_summary(raw, eco);
            let savings = 100.0 - (count_tokens(&out) as f64 / count_tokens(raw) as f64 * 100.0);
            assert!(savings >= 20.0, "{eco:?}: got {savings:.1}%");
        }
    }

    #[test]
    fn test_deno_typecheck_error_keeps_its_code_and_location() {
        // A type error aborts the run before any test executes: deno prints no
        // FAILURES section and no "error:"-introduced diagnostic, so the TS code
        // and the frame under it are all there is to report.
        let raw = include_str!("../../tests/fixtures/deno_test_typecheck_raw.txt");
        let out = extract_test_summary(raw, TestEcosystem::Deno);
        assert!(out.contains("TS2322 [ERROR]"), "{out}");
        assert!(out.contains("typ_test.ts:1:7"), "{out}");
        // The echoed source and its caret stay out, as they do for bun.
        assert!(!out.contains("const n: number"), "{out}");
    }

    #[test]
    fn test_consecutive_stack_free_failures_do_not_merge() {
        // Deno leak failures carry no "at " frame, so nothing but the next
        // entry header closes the block.
        let raw = include_str!("../../tests/fixtures/deno_test_twoleak_raw.txt");
        let out = extract_test_summary(raw, TestEcosystem::Deno);
        assert_eq!(out.matches("t3 leaks =>").count(), 1, "{out}");
        assert!(out.contains("FAILED | 1 passed | 2 failed"), "{out}");
        assert!(!out.contains("error: Test failed"), "{out}");
    }

    #[test]
    fn test_module_load_error_survives_on_either_runtime() {
        // Bun reports counts and no failure marker, deno reports neither, so
        // the two reach the user by different paths. Both must say why.
        let bun = extract_test_summary(
            include_str!("../../tests/fixtures/bun_test_load_error_raw.txt"),
            TestEcosystem::Bun,
        );
        assert!(bun.contains("Cannot find module"), "{bun}");
        let deno = extract_test_summary(
            include_str!("../../tests/fixtures/deno_test_load_error_raw.txt"),
            TestEcosystem::Deno,
        );
        assert!(deno.contains("Module not found"), "{deno}");
    }

    #[test]
    fn test_ecosystem_detect_first_match_wins() {
        // "cargo test" contains "go test" as a substring; the enum makes
        // that co-firing unrepresentable.
        assert_eq!(
            TestEcosystem::detect("cargo test --all"),
            TestEcosystem::Cargo
        );
        assert_eq!(TestEcosystem::detect("go test ./..."), TestEcosystem::Go);
        assert_eq!(TestEcosystem::detect("bun test"), TestEcosystem::Bun);
        assert_eq!(
            TestEcosystem::detect("deno test --allow-read"),
            TestEcosystem::Deno
        );
        assert_eq!(TestEcosystem::detect("pytest -x"), TestEcosystem::Pytest);
        assert_eq!(TestEcosystem::detect("npm test"), TestEcosystem::Jest);
        assert_eq!(TestEcosystem::detect("make check"), TestEcosystem::Unknown);
    }

    #[test]
    fn test_bun_count_line_anchoring() {
        assert!(is_bun_count_line("6 pass"));
        assert!(is_bun_count_line("0 fail"));
        assert!(is_bun_count_line("1 skip"));
        assert!(is_bun_count_line("1 todo"));
        // Echoed source naming a test "fails"/"passes" must not match.
        assert!(!is_bun_count_line(
            "3 | test(\"t2 fails\", () => { expect(1 + 1).toBe(3); });"
        ));
        assert!(!is_bun_count_line("pass"));
        assert!(!is_bun_count_line("6 passing"));
        assert!(!is_bun_count_line("x fail"));
        assert!(!is_bun_count_line("10 expect() calls"));
    }
}
