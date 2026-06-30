//! tmux wrappers for common agent-orchestration pane operations.

use crate::core::tracking;
use crate::core::utils::resolved_command;
use anyhow::{bail, Context, Result};
use regex::Regex;
use std::process::Stdio;

const DEFAULT_CAPTURE_TAIL: usize = 50;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    match args.first().map(String::as_str) {
        Some("capture-pane") => run_capture_pane(args, verbose),
        Some("send-keys") => run_send_keys(args, verbose),
        _ => run_passthrough(args, verbose),
    }
}

fn run_capture_pane(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let CaptureArgs {
        tmux_args,
        since_marker,
    } = parse_capture_args(args)?;

    let mut cmd = resolved_command("tmux");
    for arg in &tmux_args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: tmux {}", tmux_args.join(" "));
    }

    let output = cmd.output().context("Failed to run tmux capture-pane")?;
    let exit_code = output.status.code().unwrap_or(1);

    if let Some((stdout, stderr)) = output_to_print_on_failure(
        output.status.success(),
        &output.stdout,
        &output.stderr,
    ) {
        print_output(stdout, stderr)?;
        return Ok(exit_code);
    }

    let raw = String::from_utf8_lossy(&output.stdout).into_owned();
    let has_scrollback_flag = has_scrollback_flag(&tmux_args);
    let filtered = filter_capture_pane(&raw, since_marker.as_ref(), has_scrollback_flag);

    if !filtered.is_empty() {
        println!("{filtered}");
    }

    timer.track(
        &format!("tmux {}", tmux_args.join(" ")),
        &format!("rtk tmux {}", args.join(" ")),
        &raw,
        &filtered,
    );

    Ok(exit_code)
}

fn run_send_keys(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let mut cmd = resolved_command("tmux");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: tmux {}", args.join(" "));
    }

    let output = cmd.output().context("Failed to run tmux send-keys")?;
    let exit_code = output.status.code().unwrap_or(1);

    if let Some((stdout, stderr)) = output_to_print_on_failure(
        output.status.success(),
        &output.stdout,
        &output.stderr,
    ) {
        print_output(stdout, stderr)?;
        return Ok(exit_code);
    }

    timer.track_passthrough(
        &format!("tmux {}", args.join(" ")),
        &format!("rtk tmux {}", args.join(" ")),
    );

    Ok(exit_code)
}

fn run_passthrough(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let mut cmd = resolved_command("tmux");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: tmux {}", args.join(" "));
    }

    let status = cmd
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("Failed to run tmux")?;
    let exit_code = status.code().unwrap_or(1);

    timer.track_passthrough(
        &format!("tmux {}", args.join(" ")),
        &format!("rtk tmux {}", args.join(" ")),
    );

    Ok(exit_code)
}

struct CaptureArgs {
    tmux_args: Vec<String>,
    since_marker: Option<Regex>,
}

fn parse_capture_args(args: &[String]) -> Result<CaptureArgs> {
    let mut tmux_args = Vec::with_capacity(args.len());
    let mut since_marker = None;
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        if arg == "--since-marker" {
            let Some(pattern) = iter.next() else {
                bail!("--since-marker requires a regex");
            };
            since_marker = Some(Regex::new(pattern).context("invalid --since-marker regex")?);
        } else {
            tmux_args.push(arg.clone());
        }
    }

    Ok(CaptureArgs {
        tmux_args,
        since_marker,
    })
}

fn has_scrollback_flag(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg == "-S" || arg.starts_with("-S") && arg.len() > 2)
}

fn filter_capture_pane(
    raw: &str,
    since_marker: Option<&Regex>,
    has_scrollback_flag: bool,
) -> String {
    let mut lines: Vec<&str> = raw.lines().collect();

    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }

    if let Some(marker) = since_marker {
        if let Some(index) = lines.iter().rposition(|line| marker.is_match(line)) {
            lines = lines.into_iter().skip(index + 1).collect();
        } else {
            lines.clear();
        }
    } else if !has_scrollback_flag && lines.len() > DEFAULT_CAPTURE_TAIL {
        lines = lines[lines.len() - DEFAULT_CAPTURE_TAIL..].to_vec();
    }

    lines.join("\n")
}

fn output_to_print_on_failure<'a>(
    success: bool,
    stdout: &'a [u8],
    stderr: &'a [u8],
) -> Option<(&'a [u8], &'a [u8])> {
    if success {
        None
    } else {
        Some((stdout, stderr))
    }
}

fn print_output(stdout: &[u8], stderr: &[u8]) -> Result<()> {
    use std::io::Write;

    let mut out = std::io::stdout().lock();
    out.write_all(stdout).context("Failed to write tmux stdout")?;

    let mut err = std::io::stderr().lock();
    err.write_all(stderr).context("Failed to write tmux stderr")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::utils::count_tokens;

    #[test]
    fn test_filter_capture_pane_strips_trailing_blank_lines() {
        let raw = "build started\nbuild done\n\n   \n";
        assert_eq!(
            filter_capture_pane(raw, None, false),
            "build started\nbuild done"
        );
    }

    #[test]
    fn test_filter_capture_pane_defaults_to_last_50_lines() {
        let raw = (1..=75)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let filtered = filter_capture_pane(&raw, None, false);
        let lines: Vec<&str> = filtered.lines().collect();

        assert_eq!(lines.len(), DEFAULT_CAPTURE_TAIL);
        assert_eq!(lines.first(), Some(&"line 26"));
        assert_eq!(lines.last(), Some(&"line 75"));
    }

    #[test]
    fn test_filter_capture_pane_scrollback_flag_bypasses_tail_cap() {
        let raw = (1..=75)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let filtered = filter_capture_pane(&raw, None, true);

        assert_eq!(filtered.lines().count(), 75);
        assert!(filtered.starts_with("line 1\n"));
    }

    #[test]
    fn test_filter_capture_pane_since_marker_keeps_after_last_match() {
        let raw = "old output\nDONE first\nstale\nDONE second\nfresh one\nfresh two\n\n";
        let marker = Regex::new("DONE").unwrap();

        assert_eq!(
            filter_capture_pane(raw, Some(&marker), false),
            "fresh one\nfresh two"
        );
    }

    #[test]
    fn test_filter_capture_pane_since_marker_no_match_returns_empty() {
        let raw = (1..=75)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let marker = Regex::new("DONE").unwrap();

        assert_eq!(filter_capture_pane(&raw, Some(&marker), true), "");
    }

    #[test]
    fn test_filter_capture_pane_empty_input() {
        assert_eq!(filter_capture_pane("", None, false), "");
        assert_eq!(filter_capture_pane("\n\n", None, false), "");
    }

    #[test]
    fn test_filter_capture_pane_saves_tokens_on_stale_output() {
        let raw = (1..=150)
            .map(|i| format!("stale prompt and build output line {i:03}"))
            .chain((1..=20).map(|i| format!("leader analysis line {i:02}")))
            .collect::<Vec<_>>()
            .join("\n");
        let filtered = filter_capture_pane(&raw, None, false);
        let savings =
            100.0 - (count_tokens(&filtered) as f64 / count_tokens(&raw) as f64 * 100.0);

        assert!(savings >= 60.0, "expected >=60% savings, got {savings:.1}%");
    }

    #[test]
    fn test_parse_capture_args_strips_since_marker() {
        let args = vec![
            "capture-pane".to_string(),
            "--since-marker".to_string(),
            "ERROR|DONE".to_string(),
            "-t".to_string(),
            "gma:1.2".to_string(),
            "-p".to_string(),
        ];
        let parsed = parse_capture_args(&args).unwrap();

        assert_eq!(
            parsed.tmux_args,
            vec![
                "capture-pane".to_string(),
                "-t".to_string(),
                "gma:1.2".to_string(),
                "-p".to_string()
            ]
        );
        assert!(parsed.since_marker.unwrap().is_match("DONE"));
    }

    #[test]
    fn test_send_keys_output_to_print_suppresses_success_output() {
        assert!(output_to_print_on_failure(true, b"ignored stdout", b"ignored stderr").is_none());
    }

    #[test]
    fn test_send_keys_output_to_print_keeps_failure_output() {
        assert_eq!(
            output_to_print_on_failure(false, b"stdout", b"stderr"),
            Some((&b"stdout"[..], &b"stderr"[..]))
        );
    }
}
