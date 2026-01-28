use anyhow::{Context, Result};
use std::process::Command;
use crate::tracking;

#[derive(Debug, Clone)]
pub enum GitCommand {
    Diff,
    Log,
    Status,
    Add { files: Vec<String> },
    Commit { message: String },
    Push,
    Pull,
}

pub fn run(cmd: GitCommand, args: &[String], max_lines: Option<usize>, verbose: u8) -> Result<()> {
    match cmd {
        GitCommand::Diff => run_diff(args, max_lines, verbose),
        GitCommand::Log => run_log(args, max_lines, verbose),
        GitCommand::Status => run_status(verbose),
        GitCommand::Add { files } => run_add(&files, verbose),
        GitCommand::Commit { message } => run_commit(&message, verbose),
        GitCommand::Push => run_push(verbose),
        GitCommand::Pull => run_pull(verbose),
    }
}

fn run_diff(args: &[String], max_lines: Option<usize>, verbose: u8) -> Result<()> {
    // Check if user wants stat output
    let wants_stat = args.iter().any(|arg| arg == "--stat" || arg == "--numstat" || arg == "--shortstat");

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

fn compact_diff(diff: &str, max_lines: usize) -> String {
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
            current_file = line
                .split(" b/")
                .nth(1)
                .unwrap_or("unknown")
                .to_string();
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
        arg.starts_with("--oneline")
            || arg.starts_with("--pretty")
            || arg.starts_with("--format")
    });

    // Check if user provided limit flag
    let has_limit_flag = args.iter().any(|arg| arg.starts_with('-') && arg.chars().nth(1).map_or(false, |c| c.is_ascii_digit()));

    // Apply RTK defaults only if user didn't specify them
    if !has_format_flag {
        cmd.args(["--pretty=format:%h %s (%ar) <%an>"]);
    }

    if !has_limit_flag {
        cmd.arg("-10");
    }

    // Only add --no-merges if user didn't explicitly request merge commits
    let wants_merges = args.iter().any(|arg| arg == "--merges" || arg == "--min-parents=2");
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
        tracking::track("git status", "rtk git status", &raw_output, "Clean working tree");
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
    let rtk_output = format!("branch + {} staged + {} modified + {} untracked", staged, modified, untracked);
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

fn run_push(verbose: u8) -> Result<()> {
    if verbose > 0 {
        eprintln!("git push");
    }

    let output = Command::new("git")
        .arg("push")
        .output()
        .context("Failed to run git push")?;

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
                        let msg = format!("ok ✓ {}", parts[parts.len()-1]);
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

fn run_pull(verbose: u8) -> Result<()> {
    if verbose > 0 {
        eprintln!("git pull");
    }

    let output = Command::new("git")
        .arg("pull")
        .output()
        .context("Failed to run git pull")?;

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
                            files = part.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0);
                        } else if part.contains("insertion") {
                            insertions = part.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0);
                        } else if part.contains("deletion") {
                            deletions = part.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0);
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
}
