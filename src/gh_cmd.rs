//! GitHub CLI (gh) command output compression.
//!
//! Provides token-optimized alternatives to verbose `gh` commands.
//! Focuses on extracting essential information from JSON outputs.

use crate::git;
use crate::tracking;
use crate::utils::{ok_confirmation, resolved_command, truncate};
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use serde_json::Value;

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

/// Run a gh command with token-optimized output
pub fn run(subcommand: &str, args: &[String], verbose: u8, ultra_compact: bool) -> Result<()> {
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

fn run_pr(args: &[String], verbose: u8, ultra_compact: bool) -> Result<()> {
    if args.is_empty() {
        return run_passthrough("gh", "pr", args);
    }

    match args[0].as_str() {
        "list" => list_prs(&args[1..], verbose, ultra_compact),
        "view" => view_pr(&args[1..], verbose, ultra_compact),
        "checks" => pr_checks(&args[1..], verbose, ultra_compact),
        "status" => pr_status(verbose, ultra_compact),
        "create" => pr_create(&args[1..], verbose),
        "merge" => pr_merge(&args[1..], verbose),
        "diff" => pr_diff(&args[1..], verbose),
        "comment" => pr_action("commented", args, verbose),
        "edit" => pr_action("edited", args, verbose),
        _ => run_passthrough("gh", "pr", args),
    }
}

fn list_prs(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("gh");
    cmd.args([
        "pr",
        "list",
        "--json",
        "number,title,state,author,updatedAt",
    ]);

    // Pass through additional flags
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run gh pr list")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        timer.track("gh pr list", "rtk gh pr list", &stderr, &stderr);
        eprintln!("{}", stderr.trim());
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let json: Value =
        serde_json::from_slice(&output.stdout).context("Failed to parse gh pr list output")?;

    let mut filtered = String::new();

    if let Some(prs) = json.as_array() {
        if ultra_compact {
            filtered.push_str("PRs\n");
            println!("PRs");
        } else {
            filtered.push_str("Pull Requests\n");
            println!("Pull Requests");
        }

        for pr in prs.iter().take(20) {
            let number = pr["number"].as_i64().unwrap_or(0);
            let title = pr["title"].as_str().unwrap_or("???");
            let state = pr["state"].as_str().unwrap_or("???");
            let author = pr["author"]["login"].as_str().unwrap_or("???");

            let state_icon = if ultra_compact {
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
            };

            let line = format!(
                "  {} #{} {} ({})\n",
                state_icon,
                number,
                truncate(title, 60),
                author
            );
            filtered.push_str(&line);
            print!("{}", line);
        }

        if prs.len() > 20 {
            let more_line = format!("  ... {} more (use gh pr list for all)\n", prs.len() - 20);
            filtered.push_str(&more_line);
            print!("{}", more_line);
        }
    }

    timer.track("gh pr list", "rtk gh pr list", &raw, &filtered);
    Ok(())
}

fn should_passthrough_pr_view(extra_args: &[String]) -> bool {
    extra_args
        .iter()
        .any(|a| a == "--json" || a == "--jq" || a == "--web")
}

fn view_pr(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let (pr_number, extra_args) = match extract_identifier_and_extra_args(args) {
        Some(result) => result,
        None => return Err(anyhow::anyhow!("PR number required")),
    };

    // If the user provides --jq or --web, pass through directly.
    // Note: --json is already handled globally by run() via has_json_flag.
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

    let output = cmd.output().context("Failed to run gh pr view")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        timer.track(
            &format!("gh pr view {}", pr_number),
            &format!("rtk gh pr view {}", pr_number),
            &stderr,
            &stderr,
        );
        eprintln!("{}", stderr.trim());
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let json: Value =
        serde_json::from_slice(&output.stdout).context("Failed to parse gh pr view output")?;

    let mut filtered = String::new();

    // Extract essential info
    let number = json["number"].as_i64().unwrap_or(0);
    let title = json["title"].as_str().unwrap_or("???");
    let state = json["state"].as_str().unwrap_or("???");
    let author = json["author"]["login"].as_str().unwrap_or("???");
    let url = json["url"].as_str().unwrap_or("");
    let mergeable = json["mergeable"].as_str().unwrap_or("UNKNOWN");

    let state_icon = if ultra_compact {
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
    };

    let line = format!("{} PR #{}: {}\n", state_icon, number, title);
    filtered.push_str(&line);
    print!("{}", line);

    let line = format!("  {}\n", author);
    filtered.push_str(&line);
    print!("{}", line);

    let mergeable_str = match mergeable {
        "MERGEABLE" => "[ok]",
        "CONFLICTING" => "[x]",
        _ => "?",
    };
    let line = format!("  {} | {}\n", state, mergeable_str);
    filtered.push_str(&line);
    print!("{}", line);

    // Show reviews summary
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
            let line = format!(
                "  Reviews: {} approved, {} changes requested\n",
                approved, changes
            );
            filtered.push_str(&line);
            print!("{}", line);
        }
    }

    // Show checks summary
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
                let line = format!("  [x]{}/{}  {} fail\n", passed, total, failed);
                filtered.push_str(&line);
                print!("{}", line);
            } else {
                let line = format!("  {}/{}\n", passed, total);
                filtered.push_str(&line);
                print!("{}", line);
            }
        } else {
            let line = format!("  Checks: {}/{} passed\n", passed, total);
            filtered.push_str(&line);
            print!("{}", line);
            if failed > 0 {
                let line = format!("  [warn] {} checks failed\n", failed);
                filtered.push_str(&line);
                print!("{}", line);
            }
        }
    }

    let line = format!("  {}\n", url);
    filtered.push_str(&line);
    print!("{}", line);

    // Show filtered body
    if let Some(body) = json["body"].as_str() {
        if !body.is_empty() {
            let body_filtered = filter_markdown_body(body);
            if !body_filtered.is_empty() {
                filtered.push('\n');
                println!();
                for line in body_filtered.lines() {
                    let formatted = format!("  {}\n", line);
                    filtered.push_str(&formatted);
                    print!("{}", formatted);
                }
            }
        }
    }

    timer.track(
        &format!("gh pr view {}", pr_number),
        &format!("rtk gh pr view {}", pr_number),
        &raw,
        &filtered,
    );
    Ok(())
}

fn pr_checks(args: &[String], _verbose: u8, _ultra_compact: bool) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let (pr_number, extra_args) = match extract_identifier_and_extra_args(args) {
        Some(result) => result,
        None => return Err(anyhow::anyhow!("PR number required")),
    };

    let mut cmd = resolved_command("gh");
    cmd.args(["pr", "checks", &pr_number]);
    for arg in &extra_args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run gh pr checks")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        timer.track(
            &format!("gh pr checks {}", pr_number),
            &format!("rtk gh pr checks {}", pr_number),
            &stderr,
            &stderr,
        );
        eprintln!("{}", stderr.trim());
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse and compress checks output
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

    let mut filtered = String::new();

    let line = "CI Checks Summary:\n";
    filtered.push_str(line);
    print!("{}", line);

    let line = format!("  [ok] Passed: {}\n", passed);
    filtered.push_str(&line);
    print!("{}", line);

    let line = format!("  [FAIL] Failed: {}\n", failed);
    filtered.push_str(&line);
    print!("{}", line);

    if pending > 0 {
        let line = format!("  [pending] Pending: {}\n", pending);
        filtered.push_str(&line);
        print!("{}", line);
    }

    if !failed_checks.is_empty() {
        let line = "\n  Failed checks:\n";
        filtered.push_str(line);
        print!("{}", line);
        for check in failed_checks {
            let line = format!("    {}\n", check);
            filtered.push_str(&line);
            print!("{}", line);
        }
    }

    timer.track(
        &format!("gh pr checks {}", pr_number),
        &format!("rtk gh pr checks {}", pr_number),
        &raw,
        &filtered,
    );
    Ok(())
}

fn pr_status(_verbose: u8, _ultra_compact: bool) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("gh");
    cmd.args([
        "pr",
        "status",
        "--json",
        "currentBranch,createdBy,reviewDecision,statusCheckRollup",
    ]);

    let output = cmd.output().context("Failed to run gh pr status")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        timer.track("gh pr status", "rtk gh pr status", &stderr, &stderr);
        eprintln!("{}", stderr.trim());
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let json: Value =
        serde_json::from_slice(&output.stdout).context("Failed to parse gh pr status output")?;

    let mut filtered = String::new();

    if let Some(created_by) = json["createdBy"].as_array() {
        let line = format!("Your PRs ({}):\n", created_by.len());
        filtered.push_str(&line);
        print!("{}", line);
        for pr in created_by.iter().take(5) {
            let number = pr["number"].as_i64().unwrap_or(0);
            let title = pr["title"].as_str().unwrap_or("???");
            let reviews = pr["reviewDecision"].as_str().unwrap_or("PENDING");
            let line = format!("  #{} {} [{}]\n", number, truncate(title, 50), reviews);
            filtered.push_str(&line);
            print!("{}", line);
        }
    }

    timer.track("gh pr status", "rtk gh pr status", &raw, &filtered);
    Ok(())
}

fn run_issue(args: &[String], verbose: u8, ultra_compact: bool) -> Result<()> {
    if args.is_empty() {
        return run_passthrough("gh", "issue", args);
    }

    match args[0].as_str() {
        "list" => list_issues(&args[1..], verbose, ultra_compact),
        "view" => view_issue(&args[1..], verbose),
        _ => run_passthrough("gh", "issue", args),
    }
}

fn list_issues(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("gh");
    cmd.args(["issue", "list", "--json", "number,title,state,author"]);

    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run gh issue list")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        timer.track("gh issue list", "rtk gh issue list", &stderr, &stderr);
        eprintln!("{}", stderr.trim());
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let json: Value =
        serde_json::from_slice(&output.stdout).context("Failed to parse gh issue list output")?;

    let mut filtered = String::new();

    if let Some(issues) = json.as_array() {
        filtered.push_str("Issues\n");
        println!("Issues");
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
            } else {
                if state == "OPEN" {
                    "[open]"
                } else {
                    "[closed]"
                }
            };
            let line = format!("  {} #{} {}\n", icon, number, truncate(title, 60));
            filtered.push_str(&line);
            print!("{}", line);
        }

        if issues.len() > 20 {
            let line = format!("  ... {} more\n", issues.len() - 20);
            filtered.push_str(&line);
            print!("{}", line);
        }
    }

    timer.track("gh issue list", "rtk gh issue list", &raw, &filtered);
    Ok(())
}

fn view_issue(args: &[String], _verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let (issue_number, extra_args) = match extract_identifier_and_extra_args(args) {
        Some(result) => result,
        None => return Err(anyhow::anyhow!("Issue number required")),
    };

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

    let output = cmd.output().context("Failed to run gh issue view")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        timer.track(
            &format!("gh issue view {}", issue_number),
            &format!("rtk gh issue view {}", issue_number),
            &stderr,
            &stderr,
        );
        eprintln!("{}", stderr.trim());
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let json: Value =
        serde_json::from_slice(&output.stdout).context("Failed to parse gh issue view output")?;

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

    let mut filtered = String::new();

    let line = format!("{} Issue #{}: {}\n", icon, number, title);
    filtered.push_str(&line);
    print!("{}", line);

    let line = format!("  Author: @{}\n", author);
    filtered.push_str(&line);
    print!("{}", line);

    let line = format!("  Status: {}\n", state);
    filtered.push_str(&line);
    print!("{}", line);

    let line = format!("  URL: {}\n", url);
    filtered.push_str(&line);
    print!("{}", line);

    if let Some(body) = json["body"].as_str() {
        if !body.is_empty() {
            let body_filtered = filter_markdown_body(body);
            if !body_filtered.is_empty() {
                let line = "\n  Description:\n";
                filtered.push_str(line);
                print!("{}", line);
                for line in body_filtered.lines() {
                    let formatted = format!("    {}\n", line);
                    filtered.push_str(&formatted);
                    print!("{}", formatted);
                }
            }
        }
    }

    timer.track(
        &format!("gh issue view {}", issue_number),
        &format!("rtk gh issue view {}", issue_number),
        &raw,
        &filtered,
    );
    Ok(())
}

fn run_workflow(args: &[String], verbose: u8, ultra_compact: bool) -> Result<()> {
    if args.is_empty() {
        return run_passthrough("gh", "run", args);
    }

    match args[0].as_str() {
        "list" => list_runs(&args[1..], verbose, ultra_compact),
        "view" => view_run(&args[1..], verbose),
        _ => run_passthrough("gh", "run", args),
    }
}

fn list_runs(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<()> {
    let timer = tracking::TimedExecution::start();

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

    let output = cmd.output().context("Failed to run gh run list")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        timer.track("gh run list", "rtk gh run list", &stderr, &stderr);
        eprintln!("{}", stderr.trim());
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let json: Value =
        serde_json::from_slice(&output.stdout).context("Failed to parse gh run list output")?;

    let mut filtered = String::new();

    if let Some(runs) = json.as_array() {
        if ultra_compact {
            filtered.push_str("Runs\n");
            println!("Runs");
        } else {
            filtered.push_str("Workflow Runs\n");
            println!("Workflow Runs");
        }
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
                    _ => {
                        if status == "in_progress" {
                            "~"
                        } else {
                            "?"
                        }
                    }
                }
            } else {
                match conclusion {
                    "success" => "[ok]",
                    "failure" => "[FAIL]",
                    "cancelled" => "[X]",
                    _ => {
                        if status == "in_progress" {
                            "[time]"
                        } else {
                            "[pending]"
                        }
                    }
                }
            };

            let line = format!("  {} {} [{}]\n", icon, truncate(name, 50), id);
            filtered.push_str(&line);
            print!("{}", line);
        }
    }

    timer.track("gh run list", "rtk gh run list", &raw, &filtered);
    Ok(())
}

/// Check if run view args should bypass filtering and pass through directly.
/// Flags like --log-failed, --log, and --json produce output that the filter
/// would incorrectly strip.
fn should_passthrough_run_view(extra_args: &[String]) -> bool {
    extra_args
        .iter()
        .any(|a| a == "--log-failed" || a == "--log" || a == "--json")
}

fn view_run(args: &[String], _verbose: u8) -> Result<()> {
    let (run_id, extra_args) = match extract_identifier_and_extra_args(args) {
        Some(result) => result,
        None => return Err(anyhow::anyhow!("Run ID required")),
    };

    // Pass through when user requests logs or JSON — the filter would strip them
    if should_passthrough_run_view(&extra_args) {
        return run_passthrough_with_extra("gh", &["run", "view", &run_id], &extra_args);
    }

    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("gh");
    cmd.args(["run", "view", &run_id]);
    for arg in &extra_args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run gh run view")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        timer.track(
            &format!("gh run view {}", run_id),
            &format!("rtk gh run view {}", run_id),
            &stderr,
            &stderr,
        );
        eprintln!("{}", stderr.trim());
        std::process::exit(output.status.code().unwrap_or(1));
    }

    // Parse output and show only failures
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut in_jobs = false;

    let mut filtered = String::new();

    let line = format!("Workflow Run #{}\n", run_id);
    filtered.push_str(&line);
    print!("{}", line);

    for line in stdout.lines() {
        if line.contains("JOBS") {
            in_jobs = true;
        }

        if in_jobs {
            if line.contains('✓') || line.contains("success") {
                // Skip successful jobs in compact mode
                continue;
            }
            if line.contains("[x]") || line.contains("fail") {
                let formatted = format!("  [FAIL] {}\n", line.trim());
                filtered.push_str(&formatted);
                print!("{}", formatted);
            }
        } else if line.contains("Status:") || line.contains("Conclusion:") {
            let formatted = format!("  {}\n", line.trim());
            filtered.push_str(&formatted);
            print!("{}", formatted);
        }
    }

    timer.track(
        &format!("gh run view {}", run_id),
        &format!("rtk gh run view {}", run_id),
        &raw,
        &filtered,
    );
    Ok(())
}

fn run_repo(args: &[String], _verbose: u8, _ultra_compact: bool) -> Result<()> {
    // Parse subcommand (default to "view")
    let (subcommand, rest_args) = if args.is_empty() {
        ("view", args)
    } else {
        (args[0].as_str(), &args[1..])
    };

    if subcommand != "view" {
        return run_passthrough("gh", "repo", args);
    }

    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("gh");
    cmd.arg("repo").arg("view");

    for arg in rest_args {
        cmd.arg(arg);
    }

    cmd.args([
        "--json",
        "name,owner,description,url,stargazerCount,forkCount,isPrivate",
    ]);

    let output = cmd.output().context("Failed to run gh repo view")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        timer.track("gh repo view", "rtk gh repo view", &stderr, &stderr);
        eprintln!("{}", stderr.trim());
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let json: Value =
        serde_json::from_slice(&output.stdout).context("Failed to parse gh repo view output")?;

    let name = json["name"].as_str().unwrap_or("???");
    let owner = json["owner"]["login"].as_str().unwrap_or("???");
    let description = json["description"].as_str().unwrap_or("");
    let url = json["url"].as_str().unwrap_or("");
    let stars = json["stargazerCount"].as_i64().unwrap_or(0);
    let forks = json["forkCount"].as_i64().unwrap_or(0);
    let private = json["isPrivate"].as_bool().unwrap_or(false);

    let visibility = if private { "[private]" } else { "[public]" };

    let mut filtered = String::new();

    let line = format!("{}/{}\n", owner, name);
    filtered.push_str(&line);
    print!("{}", line);

    let line = format!("  {}\n", visibility);
    filtered.push_str(&line);
    print!("{}", line);

    if !description.is_empty() {
        let line = format!("  {}\n", truncate(description, 80));
        filtered.push_str(&line);
        print!("{}", line);
    }

    let line = format!("  {} stars | {} forks\n", stars, forks);
    filtered.push_str(&line);
    print!("{}", line);

    let line = format!("  {}\n", url);
    filtered.push_str(&line);
    print!("{}", line);

    timer.track("gh repo view", "rtk gh repo view", &raw, &filtered);
    Ok(())
}

fn pr_create(args: &[String], _verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("gh");
    cmd.args(["pr", "create"]);
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run gh pr create")?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        timer.track("gh pr create", "rtk gh pr create", &stderr, &stderr);
        eprintln!("{}", stderr.trim());
        std::process::exit(output.status.code().unwrap_or(1));
    }

    // gh pr create outputs the URL on success
    let url = stdout.trim();

    // Try to extract PR number from URL (e.g., https://github.com/owner/repo/pull/42)
    let pr_num = url.rsplit('/').next().unwrap_or("");

    let detail = if !pr_num.is_empty() && pr_num.chars().all(|c| c.is_ascii_digit()) {
        format!("#{} {}", pr_num, url)
    } else {
        url.to_string()
    };

    let filtered = ok_confirmation("created", &detail);
    println!("{}", filtered);

    timer.track("gh pr create", "rtk gh pr create", &stdout, &filtered);
    Ok(())
}

fn pr_merge(args: &[String], _verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("gh");
    cmd.args(["pr", "merge"]);
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run gh pr merge")?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        timer.track("gh pr merge", "rtk gh pr merge", &stderr, &stderr);
        eprintln!("{}", stderr.trim());
        std::process::exit(output.status.code().unwrap_or(1));
    }

    // Extract PR number from args (first non-flag arg)
    let pr_num = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(|s| s.as_str())
        .unwrap_or("");

    let detail = if !pr_num.is_empty() {
        format!("#{}", pr_num)
    } else {
        String::new()
    };

    let filtered = ok_confirmation("merged", &detail);
    println!("{}", filtered);

    // Use stdout or detail as raw input (gh pr merge doesn't output much)
    let raw = if !stdout.trim().is_empty() {
        stdout
    } else {
        detail.clone()
    };

    timer.track("gh pr merge", "rtk gh pr merge", &raw, &filtered);
    Ok(())
}

fn pr_diff(args: &[String], _verbose: u8) -> Result<()> {
    // --no-compact: pass full diff through (gh CLI doesn't know this flag, strip it)
    let no_compact = args.iter().any(|a| a == "--no-compact");
    let gh_args: Vec<String> = args
        .iter()
        .filter(|a| *a != "--no-compact")
        .cloned()
        .collect();

    if no_compact {
        return run_passthrough_with_extra("gh", &["pr", "diff"], &gh_args);
    }

    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("gh");
    cmd.args(["pr", "diff"]);
    for arg in gh_args.iter() {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run gh pr diff")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        timer.track("gh pr diff", "rtk gh pr diff", &stderr, &stderr);
        eprintln!("{}", stderr.trim());
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let filtered = if raw.trim().is_empty() {
        let msg = "No diff\n";
        print!("{}", msg);
        msg.to_string()
    } else {
        let compacted = git::compact_diff(&raw, 500);
        println!("{}", compacted);
        compacted
    };

    timer.track("gh pr diff", "rtk gh pr diff", &raw, &filtered);
    Ok(())
}

/// Generic PR action handler for comment/edit
fn pr_action(action: &str, args: &[String], _verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let subcmd = &args[0];

    let mut cmd = resolved_command("gh");
    cmd.arg("pr");
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd
        .output()
        .context(format!("Failed to run gh pr {}", subcmd))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        timer.track(
            &format!("gh pr {}", subcmd),
            &format!("rtk gh pr {}", subcmd),
            &stderr,
            &stderr,
        );
        eprintln!("{}", stderr.trim());
        std::process::exit(output.status.code().unwrap_or(1));
    }

    // Extract PR number from args (skip args[0] which is the subcommand)
    let pr_num = args[1..]
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(|s| format!("#{}", s))
        .unwrap_or_default();

    let filtered = ok_confirmation(action, &pr_num);
    println!("{}", filtered);

    // Use stdout or pr_num as raw input
    let raw = if !stdout.trim().is_empty() {
        stdout
    } else {
        pr_num.clone()
    };

    timer.track(
        &format!("gh pr {}", subcmd),
        &format!("rtk gh pr {}", subcmd),
        &raw,
        &filtered,
    );
    Ok(())
}

fn run_api(args: &[String], _verbose: u8) -> Result<()> {
    // gh api is an explicit/advanced command — the user knows what they asked for.
    // Converting JSON to a schema destroys all values and forces Claude to re-fetch.
    // Passthrough preserves the full response and tracks metrics at 0% savings.
    run_passthrough("gh", "api", args)
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
    fn test_should_passthrough_pr_view_other_flags() {
        assert!(!should_passthrough_pr_view(&["--comments".into()]));
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
