use crate::tracking;
use anyhow::{Context, Result};
use std::process::Command;

#[derive(Debug, Clone)]
pub enum GitCommand {
    Diff,
    Log,
    Status,
    Show,
    Add { files: Vec<String> },
    Commit { message: String },
    Push,
    Pull,
    Branch,
    Fetch,
    Stash { subcommand: Option<String> },
    Worktree,
}

pub fn run(cmd: GitCommand, args: &[String], max_lines: Option<usize>, verbose: u8) -> Result<()> {
    match cmd {
        GitCommand::Diff => run_diff(args, max_lines, verbose),
        GitCommand::Log => run_log(args, max_lines, verbose),
        GitCommand::Status => run_status(verbose),
        GitCommand::Show => run_show(args, max_lines, verbose),
        GitCommand::Add { files } => run_add(&files, verbose),
        GitCommand::Commit { message } => run_commit(&message, verbose),
        GitCommand::Push => run_push(args, verbose),
        GitCommand::Pull => run_pull(args, verbose),
        GitCommand::Branch => run_branch(args, verbose),
        GitCommand::Fetch => run_fetch(args, verbose),
        GitCommand::Stash { subcommand } => run_stash(subcommand.as_deref(), args, verbose),
        GitCommand::Worktree => run_worktree(args, verbose),
    }
}

fn run_diff(args: &[String], max_lines: Option<usize>, verbose: u8) -> Result<()> {
    // Check if user wants stat output
    let wants_stat = args
        .iter()
        .any(|arg| arg == "--stat" || arg == "--numstat" || arg == "--shortstat");

    // Check if user wants compact diff (default RTK behavior)
    let wants_compact = !args.iter().any(|arg| arg == "--no-compact");

    if wants_stat || !wants_compact {
        // User wants stat or explicitly no compacting - pass through directly
        let mut cmd = Command::new("git");
        cmd.arg("diff");
        for arg in args {
            cmd.arg(arg);
        }

        let output = cmd.output().context("Failed to run git diff")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("{}", stderr);
            std::process::exit(output.status.code().unwrap_or(1));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        println!("{}", stdout.trim());
        return Ok(());
    }

    // Default RTK behavior: stat first, then compacted diff
    let mut cmd = Command::new("git");
    cmd.arg("diff").arg("--stat");

    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run git diff")?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    if verbose > 0 {
        eprintln!("Git diff summary:");
    }

    // Print stat summary first
    println!("{}", stdout.trim());

    // Now get actual diff but compact it
    let mut diff_cmd = Command::new("git");
    diff_cmd.arg("diff");
    for arg in args {
        diff_cmd.arg(arg);
    }

    let diff_output = diff_cmd.output().context("Failed to run git diff")?;
    let diff_stdout = String::from_utf8_lossy(&diff_output.stdout);

    if !diff_stdout.is_empty() {
        println!("\n--- Changes ---");
        let compacted = compact_diff(&diff_stdout, max_lines.unwrap_or(100));
        println!("{}", compacted);
    }

    Ok(())
}

fn run_show(args: &[String], max_lines: Option<usize>, verbose: u8) -> Result<()> {
    // If user wants --stat or --format only, pass through
    let wants_stat_only = args
        .iter()
        .any(|arg| arg == "--stat" || arg == "--numstat" || arg == "--shortstat");

    let wants_format = args
        .iter()
        .any(|arg| arg.starts_with("--pretty") || arg.starts_with("--format"));

    if wants_stat_only || wants_format {
        let mut cmd = Command::new("git");
        cmd.arg("show");
        for arg in args {
            cmd.arg(arg);
        }
        let output = cmd.output().context("Failed to run git show")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("{}", stderr);
            std::process::exit(output.status.code().unwrap_or(1));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        println!("{}", stdout.trim());
        return Ok(());
    }

    // Get raw output for tracking
    let mut raw_cmd = Command::new("git");
    raw_cmd.arg("show");
    for arg in args {
        raw_cmd.arg(arg);
    }
    let raw_output = raw_cmd
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    // Step 1: one-line commit summary
    let mut summary_cmd = Command::new("git");
    summary_cmd.args(["show", "--no-patch", "--pretty=format:%h %s (%ar) <%an>"]);
    for arg in args {
        summary_cmd.arg(arg);
    }
    let summary_output = summary_cmd.output().context("Failed to run git show")?;
    if !summary_output.status.success() {
        let stderr = String::from_utf8_lossy(&summary_output.stderr);
        eprintln!("{}", stderr);
        std::process::exit(summary_output.status.code().unwrap_or(1));
    }
    let summary = String::from_utf8_lossy(&summary_output.stdout);
    println!("{}", summary.trim());

    // Step 2: --stat summary
    let mut stat_cmd = Command::new("git");
    stat_cmd.args(["show", "--stat", "--pretty=format:"]);
    for arg in args {
        stat_cmd.arg(arg);
    }
    let stat_output = stat_cmd.output().context("Failed to run git show --stat")?;
    let stat_stdout = String::from_utf8_lossy(&stat_output.stdout);
    let stat_text = stat_stdout.trim();
    if !stat_text.is_empty() {
        println!("{}", stat_text);
    }

    // Step 3: compacted diff
    let mut diff_cmd = Command::new("git");
    diff_cmd.args(["show", "--pretty=format:"]);
    for arg in args {
        diff_cmd.arg(arg);
    }
    let diff_output = diff_cmd.output().context("Failed to run git show (diff)")?;
    let diff_stdout = String::from_utf8_lossy(&diff_output.stdout);
    let diff_text = diff_stdout.trim();

    if !diff_text.is_empty() {
        if verbose > 0 {
            println!("\n--- Changes ---");
        }
        let compacted = compact_diff(diff_text, max_lines.unwrap_or(100));
        println!("{}", compacted);
    }

    tracking::track("git show", "rtk git show", &raw_output, &summary);

    Ok(())
}

pub(crate) fn compact_diff(diff: &str, max_lines: usize) -> String {
    let mut result = Vec::new();
    let mut current_file = String::new();
    let mut added = 0;
    let mut removed = 0;
    let mut in_hunk = false;
    let mut hunk_lines = 0;
    let max_hunk_lines = 10;

    for line in diff.lines() {
        if line.starts_with("diff --git") {
            // New file
            if !current_file.is_empty() && (added > 0 || removed > 0) {
                result.push(format!("  +{} -{}", added, removed));
            }
            current_file = line.split(" b/").nth(1).unwrap_or("unknown").to_string();
            result.push(format!("\n📄 {}", current_file));
            added = 0;
            removed = 0;
            in_hunk = false;
        } else if line.starts_with("@@") {
            // New hunk
            in_hunk = true;
            hunk_lines = 0;
            let hunk_info = line.split("@@").nth(1).unwrap_or("").trim();
            result.push(format!("  @@ {} @@", hunk_info));
        } else if in_hunk {
            if line.starts_with('+') && !line.starts_with("+++") {
                added += 1;
                if hunk_lines < max_hunk_lines {
                    result.push(format!("  {}", line));
                    hunk_lines += 1;
                }
            } else if line.starts_with('-') && !line.starts_with("---") {
                removed += 1;
                if hunk_lines < max_hunk_lines {
                    result.push(format!("  {}", line));
                    hunk_lines += 1;
                }
            } else if hunk_lines < max_hunk_lines && !line.starts_with("\\") {
                // Context line
                if hunk_lines > 0 {
                    result.push(format!("  {}", line));
                    hunk_lines += 1;
                }
            }

            if hunk_lines == max_hunk_lines {
                result.push("  ... (truncated)".to_string());
                hunk_lines += 1;
            }
        }

        if result.len() >= max_lines {
            result.push("\n... (more changes truncated)".to_string());
            break;
        }
    }

    if !current_file.is_empty() && (added > 0 || removed > 0) {
        result.push(format!("  +{} -{}", added, removed));
    }

    result.join("\n")
}

fn run_log(args: &[String], _max_lines: Option<usize>, verbose: u8) -> Result<()> {
    let mut cmd = Command::new("git");
    cmd.arg("log");

    // Check if user provided format flags
    let has_format_flag = args.iter().any(|arg| {
        arg.starts_with("--oneline") || arg.starts_with("--pretty") || arg.starts_with("--format")
    });

    // Check if user provided limit flag
    let has_limit_flag = args.iter().any(|arg| {
        arg.starts_with('-') && arg.chars().nth(1).map_or(false, |c| c.is_ascii_digit())
    });

    // Apply RTK defaults only if user didn't specify them
    if !has_format_flag {
        cmd.args(["--pretty=format:%h %s (%ar) <%an>"]);
    }

    if !has_limit_flag {
        cmd.arg("-10");
    }

    // Only add --no-merges if user didn't explicitly request merge commits
    let wants_merges = args
        .iter()
        .any(|arg| arg == "--merges" || arg == "--min-parents=2");
    if !wants_merges {
        cmd.arg("--no-merges");
    }

    // Pass all user arguments
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run git log")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("{}", stderr);
        // Propagate git's exit code
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    if verbose > 0 {
        eprintln!("Git log output:");
    }

    println!("{}", stdout.trim());

    Ok(())
}

fn run_status(_verbose: u8) -> Result<()> {
    // Get raw git status for tracking
    let raw_output = Command::new("git")
        .args(["status"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    let output = Command::new("git")
        .args(["status", "--porcelain", "-b"])
        .output()
        .context("Failed to run git status")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();

    if lines.is_empty() {
        println!("Clean working tree");
        tracking::track(
            "git status",
            "rtk git status",
            &raw_output,
            "Clean working tree",
        );
        return Ok(());
    }

    // Parse branch info
    if let Some(branch_line) = lines.first() {
        if branch_line.starts_with("##") {
            let branch = branch_line.trim_start_matches("## ");
            println!("📌 {}", branch);
        }
    }

    // Count changes by type
    let mut staged = 0;
    let mut modified = 0;
    let mut untracked = 0;
    let mut conflicts = 0;

    let mut staged_files = Vec::new();
    let mut modified_files = Vec::new();
    let mut untracked_files = Vec::new();

    for line in lines.iter().skip(1) {
        if line.len() < 3 {
            continue;
        }
        let status = &line[0..2];
        let file = &line[3..];

        match status.chars().next().unwrap_or(' ') {
            'M' | 'A' | 'D' | 'R' | 'C' => {
                staged += 1;
                staged_files.push(file);
            }
            'U' => conflicts += 1,
            _ => {}
        }

        match status.chars().nth(1).unwrap_or(' ') {
            'M' | 'D' => {
                modified += 1;
                modified_files.push(file);
            }
            _ => {}
        }

        if status == "??" {
            untracked += 1;
            untracked_files.push(file);
        }
    }

    // Print summary
    if staged > 0 {
        println!("✅ Staged: {} files", staged);
        for f in staged_files.iter().take(5) {
            println!("   {}", f);
        }
        if staged_files.len() > 5 {
            println!("   ... +{} more", staged_files.len() - 5);
        }
    }

    if modified > 0 {
        println!("📝 Modified: {} files", modified);
        for f in modified_files.iter().take(5) {
            println!("   {}", f);
        }
        if modified_files.len() > 5 {
            println!("   ... +{} more", modified_files.len() - 5);
        }
    }

    if untracked > 0 {
        println!("❓ Untracked: {} files", untracked);
        for f in untracked_files.iter().take(3) {
            println!("   {}", f);
        }
        if untracked_files.len() > 3 {
            println!("   ... +{} more", untracked_files.len() - 3);
        }
    }

    if conflicts > 0 {
        println!("⚠️  Conflicts: {} files", conflicts);
    }

    // Estimate output size for tracking
    let rtk_output = format!(
        "branch + {} staged + {} modified + {} untracked",
        staged, modified, untracked
    );
    tracking::track("git status", "rtk git status", &raw_output, &rtk_output);

    Ok(())
}

fn run_add(files: &[String], verbose: u8) -> Result<()> {
    let mut cmd = Command::new("git");
    cmd.arg("add");

    if files.is_empty() {
        cmd.arg(".");
    } else {
        for f in files {
            cmd.arg(f);
        }
    }

    let output = cmd.output().context("Failed to run git add")?;

    if verbose > 0 {
        eprintln!("git add executed");
    }

    if output.status.success() {
        // Count what was added
        let status_output = Command::new("git")
            .args(["diff", "--cached", "--stat", "--shortstat"])
            .output()
            .context("Failed to check staged files")?;

        let stat = String::from_utf8_lossy(&status_output.stdout);
        if stat.trim().is_empty() {
            println!("ok (nothing to add)");
        } else {
            // Parse "1 file changed, 5 insertions(+)" format
            let short = stat.lines().last().unwrap_or("").trim();
            if short.is_empty() {
                println!("ok ✓");
            } else {
                println!("ok ✓ {}", short);
            }
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        eprintln!("FAILED: git add");
        if !stderr.trim().is_empty() {
            eprintln!("{}", stderr);
        }
        if !stdout.trim().is_empty() {
            eprintln!("{}", stdout);
        }
    }

    Ok(())
}

fn run_commit(message: &str, verbose: u8) -> Result<()> {
    if verbose > 0 {
        eprintln!("git commit -m \"{}\"", message);
    }

    let output = Command::new("git")
        .args(["commit", "-m", message])
        .output()
        .context("Failed to run git commit")?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Extract commit hash from output like "[main abc1234] message"
        if let Some(line) = stdout.lines().next() {
            if let Some(hash_start) = line.find(' ') {
                let hash = line[1..hash_start].split(' ').last().unwrap_or("");
                if !hash.is_empty() && hash.len() >= 7 {
                    println!("ok ✓ {}", &hash[..7.min(hash.len())]);
                    return Ok(());
                }
            }
        }
        println!("ok ✓");
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stderr.contains("nothing to commit") || stdout.contains("nothing to commit") {
            println!("ok (nothing to commit)");
        } else {
            eprintln!("FAILED: git commit");
            if !stderr.trim().is_empty() {
                eprintln!("{}", stderr);
            }
            if !stdout.trim().is_empty() {
                eprintln!("{}", stdout);
            }
        }
    }

    Ok(())
}

fn run_push(args: &[String], verbose: u8) -> Result<()> {
    if verbose > 0 {
        eprintln!("git push");
    }

    let mut cmd = Command::new("git");
    cmd.arg("push");
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run git push")?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let raw = format!("{}{}", stdout, stderr);

    if output.status.success() {
        if stderr.contains("Everything up-to-date") {
            println!("ok (up-to-date)");
            tracking::track("git push", "rtk git push", &raw, "ok (up-to-date)");
        } else {
            for line in stderr.lines() {
                if line.contains("->") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 3 {
                        let msg = format!("ok ✓ {}", parts[parts.len() - 1]);
                        println!("{}", msg);
                        tracking::track("git push", "rtk git push", &raw, &msg);
                        return Ok(());
                    }
                }
            }
            println!("ok ✓");
            tracking::track("git push", "rtk git push", &raw, "ok ✓");
        }
    } else {
        eprintln!("FAILED: git push");
        if !stderr.trim().is_empty() {
            eprintln!("{}", stderr);
        }
        if !stdout.trim().is_empty() {
            eprintln!("{}", stdout);
        }
    }

    Ok(())
}

fn run_pull(args: &[String], verbose: u8) -> Result<()> {
    if verbose > 0 {
        eprintln!("git pull");
    }

    let mut cmd = Command::new("git");
    cmd.arg("pull");
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run git pull")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        if stdout.contains("Already up to date") || stdout.contains("Already up-to-date") {
            println!("ok (up-to-date)");
        } else {
            // Count files changed
            let mut files = 0;
            let mut insertions = 0;
            let mut deletions = 0;

            for line in stdout.lines() {
                if line.contains("file") && line.contains("changed") {
                    // Parse "3 files changed, 10 insertions(+), 2 deletions(-)"
                    for part in line.split(',') {
                        let part = part.trim();
                        if part.contains("file") {
                            files = part
                                .split_whitespace()
                                .next()
                                .and_then(|n| n.parse().ok())
                                .unwrap_or(0);
                        } else if part.contains("insertion") {
                            insertions = part
                                .split_whitespace()
                                .next()
                                .and_then(|n| n.parse().ok())
                                .unwrap_or(0);
                        } else if part.contains("deletion") {
                            deletions = part
                                .split_whitespace()
                                .next()
                                .and_then(|n| n.parse().ok())
                                .unwrap_or(0);
                        }
                    }
                }
            }

            if files > 0 {
                println!("ok ✓ {} files +{} -{}", files, insertions, deletions);
            } else {
                println!("ok ✓");
            }
        }
    } else {
        eprintln!("FAILED: git pull");
        if !stderr.trim().is_empty() {
            eprintln!("{}", stderr);
        }
        if !stdout.trim().is_empty() {
            eprintln!("{}", stdout);
        }
    }

    Ok(())
}

fn run_branch(args: &[String], verbose: u8) -> Result<()> {
    if verbose > 0 {
        eprintln!("git branch");
    }

    let mut cmd = Command::new("git");
    cmd.arg("branch");

    // If user passes flags like -d, -D, -m, pass through directly
    let has_action_flag = args
        .iter()
        .any(|a| a == "-d" || a == "-D" || a == "-m" || a == "-M" || a == "-c" || a == "-C");

    if has_action_flag {
        for arg in args {
            cmd.arg(arg);
        }
        let output = cmd.output().context("Failed to run git branch")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.success() {
            println!("ok ✓");
        } else {
            eprintln!("FAILED: git branch");
            if !stderr.trim().is_empty() {
                eprintln!("{}", stderr);
            }
            if !stdout.trim().is_empty() {
                eprintln!("{}", stdout);
            }
        }
        return Ok(());
    }

    // List mode: show compact branch list
    cmd.arg("-a").arg("--no-color");
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run git branch")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let raw = stdout.to_string();

    let filtered = filter_branch_output(&stdout);
    println!("{}", filtered);

    tracking::track("git branch -a", "rtk git branch", &raw, &filtered);

    Ok(())
}

fn filter_branch_output(output: &str) -> String {
    let mut current = String::new();
    let mut local: Vec<String> = Vec::new();
    let mut remote: Vec<String> = Vec::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(branch) = line.strip_prefix("* ") {
            current = branch.to_string();
        } else if line.starts_with("remotes/origin/") {
            let branch = line.strip_prefix("remotes/origin/").unwrap_or(line);
            // Skip HEAD pointer
            if branch.starts_with("HEAD ") {
                continue;
            }
            remote.push(branch.to_string());
        } else {
            local.push(line.to_string());
        }
    }

    let mut result = Vec::new();
    result.push(format!("* {}", current));

    if !local.is_empty() {
        for b in &local {
            result.push(format!("  {}", b));
        }
    }

    if !remote.is_empty() {
        // Filter out remotes that already exist locally
        let remote_only: Vec<&String> = remote
            .iter()
            .filter(|r| *r != &current && !local.contains(r))
            .collect();
        if !remote_only.is_empty() {
            result.push(format!("  remote-only ({}):", remote_only.len()));
            for b in remote_only.iter().take(10) {
                result.push(format!("    {}", b));
            }
            if remote_only.len() > 10 {
                result.push(format!("    ... +{} more", remote_only.len() - 10));
            }
        }
    }

    result.join("\n")
}

fn run_fetch(args: &[String], verbose: u8) -> Result<()> {
    if verbose > 0 {
        eprintln!("git fetch");
    }

    let mut cmd = Command::new("git");
    cmd.arg("fetch");
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run git fetch")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}{}", stdout, stderr);

    if !output.status.success() {
        eprintln!("FAILED: git fetch");
        if !stderr.trim().is_empty() {
            eprintln!("{}", stderr);
        }
        return Ok(());
    }

    // Count new refs from stderr (git fetch outputs to stderr)
    let new_refs: usize = stderr
        .lines()
        .filter(|l| l.contains("->") || l.contains("[new"))
        .count();

    let msg = if new_refs > 0 {
        format!("ok fetched ({} new refs)", new_refs)
    } else {
        "ok fetched".to_string()
    };

    println!("{}", msg);
    tracking::track("git fetch", "rtk git fetch", &raw, &msg);

    Ok(())
}

fn run_stash(subcommand: Option<&str>, args: &[String], verbose: u8) -> Result<()> {
    if verbose > 0 {
        eprintln!("git stash {:?}", subcommand);
    }

    match subcommand {
        Some("list") => {
            let output = Command::new("git")
                .args(["stash", "list"])
                .output()
                .context("Failed to run git stash list")?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let raw = stdout.to_string();

            if stdout.trim().is_empty() {
                let msg = "No stashes";
                println!("{}", msg);
                tracking::track("git stash list", "rtk git stash list", &raw, msg);
                return Ok(());
            }

            let filtered = filter_stash_list(&stdout);
            println!("{}", filtered);
            tracking::track("git stash list", "rtk git stash list", &raw, &filtered);
        }
        Some("show") => {
            let mut cmd = Command::new("git");
            cmd.args(["stash", "show", "-p"]);
            for arg in args {
                cmd.arg(arg);
            }
            let output = cmd.output().context("Failed to run git stash show")?;
            let stdout = String::from_utf8_lossy(&output.stdout);

            if stdout.trim().is_empty() {
                println!("Empty stash");
            } else {
                let compacted = compact_diff(&stdout, 100);
                println!("{}", compacted);
            }
        }
        Some("pop") | Some("apply") | Some("drop") | Some("push") => {
            let sub = subcommand.unwrap();
            let mut cmd = Command::new("git");
            cmd.args(["stash", sub]);
            for arg in args {
                cmd.arg(arg);
            }
            let output = cmd.output().context("Failed to run git stash")?;
            if output.status.success() {
                println!("ok stash {}", sub);
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!("FAILED: git stash {}", sub);
                if !stderr.trim().is_empty() {
                    eprintln!("{}", stderr);
                }
            }
        }
        _ => {
            // Default: git stash (push)
            let mut cmd = Command::new("git");
            cmd.arg("stash");
            for arg in args {
                cmd.arg(arg);
            }
            let output = cmd.output().context("Failed to run git stash")?;
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if stdout.contains("No local changes") {
                    println!("ok (nothing to stash)");
                } else {
                    println!("ok stashed");
                }
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!("FAILED: git stash");
                if !stderr.trim().is_empty() {
                    eprintln!("{}", stderr);
                }
            }
        }
    }

    Ok(())
}

fn filter_stash_list(output: &str) -> String {
    // Format: "stash@{0}: WIP on main: abc1234 commit message"
    let mut result = Vec::new();
    for line in output.lines() {
        if let Some(colon_pos) = line.find(": ") {
            let index = &line[..colon_pos];
            let rest = &line[colon_pos + 2..];
            // Compact: strip "WIP on branch:" prefix if present
            let message = if let Some(second_colon) = rest.find(": ") {
                rest[second_colon + 2..].trim()
            } else {
                rest.trim()
            };
            result.push(format!("{}: {}", index, message));
        } else {
            result.push(line.to_string());
        }
    }
    result.join("\n")
}

fn run_worktree(args: &[String], verbose: u8) -> Result<()> {
    if verbose > 0 {
        eprintln!("git worktree list");
    }

    // If args contain "add", "remove", "prune" etc., pass through
    let has_action = args.iter().any(|a| {
        a == "add" || a == "remove" || a == "prune" || a == "lock" || a == "unlock" || a == "move"
    });

    if has_action {
        let mut cmd = Command::new("git");
        cmd.arg("worktree");
        for arg in args {
            cmd.arg(arg);
        }
        let output = cmd.output().context("Failed to run git worktree")?;
        if output.status.success() {
            println!("ok ✓");
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("FAILED: git worktree {}", args.join(" "));
            if !stderr.trim().is_empty() {
                eprintln!("{}", stderr);
            }
        }
        return Ok(());
    }

    // Default: list mode
    let output = Command::new("git")
        .args(["worktree", "list"])
        .output()
        .context("Failed to run git worktree list")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let raw = stdout.to_string();

    let filtered = filter_worktree_list(&stdout);
    println!("{}", filtered);
    tracking::track("git worktree list", "rtk git worktree", &raw, &filtered);

    Ok(())
}

fn filter_worktree_list(output: &str) -> String {
    let home = dirs::home_dir()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut result = Vec::new();
    for line in output.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // Format: "/path/to/worktree  abc1234 [branch]"
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 {
            let mut path = parts[0].to_string();
            if !home.is_empty() && path.starts_with(&home) {
                path = format!("~{}", &path[home.len()..]);
            }
            let hash = parts[1];
            let branch = parts[2..].join(" ");
            result.push(format!("{} {} {}", path, hash, branch));
        } else {
            result.push(line.to_string());
        }
    }
    result.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compact_diff() {
        let diff = r#"diff --git a/foo.rs b/foo.rs
--- a/foo.rs
+++ b/foo.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("hello");
 }
"#;
        let result = compact_diff(diff, 100);
        assert!(result.contains("foo.rs"));
        assert!(result.contains("+"));
    }

    #[test]
    fn test_filter_branch_output() {
        let output = "* main\n  feature/auth\n  fix/bug-123\n  remotes/origin/HEAD -> origin/main\n  remotes/origin/main\n  remotes/origin/feature/auth\n  remotes/origin/release/v2\n";
        let result = filter_branch_output(output);
        assert!(result.contains("* main"));
        assert!(result.contains("feature/auth"));
        assert!(result.contains("fix/bug-123"));
        // remote-only should show release/v2 but not main or feature/auth (already local)
        assert!(result.contains("remote-only"));
        assert!(result.contains("release/v2"));
    }

    #[test]
    fn test_filter_branch_no_remotes() {
        let output = "* main\n  develop\n";
        let result = filter_branch_output(output);
        assert!(result.contains("* main"));
        assert!(result.contains("develop"));
        assert!(!result.contains("remote-only"));
    }

    #[test]
    fn test_filter_stash_list() {
        let output =
            "stash@{0}: WIP on main: abc1234 fix login\nstash@{1}: On feature: def5678 wip\n";
        let result = filter_stash_list(output);
        assert!(result.contains("stash@{0}: abc1234 fix login"));
        assert!(result.contains("stash@{1}: def5678 wip"));
    }

    #[test]
    fn test_filter_worktree_list() {
        let output =
            "/home/user/project  abc1234 [main]\n/home/user/worktrees/feat  def5678 [feature]\n";
        let result = filter_worktree_list(output);
        assert!(result.contains("abc1234"));
        assert!(result.contains("[main]"));
        assert!(result.contains("[feature]"));
    }
}
