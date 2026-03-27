use crate::core::tee;
use crate::core::tracking;
use crate::core::utils::strip_ansi;
use anyhow::{Context, Result};
use std::process::Command;

pub fn run_review(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let args_str = args.join(" ");

    let mut cmd = Command::new("codex");
    cmd.arg("review");
    cmd.args(args);

    if verbose > 0 {
        eprintln!("Running: {}", format_command("codex review", &args_str));
    }

    let output = cmd.output().context("Failed to run codex review")?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let raw = join_non_empty(&stderr, &stdout);
    let exit_code = output.status.code().unwrap_or(1);
    let filtered = select_filtered_review_output(&stdout, &stderr);

    if let Some(hint) = tee::tee_and_hint(&raw, "codex", exit_code) {
        if filtered.is_empty() {
            println!("{}", hint);
        } else {
            println!("{}\n{}", filtered, hint);
        }
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format_command("codex review", &args_str),
        &format_command("rtk codex review", &args_str),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

fn format_command(base: &str, args: &str) -> String {
    if args.is_empty() {
        base.to_string()
    } else {
        format!("{base} {args}")
    }
}

fn join_non_empty(primary: &str, secondary: &str) -> String {
    match (primary.trim_end(), secondary.trim_end()) {
        ("", "") => String::new(),
        (primary, "") => primary.to_string(),
        ("", secondary) => secondary.to_string(),
        (primary, secondary) => format!("{primary}\n{secondary}"),
    }
}

fn select_filtered_review_output(stdout: &str, stderr: &str) -> String {
    let cleaned_stderr = strip_ansi(stderr);
    let stderr_result = filter_codex_review_output(&cleaned_stderr);
    if stderr_result.reached_body {
        return stderr_result.output;
    }

    let cleaned_stdout = strip_ansi(stdout);
    let stdout_result = filter_codex_review_output(&cleaned_stdout);
    if stdout_result.reached_body {
        return stdout_result.output;
    }

    let combined = join_non_empty(&cleaned_stderr, &cleaned_stdout);
    filter_codex_review_output(&combined).output
}

#[derive(Debug, PartialEq, Eq)]
struct FilterResult {
    output: String,
    reached_body: bool,
}

/// Filter `codex review` output by stripping the preamble and keeping only the
/// final review text once the structured `codex` section begins.
fn filter_codex_review_output(output: &str) -> FilterResult {
    let lines: Vec<&str> = output.lines().collect();

    if let Some(body_start) = find_review_body_start(&lines) {
        let body_lines = lines[body_start..]
            .iter()
            .filter(|line| !is_internal_log_line(line.trim()))
            .map(|line| (*line).to_string())
            .collect();

        return FilterResult {
            output: trim_blank_lines(body_lines),
            reached_body: true,
        };
    }

    FilterResult {
        output: output.trim().to_string(),
        reached_body: false,
    }
}

fn find_review_body_start(lines: &[&str]) -> Option<usize> {
    let mut saw_user = false;

    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        if trimmed.is_empty() || is_internal_log_line(trimmed) || is_preamble_line(trimmed) {
            if trimmed == "user" {
                saw_user = true;
            }
            continue;
        }

        if !saw_user {
            return None;
        }

        if trimmed != "codex" {
            continue;
        }

        let next_nonempty = next_nonempty_line(lines, index + 1);
        if next_nonempty.is_none() || next_nonempty.is_some_and(looks_like_review_line) {
            return Some(index + 1);
        }
    }

    None
}

fn next_nonempty_line<'a>(lines: &'a [&'a str], start: usize) -> Option<&'a str> {
    lines[start..]
        .iter()
        .map(|line| line.trim())
        .find(|line| !line.is_empty() && !is_internal_log_line(line))
}

fn trim_blank_lines(lines: Vec<String>) -> String {
    let start = lines
        .iter()
        .position(|line| !line.trim().is_empty())
        .unwrap_or(lines.len());
    let end = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map(|index| index + 1)
        .unwrap_or(start);

    if start >= end {
        String::new()
    } else {
        lines[start..end].join("\n")
    }
}

fn is_preamble_line(line: &str) -> bool {
    matches!(line, "user" | "assistant")
        || is_separator_line(line)
        || line.starts_with("OpenAI Codex")
        || line.starts_with("workdir:")
        || line.starts_with("model:")
        || line.starts_with("provider:")
        || line.starts_with("approval:")
        || line.starts_with("sandbox:")
        || line.starts_with("reasoning effort:")
        || line.starts_with("reasoning summaries:")
        || line.starts_with("session id:")
        || line.starts_with("mcp startup:")
}

fn is_separator_line(line: &str) -> bool {
    line.len() >= 3 && line.chars().all(|char| char == '-')
}

fn looks_like_review_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("- [P")
        || trimmed.starts_with("- [N")
        || trimmed.starts_with("Review")
        || trimmed.starts_with("No issues")
        || trimmed.starts_with("The ")
        || trimmed.starts_with("This ")
        || trimmed.starts_with("There ")
        || trimmed.starts_with("Looks ")
        || trimmed.starts_with("I ")
        || trimmed.starts_with("Potential ")
        || trimmed.starts_with("Consider ")
        || trimmed.starts_with("Minor ")
        || trimmed.starts_with("Final ")
}

fn is_internal_log_line(line: &str) -> bool {
    line.starts_with("WARNING: failed to clean up stale arg0 temp dirs:")
        || line.starts_with("WARNING: proceeding, even though we could not update PATH:")
        || looks_like_timestamped_codex_log(line)
}

fn looks_like_timestamped_codex_log(line: &str) -> bool {
    line.len() > 25
        && line.as_bytes().get(4) == Some(&b'-')
        && line.as_bytes().get(7) == Some(&b'-')
        && line.contains(" codex_")
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAW_FIXTURE: &str = include_str!("../../../tests/fixtures/codex_review_raw.txt");

    #[test]
    fn test_filter_codex_review_output_real_fixture() {
        let result = filter_codex_review_output(RAW_FIXTURE);

        assert!(result.reached_body);
        assert!(!result.output.contains("OpenAI Codex"));
        assert!(!result.output.contains("workdir:"));
        assert!(!result.output.contains("mcp startup:"));
        assert!(!result.output.contains("exec"));
        assert!(!result.output.contains("/usr/bin/zsh"));
        assert!(result
            .output
            .contains("The change regresses `update_settings`"));
        assert!(result
            .output
            .contains("[P1] Read the settings file before opening it in write mode"));
    }

    #[test]
    fn test_select_filtered_review_output_prefers_stderr_body() {
        let stdout = "Codex CLI\n\nUsage: codex review [OPTIONS] [PROMPT]\n";
        let filtered = select_filtered_review_output(stdout, RAW_FIXTURE);

        assert!(filtered.contains("[P1] Read the settings file before opening it in write mode"));
        assert!(!filtered.contains("Usage:"));
    }

    #[test]
    fn test_filter_codex_output_simple_review() {
        let output = r#"OpenAI Codex v0.111.0 (research preview)
--------
workdir: /tmp/test
model: gpt-5.3-codex
provider: openai
approval: never
sandbox: read-only
reasoning effort: xhigh
reasoning summaries: none
session id: abc-123
--------
user
changes against 'main'
codex
No issues found. The code looks clean.
"#;
        let result = filter_codex_review_output(output);
        assert!(result.reached_body);
        assert_eq!(result.output, "No issues found. The code looks clean.");
    }

    #[test]
    fn test_filter_codex_skips_prompt_line_named_codex() {
        let output = r#"OpenAI Codex v0.111.0 (research preview)
--------
session id: abc-123
--------
user
changes against 'main'
codex
lowercase prompt continuation
codex
There are 2 files in the project: file1.txt and file2.txt.
"#;
        let result = filter_codex_review_output(output);
        assert!(result.reached_body);
        assert!(!result.output.contains("lowercase prompt continuation"));
        assert!(result.output.contains("There are 2 files"));
    }

    #[test]
    fn test_filter_codex_fallback_to_raw_for_help() {
        let output =
            "Run a code review non-interactively\n\nUsage: codex review [OPTIONS] [PROMPT]\n";
        let result = filter_codex_review_output(output);

        assert!(!result.reached_body);
        assert!(result.output.contains("Usage: codex review"));
    }

    #[test]
    fn test_filter_codex_output_empty() {
        let result = filter_codex_review_output("\n\n\n");
        assert_eq!(
            result,
            FilterResult {
                output: String::new(),
                reached_body: false,
            }
        );
    }

    #[test]
    fn test_filter_empty_body_returns_empty_string() {
        let output = r#"OpenAI Codex v0.111.0 (research preview)
--------
session id: abc-123
--------
user
changes against 'main'
codex
"#;
        let result = filter_codex_review_output(output);
        assert!(result.reached_body);
        assert!(result.output.is_empty());
    }

    #[test]
    fn test_token_savings_real_fixture() {
        fn count_tokens(text: &str) -> usize {
            text.split_whitespace().count()
        }

        let output = filter_codex_review_output(RAW_FIXTURE);
        let input_tokens = count_tokens(RAW_FIXTURE);
        let output_tokens = count_tokens(&output.output);
        let reduction = 100.0 * (1.0 - output_tokens as f64 / input_tokens as f64);

        assert!(
            reduction >= 60.0,
            "Token reduction {:.1}% should be >= 60%",
            reduction
        );
    }
}
