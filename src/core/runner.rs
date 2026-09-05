//! Shared command execution skeleton for filter modules.

use anyhow::{Context, Result};
use regex::Regex;
use std::process::Command;
use std::sync::LazyLock;

use crate::core::ai_output::{
    prepare_emission_with_baseline, render, render_with_max_tokens, AiDocument, AiRecord,
    BudgetClass, EmissionMeta, ExactReason, Omission, OutputContract, Severity,
};
use crate::core::stream::{self, FilterMode, StdinMode, StreamFilter};
use crate::core::tracking;
use crate::core::truncate::{CAP_LIST, CAP_WARNINGS};

pub fn emit_prepared(prepared: &crate::core::ai_output::PreparedEmission) {
    print!("{}", prepared.as_str());
}

pub(crate) struct AiEmission<'a> {
    pub timer: &'a tracking::TimedExecution,
    pub original_cmd: &'a str,
    pub rtk_cmd: &'a str,
    pub raw: &'a str,
    pub fallback_baseline: &'a str,
    pub command_slug: &'a str,
    pub budget: BudgetClass,
    pub trailing_newline: bool,
}

pub(crate) fn emit_ai_document_with_baseline(
    emission: AiEmission<'_>,
    document: AiDocument,
) -> String {
    let prepared = prepare_emission_with_baseline(
        emission.raw,
        emission.fallback_baseline,
        emission.command_slug,
        render_with_max_tokens(&document, emission.budget, requested_max_tokens()),
        emission.trailing_newline,
    );
    let shown = prepared.as_str().to_string();
    let meta = prepared.meta();
    emit_prepared(&prepared);
    emission.timer.track_output(
        emission.original_cmd,
        emission.rtk_cmd,
        emission.raw,
        &shown,
        output_tracking_from_emission(OutputContract::AiOwned(emission.budget), meta),
    );
    shown
}

fn guard_framed_payload(raw: &str, candidate: &str, trailing_newline: bool) -> String {
    let framed_raw = crate::core::ai_output::frame_payload(raw, trailing_newline);
    let framed_candidate = crate::core::ai_output::frame_payload(candidate, trailing_newline);
    crate::core::guard::never_worse(&framed_raw, &framed_candidate).to_string()
}

/// Compose `filtered` with an optional recovery `hint`, cap the total at `raw`
/// (never emit more tokens than the command), print it, and return what was
/// emitted so the caller tracks exactly that.
pub fn emit_guarded(filtered: &str, hint: Option<&str>, raw: &str) -> String {
    let body = match hint {
        Some(h) => format!("{}\n{}", filtered, h),
        None => filtered.to_string(),
    };
    let framed = guard_framed_payload(raw, &body, true);
    let prepared = crate::core::ai_output::PreparedEmission::Plain {
        output: framed.clone(),
        meta: crate::core::ai_output::EmissionMeta::default(),
    };
    emit_prepared(&prepared);
    framed
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
}

pub type CaptureFilter<'a> = Box<dyn Fn(&str) -> String + 'a>;
pub type ExitAwareCaptureFilter<'a> = Box<dyn Fn(&str, i32) -> String + 'a>;
#[allow(dead_code)] // Semantic adapters are added in later foundation tasks.
pub type AiFilterResult = Result<AiDocument>;
#[allow(dead_code)] // Semantic adapters are added in later foundation tasks.
pub type AiCaptureFilter<'a> = Box<dyn Fn(&str) -> AiFilterResult + 'a>;
#[allow(dead_code)] // Semantic adapters are added in later foundation tasks.
pub type ExitAwareAiCaptureFilter<'a> = Box<dyn Fn(&str, i32) -> AiFilterResult + 'a>;

#[allow(dead_code)] // Semantic variants are consumed by later foundation tasks.
pub enum RunMode<'a> {
    Filtered(CaptureFilter<'a>),
    FilteredWithExit(ExitAwareCaptureFilter<'a>),
    AiFiltered {
        budget: BudgetClass,
        filter: AiCaptureFilter<'a>,
    },
    AiFilteredWithExit {
        budget: BudgetClass,
        filter: ExitAwareAiCaptureFilter<'a>,
    },
    Streamed(Box<dyn StreamFilter + 'a>),
    Passthrough(ExactReason),
}

#[derive(Clone, Copy)]
enum CapturedContract {
    Legacy,
    Ai(BudgetClass),
}

fn produce_document<F>(text: &str, exit_code: i32, filter_fn: F) -> AiDocument
where
    F: Fn(&str, i32) -> AiFilterResult,
{
    filter_fn(text, exit_code)
        .unwrap_or_else(|error| AiDocument::parse_failure(text, &error.to_string()))
}

const MIN_REQUESTED_MAX_TOKENS: usize = 64;
const MAX_REQUESTED_MAX_TOKENS: usize = 65_536;

fn parse_requested_max_tokens(value: Option<&str>) -> Option<usize> {
    let value = value?.parse().ok()?;
    (MIN_REQUESTED_MAX_TOKENS..=MAX_REQUESTED_MAX_TOKENS)
        .contains(&value)
        .then_some(value)
}

pub(crate) fn requested_max_tokens() -> Option<usize> {
    parse_requested_max_tokens(std::env::var("RTK_MAX_OUTPUT_TOKENS").ok().as_deref())
}

fn track_captured_emission(
    timer: tracking::TimedExecution,
    cmd_label: &str,
    raw: &str,
    shown: &str,
    contract: OutputContract,
    meta: EmissionMeta,
) {
    timer.track_output(
        cmd_label,
        &format!("rtk {}", cmd_label),
        raw,
        shown,
        output_tracking_from_emission(contract, meta),
    );
}

fn track_captured_replay(
    timer: tracking::TimedExecution,
    cmd_label: &str,
    native_byte_count: usize,
    contract: OutputContract,
) {
    let native_tokens = tracking::estimate_tokens_from_bytes(native_byte_count);
    timer.track_output_tokens(
        cmd_label,
        &format!("rtk {}", cmd_label),
        native_tokens,
        native_tokens,
        output_tracking_from_emission(
            contract,
            EmissionMeta {
                used_raw_fallback: true,
                runtime_error: Some("capture_incomplete"),
                ..EmissionMeta::default()
            },
        ),
    );
}

fn track_exact_execution(timer: tracking::TimedExecution, cmd_label: &str, reason: ExactReason) {
    timer.track_exact(
        cmd_label,
        &format!("rtk {} (passthrough)", cmd_label),
        reason.as_str(),
    );
}

pub(crate) fn output_tracking_from_emission(
    contract: OutputContract,
    meta: EmissionMeta,
) -> tracking::OutputTracking {
    tracking::OutputTracking {
        contract: contract.as_str().into(),
        exact_reason: match contract {
            OutputContract::Exact(reason) => Some(reason.as_str().into()),
            OutputContract::AiOwned(_) | OutputContract::Legacy => None,
        },
        omitted_items: meta.omitted_items,
        omitted_groups: meta.omitted_groups,
        recovery_created: meta.recovery_created,
        filter_failed: meta.parser_failed,
        runtime_error: meta.runtime_error.map(str::to_owned),
    }
}

fn run_captured_filter<F>(
    mut cmd: Command,
    tool_name: &str,
    cmd_label: &str,
    contract: CapturedContract,
    filter_fn: F,
    opts: RunOptions<'_>,
    timer: tracking::TimedExecution,
) -> Result<i32>
where
    F: Fn(&str, i32) -> AiFilterResult,
{
    let output_contract = match contract {
        CapturedContract::Legacy => OutputContract::Legacy,
        CapturedContract::Ai(budget) => OutputContract::AiOwned(budget),
    };
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

    if !result.capture_complete {
        track_captured_replay(
            timer,
            cmd_label,
            result.observed_output_bytes(),
            output_contract,
        );
        return Ok(exit_code);
    }

    if opts.filter_stdout_only {
        result
            .write_captured_stderr()
            .context("Failed to preserve captured stderr")?;
    }

    if opts.skip_filter_on_failure && exit_code != 0 {
        result
            .write_captured_stdout()
            .context("Failed to replay captured stdout")?;
        if !opts.filter_stdout_only {
            result
                .write_captured_stderr()
                .context("Failed to replay captured stderr")?;
        }
        track_captured_emission(
            timer,
            cmd_label,
            raw,
            raw,
            output_contract,
            EmissionMeta {
                used_raw_fallback: true,
                ..EmissionMeta::default()
            },
        );
        return Ok(exit_code);
    }

    let text_to_filter = if opts.filter_stdout_only {
        raw_stdout
    } else {
        raw
    };
    let document = produce_document(text_to_filter, exit_code, filter_fn);
    let requested_max_tokens = requested_max_tokens();

    let raw_for_tracking = if opts.filter_stdout_only {
        raw_stdout
    } else {
        raw
    };

    let lossless_baseline = document.lossless_baseline().unwrap_or(raw_for_tracking);
    let (shown, meta) = match contract {
        CapturedContract::Legacy => {
            let filtered = render(&document, BudgetClass::Source).text;
            let shown = if let Some(label) = opts.tee_label {
                print_with_hint(
                    &filtered,
                    raw_for_tracking,
                    raw_for_tracking,
                    label,
                    exit_code,
                )
            } else {
                let framed =
                    guard_framed_payload(raw_for_tracking, &filtered, !opts.no_trailing_newline);
                print!("{}", framed);
                framed
            };
            (shown, EmissionMeta::default())
        }
        CapturedContract::Ai(budget) => {
            let rendered = render_with_max_tokens(&document, budget, requested_max_tokens);
            let command_slug = opts.tee_label.unwrap_or(cmd_label);
            let prepared = prepare_emission_with_baseline(
                lossless_baseline,
                lossless_baseline,
                command_slug,
                rendered,
                !opts.no_trailing_newline,
            );
            let shown = prepared.as_str().to_string();
            let meta = prepared.meta();
            emit_prepared(&prepared);
            (shown, meta)
        }
    };

    let (tracking_raw, tracking_shown) = if opts.filter_stdout_only {
        (
            format!("{}{}", lossless_baseline, result.raw_stderr),
            format!("{}{}", result.raw_stderr, shown),
        )
    } else {
        (lossless_baseline.to_string(), shown)
    };
    track_captured_emission(
        timer,
        cmd_label,
        &tracking_raw,
        &tracking_shown,
        output_contract,
        meta,
    );
    Ok(exit_code)
}

pub fn run(
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
            CapturedContract::Legacy,
            move |text, _| Ok(AiDocument::legacy(filter_fn(text))),
            opts,
            timer,
        ),
        RunMode::FilteredWithExit(filter_fn) => run_captured_filter(
            cmd,
            tool_name,
            &cmd_label,
            CapturedContract::Legacy,
            move |text, exit_code| Ok(AiDocument::legacy(filter_fn(text, exit_code))),
            opts,
            timer,
        ),
        RunMode::AiFiltered { budget, filter } => run_captured_filter(
            cmd,
            tool_name,
            &cmd_label,
            CapturedContract::Ai(budget),
            move |text, _| filter(text),
            opts,
            timer,
        ),
        RunMode::AiFilteredWithExit { budget, filter } => run_captured_filter(
            cmd,
            tool_name,
            &cmd_label,
            CapturedContract::Ai(budget),
            move |text, exit_code| filter(text, exit_code),
            opts,
            timer,
        ),
        RunMode::Streamed(filter) => {
            let result =
                stream::run_streaming(&mut cmd, StdinMode::Null, FilterMode::Streaming(filter))
                    .with_context(|| format!("Failed to run {}", tool_name))?;

            if let Some(label) = opts.tee_label {
                if let Some(hint) =
                    crate::core::tee::tee_and_hint(&result.raw, label, result.exit_code)
                {
                    println!("{}", hint);
                }
            }

            timer.track(
                &cmd_label,
                &format!("rtk {}", cmd_label),
                &result.raw,
                &result.filtered,
            );
            Ok(result.exit_code)
        }
        RunMode::Passthrough(reason) => {
            let result =
                stream::run_streaming(&mut cmd, StdinMode::Inherit, FilterMode::Passthrough)
                    .with_context(|| format!("Failed to run {}", tool_name))?;

            track_exact_execution(timer, &cmd_label, reason);
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

#[allow(dead_code)] // Retained for compatibility with external adapters during migration.
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

#[allow(dead_code)] // Semantic adapters are added in later foundation tasks.
pub fn run_ai_filtered<F>(
    cmd: Command,
    tool_name: &str,
    args_display: &str,
    budget: BudgetClass,
    filter_fn: F,
    opts: RunOptions<'_>,
) -> Result<i32>
where
    F: Fn(&str) -> AiFilterResult,
{
    run(
        cmd,
        tool_name,
        args_display,
        RunMode::AiFiltered {
            budget,
            filter: Box::new(filter_fn),
        },
        opts,
    )
}

#[allow(dead_code)] // Semantic adapters are added in later foundation tasks.
pub fn run_ai_filtered_with_exit<F>(
    cmd: Command,
    tool_name: &str,
    args_display: &str,
    budget: BudgetClass,
    filter_fn: F,
    opts: RunOptions<'_>,
) -> Result<i32>
where
    F: Fn(&str, i32) -> AiFilterResult,
{
    run(
        cmd,
        tool_name,
        args_display,
        RunMode::AiFilteredWithExit {
            budget,
            filter: Box::new(filter_fn),
        },
        opts,
    )
}

/// Adapt an existing bounded string filter to the semantic output contract.
///
/// This is intentionally a migration helper, not a replacement for a
/// command-specific parser. The existing filter remains responsible for
/// deciding what content is useful; this wrapper adds status/exit facts,
/// severity ordering, omission accounting, and the shared recovery/no-worse
/// emission policy.
pub fn run_ai_from_filter<F>(
    cmd: Command,
    tool_name: &str,
    args_display: &str,
    budget: BudgetClass,
    filter_fn: F,
    opts: RunOptions<'_>,
) -> Result<i32>
where
    F: Fn(&str) -> String,
{
    run_ai_filtered_with_exit(
        cmd,
        tool_name,
        args_display,
        budget,
        move |raw, exit_code| {
            let filtered = filter_fn(raw);
            Ok(document_from_filtered(
                raw,
                &filtered,
                args_display,
                exit_code,
            ))
        },
        opts,
    )
}

/// Build a conservative semantic document around a legacy filter result.
///
/// The wrapper deliberately does not invent success when the producer failed
/// without diagnostics. It also counts removed non-empty lines as omitted
/// items so the common runner can attach a recovery reference when available.
pub fn document_from_filtered(
    raw: &str,
    filtered: &str,
    label: &str,
    exit_code: i32,
) -> AiDocument {
    let status = if exit_code == 0 { "ok" } else { "failed" };
    let mut document = AiDocument::new(Some(status));
    document.fact("command", label);
    if exit_code != 0 {
        document.fact("exit", exit_code.to_string());
    }

    let filtered = if exit_code != 0 && raw.trim().is_empty() && filtered.trim() == "ok" {
        ""
    } else {
        filtered
    };
    let raw_items = raw.lines().filter(|line| !line.trim().is_empty()).count();
    let filtered_items = filtered
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();

    if exit_code != 0 && filtered.trim().is_empty() {
        document.push(AiRecord::new(
            Severity::Error,
            "producer failed; no diagnostic text was captured",
        ));
    } else {
        for line in filtered.lines().filter(|line| !line.trim().is_empty()) {
            document.push(AiRecord::new(classify_filtered_line(line), line));
        }
    }

    if raw_items > filtered_items {
        document = document.with_omission(Omission {
            items: raw_items - filtered_items,
            groups: 0,
        });
    }
    document
}

fn classify_filtered_line(line: &str) -> Severity {
    let lower = line.to_ascii_lowercase();
    if lower.contains("error")
        || lower.contains("failed")
        || lower.contains("failure")
        || lower.contains("fatal")
    {
        Severity::Error
    } else if lower.contains("warn") || lower.contains("deprecated") {
        Severity::Warning
    } else if lower.contains("success") || lower.starts_with("ok") || lower.starts_with("passed") {
        Severity::Success
    } else {
        Severity::Info
    }
}

pub fn run_passthrough(tool: &str, args: &[std::ffi::OsString], verbose: u8) -> Result<i32> {
    run_passthrough_with_reason(tool, args, verbose, ExactReason::Unknown)
}

pub fn run_passthrough_with_reason(
    tool: &str,
    args: &[std::ffi::OsString],
    verbose: u8,
    reason: ExactReason,
) -> Result<i32> {
    if verbose > 0 {
        eprintln!("{} passthrough: {:?}", tool, args);
    }
    let mut cmd = crate::core::utils::resolved_command(tool);
    cmd.args(args);
    let args_str = tracking::args_display(args);
    run(
        cmd,
        tool,
        &args_str,
        RunMode::Passthrough(reason),
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
mod err_test_runner_tests {
    use super::*;
    use crate::core::ai_output::{
        render, AiDocument, AiRecord, BudgetClass, ExactReason, Severity,
    };
    use std::io::Write;
    use std::sync::Mutex;

    static REQUEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn direct_semantic_emission_honors_requested_token_limit() {
        let _guard = REQUEST_LOCK.lock().unwrap();
        std::env::set_var("RTK_MAX_OUTPUT_TOKENS", "64");
        let raw = "unfiltered source line\n".repeat(500);
        let mut document = AiDocument::new(Some("source"));
        for index in 0..300 {
            document.push(AiRecord::new(
                Severity::Info,
                format!("src/generated/{index:03}.rs match=value"),
            ));
        }

        let timer = tracking::TimedExecution::start();
        let shown = emit_ai_document_with_baseline(
            AiEmission {
                timer: &timer,
                original_cmd: "cat source",
                rtk_cmd: "rtk read source",
                raw: &raw,
                fallback_baseline: &raw,
                command_slug: "read",
                budget: BudgetClass::Source,
                trailing_newline: true,
            },
            document,
        );

        assert!(!shown.contains("src/generated/100.rs"));
        std::env::remove_var("RTK_MAX_OUTPUT_TOKENS");
    }

    const SEMANTIC_WRAPPER_HELPER: &str =
        "core::runner::err_test_runner_tests::semantic_wrapper_subprocess_helper";

    #[test]
    fn legacy_guard_compares_the_final_framed_payload() {
        let shown = guard_framed_payload("12345", "abcdefgh", true);

        assert_eq!(shown, "12345\n");
    }

    #[test]
    fn semantic_adapter_emits_prepared_source_document() {
        let raw = "original source that is intentionally much longer than the compact record\n"
            .repeat(32);
        let mut document = AiDocument::new(Some("source"));
        document.fact("file", "sample.rs");
        document.push(AiRecord::new(Severity::Info, "3: fn kept() {}"));

        let timer = tracking::TimedExecution::start();
        let shown = emit_ai_document_with_baseline(
            AiEmission {
                timer: &timer,
                original_cmd: "cat sample.rs",
                rtk_cmd: "rtk read sample.rs",
                raw: &raw,
                fallback_baseline: &raw,
                command_slug: "read",
                budget: BudgetClass::Source,
                trailing_newline: true,
            },
            document,
        );

        assert!(shown.contains("status=source"));
        assert!(shown.contains("file=sample.rs"));
        assert!(shown.contains("3: fn kept() {}"));
        assert!(
            crate::core::tracking::estimate_tokens(&shown)
                < crate::core::tracking::estimate_tokens(&raw)
        );
    }

    #[test]
    fn semantic_adapter_keeps_required_disclosure_baseline() {
        let raw = "visible.txt";
        let baseline = "visible.txt\n(1 filtered by policy)";
        let mut document = AiDocument::new(Some("inventory"));
        document.push(AiRecord::new(Severity::Info, "visible.txt"));
        document.push(AiRecord::new(Severity::Warning, "(1 filtered by policy)"));

        let timer = tracking::TimedExecution::start();
        let shown = emit_ai_document_with_baseline(
            AiEmission {
                timer: &timer,
                original_cmd: "find .",
                rtk_cmd: "rtk find .",
                raw,
                fallback_baseline: baseline,
                command_slug: "find",
                budget: BudgetClass::Collection,
                trailing_newline: true,
            },
            document,
        );

        assert_eq!(shown, "visible.txt\n(1 filtered by policy)\n");
    }

    #[test]
    fn semantic_wrapper_subprocess_helper() {
        if let Ok(source) = std::env::var("RTK_TEST_SEMANTIC_SOURCE") {
            match source.as_str() {
                "stdout-overflow" => {
                    let mut stdout = std::io::stdout().lock();
                    stdout.write_all(b"BEGIN\n").unwrap();
                    stdout
                        .write_all(&vec![b'x'; crate::core::stream::RAW_CAP + 1])
                        .unwrap();
                    stdout.write_all(b"\nEND\n").unwrap();
                    stdout.flush().unwrap();
                    std::process::exit(0);
                }
                "stdout-stderr-exit7" => {
                    let mut stdout = std::io::stdout().lock();
                    stdout.write_all(b"native stdout payload\n").unwrap();
                    stdout.flush().unwrap();
                    let mut stderr = std::io::stderr().lock();
                    stderr.write_all(b"native stderr payload\n").unwrap();
                    stderr.flush().unwrap();
                    std::process::exit(7);
                }
                "many-lines" => {
                    let mut stdout = std::io::stdout().lock();
                    for index in 0..500 {
                        writeln!(stdout, "diagnostic record {index}: {}", "x".repeat(48)).unwrap();
                    }
                    stdout.flush().unwrap();
                    std::process::exit(0);
                }
                other => panic!("unknown semantic source helper: {other}"),
            }
        }

        let Ok(mode) = std::env::var("RTK_TEST_SEMANTIC_WRAPPER") else {
            return;
        };
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", SEMANTIC_WRAPPER_HELPER, "--nocapture"])
            .env("RTK_TEST_SEMANTIC_SOURCE", &mode);
        let marker =
            std::env::var_os("RTK_TEST_SEMANTIC_FILTER_MARKER").map(std::path::PathBuf::from);
        let filter_fails = std::env::var_os("RTK_TEST_SEMANTIC_FILTER_FAIL").is_some();
        let options = if std::env::var_os("RTK_TEST_SEMANTIC_EARLY_FAILURE").is_some() {
            RunOptions::stdout_only().early_exit_on_failure()
        } else {
            RunOptions::stdout_only()
        };
        let exit_code = run_ai_filtered(
            command,
            "semantic-helper",
            &mode,
            BudgetClass::State,
            move |_| {
                if let Some(path) = &marker {
                    std::fs::write(path, "filter-called").unwrap();
                }
                if filter_fails {
                    Err(anyhow::anyhow!("synthetic parser failure"))
                } else {
                    Ok(AiDocument::legacy("FILTER_RAN"))
                }
            },
            options,
        )
        .unwrap();
        std::process::exit(exit_code);
    }

    #[test]
    fn semantic_wrapper_does_not_parse_or_append_to_overflow_replay() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("filter-called");
        let database = temp.path().join("tracking.db");
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", SEMANTIC_WRAPPER_HELPER, "--nocapture"])
            .env("RTK_TEST_SEMANTIC_WRAPPER", "stdout-overflow")
            .env("RTK_TEST_SEMANTIC_FILTER_MARKER", &marker)
            .env("RTK_DB_PATH", &database)
            .output()
            .unwrap();

        assert_eq!(output.status.code(), Some(0));
        assert!(
            !marker.exists(),
            "incomplete capture reached semantic parser"
        );
        let begin = output
            .stdout
            .windows(b"BEGIN\n".len())
            .position(|window| window == b"BEGIN\n")
            .expect("native replay begin marker");
        let replay = &output.stdout[begin..];
        let mut expected = b"BEGIN\n".to_vec();
        expected.extend(std::iter::repeat_n(b'x', crate::core::stream::RAW_CAP + 1));
        expected.extend_from_slice(b"\nEND\n");
        assert_eq!(replay, expected, "overflow replay must be byte-complete");

        let connection = rusqlite::Connection::open(database).unwrap();
        let stored: (String, Option<String>, i64, i64, i64, i64, bool, bool) = connection
            .query_row(
                "SELECT output_contract, exact_reason, input_tokens, output_tokens,
                        omitted_items, omitted_groups, recovery_created, filter_failed
                 FROM commands ORDER BY id DESC LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(stored.0, "ai_owned");
        assert_eq!(stored.1, None);
        assert!(stored.2 > 0, "overflow replay must retain token accounting");
        assert_eq!(stored.2, stored.3, "native replay is exact");
        assert_eq!((stored.4, stored.5), (0, 0));
        assert!(!stored.6);
        assert!(!stored.7);
    }

    #[test]
    fn semantic_wrapper_stdout_only_preserves_stderr_once_and_nonzero_exit() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("tracking.db");
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", SEMANTIC_WRAPPER_HELPER, "--nocapture"])
            .env("RTK_TEST_SEMANTIC_WRAPPER", "stdout-stderr-exit7")
            .env("RTK_DB_PATH", &database)
            .output()
            .unwrap();

        assert_eq!(output.status.code(), Some(7));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("FILTER_RAN\n"), "stdout={stdout:?}");
        assert!(
            !stdout.contains("native stderr payload"),
            "stdout={stdout:?}"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            stderr.matches("native stderr payload\n").count(),
            1,
            "stderr={stderr:?}"
        );
        let connection = rusqlite::Connection::open(database).unwrap();
        let output_tokens: i64 = connection
            .query_row(
                "SELECT output_tokens FROM commands ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            output_tokens
                >= crate::core::tracking::estimate_tokens("FILTER_RAN\nnative stderr payload\n")
                    as i64,
            "tracking must include the native stderr emission; got {output_tokens}"
        );
    }

    #[test]
    fn semantic_early_failure_tracks_definitive_contract_and_native_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("tracking.db");
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", SEMANTIC_WRAPPER_HELPER, "--nocapture"])
            .env("RTK_TEST_SEMANTIC_WRAPPER", "stdout-stderr-exit7")
            .env("RTK_TEST_SEMANTIC_EARLY_FAILURE", "1")
            .env("RTK_DB_PATH", &database)
            .output()
            .unwrap();

        assert_eq!(output.status.code(), Some(7));
        assert!(output.stdout.ends_with(b"native stdout payload\n"));
        assert_eq!(
            String::from_utf8_lossy(&output.stderr)
                .matches("native stderr payload\n")
                .count(),
            1
        );
        let connection = rusqlite::Connection::open(database).unwrap();
        let stored: (String, Option<String>, i64, i64, bool) = connection
            .query_row(
                "SELECT output_contract, exact_reason, input_tokens, output_tokens, filter_failed
                 FROM commands ORDER BY id DESC LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(stored.0, "ai_owned");
        assert_eq!(stored.1, None);
        assert_eq!(stored.2, stored.3, "native replay is exact");
        assert!(!stored.4);
    }

    #[test]
    fn semantic_parse_failure_tracks_omission_recovery_and_failure() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("tracking.db");
        let tee_dir = temp.path().join("lossless");
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", SEMANTIC_WRAPPER_HELPER, "--nocapture"])
            .env("RTK_TEST_SEMANTIC_WRAPPER", "many-lines")
            .env("RTK_TEST_SEMANTIC_FILTER_FAIL", "1")
            .env("RTK_DB_PATH", &database)
            .env("RTK_TEE_DIR", &tee_dir)
            .output()
            .unwrap();

        assert!(output.status.success());
        let connection = rusqlite::Connection::open(database).unwrap();
        let stored: (String, Option<String>, i64, i64, bool, bool) = connection
            .query_row(
                "SELECT output_contract, exact_reason, omitted_items, omitted_groups,
                        recovery_created, filter_failed
                 FROM commands ORDER BY id DESC LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(stored.0, "ai_owned");
        assert_eq!(stored.1, None);
        assert!(stored.2 > 0 || stored.3 > 0, "stored={stored:?}");
        assert!(stored.4, "stored={stored:?}");
        assert!(stored.5, "stored={stored:?}");
    }

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

    #[test]
    fn run_mode_accepts_semantic_filter() {
        let mode = RunMode::AiFiltered {
            budget: BudgetClass::State,
            filter: Box::new(|text| {
                let mut doc = AiDocument::new(Some("ok"));
                doc.fact("bytes", text.len().to_string());
                Ok(doc)
            }),
        };
        assert!(matches!(mode, RunMode::AiFiltered { .. }));
    }

    #[test]
    fn requested_semantic_token_limit_parser_rejects_out_of_range_values() {
        assert_eq!(parse_requested_max_tokens(Some("64")), Some(64));
        assert_eq!(parse_requested_max_tokens(Some("65536")), Some(65_536));
        assert_eq!(parse_requested_max_tokens(Some("63")), None);
        assert_eq!(parse_requested_max_tokens(Some("65537")), None);
        assert_eq!(parse_requested_max_tokens(Some("not-a-number")), None);
    }

    #[test]
    fn passthrough_reason_is_exposed_for_tracking() {
        assert_eq!(ExactReason::Structured.as_str(), "structured");
    }

    #[test]
    fn emission_metadata_maps_to_output_tracking() {
        let tracking = output_tracking_from_emission(
            crate::core::ai_output::OutputContract::AiOwned(BudgetClass::Diagnostic),
            EmissionMeta {
                omitted_items: 11,
                omitted_groups: 2,
                recovery_created: true,
                parser_failed: true,
                used_raw_fallback: false,
                runtime_error: Some("filter_failed"),
            },
        );

        assert_eq!(tracking.contract, "ai_owned");
        assert_eq!(tracking.exact_reason, None);
        assert_eq!(tracking.omitted_items, 11);
        assert_eq!(tracking.omitted_groups, 2);
        assert!(tracking.recovery_created);
        assert!(tracking.filter_failed);
        assert_eq!(tracking.runtime_error.as_deref(), Some("filter_failed"));
    }

    #[test]
    fn parse_failure_document_is_bounded_and_recoverable() {
        let raw = (0..500)
            .map(|n| format!("line-{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let doc = AiDocument::parse_failure(&raw, "unexpected table");
        let rendered = render(&doc, BudgetClass::Diagnostic);
        assert!(rendered
            .text
            .starts_with("status=error filter=parse-failed"));
        assert!(rendered.parser_failed);
        assert!(rendered.omission.as_ref().is_some_and(|o| o.items >= 480));
        assert!(!rendered.text.contains("line-250"));
    }

    #[test]
    fn semantic_filter_error_becomes_parse_failure_document() {
        let raw = "head\nmiddle\ntail";
        let doc = produce_document(raw, 7, |text, exit_code| {
            assert_eq!(text, raw);
            assert_eq!(exit_code, 7);
            Err(anyhow::anyhow!("unexpected table"))
        });

        let rendered = render(&doc, BudgetClass::Diagnostic);
        assert!(rendered
            .text
            .starts_with("status=error filter=parse-failed"));
        assert!(rendered.text.contains("detail=unexpected_table"));
        assert!(rendered.parser_failed);
    }

    #[test]
    fn legacy_filter_adapter_preserves_failure_without_inventing_success() {
        let document = document_from_filtered("", "ok", "fixture", 17);
        let rendered = render(&document, BudgetClass::Acknowledgement);

        assert!(rendered.text.contains("status=failed"));
        assert!(rendered.text.contains("command=fixture"));
        assert!(rendered.text.contains("exit=17"));
        assert!(rendered
            .text
            .contains("producer failed; no diagnostic text was captured"));
        assert!(!rendered.text.contains("status=ok"));
    }

    #[test]
    fn legacy_filter_adapter_declares_removed_items() {
        let document = document_from_filtered("one\ntwo\nthree", "one", "fixture", 0);
        let rendered = render(&document, BudgetClass::Collection);

        assert!(rendered.text.contains("status=ok"));
        assert!(rendered.text.contains("one"));
        assert_eq!(
            rendered.omission.as_ref().map(|omission| omission.items),
            Some(2)
        );
    }
}
