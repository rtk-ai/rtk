//! Filters `poetry run` output while preserving poetry-managed environment semantics.
//!
//! Structurally mirrors uv_cmd.rs: same generic traceback/error filtering,
//! since `poetry run <tool>` and `uv run <tool>` both just wrap an arbitrary
//! tool invocation, and the noise worth filtering (Python tracebacks, error/
//! failure summary lines) is tool-specific, not wrapper-specific.

use crate::core::runner;
use crate::core::stream::{self, FilterMode, StdinMode};
use crate::core::tracking;
use crate::core::truncate::CAP_WARNINGS;
use crate::core::utils::{exit_code_from_status, resolved_command, strip_ansi, truncate};
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref PYTHON_FRAME_RE: Regex = Regex::new(r#"^\s*File ".*", line \d+.*$"#).unwrap();
    static ref PYTHON_EXCEPTION_RE: Regex =
        Regex::new(r"^\s*[A-Za-z_][A-Za-z0-9_.]*(?:Error|Exception):").unwrap();
    static ref ERROR_START_PATTERNS: Vec<Regex> = vec![
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
    ];
}

const MAX_TRACEBACK_FRAMES: usize = CAP_WARNINGS;
const MAX_ERROR_CONTINUATION_LINES: usize = CAP_WARNINGS;
const MAX_FALLBACK_TAIL_LINES: usize = CAP_WARNINGS;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let args_display = args.join(" ");
    let original_cmd = display_command("poetry", &args_display);
    let rtk_cmd = display_command("rtk poetry", &args_display);

    let mut cmd = resolved_command("poetry");
    cmd.args(args);

    if verbose > 0 {
        eprintln!("Running: {}", original_cmd);
    }

    if args.first().map(String::as_str) != Some("run") {
        let status = cmd.status().context("Failed to run poetry")?;
        timer.track_passthrough(&original_cmd, &format!("{rtk_cmd} (passthrough)"));
        return Ok(exit_code_from_status(&status, "poetry"));
    }

    let result = stream::run_streaming(&mut cmd, StdinMode::Inherit, FilterMode::CaptureOnly)
        .context("Failed to run poetry")?;
    let filtered = filter_poetry_run_output(&result.raw, result.exit_code);

    runner::print_with_hint(&filtered, &result.raw, &result.raw, "poetry", result.exit_code);
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

fn filter_poetry_run_output(output: &str, exit_code: i32) -> String {
    let clean = strip_ansi(output);
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

    let filtered = selected.join("\n").trim().to_string();
    if !filtered.is_empty() {
        return filtered;
    }

    if exit_code == 0 {
        return "ok".to_string();
    }

    let tail: Vec<String> = clean
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| truncate(line, 200))
        .collect();

    if tail.is_empty() {
        return format!("[FAIL] poetry run failed (exit code: {exit_code})");
    }

    let summary = tail
        .into_iter()
        .rev()
        .take(MAX_FALLBACK_TAIL_LINES)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();

    format!(
        "[FAIL] poetry run failed (exit code: {exit_code})\n{}",
        summary.join("\n")
    )
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
        if let Some(hint) = crate::core::tee::force_tee_hint(&full_traceback, "poetry-traceback") {
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
        if let Some(hint) = crate::core::tee::force_tee_hint(&full_block, "poetry-error-block") {
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
    if is_traceback_start(line) || PYTHON_FRAME_RE.is_match(line) || PYTHON_EXCEPTION_RE.is_match(line) {
        return true;
    }

    if line.contains("No module named ") {
        return true;
    }

    ERROR_START_PATTERNS.iter().any(|pattern| pattern.is_match(line))
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
}

#[cfg(test)]
mod tests {
    use super::filter_poetry_run_output;

    #[test]
    fn test_filter_poetry_run_suppresses_success_noise() {
        let output = r#"
Installing dependencies from lock file

No dependencies to install or update
hello from script
"#;

        assert_eq!(filter_poetry_run_output(output, 0), "ok");
    }

    #[test]
    fn test_filter_poetry_run_truncates_python_tracebacks() {
        let output = r#"
Traceback (most recent call last):
  File "/tmp/project/main.py", line 10, in <module>
    run()
  File "/tmp/project/app.py", line 22, in run
    inner()
RuntimeError: kaboom
"#;

        let result = filter_poetry_run_output(output, 1);
        assert!(result.contains("Traceback (most recent call last):"));
        assert!(result.contains(r#"File "/tmp/project/main.py", line 10, in <module>"#));
        assert!(result.contains("RuntimeError: kaboom"));
        assert!(!result.contains("run()"));
    }

    #[test]
    fn test_filter_poetry_run_keeps_failure_summary_lines() {
        let output = r#"
Installing dependencies from lock file
============================= test session starts =============================
FAILED tests/test_api.py::test_healthcheck - AssertionError: expected 200
1 failed, 12 passed in 0.31s
"#;

        let result = filter_poetry_run_output(output, 1);
        assert!(result.contains("FAILED tests/test_api.py::test_healthcheck"));
        assert!(result.contains("1 failed, 12 passed in 0.31s"));
        assert!(!result.contains("Installing dependencies"));
    }

    #[test]
    fn test_filter_poetry_run_has_failure_fallback() {
        let output = "poetry aborted by signal";
        let result = filter_poetry_run_output(output, 2);

        assert!(result.contains("[FAIL] poetry run failed (exit code: 2)"));
        assert!(result.contains("poetry aborted by signal"));
    }
}
