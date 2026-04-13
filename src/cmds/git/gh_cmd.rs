//! GitHub CLI (gh) command output compression.
//!
//! Provides token-optimized alternatives to verbose `gh` commands.
//! Focuses on extracting essential information from JSON outputs.

use crate::core::runner::{self, RunOptions};
use crate::core::utils::{ok_confirmation, resolved_command, truncate};
use crate::git;
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;
use serde_json::Value;
use std::process::Command;

lazy_static! {
    static ref HTML_COMMENT_RE: Regex = Regex::new(r"(?s)<!--.*?-->").unwrap();
    static ref BADGE_LINE_RE: Regex =
        Regex::new(r"(?m)^\s*\[!\[[^\]]*\]\([^)]*\)\]\([^)]*\)\s*$").unwrap();
    static ref IMAGE_ONLY_LINE_RE: Regex = Regex::new(r"(?m)^\s*!\[[^\]]*\]\([^)]*\)\s*$").unwrap();
    static ref HORIZONTAL_RULE_RE: Regex =
        Regex::new(r"(?m)^\s*(?:---+|\*\*\*+|___+)\s*$").unwrap();
    static ref MULTI_BLANK_RE: Regex = Regex::new(r"\n{3,}").unwrap();
}

/// Filter markdown body to remove noise while preserving meaningful content.
/// Removes HTML comments, badge lines, image-only lines, horizontal rules,
/// and collapses excessive blank lines. Preserves code blocks untouched.
fn filter_markdown_body(body: &str) -> String {
    if body.is_empty() {
        return String::new();
    }

    // Split into code blocks and non-code segments
    let mut result = String::new();
    let mut remaining = body;

    loop {
        // Find next code block opening (``` or ~~~)
        let fence_pos = remaining
            .find("```")
            .or_else(|| remaining.find("~~~"))
            .map(|pos| {
                let fence = if remaining[pos..].starts_with("```") {
                    "```"
                } else {
                    "~~~"
                };
                (pos, fence)
            });

        match fence_pos {
            Some((start, fence)) => {
                // Filter the text before the code block
                let before = &remaining[..start];
                result.push_str(&filter_markdown_segment(before));

                // Find the closing fence
                let after_open = start + fence.len();
                // Skip past the opening fence line
                let code_start = remaining[after_open..]
                    .find('\n')
                    .map(|p| after_open + p + 1)
                    .unwrap_or(remaining.len());

                let close_pos = remaining[code_start..]
                    .find(fence)
                    .map(|p| code_start + p + fence.len());

                match close_pos {
                    Some(end) => {
                        // Preserve the entire code block as-is
                        result.push_str(&remaining[start..end]);
                        // Include the rest of the closing fence line
                        let after_close = remaining[end..]
                            .find('\n')
                            .map(|p| end + p + 1)
                            .unwrap_or(remaining.len());
                        result.push_str(&remaining[end..after_close]);
                        remaining = &remaining[after_close..];
                    }
                    None => {
                        // Unclosed code block — preserve everything
                        result.push_str(&remaining[start..]);
                        remaining = "";
                    }
                }
            }
            None => {
                // No more code blocks, filter the rest
                result.push_str(&filter_markdown_segment(remaining));
                break;
            }
        }
    }

    // Final cleanup: trim trailing whitespace
    result.trim().to_string()
}

/// Filter a markdown segment that is NOT inside a code block.
fn filter_markdown_segment(text: &str) -> String {
    let mut s = HTML_COMMENT_RE.replace_all(text, "").to_string();
    s = BADGE_LINE_RE.replace_all(&s, "").to_string();
    s = IMAGE_ONLY_LINE_RE.replace_all(&s, "").to_string();
    s = HORIZONTAL_RULE_RE.replace_all(&s, "").to_string();
    s = MULTI_BLANK_RE.replace_all(&s, "\n\n").to_string();
    s
}

/// Check if args contain --json flag (user wants specific JSON fields, not RTK filtering)
fn has_json_flag(args: &[String]) -> bool {
    args.iter().any(|a| a == "--json")
}

/// Extract a positional identifier (PR/issue number) from args, returning it
/// separately from the remaining extra flags (like -R, --repo, etc.).
/// Handles both `view 123 -R owner/repo` and `view -R owner/repo 123`.
fn extract_identifier_and_extra_args(args: &[String]) -> Option<(String, Vec<String>)> {
    if args.is_empty() {
        return None;
    }

    // Known gh flags that take a value — skip these and their values
    let flags_with_value = [
        "-R",
        "--repo",
        "-q",
        "--jq",
        "-t",
        "--template",
        "--job",
        "--attempt",
    ];
    let mut identifier = None;
    let mut extra = Vec::new();
    let mut skip_next = false;

    for arg in args {
        if skip_next {
            extra.push(arg.clone());
            skip_next = false;
            continue;
        }
        if flags_with_value.contains(&arg.as_str()) {
            extra.push(arg.clone());
            skip_next = true;
            continue;
        }
        if arg.starts_with('-') {
            extra.push(arg.clone());
            continue;
        }
        // First non-flag arg is the identifier (number/URL)
        if identifier.is_none() {
            identifier = Some(arg.clone());
        } else {
            extra.push(arg.clone());
        }
    }

    identifier.map(|id| (id, extra))
}

fn run_gh_json<F>(cmd: Command, label: &str, filter_fn: F) -> Result<i32>
where
    F: Fn(&Value) -> String,
{
    runner::run_filtered(
        cmd,
        "gh",
        label,
        |stdout| match serde_json::from_str::<Value>(stdout) {
            Ok(json) => filter_fn(&json),
            Err(_) => stdout.to_string(),
        },
        RunOptions::stdout_only()
            .early_exit_on_failure()
            .no_trailing_newline(),
    )
}

pub fn run(subcommand: &str, args: &[String], verbose: u8, ultra_compact: bool) -> Result<i32> {
    // When user explicitly passes --json, they want raw gh JSON output, not RTK filtering
    if has_json_flag(args) {
        return run_passthrough("gh", subcommand, args);
    }

    match subcommand {
        "pr" => run_pr(args, verbose, ultra_compact),
        "issue" => run_issue(args, verbose, ultra_compact),
        "run" => run_workflow(args, verbose, ultra_compact),
        "repo" => run_repo(args, verbose, ultra_compact),
        "api" => run_api(args, verbose),
        _ => {
            // Unknown subcommand, pass through
            run_passthrough("gh", subcommand, args)
        }
    }
}

fn run_pr(args: &[String], verbose: u8, ultra_compact: bool) -> Result<i32> {
    if args.is_empty() {
        return run_passthrough("gh", "pr", args);
    }

    match args[0].as_str() {
        "list" => list_prs(&args[1..], verbose, ultra_compact),
        "view" => view_pr(&args[1..], verbose, ultra_compact),
        "checks" => pr_checks(&args[1..], verbose, ultra_compact),
        "status" => pr_status(&args[1..], verbose, ultra_compact),
        "create" => pr_create(&args[1..], verbose),
        "merge" => pr_merge(&args[1..], verbose),
        "diff" => pr_diff(&args[1..], verbose),
        "comment" => pr_action("commented", args, verbose),
        "edit" => pr_action("edited", args, verbose),
        _ => run_passthrough("gh", "pr", args),
    }
}

fn list_prs(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<i32> {
    let mut cmd = resolved_command("gh");
    cmd.args([
        "pr",
        "list",
        "--json",
        "number,title,state,author,updatedAt",
    ]);
    for arg in args {
        cmd.arg(arg);
    }
    run_gh_json(cmd, "pr list", |json| format_pr_list(json, ultra_compact))
}

fn format_pr_list(json: &Value, ultra_compact: bool) -> String {
    let prs = match json.as_array() {
        Some(prs) => prs,
        None => return String::new(),
    };
    if prs.is_empty() {
        return if ultra_compact {
            "No PRs\n".to_string()
        } else {
            "No Pull Requests\n".to_string()
        };
    }
    let mut out = String::new();
    out.push_str(if ultra_compact {
        "PRs\n"
    } else {
        "Pull Requests\n"
    });
    for pr in prs.iter().take(20) {
        let number = pr["number"].as_i64().unwrap_or(0);
        let title = pr["title"].as_str().unwrap_or("???");
        let state = pr["state"].as_str().unwrap_or("???");
        let author = pr["author"]["login"].as_str().unwrap_or("???");
        let icon = state_icon(state, ultra_compact);
        out.push_str(&format!(
            "  {} #{} {} ({})\n",
            icon,
            number,
            truncate(title, 60),
            author
        ));
    }
    if prs.len() > 20 {
        out.push_str(&format!(
            "  ... {} more (use gh pr list for all)\n",
            prs.len() - 20
        ));
    }
    out
}

fn state_icon(state: &str, ultra_compact: bool) -> &'static str {
    if ultra_compact {
        match state {
            "OPEN" => "O",
            "MERGED" => "M",
            "CLOSED" => "C",
            _ => "?",
        }
    } else {
        match state {
            "OPEN" => "[open]",
            "MERGED" => "[merged]",
            "CLOSED" => "[closed]",
            _ => "[unknown]",
        }
    }
}

fn should_passthrough_pr_view(extra_args: &[String]) -> bool {
    extra_args
        .iter()
        .any(|a| a == "--json" || a == "--jq" || a == "--web" || a == "--comments")
}

fn should_passthrough_issue_view(extra_args: &[String]) -> bool {
    extra_args
        .iter()
        .any(|a| a == "--json" || a == "--jq" || a == "--web" || a == "--comments")
}

fn should_passthrough_pr_status(args: &[String]) -> bool {
    args.iter().any(|a| {
        matches!(
            a.as_str(),
            "--help" | "-h" | "--web" | "--jq" | "--template"
        )
    })
}

fn pr_status_json_fields() -> &'static str {
    "number,title,reviewDecision,statusCheckRollup"
}

fn view_pr(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<i32> {
    let (pr_number, extra_args) = match extract_identifier_and_extra_args(args) {
        Some(result) => result,
        None => return Err(anyhow::anyhow!("PR number required")),
    };
    if should_passthrough_pr_view(&extra_args) {
        return run_passthrough_with_extra("gh", &["pr", "view", &pr_number], &extra_args);
    }
    let mut cmd = resolved_command("gh");
    cmd.args([
        "pr",
        "view",
        &pr_number,
        "--json",
        "number,title,state,author,body,url,mergeable,reviews,statusCheckRollup",
    ]);
    for arg in &extra_args {
        cmd.arg(arg);
    }
    run_gh_json(cmd, &format!("pr view {}", pr_number), |json| {
        format_pr_view(json, ultra_compact)
    })
}

fn format_pr_view(json: &Value, ultra_compact: bool) -> String {
    let mut out = String::new();
    let number = json["number"].as_i64().unwrap_or(0);
    let title = json["title"].as_str().unwrap_or("???");
    let state = json["state"].as_str().unwrap_or("???");
    let author = json["author"]["login"].as_str().unwrap_or("???");
    let url = json["url"].as_str().unwrap_or("");
    let mergeable = json["mergeable"].as_str().unwrap_or("UNKNOWN");

    let icon = state_icon(state, ultra_compact);
    out.push_str(&format!("{} PR #{}: {}\n", icon, number, title));
    out.push_str(&format!("  {}\n", author));

    let mergeable_str = match mergeable {
        "MERGEABLE" => "[ok]",
        "CONFLICTING" => "[x]",
        _ => "?",
    };
    out.push_str(&format!("  {} | {}\n", state, mergeable_str));

    if let Some(reviews) = json["reviews"]["nodes"].as_array() {
        let approved = reviews
            .iter()
            .filter(|r| r["state"].as_str() == Some("APPROVED"))
            .count();
        let changes = reviews
            .iter()
            .filter(|r| r["state"].as_str() == Some("CHANGES_REQUESTED"))
            .count();
        if approved > 0 || changes > 0 {
            out.push_str(&format!(
                "  Reviews: {} approved, {} changes requested\n",
                approved, changes
            ));
        }
    }

    if let Some(checks) = json["statusCheckRollup"].as_array() {
        let total = checks.len();
        let passed = checks
            .iter()
            .filter(|c| {
                c["conclusion"].as_str() == Some("SUCCESS")
                    || c["state"].as_str() == Some("SUCCESS")
            })
            .count();
        let failed = checks
            .iter()
            .filter(|c| {
                c["conclusion"].as_str() == Some("FAILURE")
                    || c["state"].as_str() == Some("FAILURE")
            })
            .count();
        if ultra_compact {
            if failed > 0 {
                out.push_str(&format!("  [x]{}/{}  {} fail\n", passed, total, failed));
            } else {
                out.push_str(&format!("  {}/{}\n", passed, total));
            }
        } else {
            out.push_str(&format!("  Checks: {}/{} passed\n", passed, total));
            if failed > 0 {
                out.push_str(&format!("  [warn] {} checks failed\n", failed));
            }
        }
    }

    out.push_str(&format!("  {}\n", url));

    if let Some(body) = json["body"].as_str() {
        if !body.is_empty() {
            let body_filtered = filter_markdown_body(body);
            if !body_filtered.is_empty() {
                out.push('\n');
                for line in body_filtered.lines() {
                    out.push_str(&format!("  {}\n", line));
                }
            }
        }
    }

    out
}

fn pr_checks(args: &[String], _verbose: u8, _ultra_compact: bool) -> Result<i32> {
    let (pr_number, extra_args) = match extract_identifier_and_extra_args(args) {
        Some(result) => result,
        None => return Err(anyhow::anyhow!("PR number required")),
    };
    let mut cmd = resolved_command("gh");
    cmd.args(["pr", "checks", &pr_number]);
    for arg in &extra_args {
        cmd.arg(arg);
    }
    runner::run_filtered(
        cmd,
        "gh",
        &format!("pr checks {}", pr_number),
        format_pr_checks,
        RunOptions::stdout_only()
            .early_exit_on_failure()
            .no_trailing_newline(),
    )
}

fn format_pr_checks(stdout: &str) -> String {
    let mut passed = 0;
    let mut failed = 0;
    let mut pending = 0;
    let mut failed_checks = Vec::new();

    for line in stdout.lines() {
        if line.contains("[ok]") || line.contains("pass") {
            passed += 1;
        } else if line.contains("[x]") || line.contains("fail") {
            failed += 1;
            failed_checks.push(line.trim().to_string());
        } else if line.contains('*') || line.contains("pending") {
            pending += 1;
        }
    }

    let mut out = String::new();
    out.push_str("CI Checks Summary:\n");
    out.push_str(&format!("  [ok] Passed: {}\n", passed));
    out.push_str(&format!("  [FAIL] Failed: {}\n", failed));
    if pending > 0 {
        out.push_str(&format!("  [pending] Pending: {}\n", pending));
    }
    if !failed_checks.is_empty() {
        out.push_str("\n  Failed checks:\n");
        for check in failed_checks {
            out.push_str(&format!("    {}\n", check));
        }
    }
    out
}

fn pr_status(args: &[String], _verbose: u8, _ultra_compact: bool) -> Result<i32> {
    if should_passthrough_pr_status(args) {
        let mut passthrough_args = Vec::with_capacity(args.len() + 1);
        passthrough_args.push("status".to_string());
        passthrough_args.extend(args.iter().cloned());
        return run_passthrough("gh", "pr", &passthrough_args);
    }

    let mut cmd = resolved_command("gh");
    cmd.args(["pr", "status", "--json", pr_status_json_fields()]);
    for arg in args {
        cmd.arg(arg);
    }
    run_gh_json(cmd, "pr status", format_pr_status)
}

fn format_pr_status(json: &Value) -> String {
    let mut out = String::new();

    if !json["currentBranch"].is_null() {
        let current_branch = format_pr_status_entry(&json["currentBranch"]);
        if !current_branch.is_empty() {
            out.push_str("Current Branch\n");
            out.push_str(&current_branch);
            out.push('\n');
        }
    }

    if let Some(created_by) = json["createdBy"].as_array() {
        out.push_str(&format!("Your PRs ({}):\n", created_by.len()));
        for pr in created_by.iter().take(5) {
            let entry = format_pr_status_entry(pr);
            if !entry.is_empty() {
                out.push_str(&entry);
            }
        }
    }
    out
}

fn format_pr_status_entry(pr: &Value) -> String {
    if pr.is_null() {
        return String::new();
    }

    let number = pr["number"].as_i64().unwrap_or(0);
    let title = pr["title"].as_str().unwrap_or("???");
    let reviews = pr["reviewDecision"].as_str().unwrap_or("PENDING");
    let mut out = format!("  #{} {} [{}]", number, truncate(title, 50), reviews);

    if let Some(checks) = pr["statusCheckRollup"].as_array() {
        let total = checks.len();
        if total > 0 {
            let passed = checks
                .iter()
                .filter(|c| {
                    c["conclusion"].as_str() == Some("SUCCESS")
                        || c["state"].as_str() == Some("SUCCESS")
                })
                .count();
            let failed = checks
                .iter()
                .filter(|c| {
                    c["conclusion"].as_str() == Some("FAILURE")
                        || c["state"].as_str() == Some("FAILURE")
                })
                .count();

            out.push_str(&format!(" checks {}/{}", passed, total));
            if failed > 0 {
                out.push_str(&format!(" fail {}", failed));
            }
        }
    }

    out.push('\n');
    out
}

fn run_issue(args: &[String], verbose: u8, ultra_compact: bool) -> Result<i32> {
    if args.is_empty() {
        return run_passthrough("gh", "issue", args);
    }

    match args[0].as_str() {
        "list" => list_issues(&args[1..], verbose, ultra_compact),
        "view" => view_issue(&args[1..], verbose),
        _ => run_passthrough("gh", "issue", args),
    }
}

fn list_issues(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<i32> {
    let mut cmd = resolved_command("gh");
    cmd.args(["issue", "list", "--json", "number,title,state,author"]);
    for arg in args {
        cmd.arg(arg);
    }
    run_gh_json(cmd, "issue list", |json| {
        format_issue_list(json, ultra_compact)
    })
}

fn format_issue_list(json: &Value, ultra_compact: bool) -> String {
    let issues = match json.as_array() {
        Some(issues) => issues,
        None => return String::new(),
    };
    if issues.is_empty() {
        return "No Issues\n".to_string();
    }
    let mut out = String::new();
    out.push_str("Issues\n");
    for issue in issues.iter().take(20) {
        let number = issue["number"].as_i64().unwrap_or(0);
        let title = issue["title"].as_str().unwrap_or("???");
        let state = issue["state"].as_str().unwrap_or("???");
        let icon = if ultra_compact {
            if state == "OPEN" {
                "O"
            } else {
                "C"
            }
        } else if state == "OPEN" {
            "[open]"
        } else {
            "[closed]"
        };
        out.push_str(&format!("  {} #{} {}\n", icon, number, truncate(title, 60)));
    }
    if issues.len() > 20 {
        out.push_str(&format!("  ... {} more\n", issues.len() - 20));
    }
    out
}

fn view_issue(args: &[String], _verbose: u8) -> Result<i32> {
    let (issue_number, extra_args) = match extract_identifier_and_extra_args(args) {
        Some(result) => result,
        None => return Err(anyhow::anyhow!("Issue number required")),
    };
    if should_passthrough_issue_view(&extra_args) {
        return run_passthrough_with_extra("gh", &["issue", "view", &issue_number], &extra_args);
    }
    let mut cmd = resolved_command("gh");
    cmd.args([
        "issue",
        "view",
        &issue_number,
        "--json",
        "number,title,state,author,body,url",
    ]);
    for arg in &extra_args {
        cmd.arg(arg);
    }
    run_gh_json(cmd, &format!("issue view {}", issue_number), |json| {
        format_issue_view(json)
    })
}

fn format_issue_view(json: &Value) -> String {
    let mut out = String::new();
    let number = json["number"].as_i64().unwrap_or(0);
    let title = json["title"].as_str().unwrap_or("???");
    let state = json["state"].as_str().unwrap_or("???");
    let author = json["author"]["login"].as_str().unwrap_or("???");
    let url = json["url"].as_str().unwrap_or("");

    let icon = if state == "OPEN" {
        "[open]"
    } else {
        "[closed]"
    };
    out.push_str(&format!("{} Issue #{}: {}\n", icon, number, title));
    out.push_str(&format!("  Author: @{}\n", author));
    out.push_str(&format!("  Status: {}\n", state));
    out.push_str(&format!("  URL: {}\n", url));

    if let Some(body) = json["body"].as_str() {
        if !body.is_empty() {
            let body_filtered = filter_markdown_body(body);
            if !body_filtered.is_empty() {
                out.push_str("\n  Description:\n");
                for line in body_filtered.lines() {
                    out.push_str(&format!("    {}\n", line));
                }
            }
        }
    }
    out
}

fn run_workflow(args: &[String], verbose: u8, ultra_compact: bool) -> Result<i32> {
    if args.is_empty() {
        return run_passthrough("gh", "run", args);
    }

    match args[0].as_str() {
        "list" => list_runs(&args[1..], verbose, ultra_compact),
        "view" => view_run(&args[1..], verbose),
        _ => run_passthrough("gh", "run", args),
    }
}

fn list_runs(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<i32> {
    let mut cmd = resolved_command("gh");
    cmd.args([
        "run",
        "list",
        "--json",
        "databaseId,name,status,conclusion,createdAt",
    ]);
    cmd.arg("--limit").arg("10");
    for arg in args {
        cmd.arg(arg);
    }
    run_gh_json(cmd, "run list", |json| format_run_list(json, ultra_compact))
}

fn format_run_list(json: &Value, ultra_compact: bool) -> String {
    let runs = match json.as_array() {
        Some(runs) => runs,
        None => return String::new(),
    };
    let mut out = String::new();
    out.push_str(if ultra_compact {
        "Runs\n"
    } else {
        "Workflow Runs\n"
    });
    for run in runs {
        let id = run["databaseId"].as_i64().unwrap_or(0);
        let name = run["name"].as_str().unwrap_or("???");
        let status = run["status"].as_str().unwrap_or("???");
        let conclusion = run["conclusion"].as_str().unwrap_or("");
        let icon = if ultra_compact {
            match conclusion {
                "success" => "[ok]",
                "failure" => "[x]",
                "cancelled" => "X",
                _ if status == "in_progress" => "~",
                _ => "?",
            }
        } else {
            match conclusion {
                "success" => "[ok]",
                "failure" => "[FAIL]",
                "cancelled" => "[X]",
                _ if status == "in_progress" => "[time]",
                _ => "[pending]",
            }
        };
        out.push_str(&format!("  {} {} [{}]\n", icon, truncate(name, 50), id));
    }
    out
}

/// Check if run view args should bypass filtering and pass through directly.
/// Flags like --log-failed, --log, and --json produce output that the filter
/// would incorrectly strip.
fn should_passthrough_run_view(extra_args: &[String]) -> bool {
    extra_args
        .iter()
        .any(|a| a == "--log-failed" || a == "--log" || a == "--json")
}

fn view_run(args: &[String], _verbose: u8) -> Result<i32> {
    let (run_id, extra_args) = match extract_identifier_and_extra_args(args) {
        Some(result) => result,
        None => return Err(anyhow::anyhow!("Run ID required")),
    };
    if should_passthrough_run_view(&extra_args) {
        return run_passthrough_with_extra("gh", &["run", "view", &run_id], &extra_args);
    }
    let mut cmd = resolved_command("gh");
    cmd.args(["run", "view", &run_id]);
    for arg in &extra_args {
        cmd.arg(arg);
    }
    let run_id_owned = run_id.clone();
    runner::run_filtered(
        cmd,
        "gh",
        &format!("run view {}", run_id),
        move |stdout| format_run_view(stdout, &run_id_owned),
        RunOptions::stdout_only()
            .early_exit_on_failure()
            .no_trailing_newline(),
    )
}

fn format_run_view(stdout: &str, run_id: &str) -> String {
    let mut out = String::new();
    let mut in_jobs = false;

    out.push_str(&format!("Workflow Run #{}\n", run_id));
    for line in stdout.lines() {
        if line.contains("JOBS") {
            in_jobs = true;
        }
        if in_jobs {
            if line.contains('✓') || line.contains("success") {
                continue;
            }
            if line.contains("[x]") || line.contains("fail") {
                out.push_str(&format!("  [FAIL] {}\n", line.trim()));
            }
        } else if line.contains("Status:") || line.contains("Conclusion:") {
            out.push_str(&format!("  {}\n", line.trim()));
        }
    }
    out
}

fn run_repo(args: &[String], _verbose: u8, _ultra_compact: bool) -> Result<i32> {
    let (subcommand, rest_args) = if args.is_empty() {
        ("view", args)
    } else {
        (args[0].as_str(), &args[1..])
    };
    if subcommand != "view" {
        return run_passthrough("gh", "repo", args);
    }
    let mut cmd = resolved_command("gh");
    cmd.arg("repo").arg("view");
    for arg in rest_args {
        cmd.arg(arg);
    }
    cmd.args([
        "--json",
        "name,owner,description,url,stargazerCount,forkCount,isPrivate",
    ]);
    run_gh_json(cmd, "repo view", format_repo_view)
}

fn format_repo_view(json: &Value) -> String {
    let mut out = String::new();
    let name = json["name"].as_str().unwrap_or("???");
    let owner = json["owner"]["login"].as_str().unwrap_or("???");
    let description = json["description"].as_str().unwrap_or("");
    let url = json["url"].as_str().unwrap_or("");
    let stars = json["stargazerCount"].as_i64().unwrap_or(0);
    let forks = json["forkCount"].as_i64().unwrap_or(0);
    let private = json["isPrivate"].as_bool().unwrap_or(false);
    let visibility = if private { "[private]" } else { "[public]" };

    out.push_str(&format!("{}/{}\n", owner, name));
    out.push_str(&format!("  {}\n", visibility));
    if !description.is_empty() {
        out.push_str(&format!("  {}\n", truncate(description, 80)));
    }
    out.push_str(&format!("  {} stars | {} forks\n", stars, forks));
    out.push_str(&format!("  {}\n", url));
    out
}

fn pr_create(args: &[String], _verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("gh");
    cmd.args(["pr", "create"]);
    for arg in args {
        cmd.arg(arg);
    }
    runner::run_filtered(
        cmd,
        "gh",
        "pr create",
        |stdout| {
            let url = stdout.trim();
            let pr_num = url.rsplit('/').next().unwrap_or("");
            let detail = if !pr_num.is_empty() && pr_num.chars().all(|c| c.is_ascii_digit()) {
                format!("#{} {}", pr_num, url)
            } else {
                url.to_string()
            };
            ok_confirmation("created", &detail)
        },
        RunOptions::stdout_only().early_exit_on_failure(),
    )
}

fn pr_merge(args: &[String], _verbose: u8) -> Result<i32> {
    // gh pr merge is a destructive action — pass through the real output
    // so the user (or AI agent) sees exactly what happened.
    run_passthrough("gh", "pr", &{
        let mut a = vec!["merge".to_string()];
        a.extend_from_slice(args);
        a
    })
}

/// Flags that change `gh pr diff` output from unified diff to a different format.
/// When present, compact_diff would produce empty output since it expects diff headers.
fn has_non_diff_format_flag(args: &[String]) -> bool {
    args.iter().any(|a| {
        a == "--name-only"
            || a == "--name-status"
            || a == "--stat"
            || a == "--numstat"
            || a == "--shortstat"
    })
}

fn pr_diff(args: &[String], _verbose: u8) -> Result<i32> {
    let no_compact = args.iter().any(|a| a == "--no-compact");
    let gh_args: Vec<String> = args
        .iter()
        .filter(|a| *a != "--no-compact")
        .cloned()
        .collect();
    if no_compact || has_non_diff_format_flag(&gh_args) {
        return run_passthrough_with_extra("gh", &["pr", "diff"], &gh_args);
    }
    let mut cmd = resolved_command("gh");
    cmd.args(["pr", "diff"]);
    for arg in gh_args.iter() {
        cmd.arg(arg);
    }
    runner::run_filtered(
        cmd,
        "gh",
        "pr diff",
        |raw| {
            if raw.trim().is_empty() {
                "No diff".to_string()
            } else {
                git::compact_diff(raw, 500)
            }
        },
        RunOptions::stdout_only().early_exit_on_failure(),
    )
}

fn pr_action(action: &str, args: &[String], _verbose: u8) -> Result<i32> {
    let subcmd = &args[0];
    let pr_num = args[1..]
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(|s| format!("#{}", s))
        .unwrap_or_default();
    let mut cmd = resolved_command("gh");
    cmd.arg("pr");
    for arg in args {
        cmd.arg(arg);
    }
    let action = action.to_string();
    runner::run_filtered(
        cmd,
        "gh",
        &format!("pr {}", subcmd),
        move |_stdout| ok_confirmation(&action, &pr_num),
        RunOptions::stdout_only().early_exit_on_failure(),
    )
}

fn run_api(args: &[String], _verbose: u8) -> Result<i32> {
    // gh api is an explicit/advanced command — the user knows what they asked for.
    // Converting JSON to a schema destroys all values and forces Claude to re-fetch.
    // Passthrough preserves the full response and tracks metrics at 0% savings.
    run_passthrough("gh", "api", args)
}

// Edge case: error context is now "Failed to run {cmd}" (loses subcommand detail)
fn run_passthrough_with_extra(cmd: &str, base_args: &[&str], extra_args: &[String]) -> Result<i32> {
    let mut os_args: Vec<std::ffi::OsString> =
        base_args.iter().map(std::ffi::OsString::from).collect();
    os_args.extend(extra_args.iter().map(std::ffi::OsString::from));
    crate::core::runner::run_passthrough(cmd, &os_args, 0)
}

fn run_passthrough(cmd: &str, subcommand: &str, args: &[String]) -> Result<i32> {
    let mut os_args: Vec<std::ffi::OsString> = vec![std::ffi::OsString::from(subcommand)];
    os_args.extend(args.iter().map(std::ffi::OsString::from));
    crate::core::runner::run_passthrough(cmd, &os_args, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(
            truncate("this is a very long string", 15),
            "this is a ve..."
        );
    }

    #[test]
    fn test_truncate_multibyte_utf8() {
        // Emoji: 🚀 = 4 bytes, 1 char
        assert_eq!(truncate("🚀🎉🔥abc", 6), "🚀🎉🔥abc"); // 6 chars, fits
        assert_eq!(truncate("🚀🎉🔥abcdef", 8), "🚀🎉🔥ab..."); // 10 chars > 8
                                                                // Edge case: all multibyte
        assert_eq!(truncate("🚀🎉🔥🌟🎯", 5), "🚀🎉🔥🌟🎯"); // exact fit
        assert_eq!(truncate("🚀🎉🔥🌟🎯x", 5), "🚀🎉..."); // 6 chars > 5
    }

    #[test]
    fn test_truncate_empty_and_short() {
        assert_eq!(truncate("", 10), "");
        assert_eq!(truncate("ab", 10), "ab");
        assert_eq!(truncate("abc", 3), "abc"); // exact fit
    }

    #[test]
    fn test_ok_confirmation_pr_create() {
        let result = ok_confirmation("created", "#42 https://github.com/foo/bar/pull/42");
        assert!(result.contains("ok created"));
        assert!(result.contains("#42"));
    }

    #[test]
    fn test_ok_confirmation_pr_merge() {
        let result = ok_confirmation("merged", "#42");
        assert_eq!(result, "ok merged #42");
    }

    #[test]
    fn test_ok_confirmation_pr_comment() {
        let result = ok_confirmation("commented", "#42");
        assert_eq!(result, "ok commented #42");
    }

    #[test]
    fn test_ok_confirmation_pr_edit() {
        let result = ok_confirmation("edited", "#42");
        assert_eq!(result, "ok edited #42");
    }

    #[test]
    fn test_has_json_flag_present() {
        assert!(has_json_flag(&[
            "view".into(),
            "--json".into(),
            "number,url".into()
        ]));
    }

    #[test]
    fn test_has_json_flag_absent() {
        assert!(!has_json_flag(&["view".into(), "42".into()]));
    }

    #[test]
    fn test_extract_identifier_simple() {
        let args: Vec<String> = vec!["123".into()];
        let (id, extra) = extract_identifier_and_extra_args(&args).unwrap();
        assert_eq!(id, "123");
        assert!(extra.is_empty());
    }

    #[test]
    fn test_extract_identifier_with_repo_flag_after() {
        // gh issue view 185 -R rtk-ai/rtk
        let args: Vec<String> = vec!["185".into(), "-R".into(), "rtk-ai/rtk".into()];
        let (id, extra) = extract_identifier_and_extra_args(&args).unwrap();
        assert_eq!(id, "185");
        assert_eq!(extra, vec!["-R", "rtk-ai/rtk"]);
    }

    #[test]
    fn test_extract_identifier_with_repo_flag_before() {
        // gh issue view -R rtk-ai/rtk 185
        let args: Vec<String> = vec!["-R".into(), "rtk-ai/rtk".into(), "185".into()];
        let (id, extra) = extract_identifier_and_extra_args(&args).unwrap();
        assert_eq!(id, "185");
        assert_eq!(extra, vec!["-R", "rtk-ai/rtk"]);
    }

    #[test]
    fn test_extract_identifier_with_long_repo_flag() {
        let args: Vec<String> = vec!["42".into(), "--repo".into(), "owner/repo".into()];
        let (id, extra) = extract_identifier_and_extra_args(&args).unwrap();
        assert_eq!(id, "42");
        assert_eq!(extra, vec!["--repo", "owner/repo"]);
    }

    #[test]
    fn test_extract_identifier_empty() {
        let args: Vec<String> = vec![];
        assert!(extract_identifier_and_extra_args(&args).is_none());
    }

    #[test]
    fn test_extract_identifier_only_flags() {
        // No positional identifier, only flags
        let args: Vec<String> = vec!["-R".into(), "rtk-ai/rtk".into()];
        assert!(extract_identifier_and_extra_args(&args).is_none());
    }

    #[test]
    fn test_extract_identifier_with_web_flag() {
        let args: Vec<String> = vec!["123".into(), "--web".into()];
        let (id, extra) = extract_identifier_and_extra_args(&args).unwrap();
        assert_eq!(id, "123");
        assert_eq!(extra, vec!["--web"]);
    }

    #[test]
    fn test_run_view_passthrough_log_failed() {
        assert!(should_passthrough_run_view(&["--log-failed".into()]));
    }

    #[test]
    fn test_run_view_passthrough_log() {
        assert!(should_passthrough_run_view(&["--log".into()]));
    }

    #[test]
    fn test_run_view_passthrough_json() {
        assert!(should_passthrough_run_view(&[
            "--json".into(),
            "jobs".into()
        ]));
    }

    #[test]
    fn test_run_view_no_passthrough_empty() {
        assert!(!should_passthrough_run_view(&[]));
    }

    #[test]
    fn test_run_view_no_passthrough_other_flags() {
        assert!(!should_passthrough_run_view(&["--web".into()]));
    }

    #[test]
    fn test_extract_identifier_with_job_flag_after() {
        // gh run view 12345 --job 67890
        let args: Vec<String> = vec!["12345".into(), "--job".into(), "67890".into()];
        let (id, extra) = extract_identifier_and_extra_args(&args).unwrap();
        assert_eq!(id, "12345");
        assert_eq!(extra, vec!["--job", "67890"]);
    }

    #[test]
    fn test_extract_identifier_with_job_flag_before() {
        // gh run view --job 67890 12345
        let args: Vec<String> = vec!["--job".into(), "67890".into(), "12345".into()];
        let (id, extra) = extract_identifier_and_extra_args(&args).unwrap();
        assert_eq!(id, "12345");
        assert_eq!(extra, vec!["--job", "67890"]);
    }

    #[test]
    fn test_extract_identifier_with_job_and_log_failed() {
        // gh run view --log-failed --job 67890 12345
        let args: Vec<String> = vec![
            "--log-failed".into(),
            "--job".into(),
            "67890".into(),
            "12345".into(),
        ];
        let (id, extra) = extract_identifier_and_extra_args(&args).unwrap();
        assert_eq!(id, "12345");
        assert_eq!(extra, vec!["--log-failed", "--job", "67890"]);
    }

    #[test]
    fn test_extract_identifier_with_attempt_flag() {
        // gh run view 12345 --attempt 3
        let args: Vec<String> = vec!["12345".into(), "--attempt".into(), "3".into()];
        let (id, extra) = extract_identifier_and_extra_args(&args).unwrap();
        assert_eq!(id, "12345");
        assert_eq!(extra, vec!["--attempt", "3"]);
    }

    // --- should_passthrough_pr_view tests ---

    #[test]
    fn test_should_passthrough_pr_view_json() {
        assert!(should_passthrough_pr_view(&[
            "--json".into(),
            "body,comments".into()
        ]));
    }

    #[test]
    fn test_should_passthrough_pr_view_jq() {
        assert!(should_passthrough_pr_view(&["--jq".into(), ".body".into()]));
    }

    #[test]
    fn test_should_passthrough_pr_view_web() {
        assert!(should_passthrough_pr_view(&["--web".into()]));
    }

    #[test]
    fn test_should_passthrough_pr_view_default() {
        assert!(!should_passthrough_pr_view(&[]));
    }

    #[test]
    fn test_should_passthrough_pr_view_comments() {
        assert!(should_passthrough_pr_view(&["--comments".into()]));
    }

    #[test]
    fn test_should_passthrough_pr_status_help() {
        assert!(should_passthrough_pr_status(&["--help".into()]));
        assert!(should_passthrough_pr_status(&["-h".into()]));
    }

    #[test]
    fn test_should_passthrough_pr_status_output_transform_flags() {
        assert!(should_passthrough_pr_status(&["--web".into()]));
        assert!(should_passthrough_pr_status(&[
            "--jq".into(),
            ".currentBranch".into()
        ]));
        assert!(should_passthrough_pr_status(&[
            "--template".into(),
            "{{.currentBranch.title}}".into()
        ]));
    }

    #[test]
    fn test_should_passthrough_pr_status_repo_flag_stays_filtered() {
        assert!(!should_passthrough_pr_status(&[
            "-R".into(),
            "owner/repo".into()
        ]));
    }

    #[test]
    fn test_pr_status_json_fields_excludes_current_branch() {
        let fields = pr_status_json_fields();
        assert!(!fields.contains("currentBranch"));
        assert!(fields.contains("number"));
        assert!(fields.contains("title"));
        assert!(fields.contains("reviewDecision"));
        assert!(fields.contains("statusCheckRollup"));
    }

    #[test]
    fn test_format_pr_status_includes_current_branch_summary() {
        let json = serde_json::json!({
            "currentBranch": {
                "number": 934,
                "title": "fix wrappers for standardization and exit codes",
                "reviewDecision": "CHANGES_REQUESTED",
                "statusCheckRollup": [
                    {"conclusion": "SUCCESS"},
                    {"state": "SUCCESS"},
                    {"conclusion": "FAILURE"}
                ]
            },
            "createdBy": []
        });

        let result = format_pr_status(&json);
        assert!(result.contains("Current Branch"));
        assert!(result.contains("#934"));
        assert!(result.contains("CHANGES_REQUESTED"));
        assert!(result.contains("checks 2/3"));
        assert!(result.contains("fail 1"));
    }

    // --- should_passthrough_issue_view tests ---

    #[test]
    fn test_should_passthrough_issue_view_comments() {
        assert!(should_passthrough_issue_view(&["--comments".into()]));
    }

    #[test]
    fn test_should_passthrough_issue_view_json() {
        assert!(should_passthrough_issue_view(&[
            "--json".into(),
            "body,comments".into()
        ]));
    }

    #[test]
    fn test_should_passthrough_issue_view_jq() {
        assert!(should_passthrough_issue_view(&[
            "--jq".into(),
            ".body".into()
        ]));
    }

    #[test]
    fn test_should_passthrough_issue_view_web() {
        assert!(should_passthrough_issue_view(&["--web".into()]));
    }

    #[test]
    fn test_should_passthrough_issue_view_default() {
        assert!(!should_passthrough_issue_view(&[]));
    }

    // --- has_non_diff_format_flag tests ---

    #[test]
    fn test_non_diff_format_flag_name_only() {
        assert!(has_non_diff_format_flag(&["--name-only".into()]));
    }

    #[test]
    fn test_non_diff_format_flag_stat() {
        assert!(has_non_diff_format_flag(&["--stat".into()]));
    }

    #[test]
    fn test_non_diff_format_flag_name_status() {
        assert!(has_non_diff_format_flag(&["--name-status".into()]));
    }

    #[test]
    fn test_non_diff_format_flag_numstat() {
        assert!(has_non_diff_format_flag(&["--numstat".into()]));
    }

    #[test]
    fn test_non_diff_format_flag_shortstat() {
        assert!(has_non_diff_format_flag(&["--shortstat".into()]));
    }

    #[test]
    fn test_non_diff_format_flag_absent() {
        assert!(!has_non_diff_format_flag(&[]));
    }

    #[test]
    fn test_non_diff_format_flag_regular_args() {
        assert!(!has_non_diff_format_flag(&[
            "123".into(),
            "--color=always".into()
        ]));
    }

    // --- filter_markdown_body tests ---

    #[test]
    fn test_filter_markdown_body_html_comment_single_line() {
        let input = "Hello\n<!-- this is a comment -->\nWorld";
        let result = filter_markdown_body(input);
        assert!(!result.contains("<!--"));
        assert!(result.contains("Hello"));
        assert!(result.contains("World"));
    }

    #[test]
    fn test_filter_markdown_body_html_comment_multiline() {
        let input = "Before\n<!--\nmultiline\ncomment\n-->\nAfter";
        let result = filter_markdown_body(input);
        assert!(!result.contains("<!--"));
        assert!(!result.contains("multiline"));
        assert!(result.contains("Before"));
        assert!(result.contains("After"));
    }

    #[test]
    fn test_filter_markdown_body_badge_lines() {
        let input = "# Title\n[![CI](https://img.shields.io/badge.svg)](https://github.com/actions)\nSome text";
        let result = filter_markdown_body(input);
        assert!(!result.contains("shields.io"));
        assert!(result.contains("# Title"));
        assert!(result.contains("Some text"));
    }

    #[test]
    fn test_filter_markdown_body_image_only_lines() {
        let input = "# Title\n![screenshot](https://example.com/img.png)\nSome text";
        let result = filter_markdown_body(input);
        assert!(!result.contains("![screenshot]"));
        assert!(result.contains("# Title"));
        assert!(result.contains("Some text"));
    }

    #[test]
    fn test_filter_markdown_body_horizontal_rules() {
        let input = "Section 1\n---\nSection 2\n***\nSection 3\n___\nEnd";
        let result = filter_markdown_body(input);
        assert!(!result.contains("---"));
        assert!(!result.contains("***"));
        assert!(!result.contains("___"));
        assert!(result.contains("Section 1"));
        assert!(result.contains("Section 2"));
        assert!(result.contains("Section 3"));
    }

    #[test]
    fn test_filter_markdown_body_blank_lines_collapse() {
        let input = "Line 1\n\n\n\n\nLine 2";
        let result = filter_markdown_body(input);
        // Should collapse to at most one blank line (2 newlines)
        assert!(!result.contains("\n\n\n"));
        assert!(result.contains("Line 1"));
        assert!(result.contains("Line 2"));
    }

    #[test]
    fn test_filter_markdown_body_code_block_preserved() {
        let input = "Text before\n```python\n<!-- not a comment -->\n![not an image](url)\n---\n```\nText after";
        let result = filter_markdown_body(input);
        // Content inside code block should be preserved
        assert!(result.contains("<!-- not a comment -->"));
        assert!(result.contains("![not an image](url)"));
        assert!(result.contains("---"));
        assert!(result.contains("Text before"));
        assert!(result.contains("Text after"));
    }

    #[test]
    fn test_filter_markdown_body_empty() {
        assert_eq!(filter_markdown_body(""), "");
    }

    #[test]
    fn test_filter_markdown_body_meaningful_content_preserved() {
        let input = "## Summary\n- Item 1\n- Item 2\n\n[Link](https://example.com)\n\n| Col1 | Col2 |\n| --- | --- |\n| a | b |";
        let result = filter_markdown_body(input);
        assert!(result.contains("## Summary"));
        assert!(result.contains("- Item 1"));
        assert!(result.contains("- Item 2"));
        assert!(result.contains("[Link](https://example.com)"));
        assert!(result.contains("| Col1 | Col2 |"));
    }

    #[test]
    fn test_filter_markdown_body_token_savings() {
        // Realistic PR body with noise
        let input = r#"<!-- This PR template is auto-generated -->
<!-- Please fill in the following sections -->

## Summary

Added smart markdown filtering for gh issue/pr view commands.

[![CI](https://img.shields.io/github/actions/workflow/status/rtk-ai/rtk/ci.yml)](https://github.com/rtk-ai/rtk/actions)
[![Coverage](https://img.shields.io/codecov/c/github/rtk-ai/rtk)](https://codecov.io/gh/rtk-ai/rtk)

![screenshot](https://user-images.githubusercontent.com/123/screenshot.png)

---

## Changes

- Filter HTML comments
- Filter badge lines
- Filter image-only lines
- Collapse blank lines

***

## Test Plan

- [x] Unit tests added
- [x] Snapshot tests pass
- [ ] Manual testing

___

<!-- Do not edit below this line -->
<!-- Auto-generated footer -->"#;

        let result = filter_markdown_body(input);

        fn count_tokens(text: &str) -> usize {
            text.split_whitespace().count()
        }

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 30.0,
            "Expected ≥30% savings, got {:.1}% (input: {} tokens, output: {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );

        // Verify meaningful content preserved
        assert!(result.contains("## Summary"));
        assert!(result.contains("## Changes"));
        assert!(result.contains("## Test Plan"));
        assert!(result.contains("Filter HTML comments"));
    }
}
