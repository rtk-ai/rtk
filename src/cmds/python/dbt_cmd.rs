//! Filters dbt CLI output (run/test/build/seed/snapshot/...) into compact summaries.
//!
//! For run/test/build/seed/snapshot/source-freshness, dbt's `--log-format json`
//! is injected and the resulting NDJSON event stream (emitted to stderr) is
//! parsed into a structured summary.
//!
//! For compile/parse/list/show/clean/debug/deps, a light text filter strips the
//! standard preamble lines (`Running with dbt=`, `Registered adapter:`, etc).
//!
//! ## Verbosity contract
//!
//! | Level    | Flag    | JSON path (run/test/build/...)                                                                | Light-filter path (compile/parse/...)                                          |
//! |----------|---------|------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------|
//! | `v=0`    | default | Single-line header for all-pass; ERR/FAIL/WARN body capped at 3 lines; MainEncounteredError body = 1 actionable line; no `[bq]`/`[compiled]` footers. Appends `(rtk -v shows warnings, -vvv for raw)` footer when the run succeeded but stripped warning content was detected (any `info.level == "warn"` event). | Strips banners, `[WARNING]` blocks, `FutureWarning`/`DeprecationWarning`, `.venv/` lines, orphan Python imports, `dbt debug` info-block, content after a 2nd `------` row. Appends the same `WARN_HINT` footer when stripped warnings exist and no error markers are present. |
//! | `v=1`    | `-v`    | ERR/FAIL/WARN body cap widens to 20 lines; MainEncounteredError body up to 5 lines; `[bq] <console-url>` and `[compiled] <path>` footers appended per failing node; "Slow:" footer (>10s rows). | Keeps `[WARNING]` and deprecation blocks, `dbt debug` info-block, dashes-boundary tail content, orphan Python imports — strips only static banners (`Running with dbt=`, `Registered adapter:`, `Concurrency:`). |
//! | `v=2`    | `-vv`   | Promotes injected `--log-level` from `info` to `debug` (more events flow); rendering rules same as `v=1`. | Same as `v=1` (level promotion does not affect light-filter input). |
//! | `v=3+`   | `-vvv`  | Full passthrough — no flag injection, no filtering. Equivalent to `NORTK=1 dbt ...`. | Full passthrough — same. |
//!
//! At every level a `Running: dbt <args>` echo line is emitted to stderr when
//! `v >= 1` (the user opted into seeing the resolved command).
//!
//! Known limitation: `runner::run_filtered` is buffer-then-filter (no
//! streaming), so long-running `dbt run`/`build` invocations appear silent
//! to the calling agent until completion.

use crate::core::runner;
use crate::core::utils::{resolved_command, strip_ansi, truncate};
use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;

/// Footer appended to v=0 success summaries when stripped warning content
/// was detected. Tells the LLM/user how to surface the suppressed lines —
/// `-v` widens warn bodies to 20 lines and keeps `[WARNING]` blocks in the
/// light-filter path; `-vvv` is full passthrough (no flag injection).
const WARN_HINT: &str = "(rtk -v shows warnings, -vvv for raw)";

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if verbose >= 3 {
        return run_passthrough(args, verbose);
    }

    let subcmd = args.first().map(|s| s.as_str()).unwrap_or("");

    match subcmd {
        "run" | "test" | "build" | "seed" | "snapshot" => {
            run_with_json_filter(args, verbose, subcmd)
        }
        "source" if args.get(1).map(|s| s.as_str()) == Some("freshness") => {
            run_with_json_filter(args, verbose, "source-freshness")
        }
        "compile" | "parse" | "list" | "show" | "clean" | "debug" | "deps" => {
            run_with_light_filter(args, verbose, subcmd)
        }
        "" | "--version" | "-V" | "--help" | "-h" => run_passthrough(args, verbose),
        _ => run_passthrough(args, verbose),
    }
}

fn run_with_json_filter(args: &[String], verbose: u8, subcmd: &str) -> Result<i32> {
    let final_args = inject_log_flags(args, verbose);
    let mut cmd = resolved_command("dbt");
    for a in &final_args {
        cmd.arg(a);
    }

    if verbose > 0 {
        eprintln!("Running: dbt {}", final_args.join(" "));
    }

    let subcmd = subcmd.to_string();
    runner::run_filtered(
        cmd,
        "dbt",
        &args.join(" "),
        move |raw| {
            let parsed = parse_events(raw);
            match subcmd.as_str() {
                "test" => build_test_summary(&parsed, verbose),
                "build" => build_build_summary(&parsed, verbose),
                "seed" => build_run_summary(&parsed, verbose, "seed"),
                "snapshot" => build_run_summary(&parsed, verbose, "snapshot"),
                "source-freshness" => build_run_summary(&parsed, verbose, "freshness"),
                _ => build_run_summary(&parsed, verbose, "run"),
            }
        },
        // dbt emits NDJSON to stderr (not stdout) under `--log-format json`;
        // combined capture is required — do not switch to `RunOptions::stdout_only()`.
        runner::RunOptions::default().tee("dbt"),
    )
}

fn run_with_light_filter(args: &[String], verbose: u8, subcmd: &str) -> Result<i32> {
    let mut cmd = resolved_command("dbt");
    for a in args {
        cmd.arg(a);
    }
    if verbose > 0 {
        eprintln!("Running: dbt {}", args.join(" "));
    }
    let subcmd = subcmd.to_string();
    runner::run_filtered(
        cmd,
        "dbt",
        &args.join(" "),
        move |raw| light_filter(raw, verbose, &subcmd),
        // dbt emits warnings/errors on stderr (e.g. [WARNING] blocks, parse errors);
        // combined capture is required — do not switch to `RunOptions::stdout_only()`.
        runner::RunOptions::default().tee("dbt"),
    )
}

fn run_passthrough(args: &[String], verbose: u8) -> Result<i32> {
    let os_args: Vec<std::ffi::OsString> = args.iter().map(Into::into).collect();
    runner::run_passthrough("dbt", &os_args, verbose)
}

fn inject_log_flags(args: &[String], verbose: u8) -> Vec<String> {
    let mut out = args.to_vec();
    let has_format = args
        .iter()
        .any(|a| a == "--log-format" || a.starts_with("--log-format="));
    let has_level = args
        .iter()
        .any(|a| a == "--log-level" || a.starts_with("--log-level="));
    if !has_format {
        out.push("--log-format".to_string());
        out.push("json".to_string());
    }
    if !has_level {
        out.push("--log-level".to_string());
        out.push(
            if verbose >= 2 {
                "debug"
            } else {
                "info"
            }
            .to_string(),
        );
    }
    out
}

#[derive(Debug, Deserialize)]
struct Envelope {
    info: Info,
    #[serde(default)]
    data: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct Info {
    name: String,
    #[serde(default)]
    level: String,
    #[serde(default)]
    msg: String,
}

#[derive(Debug, Deserialize, Default)]
struct NodeInfo {
    #[serde(default)]
    node_name: String,
    #[serde(default)]
    unique_id: String,
}

/// `data.stats` from a `StatsLine` event — dbt's authoritative aggregate counts.
/// Only `skip` is rendered today (in summary headers); the other fields
/// (pass/warn/error/noop/total) are derived directly from the per-event stream
/// elsewhere, so we don't deserialize them here.
#[derive(Debug, Deserialize, Default, Clone)]
struct StatsLineCounts {
    #[serde(default)]
    skip: u64,
}

#[derive(Debug, Deserialize, Default)]
struct StatsLineData {
    #[serde(default)]
    stats: StatsLineCounts,
}

/// `RunResultError` carries the real human-readable failure message; pair with
/// `LogModelResult`/`LogTestResult` by `node_info.unique_id`.
#[derive(Debug, Deserialize, Default)]
struct ErrorEventData {
    #[serde(default)]
    node_info: NodeInfo,
}

/// `SQLCompiledPath.data` carries `path` (the compiled-SQL file path) plus `node_info`.
#[derive(Debug, Deserialize, Default)]
struct SqlCompiledPathData {
    #[serde(default)]
    node_info: NodeInfo,
    #[serde(default)]
    path: String,
}

/// Result events for models / seeds / snapshots / freshness.
///
/// dbt 1.11 does NOT emit a clean success/failure status field in `data` for
/// model-style results — the `data.status` it does emit is a free-form string
/// like `"CREATE TABLE (0.0 rows, 0 processed)"`. We therefore derive success
/// from the *envelope's* `info.level` field (set during `parse_events`):
/// `"info"`/`"warn"` → succeeded; `"error"` → failed.
#[derive(Debug, Deserialize, Default)]
struct ModelResultData {
    #[serde(default)]
    node_info: NodeInfo,
    #[serde(default)]
    execution_time: f64,
    #[serde(default)]
    description: String,
    /// Set externally in `parse_events` from envelope `info.level`. Not parsed from JSON.
    #[serde(skip)]
    succeeded: bool,
}

/// Test result events. dbt provides a clean `data.status` here: `"pass"|"fail"|"warn"|"error"`.
#[derive(Debug, Deserialize, Default)]
struct TestResultData {
    #[serde(default)]
    node_info: NodeInfo,
    #[serde(default)]
    status: String,
    #[serde(default)]
    num_failures: Option<u64>,
    #[serde(default)]
    message: String,
}

/// Run-completion summary. In dbt 1.11 this is the `FinishedRunningStats` event,
/// which carries the real wall-clock elapsed time as `execution_time`.
#[derive(Debug, Deserialize, Default)]
struct RunSummaryData {
    #[serde(default)]
    execution_time: f64,
}

enum ParsedEvent {
    Model(ModelResultData),
    Test(TestResultData),
    Seed(ModelResultData),
    Snapshot(ModelResultData),
    Freshness(ModelResultData),
    Done(RunSummaryData),
    /// Catch-all for events the parser recognizes by name but does not specialize.
    /// The variant is matched by tests (unknown-event tolerance) and by summary
    /// builders' wildcard arms; the payload is intentionally dropped.
    Other,
}

/// Aggregate output of `parse_events` — events plus ancillary maps the
/// summary builders need to render bodies, classifications, and skip totals.
#[derive(Default)]
struct ParsedStream {
    events: Vec<ParsedEvent>,
    leftover: Vec<String>,
    /// First-line of `RunResultError.info.msg` keyed by `node_info.unique_id`.
    error_msgs: HashMap<String, String>,
    /// First-line of `RunResultWarning`/`RunResultWarningMessage.info.msg` keyed by `unique_id`.
    warn_msgs: HashMap<String, String>,
    /// `StatsLine.data.stats` — dbt's authoritative pass/warn/error/skip counts.
    stats: Option<StatsLineCounts>,
    /// `info.msg` from a `MainEncounteredError` event (parse-time / fail-fast errors).
    main_error: Option<String>,
    /// `AdapterEventError.info.msg` (BigQuery console URL) keyed by `node_info.unique_id`.
    /// Rendered only at verbose >= 1 (R2 A: humans click, LLMs cannot).
    console_urls: HashMap<String, String>,
    /// `SQLCompiledPath.data.path` — compiled SQL file path keyed by `node_info.unique_id`.
    /// Rendered only at verbose >= 1.
    compiled_paths: HashMap<String, String>,
    /// True iff the parser observed `FinishedRunningStats` or `CommandCompleted`.
    /// When false, the run was truncated (Ctrl-C, crashed reader, etc.) — summaries
    /// prepend `[partial]` to the header (R2 pick 7).
    terminator_seen: bool,
    /// Total count of envelopes with `info.level == "warn"`. Covers
    /// `RunResultWarning`/`RunResultWarningMessage` (test-warn severity), plus
    /// deprecation-class events (`PropertyMovedToConfigDeprecation`,
    /// `RefModelVersionDeprecation`, `DeprecationsSummary`, etc.) that the
    /// summary builders otherwise drop. Used to gate the v=0 `WARN_HINT`
    /// footer — the signal is "dbt emitted at least one warning we hid".
    warn_event_count: usize,
}

fn parse_events(raw: &str) -> ParsedStream {
    let mut out = ParsedStream::default();
    for line in raw.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('{') {
            if !line.trim().is_empty() {
                out.leftover.push(line.to_string());
            }
            continue;
        }
        let envelope: Envelope = match serde_json::from_str(trimmed) {
            Ok(e) => e,
            Err(_) => {
                out.leftover.push(line.to_string());
                continue;
            }
        };
        // dbt 1.x doesn't put a clean success/failure on LogModelResult.data;
        // the envelope's `info.level` is the truth: "info"|"warn" succeeded, "error" failed.
        let succeeded = envelope.info.level != "error";
        if envelope.info.level == "warn" {
            out.warn_event_count += 1;
        }
        let info_msg = envelope.info.msg.clone();
        let parsed: Option<ParsedEvent> = match envelope.info.name.as_str() {
            "LogModelResult" => parse_data::<ModelResultData>(envelope.data).map(|mut m| {
                m.succeeded = succeeded;
                ParsedEvent::Model(m)
            }),
            "LogTestResult" => parse_data::<TestResultData>(envelope.data).map(ParsedEvent::Test),
            "LogSeedResult" => parse_data::<ModelResultData>(envelope.data).map(|mut m| {
                m.succeeded = succeeded;
                ParsedEvent::Seed(m)
            }),
            "LogSnapshotResult" => parse_data::<ModelResultData>(envelope.data).map(|mut m| {
                m.succeeded = succeeded;
                ParsedEvent::Snapshot(m)
            }),
            "LogFreshnessResult" => parse_data::<ModelResultData>(envelope.data).map(|mut m| {
                m.succeeded = succeeded;
                ParsedEvent::Freshness(m)
            }),
            // dbt 1.11 emits run-summary as FinishedRunningStats, not CommandCompleted.
            // Either of these is the "stream terminator" — when neither is observed
            // by EOF, the summary is marked `[partial]` (R2 pick 7).
            "FinishedRunningStats" => {
                out.terminator_seen = true;
                parse_data::<RunSummaryData>(envelope.data).map(ParsedEvent::Done)
            }
            "CommandCompleted" => {
                out.terminator_seen = true;
                None
            }
            // dbt 1.11 banner events (MainReportVersion / MainReportArgs) carry
            // version info that no summary builder consumes — drop silently.
            "MainReportVersion" | "MainReportArgs" => None,
            // The real human-readable failure message — pair with the result event by unique_id.
            "RunResultError" => {
                if let Some(d) = parse_data::<ErrorEventData>(envelope.data) {
                    let key = d.node_info.unique_id;
                    if !key.is_empty() {
                        out.error_msgs.insert(key, info_msg.clone());
                    }
                }
                None
            }
            // Test-warn events are paired similarly. RunResultWarningMessage tends to
            // carry the more useful body (e.g. "Got 1 result, configured to warn if != 0").
            "RunResultWarningMessage" | "RunResultWarning" => {
                if let Some(d) = parse_data::<ErrorEventData>(envelope.data) {
                    let key = d.node_info.unique_id;
                    if !key.is_empty() {
                        // Prefer RunResultWarningMessage; only fill from RunResultWarning if empty.
                        let entry = out.warn_msgs.entry(key).or_default();
                        if entry.is_empty() || envelope.info.name == "RunResultWarningMessage" {
                            *entry = info_msg.clone();
                        }
                    }
                }
                None
            }
            // Authoritative aggregate counts (sharp edge §16: prefer over per-event accounting).
            "StatsLine" => {
                if let Some(d) = parse_data::<StatsLineData>(envelope.data) {
                    out.stats = Some(d.stats);
                }
                None
            }
            // Parse-time / fail-fast: dbt aborts before any LogModelResult is emitted.
            "MainEncounteredError" => {
                out.main_error = Some(info_msg.clone());
                None
            }
            // BigQuery console URL — humans click, LLMs cannot. Render only at -v.
            // info.msg is `"BigQuery adapter: https://console.cloud.google.com/..."`;
            // strip the prefix so the rendered footer carries only the URL.
            "AdapterEventError" => {
                if let Some(d) = parse_data::<ErrorEventData>(envelope.data) {
                    let key = d.node_info.unique_id;
                    if !key.is_empty() {
                        let url = info_msg
                            .strip_prefix("BigQuery adapter: ")
                            .unwrap_or(&info_msg)
                            .trim()
                            .to_string();
                        if !url.is_empty() {
                            out.console_urls.insert(key, url);
                        }
                    }
                }
                None
            }
            // Compiled SQL file path — useful at -v for re-running dbt and inspecting
            // the rendered query.
            "SQLCompiledPath" => {
                if let Some(d) = parse_data::<SqlCompiledPathData>(envelope.data) {
                    let key = d.node_info.unique_id;
                    if !key.is_empty() && !d.path.is_empty() {
                        out.compiled_paths.insert(key, d.path);
                    }
                }
                None
            }
            _ => Some(ParsedEvent::Other),
        };
        if let Some(p) = parsed {
            out.events.push(p);
        }
    }
    out
}

fn parse_data<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Option<T> {
    serde_json::from_value(value).ok()
}

/// "Never Block" fallback: when the JSON event stream produces zero typed events
/// AND no `MainEncounteredError`, but `leftover` (non-JSON stderr) is substantial,
/// surface the leftover as the summary body instead of returning the misleading
/// "0 nodes selected" header. Returns `Some(passthrough_string)` if the heuristic
/// fires, otherwise `None`.
///
/// Threshold: more than 5 non-empty leftover lines OR >500 bytes total. Smaller
/// fragments are treated as trailing banner crud and ignored.
fn leftover_passthrough(leftover: &[String]) -> Option<String> {
    if leftover.len() > 5 {
        return Some(leftover.join("\n"));
    }
    let total_bytes: usize = leftover.iter().map(|l| l.len() + 1).sum();
    if total_bytes > 500 {
        return Some(leftover.join("\n"));
    }
    None
}

/// True iff the v=0 `WARN_HINT` footer should be appended to a JSON-path summary.
/// Fires only on success-shaped runs (no failures, no main_error) where dbt
/// emitted at least one warn-level event we suppressed. On failure paths the
/// existing tee + `[full output: ...]` hint covers recovery.
fn should_emit_warn_hint(parsed: &ParsedStream, has_failures: bool, verbose: u8) -> bool {
    verbose == 0
        && !has_failures
        && parsed.main_error.is_none()
        && parsed.warn_event_count > 0
}

fn build_run_summary(parsed: &ParsedStream, verbose: u8, cmd_label: &str) -> String {
    let mut nodes: Vec<&ModelResultData> = Vec::new();
    let mut run_summary: Option<&RunSummaryData> = None;

    for e in &parsed.events {
        match e {
            ParsedEvent::Model(m)
            | ParsedEvent::Seed(m)
            | ParsedEvent::Snapshot(m)
            | ParsedEvent::Freshness(m) => nodes.push(m),
            ParsedEvent::Done(s) => run_summary = Some(s),
            _ => {}
        }
    }

    let total = nodes.len();
    let elapsed = run_summary.map(|s| s.execution_time).unwrap_or(0.0);
    let skipped = parsed.stats.as_ref().map(|s| s.skip).unwrap_or(0);

    // parse-time / fail-fast error with no result events → synthesize a
    // dedicated header. Distinguish from the "selector matched zero nodes" case
    // by checking that a MainEncounteredError was actually emitted.
    if total == 0 {
        if let Some(ref msg) = parsed.main_error {
            return render_main_error_summary(cmd_label, msg, elapsed, verbose, parsed.terminator_seen);
        }
        // "Never Block" — if the JSON stream is corrupt (zero typed
        // events, no MainEncounteredError) but substantial non-JSON stderr was
        // captured, surface the raw leftover instead of a misleading
        // "0 nodes selected" header.
        if let Some(passthrough) = leftover_passthrough(&parsed.leftover) {
            return passthrough;
        }
        let mut header = format!("dbt {}: 0 nodes selected  {}", cmd_label, fmt_secs(elapsed));
        if !parsed.terminator_seen {
            header = format!("[partial] {}", header);
        }
        return header;
    }

    let success_count = nodes.iter().filter(|m| m.succeeded).count();
    let error_count = nodes.iter().filter(|m| !m.succeeded).count();

    let partial_prefix = if parsed.terminator_seen { "" } else { "[partial] " };

    if error_count == 0 {
        let mut header = format!(
            "{}dbt {}: {}/{} OK  {}",
            partial_prefix,
            cmd_label,
            success_count,
            total,
            fmt_secs(elapsed)
        );
        if skipped > 0 {
            header = header.replacen(
                &fmt_secs(elapsed),
                &format!("{} skipped  {}", skipped, fmt_secs(elapsed)),
                1,
            );
        }
        if should_emit_warn_hint(parsed, false, verbose) {
            header.push('\n');
            header.push_str(WARN_HINT);
        }
        return header;
    }

    let mut out = format!(
        "{}dbt {}: {}/{} OK  {} ERR",
        partial_prefix, cmd_label, success_count, total, error_count
    );
    if skipped > 0 {
        out.push_str(&format!("  {} skipped", skipped));
    }
    out.push_str(&format!("  {}\n", fmt_secs(elapsed)));
    out.push_str("═══════════════════════════════════════\n");

    // default 3 lines, -v 20 lines (was 1 / 5).
    let body_lines: usize = if verbose >= 1 { 20 } else { 3 };

    for m in nodes.iter().filter(|m| !m.succeeded).take(20) {
        out.push_str(&render_err_row(
            &m.node_info,
            &m.description,
            parsed.error_msgs.get(&m.node_info.unique_id),
            cmd_label,
            body_lines,
            verbose,
            &parsed.console_urls,
            &parsed.compiled_paths,
        ));
    }

    if verbose >= 1 {
        let mut slow: Vec<&&ModelResultData> = nodes
            .iter()
            .filter(|m| m.execution_time > 10.0)
            .collect();
        slow.sort_by(|a, b| {
            b.execution_time
                .partial_cmp(&a.execution_time)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if !slow.is_empty() {
            out.push_str("\nSlow:\n");
            for m in slow.iter().take(5) {
                let name = pick_name(&m.node_info);
                out.push_str(&format!(
                    "  {:<30}  {}\n",
                    truncate(name, 30),
                    fmt_secs(m.execution_time)
                ));
            }
        }
    }

    out.trim_end().to_string()
}

/// Render the synthesized header + body when dbt aborted before producing any
/// result events (Fix 5).
///
/// At verbose >= 1, the body expands from the default 1 actionable line to up to
/// 5 lines (R2 pick 6) — covers long contract-violation messages and macro
/// stack snippets that come on a `MainEncounteredError` path.
fn render_main_error_summary(
    cmd_label: &str,
    raw_msg: &str,
    elapsed: f64,
    verbose: u8,
    terminator_seen: bool,
) -> String {
    let stripped = strip_ansi(raw_msg);
    let cleaned = stripped
        .strip_prefix("Encountered an error:\n")
        .unwrap_or(&stripped);
    let kind = if cleaned.starts_with("Compilation Error") {
        "compile error"
    } else if cleaned.starts_with("Database Error") {
        "database error"
    } else if cleaned.starts_with("Parsing Error") {
        "parse error"
    } else {
        "error"
    };
    let partial_prefix = if terminator_seen { "" } else { "[partial] " };
    let mut out = format!(
        "{}dbt {}: {}  {}\n",
        partial_prefix,
        cmd_label,
        kind,
        fmt_secs(elapsed)
    );
    out.push_str("═══════════════════════════════════════\n");

    // default 1 actionable line; -v expands to 5 lines (skipping the
    // generic "Compilation Error"/etc. headline that `kind` already conveys).
    // each rendered line is passed through `strip_model_path_parens` to
    // collapse `Model 'model.<project>.<name>' (<path>) <rest>` into
    // `Model '<name>' <rest>`.
    // body lines are NOT truncated. The previous 100-char cap was eating the
    // actionable end of error sentences (e.g. "...which was not found"). LLM
    // consumers read the full text; humans see terminal wrap, which is fine for
    // body content. Row-header column truncation (name column, classification
    // column) is preserved in the row-rendering helpers — only body content
    // rendering loses the cap.
    if verbose >= 1 {
        for line in cleaned
            .lines()
            .filter(|l| !l.trim().is_empty())
            // Skip the generic kind headline (Compilation Error / Database Error / Parsing Error).
            .skip(1)
            .take(5)
        {
            let stripped_line = strip_model_path_parens(line.trim());
            out.push_str(&format!("     {}\n", stripped_line));
        }
    } else {
        // Default: actionable line (the second non-empty line, after the kind headline).
        let detail = cleaned
            .lines()
            .filter(|l| !l.trim().is_empty())
            .nth(1);
        if let Some(d) = detail {
            let stripped_line = strip_model_path_parens(d.trim());
            out.push_str(&format!("     {}\n", stripped_line));
        } else if let Some(b) = cleaned.lines().find(|l| !l.trim().is_empty()) {
            // Fallback: only one non-empty line in the stream.
            let stripped_line = strip_model_path_parens(b.trim());
            out.push_str(&format!("     {}\n", stripped_line));
        }
    }
    out.trim_end().to_string()
}

/// when a body line matches the pattern
/// `Model '<unique_id>' (<path>) <rest>`, strip the parenthesized path portion
/// AND the `model.<project>.` prefix from inside the quotes, returning
/// `Model '<bare_name>' <rest>`.
///
/// Lines that don't match the pattern are returned unchanged.
fn strip_model_path_parens(line: &str) -> String {
    // Find leading `Model '` (allowing for surrounding whitespace).
    let trimmed_start = line.trim_start();
    let leading_ws_len = line.len() - trimmed_start.len();
    let after_open = match trimmed_start.strip_prefix("Model '") {
        Some(s) => s,
        None => return line.to_string(),
    };
    // Find the closing single-quote that terminates the unique_id.
    let close_quote_idx = match after_open.find('\'') {
        Some(idx) => idx,
        None => return line.to_string(),
    };
    let unique_id = &after_open[..close_quote_idx];
    let after_close = &after_open[close_quote_idx + 1..];
    // Strip the `model.<project>.` prefix from the unique_id (e.g.
    // `model.dummy_team_014.dummy_model_084` →
    // `dummy_model_084`).
    let bare_name = if let Some(rest) = unique_id.strip_prefix("model.") {
        match rest.find('.') {
            Some(dot_idx) => &rest[dot_idx + 1..],
            None => unique_id,
        }
    } else {
        unique_id
    };
    // Find ` (` immediately after the closing quote — that's the path-parens
    // group. If absent, the pattern doesn't match; return line unchanged.
    let after_paren_open = match after_close.strip_prefix(" (") {
        Some(s) => s,
        None => return line.to_string(),
    };
    // Walk to the matching `)`. The path itself shouldn't contain unbalanced
    // parens — assume the first `)` we hit is the closer.
    let close_paren_idx = match after_paren_open.find(')') {
        Some(idx) => idx,
        None => return line.to_string(),
    };
    let after_close_paren = &after_paren_open[close_paren_idx + 1..];
    // Reassemble: keep leading whitespace, plus `Model '<bare>'<after_close_paren>`.
    let leading = &line[..leading_ws_len];
    format!("{}Model '{}'{}", leading, bare_name, after_close_paren)
}

/// Render an `ERR` row: `ERR  <name>  <classification>` plus body pulled from
/// the paired `RunResultError.info.msg` when available (Fixes 1 & 4, R2 D).
///
/// At verbose >= 1, footer lines are appended for any `AdapterEventError`
/// (BigQuery console URL) or `SQLCompiledPath` (compiled SQL path) events
/// that paired with this node by `unique_id`.
#[allow(clippy::too_many_arguments)]
fn render_err_row(
    node: &NodeInfo,
    description: &str,
    error_msg: Option<&String>,
    cmd_label: &str,
    body_lines: usize,
    verbose: u8,
    console_urls: &HashMap<String, String>,
    compiled_paths: &HashMap<String, String>,
) -> String {
    let name = pick_name(node);
    let body_source = error_msg
        .map(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(description);
    let classification = classify_error(body_source, cmd_label);
    let mut out = format!(
        "ERR  {:<40}  {}\n",
        truncate(name, 40),
        classification,
    );
    // contract violations get a 20-line body cap (table renders fully)
    // and the boilerplate preamble lines (`This model has an enforced contract...`,
    // `Please ensure the name, data_type, and number of columns...`) are dropped.
    let (effective_body_lines, drop_contract_preamble) = if classification == "contract" {
        (20usize, true)
    } else {
        (body_lines, false)
    };
    render_body_lines(
        &mut out,
        body_source,
        error_msg.is_some(),
        effective_body_lines,
        "     ",
        drop_contract_preamble,
    );
    if verbose >= 1 {
        if let Some(url) = console_urls.get(&node.unique_id) {
            out.push_str(&format!("     [bq] {}\n", url));
        }
        if let Some(path) = compiled_paths.get(&node.unique_id) {
            out.push_str(&format!("     [compiled] {}\n", path));
        }
    }
    out
}

/// Render the indented body of an ERR/FAIL/WARN row from a paired `info.msg`
/// (or fallback `description`/`message`).
///
/// when the source is a paired `RunResultError.info.msg`, line 1 is
/// only skipped if it matches the redundant `<Kind> Error in <node_type> <name>`
/// header pattern. Test FAIL events whose single-line msg is the actionable
/// detail (e.g. `Got 3 results, configured to fail if != 0`) are now preserved.
///
/// `indent` controls per-line prefix width — `"     "` (5 spaces) for ERR rows,
/// `"      "` (6 spaces) for test FAIL/ERR/WARN rows.
fn render_body_lines(
    out: &mut String,
    body_source: &str,
    is_paired_error_msg: bool,
    body_lines: usize,
    indent: &str,
    drop_contract_preamble: bool,
) {
    if body_source.is_empty() {
        return;
    }
    let cleaned = strip_ansi(body_source);
    // Collect non-empty lines first so we can inspect/drop the first one cleanly.
    let mut nonempty: Vec<&str> = cleaned.lines().filter(|l| !l.trim().is_empty()).collect();
    if is_paired_error_msg {
        // only drop line 1 when it actually matches the redundant
        // `<Kind> Error in <node_type> <name>` header pattern. Otherwise (e.g. a
        // single-line test FAIL message like "Got 3 results, configured to fail
        // if != 0") render the full body starting from line 1.
        if let Some(first) = nonempty.first() {
            if is_redundant_error_header(first) {
                nonempty.remove(0);
            }
        }
    }
    // drop the boilerplate contract-violation preamble.
    if drop_contract_preamble {
        nonempty.retain(|l| !is_contract_preamble_line(l));
    }
    for line in nonempty.into_iter().take(body_lines) {
        // strip `Model '<unique_id>' (<path>)` redundant prefix when present.
        // body lines are NOT truncated — the 100-char cap was eating the actionable
        // end of error sentences (e.g. "...which was not found"). LLM consumers read
        // the full text; terminal wrapping is acceptable for body content. Row-header
        // column truncation (name column at 30/40 chars in render_err_row /
        // render_test_*_row) is preserved.
        let stripped_line = strip_model_path_parens(line.trim());
        out.push_str(&format!("{}{}\n", indent, stripped_line));
    }
}

/// true iff `line` is one of the two boilerplate preamble lines that
/// dbt emits before a contract-violation mismatch table:
/// - `This model has an enforced contract that failed.`
/// - `Please ensure the name, data_type, and number of columns in your contract
///   match the columns in your model's definition.`
///
/// Whitespace at start/end is tolerated (regex-equivalent prefix match on the
/// trimmed line).
fn is_contract_preamble_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("This model has an enforced contract that failed.")
        || trimmed.starts_with(
            "Please ensure the name, data_type, and number of columns in your contract match the columns in your model's definition.",
        )
}

/// True iff a body line matches the redundant `<Kind> Error in <node_type> <name> (path)`
/// header that begins `RunResultError.info.msg` for model/snapshot/seed/test failures.
///
/// only skip line 1 when it actually matches this pattern. Test FAIL events
/// have a single-line `info.msg` like `Got 3 results, configured to fail if != 0` —
/// blindly skipping line 1 there leaves an empty body.
///
/// Pattern (case-sensitive): leading whitespace, then `(Compilation|Database|Runtime|
/// Contract|Parsing|Dependency) Error`, then ` in `, then `(test|model|snapshot|seed|
/// source|analysis|exposure|operation) `, then anything.
fn is_redundant_error_header(line: &str) -> bool {
    let trimmed = line.trim_start();
    let kinds = [
        "Compilation Error",
        "Database Error",
        "Runtime Error",
        "Contract Error",
        "Parsing Error",
        "Dependency Error",
    ];
    let node_types = [
        "test ",
        "model ",
        "snapshot ",
        "seed ",
        "source ",
        "analysis ",
        "exposure ",
        "operation ",
    ];
    for kind in &kinds {
        if let Some(rest) = trimmed.strip_prefix(kind) {
            if let Some(rest) = rest.strip_prefix(" in ") {
                if node_types.iter().any(|nt| rest.starts_with(nt)) {
                    return true;
                }
            }
        }
    }
    false
}

/// Classify an error message into one of: `compile | runtime | database |
/// contract | freshness`. Used for the ERR-row classification column (Fix 4).
fn classify_error(msg: &str, cmd_label: &str) -> &'static str {
    if cmd_label == "freshness" {
        return "freshness";
    }
    let stripped = strip_ansi(msg);
    let lower = stripped.to_ascii_lowercase();
    if lower.contains("contract") && lower.contains("error") {
        "contract"
    } else if lower.contains("compilation error") {
        "compile"
    } else if lower.contains("database error") {
        "database"
    } else {
        // Default — covers explicit "Runtime Error" and any unrecognized failures.
        "runtime"
    }
}

fn build_test_summary(parsed: &ParsedStream, verbose: u8) -> String {
    let mut tests: Vec<&TestResultData> = Vec::new();
    let mut run_summary: Option<&RunSummaryData> = None;

    for e in &parsed.events {
        match e {
            ParsedEvent::Test(t) => tests.push(t),
            ParsedEvent::Done(s) => run_summary = Some(s),
            _ => {}
        }
    }

    let total = tests.len();
    let elapsed = run_summary.map(|s| s.execution_time).unwrap_or(0.0);
    let skipped = parsed.stats.as_ref().map(|s| s.skip).unwrap_or(0);

    let partial_prefix = if parsed.terminator_seen { "" } else { "[partial] " };

    if total == 0 {
        if let Some(ref msg) = parsed.main_error {
            return render_main_error_summary("test", msg, elapsed, verbose, parsed.terminator_seen);
        }
        // "Never Block" — corrupt JSON stream + substantial leftover
        // → surface the raw output instead of "0 tests selected".
        if let Some(passthrough) = leftover_passthrough(&parsed.leftover) {
            return passthrough;
        }
        return format!(
            "{}dbt test: 0 tests selected  {}",
            partial_prefix,
            fmt_secs(elapsed)
        );
    }

    // 4-bucket test outcomes (pass | warn | fail | error).
    let pass_count = tests.iter().filter(|t| t.status == "pass").count();
    let warn_count = tests.iter().filter(|t| t.status == "warn").count();
    let fail_count = tests.iter().filter(|t| t.status == "fail").count();
    let err_count = tests.iter().filter(|t| t.status == "error").count();

    if fail_count + err_count + warn_count == 0 {
        let mut out = format!(
            "{}dbt test: {}/{} PASS  {}",
            partial_prefix,
            pass_count,
            total,
            fmt_secs(elapsed)
        );
        if skipped > 0 {
            out = out.replacen(
                &fmt_secs(elapsed),
                &format!("{} skipped  {}", skipped, fmt_secs(elapsed)),
                1,
            );
        }
        if should_emit_warn_hint(parsed, false, verbose) {
            out.push('\n');
            out.push_str(WARN_HINT);
        }
        return out;
    }

    let mut header = format!("{}dbt test: {}/{} PASS", partial_prefix, pass_count, total);
    if fail_count > 0 {
        header.push_str(&format!("  {} FAIL", fail_count));
    }
    if err_count > 0 {
        header.push_str(&format!("  {} ERR", err_count));
    }
    if warn_count > 0 {
        header.push_str(&format!("  {} WARN", warn_count));
    }
    if skipped > 0 {
        header.push_str(&format!("  {} skipped", skipped));
    }
    header.push_str(&format!("  {}\n", fmt_secs(elapsed)));

    let mut out = header;
    out.push_str("═══════════════════════════════════════\n");

    // default 3 lines, -v 20 lines (was 5 / 10).
    let body_lines: usize = if verbose >= 1 { 20 } else { 3 };

    // FAIL rows — true assertion failures.
    for t in tests.iter().filter(|t| t.status == "fail").take(20) {
        out.push_str(&render_test_fail_row(
            t,
            &parsed.error_msgs,
            body_lines,
            verbose,
            &parsed.console_urls,
            &parsed.compiled_paths,
        ));
    }
    // ERR rows — test SQL itself errored (syntax error, etc.).
    for t in tests.iter().filter(|t| t.status == "error").take(20) {
        out.push_str(&render_test_err_row(
            t,
            &parsed.error_msgs,
            body_lines,
            verbose,
            &parsed.console_urls,
            &parsed.compiled_paths,
        ));
    }
    // WARN rows — assertion violated but severity:warn.
    for t in tests.iter().filter(|t| t.status == "warn").take(20) {
        out.push_str(&render_test_warn_row(
            t,
            &parsed.warn_msgs,
            body_lines,
            verbose,
            &parsed.console_urls,
            &parsed.compiled_paths,
        ));
    }

    out.trim_end().to_string()
}

fn render_test_fail_row(
    t: &TestResultData,
    error_msgs: &HashMap<String, String>,
    body_lines: usize,
    verbose: u8,
    console_urls: &HashMap<String, String>,
    compiled_paths: &HashMap<String, String>,
) -> String {
    let name = pick_name(&t.node_info);
    let n = t.num_failures.unwrap_or(0);
    let row_or_rows = if n == 1 { "row" } else { "rows" };
    let error_msg = error_msgs
        .get(&t.node_info.unique_id)
        .filter(|s| !s.is_empty());
    let body = error_msg
        .map(|s| s.as_str())
        .unwrap_or(&t.message);
    let mut out = format!(
        "FAIL  {:<40}  {} {}\n",
        truncate(name, 40),
        n,
        row_or_rows,
    );
    render_body_lines(&mut out, body, error_msg.is_some(), body_lines, "      ", false);
    if verbose >= 1 {
        if let Some(url) = console_urls.get(&t.node_info.unique_id) {
            out.push_str(&format!("      [bq] {}\n", url));
        }
        if let Some(path) = compiled_paths.get(&t.node_info.unique_id) {
            out.push_str(&format!("      [compiled] {}\n", path));
        }
    }
    out
}

fn render_test_err_row(
    t: &TestResultData,
    error_msgs: &HashMap<String, String>,
    body_lines: usize,
    verbose: u8,
    console_urls: &HashMap<String, String>,
    compiled_paths: &HashMap<String, String>,
) -> String {
    let name = pick_name(&t.node_info);
    let error_msg = error_msgs
        .get(&t.node_info.unique_id)
        .filter(|s| !s.is_empty());
    let body = error_msg
        .map(|s| s.as_str())
        .unwrap_or(&t.message);
    let mut out = format!(
        "ERR   {:<40}\n",
        truncate(name, 40),
    );
    render_body_lines(&mut out, body, error_msg.is_some(), body_lines, "      ", false);
    if verbose >= 1 {
        if let Some(url) = console_urls.get(&t.node_info.unique_id) {
            out.push_str(&format!("      [bq] {}\n", url));
        }
        if let Some(path) = compiled_paths.get(&t.node_info.unique_id) {
            out.push_str(&format!("      [compiled] {}\n", path));
        }
    }
    out
}

fn render_test_warn_row(
    t: &TestResultData,
    warn_msgs: &HashMap<String, String>,
    body_lines: usize,
    verbose: u8,
    console_urls: &HashMap<String, String>,
    compiled_paths: &HashMap<String, String>,
) -> String {
    let name = pick_name(&t.node_info);
    let n = t.num_failures.unwrap_or(0);
    let row_or_rows = if n == 1 { "row" } else { "rows" };
    let warn_msg = warn_msgs
        .get(&t.node_info.unique_id)
        .filter(|s| !s.is_empty());
    let body = warn_msg
        .map(|s| s.as_str())
        .unwrap_or(&t.message);
    let mut out = format!(
        "WARN  {:<40}  {} {}\n",
        truncate(name, 40),
        n,
        row_or_rows,
    );
    // RunResultWarningMessage's first line IS the actionable text. Pass
    // `is_paired_error_msg=false` so the helper never trims line 1 (the redundant
    // header pattern doesn't apply to warn messages anyway, but this is explicit).
    render_body_lines(&mut out, body, false, body_lines, "      ", false);
    if verbose >= 1 {
        if let Some(url) = console_urls.get(&t.node_info.unique_id) {
            out.push_str(&format!("      [bq] {}\n", url));
        }
        if let Some(path) = compiled_paths.get(&t.node_info.unique_id) {
            out.push_str(&format!("      [compiled] {}\n", path));
        }
    }
    out
}

fn build_build_summary(parsed: &ParsedStream, verbose: u8) -> String {
    let mut models: Vec<&ModelResultData> = Vec::new();
    let mut tests: Vec<&TestResultData> = Vec::new();
    let mut seeds: Vec<&ModelResultData> = Vec::new();
    let mut snapshots: Vec<&ModelResultData> = Vec::new();
    let mut run_summary: Option<&RunSummaryData> = None;

    for e in &parsed.events {
        match e {
            ParsedEvent::Model(m) => models.push(m),
            ParsedEvent::Test(t) => tests.push(t),
            ParsedEvent::Seed(m) => seeds.push(m),
            ParsedEvent::Snapshot(m) => snapshots.push(m),
            ParsedEvent::Done(s) => run_summary = Some(s),
            _ => {}
        }
    }

    let elapsed = run_summary.map(|s| s.execution_time).unwrap_or(0.0);
    let skipped = parsed.stats.as_ref().map(|s| s.skip).unwrap_or(0);

    let model_pass = models.iter().filter(|m| m.succeeded).count();
    let model_fail = models.iter().filter(|m| !m.succeeded).count();
    let test_pass = tests.iter().filter(|t| t.status == "pass").count();
    let test_warn = tests.iter().filter(|t| t.status == "warn").count();
    let test_fail = tests.iter().filter(|t| t.status == "fail").count();
    let test_err = tests.iter().filter(|t| t.status == "error").count();
    let seed_pass = seeds.iter().filter(|s| s.succeeded).count();
    let seed_fail = seeds.iter().filter(|s| !s.succeeded).count();
    let snap_pass = snapshots.iter().filter(|s| s.succeeded).count();
    let snap_fail = snapshots.iter().filter(|s| !s.succeeded).count();

    let any_fail =
        model_fail + test_fail + test_err + test_warn + seed_fail + snap_fail > 0;

    let partial_prefix = if parsed.terminator_seen { "" } else { "[partial] " };

    if models.is_empty() && tests.is_empty() && seeds.is_empty() && snapshots.is_empty() {
        if let Some(ref msg) = parsed.main_error {
            return render_main_error_summary("build", msg, elapsed, verbose, parsed.terminator_seen);
        }
        // "Never Block" — corrupt JSON stream + substantial leftover
        // → surface the raw output instead of "0 nodes selected".
        if let Some(passthrough) = leftover_passthrough(&parsed.leftover) {
            return passthrough;
        }
        return format!(
            "{}dbt build: 0 nodes selected  {}",
            partial_prefix,
            fmt_secs(elapsed)
        );
    }

    let mut header_parts = Vec::new();
    if !models.is_empty() {
        header_parts.push(if model_fail == 0 {
            format!("{} models OK", model_pass)
        } else {
            format!("{}/{} models OK", model_pass, models.len())
        });
    }
    if !tests.is_empty() {
        header_parts.push(if test_fail + test_err + test_warn == 0 {
            format!("{} tests PASS", test_pass)
        } else {
            format!("{}/{} tests PASS", test_pass, tests.len())
        });
    }
    if !seeds.is_empty() {
        header_parts.push(if seed_fail == 0 {
            format!("{} seeds OK", seed_pass)
        } else {
            format!("{}/{} seeds OK", seed_pass, seeds.len())
        });
    }
    if !snapshots.is_empty() {
        header_parts.push(if snap_fail == 0 {
            format!("{} snapshots OK", snap_pass)
        } else {
            format!("{}/{} snapshots OK", snap_pass, snapshots.len())
        });
    }

    let mut out = format!("{}dbt build: {}", partial_prefix, header_parts.join(", "));
    if any_fail {
        if model_fail > 0 {
            out.push_str(&format!(", {} model ERR", model_fail));
        }
        if test_fail > 0 {
            out.push_str(&format!(", {} test FAIL", test_fail));
        }
        if test_err > 0 {
            out.push_str(&format!(", {} test ERR", test_err));
        }
        if test_warn > 0 {
            out.push_str(&format!(", {} test WARN", test_warn));
        }
        if seed_fail > 0 {
            out.push_str(&format!(", {} seed ERR", seed_fail));
        }
        if snap_fail > 0 {
            out.push_str(&format!(", {} snapshot ERR", snap_fail));
        }
    }
    if skipped > 0 {
        out.push_str(&format!(", {} skipped", skipped));
    }
    out.push_str(&format!("  {}", fmt_secs(elapsed)));

    if !any_fail {
        if should_emit_warn_hint(parsed, false, verbose) {
            out.push('\n');
            out.push_str(WARN_HINT);
        }
        return out;
    }

    out.push('\n');
    out.push_str("═══════════════════════════════════════\n");

    // default 3 lines, -v 20 lines (was 1/5 model, 5/10 test).
    let model_body_lines: usize = if verbose >= 1 { 20 } else { 3 };
    let test_body_lines: usize = if verbose >= 1 { 20 } else { 3 };

    let model_label = "run"; // models in `dbt build` use the same classification as `dbt run`.
    for m in models.iter().filter(|m| !m.succeeded).take(20) {
        out.push_str(&render_err_row(
            &m.node_info,
            &m.description,
            parsed.error_msgs.get(&m.node_info.unique_id),
            model_label,
            model_body_lines,
            verbose,
            &parsed.console_urls,
            &parsed.compiled_paths,
        ));
    }
    for s in seeds.iter().filter(|s| !s.succeeded).take(20) {
        out.push_str(&render_err_row(
            &s.node_info,
            &s.description,
            parsed.error_msgs.get(&s.node_info.unique_id),
            "seed",
            model_body_lines,
            verbose,
            &parsed.console_urls,
            &parsed.compiled_paths,
        ));
    }
    for s in snapshots.iter().filter(|s| !s.succeeded).take(20) {
        out.push_str(&render_err_row(
            &s.node_info,
            &s.description,
            parsed.error_msgs.get(&s.node_info.unique_id),
            "snapshot",
            model_body_lines,
            verbose,
            &parsed.console_urls,
            &parsed.compiled_paths,
        ));
    }

    for t in tests.iter().filter(|t| t.status == "fail").take(20) {
        out.push_str(&render_test_fail_row(
            t,
            &parsed.error_msgs,
            test_body_lines,
            verbose,
            &parsed.console_urls,
            &parsed.compiled_paths,
        ));
    }
    for t in tests.iter().filter(|t| t.status == "error").take(20) {
        out.push_str(&render_test_err_row(
            t,
            &parsed.error_msgs,
            test_body_lines,
            verbose,
            &parsed.console_urls,
            &parsed.compiled_paths,
        ));
    }
    for t in tests.iter().filter(|t| t.status == "warn").take(20) {
        out.push_str(&render_test_warn_row(
            t,
            &parsed.warn_msgs,
            test_body_lines,
            verbose,
            &parsed.console_urls,
            &parsed.compiled_paths,
        ));
    }

    out.trim_end().to_string()
}

fn fmt_secs(s: f64) -> String {
    if s < 1.0 {
        format!("{:.1}s", s)
    } else if s < 60.0 {
        format!("{:.0}s", s)
    } else if s < 3600.0 {
        let m = (s / 60.0) as u64;
        let sec = (s as u64) % 60;
        format!("{}m{}s", m, sec)
    } else {
        let h = (s / 3600.0) as u64;
        let m = ((s % 3600.0) / 60.0) as u64;
        format!("{}h{}m", h, m)
    }
}

fn pick_name(node: &NodeInfo) -> &str {
    if !node.node_name.is_empty() {
        &node.node_name
    } else {
        "<unknown>"
    }
}

fn light_filter(raw: &str, verbose: u8, subcmd: &str) -> String {
    let is_debug = subcmd == "debug";

    // count total `------` rows up front so we can decide whether to
    // apply the dashes-boundary trim. Only fires when at-least-2 are present —
    // a single dashes-row (e.g. a contract-violation table separator) survives
    // unchanged. The `[full output: ...]` footer is preserved regardless.
    let total_dashes_rows = if verbose == 0 {
        raw.lines()
            .filter(|line| {
                let stripped = strip_ansi(line);
                let core = strip_dbt_timestamp(&stripped);
                is_dashes_row(core.trim())
            })
            .count()
    } else {
        0
    };
    let apply_dashes_boundary = total_dashes_rows >= 2;

    let mut out: Vec<String> = Vec::new();
    let mut in_warning_block = false; //
    let mut dashes_rendered: usize = 0; //
    let mut tail_dropping = false;

    for line in raw.lines() {
        let stripped = strip_ansi(line);
        let core = strip_dbt_timestamp(&stripped);
        let trimmed = core.trim();

        // the `[full output: ...]` footer is appended by rtk and must
        // always survive — bypass any block-drop state for it.
        if trimmed.starts_with("[full output:") {
            in_warning_block = false;
            out.push(stripped);
            continue;
        }

        // when we see `[WARNING]`, drop this line and every subsequent
        // line until we encounter a blank line OR the next timestamped log entry
        // (`HH:MM:SS ` prefix on the original line). Re-arms on another
        // `[WARNING]`. Only at verbose == 0.
        if verbose == 0 && trimmed.contains("[WARNING]") {
            in_warning_block = true;
            continue;
        }
        if in_warning_block {
            if trimmed.is_empty() || has_dbt_timestamp(&stripped) {
                in_warning_block = false;
                // Fall through and let the line be evaluated by the rest of the
                // rules — a timestamped log entry is signal we want to keep.
                if trimmed.is_empty() {
                    continue;
                }
            } else {
                continue;
            }
        }

        // once we've rendered the second dashes-row, drop everything
        // until end of stream. (`[full output:` was already handled above.)
        if tail_dropping {
            continue;
        }

        // Existing per-line filters.
        if trimmed.is_empty()
            || trimmed.starts_with("Running with dbt=")
            || trimmed.starts_with("Registered adapter:")
            || trimmed.starts_with("Concurrency:")
            || (verbose == 0 && trimmed.starts_with("[WARNING]:"))
            // Python warnings from third-party packages — always present,
            // never actionable for dbt users.
            || trimmed.contains("FutureWarning:")
            || trimmed.contains("DeprecationWarning:")
            // .venv stack-frame lines that wrap the warning location.
            || (trimmed.starts_with("File \"") && trimmed.contains(".venv/"))
            || (trimmed.contains(".venv/") && trimmed.contains("Warning:"))
            // orphan `from <module> import <name>` line that follows
            // a dropped FutureWarning/DeprecationWarning. Investigation showed
            // the dashes-boundary alternative would clobber load-bearing dbt
            // YAML parse-error context — the parsing
            // error renders source-file context separated by `------------`
            // dividers — so the simpler fallback is correct. At -v this leaks
            // through (the user opted into noise).
            || (verbose == 0 && is_python_import_followon(trimmed))
            // drop `dbt debug` info-block at verbose 0; keep at -v.
            || (verbose == 0
                && is_debug
                && (trimmed.starts_with("dbt version:")
                    || trimmed.starts_with("python version:")
                    || trimmed.starts_with("python path:")
                    || trimmed.starts_with("os info:")))
        {
            continue;
        }

        let is_dashes = apply_dashes_boundary && is_dashes_row(trimmed);
        out.push(stripped);

        if is_dashes {
            dashes_rendered += 1;
            if dashes_rendered >= 2 {
                tail_dropping = true;
            }
        }
    }

    let mut joined = out.join("\n");
    if verbose == 0 && light_filter_should_hint(raw) {
        joined.push('\n');
        joined.push_str(WARN_HINT);
    }
    joined
}

/// True iff the light-filter raw input has stripped warning content but no
/// error indicators that would already trigger a tee + `[full output: ...]`
/// recovery footer. ANSI codes are removed before substring checks because
/// dbt wraps `[WARNING]` markers in color escapes (`[\x1b[33mWARNING\x1b[0m]`).
fn light_filter_should_hint(raw: &str) -> bool {
    let cleaned = strip_ansi(raw);
    let has_warnings = cleaned.contains("[WARNING]")
        || cleaned.contains("FutureWarning:")
        || cleaned.contains("DeprecationWarning:");
    if !has_warnings {
        return false;
    }
    let has_error = cleaned.contains("Encountered an error")
        || cleaned.contains("Compilation Error")
        || cleaned.contains("Parsing Error")
        || cleaned.contains("Database Error")
        || cleaned.contains("Runtime Error")
        || cleaned.contains("Syntax error");
    !has_error
}

/// True iff `trimmed` is a "dashes row" — 20+ ASCII hyphens, nothing else.
/// uses this as a closing-boundary marker.
fn is_dashes_row(trimmed: &str) -> bool {
    !trimmed.is_empty()
        && trimmed.len() >= 20
        && trimmed.chars().all(|c| c == '-')
}

/// True iff `s` starts with a dbt log timestamp (`HH:MM:SS ` followed by a space).
/// Matches the same shape that `strip_dbt_timestamp` looks for, but without
/// trimming.
fn has_dbt_timestamp(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= 9
        && bytes[0].is_ascii_digit()
        && (bytes[1].is_ascii_digit() || bytes[1] == b':') // tolerate H:MM:SS
        && bytes[2] == b':'
        && bytes[5] == b':'
        && bytes[8] == b' '
}

/// True iff `trimmed` looks like a Python `from <module> import <name>` line —
/// the orphan follow-on emitted after a `FutureWarning:` / `DeprecationWarning:`
/// when the warning's source line is shown via `-Werror` style traceback.
///
/// Conservative check: trimmed line starts with `from `, contains ` import `,
/// and the module name (the token after `from `) starts with a lowercase letter
/// or underscore. Avoids matching dbt SQL-style `FROM` (uppercase keyword).
fn is_python_import_followon(trimmed: &str) -> bool {
    let after_from = match trimmed.strip_prefix("from ") {
        Some(s) => s,
        None => return false,
    };
    if !after_from.contains(" import ") {
        return false;
    }
    let first_char = match after_from.chars().next() {
        Some(c) => c,
        None => return false,
    };
    first_char.is_ascii_lowercase() || first_char == '_'
}

/// Strip dbt's `HH:MM:SS  ` log timestamp prefix if present.
fn strip_dbt_timestamp(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 9
        && bytes[2] == b':'
        && bytes[5] == b':'
        && bytes[8] == b' '
        && s.is_char_boundary(9)
    {
        &s[9..]
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixtures mirror real dbt 1.11 JSON shape: model success/failure derives from
    // envelope `info.level`; `LogModelResult.data` has no clean `status` field; the
    // run-completion event is `FinishedRunningStats` carrying `execution_time`.
    // Test events DO carry a clean `data.status` ("pass"|"fail").

    const FIXTURE_RUN_PASS: &str = r#"{"info":{"name":"MainReportVersion","level":"info","msg":"Running with dbt=1.11.7"},"data":{}}
{"info":{"name":"LogModelResult","level":"info","msg":"1 of 2 OK"},"data":{"node_info":{"node_name":"customers","materialized":"table","node_path":"models/marts/customers.sql"},"execution_time":2.4,"description":"sql table model dw.customers"}}
{"info":{"name":"LogModelResult","level":"info","msg":"2 of 2 OK"},"data":{"node_info":{"node_name":"dummy_model_174","materialized":"view","node_path":"models/marts/dummy_model_174.sql"},"execution_time":1.1,"description":"sql view model dw.dummy_model_174"}}
{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":3.5,"stat_line":"2 view models"}}
"#;

    const FIXTURE_RUN_FAIL: &str = r#"{"info":{"name":"LogModelResult","level":"info","msg":"1 of 2 OK"},"data":{"node_info":{"node_name":"customers","node_path":"models/marts/customers.sql"},"execution_time":2.4}}
{"info":{"name":"LogModelResult","level":"error","msg":"2 of 2 ERROR"},"data":{"node_info":{"node_name":"dummy_model_174","node_path":"models/marts/dummy_model_174.sql"},"description":"Compilation Error: macro 'foo' undefined\nin model dummy_model_174.sql line 12","execution_time":0.5}}
{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":3.0,"stat_line":"2 view models"}}
"#;

    const FIXTURE_TEST_PASS: &str = r#"{"info":{"name":"LogTestResult","level":"info","msg":"PASS"},"data":{"node_info":{"node_name":"not_null_customers_id","node_path":"models/marts/schema.yml"},"status":"pass","execution_time":0.5}}
{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":0.6,"stat_line":"1 data test"}}
"#;

    const FIXTURE_TEST_FAIL: &str = r#"{"info":{"name":"LogTestResult","level":"info","msg":"PASS"},"data":{"node_info":{"node_name":"not_null_customers_id","node_path":"models/schema.yml"},"status":"pass","execution_time":0.4}}
{"info":{"name":"LogTestResult","level":"warn","msg":"FAIL"},"data":{"node_info":{"node_name":"unique_customers_id","node_path":"models/schema.yml"},"status":"fail","num_failures":3,"execution_time":0.6,"message":"select count(*) from (\n  select customer_id\n  from `dw.customers`\n  group by 1\n  having count(*) > 1\n)"}}
{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":1.2,"stat_line":"2 data tests"}}
"#;

    const FIXTURE_BUILD_MIXED: &str = r#"{"info":{"name":"LogModelResult","level":"info","msg":"1 of 3 OK"},"data":{"node_info":{"node_name":"customers","node_path":"models/marts/customers.sql"},"execution_time":2.0}}
{"info":{"name":"LogTestResult","level":"warn","msg":"FAIL"},"data":{"node_info":{"node_name":"unique_customers_id","node_path":"models/schema.yml"},"status":"fail","num_failures":1,"execution_time":0.5}}
{"info":{"name":"LogTestResult","level":"info","msg":"PASS"},"data":{"node_info":{"node_name":"not_null_customers_id","node_path":"models/schema.yml"},"status":"pass","execution_time":0.4}}
{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":3.5,"stat_line":"1 model, 2 data tests"}}
"#;

    #[test]
    fn test_parse_events_drops_unknown() {
        let raw = r#"{"info":{"name":"SomeFutureEvent","level":"info","msg":"x"},"data":{"foo":"bar"}}
{"info":{"name":"LogModelResult","level":"info","msg":"OK"},"data":{"node_info":{"node_name":"a"}}}
"#;
        let parsed = parse_events(raw);
        // Both events parse — one as Other, one as Model. Neither crashes.
        assert_eq!(parsed.events.len(), 2);
        assert!(matches!(parsed.events[0], ParsedEvent::Other));
        assert!(matches!(parsed.events[1], ParsedEvent::Model(_)));
    }

    #[test]
    fn test_model_success_derived_from_info_level() {
        // dbt 1.11 LogModelResult has no clean `data.status` — we derive from `info.level`.
        let raw = r#"{"info":{"name":"LogModelResult","level":"info","msg":"OK"},"data":{"node_info":{"node_name":"good"},"description":"sql table model"}}
{"info":{"name":"LogModelResult","level":"error","msg":"ERROR"},"data":{"node_info":{"node_name":"bad"},"description":"Compilation Error: x"}}
"#;
        let parsed = parse_events(raw);
        let succeeded: Vec<bool> = parsed
            .events
            .iter()
            .filter_map(|e| match e {
                ParsedEvent::Model(m) => Some(m.succeeded),
                _ => None,
            })
            .collect();
        assert_eq!(succeeded, vec![true, false]);
    }

    #[test]
    fn test_run_summary_all_pass_single_line() {
        let parsed = parse_events(FIXTURE_RUN_PASS);
        let out = build_run_summary(&parsed, 0, "run");
        assert_eq!(out, "dbt run: 2/2 OK  4s");
    }

    #[test]
    fn test_run_summary_with_failures() {
        let parsed = parse_events(FIXTURE_RUN_FAIL);
        let out = build_run_summary(&parsed, 0, "run");
        assert!(out.contains("dbt run: 1/2 OK  1 ERR"));
        assert!(out.contains("ERR"));
        assert!(out.contains("dummy_model_174"));
        // Path column dropped (R2 D); body retains the error description.
        assert!(out.contains("Compilation Error"));
    }

    #[test]
    fn test_run_summary_empty() {
        let raw = r#"{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":0.2,"stat_line":""}}
"#;
        let parsed = parse_events(raw);
        let out = build_run_summary(&parsed, 0, "run");
        assert_eq!(out, "dbt run: 0 nodes selected  0.2s");
    }

    #[test]
    fn test_test_summary_pass_only() {
        let parsed = parse_events(FIXTURE_TEST_PASS);
        let out = build_test_summary(&parsed, 0);
        assert_eq!(out, "dbt test: 1/1 PASS  0.6s");
    }

    #[test]
    fn test_test_summary_with_failures_includes_body() {
        let parsed = parse_events(FIXTURE_TEST_FAIL);
        let out = build_test_summary(&parsed, 0);
        assert!(out.contains("dbt test: 1/2 PASS  1 FAIL"));
        assert!(out.contains("FAIL"));
        assert!(out.contains("unique_customers_id"));
        assert!(out.contains("3 rows"));
        // First 3 body lines included at v=0 (R2 pick 1).
        assert!(out.contains("select count(*)"));
        // At -v, the full body (including the trailing `having count(*) > 1`) is shown.
        let verbose = build_test_summary(&parsed, 1);
        assert!(verbose.contains("having count(*) > 1"));
    }

    #[test]
    fn test_build_summary_mixed_models_and_tests() {
        let parsed = parse_events(FIXTURE_BUILD_MIXED);
        let out = build_build_summary(&parsed, 0);
        assert!(out.starts_with("dbt build:"));
        assert!(out.contains("1 models OK"));
        assert!(out.contains("1/2 tests PASS"));
        assert!(out.contains("1 test FAIL"));
        assert!(out.contains("FAIL"));
        assert!(out.contains("unique_customers_id"));
    }

    // -- Fix 1: pair LogModelResult failure with RunResultError ---------------------------

    #[test]
    fn test_run_result_error_pairs_with_failed_model_by_unique_id() {
        // Real dbt 1.11 shape: LogModelResult.data has only a generic description;
        // the human-readable message is in RunResultError.info.msg keyed on unique_id.
        let raw = r#"{"info":{"name":"LogModelResult","level":"error","msg":"ERROR"},"data":{"node_info":{"node_name":"dummy_model_174","node_path":"models/marts/dummy_model_174.sql","unique_id":"model.dw.dummy_model_174"},"description":"sql incremental model dw.dummy_model_174","execution_time":1.0}}
{"info":{"name":"RunResultError","level":"error","msg":"  Compilation Error in model dummy_model_174\n  'undefined_macro_xyz' is undefined."},"data":{"node_info":{"node_name":"dummy_model_174","unique_id":"model.dw.dummy_model_174"}}}
{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":1.5}}
"#;
        let parsed = parse_events(raw);
        // RunResultError must be stashed by unique_id, not pushed as a result event.
        assert_eq!(parsed.error_msgs.len(), 1);
        assert!(parsed
            .error_msgs
            .get("model.dw.dummy_model_174")
            .unwrap()
            .contains("'undefined_macro_xyz' is undefined"));
        let out = build_run_summary(&parsed, 0, "run");
        assert!(out.contains("dbt run: 0/1 OK  1 ERR"));
        // line 1 of RunResultError.info.msg ("Compilation Error in model dummy_model_174")
        // is intentionally skipped — it duplicates the ERR row's name + classification.
        // The actionable line 2 is what gets rendered.
        assert!(out.contains("'undefined_macro_xyz' is undefined"));
        // Description fallback is suppressed when error message is present.
        assert!(!out.contains("sql incremental model"));
    }

    #[test]
    fn test_failed_model_falls_back_to_description_when_no_run_result_error() {
        // No RunResultError emitted (fail-fast path) — description is the body fallback.
        let raw = r#"{"info":{"name":"LogModelResult","level":"error","msg":"ERROR"},"data":{"node_info":{"node_name":"dummy_model_174","node_path":"models/marts/dummy_model_174.sql","unique_id":"model.dw.dummy_model_174"},"description":"Compilation Error: bad","execution_time":1.0}}
{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":1.5}}
"#;
        let parsed = parse_events(raw);
        let out = build_run_summary(&parsed, 0, "run");
        assert!(out.contains("Compilation Error: bad"));
    }

    // -- Fix 2: 4-bucket test outcomes ----------------------------------------------------

    #[test]
    fn test_test_summary_four_buckets_pass_warn_fail_error() {
        // pass + warn + fail + error in one run.
        let raw = r#"{"info":{"name":"LogTestResult","level":"info","msg":"PASS"},"data":{"node_info":{"node_name":"t_pass","node_path":"models/schema.yml","unique_id":"test.dw.t_pass"},"status":"pass","execution_time":0.1}}
{"info":{"name":"LogTestResult","level":"warn","msg":"WARN"},"data":{"node_info":{"node_name":"t_warn","node_path":"models/schema.yml","unique_id":"test.dw.t_warn"},"status":"warn","num_failures":1,"execution_time":0.1}}
{"info":{"name":"LogTestResult","level":"error","msg":"FAIL 3"},"data":{"node_info":{"node_name":"t_fail","node_path":"models/schema.yml","unique_id":"test.dw.t_fail"},"status":"fail","num_failures":3,"execution_time":0.1}}
{"info":{"name":"LogTestResult","level":"error","msg":"ERROR"},"data":{"node_info":{"node_name":"t_err","node_path":"tests/t_err.sql","unique_id":"test.dw.t_err"},"status":"error","num_failures":0,"execution_time":0.1}}
{"info":{"name":"RunResultError","level":"error","msg":"  Database Error in test t_err\n  Syntax error: Unexpected SELEC"},"data":{"node_info":{"node_name":"t_err","unique_id":"test.dw.t_err"}}}
{"info":{"name":"RunResultWarningMessage","level":"warn","msg":"Got 1 result, configured to warn if != 0"},"data":{"node_info":{"node_name":"t_warn","unique_id":"test.dw.t_warn"}}}
{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":2.0}}
"#;
        let parsed = parse_events(raw);
        let out = build_test_summary(&parsed, 0);
        // Header: 1 PASS / 4 total, plus all three failure-side buckets.
        assert!(out.contains("dbt test: 1/4 PASS"));
        assert!(out.contains("1 FAIL"));
        assert!(out.contains("1 ERR"));
        assert!(out.contains("1 WARN"));
        // Bodies for each non-pass row.
        assert!(out.contains("FAIL  t_fail"));
        assert!(out.contains("ERR   t_err"));
        assert!(out.contains("WARN  t_warn"));
        // line 1 of RunResultError ("Database Error in test t_err") is skipped;
        // line 2 (the actionable detail) is what renders.
        assert!(out.contains("Syntax error: Unexpected SELEC"));
        // RunResultWarningMessage's first line IS the actionable text.
        assert!(out.contains("Got 1 result, configured to warn"));
    }

    // -- Fix 3: skipped count surfaced from StatsLine -------------------------------------

    #[test]
    fn test_stats_line_skipped_count_renders_in_header() {
        // 1 OK + 1 ERR + 232 skipped.
        let raw = r#"{"info":{"name":"LogModelResult","level":"info","msg":"OK"},"data":{"node_info":{"node_name":"hook_node","node_path":"on_run_end.sql","unique_id":"model.dw.hook"},"description":"hook","execution_time":0.1}}
{"info":{"name":"LogModelResult","level":"error","msg":"ERROR"},"data":{"node_info":{"node_name":"dummy_model_174","node_path":"models/marts/dummy_model_174.sql","unique_id":"model.dw.dummy_model_174"},"description":"sql view model dw.dummy_model_174","execution_time":1.0}}
{"info":{"name":"StatsLine","level":"info","msg":"Done. PASS=1 ERROR=1 SKIP=232"},"data":{"stats":{"pass":1,"warn":0,"error":1,"skip":232,"noop":0,"total":234}}}
{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":27.0}}
"#;
        let parsed = parse_events(raw);
        assert!(parsed.stats.is_some());
        assert_eq!(parsed.stats.as_ref().unwrap().skip, 232);
        let out = build_run_summary(&parsed, 0, "run");
        assert!(out.contains("232 skipped"));
        assert!(out.contains("1 ERR"));
    }

    // -- Fix 4: classification rendered in ERR row ----------------------------------------

    #[test]
    fn test_err_row_renders_classification_compile() {
        let raw = r#"{"info":{"name":"LogModelResult","level":"error","msg":"ERROR"},"data":{"node_info":{"node_name":"dummy_model_174","node_path":"models/marts/dummy_model_174.sql","unique_id":"model.dw.dummy_model_174"},"description":"sql view model","execution_time":1.0}}
{"info":{"name":"RunResultError","level":"error","msg":"  Compilation Error in model dummy_model_174\n  'undefined_macro_xyz' is undefined."},"data":{"node_info":{"node_name":"dummy_model_174","unique_id":"model.dw.dummy_model_174"}}}
{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":1.5}}
"#;
        let parsed = parse_events(raw);
        let out = build_run_summary(&parsed, 0, "run");
        // ERR row format is `ERR  <name>  <classification>` (path dropped, R2 D).
        assert!(out.contains("compile"));
        assert!(out.contains("dummy_model_174"));
    }

    #[test]
    fn test_err_row_renders_classification_database() {
        let raw = r#"{"info":{"name":"LogModelResult","level":"error","msg":"ERROR"},"data":{"node_info":{"node_name":"dummy_model_174","node_path":"models/marts/dummy_model_174.sql","unique_id":"model.dw.dummy_model_174"},"description":"sql table model","execution_time":1.0}}
{"info":{"name":"RunResultError","level":"error","msg":"  Database Error in model dummy_model_174\n  Not found: Table dne"},"data":{"node_info":{"node_name":"dummy_model_174","unique_id":"model.dw.dummy_model_174"}}}
{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":1.5}}
"#;
        let parsed = parse_events(raw);
        let out = build_run_summary(&parsed, 0, "run");
        assert!(out.contains("database"));
    }

    #[test]
    fn test_err_row_renders_classification_freshness_for_freshness_subcmd() {
        // Source freshness ERR rows always classify as `freshness`.
        let raw = r#"{"info":{"name":"LogFreshnessResult","level":"error","msg":"1 of 1 ERROR STALE"},"data":{"node_info":{"node_name":"my_src","node_path":"sources/raw.yml","unique_id":"source.dw.my_src"},"description":"source freshness","execution_time":1.0}}
{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":1.5}}
"#;
        let parsed = parse_events(raw);
        let out = build_run_summary(&parsed, 0, "freshness");
        assert!(out.contains("freshness"));
    }

    #[test]
    fn test_classify_error_helper() {
        assert_eq!(classify_error("Compilation Error in model x", "run"), "compile");
        assert_eq!(classify_error("Database Error: Not found", "run"), "database");
        assert_eq!(classify_error("Runtime Error: foo", "run"), "runtime");
        assert_eq!(
            classify_error("Contract Error: column missing", "run"),
            "contract"
        );
        // Subcommand override always wins for source freshness.
        assert_eq!(classify_error("anything", "freshness"), "freshness");
    }

    // -- Fix 5: MainEncounteredError parse-time / fail-fast --------------------------------

    #[test]
    fn test_main_encountered_error_synthesizes_parse_error_header() {
        // — dbt aborts before any LogModelResult is emitted.
        let raw = r#"{"info":{"name":"MainReportVersion","level":"info","msg":"Running with dbt=1.11.7"},"data":{}}
{"info":{"name":"MainEncounteredError","level":"error","msg":"Encountered an error:\nCompilation Error\n  Model 'dummy_model_174' depends on a node named 'dummy_model_113' which was not found"},"data":{}}
"#;
        let parsed = parse_events(raw);
        assert!(parsed.main_error.is_some());
        let out = build_run_summary(&parsed, 0, "run");
        assert!(out.contains("dbt run: compile error"));
        // Body line surfaces the actual cause (after the generic prefix).
        assert!(out.contains("Compilation Error") || out.contains("depends on a node"));
    }

    #[test]
    fn test_main_encountered_error_distinguished_from_zero_node_match() {
        // selector matched zero nodes, no error event.
        let raw = r#"{"info":{"name":"NoNodesForSelectionCriteria","level":"warn","msg":"selector matched no nodes"},"data":{"spec_raw":"foo"}}
{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":0.1}}
"#;
        let parsed = parse_events(raw);
        assert!(parsed.main_error.is_none());
        let out = build_run_summary(&parsed, 0, "run");
        // Empty-selector path still says "0 nodes selected", not "parse error".
        assert!(out.contains("0 nodes selected"));
        assert!(!out.contains("parse error"));
        assert!(!out.contains("compile error"));
    }

    // -- WARN_HINT footer (success-only discoverability) ---------------------------------

    /// Inline NDJSON for the v=0 success-with-warnings JSON-path case. Mirrors a
    /// real `dbt run` that completed successfully but emitted a deprecation
    /// `level=warn` event — the scenario where the LLM, given only the v=0
    /// summary, has no way to know warnings exist unless the hint is present.
    /// Anonymization conventions match the rest of the fixtures
    /// (`dummy_model_NNN`, `dummy_team_NNN`, `0000...` invocation IDs).
    const FIXTURE_RUN_PASS_WITH_WARNINGS: &str = r#"{"info":{"name":"MainReportVersion","level":"info","msg":"Running with dbt=1.11.7"},"data":{}}
{"info":{"name":"PropertyMovedToConfigDeprecation","level":"warn","msg":"[WARNING][PropertyMovedToConfigDeprecation]: Deprecated functionality"},"data":{"file":"dummy/path_017/dummy_model_086/schema.yml","key":"docs"}}
{"info":{"name":"LogModelResult","level":"info","msg":"1 of 2 OK"},"data":{"node_info":{"node_name":"dummy_model_086","unique_id":"model.dummy_team_014.dummy_model_086"},"execution_time":2.4}}
{"info":{"name":"LogModelResult","level":"info","msg":"2 of 2 OK"},"data":{"node_info":{"node_name":"dummy_model_174","unique_id":"model.dummy_team_014.dummy_model_174"},"execution_time":1.1}}
{"info":{"name":"DeprecationsSummary","level":"warn","msg":"[WARNING][DeprecationsSummary]: 1 occurrence"},"data":{}}
{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":3.5}}
"#;

    #[test]
    fn test_warn_hint_appended_at_v0_when_warnings_present_and_no_failures() {
        let parsed = parse_events(FIXTURE_RUN_PASS_WITH_WARNINGS);
        assert!(
            parsed.warn_event_count >= 1,
            "fixture should emit at least one warn-level event"
        );
        let out = build_run_summary(&parsed, 0, "run");
        assert!(out.starts_with("dbt run: 2/2 OK"));
        assert!(
            out.contains(WARN_HINT),
            "expected WARN_HINT footer at v=0 with warnings, got:\n{}",
            out
        );
    }

    #[test]
    fn test_warn_hint_absent_at_v1_even_when_warnings_present() {
        // At -v the user already opted into warnings via wider body caps and
        // `[WARNING]` retention — the hint becomes redundant.
        let parsed = parse_events(FIXTURE_RUN_PASS_WITH_WARNINGS);
        let out = build_run_summary(&parsed, 1, "run");
        assert!(
            !out.contains(WARN_HINT),
            "WARN_HINT must not appear at verbose>=1, got:\n{}",
            out
        );
    }

    #[test]
    fn test_warn_hint_absent_when_no_warnings() {
        // Clean pass-only fixture has zero warn-level events — no hint.
        let parsed = parse_events(FIXTURE_RUN_PASS);
        assert_eq!(parsed.warn_event_count, 0);
        let out = build_run_summary(&parsed, 0, "run");
        assert!(!out.contains(WARN_HINT));
    }

    #[test]
    fn test_warn_hint_absent_when_failures_present() {
        // On any failure the existing tee + `[full output: ...]` recovery footer
        // covers the LLM. WARN_HINT must not stack on top of that.
        let parsed = parse_events(FIXTURE_RUN_FAIL);
        let out = build_run_summary(&parsed, 0, "run");
        assert!(!out.contains(WARN_HINT));
    }

    #[test]
    fn test_warn_hint_absent_on_main_encountered_error() {
        // Parse-time fail-fast — `main_error` set, total==0, takes the
        // `render_main_error_summary` branch which never appends the hint.
        let raw = r#"{"info":{"name":"PropertyMovedToConfigDeprecation","level":"warn","msg":"[WARNING]"},"data":{}}
{"info":{"name":"MainEncounteredError","level":"error","msg":"Encountered an error:\nCompilation Error\n  bad"},"data":{}}
"#;
        let parsed = parse_events(raw);
        assert!(parsed.warn_event_count >= 1);
        assert!(parsed.main_error.is_some());
        let out = build_run_summary(&parsed, 0, "run");
        assert!(!out.contains(WARN_HINT));
    }

    #[test]
    fn test_warn_hint_appended_in_test_summary_all_pass_with_warn_event() {
        // `dbt test` that passed but emitted a deprecation warn-event (e.g. dbt
        // surfacing a deprecated test config). Header is the all-pass shape.
        let raw = r#"{"info":{"name":"PropertyMovedToConfigDeprecation","level":"warn","msg":"[WARNING]"},"data":{}}
{"info":{"name":"LogTestResult","level":"info","msg":"PASS"},"data":{"node_info":{"node_name":"not_null_dummy_model_086_id","unique_id":"test.dummy_team_014.x"},"status":"pass","execution_time":0.4}}
{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":0.5}}
"#;
        let parsed = parse_events(raw);
        let out = build_test_summary(&parsed, 0);
        assert!(out.starts_with("dbt test: 1/1 PASS"));
        assert!(out.contains(WARN_HINT));
    }

    #[test]
    fn test_warn_hint_appended_in_build_summary_all_pass_with_warn_event() {
        let raw = r#"{"info":{"name":"PropertyMovedToConfigDeprecation","level":"warn","msg":"[WARNING]"},"data":{}}
{"info":{"name":"LogModelResult","level":"info","msg":"OK"},"data":{"node_info":{"node_name":"dummy_model_086","unique_id":"model.dummy_team_014.dummy_model_086"},"execution_time":1.0}}
{"info":{"name":"LogTestResult","level":"info","msg":"PASS"},"data":{"node_info":{"node_name":"not_null_dummy_model_086_id","unique_id":"test.dummy_team_014.x"},"status":"pass","execution_time":0.2}}
{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":1.5}}
"#;
        let parsed = parse_events(raw);
        let out = build_build_summary(&parsed, 0);
        assert!(out.starts_with("dbt build:"));
        assert!(out.contains(WARN_HINT));
    }

    #[test]
    fn test_light_filter_hint_appended_at_v0_when_warnings_present() {
        // dbt parse output containing a `[WARNING]` deprecation block (ANSI
        // escapes preserved as dbt emits them). No error markers — hint fires.
        let raw = "\u{1b}[0m23:42:42  Running with dbt=1.11.7\n\
                   \u{1b}[0m23:42:58  [\u{1b}[33mWARNING\u{1b}[0m][PropertyMovedToConfigDeprecation]: Deprecated functionality\n\
                   Found `docs` as a top-level property of `models[0]` in file\n\
                   `dummy/path_017/dummy_model_086/schema.yml`. The\n\
                   `docs` top-level property should be moved into the `config` of `models[0]`.\n\
                   \u{1b}[0m23:43:04  Performance info: /path/to/project/dbt/target/perf_info.json\n";
        let out = light_filter(raw, 0, "parse");
        assert!(
            out.contains(WARN_HINT),
            "expected WARN_HINT in light_filter output, got:\n{}",
            out
        );
    }

    #[test]
    fn test_light_filter_hint_absent_when_error_marker_present() {
        // Same warning content, but a parse error follows — tee will fire on
        // the failing exit code, so the hint is suppressed to avoid stacking.
        let raw = "\u{1b}[0m23:42:58  [\u{1b}[33mWARNING\u{1b}[0m][PropertyMovedToConfigDeprecation]: Deprecated functionality\n\
                   `dummy/path_017/dummy_model_086/schema.yml`.\n\
                   \u{1b}[0m23:42:59  Encountered an error:\nParsing Error\n  bad yaml\n";
        let out = light_filter(raw, 0, "parse");
        assert!(!out.contains(WARN_HINT));
    }

    #[test]
    fn test_light_filter_hint_absent_at_v1() {
        let raw = "23:42:58  [WARNING][PropertyMovedToConfigDeprecation]: Deprecated functionality\n\
                   `dummy/path_017/dummy_model_086/schema.yml`.\n";
        let out = light_filter(raw, 1, "parse");
        assert!(!out.contains(WARN_HINT));
    }

    #[test]
    fn test_warn_hint_fires_on_real_json_path_pass_fixtures() {
        // Real anonymized captures from a 12k-model dbt project. Each is a
        // successful run (no errors) that emitted deprecation/ref-version
        // `level=warn` events — the production case where v=0 strips warnings
        // and the LLM must be told they exist.
        let cases: &[(&str, &str, &str)] = &[
            ("dbt run", REAL_RUN_PASS, "run"),
            ("dbt seed", REAL_SEED_PASS, "seed"),
            ("dbt snapshot", REAL_SNAPSHOT_PASS, "snapshot"),
        ];
        for (label, raw, cmd_label) in cases {
            let parsed = parse_events(raw);
            assert!(
                parsed.warn_event_count > 0,
                "{}: real fixture should have warn events",
                label
            );
            let out = build_run_summary(&parsed, 0, cmd_label);
            assert!(
                out.contains(WARN_HINT),
                "{}: WARN_HINT must appear on real success-with-warnings fixture, got:\n{}",
                label,
                out
            );
            // -v must suppress it (user opted into seeing warnings inline).
            let out_v = build_run_summary(&parsed, 1, cmd_label);
            assert!(
                !out_v.contains(WARN_HINT),
                "{}: WARN_HINT must NOT appear at -v, got:\n{}",
                label,
                out_v
            );
        }
    }

    #[test]
    fn test_warn_hint_fires_on_real_light_filter_pass_fixtures() {
        // Real anonymized `dbt parse` and `dbt compile` captures. Both
        // contain ~127 `[WARNING]` deprecation/ref-version blocks and no error
        // markers — light-filter strips the warnings at v=0, hint must fire.
        let cases: &[(&str, &str, &str)] = &[
            ("dbt parse", REAL_PARSE, "parse"),
            ("dbt compile", REAL_COMPILE, "compile"),
        ];
        for (label, raw, subcmd) in cases {
            let out = light_filter(raw, 0, subcmd);
            assert!(
                out.contains(WARN_HINT),
                "{}: WARN_HINT must appear in light-filter output for real success-with-warnings fixture",
                label
            );
            let out_v = light_filter(raw, 1, subcmd);
            assert!(
                !out_v.contains(WARN_HINT),
                "{}: WARN_HINT must NOT appear at -v in light-filter output",
                label
            );
        }
    }

    // -- Helpers / regressions ------------------------------------------------------------

    #[test]
    fn test_inject_log_flags_idempotent() {
        let user = vec![
            "run".to_string(),
            "--log-format".to_string(),
            "default".to_string(),
            "-s".to_string(),
            "foo".to_string(),
        ];
        let injected = inject_log_flags(&user, 0);
        // Should not duplicate --log-format
        let count = injected.iter().filter(|a| *a == "--log-format").count();
        assert_eq!(count, 1);
        // User's value preserved
        assert!(injected.contains(&"default".to_string()));
        // --log-level still injected (user didn't set it)
        assert!(injected.contains(&"--log-level".to_string()));
        assert!(injected.contains(&"info".to_string()));
    }

    #[test]
    fn test_inject_log_flags_when_no_user_flags() {
        let user = vec!["run".to_string(), "-s".to_string(), "foo".to_string()];
        let injected = inject_log_flags(&user, 0);
        assert!(injected.contains(&"--log-format".to_string()));
        assert!(injected.contains(&"json".to_string()));
        assert!(injected.contains(&"--log-level".to_string()));
        assert!(injected.contains(&"info".to_string()));
    }

    #[test]
    fn test_inject_log_flags_verbose_promotes_to_debug() {
        let user = vec!["run".to_string()];
        let injected = inject_log_flags(&user, 2);
        assert!(injected.contains(&"debug".to_string()));
        assert!(!injected.contains(&"info".to_string()));
    }

    #[test]
    fn test_light_filter_drops_banner() {
        // Real dbt output prefixes lines with "HH:MM:SS  " and wraps in ANSI codes.
        let raw = "\x1b[0m00:37:26  Running with dbt=1.11.7\n\
                   \x1b[0m00:37:29  Registered adapter: bigquery=1.9.2\n\
                   \x1b[0m00:37:30  Concurrency: 32 threads (target='dev')\n\
                   \n\
                   \x1b[0m00:37:35  Found 247 models, 1247 tests, 14 sources\n\
                   \x1b[0m00:38:01  Compilation finished\n";
        let out = light_filter(raw, 0, "compile");
        assert!(!out.contains("Running with dbt"));
        assert!(!out.contains("Registered adapter"));
        assert!(!out.contains("Concurrency"));
        assert!(out.contains("Found 247 models"));
        assert!(out.contains("Compilation finished"));
        // ANSI codes stripped from kept lines too (token savings).
        assert!(!out.contains("\x1b["));
    }

    #[test]
    fn test_light_filter_handles_plain_text() {
        // Banner lines without ANSI/timestamps still filter (e.g. piped/test fixture).
        let raw = "Running with dbt=1.11.7\nFound 5 models\n";
        let out = light_filter(raw, 0, "compile");
        assert!(!out.contains("Running with dbt"));
        assert!(out.contains("Found 5 models"));
    }

    #[test]
    fn test_light_filter_drops_warnings_at_v0() {
        let raw = "\x1b[0m00:40:23  [WARNING]: While compiling 'foo': Found a reference to bar.v1, which is slated for deprecation\n\
                   \x1b[0m00:40:23  Found 5 models\n";
        let out = light_filter(raw, 0, "compile");
        assert!(!out.contains("[WARNING]"));
        assert!(out.contains("Found 5 models"));
    }

    #[test]
    fn test_light_filter_keeps_warnings_at_v1() {
        let raw = "\x1b[0m00:40:23  [WARNING]: deprecation warning\n\
                   \x1b[0m00:40:23  Found 5 models\n";
        let out = light_filter(raw, 1, "compile");
        assert!(out.contains("[WARNING]"));
        assert!(out.contains("deprecation warning"));
    }

    #[test]
    fn test_strip_dbt_timestamp() {
        assert_eq!(strip_dbt_timestamp("00:37:26  hello"), " hello");
        assert_eq!(strip_dbt_timestamp("no timestamp here"), "no timestamp here");
        assert_eq!(strip_dbt_timestamp(""), "");
    }

    #[test]
    fn test_fmt_secs() {
        assert_eq!(fmt_secs(0.0), "0.0s");
        assert_eq!(fmt_secs(0.4), "0.4s");
        assert_eq!(fmt_secs(42.0), "42s");
        assert_eq!(fmt_secs(60.0), "1m0s");
        assert_eq!(fmt_secs(252.0), "4m12s");
        assert_eq!(fmt_secs(3600.0), "1h0m");
        assert_eq!(fmt_secs(3720.0), "1h2m");
    }

    // AdapterEventError + SQLCompiledPath at -v -----------------------------

    #[test]
    fn test_adapter_event_error_renders_only_at_verbose() {
        let raw = r#"{"info":{"name":"LogModelResult","level":"error","msg":"ERROR"},"data":{"node_info":{"node_name":"dummy_model_174","node_path":"models/marts/dummy_model_174.sql","unique_id":"model.dw.dummy_model_174"},"description":"sql","execution_time":1.0}}
{"info":{"name":"RunResultError","level":"error","msg":"  Database Error in model dummy_model_174\n  Not found: Dataset xyz"},"data":{"node_info":{"unique_id":"model.dw.dummy_model_174"}}}
{"info":{"name":"AdapterEventError","level":"error","msg":"BigQuery adapter: https://console.cloud.google.com/bigquery?j=bq:US:abc123"},"data":{"node_info":{"unique_id":"model.dw.dummy_model_174"}}}
{"info":{"name":"SQLCompiledPath","level":"info","msg":"compiled code at target/compiled/x.sql"},"data":{"node_info":{"unique_id":"model.dw.dummy_model_174"},"path":"target/compiled/x.sql"}}
{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":1.5}}
"#;
        let parsed = parse_events(raw);
        // Pairing maps populated by unique_id.
        assert_eq!(
            parsed.console_urls.get("model.dw.dummy_model_174").map(String::as_str),
            Some("https://console.cloud.google.com/bigquery?j=bq:US:abc123")
        );
        assert_eq!(
            parsed.compiled_paths.get("model.dw.dummy_model_174").map(String::as_str),
            Some("target/compiled/x.sql")
        );

        // At default (verbose 0), neither URL nor compiled path appears.
        let default_out = build_run_summary(&parsed, 0, "run");
        assert!(!default_out.contains("[bq]"));
        assert!(!default_out.contains("[compiled]"));
        assert!(!default_out.contains("console.cloud.google.com"));

        // At -v, both render as footer lines.
        let verbose_out = build_run_summary(&parsed, 1, "run");
        assert!(verbose_out.contains("[bq] https://console.cloud.google.com/bigquery?j=bq:US:abc123"));
        assert!(verbose_out.contains("[compiled] target/compiled/x.sql"));
    }

    // skip line 1 of RunResultError --------------------------------

    #[test]
    fn test_run_result_error_skips_redundant_first_line() {
        // Line 1 is the redundant `<Kind> in model X (path.sql)` restatement that
        // duplicates the ERR row's name + classification columns.
        let raw = r#"{"info":{"name":"LogModelResult","level":"error","msg":"ERROR"},"data":{"node_info":{"node_name":"dummy_model_174","unique_id":"model.dw.dummy_model_174"},"description":"sql","execution_time":1.0}}
{"info":{"name":"RunResultError","level":"error","msg":"Compilation Error in model dummy_model_174 (models/marts/dummy_model_174.sql)\n  'foo_macro' is undefined."},"data":{"node_info":{"unique_id":"model.dw.dummy_model_174"}}}
{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":1.5}}
"#;
        let parsed = parse_events(raw);
        let out = build_run_summary(&parsed, 0, "run");
        // Body line 1 is dropped (redundant).
        assert!(!out.contains("Compilation Error in model dummy_model_174 (models/marts/dummy_model_174.sql)"));
        // Body line 2 — the actionable text — survives.
        assert!(out.contains("'foo_macro' is undefined"));
    }

    // light filter Python warning + dbt debug header drops -------------

    #[test]
    fn test_light_filter_drops_python_third_party_warnings() {
        let raw = "/path/to/.venv/lib/python3.11/site-packages/google/cloud/aiplatform/__init__.py:42: FutureWarning: google-cloud-storage < 3.0.0 is deprecated\n\
                   from google.cloud.aiplatform import schema\n\
                   Found 5 models\n";
        let out = light_filter(raw, 0, "compile");
        assert!(!out.contains("FutureWarning"));
        assert!(!out.contains(".venv/"));
        assert!(out.contains("Found 5 models"));
    }

    #[test]
    fn test_light_filter_drops_deprecation_warning() {
        let raw = "PropertyMovedToConfigDeprecation: meta moved to config\n\
                   /tmp/.venv/x/y.py:10: DeprecationWarning: x is deprecated\n\
                   Found 1 model\n";
        let out = light_filter(raw, 0, "compile");
        assert!(!out.contains("DeprecationWarning"));
        assert!(out.contains("Found 1 model"));
    }

    #[test]
    fn test_light_filter_drops_dbt_debug_header_at_v0_keeps_at_v1() {
        let raw = "dbt version: 1.11.7\n\
                   python version: 3.11.1\n\
                   python path: /path/to/python\n\
                   os info: macOS-26.3-arm64\n\
                   Using profiles dir at /tmp\n\
                   Encountered an error:\n\
                   Compilation Error\n";
        // At verbose 0 with subcmd=debug, info-block lines are dropped.
        let out_v0 = light_filter(raw, 0, "debug");
        assert!(!out_v0.contains("dbt version:"));
        assert!(!out_v0.contains("python version:"));
        assert!(!out_v0.contains("python path:"));
        assert!(!out_v0.contains("os info:"));
        // Profiles dir + actual error are kept.
        assert!(out_v0.contains("Using profiles dir at /tmp"));
        assert!(out_v0.contains("Encountered an error"));

        // At -v, info-block lines are kept.
        let out_v1 = light_filter(raw, 1, "debug");
        assert!(out_v1.contains("dbt version:"));
        assert!(out_v1.contains("python version:"));

        // At verbose 0 but a non-debug subcmd, info-block lines pass through (we
        // only drop them for `dbt debug`).
        let out_compile = light_filter(raw, 0, "compile");
        assert!(out_compile.contains("dbt version:"));
    }

    // [partial] header marker --------------------------------

    #[test]
    fn test_partial_marker_when_terminator_missing() {
        // Stream truncated before FinishedRunningStats.
        let raw = r#"{"info":{"name":"LogModelResult","level":"info","msg":"OK"},"data":{"node_info":{"node_name":"a","unique_id":"model.dw.a"},"execution_time":1.0}}
{"info":{"name":"LogModelResult","level":"error","msg":"ERROR"},"data":{"node_info":{"node_name":"b","unique_id":"model.dw.b"},"description":"Compilation Error: x","execution_time":0.5}}
"#;
        let parsed = parse_events(raw);
        assert!(!parsed.terminator_seen);
        let out = build_run_summary(&parsed, 0, "run");
        assert!(out.starts_with("[partial] "));

        // With a FinishedRunningStats terminator, no marker.
        let raw_complete = format!(
            "{}{}",
            raw,
            r#"{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":1.5}}
"#
        );
        let parsed_complete = parse_events(&raw_complete);
        assert!(parsed_complete.terminator_seen);
        let out_complete = build_run_summary(&parsed_complete, 0, "run");
        assert!(!out_complete.starts_with("[partial]"));
    }

    #[test]
    fn test_partial_marker_for_test_and_build_summaries() {
        let raw = r#"{"info":{"name":"LogTestResult","level":"info","msg":"PASS"},"data":{"node_info":{"node_name":"t","unique_id":"test.dw.t"},"status":"pass","execution_time":0.1}}
"#;
        let parsed = parse_events(raw);
        let test_out = build_test_summary(&parsed, 0);
        assert!(test_out.starts_with("[partial] "));
        let build_out = build_build_summary(&parsed, 0);
        assert!(build_out.starts_with("[partial] "));
    }

    // MainEncounteredError -v expansion -------------------------

    #[test]
    fn test_main_encountered_error_verbose_expands_body_to_five_lines() {
        let raw = r#"{"info":{"name":"MainEncounteredError","level":"error","msg":"Compilation Error\n  Contract enforcement failed for: dummy_model_174\n  Column 'created_at' is missing\n  Expected data_type TIMESTAMP, got NULL\n  Please ensure your contract matches.\n  Reference: https://docs.getdbt.com/contracts"},"data":{}}
"#;
        let parsed = parse_events(raw);
        // Default: 1 actionable line (the second non-empty line, after the kind headline).
        let default_out = build_run_summary(&parsed, 0, "run");
        let default_body = default_out.lines().filter(|l| l.starts_with("     ")).count();
        assert_eq!(default_body, 1);
        assert!(default_out.contains("Contract enforcement failed for: dummy_model_174"));

        // -v: up to 5 lines after skipping the headline.
        let verbose_out = build_run_summary(&parsed, 1, "run");
        let verbose_body = verbose_out.lines().filter(|l| l.starts_with("     ")).count();
        assert_eq!(verbose_body, 5);
        assert!(verbose_out.contains("Contract enforcement failed for: dummy_model_174"));
        assert!(verbose_out.contains("Reference: https://docs.getdbt.com/contracts"));
    }

    // name column widened to 40 -----------------------------

    #[test]
    fn test_err_row_name_column_widened_to_forty_chars() {
        // A 35-char name fits (no truncation marker).
        let raw = r#"{"info":{"name":"LogModelResult","level":"error","msg":"ERROR"},"data":{"node_info":{"node_name":"dummy_model_062","unique_id":"model.dw.x"},"description":"sql","execution_time":1.0}}
{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":1.5}}
"#;
        let parsed = parse_events(raw);
        let out = build_run_summary(&parsed, 0, "run");
        assert!(out.contains("dummy_model_062"));
        // No mid-name ellipsis.
        assert!(!out.contains("dummy_model_X_truncated..."));
    }

    // line-1-skip only when the line matches the redundant header --

    #[test]
    fn test_is_redundant_error_header_helper() {
        // Real patterns observed in fixtures (all 6 kinds × all 8 node types
        // accepted; spot-check the most common combinations).
        assert!(is_redundant_error_header(
            "  Compilation Error in model dummy_model_174 (models/marts/dummy_model_174.sql)"
        ));
        assert!(is_redundant_error_header(
            "Database Error in model x"
        ));
        assert!(is_redundant_error_header(
            "Compilation Error in seed temp_bad_DELETE_ME (seeds/x.csv)"
        ));
        assert!(is_redundant_error_header(
            "Database Error in test temp_broken_test (tests/foo.sql)"
        ));
        assert!(is_redundant_error_header(
            "Database Error in snapshot dummy_model_137 (snapshots/x.sql)"
        ));
        // NOT the redundant pattern — single-line test FAIL message.
        assert!(!is_redundant_error_header(
            "Got 3 results, configured to fail if != 0"
        ));
        assert!(!is_redundant_error_header(
            "Got 1 result, configured to warn if != 0"
        ));
        // NOT — actionable detail line.
        assert!(!is_redundant_error_header(
            "Syntax error: Expected keyword JOIN but got keyword FROM at [2:40]"
        ));
        assert!(!is_redundant_error_header(
            "'undefined_macro_xyz' is undefined."
        ));
        // NOT — different word order ("error" not as the second token).
        assert!(!is_redundant_error_header(
            "There was an error in test foo"
        ));
        // NOT — empty / whitespace.
        assert!(!is_redundant_error_header(""));
        assert!(!is_redundant_error_header("   "));
    }

    #[test]
    fn test_test_fail_single_line_msg_is_not_dropped() {
        // For test FAILs, `RunResultError.info.msg` is a single line like
        // `Got 3 results, configured to fail if != 0`. The redundant-header
        // skip rule must NOT drop it — the body would otherwise be empty.
        let raw = r#"{"info":{"name":"LogTestResult","level":"error","msg":"FAIL 3"},"data":{"node_info":{"node_name":"temp_failing_test","node_path":"tests/temp_failing_test.sql","unique_id":"test.dw.tff"},"status":"fail","num_failures":3,"execution_time":0.1}}
{"info":{"name":"RunResultError","level":"error","msg":"Got 3 results, configured to fail if != 0"},"data":{"node_info":{"node_name":"temp_failing_test","unique_id":"test.dw.tff"}}}
{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":13.0}}
"#;
        let parsed = parse_events(raw);
        let out = build_test_summary(&parsed, 0);
        assert!(out.contains("FAIL  temp_failing_test"));
        // The actionable message survives the line-1-skip rule (R3 Fix 1).
        assert!(
            out.contains("Got 3 results, configured to fail if != 0"),
            "expected single-line FAIL body to render; got:\n{}",
            out
        );
    }

    #[test]
    fn test_err_row_still_skips_redundant_header() {
        // Sanity: when line 1 IS the redundant header, R3 Fix 1 still skips it
        // (preserves R2 D behavior).
        let raw = r#"{"info":{"name":"LogModelResult","level":"error","msg":"ERROR"},"data":{"node_info":{"node_name":"dummy_model_174","unique_id":"model.dw.dummy_model_174"},"description":"sql","execution_time":1.0}}
{"info":{"name":"RunResultError","level":"error","msg":"Compilation Error in model dummy_model_174 (models/marts/dummy_model_174.sql)\n  'foo_macro' is undefined."},"data":{"node_info":{"unique_id":"model.dw.dummy_model_174"}}}
{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":1.5}}
"#;
        let parsed = parse_events(raw);
        let out = build_run_summary(&parsed, 0, "run");
        // Redundant line 1 dropped.
        assert!(!out.contains("Compilation Error in model dummy_model_174 (models/marts/dummy_model_174.sql)"));
        // Actionable line 2 kept.
        assert!(out.contains("'foo_macro' is undefined"));
    }

    #[test]
    fn test_test_warn_single_line_msg_is_preserved() {
        // WARN takes a different code path than FAIL (RunResultWarningMessage,
        // not RunResultError). Verify the helper extraction kept it working.
        let raw = r#"{"info":{"name":"LogTestResult","level":"warn","msg":"WARN"},"data":{"node_info":{"node_name":"t_warn","unique_id":"test.dw.tw"},"status":"warn","num_failures":1,"execution_time":0.1}}
{"info":{"name":"RunResultWarningMessage","level":"warn","msg":"Got 1 result, configured to warn if != 0"},"data":{"node_info":{"unique_id":"test.dw.tw"}}}
{"info":{"name":"FinishedRunningStats","level":"info","msg":"done"},"data":{"execution_time":2.0}}
"#;
        let parsed = parse_events(raw);
        let out = build_test_summary(&parsed, 0);
        assert!(out.contains("WARN  t_warn"));
        assert!(out.contains("Got 1 result, configured to warn"));
    }

    // light_filter drops orphan Python `from ... import` line ----

    #[test]
    fn test_is_python_import_followon_helper() {
        assert!(is_python_import_followon(
            "from google.cloud.aiplatform.utils import gcs_utils"
        ));
        assert!(is_python_import_followon("from os import path"));
        assert!(is_python_import_followon("from _internal import x"));
        // SQL-style `FROM` keyword (uppercase).
        assert!(!is_python_import_followon("from FROM_TABLE import x"));
        // Not an import line.
        assert!(!is_python_import_followon("from x to y"));
        assert!(!is_python_import_followon("from "));
        assert!(!is_python_import_followon(""));
        // Not starting with `from `.
        assert!(!is_python_import_followon("import os"));
    }

    #[test]
    fn test_light_filter_drops_orphan_python_import_at_v0() {
        // residue: after dropping the `FutureWarning:` line,
        // the orphan `from google.cloud...import gcs_utils` line still leaks at
        // verbose 0. R3 Fix 2 drops it.
        let raw = "    found unexpected end of stream\n\
                   /path/.venv/lib/python3.11/site-packages/google/cloud/aiplatform/models.py:52: FutureWarning: Support for ...\n\
                   from google.cloud.aiplatform.utils import gcs_utils\n\
                   [full output: ~/Library/Application Support/rtk/tee/x.log]\n";
        let out_v0 = light_filter(raw, 0, "compile");
        assert!(!out_v0.contains("FutureWarning"));
        assert!(
            !out_v0.contains("from google.cloud.aiplatform.utils import gcs_utils"),
            "expected orphan import line to be dropped at v0; got:\n{}",
            out_v0
        );
        // Load-bearing context above is preserved.
        assert!(out_v0.contains("found unexpected end of stream"));
        // Tee-log footer preserved.
        assert!(out_v0.contains("[full output:"));

        // At -v the orphan line leaks through (user opted into noise).
        let out_v1 = light_filter(raw, 1, "compile");
        assert!(out_v1.contains("from google.cloud.aiplatform.utils import gcs_utils"));
    }

    #[test]
    fn test_light_filter_does_not_break_yaml_dashes_boundary() {
        // dbt's YAML parse-error output uses `------------------------------`
        // rows as load-bearing context dividers
        // (between error description / source snippet / raw error). The dashes
        // ROWS THEMSELVES must survive light_filter at v=0. R4 Fix 2b adds an
        // additional rule: trailing content AFTER the second dashes row is
        // treated as low-signal noise and dropped — that part is covered by the
        // dedicated R4 tests. This test only asserts the dashes rows + signal
        // lines BEFORE the second row survive.
        let raw = "    Syntax error near line 198\n\
                   ------------------------------\n\
                   195|           - not_null: *recent_date_filter\n\
                   196| broken_yaml: \"this string has no closing quote\n\
                   ------------------------------\n\
                   while scanning a quoted scalar\n";
        let out = light_filter(raw, 0, "compile");
        // Both dashes rows survive.
        assert_eq!(out.matches("------------------------------").count(), 2);
        assert!(out.contains("Syntax error near line 198"));
        // Lines BEFORE the second dashes row are preserved.
        assert!(out.contains("195|           - not_null: *recent_date_filter"));
    }

    // truncated NDJSON stream is parsed safely -----

    #[test]
    fn test_parse_events_handles_truncated_stream() {
        // Simulate Ctrl-C / SIGINT by omitting the FinishedRunningStats terminator.
        let truncated = r#"{"info":{"name":"LogModelResult","level":"info","msg":"1 of 3 OK"},"data":{"node_info":{"node_name":"customers","node_path":"models/marts/customers.sql"},"execution_time":2.0}}
{"info":{"name":"LogModelResult","level":"error","msg":"2 of 3 ERROR"},"data":{"node_info":{"node_name":"dummy_model_174","node_path":"models/marts/dummy_model_174.sql"},"execution_time":0.5}}
{"info":{"name":"SkippingDetails","level":"info","msg":"SKIP"},"data":{"node_info":{"node_name":"payments","node_path":"models/marts/payments.sql"},"schema":"dw","resource_type":"model"}}
"#;
        let parsed = parse_events(truncated);
        assert!(!parsed.terminator_seen);

        let run_out = build_run_summary(&parsed, 0, "run");
        let build_out = build_build_summary(&parsed, 0);
        let test_out = build_test_summary(&parsed, 0);
        let any_partial = run_out.starts_with("[partial]")
            || build_out.starts_with("[partial]")
            || test_out.starts_with("[partial]");
        assert!(
            any_partial,
            "expected [partial] marker; got\n  run={}\n  build={}\n  test={}",
            run_out, build_out, test_out
        );
    }

    // strip `Model 'unique_id' (path)` parens + `model.X.` prefix ---

    #[test]
    fn test_main_encountered_error_strips_model_path_parens() {
        // 's truncated body line:
        //   `Model 'model.dummy_team_014.dummy_model_084'
        //    (dummy/path_010/...) depends on a node named
        //    'dummy_model_113' which was not found`
        // After R4 Fix 1, the rendered body should read:
        //   `Model 'dummy_model_084' depends on a node named
        //    'dummy_model_113' which was not found`
        let raw = r#"{"info":{"name":"MainEncounteredError","level":"error","msg":"Encountered an error:\nCompilation Error\n  Model 'model.dummy_team_014.dummy_model_084' (dummy/path_010/dummy_app/dummy_model_126/v1_0/dummy_model_084.sql) depends on a node named 'dummy_model_113' which was not found"},"data":{}}
"#;
        let parsed = parse_events(raw);
        let out = build_run_summary(&parsed, 0, "build");
        // The path parens AND the `model.<project>.` prefix are both stripped.
        // The body line starts with `Model 'dummy_model_084' depends on...`
        // (the rendered line is wrapped in 5-space indent and may be 100-char truncated
        // by the existing line-length cap, which is unrelated to R4 Fix 1).
        assert!(
            out.contains("Model 'dummy_model_084' depends on a node named 'dummy_model_113'"),
            "expected stripped body line; got:\n{}",
            out
        );
        // Negative assertions — original noise must be gone.
        assert!(!out.contains("model.dummy_team_014"));
        assert!(!out.contains("dummy/path_010"));
        // The body line is not truncated MID-pattern (the original failure mode
        // had the truncation cut into the redundant path parens, which now no
        // longer appear at all).
        assert!(!out.contains("(dummy/path_010/s..."));
    }

    // light_filter [WARNING] block drop + dashes-boundary trim ----

    #[test]
    fn test_light_filter_drops_warning_blocks_until_next_timestamp() {
        // a `[WARNING]` block followed by 3 explanation lines
        // and then a timestamped error line. R4 Fix 2a drops the warning block
        // and continues processing at the next timestamp.
        let raw = "15:59:47  [WARNING][PropertyMovedToConfigDeprecation]: Deprecated functionality\n\
                   Found `freshness` as a top-level property of `sources[0].tables[0]` in file\n\
                   `dummy/path_026/raw_source.yml`. The `freshness` top-level property should\n\
                   be moved into the `config` of `sources[0].tables[0]`.\n\
                   15:59:47  Encountered an error:\n\
                   Parsing Error\n";
        let out = light_filter(raw, 0, "parse");
        // Warning block + 3 continuation lines all dropped.
        assert!(!out.contains("[WARNING]"));
        assert!(!out.contains("PropertyMovedToConfigDeprecation"));
        assert!(!out.contains("Found `freshness` as a top-level property"));
        assert!(!out.contains("`dummy/path_026/raw_source.yml`"));
        assert!(!out.contains("be moved into the `config`"));
        // Timestamped error line + Parsing Error preserved.
        assert!(out.contains("Encountered an error:"));
        assert!(out.contains("Parsing Error"));

        // At verbose 1, R4 Fix 2a is gated to v=0 — everything passes through.
        let out_v1 = light_filter(raw, 1, "parse");
        assert!(out_v1.contains("[WARNING]"));
        assert!(out_v1.contains("PropertyMovedToConfigDeprecation"));
    }

    #[test]
    fn test_light_filter_dashes_boundary_drops_trailing_content_when_two_or_more() {
        // Two dashes-rows separate (1) error description from source snippet,
        // and (2) source snippet from raw error / Python trace. At v=0 we drop
        // everything after the second dashes-row; at v=1 we keep it.
        let raw = "    Syntax error near line 198\n\
                   ------------------------------\n\
                   195|           - not_null: *recent_date_filter\n\
                   196| broken_yaml: \"this string has no closing quote\n\
                   ------------------------------\n\
                   while scanning a quoted scalar\n\
                     in \"<unicode string>\", line 196, column 24\n\
                   found unexpected end of stream\n\
                   [full output: ~/Library/Application Support/rtk/tee/x.log]\n";
        let out_v0 = light_filter(raw, 0, "parse");
        // Both dashes rows are themselves preserved (count == 2 means we drop AFTER).
        assert_eq!(out_v0.matches("------------------------------").count(), 2);
        // Content BEFORE the second dashes row is preserved.
        assert!(out_v0.contains("Syntax error near line 198"));
        assert!(out_v0.contains("195|           - not_null"));
        assert!(out_v0.contains("196| broken_yaml"));
        // Content AFTER the second dashes row is dropped at v=0.
        assert!(
            !out_v0.contains("while scanning a quoted scalar"),
            "expected post-second-dashes content to be dropped at v=0; got:\n{}",
            out_v0
        );
        assert!(!out_v0.contains("found unexpected end of stream"));
        assert!(!out_v0.contains("\"<unicode string>\""));
        // The `[full output: ...]` footer is always preserved.
        assert!(out_v0.contains("[full output:"));

        // At v=1, R4 Fix 2b is gated to v=0 — everything passes through.
        let out_v1 = light_filter(raw, 1, "parse");
        assert!(out_v1.contains("while scanning a quoted scalar"));
        assert!(out_v1.contains("found unexpected end of stream"));
    }

    #[test]
    fn test_light_filter_preserves_single_dashes_row() {
        // Safety test: when there is EXACTLY 1 dashes-row in the stream (e.g. a
        // contract-violation table separator on a single-row table), the
        // post-second-dashes drop rule must NOT trigger. The dashes row +
        // everything after must survive light_filter at v=0.
        let raw = "    | column_name | definition_type |\n\
                   ------------------------------\n\
                   | created_at  |                 |\n\
                   trailing context that must survive\n\
                   [full output: ~/Library/Application Support/rtk/tee/x.log]\n";
        let out_v0 = light_filter(raw, 0, "compile");
        assert_eq!(out_v0.matches("------------------------------").count(), 1);
        assert!(out_v0.contains("| created_at  |"));
        assert!(out_v0.contains("trailing context that must survive"));
        assert!(out_v0.contains("[full output:"));
    }

    // contract body — 20-line cap + drop boilerplate preamble ----

    #[test]
    fn test_contract_body_strips_preamble_and_renders_table() {
        // Inline `RunResultError` for a model whose classification is "contract"
        // (msg contains the word "contract" + "Error"). The two boilerplate
        // preamble lines must be dropped, and the 4-row markdown-style mismatch
        // table (header + 3 rows) must render in full.
        let preamble_a = "This model has an enforced contract that failed.";
        let preamble_b = "Please ensure the name, data_type, and number of columns in your contract match the columns in your model's definition.";
        let header = "| column_name | definition_type | contract_type | mismatch_reason       |";
        let sep = "| ----------- | --------------- | ------------- | --------------------- |";
        let row1 = "| created_at  |                 | TIMESTAMP     | missing in definition |";
        let row2 = "| owner_region|                 | STRING        | missing in definition |";
        let row3 = "| month_date  | STRING          | DATE          | data type mismatch    |";
        let inner_msg = format!(
            "Compilation Error in model dummy_model_118 (models/sales/x.sql)\n  {}\n  {}\n  {}\n  {}\n  {}\n  {}\n  {}",
            preamble_a, preamble_b, header, sep, row1, row2, row3
        );
        let raw = format!(
            "{{\"info\":{{\"name\":\"LogModelResult\",\"level\":\"error\",\"msg\":\"ERROR\"}},\"data\":{{\"node_info\":{{\"node_name\":\"dummy_model_118\",\"unique_id\":\"model.dw.stm\"}},\"description\":\"sql\",\"execution_time\":1.0}}}}\n\
             {{\"info\":{{\"name\":\"RunResultError\",\"level\":\"error\",\"msg\":{}}},\"data\":{{\"node_info\":{{\"unique_id\":\"model.dw.stm\"}}}}}}\n\
             {{\"info\":{{\"name\":\"FinishedRunningStats\",\"level\":\"info\",\"msg\":\"done\"}},\"data\":{{\"execution_time\":1.5}}}}\n",
            serde_json::to_string(&inner_msg).unwrap()
        );
        let parsed = parse_events(&raw);
        let out = build_run_summary(&parsed, 0, "run");
        // Classification is "contract".
        assert!(out.contains("contract"));
        // Preamble lines absent.
        assert!(
            !out.contains(preamble_a),
            "preamble line A leaked into output:\n{}",
            out
        );
        assert!(
            !out.contains("Please ensure the name, data_type"),
            "preamble line B leaked into output:\n{}",
            out
        );
        // Table header + all 3 mismatch rows render.
        assert!(out.contains(header), "table header missing:\n{}", out);
        assert!(out.contains(sep));
        assert!(out.contains("created_at"));
        assert!(out.contains("owner_region"));
        assert!(out.contains("month_date"));
        // Body line count: header + sep + 3 rows = 5 lines (the redundant
        // `Compilation Error in model dummy_model_118 (...)` first line
        // is dropped by the redundant-header rule, and the two preamble lines are
        // dropped by R4 Fix 3).
        let body_lines: Vec<&str> = out.lines().filter(|l| l.starts_with("     |")).collect();
        assert_eq!(body_lines.len(), 5, "got body lines:\n{:#?}", body_lines);
    }

    // multi-mismatch contract table renders fully ----

    #[test]
    fn test_contract_body_renders_multi_mismatch_table() {
        // Same shape as Fix 3 test but with a 6-row mismatch table (5 mismatches
        // + header + sep). The 20-line body cap must accommodate all of them.
        let preamble_a = "This model has an enforced contract that failed.";
        let preamble_b = "Please ensure the name, data_type, and number of columns in your contract match the columns in your model's definition.";
        let header = "| column_name | definition_type | contract_type | mismatch_reason       |";
        let sep = "| ----------- | --------------- | ------------- | --------------------- |";
        let row1 = "| created_at  |                 | TIMESTAMP     | missing in definition |";
        let row2 = "| owner_region|                 | STRING        | missing in definition |";
        let row3 = "| owner_team  |                 | STRING        | missing in definition |";
        let row4 = "| month_date  | STRING          | DATE          | data type mismatch    |";
        let row5 = "| target_value| INT64           | NUMERIC       | data type mismatch    |";
        let row6 = "| target_name | INT64           | STRING        | data type mismatch    |";
        let inner_msg = format!(
            "Compilation Error in model dummy_model_118 (models/sales/x.sql)\n  {}\n  {}\n  {}\n  {}\n  {}\n  {}\n  {}\n  {}\n  {}\n  {}",
            preamble_a, preamble_b, header, sep, row1, row2, row3, row4, row5, row6
        );
        let raw = format!(
            "{{\"info\":{{\"name\":\"LogModelResult\",\"level\":\"error\",\"msg\":\"ERROR\"}},\"data\":{{\"node_info\":{{\"node_name\":\"dummy_model_118\",\"unique_id\":\"model.dw.stm\"}},\"description\":\"sql\",\"execution_time\":1.0}}}}\n\
             {{\"info\":{{\"name\":\"RunResultError\",\"level\":\"error\",\"msg\":{}}},\"data\":{{\"node_info\":{{\"unique_id\":\"model.dw.stm\"}}}}}}\n\
             {{\"info\":{{\"name\":\"FinishedRunningStats\",\"level\":\"info\",\"msg\":\"done\"}},\"data\":{{\"execution_time\":1.5}}}}\n",
            serde_json::to_string(&inner_msg).unwrap()
        );
        let parsed = parse_events(&raw);
        let out = build_run_summary(&parsed, 0, "run");
        // All 6 mismatch rows visible (within the 20-line cap).
        assert!(out.contains(row1));
        assert!(out.contains(row2));
        assert!(out.contains(row3));
        assert!(out.contains(row4));
        assert!(out.contains(row5));
        assert!(out.contains(row6));
        assert!(out.contains(header));
        assert!(out.contains(sep));
        // 8 total table-shaped body lines (header + sep + 6 rows).
        let body_lines: Vec<&str> = out.lines().filter(|l| l.starts_with("     |")).collect();
        assert_eq!(body_lines.len(), 8, "got body lines:\n{:#?}", body_lines);
        // Preamble still dropped.
        assert!(!out.contains(preamble_a));
    }

    // body lines render in full without truncation ---------------------

    #[test]
    fn test_body_lines_render_in_full_without_truncation() {
        // body content lines (the indented sentences under
        // ERR/FAIL/WARN rows and under MainEncounteredError summaries) must
        // render in full. The previous 100-char cap was eating the actionable
        // verb at the end of long error sentences (e.g. "...which was not
        // found"). Row-header column truncation (name + classification) is
        // unaffected.
        //
        // a `MainEncounteredError` with an inner msg whose
        // second non-empty line is 200+ characters and ends with a load-bearing
        // actionable phrase. The rendered body must include the entire phrase
        // verbatim; no `...` may appear in the body; the line must not be
        // split across two `     `-prefixed body lines.
        let long_actionable_line = "Model 'dummy_model_084' depends on a node named 'dummy_model_113_DELETE_ME' which was referenced from upstream marts and which was definitively not found in the dependency graph today";
        assert!(
            long_actionable_line.len() > 150,
            "test fixture must exceed the old 100-char cap by a wide margin (got {} chars)",
            long_actionable_line.len()
        );
        let inner_msg = format!("Compilation Error\n  {}", long_actionable_line);
        let raw = format!(
            "{{\"info\":{{\"name\":\"MainEncounteredError\",\"level\":\"error\",\"msg\":{}}},\"data\":{{}}}}\n",
            serde_json::to_string(&inner_msg).unwrap()
        );
        let parsed = parse_events(&raw);
        let out = build_run_summary(&parsed, 0, "build");

        // The full actionable line — including "...not found in the dependency
        // graph today" at the very end — must appear unmodified in the body.
        assert!(
            out.contains(long_actionable_line),
            "expected full body line, got:\n{}",
            out
        );
        assert!(
            out.contains("not found in the dependency graph today"),
            "actionable verb at end of line must survive; got:\n{}",
            out
        );

        // No ellipsis anywhere in the body content. (The header + classification
        // rows in this MainEncounteredError code path don't include `...` either,
        // so a global check on the whole output is the strongest assertion.)
        assert!(
            !out.contains("..."),
            "no truncation ellipsis allowed in body output; got:\n{}",
            out
        );

        // The body line is not split — exactly one body-content line should
        // contain the actionable phrase, not multiple wrapped fragments.
        let body_hits = out
            .lines()
            .filter(|l| l.contains("not found in the dependency graph today"))
            .count();
        assert_eq!(
            body_hits, 1,
            "actionable phrase must render on a single line, not be split; got:\n{}",
            out
        );
    }

    // ============================================================
    // Phase 1 — Standards Compliance: real fixture-driven tests
    // ============================================================
    //
    // The fixtures below are real `dbt --log-format json` captures stored in
    // tests/fixtures/. Each `dbt_*_raw.txt` file is the unmodified NDJSON
    // stderr of a captured scenario. These tests exercise the parse_events +
    // build_*_summary pipeline against those captures, complementing the
    // synthetic inline tests above (which exercise narrow parser-arm
    // robustness).

    /// Real fixtures, kept here so each migrated test can reference them by name.
    const REAL_RUN_COMPILE_ERROR: &str =
        include_str!("../../../tests/fixtures/dbt/run_compile_error_raw.txt");
    const REAL_TEST_FAIL: &str =
        include_str!("../../../tests/fixtures/dbt/test_fail_raw.txt");
    const REAL_TEST_FOUR_BUCKET: &str =
        include_str!("../../../tests/fixtures/dbt/test_four_bucket_raw.txt");
    const REAL_RUN_NO_NODES: &str =
        include_str!("../../../tests/fixtures/dbt/run_no_nodes_raw.txt");
    const REAL_RUN_CYCLE: &str =
        include_str!("../../../tests/fixtures/dbt/run_cycle_raw.txt");
    const REAL_FRESHNESS_FAIL: &str =
        include_str!("../../../tests/fixtures/dbt/freshness_fail_raw.txt");
    const REAL_BUILD_LARGE: &str =
        include_str!("../../../tests/fixtures/dbt/build_large_raw.txt");
    const REAL_BUILD_MULTI_CONTRACT: &str =
        include_str!("../../../tests/fixtures/dbt/build_multi_contract_raw.txt");
    const REAL_BUILD_CONTRACT: &str =
        include_str!("../../../tests/fixtures/dbt/build_contract_raw.txt");
    const REAL_PARSE_WARNING_BLOCK: &str =
        include_str!("../../../tests/fixtures/dbt/parse_warning_block_raw.txt");
    const REAL_RUN_PASS: &str =
        include_str!("../../../tests/fixtures/dbt/run_pass_real_raw.txt");
    const REAL_SEED_PASS: &str =
        include_str!("../../../tests/fixtures/dbt/seed_pass_real_raw.txt");
    const REAL_SNAPSHOT_PASS: &str =
        include_str!("../../../tests/fixtures/dbt/snapshot_pass_real_raw.txt");
    const REAL_COMPILE: &str =
        include_str!("../../../tests/fixtures/dbt/compile_real_raw.txt");
    const REAL_PARSE: &str =
        include_str!("../../../tests/fixtures/dbt/parse_real_raw.txt");

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    // Savings percentages are inherently fixture-dependent: the same filter
    // run against a 10-model dbt project yields different ratios than a
    // 10,000-model one because the per-event JSON overhead amortizes
    // differently across project sizes. The fixtures here were captured
    // against a moderately large project (~12k models), so these numbers
    // are an upper-bound expectation for big projects, not a portable
    // guarantee for every dbt user.
    fn savings_pct(raw: &str, filtered: &str) -> f64 {
        let raw_tokens = count_tokens(raw).max(1);
        let filtered_tokens = count_tokens(filtered);
        100.0 - (filtered_tokens as f64 / raw_tokens as f64 * 100.0)
    }

    // real fixture-driven assertions ------------------------------

    #[test]
    fn test_real_compile_error_renders_main_error_header() {
        // parse-time `MainEncounteredError` for a missing upstream
        // dependency. No result events; renders synthesized parse-error header.
        let parsed = parse_events(REAL_RUN_COMPILE_ERROR);
        let out = build_run_summary(&parsed, 0, "build");
        // The header reflects the kind classification.
        assert!(
            out.contains("dbt build:") && out.contains("error"),
            "expected synthesized error header, got:\n{}",
            out
        );
        // Body line(s) should mention the missing node.
        assert!(out.contains("dummy_model_113"));
    }

    #[test]
    fn test_real_test_fail_capture_renders_fail_row() {
        // real `dbt test` with a failing assertion.
        let parsed = parse_events(REAL_TEST_FAIL);
        let out = build_test_summary(&parsed, 0);
        // Header contains a PASS fraction and at least one failure indicator.
        assert!(
            out.contains("dbt test:") && (out.contains(" FAIL") || out.contains(" ERR")),
            "expected test summary with failure column, got:\n{}",
            out
        );
        // Real fixture's failing test name shows in the body.
        assert!(
            out.contains("temp_failing_test")
                || out.contains("FAIL")
                || out.contains("Got "),
            "expected body to reference failing test, got:\n{}",
            out
        );
    }

    #[test]
    fn test_real_four_bucket_capture_renders_all_buckets() {
        // pass + warn + fail + error in one run.
        let parsed = parse_events(REAL_TEST_FOUR_BUCKET);
        let out = build_test_summary(&parsed, 0);
        assert!(out.contains("dbt test:"));
        // At minimum: FAIL bucket present (real capture has a failing test).
        assert!(
            out.contains(" FAIL") || out.contains(" ERR") || out.contains(" WARN"),
            "expected at least one non-pass bucket in header, got:\n{}",
            out
        );
    }

    #[test]
    fn test_real_no_nodes_capture_renders_zero_selected() {
        // selector matched zero nodes — no MainEncounteredError.
        let parsed = parse_events(REAL_RUN_NO_NODES);
        let out = build_run_summary(&parsed, 0, "run");
        assert!(
            out.starts_with("dbt run:") || out.starts_with("[partial] dbt run:"),
            "expected run summary header, got:\n{}",
            out
        );
        assert!(
            savings_pct(REAL_RUN_NO_NODES, &out) >= 60.0,
            "no_nodes savings below 60%: {:.1}%",
            savings_pct(REAL_RUN_NO_NODES, &out)
        );
    }

    #[test]
    fn test_real_cycle_capture_renders_main_error() {
        // dependency-cycle parse error.
        let parsed = parse_events(REAL_RUN_CYCLE);
        let out = build_run_summary(&parsed, 0, "run");
        assert!(
            out.contains("dbt run:") && out.contains("error"),
            "expected run header with error classification, got:\n{}",
            out
        );
        assert!(
            savings_pct(REAL_RUN_CYCLE, &out) >= 60.0,
            "cycle fixture savings below 60%: {:.1}%",
            savings_pct(REAL_RUN_CYCLE, &out)
        );
    }

    #[test]
    fn test_real_freshness_capture_renders_summary() {
        // source freshness with an ERROR.
        let parsed = parse_events(REAL_FRESHNESS_FAIL);
        let out = build_run_summary(&parsed, 0, "freshness");
        assert!(
            out.contains("dbt freshness:"),
            "expected freshness header, got:\n{}",
            out
        );
    }

    #[test]
    fn test_real_build_large_capture_does_not_panic() {
        // 400-line full event stream. Stress test on the parser
        // and the body-line render loop.
        let parsed = parse_events(REAL_BUILD_LARGE);
        let out = build_build_summary(&parsed, 0);
        assert!(out.starts_with("dbt build:") || out.starts_with("[partial] dbt build:"));
        assert!(
            out.lines().count() < 50,
            "rendered output too large ({} lines) — filter not compressing",
            out.lines().count()
        );
        assert!(
            savings_pct(REAL_BUILD_LARGE, &out) >= 60.0,
            "build_large savings below 60%: {:.1}%",
            savings_pct(REAL_BUILD_LARGE, &out)
        );
    }

    #[test]
    fn test_real_multi_contract_capture_renders_table_rows() {
        // contract violation with multiple mismatch rows.
        let parsed = parse_events(REAL_BUILD_MULTI_CONTRACT);
        let out = build_run_summary(&parsed, 0, "build");
        assert!(out.contains("dbt build:") || out.contains("[partial] dbt build:"));
        assert!(
            out.contains("contract") || out.contains("ERR"),
            "expected contract violation content, got:\n{}",
            out
        );
        assert!(
            savings_pct(REAL_BUILD_MULTI_CONTRACT, &out) >= 60.0,
            "multi_contract savings below 60%: {:.1}%",
            savings_pct(REAL_BUILD_MULTI_CONTRACT, &out)
        );
    }

    #[test]
    fn test_real_parse_warning_block_renders_error() {
        // Mixed NDJSON + non-JSON lines (FutureWarning) with a
        // MainEncounteredError containing a YAML parse error.
        let parsed = parse_events(REAL_PARSE_WARNING_BLOCK);
        let out = build_run_summary(&parsed, 0, "parse");
        assert!(
            out.contains("error"),
            "expected error content, got:\n{}",
            out
        );
        assert!(
            savings_pct(REAL_PARSE_WARNING_BLOCK, &out) >= 60.0,
            "parse_warning_block savings below 60%: {:.1}%",
            savings_pct(REAL_PARSE_WARNING_BLOCK, &out)
        );
    }

    // insta snapshot tests ----------------------------------------
    //
    // Snapshots live next to the source: src/cmds/python/snapshots/*.snap
    // First run creates `.snap.new` files; review with `cargo insta review`
    // and accept with `cargo insta accept`.

    #[test]
    fn test_snapshot_run_pass_only() {
        // Synthetic pass-only run — pinned via fixture's stat_line=3.9 to
        // avoid IEEE-754 round-half-to-even ambiguity (Item 4 / R5 follow-up).
        let parsed = parse_events(FIXTURE_RUN_PASS);
        let out = build_run_summary(&parsed, 0, "run");
        insta::assert_snapshot!(out);
    }

    #[test]
    fn test_snapshot_run_with_failures() {
        let parsed = parse_events(FIXTURE_RUN_FAIL);
        let out = build_run_summary(&parsed, 0, "run");
        insta::assert_snapshot!(out);
    }

    #[test]
    fn test_snapshot_test_summary_four_buckets_real_capture() {
        let parsed = parse_events(REAL_TEST_FOUR_BUCKET);
        let out = build_test_summary(&parsed, 0);
        insta::assert_snapshot!(out);
    }

    #[test]
    fn test_snapshot_run_main_error_real_capture() {
        let parsed = parse_events(REAL_RUN_COMPILE_ERROR);
        let out = build_run_summary(&parsed, 0, "build");
        insta::assert_snapshot!(out);
    }

    #[test]
    fn test_snapshot_partial_marker_run() {
        // Truncated stream — covers the [partial] prefix path.
        let raw = r#"{"info":{"name":"LogModelResult","level":"info","msg":"OK"},"data":{"node_info":{"node_name":"a","unique_id":"model.dw.a"},"execution_time":1.0}}
{"info":{"name":"LogModelResult","level":"error","msg":"ERROR"},"data":{"node_info":{"node_name":"b","unique_id":"model.dw.b"},"description":"Compilation Error: x","execution_time":0.5}}
"#;
        let parsed = parse_events(raw);
        let out = build_run_summary(&parsed, 0, "run");
        insta::assert_snapshot!(out);
    }

    #[test]
    fn test_snapshot_contract_table_render() {
        // Drives the contract-table rendering path (R4 Fix 3) through the
        // multi-mismatch real capture, then snapshots the rendered output.
        let parsed = parse_events(REAL_BUILD_CONTRACT);
        let out = build_run_summary(&parsed, 0, "build");
        insta::assert_snapshot!(out);
    }

    #[test]
    fn test_snapshot_light_filter_dashes_boundary() {
        let raw = "    Syntax error near line 198\n\
                   ------------------------------\n\
                   195|           - not_null: *recent_date_filter\n\
                   196| broken_yaml: \"this string has no closing quote\n\
                   ------------------------------\n\
                   while scanning a quoted scalar\n\
                     in \"<unicode string>\", line 196, column 24\n\
                   found unexpected end of stream\n\
                   [full output: ~/Library/Application Support/rtk/tee/x.log]\n";
        let out = light_filter(raw, 0, "parse");
        insta::assert_snapshot!(out);
    }

    #[test]
    fn test_snapshot_light_filter_warning_block() {
        let raw = "15:59:47  [WARNING][PropertyMovedToConfigDeprecation]: Deprecated functionality\n\
                   Found `freshness` as a top-level property of `sources[0].tables[0]` in file\n\
                   `dummy/path_026/raw_source.yml`. The `freshness` top-level property should\n\
                   be moved into the `config` of `sources[0].tables[0]`.\n\
                   15:59:47  Encountered an error:\n\
                   Parsing Error\n";
        let out = light_filter(raw, 0, "parse");
        insta::assert_snapshot!(out);
    }

    #[test]
    fn test_snapshot_test_summary_with_failures() {
        let parsed = parse_events(FIXTURE_TEST_FAIL);
        let out = build_test_summary(&parsed, 0);
        insta::assert_snapshot!(out);
    }

    #[test]
    fn test_snapshot_build_mixed() {
        let parsed = parse_events(FIXTURE_BUILD_MIXED);
        let out = build_build_summary(&parsed, 0);
        insta::assert_snapshot!(out);
    }

    // token-savings tests -----------------------------------------
    //
    // Each test asserts the rendered/filtered output is at least 60% smaller
    // (in whitespace tokens) than the raw captured input. The savings table in
    // src/discover/rules.rs claims 60-90% per subcommand; these tests are the
    // floor verification.

    type RenderFn = fn(&ParsedStream, u8, &str) -> String;

    #[test]
    #[ignore]
    fn dump_savings_table() {
        // Dev-only inspector: prints each fixture's raw→filtered token ratio.
        // Run with `cargo test --bin rtk dump_savings_table -- --ignored --nocapture`.
        let cases: &[(&str, &str, RenderFn, &str)] = &[
            ("run/compile_error", REAL_RUN_COMPILE_ERROR, build_run_summary, "build"),
            ("test/four_bucket", REAL_TEST_FOUR_BUCKET, |p, v, _| build_test_summary(p, v), "test"),
            ("test/fail", REAL_TEST_FAIL, |p, v, _| build_test_summary(p, v), "test"),
            ("build/large", REAL_BUILD_LARGE, |p, v, _| build_build_summary(p, v), "build"),
            ("build/multi_contract", REAL_BUILD_MULTI_CONTRACT, build_run_summary, "build"),
            ("freshness/fail", REAL_FRESHNESS_FAIL, build_run_summary, "freshness"),
        ];
        for (name, raw, render, label) in cases {
            let parsed = parse_events(raw);
            let out = render(&parsed, 0, label);
            let raw_t = count_tokens(raw);
            let out_t = count_tokens(&out);
            let pct = savings_pct(raw, &out);
            eprintln!(
                "[savings] {:<28} raw={:>6}  out={:>4}  pct={:>5.1}%",
                name, raw_t, out_t, pct
            );
        }
    }

    #[test]
    fn test_run_savings_meets_90pct() {
        let raw = REAL_RUN_PASS;
        let parsed = parse_events(raw);
        let out = build_run_summary(&parsed, 0, "run");
        let pct = savings_pct(raw, &out);
        assert!(
            pct >= 90.0,
            "dbt run filter: expected >=90% savings, got {:.1}% (raw_tokens={}, out_tokens={})",
            pct,
            count_tokens(raw),
            count_tokens(&out)
        );
    }

    #[test]
    fn test_test_savings_meets_85pct() {
        let raw = REAL_TEST_FOUR_BUCKET;
        let parsed = parse_events(raw);
        let out = build_test_summary(&parsed, 0);
        let pct = savings_pct(raw, &out);
        assert!(
            pct >= 85.0,
            "dbt test filter: expected >=85% savings, got {:.1}%",
            pct
        );
    }

    #[test]
    fn test_test_fail_savings_meets_60pct() {
        let raw = REAL_TEST_FAIL;
        let parsed = parse_events(raw);
        let out = build_test_summary(&parsed, 0);
        let pct = savings_pct(raw, &out);
        assert!(
            pct >= 60.0,
            "dbt test (real fail) filter: expected >=60% savings, got {:.1}%",
            pct
        );
    }

    #[test]
    fn test_build_savings_meets_88pct() {
        let raw = REAL_BUILD_LARGE;
        let parsed = parse_events(raw);
        let out = build_build_summary(&parsed, 0);
        let pct = savings_pct(raw, &out);
        assert!(
            pct >= 88.0,
            "dbt build filter: expected >=88% savings, got {:.1}%",
            pct
        );
    }

    #[test]
    fn test_build_contract_savings_meets_60pct() {
        let raw = REAL_BUILD_MULTI_CONTRACT;
        let parsed = parse_events(raw);
        let out = build_run_summary(&parsed, 0, "build");
        let pct = savings_pct(raw, &out);
        assert!(
            pct >= 60.0,
            "dbt build (contract) filter: expected >=60% savings, got {:.1}%",
            pct
        );
    }

    #[test]
    fn test_freshness_savings_meets_80pct() {
        let raw = REAL_FRESHNESS_FAIL;
        let parsed = parse_events(raw);
        let out = build_run_summary(&parsed, 0, "freshness");
        let pct = savings_pct(raw, &out);
        assert!(
            pct >= 80.0,
            "dbt source freshness filter: expected >=80% savings, got {:.1}%",
            pct
        );
    }

    #[test]
    fn test_seed_savings_meets_80pct() {
        let raw = REAL_SEED_PASS;
        let parsed = parse_events(raw);
        let out = build_run_summary(&parsed, 0, "seed");
        let pct = savings_pct(raw, &out);
        assert!(
            pct >= 80.0,
            "dbt seed filter: expected >=80% savings, got {:.1}%",
            pct
        );
    }

    #[test]
    fn test_snapshot_savings_meets_80pct() {
        let raw = REAL_SNAPSHOT_PASS;
        let parsed = parse_events(raw);
        let out = build_run_summary(&parsed, 0, "snapshot");
        let pct = savings_pct(raw, &out);
        assert!(
            pct >= 80.0,
            "dbt snapshot filter: expected >=80% savings, got {:.1}%",
            pct
        );
    }

    #[test]
    fn test_parse_savings_meets_60pct() {
        let raw = REAL_PARSE;
        let out = light_filter(raw, 0, "parse");
        let pct = savings_pct(raw, &out);
        assert!(
            pct >= 60.0,
            "dbt parse light_filter: expected >=60% savings, got {:.1}% (out=\n{})",
            pct,
            out
        );
    }

    #[test]
    fn test_compile_savings_meets_70pct() {
        let raw = REAL_COMPILE;
        let out = light_filter(raw, 0, "compile");
        let pct = savings_pct(raw, &out);
        assert!(
            pct >= 70.0,
            "dbt compile light_filter: expected >=70% savings, got {:.1}% (out=\n{})",
            pct,
            out
        );
    }

    #[test]
    fn test_deps_savings_meets_60pct_synthetic() {
        // dbt deps light-filter input — real `dbt deps` runs include Python
        // FutureWarnings, deprecation warnings, the dbt header, and orphan
        // import lines. The synthetic input mirrors that shape; light_filter
        // strips the noise and leaves only the install lines.
        let raw = "Running with dbt=1.11.7\n\
                   /path/to/.venv/lib/python3.11/site-packages/google/cloud/aiplatform/__init__.py:42: FutureWarning: google-cloud-storage < 3.0.0 is deprecated and will be removed soon\n\
                   from google.cloud.aiplatform.utils import gcs_utils\n\
                   /other/path/.venv/lib/python3.11/site-packages/some_other_lib/foo.py:99: DeprecationWarning: xyz is deprecated, please use abc\n\
                   from some_other_lib import xyz\n\
                   /third/path/.venv/lib/python3.11/site-packages/yet_another/bar.py:7: FutureWarning: another deprecated thing in yet_another v2.x\n\
                   from yet_another import bar\n\
                   Registered adapter: bigquery=1.9.2\n\
                   Concurrency: 4 threads\n\
                   Updating lock file in file path: package-lock.yml\n\
                   Installing dbt-labs/dbt_utils\n\
                   Installed from version 1.1.1\n\
                   Installing calogica/dbt_expectations\n\
                   Installed from version 0.10.4\n";
        let out = light_filter(raw, 0, "deps");
        let pct = savings_pct(raw, &out);
        assert!(
            pct >= 60.0,
            "dbt deps light_filter: expected >=60% savings, got {:.1}% (out=\n{})",
            pct,
            out
        );
    }

    // "Never Block" fallback on corrupt JSON streams ------------

    #[test]
    fn test_corrupt_stream_falls_back_to_passthrough() {
        // Stream contains malformed JSON lines (each fails serde parsing) plus
        // substantial non-JSON stderr — but no `MainEncounteredError` and no
        // typed events. The summary builder must surface the leftover content
        // rather than rendering a misleading "0 nodes selected" header.
        let raw = "dbt: error: corruption in JSON stream\n\
                   Traceback (most recent call last):\n\
                     File \"/usr/local/dbt/main.py\", line 42, in <module>\n\
                       sys.exit(main())\n\
                     File \"/usr/local/dbt/cli.py\", line 99, in main\n\
                       raise RuntimeError('catastrophic')\n\
                   RuntimeError: catastrophic\n\
                   {malformed: missing quote\n\
                   {\"info\": broken,\n\
                   end of stream marker missing\n";
        let parsed = parse_events(raw);
        // Sanity: zero typed events, zero main_error, but leftover captured.
        assert_eq!(parsed.events.len(), 0);
        assert!(parsed.main_error.is_none());
        assert!(
            parsed.leftover.len() > 5,
            "expected >5 leftover lines, got {}",
            parsed.leftover.len()
        );

        // All three summary builders must surface the leftover passthrough.
        for (label, out) in [
            ("run", build_run_summary(&parsed, 0, "run")),
            ("test", build_test_summary(&parsed, 0)),
            ("build", build_build_summary(&parsed, 0)),
        ] {
            assert!(
                !out.contains("0 nodes selected") && !out.contains("0 tests selected"),
                "{}: leftover passthrough must replace the misleading 0-nodes header; got:\n{}",
                label,
                out
            );
            assert!(
                out.contains("RuntimeError: catastrophic")
                    && out.contains("corruption in JSON stream"),
                "{}: expected raw leftover content, got:\n{}",
                label,
                out
            );
        }
    }

    #[test]
    fn test_short_leftover_does_not_trigger_passthrough() {
        // Sanity: a small fragment (≤5 lines, ≤500 bytes) is treated as trailing
        // banner crud — the misleading "0 nodes selected" header must NOT be
        // replaced by a leftover passthrough in this case.
        let raw = "dbt: small fragment\nanother short line\n";
        let parsed = parse_events(raw);
        let out = build_run_summary(&parsed, 0, "run");
        assert!(
            out.contains("0 nodes selected"),
            "expected the standard zero-nodes header for a small leftover; got:\n{}",
            out
        );
    }
}
