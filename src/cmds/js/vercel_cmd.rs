//! Filters Vercel CLI output to keep deploy summaries and compact project listings.

use crate::core::runner;
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;

const MAX_OUTPUT_LINES: usize = 40;

lazy_static! {
    static ref TABLE_SPLIT_RE: Regex = Regex::new(r"\s{2,}").unwrap();
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("vercel");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        if args.is_empty() {
            eprintln!("Running: vercel");
        } else {
            eprintln!("Running: vercel {}", args.join(" "));
        }
    }

    runner::run_filtered(
        cmd,
        "vercel",
        &args.join(" "),
        filter_vercel_output,
        runner::RunOptions::with_tee("vercel"),
    )
}

pub fn filter_vercel_output(output: &str) -> String {
    let normalized = strip_ansi(output).replace('\r', "\n");
    let mut lines = Vec::new();
    let mut in_project_table = false;

    for raw_line in normalized.lines() {
        let line = raw_line.trim();
        if line.is_empty() || is_border_line(line) {
            continue;
        }

        if is_project_table_header(line) {
            in_project_table = true;
        }

        if in_project_table {
            if let Some(table_row) = compress_table_row(line) {
                push_unique(&mut lines, table_row);
                continue;
            }
            in_project_table = false;
        }

        if should_keep_line(line) {
            push_unique(&mut lines, collapse_whitespace(line));
            continue;
        }

        if is_spinner_line(line) || is_progress_line(line) {
            continue;
        }

        if let Some(line) = strip_status_prefix(line).filter(|line| should_keep_line(line)) {
            push_unique(&mut lines, collapse_whitespace(line));
        }
    }

    if lines.is_empty() {
        return "vercel: ok".to_string();
    }

    if lines.len() > MAX_OUTPUT_LINES {
        let omitted = lines.len() - MAX_OUTPUT_LINES;
        lines.truncate(MAX_OUTPUT_LINES);
        lines.push(format!("... +{} more lines omitted", omitted));
    }

    lines.join("\n")
}

fn push_unique(lines: &mut Vec<String>, line: String) {
    if lines.last().map(|last| last == &line).unwrap_or(false) {
        return;
    }
    lines.push(line);
}

fn compress_table_row(line: &str) -> Option<String> {
    if !line.contains("  ") && !line.contains('\t') {
        return None;
    }

    let parts: Vec<&str> = TABLE_SPLIT_RE
        .split(line)
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();

    if parts.len() < 2 {
        return None;
    }

    Some(parts.join(" | "))
}

fn is_project_table_header(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.starts_with("name") && lower.contains("production") && lower.contains("updated")
}

fn should_keep_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();

    if line.contains("http://") || line.contains("https://") {
        return true;
    }

    if lower.starts_with("error")
        || lower.starts_with("failed")
        || lower.starts_with("failure")
        || lower.starts_with("fatal")
        || lower.starts_with("warning")
        || lower.starts_with("production:")
        || lower.starts_with("preview:")
        || lower.starts_with("inspect:")
        || lower.starts_with("status:")
        || lower.starts_with("ready")
        || lower.starts_with("deployed")
        || lower.starts_with("completed")
        || lower.starts_with("success")
        || lower.starts_with("project:")
        || lower.starts_with("projects:")
        || lower.starts_with("alias:")
        || lower.starts_with("domain:")
    {
        return true;
    }

    lower.contains("deployment") || lower.contains("project list") || lower.contains("project")
}

fn strip_status_prefix(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let idx = trimmed
        .char_indices()
        .find_map(|(idx, ch)| ch.is_ascii_alphabetic().then_some(idx))?;
    Some(trimmed[idx..].trim_start())
}

fn is_spinner_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }

    let spinner_prefix = matches!(
        trimmed.chars().next(),
        Some('⠋' | '⠙' | '⠹' | '⠸' | '⠼' | '⠴' | '⠦' | '⠧' | '⠇' | '⠏' | '◐' | '◓' | '◑' | '◒')
    );

    spinner_prefix || (trimmed.len() <= 3 && matches!(trimmed, "|" | "/" | "-" | "\\"))
}

fn is_progress_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    if lower.contains('%') || lower.contains("progress") {
        return true;
    }

    let noise_prefixes = [
        "building",
        "bundling",
        "collecting",
        "compiling",
        "creating",
        "deploying",
        "fetching",
        "generating",
        "installing",
        "linking",
        "loading",
        "publishing",
        "running",
        "scanning",
        "searching",
        "starting",
        "uploading",
        "verifying",
        "waiting",
        "writing",
    ];

    noise_prefixes.iter().any(|prefix| lower.starts_with(prefix))
}

fn is_border_line(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|c| matches!(c, '─' | '═' | '―' | '-' | '=' | '·' | '•' | ' ' | '┈' | '┄'))
}

fn collapse_whitespace(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_vercel_deploy_output() {
        let output = "\
\x1b[32mVercel CLI 34.2.0\x1b[0m
⠋ Deploying...
  Building...
  Bundling...
\x1b[32mProduction:\x1b[0m https://my-app.vercel.app
  Inspect: https://vercel.com/acme/my-app/abcd1234
  Ready in 12.3s
";

        let result = filter_vercel_output(output);
        assert!(result.contains("Production: https://my-app.vercel.app"));
        assert!(result.contains("Inspect: https://vercel.com/acme/my-app/abcd1234"));
        assert!(result.contains("Ready in 12.3s"));
        assert!(!result.contains("Deploying"));
        assert!(!result.contains("Bundling"));
        assert!(!result.contains("Vercel CLI"));
    }

    #[test]
    fn test_filter_vercel_project_ls() {
        let output = "\
Vercel CLI 34.2.0

Name                Team     Production Branch   Last Updated
my-app              acme     main                2 days ago
docs-site           acme     develop             1 day ago
";

        let result = filter_vercel_output(output);
        assert!(result.contains("Name | Team | Production Branch | Last Updated"));
        assert!(result.contains("my-app | acme | main | 2 days ago"));
        assert!(result.contains("docs-site | acme | develop | 1 day ago"));
    }

    #[test]
    fn test_filter_vercel_empty_output() {
        let output = "\
⠋ Deploying...
  Building...
  Uploading...
";

        let result = filter_vercel_output(output);
        assert_eq!(result, "vercel: ok");
    }

    #[test]
    fn test_filter_vercel_error_with_percent_preserved() {
        let output = "Error: Deployment reached 100% but failed during verification\n";

        let result = filter_vercel_output(output);

        assert_eq!(
            result,
            "Error: Deployment reached 100% but failed during verification"
        );
    }

    #[test]
    fn test_filter_vercel_caps_long_output() {
        let mut output = String::from("Name                Team     Production Branch   Last Updated\n");
        for idx in 1..=45 {
            output.push_str(&format!(
                "project-{idx:02}          team     main                {idx} days ago\n"
            ));
        }

        let result = filter_vercel_output(&output);
        assert!(result.contains("... +6 more lines omitted"));
        assert!(result.contains("project-01 | team | main | 1 days ago"));
    }
}
