//! GitHub CLI (gh) command output compression.
//!
//! Provides token-optimized alternatives to verbose `gh` commands.
//! Focuses on extracting essential information from JSON outputs.

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
        _ => run_passthrough("gh", "pr", args),
    }
}

fn list_prs(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<()> {
    let mut cmd = Command::new("gh");
    cmd.args(["pr", "list", "--json", "number,title,state,author,updatedAt"]);

    // Pass through additional flags
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run gh pr list")?;

    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let json: Value = serde_json::from_slice(&output.stdout)
        .context("Failed to parse gh pr list output")?;

    if let Some(prs) = json.as_array() {
        if ultra_compact {
            println!("PRs");
        } else {
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

            println!("  {} #{} {} ({})", state_icon, number, truncate(title, 60), author);
        }

        if prs.len() > 20 {
            println!("  ... {} more (use gh pr list for all)", prs.len() - 20);
        }
    }

    Ok(())
}

fn view_pr(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<()> {
    if args.is_empty() {
        return Err(anyhow::anyhow!("PR number required"));
    }

    let pr_number = &args[0];

    let mut cmd = Command::new("gh");
    cmd.args([
        "pr", "view", pr_number,
        "--json", "number,title,state,author,body,url,mergeable,reviews,statusCheckRollup"
    ]);

    let output = cmd.output().context("Failed to run gh pr view")?;

    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let json: Value = serde_json::from_slice(&output.stdout)
        .context("Failed to parse gh pr view output")?;

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

    println!("{} PR #{}: {}", state_icon, number, title);
    println!("  {}", author);
    let mergeable_str = match mergeable {
        "MERGEABLE" => "✓",
        "CONFLICTING" => "✗",
        _ => "?",
    };
    println!("  {} | {}", state, mergeable_str);

    // Show reviews summary
    if let Some(reviews) = json["reviews"]["nodes"].as_array() {
        let approved = reviews.iter().filter(|r| r["state"].as_str() == Some("APPROVED")).count();
        let changes = reviews.iter().filter(|r| r["state"].as_str() == Some("CHANGES_REQUESTED")).count();

        if approved > 0 || changes > 0 {
            println!("  Reviews: {} approved, {} changes requested", approved, changes);
        }
    }

    // Show checks summary
    if let Some(checks) = json["statusCheckRollup"].as_array() {
        let total = checks.len();
        let passed = checks.iter().filter(|c| {
            c["conclusion"].as_str() == Some("SUCCESS") || c["state"].as_str() == Some("SUCCESS")
        }).count();
        let failed = checks.iter().filter(|c| {
            c["conclusion"].as_str() == Some("FAILURE") || c["state"].as_str() == Some("FAILURE")
        }).count();

        if ultra_compact {
            if failed > 0 {
                println!("  ✗{}/{}  {} fail", passed, total, failed);
            } else {
                println!("  ✓{}/{}", passed, total);
            }
        } else {
            println!("  Checks: {}/{} passed", passed, total);
            if failed > 0 {
                println!("  ⚠️  {} checks failed", failed);
            }
        }
    }

    println!("  {}", url);

    // Show body summary (first 3 lines max)
    if let Some(body) = json["body"].as_str() {
        if !body.is_empty() {
            println!();
            for line in body.lines().take(3) {
                if !line.trim().is_empty() {
                    println!("  {}", truncate(line, 80));
                }
            }
            if body.lines().count() > 3 {
                println!("  ... (gh pr view {} for full)", pr_number);
            }
        }
    }

    Ok(())
}

fn pr_checks(args: &[String], _verbose: u8, _ultra_compact: bool) -> Result<()> {
    if args.is_empty() {
        return Err(anyhow::anyhow!("PR number required"));
    }

    let pr_number = &args[0];

    let mut cmd = Command::new("gh");
    cmd.args(["pr", "checks", pr_number]);

    let output = cmd.output().context("Failed to run gh pr checks")?;

    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
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

    println!("🔍 CI Checks Summary:");
    println!("  ✅ Passed: {}", passed);
    println!("  ❌ Failed: {}", failed);
    if pending > 0 {
        println!("  ⏳ Pending: {}", pending);
    }

    if !failed_checks.is_empty() {
        println!("\n  Failed checks:");
        for check in failed_checks {
            println!("    {}", check);
        }
    }

    Ok(())
}

fn pr_status(_verbose: u8, _ultra_compact: bool) -> Result<()> {
    let mut cmd = Command::new("gh");
    cmd.args(["pr", "status", "--json", "currentBranch,createdBy,reviewDecision,statusCheckRollup"]);

    let output = cmd.output().context("Failed to run gh pr status")?;

    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let json: Value = serde_json::from_slice(&output.stdout)
        .context("Failed to parse gh pr status output")?;

    if let Some(created_by) = json["createdBy"].as_array() {
        println!("📝 Your PRs ({}):", created_by.len());
        for pr in created_by.iter().take(5) {
            let number = pr["number"].as_i64().unwrap_or(0);
            let title = pr["title"].as_str().unwrap_or("???");
            let reviews = pr["reviewDecision"].as_str().unwrap_or("PENDING");
            println!("  #{} {} [{}]", number, truncate(title, 50), reviews);
        }
    }

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
    let mut cmd = Command::new("gh");
    cmd.args(["issue", "list", "--json", "number,title,state,author"]);

    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run gh issue list")?;

    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let json: Value = serde_json::from_slice(&output.stdout)
        .context("Failed to parse gh issue list output")?;

    if let Some(issues) = json.as_array() {
        if ultra_compact {
            println!("Issues");
        } else {
            println!("🐛 Issues");
        }
        for issue in issues.iter().take(20) {
            let number = issue["number"].as_i64().unwrap_or(0);
            let title = issue["title"].as_str().unwrap_or("???");
            let state = issue["state"].as_str().unwrap_or("???");

            let icon = if ultra_compact {
                if state == "OPEN" { "O" } else { "C" }
            } else {
                if state == "OPEN" { "🟢" } else { "🔴" }
            };
            println!("  {} #{} {}", icon, number, truncate(title, 60));
        }

        if issues.len() > 20 {
            println!("  ... {} more", issues.len() - 20);
        }
    }

    Ok(())
}

fn view_issue(args: &[String], _verbose: u8) -> Result<()> {
    if args.is_empty() {
        return Err(anyhow::anyhow!("Issue number required"));
    }

    let issue_number = &args[0];

    let mut cmd = Command::new("gh");
    cmd.args(["issue", "view", issue_number, "--json", "number,title,state,author,body,url"]);

    let output = cmd.output().context("Failed to run gh issue view")?;

    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let json: Value = serde_json::from_slice(&output.stdout)
        .context("Failed to parse gh issue view output")?;

    let number = json["number"].as_i64().unwrap_or(0);
    let title = json["title"].as_str().unwrap_or("???");
    let state = json["state"].as_str().unwrap_or("???");
    let author = json["author"]["login"].as_str().unwrap_or("???");
    let url = json["url"].as_str().unwrap_or("");

    let icon = if state == "OPEN" { "🟢" } else { "🔴" };

    println!("{} Issue #{}: {}", icon, number, title);
    println!("  Author: @{}", author);
    println!("  Status: {}", state);
    println!("  URL: {}", url);

    if let Some(body) = json["body"].as_str() {
        if !body.is_empty() {
            println!("\n  Description:");
            for line in body.lines().take(3) {
                if !line.trim().is_empty() {
                    println!("    {}", truncate(line, 80));
                }
            }
        }
    }

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
    let mut cmd = Command::new("gh");
    cmd.args(["run", "list", "--json", "databaseId,name,status,conclusion,createdAt"]);
    cmd.arg("--limit").arg("10");

    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run gh run list")?;

    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let json: Value = serde_json::from_slice(&output.stdout)
        .context("Failed to parse gh run list output")?;

    if let Some(runs) = json.as_array() {
        if ultra_compact {
            println!("Runs");
        } else {
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
                    _ => if status == "in_progress" { "~" } else { "?" },
                }
            } else {
                match conclusion {
                    "success" => "✅",
                    "failure" => "❌",
                    "cancelled" => "🚫",
                    _ => if status == "in_progress" { "⏳" } else { "⚪" },
                }
            };

            println!("  {} {} [{}]", icon, truncate(name, 50), id);
        }
    }

    Ok(())
}

fn view_run(args: &[String], _verbose: u8) -> Result<()> {
    if args.is_empty() {
        return Err(anyhow::anyhow!("Run ID required"));
    }

    let run_id = &args[0];

    let mut cmd = Command::new("gh");
    cmd.args(["run", "view", run_id]);

    let output = cmd.output().context("Failed to run gh run view")?;

    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        std::process::exit(output.status.code().unwrap_or(1));
    }

    // Parse output and show only failures
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut in_jobs = false;

    println!("🏃 Workflow Run #{}", run_id);

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
                println!("  ❌ {}", line.trim());
            }
        } else if line.contains("Status:") || line.contains("Conclusion:") {
            println!("  {}", line.trim());
        }
    }

    Ok(())
}

fn run_repo(args: &[String], _verbose: u8, _ultra_compact: bool) -> Result<()> {
    // Parse subcommand (default to "view")
    let (subcommand, rest_args) = if args.is_empty() {
        ("view", &args[..])
    } else {
        (args[0].as_str(), &args[1..])
    };

    if subcommand != "view" {
        return run_passthrough("gh", "repo", args);
    }

    let mut cmd = Command::new("gh");
    cmd.arg("repo").arg("view");

    for arg in rest_args {
        cmd.arg(arg);
    }

    cmd.args(["--json", "name,owner,description,url,stargazerCount,forkCount,isPrivate"]);

    let output = cmd.output().context("Failed to run gh repo view")?;

    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let json: Value = serde_json::from_slice(&output.stdout)
        .context("Failed to parse gh repo view output")?;

    let name = json["name"].as_str().unwrap_or("???");
    let owner = json["owner"]["login"].as_str().unwrap_or("???");
    let description = json["description"].as_str().unwrap_or("");
    let url = json["url"].as_str().unwrap_or("");
    let stars = json["stargazerCount"].as_i64().unwrap_or(0);
    let forks = json["forkCount"].as_i64().unwrap_or(0);
    let private = json["isPrivate"].as_bool().unwrap_or(false);

    let visibility = if private { "🔒 Private" } else { "🌐 Public" };

    println!("📦 {}/{}", owner, name);
    println!("  {}", visibility);
    if !description.is_empty() {
        println!("  {}", truncate(description, 80));
    }
    println!("  ⭐ {} stars | 🔱 {} forks", stars, forks);
    println!("  {}", url);

    Ok(())
}

fn run_passthrough(cmd: &str, subcommand: &str, args: &[String]) -> Result<()> {
    let mut command = Command::new(cmd);
    command.arg(subcommand);
    for arg in args {
        command.arg(arg);
    }

    let status = command
        .status()
        .context(format!("Failed to run {} {}", cmd, subcommand))?;

    std::process::exit(status.code().unwrap_or(1));
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("this is a very long string", 15), "this is a ve...");
    }
}
