//! Filters `uv run` output while preserving uv-managed environment semantics.
//!
//! `uv run` executes arbitrary programs, so on success its stdout and stderr are
//! the signal the caller asked for and are passed through unchanged. Collapsing a
//! successful run to a summary would discard the program's result with no way to
//! recover it. uv is silent unless it resolves or installs, so its own chatter is
//! left alone rather than stripped: suppressing it would also erase it from the
//! tee file, breaking recovery.

use crate::core::runner;
use crate::core::stream::{self, FilterMode, StdinMode};
use crate::core::tracking;
use crate::core::truncate::{CAP_INVENTORY, CAP_WARNINGS};
use crate::core::utils::{exit_code_from_status, resolved_command, strip_ansi, truncate};
use anyhow::{Context, Result};
use regex::Regex;
use std::sync::LazyLock;

static PYTHON_FRAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*File ".*", line \d+.*$"#).unwrap());
static PYTHON_EXCEPTION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*[A-Za-z_][A-Za-z0-9_.]*(?:Error|Exception):").unwrap());
static JS_FRAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*at .+:\d+:\d+.*$").unwrap());
static ERROR_START_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"(?i)\berror\b").unwrap(),
        Regex::new(r"(?i)\bfailed\b").unwrap(),
        Regex::new(r"(?i)\bfailure\b").unwrap(),
        Regex::new(r"(?i)\bexception\b").unwrap(),
        Regex::new(r"(?i)\bpanic\b").unwrap(),
        Regex::new(r"(?i)\bwarn(?:ing)?\b").unwrap(),
        Regex::new(r"(?i)\bassert(?:ion)?\b").unwrap(),
        Regex::new(r"^\s*FAILED\b").unwrap(),
        Regex::new(r"^\s*ERROR\b").unwrap(),
        Regex::new(r"^\s*E\s+").unwrap(),
        Regex::new(r"^\s*Caused by:").unwrap(),
        Regex::new(r"^\s*note:").unwrap(),
        Regex::new(r"^\s*help:").unwrap(),
    ]
});

const MAX_TRACEBACK_FRAMES: usize = CAP_WARNINGS;
const MAX_ERROR_CONTINUATION_LINES: usize = CAP_WARNINGS;
const MAX_FALLBACK_TAIL_LINES: usize = CAP_WARNINGS;
const MAX_PROGRAM_LINE_CHARS: usize = 500;
const TEE_SLUG_STDOUT: &str = "uv-run-stdout";
const TEE_SLUG_STDERR: &str = "uv-run-stderr";

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let args_display = args.join(" ");
    let original_cmd = display_command("uv", &args_display);
    let rtk_cmd = display_command("rtk uv", &args_display);

    let mut cmd = resolved_command("uv");
    cmd.args(args);

    if verbose > 0 {
        eprintln!("Running: {}", original_cmd);
    }

    if args.first().map(String::as_str) != Some("run") {
        let status = cmd.status().context("Failed to run uv")?;
        timer.track_passthrough(&original_cmd, &format!("{rtk_cmd} (passthrough)"));
        return Ok(exit_code_from_status(&status, "uv"));
    }

    let result = stream::run_streaming(&mut cmd, StdinMode::Inherit, FilterMode::CaptureOnly)
        .context("Failed to run uv")?;
    let filtered = filter_uv_run_output(
        &result.raw,
        &result.raw_stdout,
        &result.raw_stderr,
        result.exit_code,
    );

    runner::print_with_hint(&filtered, &result.raw, &result.raw, "uv", result.exit_code);
    timer.track(&original_cmd, &rtk_cmd, &result.raw, &filtered);

    Ok(result.exit_code)
}

fn display_command(prefix: &str, args_display: &str) -> String {
    if args_display.trim().is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix} {args_display}")
    }
}

fn filter_uv_run_output(output: &str, stdout: &str, stderr: &str, exit_code: i32) -> String {
    if exit_code == 0 {
        return filter_successful_run(stdout, stderr);
    }

    // On failure the streams are scanned merged: a Python traceback interleaves
    // stdout and stderr, and splitting it would break frame ordering.
    let clean = strip_ansi(output);
    let extracted = extract_diagnostics(&clean);
    if !extracted.is_empty() {
        return extracted;
    }

    let tail: Vec<String> = clean
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| truncate(line, 200))
        .collect();

    // The exit code already carries the failure; restating it would only add
    // tokens, so the command's own message is returned untouched.
    let skip = tail.len().saturating_sub(MAX_FALLBACK_TAIL_LINES);
    tail[skip..].join("\n")
}

/// Expects ANSI-stripped input.
fn extract_diagnostics(clean: &str) -> String {
    let lines: Vec<&str> = clean.lines().collect();
    let mut selected: Vec<String> = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        if is_traceback_start(trimmed) {
            let (block, next_idx) = collect_traceback_block(&lines, i);
            selected.extend(block);
            selected.push(String::new());
            i = next_idx;
            continue;
        }

        if is_error_start(trimmed) {
            let (block, next_idx) = collect_error_block(&lines, i);
            selected.extend(block);
            selected.push(String::new());
            i = next_idx;
            continue;
        }

        i += 1;
    }

    selected.join("\n").trim().to_string()
}

fn filter_successful_run(stdout: &str, stderr: &str) -> String {
    // Distinct slugs: both streams can need a tee in the same run, and tee
    // filenames are second-resolution, so a shared slug would make the second
    // write clobber the first and leave one hint pointing at the other's bytes.
    let payload = program_output(stdout, TEE_SLUG_STDOUT);
    let diagnostics = program_output(stderr, TEE_SLUG_STDERR);

    match (payload.is_empty(), diagnostics.is_empty()) {
        (true, true) => "ok".to_string(),
        (false, true) => payload,
        (true, false) => diagnostics,
        (false, false) => format!("{payload}\n{diagnostics}"),
    }
}

fn program_output(text: &str, tee_slug: &str) -> String {
    let clean = strip_ansi(text);
    let lines: Vec<&str> = clean.lines().collect();
    let last_content = lines.iter().rposition(|line| !line.trim().is_empty());

    let Some(last_content) = last_content else {
        return String::new();
    };
    let lines = &lines[..=last_content];
    let capped: Vec<String> = lines
        .iter()
        .map(|line| truncate(line, MAX_PROGRAM_LINE_CHARS))
        .collect();
    let line_was_cut = capped
        .iter()
        .zip(lines)
        .any(|(cut, full)| cut.len() != full.len());

    if capped.len() <= CAP_INVENTORY {
        let out = capped.join("\n");
        if line_was_cut {
            if let Some(hint) = crate::core::tee::force_tee_hint(&clean, tee_slug) {
                return format!("{out}\n{hint}");
            }
        }
        return out;
    }

    // A program's result is usually its last line, so keep both ends.
    let head = CAP_INVENTORY / 2;
    let tail = CAP_INVENTORY - head;
    let omitted = capped.len() - CAP_INVENTORY;

    let mut out = capped[..head].join("\n");
    out.push_str(&format!("\n... ({omitted} lines omitted)\n"));
    out.push_str(&capped[capped.len() - tail..].join("\n"));

    // A cut in the head region sits before the tail offset, so `tail -n +N` skips it.
    let head_line_was_cut = capped[..head]
        .iter()
        .zip(&lines[..head])
        .any(|(cut, full)| cut != full);

    let hint = if head_line_was_cut {
        crate::core::tee::force_tee_hint(&clean, tee_slug)
    } else {
        crate::core::tee::force_tee_tail_hint(&clean, tee_slug, head + 1)
    };

    if let Some(hint) = hint {
        out.push_str(&format!("\n{hint}"));
    }

    out
}

fn collect_traceback_block(lines: &[&str], start_idx: usize) -> (Vec<String>, usize) {
    let mut block = vec![lines[start_idx].trim().to_string()];
    let mut frames = Vec::new();
    let mut tail = Vec::new();
    let mut idx = start_idx + 1;

    while idx < lines.len() {
        let trimmed = lines[idx].trim();
        if trimmed.is_empty() {
            break;
        }

        if PYTHON_FRAME_RE.is_match(trimmed) {
            frames.push(truncate(trimmed, 160));
        } else {
            tail.push(truncate(trimmed, 200));
        }

        idx += 1;
    }

    block.extend(frames.iter().take(MAX_TRACEBACK_FRAMES).cloned());
    if frames.len() > MAX_TRACEBACK_FRAMES {
        block.push(format!(
            "... +{} more frames",
            frames.len() - MAX_TRACEBACK_FRAMES
        ));
        let full_traceback = lines[start_idx..idx].join("\n");
        if let Some(hint) = crate::core::tee::force_tee_hint(&full_traceback, "uv-traceback") {
            block.push(format!("  {hint}"));
        }
    }

    let tail_lines = tail
        .into_iter()
        .rev()
        .take(2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    block.extend(tail_lines);

    (dedupe_preserving_order(block), idx)
}

fn collect_error_block(lines: &[&str], start_idx: usize) -> (Vec<String>, usize) {
    let mut block = vec![truncate(lines[start_idx].trim(), 200)];
    let mut continuation_count = 0;
    let mut idx = start_idx + 1;

    while idx < lines.len() {
        let line = lines[idx];
        let trimmed = line.trim();

        if trimmed.is_empty() || !is_error_continuation(line) {
            break;
        }

        continuation_count += 1;
        if continuation_count <= MAX_ERROR_CONTINUATION_LINES {
            block.push(truncate(trimmed, 200));
        }

        idx += 1;
    }

    if continuation_count > MAX_ERROR_CONTINUATION_LINES {
        block.push(format!(
            "... +{} more lines",
            continuation_count - MAX_ERROR_CONTINUATION_LINES
        ));
        let full_block = lines[start_idx..idx].join("\n");
        if let Some(hint) = crate::core::tee::force_tee_hint(&full_block, "uv-error-block") {
            block.push(format!("  {hint}"));
        }
    }

    (dedupe_preserving_order(block), idx)
}

fn dedupe_preserving_order(lines: Vec<String>) -> Vec<String> {
    let mut deduped = Vec::new();
    for line in lines {
        if deduped.last() != Some(&line) {
            deduped.push(line);
        }
    }
    deduped
}

fn is_traceback_start(line: &str) -> bool {
    line.starts_with("Traceback ")
}

fn is_error_start(line: &str) -> bool {
    if is_traceback_start(line)
        || PYTHON_FRAME_RE.is_match(line)
        || PYTHON_EXCEPTION_RE.is_match(line)
        || JS_FRAME_RE.is_match(line)
    {
        return true;
    }

    if line.contains("No module named ") {
        return true;
    }

    ERROR_START_PATTERNS
        .iter()
        .any(|pattern| pattern.is_match(line))
}

fn is_error_continuation(line: &str) -> bool {
    let trimmed = line.trim();
    line.starts_with(' ')
        || line.starts_with('\t')
        || trimmed.starts_with('>')
        || trimmed.starts_with('|')
        || trimmed.starts_with("During handling of the above exception")
        || trimmed.starts_with("The above exception")
        || trimmed.starts_with("Caused by:")
        || trimmed.starts_with("note:")
        || trimmed.starts_with("help:")
        || PYTHON_FRAME_RE.is_match(trimmed)
        || PYTHON_EXCEPTION_RE.is_match(trimmed)
        || JS_FRAME_RE.is_match(trimmed)
}

#[cfg(test)]
mod tests {
    use super::{CAP_INVENTORY, MAX_TRACEBACK_FRAMES, filter_uv_run_output};
    use crate::core::utils::count_tokens;

    #[test]
    fn test_filter_uv_run_keeps_program_output_on_success() {
        let stdout = "hello from script\n";

        assert_eq!(
            filter_uv_run_output(stdout, stdout, "", 0),
            "hello from script"
        );
    }

    #[test]
    fn test_filter_uv_run_keeps_data_producing_stdout() {
        let stdout = "{\n  \"users\": 42,\n  \"active\": 37\n}\n";
        let raw = stdout.to_string();

        let result = filter_uv_run_output(&raw, stdout, "", 0);

        assert!(result.contains("\"users\": 42"));
        assert!(result.contains("\"active\": 37"));
    }

    #[test]
    fn test_filter_uv_run_keeps_non_error_stderr_on_success() {
        // uv's own chatter is suppressed upstream by `-q`, so whatever reaches
        // the filter on stderr belongs to the program and must survive.
        let stderr = "INFO:root:connected to db\nINFO:root:migrated 3 tables\n";
        let stdout = "done\n";
        let raw = format!("{stderr}{stdout}");

        let result = filter_uv_run_output(&raw, stdout, stderr, 0);

        assert!(result.contains("done"));
        assert!(result.contains("INFO:root:connected to db"));
        assert!(result.contains("INFO:root:migrated 3 tables"));
    }

    #[test]
    fn test_stdout_and_stderr_tee_slugs_are_distinct() {
        // Tee filenames are `{epoch_secs}_{slug}.log`. Both streams can need a
        // tee within the same second, so a shared slug would make the stderr
        // write clobber the stdout one and leave the stdout hint resolving to
        // stderr's bytes - the omitted stdout lines become unrecoverable.
        assert_ne!(super::TEE_SLUG_STDOUT, super::TEE_SLUG_STDERR);
    }

    #[test]
    fn test_program_output_truncates_over_line_cap_keeping_both_ends() {
        let stdout: String = (0..120).map(|i| format!("line{i}\n")).collect();

        let result = super::program_output(&stdout, super::TEE_SLUG_STDOUT);

        assert!(result.contains("line0"), "head must survive");
        assert!(result.contains("line119"), "tail must survive");
        assert!(result.contains("lines omitted"));
        assert!(result.lines().count() < 120);
    }

    #[test]
    fn test_program_output_head_cut_switches_away_from_the_tail_hint() {
        // 60 lines with a long line 4: the tail hint would start past it.
        let stdout: String = (0..60)
            .map(|i| {
                if i == 3 {
                    format!("{}\n", "x".repeat(900))
                } else {
                    format!("line{i}\n")
                }
            })
            .collect();

        let result = super::program_output(&stdout, super::TEE_SLUG_STDOUT);

        assert!(result.contains("lines omitted"));
        assert!(
            !result.contains("see remaining"),
            "a head-region cut must not be reported with a tail offset that skips it, got: {result}"
        );
    }

    #[test]
    fn test_program_output_caps_a_single_huge_line() {
        let stdout = format!("{}\n", "x".repeat(50_000));

        let result = super::program_output(&stdout, super::TEE_SLUG_STDOUT);

        assert!(
            result.len() < 2_000,
            "one huge line must be capped, got {} bytes",
            result.len()
        );
        assert!(result.contains("..."));
    }

    #[test]
    fn test_program_output_exact_cap_is_untouched() {
        let stdout: String = (0..CAP_INVENTORY).map(|i| format!("line{i}\n")).collect();

        let result = super::program_output(&stdout, super::TEE_SLUG_STDOUT);

        assert!(!result.contains("lines omitted"));
        assert_eq!(result.lines().count(), CAP_INVENTORY);
    }

    #[test]
    fn test_program_output_handles_multibyte_without_panic() {
        let stdout: String = (0..80).map(|i| format!("日本語 🎉 line{i}\n")).collect();

        let result = super::program_output(&stdout, super::TEE_SLUG_STDOUT);

        assert!(result.contains("日本語"));
        assert!(result.contains("lines omitted"));
    }

    #[test]
    fn test_filter_uv_run_silent_success_is_ok() {
        assert_eq!(filter_uv_run_output("", "", "", 0), "ok");
    }

    #[test]
    fn test_filter_uv_run_success_keeps_stderr_warnings_with_payload() {
        let stdout = "result: 7\n";
        let stderr = "WARNING: deprecated api\n";
        let raw = format!("{stderr}{stdout}");

        let result = filter_uv_run_output(&raw, stdout, stderr, 0);

        assert!(result.contains("result: 7"));
        assert!(result.contains("WARNING: deprecated api"));
    }

    #[test]
    fn test_filter_uv_run_truncates_python_tracebacks() {
        let output = r#"
Traceback (most recent call last):
  File "/tmp/project/main.py", line 10, in <module>
    run()
  File "/tmp/project/app.py", line 22, in run
    inner()
  File "/tmp/project/lib.py", line 33, in inner
    boom()
  File "/tmp/project/helpers.py", line 44, in boom
    raise RuntimeError("kaboom")
RuntimeError: kaboom
"#;

        let result = filter_uv_run_output(output, "", "", 1);
        assert!(result.contains("Traceback (most recent call last):"));
        assert!(result.contains(r#"File "/tmp/project/main.py", line 10, in <module>"#));
        assert!(result.contains("RuntimeError: kaboom"));
        assert!(!result.contains("run()"));
    }

    #[test]
    fn test_filter_uv_run_truncates_many_python_frames() {
        let mut output = String::from("Traceback (most recent call last):\n");
        for i in 0..(MAX_TRACEBACK_FRAMES + 2) {
            output.push_str(&format!(
                "  File \"/tmp/project/module_{i}.py\", line {i}, in call_{i}\n"
            ));
            output.push_str("    call_next()\n");
        }
        output.push_str("RuntimeError: kaboom\n");

        let result = filter_uv_run_output(&output, "", "", 1);
        assert!(result.contains("Traceback (most recent call last):"));
        assert!(result.contains("... +2 more frames"));
    }

    #[test]
    fn test_filter_uv_run_keeps_failure_summary_lines() {
        let output = r#"
Resolved 8 packages in 30ms
============================= test session starts =============================
FAILED tests/test_api.py::test_healthcheck - AssertionError: expected 200
1 failed, 12 passed in 0.31s
"#;

        let result = filter_uv_run_output(output, "", "", 1);
        assert!(result.contains("FAILED tests/test_api.py::test_healthcheck"));
        assert!(result.contains("1 failed, 12 passed in 0.31s"));
        assert!(!result.contains("Resolved 8 packages"));
    }

    #[test]
    fn test_filter_uv_run_failure_returns_message_without_added_marker() {
        // The exit code is propagated, so the filter must not restate it.
        let output = "sync aborted by signal";
        let result = filter_uv_run_output(output, "", "", 2);

        assert_eq!(result, "sync aborted by signal");
    }

    #[test]
    fn test_filter_uv_run_silent_failure_emits_nothing() {
        assert_eq!(filter_uv_run_output("", "", "", 2), "");
    }

    #[test]
    fn test_filter_uv_run_pytest_fixture_token_savings() {
        let input = include_str!("../../../tests/fixtures/uv_run_pytest_failure.txt");
        let output = filter_uv_run_output(input, "", "", 1);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 70.0,
            "uv run pytest: expected >=70% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
        assert!(output.contains("FAILED tests/test_users.py::test_normalize_user_rejects_empty"));
        assert!(output.contains("1 failed, 1 passed"));
        assert!(!output.contains("Downloading cpython"));
    }
}
