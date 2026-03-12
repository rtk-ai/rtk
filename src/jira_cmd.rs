//! Jira CLI (jira) command output compression.
//!
//! Provides token-optimized alternatives to verbose `jira` commands.
//! Auto-injects `--plain` / `--no-input` flags to prevent TUI/interactive mode.

use crate::tracking;
use crate::utils::truncate;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use std::process::Command;

lazy_static! {
    static ref ANSI_RE: Regex = Regex::new(r"\x1b\[[0-9;]*[mGKHFJ]").unwrap();
}

/// Strip ANSI escape sequences from a string.
fn strip_ansi(s: &str) -> String {
    ANSI_RE.replace_all(s, "").to_string()
}

/// Return args with `flag` appended if not already present.
fn ensure_flag(args: &[String], flag: &str) -> Vec<String> {
    if args.iter().any(|a| a == flag) {
        args.to_vec()
    } else {
        let mut v = args.to_vec();
        v.push(flag.to_string());
        v
    }
}

/// Return args with each flag from `flags` appended if not already present.
fn ensure_flags(args: &[String], flags: &[&str]) -> Vec<String> {
    let mut v = args.to_vec();
    for flag in flags {
        if !v.iter().any(|a| a == flag) {
            v.push(flag.to_string());
        }
    }
    v
}

/// Compact a tab-separated table (TYPE, KEY, SUMMARY, STATUS …).
///
/// The real `jira` CLI pads columns with repeated tabs, so the raw output
/// looks like: `TYPE\t\tKEY\t\tSUMMARY\t\t\t…\tSTATUS`.
/// We collapse consecutive tab runs so we always get the actual column values,
/// then truncate SUMMARY to `max_summary_len` chars.
pub fn filter_table_output(output: &str, max_summary_len: usize) -> String {
    let cleaned = strip_ansi(output);
    let mut result = Vec::new();

    for line in cleaned.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.contains('\t') {
            // Collapse consecutive tabs: split on one-or-more tabs to get real fields.
            let cols: Vec<&str> = trimmed.split('\t').filter(|s| !s.is_empty()).collect();

            // cols[0]=TYPE, cols[1]=KEY, cols[2]=SUMMARY, cols[3]=STATUS
            match cols.as_slice() {
                [typ, key, summary] => {
                    result.push(format!(
                        "{}\t{}\t{}",
                        typ,
                        key,
                        truncate(summary, max_summary_len)
                    ));
                }
                [typ, key, summary, rest @ ..] => {
                    // STATUS is the last non-empty field
                    let status = rest.last().copied().unwrap_or("").trim();
                    let summary_short = truncate(summary, max_summary_len);
                    if status.is_empty() {
                        result.push(format!("{}\t{}\t{}", typ, key, summary_short));
                    } else {
                        result.push(format!("{}\t{}\t{}\t{}", typ, key, summary_short, status));
                    }
                }
                _ => result.push(trimmed.to_string()),
            }
        } else {
            result.push(trimmed.to_string());
        }
    }

    result.join("\n")
}

/// Filter `jira issue list --plain` output.
pub fn filter_issue_list(output: &str) -> String {
    filter_table_output(output, 80)
}

/// Filter `jira issue view --plain` output.
/// Keeps key metadata lines and first ~10 description lines; strips ANSI + footer.
pub fn filter_issue_view(output: &str) -> String {
    let cleaned = strip_ansi(output);
    let mut result = Vec::new();
    let mut desc_lines = 0;
    let mut in_description = false;

    for line in cleaned.lines() {
        let trimmed = line.trim();

        // Skip pure blank lines
        if trimmed.is_empty() {
            continue;
        }

        // Skip lines containing the "View this issue on Jira" footer text.
        // jira-cli embeds this at the end of the watchers line (same line),
        // so we must use `contains` rather than `starts_with`.
        if trimmed.contains("View this issue on Jira:") {
            // Strip the footer portion but keep the rest of the line if it
            // has meaningful content before the footer.
            if let Some(pos) = trimmed.find("View this issue on Jira:") {
                let before = trimmed[..pos].trim();
                if !before.is_empty() {
                    result.push(before.to_string());
                }
            }
            continue;
        }

        // Skip the Jira URL line that follows the footer
        if trimmed.starts_with("https://") && trimmed.contains("atlassian.net/browse/") {
            continue;
        }

        // Detect description section
        if trimmed.contains("Description") && trimmed.contains("---") {
            in_description = true;
            result.push(trimmed.to_string());
            continue;
        }

        if in_description {
            if desc_lines >= 10 {
                continue;
            }
            desc_lines += 1;
        }

        result.push(trimmed.to_string());
    }

    result.join("\n")
}

/// Pass a jira command through verbatim, tracking as passthrough.
fn run_passthrough(subcommand: &str, args: &[String]) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut command = Command::new("jira");
    command.arg(subcommand);
    for arg in args {
        command.arg(arg);
    }

    let status = command
        .status()
        .context(format!("Failed to run jira {}", subcommand))?;

    let args_str = tracking::args_display(&args.iter().map(|s| s.into()).collect::<Vec<_>>());
    timer.track_passthrough(
        &format!("jira {} {}", subcommand, args_str),
        &format!("rtk jira {} {} (passthrough)", subcommand, args_str),
    );

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

/// Handle `jira issue <sub> <args>`.
fn run_issue(args: &[String], _verbose: u8) -> Result<()> {
    if args.is_empty() {
        return run_passthrough("issue", args);
    }

    match args[0].as_str() {
        "list" => issue_list(&args[1..]),
        "view" => issue_view(&args[1..]),
        // Mutation commands: passthrough with --no-input injected
        "create" | "edit" | "move" | "assign" | "comment" => {
            let patched = ensure_flag(args, "--no-input");
            run_passthrough("issue", &patched)
        }
        _ => run_passthrough("issue", args),
    }
}

fn issue_list(args: &[String]) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let patched = ensure_flag(args, "--plain");

    let mut cmd = Command::new("jira");
    cmd.arg("issue").arg("list");
    for arg in &patched {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run jira issue list")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        timer.track("jira issue list", "rtk jira issue list", &stderr, &stderr);
        eprintln!("{}", stderr.trim());
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let filtered = filter_issue_list(&raw);
    println!("{}", filtered);

    timer.track("jira issue list", "rtk jira issue list", &raw, &filtered);
    Ok(())
}

fn issue_view(args: &[String]) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    // Extract the issue key / id (first non-flag arg)
    let issue_key = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .cloned()
        .unwrap_or_default();

    let mut cmd = Command::new("jira");
    cmd.arg("issue").arg("view");
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run jira issue view")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        timer.track(
            &format!("jira issue view {}", issue_key),
            &format!("rtk jira issue view {}", issue_key),
            &stderr,
            &stderr,
        );
        eprintln!("{}", stderr.trim());
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let filtered = filter_issue_view(&raw);
    println!("{}", filtered);

    timer.track(
        &format!("jira issue view {}", issue_key),
        &format!("rtk jira issue view {}", issue_key),
        &raw,
        &filtered,
    );
    Ok(())
}

/// Handle `jira epic <sub> <args>`.
fn run_epic(args: &[String], _verbose: u8) -> Result<()> {
    if args.is_empty() {
        return run_passthrough("epic", args);
    }

    match args[0].as_str() {
        "list" => {
            let timer = tracking::TimedExecution::start();
            let patched = ensure_flags(&args[1..], &["--table", "--plain"]);

            let mut cmd = Command::new("jira");
            cmd.arg("epic").arg("list");
            for arg in &patched {
                cmd.arg(arg);
            }

            let output = cmd.output().context("Failed to run jira epic list")?;
            let raw = String::from_utf8_lossy(&output.stdout).to_string();

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                timer.track("jira epic list", "rtk jira epic list", &stderr, &stderr);
                eprintln!("{}", stderr.trim());
                std::process::exit(output.status.code().unwrap_or(1));
            }

            let filtered = filter_table_output(&raw, 80);
            println!("{}", filtered);

            timer.track("jira epic list", "rtk jira epic list", &raw, &filtered);
            Ok(())
        }
        _ => run_passthrough("epic", args),
    }
}

/// Handle `jira sprint <sub> <args>`.
fn run_sprint(args: &[String], _verbose: u8) -> Result<()> {
    if args.is_empty() {
        return run_passthrough("sprint", args);
    }

    match args[0].as_str() {
        "list" => {
            let timer = tracking::TimedExecution::start();
            // sprint list can list sprints or issues in a sprint (when sprint id given)
            // Both cases benefit from --plain; sprint-level listing also benefits from --table
            let rest = &args[1..];
            let patched = ensure_flags(rest, &["--table", "--plain"]);

            let mut cmd = Command::new("jira");
            cmd.arg("sprint").arg("list");
            for arg in &patched {
                cmd.arg(arg);
            }

            let output = cmd.output().context("Failed to run jira sprint list")?;
            let raw = String::from_utf8_lossy(&output.stdout).to_string();

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                timer.track("jira sprint list", "rtk jira sprint list", &stderr, &stderr);
                eprintln!("{}", stderr.trim());
                std::process::exit(output.status.code().unwrap_or(1));
            }

            let filtered = filter_table_output(&raw, 60);
            println!("{}", filtered);

            timer.track("jira sprint list", "rtk jira sprint list", &raw, &filtered);
            Ok(())
        }
        _ => run_passthrough("sprint", args),
    }
}

/// Public entry point called from main.rs.
pub fn run(subcommand: &str, args: &[String], verbose: u8) -> Result<()> {
    match subcommand {
        "issue" => run_issue(args, verbose),
        "epic" => run_epic(args, verbose),
        "sprint" => run_sprint(args, verbose),
        // `me` produces tiny output — pass through directly
        "me" => run_passthrough("me", args),
        _ => run_passthrough(subcommand, args),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── strip_ansi ──────────────────────────────────────────────────────────

    #[test]
    fn test_strip_ansi_basic() {
        let input = "\x1b[32mhello\x1b[0m world";
        assert_eq!(strip_ansi(input), "hello world");
    }

    #[test]
    fn test_strip_ansi_no_codes() {
        assert_eq!(strip_ansi("plain text"), "plain text");
    }

    #[test]
    fn test_strip_ansi_empty() {
        assert_eq!(strip_ansi(""), "");
    }

    // ── ensure_flag ────────────────────────────────────────────────────────

    #[test]
    fn test_ensure_flag_absent() {
        let args: Vec<String> = vec!["--project".into(), "ACME".into()];
        let result = ensure_flag(&args, "--plain");
        assert!(result.contains(&"--plain".to_string()));
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_ensure_flag_present() {
        let args: Vec<String> = vec!["--plain".into(), "--project".into(), "ACME".into()];
        let result = ensure_flag(&args, "--plain");
        assert_eq!(result.iter().filter(|a| *a == "--plain").count(), 1);
    }

    // ── ensure_flags ───────────────────────────────────────────────────────

    #[test]
    fn test_ensure_flags_adds_missing() {
        let args: Vec<String> = vec![];
        let result = ensure_flags(&args, &["--table", "--plain"]);
        assert!(result.contains(&"--table".to_string()));
        assert!(result.contains(&"--plain".to_string()));
    }

    #[test]
    fn test_ensure_flags_skips_existing() {
        let args: Vec<String> = vec!["--table".into()];
        let result = ensure_flags(&args, &["--table", "--plain"]);
        assert_eq!(result.iter().filter(|a| *a == "--table").count(), 1);
        assert!(result.contains(&"--plain".to_string()));
    }

    // ── filter_issue_list ──────────────────────────────────────────────────

    #[test]
    fn test_filter_issue_list_basic() {
        let input = "TYPE\t\tKEY\t\tSUMMARY\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\tSTATUS\nTask\t\tACME-101\tAdd retry logic to payment webhook handler\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\tBacklog\n";
        let result = filter_issue_list(input);
        assert!(result.contains("ACME-101"));
        assert!(result.contains("Backlog"));
        assert!(!result.is_empty());
    }

    #[test]
    fn test_filter_issue_list_truncates_summary() {
        // Summary longer than 80 chars
        let long_summary = "A".repeat(120);
        let input = format!("Task\tACME-1\t{}\tBacklog\n", long_summary);
        let result = filter_issue_list(&input);
        // The summary in output should be ≤ 80+3 chars (truncated with "...")
        let parts: Vec<&str> = result.splitn(4, '\t').collect();
        if parts.len() >= 3 {
            assert!(parts[2].len() <= 83, "summary too long: {}", parts[2].len());
        }
    }

    #[test]
    fn test_filter_issue_list_strips_empty_lines() {
        let input = "TYPE\tKEY\tSUMMARY\tSTATUS\n\n\nTask\tACME-1\tFoo\tBacklog\n";
        let result = filter_issue_list(input);
        assert!(!result.contains("\n\n"), "empty lines not stripped");
    }

    #[test]
    fn test_filter_issue_list_real_fixture() {
        // Simulated output from jira issue list --plain (tabs between cols)
        let input = "TYPE\t\tKEY\t\tSUMMARY\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\tSTATUS\nTask\t\tACME-101\tAdd retry logic to payment webhook handler\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\tBacklog\nTask\t\tACME-102\tUpgrade database connection pool to v3\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\tBacklog\nTask\t\tACME-103\tFix flaky integration test for auth module\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\tBacklog\n";
        let result = filter_issue_list(input);
        assert!(result.contains("ACME-101"));
        assert!(result.contains("ACME-102"));
        assert!(result.contains("ACME-103"));
        assert!(result.contains("Backlog"));

        // Character-level savings: padding tabs are collapsed
        let char_savings = 100.0 - (result.len() as f64 / input.len() as f64 * 100.0);
        assert!(
            char_savings >= 20.0,
            "Expected ≥20% character savings, got {:.1}% (in:{} out:{})",
            char_savings,
            input.len(),
            result.len()
        );
    }

    // ── filter_issue_view ──────────────────────────────────────────────────

    #[test]
    fn test_filter_issue_view_strips_ansi() {
        let input = "\x1b[32m⭐ Task\x1b[0m  🚧 Backlog  ACME-101\n\n# Title\n";
        let result = filter_issue_view(input);
        assert!(!result.contains('\x1b'), "ANSI codes not stripped");
        assert!(result.contains("ACME-101"));
    }

    #[test]
    fn test_filter_issue_view_strips_footer() {
        let input = "# Add retry logic to payment webhook handler\n\nView this issue on Jira: https://example.atlassian.net/browse/ACME-101\n";
        let result = filter_issue_view(input);
        assert!(
            !result.contains("View this issue on Jira:"),
            "footer not stripped"
        );
        assert!(result.contains("Add retry logic to payment webhook handler"));
    }

    #[test]
    fn test_filter_issue_view_strips_footer_midline() {
        // jira-cli embeds the footer at the end of the watchers line
        let input = "1 watchers View this issue on Jira: https://example.atlassian.net/browse/ACME-101\nhttps://example.atlassian.net/browse/ACME-101\n";
        let result = filter_issue_view(input);
        assert!(
            !result.contains("View this issue on Jira:"),
            "midline footer not stripped"
        );
        assert!(
            !result.contains("atlassian.net/browse/"),
            "URL line not stripped"
        );
        assert!(result.contains("1 watchers"), "content before footer lost");
    }

    #[test]
    fn test_filter_issue_view_strips_blank_lines() {
        let input = "Line 1\n\n\n\nLine 2\n";
        let result = filter_issue_view(input);
        assert!(!result.contains("\n\n"), "consecutive blanks remain");
        assert!(result.contains("Line 1"));
        assert!(result.contains("Line 2"));
    }

    #[test]
    fn test_filter_issue_view_real_fixture() {
        // Simulated jira issue view output
        let input = concat!(
            "  \u{2b50} Task  \u{1f6a7} Backlog  \u{231b} Mon, 10 Mar 26  \u{1f477} Unassigned  ",
            "\u{1f511}\u{fe0f} ACME-101  \u{1f4ad} 0 comments  \u{1f9f5} 0 linked\n",
            "                                                                                                                      \n",
            "  # Add retry logic to payment webhook handler\n",
            "                                                                                                                      \n",
            "  \u{23f1}\u{fe0f}  Mon, 10 Mar 26  \u{1f50e} Jane Smith  \u{1f680} Medium\n",
            "                                                                                                                      \n",
            "  ------------------------ Description ------------------------\n",
            "                                                                                                                      \n",
            "  The payment webhook currently fails silently on timeout\n",
            "                                                                                                                      \n",
            "  View this issue on Jira: https://example.atlassian.net/browse/ACME-101\n",
        );
        let result = filter_issue_view(input);
        assert!(result.contains("ACME-101"));
        assert!(result.contains("Add retry logic to payment webhook handler"));
        assert!(!result.contains("View this issue on Jira:"));

        // Character-level savings: blank-padding lines and footer are stripped
        let char_savings = 100.0 - (result.len() as f64 / input.len() as f64 * 100.0);
        assert!(
            char_savings >= 50.0,
            "Expected ≥50% character savings, got {:.1}% (in:{} out:{})",
            char_savings,
            input.len(),
            result.len()
        );
    }

    // ── filter_table_output (epic/sprint) ──────────────────────────────────

    #[test]
    fn test_filter_table_output_sprint_list() {
        let input = "ID\tNAME\t\t\tSTART\t\t\tEND\t\t\t\tCOMPLETE\t\tSTATE\n34036\tQ1 Sprint 3\t\t2026-01-29 03:33:51\t2026-02-12 00:00:00\t\t\t\tactive\n34035\tQ1 Sprint 2\t\t2026-01-15 10:01:10\t2026-01-29 00:00:00\t2026-01-30 03:33:23\tclosed\n";
        let result = filter_table_output(input, 60);
        assert!(result.contains("Q1 Sprint 3"));
        assert!(result.contains("Q1 Sprint 2"));
        assert!(result.contains("active"));
        assert!(!result.contains("\n\n"));
    }

    #[test]
    fn test_filter_table_output_epic_list() {
        let input = "TYPE\tKEY\t\tSUMMARY\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\tSTATUS\nEpic\tACME-50\tMigrate user auth to OAuth2\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\tDone\nEpic\tACME-51\tImplement real-time notification service\t\t\t\t\t\t\t\t\t\t\t\t\t\t\t\tIn Progress\n";
        let result = filter_table_output(input, 80);
        assert!(result.contains("ACME-50"));
        assert!(result.contains("ACME-51"));
        assert!(result.contains("Done"));
        assert!(result.contains("In Progress"));
    }

    #[test]
    fn test_filter_table_output_strips_ansi() {
        let input =
            "\x1b[1mTYPE\x1b[0m\tKEY\tSUMMARY\tSTATUS\n\x1b[32mTask\x1b[0m\tACME-1\tFoo\tDone\n";
        let result = filter_table_output(input, 80);
        assert!(!result.contains('\x1b'));
        assert!(result.contains("ACME-1"));
    }
}
