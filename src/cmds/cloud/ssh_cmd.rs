//! SSH output filter.
//!
//! Three output modes:
//! - JSON body (`ssh host 'curl ...'`) → pass through unchanged to preserve parseability
//! - Log/grep output → keep WARN/ERROR lines + 1-line context, suppress INFO/DEBUG
//! - Plain text (uptime, systemctl status, etc.) → truncate at MAX_PLAIN_LINES
//!
//! Interactive sessions (no remote command in args) → passthrough with no filtering.

use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;
use std::ffi::OsString;

const MAX_PLAIN_LINES: usize = 50;
const MAX_JSON_CHARS: usize = 4000;

lazy_static! {
    static ref LOG_IMPORTANT: Regex =
        Regex::new(r"(?i)\b(WARN|WARNING|ERROR|FATAL|CRITICAL|PANIC)\b").unwrap();
    static ref LOG_NOISE: Regex = Regex::new(r"(?i)\b(INFO|DEBUG|TRACE)\b").unwrap();
    static ref ANSI_ESCAPE: Regex = Regex::new(r"\x1b\[[0-9;]*[mGKHF]").unwrap();
}

/// SSH options that consume the next token (so we don't confuse it with the hostname).
const OPTS_WITH_ARG: &[&str] = &[
    "-b", "-c", "-D", "-E", "-e", "-F", "-I", "-i", "-J", "-L", "-l", "-m", "-o", "-p", "-Q",
    "-R", "-S", "-W", "-w",
];

/// Returns true when args contain a remote command after the [user@]host.
fn has_remote_command(args: &[String]) -> bool {
    let mut skip_next = false;
    let mut host_seen = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg.starts_with('-') {
            if OPTS_WITH_ARG.contains(&arg.as_str()) {
                skip_next = true;
            }
            continue;
        }
        if !host_seen {
            host_seen = true; // first non-option arg = [user@]host
            continue;
        }
        return true; // anything after host = remote command
    }
    false
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("Running: ssh {}", args.join(" "));
    }

    // Interactive session — inherit TTY, no filtering
    if !has_remote_command(args) {
        let os_args: Vec<OsString> = args.iter().map(Into::into).collect();
        return runner::run_passthrough("ssh", &os_args, verbose);
    }

    let mut cmd = resolved_command("ssh");
    for arg in args {
        cmd.arg(arg);
    }

    runner::run_filtered(
        cmd,
        "ssh",
        &args.join(" "),
        filter_ssh_output,
        RunOptions::stdout_only().tee("ssh"),
    )
}

pub fn filter_ssh_output(raw: &str) -> String {
    let clean = ANSI_ESCAPE.replace_all(raw.trim(), "");
    let text = clean.as_ref();
    let t = text.trim();

    // JSON body → pass through (truncating mid-JSON breaks downstream parsers)
    if (t.starts_with('{') && t.ends_with('}')) || (t.starts_with('[') && t.ends_with(']')) {
        if t.len() > MAX_JSON_CHARS {
            // Large JSON: trim with byte-safe boundary
            let mut end = MAX_JSON_CHARS;
            while !t.is_char_boundary(end) {
                end -= 1;
            }
            return format!("{}... ({} bytes total)", &t[..end], t.len());
        }
        return t.to_owned();
    }

    let lines: Vec<&str> = t.lines().collect();

    // Log/grep output → keep important lines + 1-line context.
    // Require ≥10% of lines to have a log-level prefix in their first 3 tokens
    // to avoid false positives from Prometheus metric names / HELP text.
    if looks_like_log_output(&lines) {
        return filter_log_lines(&lines);
    }

    // Plain text → truncate at MAX_PLAIN_LINES
    if lines.len() > MAX_PLAIN_LINES {
        let omitted = lines.len() - MAX_PLAIN_LINES;
        return format!("{}\n... ({} more lines)", lines[..MAX_PLAIN_LINES].join("\n"), omitted);
    }

    t.to_owned()
}

/// True when a line has a log-level keyword in its first 3 whitespace-separated tokens.
/// Scanning only leading tokens prevents metric names like `oneshield_build_info` from
/// triggering log mode — those contain `info` mid-token where `\b` doesn't match,
/// but even if they did, they'd only appear as the first token in metric lines.
fn is_log_line(line: &str) -> bool {
    line.split_whitespace()
        .take(3)
        .any(|tok| LOG_IMPORTANT.is_match(tok) || LOG_NOISE.is_match(tok))
}

/// Log mode needs ≥10% of lines to look like log entries (min 1).
/// Prometheus metrics, systemctl status, and `ps aux` output all score 0%.
/// Application logs with INFO/WARN/ERROR prefixes score 80–100%.
fn looks_like_log_output(lines: &[&str]) -> bool {
    if lines.is_empty() {
        return false;
    }
    let threshold = (lines.len() / 10).max(1);
    lines.iter().filter(|l| is_log_line(l)).count() >= threshold
}

fn filter_log_lines(lines: &[&str]) -> String {
    let mut kept: Vec<String> = Vec::new();
    let mut suppressed: usize = 0;
    let mut prev_important = false;

    for line in lines {
        if LOG_IMPORTANT.is_match(line) {
            kept.push(line.to_string());
            prev_important = true;
        } else if LOG_NOISE.is_match(line) {
            suppressed += 1;
            prev_important = false;
        } else if prev_important {
            kept.push(line.to_string()); // 1-line context
            prev_important = false;
        } else {
            kept.push(line.to_string()); // headers, prompts, plain output
        }
    }

    if suppressed > 0 {
        kept.push(format!("... ({} INFO/DEBUG lines suppressed)", suppressed));
    }
    kept.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    fn savings_pct(input: &str, output: &str) -> f64 {
        let in_tok = count_tokens(input);
        if in_tok == 0 {
            return 0.0;
        }
        100.0 - (count_tokens(output) as f64 / in_tok as f64 * 100.0)
    }

    // ── interactive detection ────────────────────────────────────────────────

    #[test]
    fn test_interactive_bare_host() {
        assert!(!has_remote_command(&["stg-dp1".into()]));
    }

    #[test]
    fn test_interactive_flag_no_cmd() {
        assert!(!has_remote_command(&["-A".into(), "stg-dp1".into()]));
    }

    #[test]
    fn test_interactive_o_option_consumes_value() {
        // -o StrictHostKeyChecking=no must not be mistaken for the host
        assert!(!has_remote_command(&[
            "-o".into(),
            "StrictHostKeyChecking=no".into(),
            "stg-dp1".into()
        ]));
    }

    #[test]
    fn test_interactive_jump_host() {
        // -J jump-host consumes next token; targethost is the host, no cmd
        assert!(!has_remote_command(&[
            "-J".into(),
            "jump-host".into(),
            "targethost".into()
        ]));
    }

    #[test]
    fn test_oneshot_bare_cmd() {
        assert!(has_remote_command(&["stg-dp1".into(), "uptime".into()]));
    }

    #[test]
    fn test_oneshot_with_p_option() {
        assert!(has_remote_command(&[
            "-p".into(),
            "22".into(),
            "stg-dp1".into(),
            "hostname".into()
        ]));
    }

    #[test]
    fn test_oneshot_user_at_host() {
        assert!(has_remote_command(&[
            "root@stg-dp1".into(),
            "hostname".into()
        ]));
    }

    #[test]
    fn test_oneshot_jump_host_with_cmd() {
        // -J jump targethost cmd
        assert!(has_remote_command(&[
            "-J".into(),
            "jump".into(),
            "targethost".into(),
            "uptime".into()
        ]));
    }

    #[test]
    fn test_oneshot_multi_o_with_cmd() {
        assert!(has_remote_command(&[
            "-o".into(),
            "ConnectTimeout=5".into(),
            "-o".into(),
            "StrictHostKeyChecking=no".into(),
            "hn01-dp1".into(),
            "curl -s http://localhost:8080/api/v1/stats".into()
        ]));
    }

    // ── JSON output ──────────────────────────────────────────────────────────

    #[test]
    fn test_json_small_passthrough() {
        let json = r#"{"status":"ok","zones":3}"#;
        assert_eq!(filter_ssh_output(json), json);
    }

    #[test]
    fn test_json_array_passthrough() {
        let json = r#"[{"id":1},{"id":2}]"#;
        assert_eq!(filter_ssh_output(json), json);
    }

    #[test]
    fn test_json_large_truncated() {
        let body = "x".repeat(5000);
        let json = format!(r#"{{"data":"{}"}}"#, body);
        let out = filter_ssh_output(&json);
        assert!(out.contains("bytes total"), "must show byte count");
        assert!(out.len() < json.len(), "must be shorter than input");
    }

    #[test]
    fn test_json_large_byte_safe_boundary() {
        // Ensure truncation doesn't split a multi-byte UTF-8 char
        let body = "é".repeat(3000); // 2 bytes each = 6000 bytes
        let json = format!(r#"{{"data":"{}"}}"#, body);
        let out = filter_ssh_output(&json);
        // Output must be valid UTF-8 (no panic = success, assert well-formed)
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
        assert!(out.contains("bytes total"));
    }

    #[test]
    fn test_json_with_ansi_stripped() {
        let raw = "\x1b[32m{\"status\":\"ok\"}\x1b[0m";
        assert_eq!(filter_ssh_output(raw), r#"{"status":"ok"}"#);
    }

    // ── log filtering ────────────────────────────────────────────────────────

    #[test]
    fn test_log_suppresses_info() {
        let raw = "2026-05-18 INFO starting\n2026-05-18 ERROR disk full\n2026-05-18 INFO done";
        let out = filter_ssh_output(raw);
        assert!(out.contains("ERROR disk full"));
        assert!(out.contains("INFO/DEBUG lines suppressed"));
        assert!(!out.contains("INFO starting"));
        assert!(!out.contains("INFO done"));
    }

    #[test]
    fn test_log_keeps_warn_with_context() {
        let raw = "DEBUG tick\nWARN high memory\ncontext line\nDEBUG tick2";
        let out = filter_ssh_output(raw);
        assert!(out.contains("WARN high memory"));
        assert!(out.contains("context line"), "1-line context after WARN must be kept");
        assert!(!out.contains("DEBUG tick2"));
    }

    #[test]
    fn test_log_keeps_fatal_critical() {
        let raw = "INFO tick\nFATAL oom killer\nCRITICAL signal 11";
        let out = filter_ssh_output(raw);
        assert!(out.contains("FATAL oom killer"));
        assert!(out.contains("CRITICAL signal 11"));
    }

    // ── plain text truncation ────────────────────────────────────────────────

    #[test]
    fn test_plain_truncated_at_50() {
        let lines: Vec<String> = (0..100).map(|i| format!("line {}", i)).collect();
        let out = filter_ssh_output(&lines.join("\n"));
        assert!(out.contains("50 more lines"), "truncation annotation required");
        assert!(!out.contains("line 99"), "must not contain lines past cutoff");
    }

    #[test]
    fn test_plain_short_passthrough() {
        let raw = "hostname\nuptime\nload: 0.5";
        assert_eq!(filter_ssh_output(raw), raw);
    }

    #[test]
    fn test_plain_strips_ansi() {
        let raw = "\x1b[32mOK\x1b[0m status";
        assert_eq!(filter_ssh_output(raw), "OK status");
    }

    #[test]
    fn test_prometheus_metrics_not_detected_as_log() {
        // Prometheus HELP/TYPE/value lines must NOT trigger log mode even if metric
        // names contain substrings like "info" in "build_info".
        let input = include_str!("../../../tests/fixtures/ssh/ssh_plain_metrics_raw.txt");
        let out = filter_ssh_output(input);
        assert!(
            out.contains("more lines"),
            "Prometheus metrics (100 lines) must be truncated as plain text, not passed through as log"
        );
    }

    #[test]
    fn test_log_threshold_requires_10_pct() {
        // 9 plain lines + 0 log lines → below 10% threshold → plain truncation path
        let lines: Vec<String> = (0..9).map(|i| format!("plain line {}", i)).collect();
        let raw = lines.join("\n");
        let out = filter_ssh_output(&raw);
        // Short output (< MAX_PLAIN_LINES), should pass through unchanged
        assert_eq!(out, raw);
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(filter_ssh_output(""), "");
    }

    #[test]
    fn test_unicode_passthrough() {
        let raw = "日本語テスト\nok";
        assert_eq!(filter_ssh_output(raw), raw);
    }

    // ── snapshot tests (insta) ───────────────────────────────────────────────

    #[test]
    fn test_snapshot_log_fixture() {
        let input = include_str!("../../../tests/fixtures/ssh/ssh_log_raw.txt");
        let output = filter_ssh_output(input);
        assert_snapshot!(output);
    }

    #[test]
    fn test_snapshot_plain_metrics_fixture() {
        let input = include_str!("../../../tests/fixtures/ssh/ssh_plain_metrics_raw.txt");
        let output = filter_ssh_output(input);
        assert_snapshot!(output);
    }

    // ── token accuracy (≥65% savings on dominant real-world patterns) ────────

    #[test]
    fn test_token_savings_log_fixture() {
        let input = include_str!("../../../tests/fixtures/ssh/ssh_log_raw.txt");
        let output = filter_ssh_output(input);
        let savings = savings_pct(input, &output);
        assert!(
            savings >= 65.0,
            "log filter: expected ≥65% savings, got {:.1}% (input {} tokens, output {} tokens)",
            savings,
            count_tokens(input),
            count_tokens(&output),
        );
    }

    #[test]
    fn test_token_savings_plain_metrics_fixture() {
        let input = include_str!("../../../tests/fixtures/ssh/ssh_plain_metrics_raw.txt");
        let output = filter_ssh_output(input);
        let savings = savings_pct(input, &output);
        assert!(
            savings >= 40.0,
            "plain truncation: expected ≥40% savings, got {:.1}%",
            savings,
        );
    }
}
