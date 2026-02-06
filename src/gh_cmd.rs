//! GitHub CLI (gh) command output compression.
//!
//! Provides token-optimized alternatives to verbose `gh` commands.
//! Focuses on extracting essential information from JSON outputs.

use crate::git;
use crate::json_cmd;
use crate::tracking;
use crate::utils::{ok_confirmation, truncate};
use anyhow::{Context, Result};
use serde_json::Value;
use std::process::Command;

/// Run a gh command with token-optimized output
pub fn run(subcommand: &str, args: &[String], verbose: u8, ultra_compact: bool) -> Result<()> {
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
        "comment" => pr_action("commented", &args[1..], verbose),
        "edit" => pr_action("edited", &args[1..], verbose),
        _ => run_passthrough("gh", "pr", args),
    }
}

fn list_prs(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("gh");
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
            filtered.push_str("📋 Pull Requests\n");
            println!("📋 Pull Requests");
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
                    "OPEN" => "🟢",
                    "MERGED" => "🟣",
                    "CLOSED" => "🔴",
                    _ => "⚪",
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

fn view_pr(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if args.is_empty() {
        return Err(anyhow::anyhow!("PR number required"));
    }

    let pr_number = &args[0];

    let mut cmd = Command::new("gh");
    cmd.args([
        "pr",
        "view",
        pr_number,
        "--json",
        "number,title,state,author,body,url,mergeable,reviews,statusCheckRollup",
    ]);

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
            "OPEN" => "🟢",
            "MERGED" => "🟣",
            "CLOSED" => "🔴",
            _ => "⚪",
        }
    };

    let line = format!("{} PR #{}: {}\n", state_icon, number, title);
    filtered.push_str(&line);
    print!("{}", line);

    let line = format!("  {}\n", author);
    filtered.push_str(&line);
    print!("{}", line);

    let mergeable_str = match mergeable {
        "MERGEABLE" => "✓",
        "CONFLICTING" => "✗",
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
                let line = format!("  ✗{}/{}  {} fail\n", passed, total, failed);
                filtered.push_str(&line);
                print!("{}", line);
            } else {
                let line = format!("  ✓{}/{}\n", passed, total);
                filtered.push_str(&line);
                print!("{}", line);
            }
        } else {
            let line = format!("  Checks: {}/{} passed\n", passed, total);
            filtered.push_str(&line);
            print!("{}", line);
            if failed > 0 {
                let line = format!("  ⚠️  {} checks failed\n", failed);
                filtered.push_str(&line);
                print!("{}", line);
            }
        }
    }

    let line = format!("  {}\n", url);
    filtered.push_str(&line);
    print!("{}", line);

    // Show body summary (first 3 lines max)
    if let Some(body) = json["body"].as_str() {
        if !body.is_empty() {
            filtered.push('\n');
            println!();
            for line in body.lines().take(3) {
                if !line.trim().is_empty() {
                    let formatted = format!("  {}\n", truncate(line, 80));
                    filtered.push_str(&formatted);
                    print!("{}", formatted);
                }
            }
            if body.lines().count() > 3 {
                let line = format!("  ... (gh pr view {} for full)\n", pr_number);
                filtered.push_str(&line);
                print!("{}", line);
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

    if args.is_empty() {
        return Err(anyhow::anyhow!("PR number required"));
    }

    let pr_number = &args[0];

    let mut cmd = Command::new("gh");
    cmd.args(["pr", "checks", pr_number]);

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
        if line.contains('✓') || line.contains("pass") {
            passed += 1;
        } else if line.contains('✗') || line.contains("fail") {
            failed += 1;
            failed_checks.push(line.trim().to_string());
        } else if line.contains('*') || line.contains("pending") {
            pending += 1;
        }
    }

    let mut filtered = String::new();

    let line = "🔍 CI Checks Summary:\n";
    filtered.push_str(line);
    print!("{}", line);

    let line = format!("  ✅ Passed: {}\n", passed);
    filtered.push_str(&line);
    print!("{}", line);

    let line = format!("  ❌ Failed: {}\n", failed);
    filtered.push_str(&line);
    print!("{}", line);

    if pending > 0 {
        let line = format!("  ⏳ Pending: {}\n", pending);
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

    let mut cmd = Command::new("gh");
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
        let line = format!("📝 Your PRs ({}):\n", created_by.len());
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

    let mut cmd = Command::new("gh");
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
        if ultra_compact {
            filtered.push_str("Issues\n");
            println!("Issues");
        } else {
            filtered.push_str("🐛 Issues\n");
            println!("🐛 Issues");
        }
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
                    "🟢"
                } else {
                    "🔴"
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

    if args.is_empty() {
        return Err(anyhow::anyhow!("Issue number required"));
    }

    let issue_number = &args[0];

    let mut cmd = Command::new("gh");
    cmd.args([
        "issue",
        "view",
        issue_number,
        "--json",
        "number,title,state,author,body,url",
    ]);

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

    let icon = if state == "OPEN" { "🟢" } else { "🔴" };

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
            let line = "\n  Description:\n";
            filtered.push_str(line);
            print!("{}", line);
            for line in body.lines().take(3) {
                if !line.trim().is_empty() {
                    let formatted = format!("    {}\n", truncate(line, 80));
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

    let mut cmd = Command::new("gh");
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
            filtered.push_str("🏃 Workflow Runs\n");
            println!("🏃 Workflow Runs");
        }
        for run in runs {
            let id = run["databaseId"].as_i64().unwrap_or(0);
            let name = run["name"].as_str().unwrap_or("???");
            let status = run["status"].as_str().unwrap_or("???");
            let conclusion = run["conclusion"].as_str().unwrap_or("");

            let icon = if ultra_compact {
                match conclusion {
                    "success" => "✓",
                    "failure" => "✗",
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
                    "success" => "✅",
                    "failure" => "❌",
                    "cancelled" => "🚫",
                    _ => {
                        if status == "in_progress" {
                            "⏳"
                        } else {
                            "⚪"
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

fn view_run(args: &[String], _verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if args.is_empty() {
        return Err(anyhow::anyhow!("Run ID required"));
    }

    let run_id = &args[0];

    let mut cmd = Command::new("gh");
    cmd.args(["run", "view", run_id]);

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

    let line = format!("🏃 Workflow Run #{}\n", run_id);
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
            if line.contains('✗') || line.contains("fail") {
                let formatted = format!("  ❌ {}\n", line.trim());
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

    let mut cmd = Command::new("gh");
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

    let visibility = if private {
        "🔒 Private"
    } else {
        "🌐 Public"
    };

    let mut filtered = String::new();

    let line = format!("📦 {}/{}\n", owner, name);
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

    let line = format!("  ⭐ {} stars | 🔱 {} forks\n", stars, forks);
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

    let mut cmd = Command::new("gh");
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

    let mut cmd = Command::new("gh");
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
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("gh");
    cmd.args(["pr", "diff"]);
    for arg in args {
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
        let compacted = git::compact_diff(&raw, 100);
        println!("{}", compacted);
        compacted
    };

    timer.track("gh pr diff", "rtk gh pr diff", &raw, &filtered);
    Ok(())
}

/// Generic PR action handler for comment/edit
fn pr_action(action: &str, args: &[String], _verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("gh");
    cmd.args(["pr", action]);
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd
        .output()
        .context(format!("Failed to run gh pr {}", action))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        timer.track(
            &format!("gh pr {}", action),
            &format!("rtk gh pr {}", action),
            &stderr,
            &stderr,
        );
        eprintln!("{}", stderr.trim());
        std::process::exit(output.status.code().unwrap_or(1));
    }

    // Extract PR number from args
    let pr_num = args
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
        &format!("gh pr {}", action),
        &format!("rtk gh pr {}", action),
        &raw,
        &filtered,
    );
    Ok(())
}

fn run_api(args: &[String], _verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("gh");
    cmd.arg("api");
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run gh api")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        timer.track("gh api", "rtk gh api", &stderr, &stderr);
        eprintln!("{}", stderr.trim());
        std::process::exit(output.status.code().unwrap_or(1));
    }

    // Try to parse as JSON and filter
    let filtered = match json_cmd::filter_json_string(&raw, 5) {
        Ok(schema) => {
            println!("{}", schema);
            schema
        }
        Err(_) => {
            // Not JSON, print truncated raw output
            let mut result = String::new();
            let lines: Vec<&str> = raw.lines().take(20).collect();
            let joined = lines.join("\n");
            result.push_str(&joined);
            print!("{}", joined);
            if raw.lines().count() > 20 {
                result.push_str("\n... (truncated)");
                println!("\n... (truncated)");
            }
            result
        }
    };

    timer.track("gh api", "rtk gh api", &raw, &filtered);
    Ok(())
}

fn run_passthrough(cmd: &str, subcommand: &str, args: &[String]) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut command = Command::new(cmd);
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
}
