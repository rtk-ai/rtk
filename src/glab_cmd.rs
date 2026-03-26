//! GitLab CLI (glab) command output compression.
//!
//! Provides token-optimized alternatives to verbose `glab` commands.
//! Focuses on extracting essential information from JSON outputs.

use crate::tracking;
use crate::utils::{resolved_command, truncate};
use anyhow::{Context, Result};
use serde_json::Value;

/// Run a glab command with token-optimized output.
pub fn run(subcommand: &str, args: &[String], verbose: u8, ultra_compact: bool) -> Result<()> {
    // When user explicitly passes --output json or -F json, they want raw glab JSON output
    if has_json_output_flag(args) {
        return run_passthrough("glab", subcommand, args);
    }

    match subcommand {
        "mr" => run_mr(args, verbose, ultra_compact),
        "issue" => run_issue(args, verbose, ultra_compact),
        _ => run_passthrough("glab", subcommand, args),
    }
}

/// Check if user explicitly requested JSON output (--output json or -F json).
fn has_json_output_flag(args: &[String]) -> bool {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--output" || arg == "-F" {
            if let Some(val) = iter.next() {
                if val == "json" {
                    return true;
                }
            }
        }
        if arg.starts_with("--output=json") || arg.starts_with("-Fjson") {
            return true;
        }
    }
    false
}

fn run_mr(args: &[String], verbose: u8, ultra_compact: bool) -> Result<()> {
    if args.is_empty() {
        return run_passthrough("glab", "mr", args);
    }

    match args[0].as_str() {
        "list" | "ls" => list_mrs(&args[1..], verbose, ultra_compact),
        "view" => view_mr(&args[1..], verbose, ultra_compact),
        _ => run_passthrough("glab", "mr", args),
    }
}

fn run_issue(args: &[String], verbose: u8, ultra_compact: bool) -> Result<()> {
    if args.is_empty() {
        return run_passthrough("glab", "issue", args);
    }

    match args[0].as_str() {
        "list" | "ls" => list_issues(&args[1..], verbose, ultra_compact),
        "view" => view_issue(&args[1..], verbose, ultra_compact),
        _ => run_passthrough("glab", "issue", args),
    }
}

fn list_mrs(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("glab");
    cmd.args(["mr", "list", "--output", "json"]);

    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run glab mr list")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        timer.track("glab mr list", "rtk glab mr list", &stderr, &stderr);
        eprintln!("{}", stderr.trim());
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let filtered = filter_mr_list(&raw, ultra_compact).unwrap_or_else(|_| raw.clone());

    print!("{}", filtered);
    timer.track("glab mr list", "rtk glab mr list", &raw, &filtered);
    Ok(())
}

/// Filter glab mr list JSON output into compact text.
fn filter_mr_list(raw: &str, ultra_compact: bool) -> Result<String> {
    let json: Value = serde_json::from_str(raw).context("Failed to parse glab mr list output")?;

    let mut filtered = String::new();

    if let Some(mrs) = json.as_array() {
        if ultra_compact {
            filtered.push_str("MRs\n");
        } else {
            filtered.push_str("Merge Requests\n");
        }

        for mr in mrs.iter().take(20) {
            let iid = mr["iid"].as_i64().unwrap_or(0);
            let title = mr["title"].as_str().unwrap_or("???");
            let state = mr["state"].as_str().unwrap_or("???");
            let author = mr["author"]["username"].as_str().unwrap_or("???");
            let draft = mr["draft"].as_bool().unwrap_or(false);

            let state_icon = format_state(state, draft, ultra_compact);

            filtered.push_str(&format!(
                "  {} !{} {} ({})\n",
                state_icon,
                iid,
                truncate(title, 60),
                author
            ));
        }

        if mrs.len() > 20 {
            filtered.push_str(&format!(
                "  ... {} more (use glab mr list for all)\n",
                mrs.len() - 20
            ));
        }
    }

    Ok(filtered)
}

fn view_mr(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let (mr_id, extra_args) = match extract_identifier_and_extra_args(args) {
        Some(result) => result,
        None => return Err(anyhow::anyhow!("MR number or branch required")),
    };

    // If user provides --web or --comments, pass through directly
    if extra_args
        .iter()
        .any(|a| a == "--web" || a == "-w" || a == "--comments" || a == "-c")
    {
        return run_passthrough_with_extra("glab", &["mr", "view", &mr_id], &extra_args);
    }

    let mut cmd = resolved_command("glab");
    cmd.args(["mr", "view", &mr_id, "--output", "json"]);
    for arg in &extra_args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run glab mr view")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        timer.track(
            &format!("glab mr view {}", mr_id),
            &format!("rtk glab mr view {}", mr_id),
            &stderr,
            &stderr,
        );
        eprintln!("{}", stderr.trim());
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let filtered = filter_mr_view(&raw, ultra_compact).unwrap_or_else(|_| raw.clone());

    print!("{}", filtered);
    timer.track(
        &format!("glab mr view {}", mr_id),
        &format!("rtk glab mr view {}", mr_id),
        &raw,
        &filtered,
    );
    Ok(())
}

/// Filter glab mr view JSON output into compact text.
fn filter_mr_view(raw: &str, ultra_compact: bool) -> Result<String> {
    let json: Value = serde_json::from_str(raw).context("Failed to parse glab mr view output")?;

    let mut filtered = String::new();

    let iid = json["iid"].as_i64().unwrap_or(0);
    let title = json["title"].as_str().unwrap_or("???");
    let state = json["state"].as_str().unwrap_or("???");
    let author = json["author"]["username"].as_str().unwrap_or("???");
    let draft = json["draft"].as_bool().unwrap_or(false);
    let web_url = json["web_url"].as_str().unwrap_or("");
    let source = json["source_branch"].as_str().unwrap_or("???");
    let target = json["target_branch"].as_str().unwrap_or("???");
    let has_conflicts = json["has_conflicts"].as_bool().unwrap_or(false);
    let merge_status = json["detailed_merge_status"].as_str().unwrap_or("unknown");

    let state_icon = format_state(state, draft, ultra_compact);

    filtered.push_str(&format!("{} MR !{}: {}\n", state_icon, iid, title));
    filtered.push_str(&format!("  {} -> {}\n", source, target));
    filtered.push_str(&format!("  Author: {}\n", author));

    // Merge status
    let status_str = if has_conflicts {
        "[conflicts]"
    } else {
        match merge_status {
            "mergeable" => "[ok]",
            "not_approved" => "[needs approval]",
            "not_open" => "[not open]",
            "ci_must_pass" => "[ci pending]",
            "discussions_not_resolved" => "[unresolved discussions]",
            _ => merge_status,
        }
    };
    filtered.push_str(&format!("  Status: {} {}\n", state, status_str));

    // Reviewers
    if let Some(reviewers) = json["reviewers"].as_array() {
        if !reviewers.is_empty() {
            let names: Vec<&str> = reviewers
                .iter()
                .filter_map(|r| r["username"].as_str())
                .collect();
            filtered.push_str(&format!("  Reviewers: {}\n", names.join(", ")));
        }
    }

    // Labels
    if let Some(labels) = json["labels"].as_array() {
        if !labels.is_empty() {
            let label_strs: Vec<&str> = labels.iter().filter_map(|l| l.as_str()).collect();
            if !label_strs.is_empty() {
                filtered.push_str(&format!("  Labels: {}\n", label_strs.join(", ")));
            }
        }
    }

    filtered.push_str(&format!("  {}\n", web_url));

    // Description (filtered)
    if let Some(desc) = json["description"].as_str() {
        if !desc.is_empty() {
            let desc_filtered = filter_description(desc);
            if !desc_filtered.is_empty() {
                filtered.push('\n');
                for line in desc_filtered.lines() {
                    filtered.push_str(&format!("  {}\n", line));
                }
            }
        }
    }

    Ok(filtered)
}

fn list_issues(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("glab");
    cmd.args(["issue", "list", "--output", "json"]);

    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run glab issue list")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        timer.track("glab issue list", "rtk glab issue list", &stderr, &stderr);
        eprintln!("{}", stderr.trim());
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let filtered = filter_issue_list(&raw, ultra_compact).unwrap_or_else(|_| raw.clone());

    print!("{}", filtered);
    timer.track("glab issue list", "rtk glab issue list", &raw, &filtered);
    Ok(())
}

/// Filter glab issue list JSON output into compact text.
fn filter_issue_list(raw: &str, ultra_compact: bool) -> Result<String> {
    let json: Value =
        serde_json::from_str(raw).context("Failed to parse glab issue list output")?;

    let mut filtered = String::new();

    if let Some(issues) = json.as_array() {
        if ultra_compact {
            filtered.push_str("Issues\n");
        } else {
            filtered.push_str("Project Issues\n");
        }

        for issue in issues.iter().take(20) {
            let iid = issue["iid"].as_i64().unwrap_or(0);
            let title = issue["title"].as_str().unwrap_or("???");
            let state = issue["state"].as_str().unwrap_or("???");
            let author = issue["author"]["username"].as_str().unwrap_or("???");

            let state_icon = if ultra_compact {
                match state {
                    "opened" => "O",
                    "closed" => "C",
                    _ => "?",
                }
            } else {
                match state {
                    "opened" => "[open]",
                    "closed" => "[closed]",
                    _ => "[unknown]",
                }
            };

            filtered.push_str(&format!(
                "  {} #{} {} ({})\n",
                state_icon,
                iid,
                truncate(title, 60),
                author
            ));
        }

        if issues.len() > 20 {
            filtered.push_str(&format!(
                "  ... {} more (use glab issue list for all)\n",
                issues.len() - 20
            ));
        }
    }

    Ok(filtered)
}

fn view_issue(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let (issue_id, extra_args) = match extract_identifier_and_extra_args(args) {
        Some(result) => result,
        None => return Err(anyhow::anyhow!("Issue number required")),
    };

    if extra_args
        .iter()
        .any(|a| a == "--web" || a == "-w" || a == "--comments" || a == "-c")
    {
        return run_passthrough_with_extra("glab", &["issue", "view", &issue_id], &extra_args);
    }

    let mut cmd = resolved_command("glab");
    cmd.args(["issue", "view", &issue_id, "--output", "json"]);
    for arg in &extra_args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run glab issue view")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        timer.track(
            &format!("glab issue view {}", issue_id),
            &format!("rtk glab issue view {}", issue_id),
            &stderr,
            &stderr,
        );
        eprintln!("{}", stderr.trim());
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let filtered = filter_issue_view(&raw, ultra_compact).unwrap_or_else(|_| raw.clone());

    print!("{}", filtered);
    timer.track(
        &format!("glab issue view {}", issue_id),
        &format!("rtk glab issue view {}", issue_id),
        &raw,
        &filtered,
    );
    Ok(())
}

/// Filter glab issue view JSON output into compact text.
fn filter_issue_view(raw: &str, ultra_compact: bool) -> Result<String> {
    let json: Value =
        serde_json::from_str(raw).context("Failed to parse glab issue view output")?;

    let mut filtered = String::new();

    let iid = json["iid"].as_i64().unwrap_or(0);
    let title = json["title"].as_str().unwrap_or("???");
    let state = json["state"].as_str().unwrap_or("???");
    let author = json["author"]["username"].as_str().unwrap_or("???");
    let web_url = json["web_url"].as_str().unwrap_or("");

    let state_icon = if ultra_compact {
        match state {
            "opened" => "O",
            "closed" => "C",
            _ => "?",
        }
    } else {
        match state {
            "opened" => "[open]",
            "closed" => "[closed]",
            _ => "[unknown]",
        }
    };

    filtered.push_str(&format!("{} Issue #{}: {}\n", state_icon, iid, title));
    filtered.push_str(&format!("  Author: {}\n", author));

    // Assignees
    if let Some(assignees) = json["assignees"].as_array() {
        if !assignees.is_empty() {
            let names: Vec<&str> = assignees
                .iter()
                .filter_map(|a| a["username"].as_str())
                .collect();
            filtered.push_str(&format!("  Assignees: {}\n", names.join(", ")));
        }
    }

    // Labels
    if let Some(labels) = json["labels"].as_array() {
        if !labels.is_empty() {
            let label_strs: Vec<&str> = labels.iter().filter_map(|l| l.as_str()).collect();
            if !label_strs.is_empty() {
                filtered.push_str(&format!("  Labels: {}\n", label_strs.join(", ")));
            }
        }
    }

    // Milestone
    if let Some(milestone) = json["milestone"].as_object() {
        if let Some(title) = milestone.get("title").and_then(|t| t.as_str()) {
            filtered.push_str(&format!("  Milestone: {}\n", title));
        }
    }

    filtered.push_str(&format!("  {}\n", web_url));

    // Description
    if let Some(desc) = json["description"].as_str() {
        if !desc.is_empty() {
            let desc_filtered = filter_description(desc);
            if !desc_filtered.is_empty() {
                filtered.push('\n');
                for line in desc_filtered.lines() {
                    filtered.push_str(&format!("  {}\n", line));
                }
            }
        }
    }

    Ok(filtered)
}

/// Format MR state as compact icon.
fn format_state(state: &str, draft: bool, ultra_compact: bool) -> &'static str {
    if draft {
        if ultra_compact {
            "D"
        } else {
            "[draft]"
        }
    } else if ultra_compact {
        match state {
            "opened" => "O",
            "merged" => "M",
            "closed" => "C",
            _ => "?",
        }
    } else {
        match state {
            "opened" => "[open]",
            "merged" => "[merged]",
            "closed" => "[closed]",
            _ => "[unknown]",
        }
    }
}

/// Filter description text to remove noise while preserving meaningful content.
/// Removes HTML comments, image-only lines, horizontal rules,
/// and collapses excessive blank lines.
fn filter_description(body: &str) -> String {
    if body.is_empty() {
        return String::new();
    }

    let mut result = String::new();
    let mut blank_count = 0;

    for line in body.lines() {
        let trimmed = line.trim();

        // Skip HTML comments (single-line)
        if trimmed.starts_with("<!--") && trimmed.ends_with("-->") {
            continue;
        }

        // Skip image-only lines
        if trimmed.starts_with("![") && trimmed.ends_with(')') && trimmed.contains("](") {
            continue;
        }

        // Skip horizontal rules
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            continue;
        }

        // Collapse blank lines
        if trimmed.is_empty() {
            blank_count += 1;
            if blank_count <= 1 {
                result.push('\n');
            }
            continue;
        }

        blank_count = 0;
        result.push_str(line);
        result.push('\n');
    }

    result.trim().to_string()
}

/// Extract the first non-flag argument as the identifier (MR/issue number),
/// and return remaining args separately.
fn extract_identifier_and_extra_args(args: &[String]) -> Option<(String, Vec<String>)> {
    let mut identifier: Option<String> = None;
    let mut extra = Vec::new();
    let mut skip_next = false;

    for arg in args {
        if skip_next {
            extra.push(arg.clone());
            skip_next = false;
            continue;
        }
        // Flags that take a value
        if arg == "-R" || arg == "--repo" || arg == "-P" || arg == "--per-page" {
            extra.push(arg.clone());
            skip_next = true;
            continue;
        }
        if arg.starts_with('-') {
            extra.push(arg.clone());
            continue;
        }
        // First non-flag arg is the identifier
        if identifier.is_none() {
            identifier = Some(arg.clone());
        } else {
            extra.push(arg.clone());
        }
    }

    identifier.map(|id| (id, extra))
}

/// Pass through a command unchanged, tracking as passthrough.
fn run_passthrough(cmd: &str, subcommand: &str, args: &[String]) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut command = resolved_command(cmd);
    command.arg(subcommand);
    for arg in args {
        command.arg(arg);
    }

    let status = command
        .status()
        .context(format!("Failed to run {} {}", cmd, subcommand))?;

    let args_str = tracking::args_display(&args.iter().map(|s| s.into()).collect::<Vec<_>>());
    timer.track_passthrough(
        &format!("{} {} {}", cmd, subcommand, args_str),
        &format!("rtk {} {} {} (passthrough)", cmd, subcommand, args_str),
    );

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

/// Pass through a command with base args + extra args, tracking as passthrough.
fn run_passthrough_with_extra(cmd: &str, base_args: &[&str], extra_args: &[String]) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut command = resolved_command(cmd);
    for arg in base_args {
        command.arg(arg);
    }
    for arg in extra_args {
        command.arg(arg);
    }

    let status =
        command
            .status()
            .context(format!("Failed to run {} {}", cmd, base_args.join(" ")))?;

    let full_cmd = format!(
        "{} {} {}",
        cmd,
        base_args.join(" "),
        tracking::args_display(&extra_args.iter().map(|s| s.into()).collect::<Vec<_>>())
    );
    timer.track_passthrough(&full_cmd, &format!("rtk {} (passthrough)", full_cmd));

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_filter_mr_list() {
        let input = include_str!("../tests/fixtures/glab_mr_list_raw.json");
        let output = filter_mr_list(input, false).unwrap();

        assert!(output.contains("Merge Requests"));
        assert!(output.contains("!42"));
        assert!(output.contains("!41"));
        assert!(output.contains("!40"));
        assert!(output.contains("jdoe"));
        assert!(output.contains("[merged]"));
        assert!(output.contains("[draft]"));
    }

    #[test]
    fn test_filter_mr_list_ultra_compact() {
        let input = include_str!("../tests/fixtures/glab_mr_list_raw.json");
        let output = filter_mr_list(input, true).unwrap();

        assert!(output.contains("MRs"));
        assert!(output.contains("D ")); // draft
        assert!(output.contains("M ")); // merged
    }

    #[test]
    fn test_filter_mr_list_savings() {
        let input = include_str!("../tests/fixtures/glab_mr_list_raw.json");
        let output = filter_mr_list(input, false).unwrap();

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);

        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "MR list filter: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_mr_view() {
        let input = include_str!("../tests/fixtures/glab_mr_view_raw.json");
        let output = filter_mr_view(input, false).unwrap();

        assert!(output.contains("MR !42"));
        assert!(output.contains("Add full-text search"));
        assert!(output.contains("jdoe"));
        assert!(output.contains("feature/add-search -> main"));
        assert!(output.contains("[ok]"));
        assert!(output.contains("areview"));
        assert!(output.contains("backend, search"));
    }

    #[test]
    fn test_filter_mr_view_savings() {
        let input = include_str!("../tests/fixtures/glab_mr_view_raw.json");
        let output = filter_mr_view(input, false).unwrap();

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);

        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "MR view filter: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_description() {
        let body = "<!-- template -->\n## Overview\n\nSome text.\n\n---\n\n![screenshot](url)\n\nMore text.";
        let filtered = filter_description(body);
        assert!(filtered.contains("## Overview"));
        assert!(filtered.contains("Some text."));
        assert!(filtered.contains("More text."));
        assert!(!filtered.contains("<!-- template -->"));
        assert!(!filtered.contains("---"));
    }

    #[test]
    fn test_filter_description_empty() {
        assert_eq!(filter_description(""), "");
    }

    #[test]
    fn test_has_json_output_flag() {
        assert!(has_json_output_flag(&["--output".into(), "json".into()]));
        assert!(has_json_output_flag(&["-F".into(), "json".into()]));
        assert!(has_json_output_flag(&["--output=json".into()]));
        assert!(!has_json_output_flag(&["--output".into(), "text".into()]));
        assert!(!has_json_output_flag(&["--all".into()]));
    }

    #[test]
    fn test_format_state() {
        assert_eq!(format_state("opened", false, false), "[open]");
        assert_eq!(format_state("merged", false, false), "[merged]");
        assert_eq!(format_state("opened", true, false), "[draft]");
        assert_eq!(format_state("opened", false, true), "O");
        assert_eq!(format_state("merged", false, true), "M");
        assert_eq!(format_state("opened", true, true), "D");
    }

    #[test]
    fn test_extract_identifier_and_extra_args() {
        let args: Vec<String> = vec!["42".into(), "--web".into()];
        let (id, extra) = extract_identifier_and_extra_args(&args).unwrap();
        assert_eq!(id, "42");
        assert_eq!(extra, vec!["--web"]);

        let args: Vec<String> = vec!["-R".into(), "group/project".into(), "10".into()];
        let (id, extra) = extract_identifier_and_extra_args(&args).unwrap();
        assert_eq!(id, "10");
        assert_eq!(extra, vec!["-R", "group/project"]);
    }
}
