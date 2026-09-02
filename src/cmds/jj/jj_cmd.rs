//! Jujutsu (jj) CLI — flag injection and compact output for agent workflows.

use crate::cmds::git::git::compact_diff;
use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::truncate::{reduced, CAP_LIST};
use crate::core::utils::{resolved_command, strip_ansi, truncate};
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use std::ffi::OsString;
use std::process::Command;

const DEFAULT_LOG_LIMIT: usize = 10;
const MAX_LOG_ENTRIES: usize = reduced(CAP_LIST, 5);

lazy_static! {
    static ref EMAIL_RE: Regex =
        Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b").unwrap();
    static ref HINT_LINE_RE: Regex = Regex::new(r"^Hint:").unwrap();
    static ref WC_NOW_AT_RE: Regex = Regex::new(r"^Working copy now at:").unwrap();
    static ref GRAPH_NODE_RE: Regex = Regex::new(r"^[@○◆◇◉]").unwrap();
    static ref OP_NOISE_RE: Regex =
        Regex::new(r"\s+lasted \d+ milliseconds").unwrap();
}

fn jj_cmd() -> Command {
    let mut cmd = resolved_command("jj");
    cmd.arg("--color").arg("never");
    cmd
}

fn has_flag(args: &[String], names: &[&str]) -> bool {
    args.iter().any(|a| names.iter().any(|n| a == *n || a.starts_with(&format!("{n}="))))
}

fn wants_no_compact(args: &[String]) -> bool {
    has_flag(args, &["--no-compact"])
}

fn strip_rtk_flags(args: &[String]) -> Vec<String> {
    args.iter()
        .filter(|a| a.as_str() != "--no-compact")
        .cloned()
        .collect()
}

fn has_user_template(args: &[String]) -> bool {
    has_flag(args, &["-T", "--template"])
}

fn has_no_graph(args: &[String]) -> bool {
    has_flag(args, &["-G", "--no-graph"])
}

fn parse_limit(args: &[String]) -> Option<usize> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if let Some(rest) = arg.strip_prefix("-n") {
            if rest.is_empty() {
                if let Some(next) = iter.next() {
                    return next.parse().ok();
                }
            } else if let Ok(n) = rest.parse::<usize>() {
                return Some(n);
            }
        }
        if let Some(rest) = arg.strip_prefix("--limit=") {
            return rest.parse().ok();
        }
        if arg == "--limit" {
            if let Some(next) = iter.next() {
                return next.parse().ok();
            }
        }
    }
    None
}

fn inject_log_args(args: &[String]) -> Vec<String> {
    let mut out = strip_rtk_flags(args);
    if has_user_template(&out) {
        if !has_flag(&out, &["--color", "-n", "--limit"]) {
            // Respect custom template but keep machine-safe defaults.
            if parse_limit(&out).is_none() {
                out.push("-n".to_string());
                out.push(DEFAULT_LOG_LIMIT.to_string());
            }
        }
        return out;
    }
    if has_no_graph(&out) && parse_limit(&out).is_some() {
        return out;
    }
    if parse_limit(&out).is_none() {
        out.push("-n".to_string());
        out.push(DEFAULT_LOG_LIMIT.to_string());
    }
    out.push("-T".to_string());
    out.push("builtin_log_oneline".to_string());
    out
}

fn inject_diff_args(args: &[String]) -> Vec<String> {
    let out = strip_rtk_flags(args);
    if has_flag(
        &out,
        &[
            "--summary",
            "-s",
            "--git",
            "--stat",
            "--name-only",
            "--types",
            "-p",
            "--patch",
        ],
    ) {
        return out;
    }
    let mut v = out;
    v.push("--summary".to_string());
    v
}

fn inject_show_args(args: &[String]) -> Vec<String> {
    let out = strip_rtk_flags(args);
    if has_flag(
        &out,
        &[
            "-s",
            "--summary",
            "--git",
            "--stat",
            "--name-only",
            "--types",
            "-p",
            "--patch",
        ],
    ) {
        return out;
    }
    let mut v = out;
    v.push("-s".to_string());
    v
}

fn inject_op_log_args(args: &[String]) -> Vec<String> {
    let out = strip_rtk_flags(args);
    if out.first().map(|s| s.as_str()) != Some("log") {
        return out;
    }
    if parse_limit(&out).is_none() {
        let mut v = out;
        v.push("-n".to_string());
        v.push(DEFAULT_LOG_LIMIT.to_string());
        return v;
    }
    out
}

fn filter_common(input: &str) -> String {
    let clean = strip_ansi(input.trim());
    clean
        .lines()
        .filter(|line| {
            let t = line.trim();
            !t.is_empty() && !HINT_LINE_RE.is_match(t) && !WC_NOW_AT_RE.is_match(t)
        })
        .map(|line| EMAIL_RE.replace_all(line, "").trim().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Post-filter line cap. `None` = user set `-n`/`--limit` (jj already bounded).
fn log_post_cap(args: &[String]) -> Option<usize> {
    if parse_limit(args).is_some() {
        return None;
    }
    Some(MAX_LOG_ENTRIES)
}

fn apply_line_cap(lines: Vec<String>, max_entries: Option<usize>) -> String {
    let cap = match max_entries {
        None => return lines.join("\n"),
        Some(n) => n,
    };
    if lines.len() <= cap {
        return lines.join("\n");
    }
    let omitted = lines.len() - cap;
    let mut out: Vec<String> = lines.into_iter().take(cap).collect();
    out.push(format!("... +{} more revisions", omitted));
    out.join("\n")
}

fn filter_log_oneline(input: &str, max_entries: Option<usize>) -> String {
    let lines: Vec<String> = filter_common(input)
        .lines()
        .map(|l| truncate(l, 120))
        .filter(|l| !l.contains("root()"))
        .collect();
    apply_line_cap(lines, max_entries)
}

fn filter_log_graph(input: &str, max_entries: Option<usize>) -> String {
    let cap = max_entries.unwrap_or(usize::MAX);
    let stripped = strip_ansi(input.trim());
    let lines: Vec<&str> = stripped.lines().collect();
    let mut result = Vec::new();
    let mut entry_count = 0;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() || HINT_LINE_RE.is_match(trimmed) || trimmed.contains("root()") {
            continue;
        }
        if GRAPH_NODE_RE.is_match(trimmed) {
            entry_count += 1;
            if entry_count > cap {
                continue;
            }
            let compressed = EMAIL_RE.replace_all(trimmed, "");
            result.push(truncate(compressed.trim(), 120));
        } else if trimmed.starts_with('│') || trimmed.starts_with('|') {
            if let Some(last) = result.last_mut() {
                let desc = trimmed.trim_start_matches(['│', '|']).trim();
                if !desc.is_empty() {
                    *last = format!("{} | {}", last, truncate(desc, 80));
                }
            }
        } else if entry_count > 0 && entry_count <= cap {
            result.push(truncate(EMAIL_RE.replace_all(trimmed, "").trim(), 120));
        }
    }

    if entry_count > cap {
        result.push(format!("... +{} more revisions", entry_count - cap));
    }
    result.join("\n")
}

fn filter_status(input: &str) -> String {
    let mut changes = Vec::new();
    let mut at_line = String::new();
    let mut at_minus = String::new();

    for line in strip_ansi(input).lines() {
        let t = line.trim();
        if t.is_empty() || HINT_LINE_RE.is_match(t) {
            continue;
        }
        if t == "Working copy changes:" {
            continue;
        }
        if t.starts_with("Working copy  (@)") || t.starts_with("Working copy (@)") {
            at_line = t
                .replace("Working copy  (@)", "@")
                .replace("Working copy (@)", "@")
                .replace(" : ", " ")
                .replace(": ", " ");
        } else if t.starts_with("Parent commit (@-)") || t.starts_with("Parent commit (@-):") {
            at_minus = t
                .replace("Parent commit (@-):", "@-")
                .replace("Parent commit (@-)", "@-")
                .replace(" : ", " ")
                .replace(": ", " ");
        } else if !t.starts_with("Working copy") {
            changes.push(t.to_string());
        }
    }

    let mut out = Vec::new();
    out.extend(changes);
    if !at_line.is_empty() {
        out.push(at_line);
    }
    if !at_minus.is_empty() {
        out.push(at_minus);
    }
    if out.is_empty() {
        return filter_common(input);
    }
    out.join("\n")
}

fn filter_op_log(input: &str, max_entries: Option<usize>) -> String {
    let lines: Vec<String> = filter_common(input)
        .lines()
        .map(|line| OP_NOISE_RE.replace_all(line, "").trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    apply_line_cap(lines, max_entries)
}

/// When post-filter caps lines, append a tee tail hint so the agent can recover hidden revisions.
fn maybe_append_tail_hint(output: &str, raw: &str, slug: &str, line_cap: Option<usize>) -> String {
    let Some(cap) = line_cap.filter(|_| output.contains("more revisions")) else {
        return output.to_string();
    };
    match crate::core::tee::force_tee_tail_hint(raw, slug, cap + 1) {
        Some(hint) if output.is_empty() => hint,
        Some(hint) => format!("{output}\n{hint}"),
        None => output.to_string(),
    }
}

fn run_jj(
    subcmd: &[&str],
    verbose: u8,
    tee_label: &str,
    jj_args: Vec<String>,
    line_cap: Option<usize>,
    filter_fn: impl Fn(&str) -> String,
) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = jj_cmd();
    for part in subcmd {
        cmd.arg(part);
    }
    for arg in &jj_args {
        cmd.arg(arg);
    }

    let subcmd_str = subcmd.join(" ");
    if verbose > 0 {
        eprintln!("Running: jj {} {}", subcmd_str, jj_args.join(" "));
    }

    let cmd_output = exec_capture(&mut cmd).with_context(|| {
        format!(
            "Failed to run jj {}. Is jj installed?",
            subcmd_str
        )
    })?;

    let raw = format!("{}\n{}", cmd_output.stdout, cmd_output.stderr);
    let clean = strip_ansi(cmd_output.stdout.trim());
    let mut output = if verbose > 0 {
        clean.clone()
    } else {
        filter_fn(&clean)
    };
    if verbose == 0 {
        output = maybe_append_tail_hint(&output, &clean, tee_label, line_cap);
    }

    if let Some(hint) = crate::core::tee::tee_and_hint(&raw, tee_label, cmd_output.exit_code) {
        println!("{}\n{}", output, hint);
    } else {
        println!("{}", output);
    }

    if !cmd_output.stderr.trim().is_empty() {
        eprintln!("{}", strip_ansi(cmd_output.stderr.trim()));
    }

    let label = if jj_args.is_empty() {
        format!("jj {}", subcmd_str)
    } else {
        format!("jj {} {}", subcmd_str, jj_args.join(" "))
    };
    timer.track(&label, &format!("rtk {}", label), &raw, &output);

    Ok(cmd_output.exit_code)
}

pub fn run_log(args: &[String], verbose: u8) -> Result<i32> {
    let passthrough = wants_no_compact(args);
    let jj_args = if passthrough {
        strip_rtk_flags(args)
    } else {
        inject_log_args(args)
    };
    let stripped = strip_rtk_flags(args);
    let cap = log_post_cap(&stripped);
    run_jj(
        &["log"],
        verbose,
        "jj_log",
        jj_args,
        cap,
        move |out| {
            if passthrough && !has_user_template(&stripped) {
                filter_log_graph(out, cap)
            } else {
                filter_log_oneline(out, cap)
            }
        },
    )
}

pub fn run_status(args: &[String], verbose: u8) -> Result<i32> {
    let jj_args = strip_rtk_flags(args);
    let filter = if wants_no_compact(args) {
        filter_common
    } else {
        filter_status
    };
    run_jj(&["status"], verbose, "jj_status", jj_args, None, filter)
}

pub fn run_diff(args: &[String], verbose: u8) -> Result<i32> {
    let passthrough = wants_no_compact(args);
    let jj_args = if passthrough {
        strip_rtk_flags(args)
    } else {
        inject_diff_args(args)
    };
    let use_git_diff = has_flag(&jj_args, &["--git"]);

    if use_git_diff && !passthrough {
        let timer = tracking::TimedExecution::start();
        let mut cmd = jj_cmd();
        cmd.arg("diff");
        for arg in &jj_args {
            cmd.arg(arg);
        }
        let result = exec_capture(&mut cmd).context("Failed to run jj diff --git")?;
        let raw = format!("{}\n{}", result.stdout, result.stderr);
        let compacted = compact_diff(&result.stdout, 500);
        let output = if verbose > 0 {
            result.stdout.trim().to_string()
        } else {
            compacted
        };
        if let Some(hint) = crate::core::tee::tee_and_hint(&raw, "jj_diff_git", result.exit_code) {
            println!("{}\n{}", output, hint);
        } else {
            println!("{}", output);
        }
        if !result.stderr.trim().is_empty() {
            eprintln!("{}", strip_ansi(result.stderr.trim()));
        }
        let label = format!("jj diff {}", jj_args.join(" "));
        timer.track(&label, &format!("rtk {}", label), &raw, &output);
        return Ok(result.exit_code);
    }

    run_jj(&["diff"], verbose, "jj_diff", jj_args, None, filter_common)
}

pub fn run_show(args: &[String], verbose: u8) -> Result<i32> {
    let jj_args = if wants_no_compact(args) {
        strip_rtk_flags(args)
    } else {
        inject_show_args(args)
    };
    run_jj(&["show"], verbose, "jj_show", jj_args, None, filter_common)
}

pub fn run_op(args: &[String], verbose: u8) -> Result<i32> {
    let jj_args = if wants_no_compact(args) {
        strip_rtk_flags(args)
    } else {
        inject_op_log_args(args)
    };
    let cap = log_post_cap(args);
    run_jj(
        &["op"],
        verbose,
        "jj_op",
        jj_args,
        cap,
        move |out| {
            if args.first().map(|s| s.as_str()) == Some("log") && !wants_no_compact(args) {
                filter_op_log(out, cap)
            } else {
                filter_common(out)
            }
        },
    )
}

pub fn run_other(args: &[OsString], verbose: u8) -> Result<i32> {
    if args.is_empty() {
        anyhow::bail!("jj: no subcommand specified");
    }
    let sub: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let head = sub[0].as_str();
    let rest = &sub[1..];
    match head {
        "log" | "l" => run_log(rest, verbose),
        "status" | "st" => run_status(rest, verbose),
        "diff" | "d" => run_diff(rest, verbose),
        "show" => run_show(rest, verbose),
        "op" => run_op(rest, verbose),
        _ => {
            let os_args: Vec<OsString> = sub.iter().map(OsString::from).collect();
            crate::core::runner::run_passthrough("jj", &os_args, verbose)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    #[test]
    fn test_inject_log_defaults() {
        let args = inject_log_args(&[]);
        assert!(args.iter().any(|a| a == "-T"));
        assert!(args.iter().any(|a| a == "builtin_log_oneline"));
        assert!(args.iter().any(|a| a == "-n"));
    }

    #[test]
    fn test_inject_log_respects_user_limit() {
        let args = inject_log_args(&["-n".into(), "50".into()]);
        assert!(args.windows(2).any(|w| w == ["-n", "50"]));
        assert!(!args.contains(&DEFAULT_LOG_LIMIT.to_string()));
    }

    #[test]
    fn test_log_post_cap_respects_user_limit() {
        assert_eq!(log_post_cap(&["-n".into(), "50".into()]), None);
        assert_eq!(log_post_cap(&[]), Some(MAX_LOG_ENTRIES));
    }

    #[test]
    fn test_filter_log_no_post_cap_when_user_limit() {
        let lines: Vec<String> = (0..20)
            .map(|i| format!("@ abc{i} user 2020-01-01 commit{i} msg{i}"))
            .collect();
        let big = lines.join("\n");
        let capped = filter_log_oneline(&big, Some(5));
        let full = filter_log_oneline(&big, None);
        assert!(capped.contains("+15 more"));
        assert!(!full.contains("more revisions"));
    }

    #[test]
    fn test_filter_log_default_savings() {
        let input = include_str!("../../../tests/fixtures/jj/002-log-default_raw.txt");
        let output = filter_log_graph(input, Some(MAX_LOG_ENTRIES));
        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 15.0,
            "graph log filter: expected ≥15% savings, got {:.1}%",
            savings
        );
        assert!(output.lines().count() <= input.lines().count());
    }

    #[test]
    fn test_filter_log_oneline_drops_root() {
        let input = include_str!("../../../tests/fixtures/jj/003-log-oneline_raw.txt");
        let output = filter_log_oneline(input, Some(MAX_LOG_ENTRIES));
        assert!(!output.contains("root()"));
        assert!(output.contains("docs: readme"));
    }

    #[test]
    fn test_filter_status_savings() {
        let input = include_str!("../../../tests/fixtures/jj/001-status_raw.txt");
        let output = filter_status(input);
        assert!(output.contains('@'));
        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 15.0,
            "status filter: expected ≥15% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_strip_no_compact() {
        let args = strip_rtk_flags(&["log".into(), "--no-compact".into(), "-n".into(), "3".into()]);
        assert!(!args.contains(&"--no-compact".to_string()));
        assert!(args.contains(&"-n".to_string()));
    }
}