//! Azure CLI output compression.
//!
//! Filters verbose JSON from `az pipelines`, `az devops invoke`, and `az account`
//! into compact summaries. Specialized handlers for high-frequency Azure DevOps
//! commands; generic JSON compression fallback for everything else.

use crate::core::tee::force_tee_hint;
use crate::core::tracking;
use crate::core::utils::{exit_code_from_output, exit_code_from_status, resolved_command, truncate_iso_date};
use crate::json_cmd;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use serde_json::Value;

const MAX_ITEMS: usize = 20;
const GENERIC_COMPRESS_DEPTH: usize = 8;
const MAX_GENERIC_RAW_BYTES: usize = 20_000;
/// Default tail size for `az devops invoke --resource logs`. Integration-test
/// failure diagnosis (Reqnroll, dotnet-test) needs ~70-200 lines between the
/// ##[error] footer and the actual AssertionCheck/Expected line. 50 was too
/// aggressive — real failures in AutoMiner pipeline 247 showed the assertion
/// reason sitting ~68 lines above the footer, outside the window.
const LOG_TAIL_LINES: usize = 200;

/// Pure parser split from the env lookup so unit tests can exercise all
/// branches without mutating the process environment.
fn parse_log_tail(raw: Option<&str>) -> usize {
    raw.and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(LOG_TAIL_LINES)
}

/// Minimum byte size for an embedded JSON payload to be elided from a log
/// line. AutoMiner pipeline 247 logs routinely emit lines like
/// `ENDPOINT_DATA_<guid>={"environment":...,...}` (~1.7 KB) and
/// `METADATA_<guid>={"name":...,...}` (~0.7 KB). Above 1 KB these crowd out
/// real failure context under the (now 200-line) tail.
const EMBEDDED_PAYLOAD_THRESHOLD: usize = 1024;

/// If `line` carries a large embedded JSON payload (`KEY={...}` or `[...]`
/// style), replace the payload with a `<json elided bytes=N>` marker so the
/// reader keeps the surrounding context without paying the token cost. The
/// prefix (everything before the first `{`/`[`) is preserved verbatim, so
/// keys like `ENDPOINT_DATA_<guid>=` stay visible.
///
/// Returns `None` for lines under `threshold`, lines without a JSON blob, or
/// lines where the candidate blob fails to parse as JSON (prevents false
/// positives on long plain-text stack traces).
fn elide_oversized_embedded_json(line: &str, threshold: usize) -> Option<String> {
    if line.len() < threshold {
        return None;
    }
    let json_start = line.find(|c: char| c == '{' || c == '[')?;
    let tail = &line[json_start..];
    if tail.len() < threshold {
        return None;
    }
    // Require the tail to parse as JSON — otherwise the `{` is probably from
    // a stack trace (`Dictionary<string, {...}>`) and we'd mangle it.
    serde_json::from_str::<Value>(tail).ok()?;
    let prefix = &line[..json_start];
    Some(format!("{prefix}<json elided bytes={}>", tail.len()))
}

/// Env override for `LOG_TAIL_LINES`. Set `RTK_AZ_LOG_TAIL=N` to widen/narrow
/// the tail for a single command or session. Falls back to the default on
/// unset or unparseable values; ignores 0 to avoid accidentally disabling
/// output entirely.
fn resolved_log_tail() -> usize {
    parse_log_tail(std::env::var("RTK_AZ_LOG_TAIL").ok().as_deref())
}

/// Keys to strip recursively from generic `az` JSON output — HATEOAS links,
/// resource URIs, internal GUIDs, revision counters, and other Azure DevOps
/// boilerplate that inflates payloads without informing a CI-triage reader.
/// Specialized filters already drop these; the generic path needs the same
/// treatment so unspecialized subcommands (e.g. `az repos list`) stay compact.
const NOISE_KEYS: &[&str] = &[
    "_links",
    "url",
    "collectionUri",
    "projectUri",
    "projectId",
    "revision",
    "uniqueName",
    "imageUrl",
    "descriptor",
    "href",
];

lazy_static! {
    static ref TIMESTAMP_RE: Regex =
        Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z\s*").unwrap();
}

struct FilterResult {
    text: String,
    truncated: bool,
}

impl FilterResult {
    fn new(text: String) -> Self {
        Self {
            text,
            truncated: false,
        }
    }

    fn truncated(text: String) -> Self {
        Self {
            text,
            truncated: true,
        }
    }
}

// ─── Entry point ───────────────────────────────────────────────────────────

/// Run an Azure CLI command with token-optimized output
pub fn run(subcommand: &str, args: &[String], verbose: u8) -> Result<i32> {
    let full_sub = if args.is_empty() {
        subcommand.to_string()
    } else {
        format!("{} {}", subcommand, args.join(" "))
    };

    match subcommand {
        "pipelines" if !args.is_empty() && args[0] == "build" => {
            run_pipelines_build(&args[1..], verbose, &full_sub)
        }
        "pipelines" if !args.is_empty() && args[0] == "runs" => {
            run_pipelines_runs(&args[1..], verbose, &full_sub)
        }
        "devops" if !args.is_empty() && args[0] == "invoke" => {
            run_devops_invoke(&args[1..], verbose, &full_sub)
        }
        "account" if !args.is_empty() && args[0] == "get-access-token" => {
            run_account_token(&args[1..], verbose, &full_sub)
        }
        _ => run_generic(subcommand, args, verbose, &full_sub),
    }
}

// ─── az pipelines build (list | show) ──────────────────────────────────────

fn run_pipelines_build(args: &[String], verbose: u8, full_sub: &str) -> Result<i32> {
    if !args.is_empty() && args[0] == "list" {
        return run_az_filtered(&build_az_args("pipelines", &["build", "list"], &args[1..]), verbose, full_sub, filter_build_list);
    }
    if !args.is_empty() && args[0] == "show" {
        return run_az_filtered(&build_az_args("pipelines", &["build", "show"], &args[1..]), verbose, full_sub, filter_build_show);
    }
    run_generic("pipelines", &prepend_arg("build", args), verbose, full_sub)
}

fn filter_build_list(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let builds = v.as_array()?;

    let total = builds.len();
    if total == 0 {
        return Some(FilterResult::new("No builds found".to_string()));
    }

    let truncated = total > MAX_ITEMS;
    let mut lines = Vec::new();

    for b in builds.iter().take(MAX_ITEMS) {
        let id = b["id"].as_i64().unwrap_or(0);
        let result = b["result"].as_str().unwrap_or("?");
        let status = b["status"].as_str().unwrap_or("?");
        let def_name = b["definition"]["name"].as_str().unwrap_or("?");
        let build_number = b["buildNumber"].as_str().unwrap_or("?");
        let branch = b["sourceBranch"].as_str().unwrap_or("?");
        let reason = b["reason"].as_str().unwrap_or("?");
        let finish = b["finishTime"]
            .as_str()
            .map(truncate_iso_date)
            .unwrap_or("running");

        // Shorten branch refs
        let short_branch = branch
            .strip_prefix("refs/heads/")
            .or_else(|| branch.strip_prefix("refs/pull/"))
            .unwrap_or(branch);

        let icon = match result {
            "succeeded" => "ok",
            "failed" => "FAIL",
            "canceled" => "skip",
            _ if status == "inProgress" => "...",
            _ => "?",
        };

        lines.push(format!(
            "{} {} #{} | {} | {} | {} | {} | {}",
            icon, id, build_number, def_name, result, short_branch, reason, finish
        ));
    }

    let text = if truncated {
        format!(
            "{}\n... +{} more builds",
            lines.join("\n"),
            total - MAX_ITEMS
        )
    } else {
        lines.join("\n")
    };

    Some(if truncated {
        FilterResult::truncated(text)
    } else {
        FilterResult::new(text)
    })
}

fn filter_build_show(json_str: &str) -> Option<FilterResult> {
    let b: Value = serde_json::from_str(json_str).ok()?;

    let id = b["id"].as_i64().unwrap_or(0);
    let result = b["result"].as_str().unwrap_or("?");
    let status = b["status"].as_str().unwrap_or("?");
    let def_name = b["definition"]["name"].as_str().unwrap_or("?");
    let build_number = b["buildNumber"].as_str().unwrap_or("?");
    let reason = b["reason"].as_str().unwrap_or("?");
    let requested_by = b["requestedBy"]["displayName"].as_str().unwrap_or("?");

    let branch = b["sourceBranch"].as_str().unwrap_or("?");
    let short_branch = branch
        .strip_prefix("refs/heads/")
        .or_else(|| branch.strip_prefix("refs/pull/"))
        .unwrap_or(branch);

    let source_version = b["sourceVersion"].as_str().unwrap_or("?");
    let short_commit = if source_version.len() >= 7 {
        &source_version[..7]
    } else {
        source_version
    };

    let start = b["startTime"].as_str();
    let finish = b["finishTime"].as_str();

    let icon = match result {
        "succeeded" => "ok",
        "failed" => "FAIL",
        "canceled" => "skip",
        _ if status == "inProgress" => "...",
        _ => "?",
    };

    // Calculate duration if both timestamps present
    let time_info = match (start, finish) {
        (Some(s), Some(f)) => {
            match (chrono::DateTime::parse_from_rfc3339(s), chrono::DateTime::parse_from_rfc3339(f)) {
                (Ok(start_dt), Ok(finish_dt)) => {
                    let duration = finish_dt.signed_duration_since(start_dt);
                    let minutes = duration.num_minutes();
                    format!(
                        "started: {} | finished: {} ({}m)",
                        truncate_iso_date(s),
                        truncate_iso_date(f),
                        minutes
                    )
                }
                _ => format!(
                    "started: {} | finished: {}",
                    truncate_iso_date(s),
                    truncate_iso_date(f)
                ),
            }
        }
        (Some(s), None) => format!("started: {} | finished: running", truncate_iso_date(s)),
        (None, Some(f)) => format!("finished: {}", truncate_iso_date(f)),
        (None, None) => "started: ? | finished: ?".to_string(),
    };

    Some(FilterResult::new(format!(
        "{} build {} | {} #{}\n  branch: {} | commit: {}\n  reason: {} | by: {}\n  {}",
        icon, id, def_name, build_number, short_branch, short_commit, reason, requested_by, time_info
    )))
}

// ─── az pipelines runs (list | show) ───────────────────────────────────────

fn run_pipelines_runs(args: &[String], verbose: u8, full_sub: &str) -> Result<i32> {
    if !args.is_empty() && args[0] == "list" {
        return run_az_filtered(&build_az_args("pipelines", &["runs", "list"], &args[1..]), verbose, full_sub, filter_runs_list);
    }
    if !args.is_empty() && args[0] == "show" {
        return run_az_filtered(&build_az_args("pipelines", &["runs", "show"], &args[1..]), verbose, full_sub, filter_runs_show);
    }
    run_generic("pipelines", &prepend_arg("runs", args), verbose, full_sub)
}

fn filter_runs_list(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let runs = v.as_array()?;

    let total = runs.len();
    if total == 0 {
        return Some(FilterResult::new("No runs found".to_string()));
    }

    let truncated = total > MAX_ITEMS;
    let mut lines = Vec::new();

    for r in runs.iter().take(MAX_ITEMS) {
        let id = r["id"].as_i64().unwrap_or(0);
        let result = r["result"].as_str().unwrap_or("?");
        let status = r["status"].as_str().unwrap_or("?");
        let def_name = r["definition"]["name"].as_str().unwrap_or("?");
        let build_number = r["buildNumber"].as_str().unwrap_or("?");
        let branch = r["sourceBranch"].as_str().unwrap_or("?");
        let reason = r["reason"].as_str().unwrap_or("?");
        let finish = r["finishTime"]
            .as_str()
            .map(truncate_iso_date)
            .unwrap_or("running");

        // Shorten branch refs
        let short_branch = branch
            .strip_prefix("refs/heads/")
            .or_else(|| branch.strip_prefix("refs/pull/"))
            .unwrap_or(branch);

        let icon = match result {
            "succeeded" => "ok",
            "failed" => "FAIL",
            "canceled" => "skip",
            _ if status == "inProgress" => "...",
            _ => "?",
        };

        lines.push(format!(
            "{} {} #{} | {} | {} | {} | {} | {}",
            icon, id, build_number, def_name, result, short_branch, reason, finish
        ));
    }

    let text = if truncated {
        format!(
            "{}\n... +{} more runs",
            lines.join("\n"),
            total - MAX_ITEMS
        )
    } else {
        lines.join("\n")
    };

    Some(if truncated {
        FilterResult::truncated(text)
    } else {
        FilterResult::new(text)
    })
}

fn filter_runs_show(json_str: &str) -> Option<FilterResult> {
    let r: Value = serde_json::from_str(json_str).ok()?;

    let id = r["id"].as_i64().unwrap_or(0);
    let result = r["result"].as_str().unwrap_or("?");
    let status = r["status"].as_str().unwrap_or("?");
    let def_name = r["definition"]["name"].as_str().unwrap_or("?");
    let build_number = r["buildNumber"].as_str().unwrap_or("?");
    let reason = r["reason"].as_str().unwrap_or("?");
    let requested_by = r["requestedBy"]["displayName"].as_str().unwrap_or("?");

    let branch = r["sourceBranch"].as_str().unwrap_or("?");
    let short_branch = branch
        .strip_prefix("refs/heads/")
        .or_else(|| branch.strip_prefix("refs/pull/"))
        .unwrap_or(branch);

    let source_version = r["sourceVersion"].as_str().unwrap_or("?");
    let short_commit = if source_version.len() >= 7 {
        &source_version[..7]
    } else {
        source_version
    };

    let start = r["startTime"].as_str();
    let finish = r["finishTime"].as_str();

    let icon = match result {
        "succeeded" => "ok",
        "failed" => "FAIL",
        "canceled" => "skip",
        _ if status == "inProgress" => "...",
        _ => "?",
    };

    // Calculate duration if both timestamps present
    let time_info = match (start, finish) {
        (Some(s), Some(f)) => {
            match (chrono::DateTime::parse_from_rfc3339(s), chrono::DateTime::parse_from_rfc3339(f)) {
                (Ok(start_dt), Ok(finish_dt)) => {
                    let duration = finish_dt.signed_duration_since(start_dt);
                    let minutes = duration.num_minutes();
                    format!(
                        "started: {} | finished: {} ({}m)",
                        truncate_iso_date(s),
                        truncate_iso_date(f),
                        minutes
                    )
                }
                _ => format!(
                    "started: {} | finished: {}",
                    truncate_iso_date(s),
                    truncate_iso_date(f)
                ),
            }
        }
        (Some(s), None) => format!("started: {} | finished: running", truncate_iso_date(s)),
        (None, Some(f)) => format!("finished: {}", truncate_iso_date(f)),
        (None, None) => "started: ? | finished: ?".to_string(),
    };

    Some(FilterResult::new(format!(
        "{} run {} | {} #{}\n  branch: {} | commit: {}\n  reason: {} | by: {}\n  {}",
        icon, id, def_name, build_number, short_branch, short_commit, reason, requested_by, time_info
    )))
}

// ─── az devops invoke (timeline | logs) ────────────────────────────────────

fn run_devops_invoke(args: &[String], verbose: u8, full_sub: &str) -> Result<i32> {
    let resource = find_flag_value(args, "--resource");

    match resource.as_deref() {
        Some("timeline") => {
            run_az_filtered(&build_az_args("devops", &["invoke"], args), verbose, full_sub, filter_timeline)
        }
        Some("logs") => {
            run_az_text_filtered(&build_az_args("devops", &["invoke"], args), verbose, full_sub, filter_logs)
        }
        _ => run_generic("devops", &prepend_arg("invoke", args), verbose, full_sub),
    }
}

fn filter_timeline(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let records = v["records"].as_array()?;

    let total = records.len();
    let mut failed_count = 0;
    let mut lines = Vec::new();

    for r in records {
        let name = r["name"].as_str().unwrap_or("?");
        let rtype = r["type"].as_str().unwrap_or("?");
        let result = r["result"].as_str().unwrap_or("?");
        let start = r["startTime"].as_str();
        let finish = r["finishTime"].as_str();

        // Calculate duration
        let duration_str = match (start, finish) {
            (Some(s), Some(f)) => {
                match (chrono::DateTime::parse_from_rfc3339(s), chrono::DateTime::parse_from_rfc3339(f)) {
                    (Ok(start_dt), Ok(finish_dt)) => {
                        let duration = finish_dt.signed_duration_since(start_dt);
                        let minutes = duration.num_minutes();
                        let seconds = duration.num_seconds() % 60;
                        if minutes > 0 {
                            format!("{}m{}s", minutes, seconds)
                        } else {
                            format!("{}s", seconds)
                        }
                    }
                    _ => "?".to_string(),
                }
            }
            _ => "?".to_string(),
        };

        if result == "failed" {
            failed_count += 1;
            let log_id = r["log"]["id"].as_i64();
            let log_str = match log_id {
                Some(id) => format!("logId:{}", id),
                None => "logId:-".to_string(),
            };

            let error_count = r["errorCount"].as_i64().unwrap_or(0);
            let warning_count = r["warningCount"].as_i64().unwrap_or(0);

            let mut counters = Vec::new();
            if error_count > 0 {
                counters.push(format!("{}err", error_count));
            }
            if warning_count > 0 {
                counters.push(format!("{}warn", warning_count));
            }
            let counter_str = if counters.is_empty() {
                String::new()
            } else {
                format!(" {}", counters.join(" "))
            };

            lines.push(format!(
                "  FAIL {} ({}) {}{} {}",
                name, rtype, log_str, counter_str, duration_str
            ));

            // Add first issue message if present
            if let Some(issues) = r["issues"].as_array() {
                if let Some(first_issue) = issues.first() {
                    if let Some(msg) = first_issue["message"].as_str() {
                        lines.push(format!("    > {}", msg));
                    }
                }
            }
        } else {
            lines.push(format!("  ok {} ({}) {}", name, rtype, duration_str));
        }
    }

    let summary = if failed_count == 0 {
        format!("ok timeline: 0 failed of {} tasks", total)
    } else {
        format!("FAIL timeline: {} failed of {} tasks", failed_count, total)
    };

    let full_text = if lines.is_empty() {
        summary
    } else {
        format!("{}\n{}", summary, lines.join("\n"))
    };

    Some(FilterResult::new(full_text))
}

fn filter_logs(raw: &str) -> Option<FilterResult> {
    // az devops invoke --resource logs returns JSON with a "value" array of log lines,
    // OR when fetching a specific log, the lines are in the "value" array too.
    if let Ok(v) = serde_json::from_str::<Value>(raw) {
        if let Some(log_lines) = v["value"].as_array() {
            // List of available logs (each has id and lineCount)
            if log_lines.first().and_then(|l| l["lineCount"].as_i64()).is_some() {
                let mut lines = Vec::new();
                for log in log_lines {
                    let id = log["id"].as_i64().unwrap_or(0);
                    let count = log["lineCount"].as_i64().unwrap_or(0);
                    lines.push(format!("logId:{} ({} lines)", id, count));
                }
                return Some(FilterResult::new(lines.join("\n")));
            }

            // Actual log content — array of strings with timestamp prefixes
            let total = log_lines.len();
            let tail_size = resolved_log_tail();
            let tail: Vec<&Value> = if total > tail_size {
                log_lines[total - tail_size..].iter().collect()
            } else {
                log_lines.iter().collect()
            };

            let truncated = total > tail_size;
            let mut stripped = Vec::new();

            for line in &tail {
                if let Some(s) = (*line).as_str() {
                    let cleaned = TIMESTAMP_RE.replace(s, "").to_string();
                    let rendered = elide_oversized_embedded_json(&cleaned, EMBEDDED_PAYLOAD_THRESHOLD)
                        .unwrap_or(cleaned);
                    stripped.push(rendered);
                }
            }

            let mut text = stripped.join("\n");
            if truncated {
                text = format!("... ({} lines total, showing last {})\n{}", total, tail_size, text);
            }

            return Some(if truncated {
                FilterResult::truncated(text)
            } else {
                FilterResult::new(text)
            });
        }
    }

    // Not JSON or unexpected structure — pass through
    None
}

// ─── az account get-access-token ───────────────────────────────────────────

fn run_account_token(args: &[String], verbose: u8, full_sub: &str) -> Result<i32> {
    run_az_filtered(
        &build_az_args("account", &["get-access-token"], args),
        verbose,
        full_sub,
        filter_access_token,
    )
}

fn filter_access_token(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let token = v["accessToken"].as_str()?;
    let token_type = v["tokenType"].as_str().unwrap_or("Bearer");
    let subscription = v["subscription"].as_str().unwrap_or("?");
    let tenant = v["tenant"].as_str().unwrap_or("?");
    let expires = v["expiresOn"]
        .as_str()
        .map(truncate_iso_date)
        .unwrap_or("?");

    // Mask the token — only show prefix to confirm it was obtained
    let prefix_len = token.len().min(10);
    let prefix = &token[..prefix_len];

    Some(FilterResult::new(format!(
        "{} [{}...] | sub:{} | tenant:{} | expires:{}",
        token_type, prefix, subscription, tenant, expires
    )))
}

// ─── Generic fallback ──────────────────────────────────────────────────────

/// Recursively strip known-noise keys from an Azure DevOps JSON payload.
/// Operates in place — callers pass an owned `Value` they want pruned.
fn prune_az_noise(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|k, _| !NOISE_KEYS.contains(&k.as_str()));
            for v in map.values_mut() {
                prune_az_noise(v);
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                prune_az_noise(v);
            }
        }
        _ => {}
    }
}

/// Generic compressor for `az` subcommands without a specialized handler.
///
/// Parses stdout as JSON, prunes Azure noise keys, caps top-level arrays at
/// `MAX_ITEMS`, then delegates to `json_cmd::filter_json_compact` for
/// pretty-compact rendering. Values are preserved — this function never emits
/// schema-only output (that was the pre-fix bug).
///
/// Returns `(rendered, truncated)`. `truncated` is true when either the
/// top-level array was capped or the raw fallback was tail-capped — callers
/// may surface a hint to the user.
fn compress_az_generic(raw: &str) -> (String, bool) {
    let Ok(mut value) = serde_json::from_str::<Value>(raw) else {
        // Non-JSON output (e.g. --output tsv, --output table). Pass through
        // with a byte-length tail cap so the output stays bounded.
        if raw.len() > MAX_GENERIC_RAW_BYTES {
            let tail = &raw[raw.len() - MAX_GENERIC_RAW_BYTES..];
            return (
                format!(
                    "... ({} bytes total, showing last {})\n{}",
                    raw.len(),
                    MAX_GENERIC_RAW_BYTES,
                    tail
                ),
                true,
            );
        }
        return (raw.to_string(), false);
    };

    prune_az_noise(&mut value);

    let truncated_array = if let Value::Array(arr) = &mut value {
        if arr.len() > MAX_ITEMS {
            let overflow = arr.len() - MAX_ITEMS;
            arr.truncate(MAX_ITEMS);
            Some(overflow)
        } else {
            None
        }
    } else {
        None
    };

    let compact = serde_json::to_string(&value).unwrap_or_else(|_| raw.to_string());

    match truncated_array {
        Some(overflow) => (format!("{}\n... +{} more items", compact, overflow), true),
        None => (compact, false),
    }
}

fn run_generic(subcommand: &str, args: &[String], verbose: u8, full_sub: &str) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("az");
    cmd.arg(subcommand);

    let mut has_output_flag = false;
    for arg in args {
        if arg == "--output" || arg == "-o" || arg.starts_with("--output=") || arg.starts_with("-o=") {
            has_output_flag = true;
        }
        cmd.arg(arg);
    }

    // Inject JSON output for structured operations if not already set
    if !has_output_flag {
        cmd.args(["--output", "json"]);
    }

    if verbose > 0 {
        eprintln!("Running: az {}", full_sub);
    }

    let output = cmd.output().context("Failed to run az CLI")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        timer.track(
            &format!("az {}", full_sub),
            &format!("rtk az {}", full_sub),
            &stderr,
            &stderr,
        );
        eprintln!("{}", stderr.trim());
        return Ok(exit_code_from_output(&output, "az"));
    }

    let (filtered, _truncated) = compress_az_generic(&raw);
    println!("{}", filtered);

    timer.track(
        &format!("az {}", full_sub),
        &format!("rtk az {}", full_sub),
        &raw,
        &filtered,
    );

    Ok(0)
}

// ─── Shared runners ────────────────────────────────────────────────────────

fn run_az_filtered(
    full_args: &[String],
    verbose: u8,
    full_sub: &str,
    filter_fn: fn(&str) -> Option<FilterResult>,
) -> Result<i32> {
    let cmd_label = format!("az {}", full_sub);
    let rtk_label = format!("rtk az {}", full_sub);
    let slug = cmd_label.replace(' ', "_");
    let timer = tracking::TimedExecution::start();

    let (stdout, stderr, status) = run_az_json(full_args, verbose, full_sub)?;

    let raw = if stderr.is_empty() {
        stdout.clone()
    } else {
        format!("{}\n{}", stdout, stderr)
    };

    if !status.success() {
        let exit_code = exit_code_from_status(&status, "az");
        if let Some(hint) = crate::core::tee::tee_and_hint(&raw, &slug, exit_code) {
            eprintln!("{}\n{}", stderr.trim(), hint);
        } else {
            eprintln!("{}", stderr.trim());
        }
        timer.track(&cmd_label, &rtk_label, &raw, &stderr);
        return Ok(exit_code);
    }

    let result = filter_fn(&stdout).unwrap_or_else(|| {
        eprintln!("rtk: filter warning: az filter returned None, passing through raw output");
        FilterResult::new(stdout.clone())
    });

    if result.truncated {
        if let Some(hint) = force_tee_hint(&raw, &slug) {
            println!("{}\n{}", result.text, hint);
        } else {
            println!("{}", result.text);
        }
    } else if let Some(hint) = crate::core::tee::tee_and_hint(&raw, &slug, 0) {
        println!("{}\n{}", result.text, hint);
    } else {
        println!("{}", result.text);
    }

    timer.track(&cmd_label, &rtk_label, &raw, &result.text);
    Ok(exit_code_from_status(&status, "az"))
}

fn run_az_text_filtered(
    full_args: &[String],
    verbose: u8,
    full_sub: &str,
    filter_fn: fn(&str) -> Option<FilterResult>,
) -> Result<i32> {
    let cmd_label = format!("az {}", full_sub);
    let rtk_label = format!("rtk az {}", full_sub);
    let slug = cmd_label.replace(' ', "_");
    let timer = tracking::TimedExecution::start();

    // Run without forcing JSON output — logs can be text or JSON
    let mut cmd = resolved_command("az");
    for arg in full_args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: az {}", full_sub);
    }

    let output = cmd.output().context("Failed to run az CLI")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    let combined = if stderr.is_empty() {
        raw.clone()
    } else {
        format!("{}\n{}", raw, stderr)
    };

    if !output.status.success() {
        timer.track(&cmd_label, &rtk_label, &combined, &stderr);
        eprintln!("{}", stderr.trim());
        return Ok(exit_code_from_output(&output, "az"));
    }

    let result = filter_fn(&raw).unwrap_or_else(|| FilterResult::new(raw.clone()));

    if result.truncated {
        if let Some(hint) = force_tee_hint(&combined, &slug) {
            println!("{}\n{}", result.text, hint);
        } else {
            println!("{}", result.text);
        }
    } else {
        println!("{}", result.text);
    }

    timer.track(&cmd_label, &rtk_label, &combined, &result.text);
    Ok(exit_code_from_output(&output, "az"))
}

fn run_az_json(
    full_args: &[String],
    verbose: u8,
    full_sub: &str,
) -> Result<(String, String, std::process::ExitStatus)> {
    let mut cmd = resolved_command("az");

    // Pass through all args but ensure JSON output
    let mut has_output_flag = false;
    for arg in full_args {
        if arg == "--output" || arg == "-o" || arg.starts_with("--output=") || arg.starts_with("-o=") {
            has_output_flag = true;
        }
        cmd.arg(arg);
    }
    if !has_output_flag {
        cmd.args(["--output", "json"]);
    }

    if verbose > 0 {
        eprintln!("Running: az {}", full_sub);
    }

    let output = cmd
        .output()
        .context(format!("Failed to run az {}", full_sub))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok((stdout, stderr, output.status))
}

// ─── Helpers ───────────────────────────────────────────────────────────────

/// Find the value of a --flag in an args slice
fn find_flag_value(args: &[String], flag: &str) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == flag {
            return iter.next().cloned();
        }
        if let Some(val) = arg.strip_prefix(&format!("{}=", flag)) {
            return Some(val.to_string());
        }
    }
    None
}

/// Build full az args: ["pipelines", "build", "list", ...extra_args]
fn build_az_args(service: &str, sub_args: &[&str], extra_args: &[String]) -> Vec<String> {
    let mut all = vec![service.to_string()];
    for s in sub_args {
        all.push(s.to_string());
    }
    all.extend(extra_args.iter().cloned());
    all
}

/// Prepend a single arg to an args slice
fn prepend_arg(first: &str, rest: &[String]) -> Vec<String> {
    let mut all = vec![first.to_string()];
    all.extend(rest.iter().cloned());
    all
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::utils::count_tokens;

    #[test]
    fn test_filter_build_list_single() {
        let json = r#"[{
            "id": 12345,
            "buildNumber": "2026.0410.1",
            "result": "succeeded",
            "status": "completed",
            "definition": {"name": "MyPipeline"},
            "sourceBranch": "refs/heads/main",
            "reason": "manual",
            "finishTime": "2026-04-10T12:00:00.000Z",
            "startTime": "2026-04-10T11:50:00.000Z"
        }]"#;
        let result = filter_build_list(json).unwrap();
        assert!(result.text.contains("ok 12345 #2026.0410.1"));
        assert!(result.text.contains("MyPipeline"));
        assert!(result.text.contains("succeeded"));
        assert!(result.text.contains("main"));
        assert!(result.text.contains("manual"));
        assert!(!result.truncated);
    }

    #[test]
    fn test_filter_build_list_failed() {
        let json = r#"[{
            "id": 99,
            "buildNumber": "1",
            "result": "failed",
            "status": "completed",
            "definition": {"name": "CI"},
            "sourceBranch": "refs/pull/42/merge",
            "reason": "pullRequest",
            "finishTime": "2026-04-10T12:00:00.000Z",
            "startTime": "2026-04-10T11:50:00.000Z"
        }]"#;
        let result = filter_build_list(json).unwrap();
        assert!(result.text.contains("FAIL 99 #1"));
        assert!(result.text.contains("42/merge"));
        assert!(result.text.contains("pullRequest"));
    }

    #[test]
    fn test_filter_build_list_empty() {
        let json = "[]";
        let result = filter_build_list(json).unwrap();
        assert_eq!(result.text, "No builds found");
    }

    #[test]
    fn test_filter_build_show() {
        let json = r#"{
            "id": 12345,
            "result": "succeeded",
            "status": "completed",
            "definition": {"name": "MyPipeline"},
            "buildNumber": "2026.0410.1",
            "reason": "pullRequest",
            "requestedBy": {"displayName": "John Doe"},
            "sourceBranch": "refs/heads/main",
            "sourceVersion": "abc1234567890abcdef",
            "startTime": "2026-04-10T11:50:00.000Z",
            "finishTime": "2026-04-10T12:00:00.000Z"
        }"#;
        let result = filter_build_show(json).unwrap();
        assert!(result.text.starts_with("ok build 12345 | MyPipeline #2026.0410.1"));
        assert!(result.text.contains("branch: main"));
        assert!(result.text.contains("commit: abc1234"));
        assert!(result.text.contains("reason: pullRequest"));
        assert!(result.text.contains("by: John Doe"));
        assert!(result.text.contains("started: 2026-04-10"));
        assert!(result.text.contains("finished: 2026-04-10"));
        assert!(result.text.contains("(10m)"));
    }

    #[test]
    fn test_filter_timeline_with_failures() {
        let json = r#"{
            "records": [
                {
                    "name": "Build",
                    "type": "Stage",
                    "result": "succeeded",
                    "log": null,
                    "startTime": "2026-04-10T11:50:00.000Z",
                    "finishTime": "2026-04-10T11:52:30.000Z",
                    "errorCount": 0,
                    "warningCount": 0
                },
                {
                    "name": "Run Tests",
                    "type": "Task",
                    "result": "failed",
                    "log": {"id": 42},
                    "startTime": "2026-04-10T11:52:30.000Z",
                    "finishTime": "2026-04-10T11:55:30.000Z",
                    "errorCount": 3,
                    "warningCount": 1,
                    "issues": [{"message": "Process completed with exit code 1"}]
                },
                {
                    "name": "Publish Results",
                    "type": "Task",
                    "result": "failed",
                    "log": {"id": 43},
                    "startTime": "2026-04-10T11:55:30.000Z",
                    "finishTime": "2026-04-10T11:56:00.000Z",
                    "errorCount": 1,
                    "warningCount": 0,
                    "issues": [{"message": "No test results found"}]
                }
            ]
        }"#;
        let result = filter_timeline(json).unwrap();
        assert!(result.text.contains("FAIL timeline: 2 failed of 3 tasks"));
        assert!(result.text.contains("ok Build (Stage)"));
        assert!(result.text.contains("2m30s"));
        assert!(result.text.contains("FAIL Run Tests (Task) logId:42 3err 1warn"));
        assert!(result.text.contains("> Process completed with exit code 1"));
        assert!(result.text.contains("FAIL Publish Results (Task) logId:43 1err"));
        assert!(result.text.contains("> No test results found"));
    }

    #[test]
    fn test_filter_timeline_all_pass() {
        let json = r#"{
            "records": [
                {
                    "name": "Build",
                    "type": "Stage",
                    "result": "succeeded",
                    "log": null,
                    "startTime": "2026-04-10T11:50:00.000Z",
                    "finishTime": "2026-04-10T11:52:00.000Z"
                },
                {
                    "name": "Test",
                    "type": "Task",
                    "result": "succeeded",
                    "log": {"id": 1},
                    "startTime": "2026-04-10T11:52:00.000Z",
                    "finishTime": "2026-04-10T11:52:45.000Z"
                }
            ]
        }"#;
        let result = filter_timeline(json).unwrap();
        assert!(result.text.contains("ok timeline: 0 failed of 2 tasks"));
        assert!(result.text.contains("ok Build (Stage) 2m0s"));
        assert!(result.text.contains("ok Test (Task) 45s"));
    }

    #[test]
    fn test_filter_logs_strips_timestamps() {
        let json = r#"{
            "value": [
                "2026-04-10T12:00:00.1234567Z First line",
                "2026-04-10T12:00:01.1234567Z Second line",
                "2026-04-10T12:00:02.1234567Z Third line"
            ]
        }"#;
        let result = filter_logs(json).unwrap();
        assert!(result.text.contains("First line"));
        assert!(result.text.contains("Second line"));
        assert!(!result.text.contains("2026-04-10T"));
    }

    #[test]
    fn test_filter_logs_list() {
        let json = r#"{
            "value": [
                {"id": 1, "lineCount": 100},
                {"id": 2, "lineCount": 50}
            ]
        }"#;
        let result = filter_logs(json).unwrap();
        assert!(result.text.contains("logId:1 (100 lines)"));
        assert!(result.text.contains("logId:2 (50 lines)"));
    }

    #[test]
    fn test_filter_access_token() {
        let json = r#"{
            "accessToken": "eyJ0eXAiOiJKV1QiLCJhbGci...",
            "expiresOn": "2026-04-10T13:00:00.000Z",
            "subscription": "sub-id",
            "tenant": "tenant-id",
            "tokenType": "Bearer"
        }"#;
        let result = filter_access_token(json).unwrap();
        assert!(result.text.contains("Bearer"));
        assert!(result.text.contains("[eyJ0eXAiOi..."));
        assert!(result.text.contains("sub:sub-id"));
        assert!(result.text.contains("tenant:tenant-id"));
        assert!(result.text.contains("expires:2026-04-10"));
        // Must NOT contain the full token
        assert!(!result.text.contains("eyJ0eXAiOiJKV1QiLCJhbGci..."));
    }

    #[test]
    fn test_find_flag_value() {
        let args: Vec<String> = vec![
            "--resource".into(),
            "timeline".into(),
            "--route-parameters".into(),
            "buildId=123".into(),
        ];
        assert_eq!(find_flag_value(&args, "--resource"), Some("timeline".to_string()));
        assert_eq!(find_flag_value(&args, "--route-parameters"), Some("buildId=123".to_string()));
        assert_eq!(find_flag_value(&args, "--missing"), None);
    }

    #[test]
    fn test_find_flag_value_equals() {
        let args: Vec<String> = vec!["--resource=logs".into()];
        assert_eq!(find_flag_value(&args, "--resource"), Some("logs".to_string()));
    }

    // ─── Token Savings Tests ───────────────────────────────────────────────────

    #[test]
    fn test_build_list_token_savings() {
        let json = r#"[
            {
                "id": 12345,
                "buildNumber": "2026.0410.1",
                "result": "succeeded",
                "status": "completed",
                "definition": {
                    "id": 1,
                    "name": "CI Pipeline",
                    "path": "\\",
                    "project": {"name": "MyProject"},
                    "revision": 5
                },
                "sourceBranch": "refs/heads/main",
                "sourceVersion": "abc1234def5678901234567890123456789012",
                "finishTime": "2026-04-10T12:00:00.000Z",
                "startTime": "2026-04-10T11:50:00.000Z",
                "queueTime": "2026-04-10T11:49:00.000Z",
                "reason": "individualCI",
                "requestedBy": {
                    "displayName": "John Doe",
                    "id": "user-guid-1",
                    "uniqueName": "john@example.com"
                },
                "requestedFor": {
                    "displayName": "John Doe",
                    "id": "user-guid-1",
                    "uniqueName": "john@example.com"
                },
                "repository": {
                    "id": "repo-guid",
                    "name": "my-repo",
                    "type": "TfsGit",
                    "url": "https://dev.azure.com/org/project/_git/my-repo"
                },
                "logs": {
                    "id": 0,
                    "type": "Container",
                    "url": "https://dev.azure.com/org/project/_apis/build/builds/12345/logs"
                },
                "url": "https://dev.azure.com/org/project/_apis/build/Builds/12345",
                "_links": {
                    "self": {"href": "https://dev.azure.com/org/project/_apis/build/Builds/12345"},
                    "web": {"href": "https://dev.azure.com/org/project/_build/results?buildId=12345"},
                    "sourceVersionDisplayUri": {"href": "https://dev.azure.com/org/project/_apis/build/builds/12345/changes"}
                },
                "tags": [],
                "priority": "normal",
                "project": {
                    "id": "proj-guid",
                    "name": "MyProject",
                    "state": "wellFormed",
                    "visibility": "private"
                }
            },
            {
                "id": 12346,
                "buildNumber": "2026.0410.2",
                "result": "failed",
                "status": "completed",
                "definition": {
                    "id": 2,
                    "name": "Nightly Build",
                    "path": "\\",
                    "project": {"name": "MyProject"},
                    "revision": 3
                },
                "sourceBranch": "refs/heads/develop",
                "sourceVersion": "def5678abc1234901234567890123456789012",
                "finishTime": "2026-04-10T13:00:00.000Z",
                "startTime": "2026-04-10T12:30:00.000Z",
                "queueTime": "2026-04-10T12:29:00.000Z",
                "reason": "schedule",
                "requestedBy": {
                    "displayName": "Azure Pipelines",
                    "id": "system-guid",
                    "uniqueName": "system@example.com"
                },
                "requestedFor": {
                    "displayName": "Jane Smith",
                    "id": "user-guid-2",
                    "uniqueName": "jane@example.com"
                },
                "repository": {
                    "id": "repo-guid",
                    "name": "my-repo",
                    "type": "TfsGit",
                    "url": "https://dev.azure.com/org/project/_git/my-repo"
                },
                "logs": {
                    "id": 0,
                    "type": "Container",
                    "url": "https://dev.azure.com/org/project/_apis/build/builds/12346/logs"
                },
                "url": "https://dev.azure.com/org/project/_apis/build/Builds/12346",
                "_links": {
                    "self": {"href": "https://dev.azure.com/org/project/_apis/build/Builds/12346"},
                    "web": {"href": "https://dev.azure.com/org/project/_build/results?buildId=12346"},
                    "sourceVersionDisplayUri": {"href": "https://dev.azure.com/org/project/_apis/build/builds/12346/changes"}
                },
                "tags": [],
                "priority": "normal",
                "project": {
                    "id": "proj-guid",
                    "name": "MyProject",
                    "state": "wellFormed",
                    "visibility": "private"
                }
            },
            {
                "id": 12347,
                "buildNumber": "2026.0410.3",
                "result": "canceled",
                "status": "completed",
                "definition": {
                    "id": 1,
                    "name": "CI Pipeline",
                    "path": "\\",
                    "project": {"name": "MyProject"},
                    "revision": 5
                },
                "sourceBranch": "refs/pull/42/merge",
                "sourceVersion": "ghi9012jkl3456789012345678901234567890",
                "finishTime": "2026-04-10T14:00:00.000Z",
                "startTime": "2026-04-10T13:50:00.000Z",
                "queueTime": "2026-04-10T13:49:00.000Z",
                "reason": "pullRequest",
                "requestedBy": {
                    "displayName": "Bob Wilson",
                    "id": "user-guid-3",
                    "uniqueName": "bob@example.com"
                },
                "requestedFor": {
                    "displayName": "Bob Wilson",
                    "id": "user-guid-3",
                    "uniqueName": "bob@example.com"
                },
                "repository": {
                    "id": "repo-guid",
                    "name": "my-repo",
                    "type": "TfsGit",
                    "url": "https://dev.azure.com/org/project/_git/my-repo"
                },
                "logs": {
                    "id": 0,
                    "type": "Container",
                    "url": "https://dev.azure.com/org/project/_apis/build/builds/12347/logs"
                },
                "url": "https://dev.azure.com/org/project/_apis/build/Builds/12347",
                "_links": {
                    "self": {"href": "https://dev.azure.com/org/project/_apis/build/Builds/12347"},
                    "web": {"href": "https://dev.azure.com/org/project/_build/results?buildId=12347"},
                    "sourceVersionDisplayUri": {"href": "https://dev.azure.com/org/project/_apis/build/builds/12347/changes"}
                },
                "tags": ["pr-build"],
                "priority": "normal",
                "project": {
                    "id": "proj-guid",
                    "name": "MyProject",
                    "state": "wellFormed",
                    "visibility": "private"
                }
            }
        ]"#;

        let result = filter_build_list(json).unwrap();
        let input_tokens = count_tokens(json);
        let output_tokens = count_tokens(&result.text);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "build_list filter: expected ≥60% savings, got {:.1}%",
            savings
        );

        // Also verify output is not empty and contains key info
        assert!(!result.text.is_empty());
        assert!(result.text.contains("12345"));
        assert!(result.text.contains("CI Pipeline"));
    }

    #[test]
    fn test_build_show_token_savings() {
        let json = r#"{
            "id": 12345,
            "buildNumber": "2026.0410.1",
            "result": "succeeded",
            "status": "completed",
            "definition": {
                "id": 1,
                "name": "CI Pipeline",
                "path": "\\",
                "project": {"name": "MyProject"},
                "revision": 5
            },
            "sourceBranch": "refs/heads/main",
            "sourceVersion": "abc1234def5678901234567890123456789012",
            "finishTime": "2026-04-10T12:00:00.000Z",
            "startTime": "2026-04-10T11:50:00.000Z",
            "queueTime": "2026-04-10T11:49:00.000Z",
            "reason": "individualCI",
            "requestedBy": {
                "displayName": "John Doe",
                "id": "user-guid-1",
                "uniqueName": "john@example.com"
            },
            "requestedFor": {
                "displayName": "John Doe",
                "id": "user-guid-1",
                "uniqueName": "john@example.com"
            },
            "repository": {
                "id": "repo-guid",
                "name": "my-repo",
                "type": "TfsGit",
                "url": "https://dev.azure.com/org/project/_git/my-repo"
            },
            "logs": {
                "id": 0,
                "type": "Container",
                "url": "https://dev.azure.com/org/project/_apis/build/builds/12345/logs"
            },
            "url": "https://dev.azure.com/org/project/_apis/build/Builds/12345",
            "_links": {
                "self": {"href": "https://dev.azure.com/org/project/_apis/build/Builds/12345"},
                "web": {"href": "https://dev.azure.com/org/project/_build/results?buildId=12345"},
                "sourceVersionDisplayUri": {"href": "https://dev.azure.com/org/project/_apis/build/builds/12345/changes"}
            },
            "tags": [],
            "priority": "normal",
            "project": {
                "id": "proj-guid",
                "name": "MyProject",
                "state": "wellFormed",
                "visibility": "private"
            }
        }"#;

        let result = filter_build_show(json).unwrap();
        let input_tokens = count_tokens(json);
        let output_tokens = count_tokens(&result.text);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "build_show filter: expected ≥60% savings, got {:.1}%",
            savings
        );

        // Also verify output format
        assert!(result.text.contains("ok build 12345"));
        assert!(result.text.contains("CI Pipeline"));
    }

    #[test]
    fn test_timeline_token_savings() {
        let json = r#"{
            "records": [
                {
                    "id": "guid-1",
                    "name": "Initialize",
                    "type": "Task",
                    "result": "succeeded",
                    "state": "completed",
                    "order": 1,
                    "startTime": "2026-04-10T11:50:00.000Z",
                    "finishTime": "2026-04-10T11:50:30.000Z",
                    "log": {"id": 1, "url": "https://dev.azure.com/org/project/_apis/build/builds/12345/logs/1"},
                    "errorCount": 0,
                    "warningCount": 0,
                    "workerName": "agent-1",
                    "percentComplete": 100,
                    "issues": [],
                    "_links": {
                        "self": {"href": "https://dev.azure.com/org/project/_apis/build/builds/12345/Timeline/guid-1"},
                        "log": {"href": "https://dev.azure.com/org/project/_apis/build/builds/12345/logs/1"}
                    },
                    "previousAttempts": [],
                    "parentId": "guid-0"
                },
                {
                    "id": "guid-2",
                    "name": "Build",
                    "type": "Task",
                    "result": "failed",
                    "state": "completed",
                    "order": 2,
                    "startTime": "2026-04-10T11:50:30.000Z",
                    "finishTime": "2026-04-10T11:52:00.000Z",
                    "log": {"id": 2, "url": "https://dev.azure.com/org/project/_apis/build/builds/12345/logs/2"},
                    "errorCount": 5,
                    "warningCount": 2,
                    "workerName": "agent-1",
                    "percentComplete": 100,
                    "issues": [
                        {
                            "type": "error",
                            "message": "Build failed with exit code 1",
                            "data": {"sourcepath": "/src/main.rs"}
                        },
                        {
                            "type": "error",
                            "message": "Compilation errors found"
                        }
                    ],
                    "_links": {
                        "self": {"href": "https://dev.azure.com/org/project/_apis/build/builds/12345/Timeline/guid-2"},
                        "log": {"href": "https://dev.azure.com/org/project/_apis/build/builds/12345/logs/2"}
                    },
                    "previousAttempts": [],
                    "parentId": "guid-0"
                },
                {
                    "id": "guid-3",
                    "name": "Test",
                    "type": "Task",
                    "result": "succeeded",
                    "state": "completed",
                    "order": 3,
                    "startTime": "2026-04-10T11:52:00.000Z",
                    "finishTime": "2026-04-10T11:54:00.000Z",
                    "log": {"id": 3, "url": "https://dev.azure.com/org/project/_apis/build/builds/12345/logs/3"},
                    "errorCount": 0,
                    "warningCount": 0,
                    "workerName": "agent-1",
                    "percentComplete": 100,
                    "issues": [],
                    "_links": {
                        "self": {"href": "https://dev.azure.com/org/project/_apis/build/builds/12345/Timeline/guid-3"},
                        "log": {"href": "https://dev.azure.com/org/project/_apis/build/builds/12345/logs/3"}
                    },
                    "previousAttempts": [],
                    "parentId": "guid-0"
                },
                {
                    "id": "guid-4",
                    "name": "Publish",
                    "type": "Task",
                    "result": "failed",
                    "state": "completed",
                    "order": 4,
                    "startTime": "2026-04-10T11:54:00.000Z",
                    "finishTime": "2026-04-10T11:54:10.000Z",
                    "log": {"id": 4, "url": "https://dev.azure.com/org/project/_apis/build/builds/12345/logs/4"},
                    "errorCount": 1,
                    "warningCount": 0,
                    "workerName": "agent-1",
                    "percentComplete": 100,
                    "issues": [
                        {
                            "type": "error",
                            "message": "No artifacts found to publish"
                        }
                    ],
                    "_links": {
                        "self": {"href": "https://dev.azure.com/org/project/_apis/build/builds/12345/Timeline/guid-4"},
                        "log": {"href": "https://dev.azure.com/org/project/_apis/build/builds/12345/logs/4"}
                    },
                    "previousAttempts": [],
                    "parentId": "guid-0"
                },
                {
                    "id": "guid-5",
                    "name": "Deploy",
                    "type": "Stage",
                    "result": "succeeded",
                    "state": "completed",
                    "order": 5,
                    "startTime": "2026-04-10T11:54:10.000Z",
                    "finishTime": "2026-04-10T11:56:00.000Z",
                    "log": null,
                    "errorCount": 0,
                    "warningCount": 0,
                    "workerName": "agent-1",
                    "percentComplete": 100,
                    "issues": [],
                    "_links": {
                        "self": {"href": "https://dev.azure.com/org/project/_apis/build/builds/12345/Timeline/guid-5"}
                    },
                    "previousAttempts": [],
                    "parentId": "guid-0"
                }
            ]
        }"#;

        let result = filter_timeline(json).unwrap();
        let input_tokens = count_tokens(json);
        let output_tokens = count_tokens(&result.text);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "timeline filter: expected ≥60% savings, got {:.1}%",
            savings
        );

        // Verify output contains key info
        assert!(result.text.contains("FAIL timeline: 2 failed of 5 tasks"));
        assert!(result.text.contains("FAIL Build"));
        assert!(result.text.contains("FAIL Publish"));
    }

    #[test]
    fn test_logs_token_savings() {
        // Fixture large enough to exercise the default 200-line tail. 500
        // lines is close to the real AutoMiner `Run Integration Tests` log
        // (~795 lines) so the measured savings reflect production reality.
        let mut log_lines = Vec::new();
        for i in 0..500 {
            log_lines.push(format!(
                "\"2026-04-10T11:{:02}:{:02}.0000000Z ##[section]Log line {} with some typical build output content that goes here\"",
                i / 60, i % 60, i + 1
            ));
        }
        let json = format!(r#"{{ "value": [{}] }}"#, log_lines.join(",\n        "));

        let result = filter_logs(&json).unwrap();
        let input_tokens = count_tokens(&json);
        let output_tokens = count_tokens(&result.text);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        // Logs savings are lower than structured filters because content is
        // preserved (timestamps stripped + tail truncated to LOG_TAIL_LINES).
        assert!(
            savings >= 40.0,
            "logs filter: expected ≥40% savings, got {:.1}%",
            savings
        );

        // Verify truncation header + timestamp removal at the new default.
        assert!(result.text.contains("(500 lines total, showing last 200)"));
        assert!(!result.text.contains("2026-04-10T"));
    }

    #[test]
    fn test_access_token_token_savings() {
        let long_token = "eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiIsIng1dCI6InEtTWpOVjN1YnF0bHFLZzFNdGRDVjVIdW1qYyIsImtpZCI6InEtTWpOVjN1YnF0bHFLZzFNdGRDVjVIdW1qYyJ9eyJhdWQiOiJodHRwczovL21hbmFnZW1lbnQuY29yZS53aW5kb3dzLm5ldC8iLCJpc3MiOiJodHRwczovL3N0cy53aW5kb3dzLm5ldC8xMjM0NTY3OC0xMjM0LTEyMzQtMTIzNC0xMjM0NTY3ODkwYWIvIiwiaWF0IjoxNzEwMDAwMDAwLCJuYmYiOjE3MTAwMDAwMDAsImV4cCI6MTcxMDAwMzYwMCwiYWlvIjoiRTJaZ1lIanIxWVgvMy90cC82bjlPZnovQitHQUE9PSIsImFwcGlkIjoiYXBwaWQtZ3VpZCIsImFwcGlkYWNyIjoiMSIsImlkcCI6Imh0dHBzOi8vc3RzLndpbmRvd3MubmV0L3RlbmFudC1ndWlkLyIsIm9pZCI6Im9pZC1ndWlkIiwicmgiOiIwLkFYQUFsNlFfb1hlZ0JrZUcxX0lULWdoalFGVEFBQUFBQUFBQXdBQUFBQUFBQUFBQUFBQS4iLCJzdWIiOiJzdWItZ3VpZCIsInRpZCI6ImUzZjNhNDk3LWE3NzctNDg2Ny04NmQ3ZjIxM2ZhMDg2MyIsInV0aSI6ImI3RGtCakdLT2s2OWhUMGozRTNmQUEiLCJ2ZXIiOiIxLjAiLCJ4bXNfdGNkdCI6MTYzOTI0NTk5OH0=";

        let json = format!(r#"{{
            "accessToken": "{}",
            "expiresOn": "2026-04-10T13:00:00.000000+00:00",
            "expires_on": 1781362800,
            "subscription": "12345678-1234-1234-1234-123456789abc",
            "tenant": "abcdef01-2345-6789-abcd-ef0123456789",
            "tokenType": "Bearer",
            "environmentName": "AzureCloud"
        }}"#, long_token);

        let result = filter_access_token(&json).unwrap();
        let input_tokens = count_tokens(&json);
        let output_tokens = count_tokens(&result.text);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        // Token filter's primary value is security (masking), not maximum savings
        // Token is masked to ~10 chars, but other metadata is preserved
        assert!(
            savings >= 40.0,
            "access_token filter: expected ≥40% savings, got {:.1}%",
            savings
        );

        // Verify token is masked
        assert!(result.text.contains("Bearer"));
        assert!(!result.text.contains(long_token));
    }

    // ─── Edge Case Tests ────────────────────────────────────────────────────────

    #[test]
    fn test_build_list_partially_succeeded() {
        let json = r#"[{
            "id": 12345,
            "buildNumber": "2026.0410.1",
            "result": "partiallySucceeded",
            "status": "completed",
            "definition": {"name": "MyPipeline"},
            "sourceBranch": "refs/heads/main",
            "reason": "manual",
            "finishTime": "2026-04-10T12:00:00.000Z"
        }]"#;
        let result = filter_build_list(json).unwrap();
        // partiallySucceeded falls through to "?" icon
        assert!(result.text.contains("? 12345"));
        assert!(result.text.contains("partiallySucceeded"));
    }

    #[test]
    fn test_timeline_failed_no_issues() {
        let json = r#"{
            "records": [
                {
                    "name": "Build",
                    "type": "Task",
                    "result": "failed",
                    "log": {"id": 42},
                    "startTime": "2026-04-10T11:50:00.000Z",
                    "finishTime": "2026-04-10T11:52:00.000Z",
                    "errorCount": 0,
                    "warningCount": 0
                }
            ]
        }"#;
        // Should NOT crash even without issues array
        let result = filter_timeline(json).unwrap();
        assert!(result.text.contains("FAIL Build (Task) logId:42"));
        assert!(!result.text.contains(">"));
    }

    #[test]
    fn test_timeline_failed_empty_issues() {
        let json = r#"{
            "records": [
                {
                    "name": "Build",
                    "type": "Task",
                    "result": "failed",
                    "log": {"id": 42},
                    "startTime": "2026-04-10T11:50:00.000Z",
                    "finishTime": "2026-04-10T11:52:00.000Z",
                    "errorCount": 0,
                    "warningCount": 0,
                    "issues": []
                }
            ]
        }"#;
        // Should NOT crash with empty issues array
        let result = filter_timeline(json).unwrap();
        assert!(result.text.contains("FAIL Build (Task) logId:42"));
        assert!(!result.text.contains(">"));
    }

    #[test]
    fn test_build_show_unicode() {
        let json = r#"{
            "id": 12345,
            "result": "succeeded",
            "status": "completed",
            "definition": {"name": "MyPipeline"},
            "buildNumber": "2026.0410.1",
            "reason": "pullRequest",
            "requestedBy": {"displayName": "田中太郎"},
            "sourceBranch": "refs/heads/feature/日本語",
            "sourceVersion": "abc1234567890abcdef",
            "startTime": "2026-04-10T11:50:00.000Z",
            "finishTime": "2026-04-10T12:00:00.000Z"
        }"#;
        let result = filter_build_show(json).unwrap();
        // Must not crash or corrupt output
        assert!(result.text.contains("田中太郎"));
        assert!(result.text.contains("feature/日本語"));
    }

    #[test]
    fn test_build_show_null_fields() {
        // Build JSON where key fields are missing entirely
        let json = r#"{
            "id": 12345,
            "result": "succeeded",
            "status": "completed",
            "definition": {"name": "MyPipeline"}
        }"#;
        let result = filter_build_show(json).unwrap();
        // All missing fields should gracefully fall back to "?"
        assert!(result.text.contains("ok build 12345"));
        assert!(result.text.contains("MyPipeline #?"));
        assert!(result.text.contains("branch: ?"));
        assert!(result.text.contains("commit: ?"));
        assert!(result.text.contains("reason: ?"));
        assert!(result.text.contains("by: ?"));
    }

    #[test]
    fn test_build_list_truncation() {
        // Create 25 builds (more than MAX_ITEMS=20)
        let mut builds = Vec::new();
        for i in 1..=25 {
            builds.push(format!(
                r#"{{
                    "id": {},
                    "buildNumber": "2026.0410.{}",
                    "result": "succeeded",
                    "status": "completed",
                    "definition": {{"name": "Pipeline"}},
                    "sourceBranch": "refs/heads/main",
                    "reason": "manual",
                    "finishTime": "2026-04-10T12:00:00.000Z"
                }}"#,
                i, i
            ));
        }
        let json = format!("[{}]", builds.join(",\n"));

        let result = filter_build_list(&json).unwrap();
        // Should be truncated and show +5 more
        assert!(result.truncated);
        assert!(result.text.contains("... +5 more builds"));
    }

    #[test]
    fn test_logs_exact_tail() {
        // 50 lines is well under the default tail — must not truncate.
        let mut log_lines = Vec::new();
        for i in 0..50 {
            log_lines.push(format!(
                "\"2026-04-10T11:{:02}:{:02}.0000000Z Log line {}\"",
                i / 60, i % 60, i + 1
            ));
        }
        let json = format!(r#"{{ "value": [{}] }}"#, log_lines.join(",\n"));

        let result = filter_logs(&json).unwrap();
        assert!(!result.truncated);
        assert!(!result.text.contains("lines total"));
        assert!(result.text.contains("Log line 1"));
        assert!(result.text.contains("Log line 50"));
    }

    // ─── G1: LOG_TAIL_LINES default + RTK_AZ_LOG_TAIL env override ───────────

    #[test]
    fn test_parse_log_tail_defaults_when_unset() {
        assert_eq!(parse_log_tail(None), LOG_TAIL_LINES);
    }

    #[test]
    fn test_parse_log_tail_accepts_valid_value() {
        assert_eq!(parse_log_tail(Some("75")), 75);
        assert_eq!(parse_log_tail(Some("1")), 1);
        assert_eq!(parse_log_tail(Some("10000")), 10000);
    }

    #[test]
    fn test_parse_log_tail_rejects_zero() {
        // Zero would silently drop all log output — fall back to default.
        assert_eq!(parse_log_tail(Some("0")), LOG_TAIL_LINES);
    }

    #[test]
    fn test_parse_log_tail_rejects_invalid() {
        assert_eq!(parse_log_tail(Some("")), LOG_TAIL_LINES);
        assert_eq!(parse_log_tail(Some("abc")), LOG_TAIL_LINES);
        assert_eq!(parse_log_tail(Some("-1")), LOG_TAIL_LINES);
        assert_eq!(parse_log_tail(Some("3.14")), LOG_TAIL_LINES);
    }

    #[test]
    fn test_logs_default_tail_is_200() {
        // 250 lines → tail at default 200, assertion reason in middle preserved.
        let mut log_lines = Vec::new();
        for i in 0..250 {
            log_lines.push(format!(
                "\"2026-04-10T11:00:00.0000000Z Log line {}\"",
                i + 1
            ));
        }
        let json = format!(r#"{{ "value": [{}] }}"#, log_lines.join(","));

        let result = filter_logs(&json).unwrap();
        assert!(result.truncated);
        assert!(result.text.contains("250 lines total, showing last 200"));
        // Line 50 is outside tail → must be absent; line 51 is at the boundary.
        assert!(!result.text.contains("Log line 50\n"));
        // Lines 51..=250 should all be present.
        assert!(result.text.contains("Log line 51"));
        assert!(result.text.contains("Log line 250"));
    }

    // ─── G5: filter_timeline must preserve log.id for every failing record ───

    #[test]
    fn test_filter_timeline_preserves_log_id_per_failing_record() {
        // Three failures with distinct log ids + one failure with log:null.
        // Each logId must round-trip into the filtered output.
        let json = r#"{
            "records": [
                {"name":"Build","type":"Stage","result":"failed","log":null,"issues":[]},
                {"name":"Run Integration Tests","type":"Task","result":"failed","log":{"id":32},"issues":[{"message":"AssertFailed"}]},
                {"name":"Publish Integration Test Results","type":"Task","result":"failed","log":{"id":36},"issues":[]},
                {"name":"Build (Linux)","type":"Job","result":"failed","log":{"id":43},"issues":[]},
                {"name":"Cache AI Evals","type":"Task","result":"succeeded","log":{"id":99}}
            ]
        }"#;

        let result = filter_timeline(json).unwrap();
        let text = &result.text;

        // Every failing record's log id must appear in the output.
        for id in [32, 36, 43] {
            assert!(
                text.contains(&format!("logId:{}", id)),
                "filter_timeline dropped logId:{} — drill-down to `invoke --resource logs --logId {}` would fail silently.\nOutput:\n{}",
                id, id, text
            );
        }

        // Null-log failing record must render as `logId:-` (not a real id).
        assert!(
            text.contains("logId:-"),
            "null log on failing Stage record must render as `logId:-`, got:\n{}",
            text
        );

        // Succeeded record's log id (99) should NOT leak — drill-down is only
        // meaningful for failures, and including it would bloat the summary.
        assert!(
            !text.contains("logId:99"),
            "log id leaked from a succeeded record — summary should stay focused on failures"
        );
    }

    // ─── G4: elide oversized embedded payloads (>=1024 bytes) in log lines ────

    #[test]
    fn test_filter_logs_elides_oversized_embedded_payload() {
        // 2 KB JSON blob stuffed inside an Azure ENDPOINT_DATA env var line
        // (this is the real shape observed in AutoMiner build 298285, log 32).
        let big_json: String = std::iter::repeat('a').take(2048).collect();
        let big_line = format!(
            "2026-04-17T17:27:09.4953295Z ENDPOINT_DATA_debac320={{\"environment\":\"AzureCloud\",\"blob\":\"{}\"}}",
            big_json
        );
        let small_line = "2026-04-17T17:27:09.5000000Z Normal log line".to_string();

        let json = format!(
            r#"{{"value":[{},{}]}}"#,
            serde_json::to_string(&big_line).unwrap(),
            serde_json::to_string(&small_line).unwrap(),
        );

        let result = filter_logs(&json).unwrap();
        // Oversized payload elision: the raw JSON contents must NOT leak, but
        // the kind + byte-count marker should be present so the reader knows
        // what was dropped.
        assert!(
            !result.text.contains(&big_json),
            "oversized embedded JSON must be elided, but raw blob still present"
        );
        assert!(
            result.text.contains("<json elided")
                && result.text.contains("bytes="),
            "elision marker missing from output:\n{}",
            result.text
        );
        // Normal line must pass through untouched.
        assert!(result.text.contains("Normal log line"));
    }

    #[test]
    fn test_access_token_missing_fields() {
        // Token JSON with only accessToken — no other fields
        let json = r#"{"accessToken": "eyJ0eXAiOiJKV1QiLCJhbGci..."}"#;
        let result = filter_access_token(json).unwrap();
        // Should show "?" for missing fields
        assert!(result.text.contains("Bearer")); // default tokenType
        assert!(result.text.contains("sub:?"));
        assert!(result.text.contains("tenant:?"));
        assert!(result.text.contains("expires:?"));
    }

    #[test]
    fn test_all_filters_invalid_json() {
        let invalid = "not valid json";

        // All filters should return None (not panic)
        assert!(filter_build_list(invalid).is_none());
        assert!(filter_build_show(invalid).is_none());
        assert!(filter_timeline(invalid).is_none());
        assert!(filter_logs(invalid).is_none());
        assert!(filter_access_token(invalid).is_none());
        assert!(filter_runs_list(invalid).is_none());
        assert!(filter_runs_show(invalid).is_none());
    }

    // ─── az pipelines runs Tests ──────────────────────────────────────────────

    #[test]
    fn test_filter_runs_list_single() {
        let json = r#"[{
            "id": 12345,
            "buildNumber": "2026.0410.1",
            "result": "succeeded",
            "status": "completed",
            "definition": {"name": "MyPipeline"},
            "sourceBranch": "refs/heads/main",
            "reason": "manual",
            "finishTime": "2026-04-10T12:00:00.000Z",
            "startTime": "2026-04-10T11:50:00.000Z"
        }]"#;
        let result = filter_runs_list(json).unwrap();
        assert!(result.text.contains("ok 12345 #2026.0410.1"));
        assert!(result.text.contains("MyPipeline"));
        assert!(result.text.contains("succeeded"));
        assert!(result.text.contains("main"));
        assert!(result.text.contains("manual"));
        assert!(!result.truncated);
    }

    #[test]
    fn test_filter_runs_list_failed() {
        let json = r#"[{
            "id": 99,
            "buildNumber": "1",
            "result": "failed",
            "status": "completed",
            "definition": {"name": "CI"},
            "sourceBranch": "refs/pull/42/merge",
            "reason": "pullRequest",
            "finishTime": "2026-04-10T12:00:00.000Z",
            "startTime": "2026-04-10T11:50:00.000Z"
        }]"#;
        let result = filter_runs_list(json).unwrap();
        assert!(result.text.contains("FAIL 99 #1"));
        assert!(result.text.contains("42/merge"));
        assert!(result.text.contains("pullRequest"));
    }

    #[test]
    fn test_filter_runs_list_empty() {
        let json = "[]";
        let result = filter_runs_list(json).unwrap();
        assert_eq!(result.text, "No runs found");
    }

    #[test]
    fn test_filter_runs_list_truncation() {
        // Create 25 runs (more than MAX_ITEMS=20)
        let mut runs = Vec::new();
        for i in 1..=25 {
            runs.push(format!(
                r#"{{
                    "id": {},
                    "buildNumber": "2026.0410.{}",
                    "result": "succeeded",
                    "status": "completed",
                    "definition": {{"name": "Pipeline"}},
                    "sourceBranch": "refs/heads/main",
                    "reason": "manual",
                    "finishTime": "2026-04-10T12:00:00.000Z"
                }}"#,
                i, i
            ));
        }
        let json = format!("[{}]", runs.join(",\n"));

        let result = filter_runs_list(&json).unwrap();
        // Should be truncated and show +5 more
        assert!(result.truncated);
        assert!(result.text.contains("... +5 more runs"));
    }

    #[test]
    fn test_filter_runs_show() {
        let json = r#"{
            "id": 12345,
            "result": "succeeded",
            "status": "completed",
            "definition": {"name": "MyPipeline"},
            "buildNumber": "2026.0410.1",
            "reason": "pullRequest",
            "requestedBy": {"displayName": "John Doe"},
            "sourceBranch": "refs/heads/main",
            "sourceVersion": "abc1234567890abcdef",
            "startTime": "2026-04-10T11:50:00.000Z",
            "finishTime": "2026-04-10T12:00:00.000Z"
        }"#;
        let result = filter_runs_show(json).unwrap();
        assert!(result.text.starts_with("ok run 12345 | MyPipeline #2026.0410.1"));
        assert!(result.text.contains("branch: main"));
        assert!(result.text.contains("commit: abc1234"));
        assert!(result.text.contains("reason: pullRequest"));
        assert!(result.text.contains("by: John Doe"));
        assert!(result.text.contains("started: 2026-04-10"));
        assert!(result.text.contains("finished: 2026-04-10"));
        assert!(result.text.contains("(10m)"));
    }

    #[test]
    fn test_filter_runs_show_null_fields() {
        // Runs JSON where key fields are missing entirely
        let json = r#"{
            "id": 12345,
            "result": "succeeded",
            "status": "completed",
            "definition": {"name": "MyPipeline"}
        }"#;
        let result = filter_runs_show(json).unwrap();
        // All missing fields should gracefully fall back to "?"
        assert!(result.text.contains("ok run 12345"));
        assert!(result.text.contains("MyPipeline #?"));
        assert!(result.text.contains("branch: ?"));
        assert!(result.text.contains("commit: ?"));
        assert!(result.text.contains("reason: ?"));
        assert!(result.text.contains("by: ?"));
    }

    #[test]
    fn test_filter_runs_show_unicode() {
        let json = r#"{
            "id": 12345,
            "result": "succeeded",
            "status": "completed",
            "definition": {"name": "MyPipeline"},
            "buildNumber": "2026.0410.1",
            "reason": "pullRequest",
            "requestedBy": {"displayName": "田中太郎"},
            "sourceBranch": "refs/heads/feature/日本語",
            "sourceVersion": "abc1234567890abcdef",
            "startTime": "2026-04-10T11:50:00.000Z",
            "finishTime": "2026-04-10T12:00:00.000Z"
        }"#;
        let result = filter_runs_show(json).unwrap();
        // Must not crash or corrupt output
        assert!(result.text.contains("田中太郎"));
        assert!(result.text.contains("feature/日本語"));
    }

    #[test]
    fn test_runs_list_token_savings() {
        let json = r#"[
            {
                "id": 12345,
                "buildNumber": "2026.0410.1",
                "result": "succeeded",
                "status": "completed",
                "definition": {
                    "id": 1,
                    "name": "CI Pipeline",
                    "path": "\\",
                    "project": {"name": "MyProject"},
                    "revision": 5
                },
                "sourceBranch": "refs/heads/main",
                "sourceVersion": "abc1234def5678901234567890123456789012",
                "finishTime": "2026-04-10T12:00:00.000Z",
                "startTime": "2026-04-10T11:50:00.000Z",
                "queueTime": "2026-04-10T11:49:00.000Z",
                "reason": "individualCI",
                "requestedBy": {
                    "displayName": "John Doe",
                    "id": "user-guid-1",
                    "uniqueName": "john@example.com"
                },
                "requestedFor": {
                    "displayName": "John Doe",
                    "id": "user-guid-1",
                    "uniqueName": "john@example.com"
                },
                "repository": {
                    "id": "repo-guid",
                    "name": "my-repo",
                    "type": "TfsGit",
                    "url": "https://dev.azure.com/org/project/_git/my-repo"
                },
                "logs": {
                    "id": 0,
                    "type": "Container",
                    "url": "https://dev.azure.com/org/project/_apis/build/builds/12345/logs"
                },
                "url": "https://dev.azure.com/org/project/_apis/build/Builds/12345",
                "_links": {
                    "self": {"href": "https://dev.azure.com/org/project/_apis/build/Builds/12345"},
                    "web": {"href": "https://dev.azure.com/org/project/_build/results?buildId=12345"},
                    "sourceVersionDisplayUri": {"href": "https://dev.azure.com/org/project/_apis/build/builds/12345/changes"}
                },
                "tags": [],
                "priority": "normal",
                "project": {
                    "id": "proj-guid",
                    "name": "MyProject",
                    "state": "wellFormed",
                    "visibility": "private"
                }
            },
            {
                "id": 12346,
                "buildNumber": "2026.0410.2",
                "result": "failed",
                "status": "completed",
                "definition": {
                    "id": 2,
                    "name": "Nightly Build",
                    "path": "\\",
                    "project": {"name": "MyProject"},
                    "revision": 3
                },
                "sourceBranch": "refs/heads/develop",
                "sourceVersion": "def5678abc1234901234567890123456789012",
                "finishTime": "2026-04-10T13:00:00.000Z",
                "startTime": "2026-04-10T12:30:00.000Z",
                "queueTime": "2026-04-10T12:29:00.000Z",
                "reason": "schedule",
                "requestedBy": {
                    "displayName": "Azure Pipelines",
                    "id": "system-guid",
                    "uniqueName": "system@example.com"
                },
                "requestedFor": {
                    "displayName": "Jane Smith",
                    "id": "user-guid-2",
                    "uniqueName": "jane@example.com"
                },
                "repository": {
                    "id": "repo-guid",
                    "name": "my-repo",
                    "type": "TfsGit",
                    "url": "https://dev.azure.com/org/project/_git/my-repo"
                },
                "logs": {
                    "id": 0,
                    "type": "Container",
                    "url": "https://dev.azure.com/org/project/_apis/build/builds/12346/logs"
                },
                "url": "https://dev.azure.com/org/project/_apis/build/Builds/12346",
                "_links": {
                    "self": {"href": "https://dev.azure.com/org/project/_apis/build/Builds/12346"},
                    "web": {"href": "https://dev.azure.com/org/project/_build/results?buildId=12346"},
                    "sourceVersionDisplayUri": {"href": "https://dev.azure.com/org/project/_apis/build/builds/12346/changes"}
                },
                "tags": [],
                "priority": "normal",
                "project": {
                    "id": "proj-guid",
                    "name": "MyProject",
                    "state": "wellFormed",
                    "visibility": "private"
                }
            },
            {
                "id": 12347,
                "buildNumber": "2026.0410.3",
                "result": "canceled",
                "status": "completed",
                "definition": {
                    "id": 1,
                    "name": "CI Pipeline",
                    "path": "\\",
                    "project": {"name": "MyProject"},
                    "revision": 5
                },
                "sourceBranch": "refs/pull/42/merge",
                "sourceVersion": "ghi9012jkl3456789012345678901234567890",
                "finishTime": "2026-04-10T14:00:00.000Z",
                "startTime": "2026-04-10T13:50:00.000Z",
                "queueTime": "2026-04-10T13:49:00.000Z",
                "reason": "pullRequest",
                "requestedBy": {
                    "displayName": "Bob Wilson",
                    "id": "user-guid-3",
                    "uniqueName": "bob@example.com"
                },
                "requestedFor": {
                    "displayName": "Bob Wilson",
                    "id": "user-guid-3",
                    "uniqueName": "bob@example.com"
                },
                "repository": {
                    "id": "repo-guid",
                    "name": "my-repo",
                    "type": "TfsGit",
                    "url": "https://dev.azure.com/org/project/_git/my-repo"
                },
                "logs": {
                    "id": 0,
                    "type": "Container",
                    "url": "https://dev.azure.com/org/project/_apis/build/builds/12347/logs"
                },
                "url": "https://dev.azure.com/org/project/_apis/build/Builds/12347",
                "_links": {
                    "self": {"href": "https://dev.azure.com/org/project/_apis/build/Builds/12347"},
                    "web": {"href": "https://dev.azure.com/org/project/_build/results?buildId=12347"},
                    "sourceVersionDisplayUri": {"href": "https://dev.azure.com/org/project/_apis/build/builds/12347/changes"}
                },
                "tags": ["pr-build"],
                "priority": "normal",
                "project": {
                    "id": "proj-guid",
                    "name": "MyProject",
                    "state": "wellFormed",
                    "visibility": "private"
                }
            }
        ]"#;

        let result = filter_runs_list(json).unwrap();
        let input_tokens = count_tokens(json);
        let output_tokens = count_tokens(&result.text);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "runs_list filter: expected ≥60% savings, got {:.1}%",
            savings
        );

        // Also verify output is not empty and contains key info
        assert!(!result.text.is_empty());
        assert!(result.text.contains("12345"));
        assert!(result.text.contains("CI Pipeline"));
    }

    #[test]
    fn test_runs_show_token_savings() {
        let json = r#"{
            "id": 12345,
            "buildNumber": "2026.0410.1",
            "result": "succeeded",
            "status": "completed",
            "definition": {
                "id": 1,
                "name": "CI Pipeline",
                "path": "\\",
                "project": {"name": "MyProject"},
                "revision": 5
            },
            "sourceBranch": "refs/heads/main",
            "sourceVersion": "abc1234def5678901234567890123456789012",
            "finishTime": "2026-04-10T12:00:00.000Z",
            "startTime": "2026-04-10T11:50:00.000Z",
            "queueTime": "2026-04-10T11:49:00.000Z",
            "reason": "individualCI",
            "requestedBy": {
                "displayName": "John Doe",
                "id": "user-guid-1",
                "uniqueName": "john@example.com"
            },
            "requestedFor": {
                "displayName": "John Doe",
                "id": "user-guid-1",
                "uniqueName": "john@example.com"
            },
            "repository": {
                "id": "repo-guid",
                "name": "my-repo",
                "type": "TfsGit",
                "url": "https://dev.azure.com/org/project/_git/my-repo"
            },
            "logs": {
                "id": 0,
                "type": "Container",
                "url": "https://dev.azure.com/org/project/_apis/build/builds/12345/logs"
            },
            "url": "https://dev.azure.com/org/project/_apis/build/Builds/12345",
            "_links": {
                "self": {"href": "https://dev.azure.com/org/project/_apis/build/Builds/12345"},
                "web": {"href": "https://dev.azure.com/org/project/_build/results?buildId=12345"},
                "sourceVersionDisplayUri": {"href": "https://dev.azure.com/org/project/_apis/build/builds/12345/changes"}
            },
            "tags": [],
            "priority": "normal",
            "project": {
                "id": "proj-guid",
                "name": "MyProject",
                "state": "wellFormed",
                "visibility": "private"
            }
        }"#;

        let result = filter_runs_show(json).unwrap();
        let input_tokens = count_tokens(json);
        let output_tokens = count_tokens(&result.text);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "runs_show filter: expected ≥60% savings, got {:.1}%",
            savings
        );

        // Also verify output format
        assert!(result.text.contains("ok run 12345"));
        assert!(result.text.contains("CI Pipeline"));
    }

    // ─── prune_az_noise / compress_az_generic Tests ───────────────────────────

    #[test]
    fn test_prune_az_noise_strips_keys() {
        let mut value: Value = serde_json::from_str(r#"{
            "name": "keep-me",
            "_links": {"self": {"href": "https://example.com"}},
            "url": "https://example.com/api",
            "collectionUri": "https://example.com/collection",
            "projectUri": "https://example.com/project",
            "projectId": "guid-123",
            "revision": 5,
            "uniqueName": "user@example.com",
            "imageUrl": "https://example.com/avatar.png",
            "descriptor": "aad.abc123",
            "href": "https://example.com/link",
            "nested": {
                "real": "data",
                "_links": {"web": {"href": "https://inner"}},
                "url": "https://inner/api",
                "collectionUri": "https://inner/collection",
                "deep": {
                    "descriptor": "aad.deep",
                    "imageUrl": "https://deep/avatar.png",
                    "uniqueName": "deep@example.com",
                    "projectId": "deep-guid",
                    "projectUri": "https://deep/project",
                    "revision": 99,
                    "value": 42
                }
            },
            "items": [
                {"id": 1, "url": "https://arr/1", "href": "https://arr/h1", "label": "first"},
                {"id": 2, "_links": {"x": {}}, "label": "second"}
            ]
        }"#).unwrap();

        prune_az_noise(&mut value);

        let obj = value.as_object().unwrap();
        // Top-level noise keys gone
        for key in NOISE_KEYS {
            assert!(!obj.contains_key(*key), "top-level key '{}' should be stripped", key);
        }
        // Top-level real key survives
        assert_eq!(obj["name"].as_str().unwrap(), "keep-me");

        // Nested level
        let nested = obj["nested"].as_object().unwrap();
        for key in NOISE_KEYS {
            assert!(!nested.contains_key(*key), "nested key '{}' should be stripped", key);
        }
        assert_eq!(nested["real"].as_str().unwrap(), "data");

        // Deep nested level
        let deep = nested["deep"].as_object().unwrap();
        for key in NOISE_KEYS {
            assert!(!deep.contains_key(*key), "deep key '{}' should be stripped", key);
        }
        assert_eq!(deep["value"].as_i64().unwrap(), 42);

        // Array items
        let items = obj["items"].as_array().unwrap();
        for (i, item) in items.iter().enumerate() {
            let item_obj = item.as_object().unwrap();
            for key in NOISE_KEYS {
                assert!(!item_obj.contains_key(*key), "items[{}] key '{}' should be stripped", i, key);
            }
        }
        assert_eq!(items[0]["label"].as_str().unwrap(), "first");
        assert_eq!(items[1]["label"].as_str().unwrap(), "second");
    }

    #[test]
    fn test_compress_az_generic_preserves_values() {
        // Critical regression test: real values must survive, not be replaced
        // by schema type names like "string" or "int".
        let raw = r#"[
            {
                "buildNumber": "2026.0415.3",
                "id": 300000,
                "sourceBranch": "refs/heads/main",
                "result": "succeeded",
                "status": "completed",
                "_links": {"self": {"href": "https://dev.azure.com/org/proj/_apis/build/123"}},
                "url": "https://dev.azure.com/org/proj/_apis/build/Builds/300000",
                "collectionUri": "https://dev.azure.com/org/"
            },
            {
                "buildNumber": "2026.0415.4",
                "id": 300001,
                "sourceBranch": "refs/heads/feature/xyz",
                "result": "failed",
                "status": "completed",
                "_links": {"web": {"href": "https://dev.azure.com/org/proj/_build?buildId=300001"}},
                "url": "https://dev.azure.com/org/proj/_apis/build/Builds/300001",
                "collectionUri": "https://dev.azure.com/org/"
            }
        ]"#;

        let (output, truncated) = compress_az_generic(raw);

        // Real values preserved
        assert!(output.contains("2026.0415.3"), "buildNumber value must survive");
        assert!(output.contains("300000"), "numeric id must survive");
        assert!(output.contains("refs/heads/main"), "sourceBranch must survive");
        assert!(output.contains("succeeded"), "result must survive");

        // Noise keys stripped
        assert!(!output.contains("\"_links\""), "_links key must be stripped");
        assert!(!output.contains("\"collectionUri\""), "collectionUri key must be stripped");
        assert!(!output.contains("\"href\""), "href key must be stripped");

        // Not truncated (only 2 items, well under MAX_ITEMS)
        assert!(!truncated, "2-element array should not be truncated");
    }

    #[test]
    fn test_compress_az_generic_array_truncation() {
        // Build a JSON array of 25 trivial objects (exceeds MAX_ITEMS=20)
        let items: Vec<String> = (0..25)
            .map(|i| format!(r#"{{"id": {}, "name": "item-{}"}}"#, i, i))
            .collect();
        let raw = format!("[{}]", items.join(","));

        let (output, truncated) = compress_az_generic(&raw);

        assert!(output.contains("... +5 more items"), "should show +5 overflow trailer");
        assert!(truncated, "truncated flag must be true");
        assert!(
            output.len() < raw.len(),
            "output ({}) should be smaller than input ({})",
            output.len(),
            raw.len()
        );
    }

    #[test]
    fn test_compress_az_generic_non_json_passthrough() {
        let raw = "hello\nworld\n";
        let (output, truncated) = compress_az_generic(raw);
        assert_eq!(output, raw);
        assert!(!truncated);
    }

    #[test]
    fn test_compress_az_generic_non_json_tail_cap() {
        let raw = "x".repeat(30_000);
        let (output, truncated) = compress_az_generic(&raw);

        assert!(
            output.starts_with("... (30000 bytes total, showing last 20000)"),
            "should start with tail-cap header, got: {}",
            &output[..output.len().min(80)]
        );
        // Header + newline + 20000 bytes of 'x' → should be < 20300 total
        assert!(
            output.len() < 20_300,
            "output length {} should be under 20300",
            output.len()
        );
        assert!(truncated, "truncated flag must be true for tail-capped output");
    }

    #[test]
    fn test_compress_az_generic_nested_object() {
        let raw = r#"{
            "name": "my-repo",
            "defaultBranch": "refs/heads/main",
            "size": 42,
            "url": "https://dev.azure.com/org/proj/_apis/git/repositories/guid",
            "_links": {
                "self": {"href": "https://dev.azure.com/org/proj/_apis/git/repositories/guid"},
                "web": {"href": "https://dev.azure.com/org/proj/_git/my-repo"}
            },
            "project": {
                "id": "proj-guid",
                "name": "MyProject",
                "projectId": "proj-guid",
                "revision": 10,
                "url": "https://dev.azure.com/org/_apis/projects/proj-guid",
                "visibility": "private"
            }
        }"#;

        let (output, truncated) = compress_az_generic(raw);

        // Real values preserved
        assert!(output.contains("my-repo"), "name must survive");
        assert!(output.contains("refs/heads/main"), "defaultBranch must survive");
        assert!(output.contains("42"), "size must survive");
        assert!(output.contains("MyProject"), "project.name must survive");
        assert!(output.contains("private"), "visibility must survive");

        // Noise keys stripped at all depths
        assert!(!output.contains("\"_links\""), "_links must be stripped");
        assert!(!output.contains("\"href\""), "href must be stripped");
        assert!(!output.contains("\"projectId\""), "projectId must be stripped");
        assert!(!output.contains("\"revision\""), "revision must be stripped");
        // url appears as both key and could appear in value; check the key form
        // The key "url" should be stripped, so no "url": pattern
        assert!(!output.contains("\"url\""), "url key must be stripped");

        assert!(!truncated, "single object should not trigger truncation");
    }

    #[test]
    fn test_compress_az_generic_empty_array() {
        let raw = "[]";
        let (output, truncated) = compress_az_generic(raw);
        assert_eq!(output.trim(), "[]");
        assert!(!truncated);
    }

    #[test]
    fn test_compress_az_generic_guards_against_schema_regression() {
        // Values that happen to be type-name words must survive verbatim.
        // Guards against a future maintainer reintroducing a schema extractor
        // that swallows string values matching type names.
        let raw = r#"{
            "description": "string identifier for the object",
            "type": "int",
            "format": "url",
            "note": "this is of type boolean"
        }"#;

        let (output, truncated) = compress_az_generic(raw);

        assert!(
            output.contains("string identifier for the object"),
            "string value containing 'string' must survive intact"
        );
        assert!(
            output.contains("\"int\""),
            "value that IS the word 'int' must survive"
        );
        assert!(
            output.contains("\"url\""),
            "value that IS the word 'url' must survive (even though 'url' is a noise KEY)"
        );
        assert!(
            output.contains("this is of type boolean"),
            "string value containing 'boolean' must survive"
        );
        assert!(!truncated);
    }
}
