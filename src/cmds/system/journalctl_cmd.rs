//! Compact human-readable `journalctl` output.

use crate::core::runner::{self, RunMode, RunOptions};
use crate::core::utils::resolved_command;
use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

static SHORT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^([A-Z][a-z]{2}\s+\d{1,2}\s+\d{2}:\d{2}:\d{2})\s+\S+\s+(.+)$"
    )
    .unwrap()
});
static ISO_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(\d{4})-(\d{2})-(\d{2})[T ](\d{2}:\d{2}:\d{2})(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?\s+\S+\s+(.+)$"
    )
    .unwrap()
});

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("journalctl");
    cmd.args(args);
    let args_display = args.join(" ");

    if verbose > 0 {
        eprintln!("Running: journalctl {args_display}");
    }

    if should_passthrough(args) {
        return runner::run(
            cmd,
            "journalctl",
            &args_display,
            RunMode::Passthrough,
            RunOptions::default(),
        );
    }

    runner::run_filtered(
        cmd,
        "journalctl",
        &args_display,
        filter_journal,
        RunOptions::with_tee("journalctl").early_exit_on_failure(),
    )
}

fn should_passthrough(args: &[String]) -> bool {
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if matches!(arg, "-f" | "--follow" | "-x" | "--catalog") {
            return true;
        }
        if arg == "-o" || arg == "--output" {
            if args
                .get(index + 1)
                .is_some_and(|value| value.starts_with("json"))
            {
                return true;
            }
            index += 1;
        } else if arg.starts_with("-ojson")
            || arg
                .strip_prefix("--output=")
                .is_some_and(|value| value.starts_with("json"))
        {
            return true;
        }
        index += 1;
    }
    false
}

fn filter_journal(raw: &str) -> String {
    let mut output = Vec::new();
    let mut pending_display: Option<String> = None;
    let mut pending_key: Option<String> = None;
    let mut repeat_count = 0usize;

    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }

        let (display, key) = compact_line(line);
        if key.is_some() && key == pending_key {
            repeat_count += 1;
            continue;
        }

        flush_pending(
            &mut output,
            pending_display.take(),
            repeat_count,
        );
        pending_display = Some(display);
        pending_key = key;
        repeat_count = 1;
    }

    flush_pending(&mut output, pending_display, repeat_count);
    output.join("\n")
}

fn flush_pending(output: &mut Vec<String>, pending: Option<String>, count: usize) {
    let Some(mut line) = pending else {
        return;
    };
    if count > 1 {
        line.push_str(&format!(" (x{count})"));
    }
    output.push(line);
}

fn compact_line(line: &str) -> (String, Option<String>) {
    if is_error_or_stacktrace(line) {
        return (line.to_string(), None);
    }

    if let Some(captures) = SHORT_RE.captures(line) {
        let payload = captures[2].to_string();
        return (format!("{} {}", &captures[1], payload), Some(payload));
    }

    if let Some(captures) = ISO_RE.captures(line) {
        let month = month_name(&captures[2]);
        let day = captures[3].trim_start_matches('0');
        let payload = captures[5].to_string();
        return (
            format!("{month} {day:>2} {} {payload}", &captures[4]),
            Some(payload),
        );
    }

    (line.to_string(), Some(line.to_string()))
}

fn month_name(month: &str) -> &'static str {
    match month {
        "01" => "Jan",
        "02" => "Feb",
        "03" => "Mar",
        "04" => "Apr",
        "05" => "May",
        "06" => "Jun",
        "07" => "Jul",
        "08" => "Aug",
        "09" => "Sep",
        "10" => "Oct",
        "11" => "Nov",
        "12" => "Dec",
        _ => "???",
    }
}

fn is_error_or_stacktrace(line: &str) -> bool {
    let trimmed = line.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    line.chars().next().is_some_and(char::is_whitespace)
        || trimmed.starts_with("at ")
        || trimmed.starts_with("Caused by:")
        || ["error", "failed", "exception", "panic", "traceback"]
            .iter()
            .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_hostname_and_collapses_consecutive_duplicates() {
        let raw = "Jul 20 10:00:01 host portal[10]: request complete\nJul 20 10:00:02 host portal[10]: request complete\nJul 20 10:00:03 host portal[10]: request complete\n";
        let filtered = filter_journal(raw);

        assert_eq!(
            filtered,
            "Jul 20 10:00:01 portal[10]: request complete (x3)"
        );
        assert!(!filtered.contains(" host "));
    }

    #[test]
    fn compacts_iso_timestamp_to_short_form() {
        let raw = "2026-07-20T10:11:12.123+07:00 node service[1]: ready";
        assert_eq!(filter_journal(raw), "Jul 20 10:11:12 service[1]: ready");
    }

    #[test]
    fn preserves_error_and_stacktrace_lines_verbatim() {
        let raw = "Jul 20 10:00:01 host app[1]: ERROR database unavailable\n    at connect (db.js:10:2)\n";
        assert_eq!(filter_journal(raw), raw.trim_end());
    }

    #[test]
    fn json_follow_and_catalog_modes_passthrough() {
        assert!(should_passthrough(&["-o".into(), "json".into()]));
        assert!(should_passthrough(&["--output=json-pretty".into()]));
        assert!(should_passthrough(&["-ojson".into()]));
        assert!(should_passthrough(&["--follow".into()]));
        assert!(should_passthrough(&["-x".into()]));
        assert!(!should_passthrough(&[
            "-u".into(),
            "portal.service".into(),
            "-n".into(),
            "50".into()
        ]));
    }

    #[test]
    fn repeated_logs_save_at_least_ninety_percent() {
        let raw = (0..100)
            .map(|second| {
                format!(
                    "Jul 20 10:00:{:02} host portal[10]: health check passed",
                    second % 60
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let filtered = filter_journal(&raw);

        assert!(filtered.len() * 10 <= raw.len());
        assert!(filtered.ends_with("(x100)"));
    }
}
