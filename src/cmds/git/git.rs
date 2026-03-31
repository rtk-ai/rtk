//! Filters git output — log, status, diff, and more — keeping just the essential info.

use crate::core::config;
use crate::core::tracking;
use crate::core::utils::{exit_code_from_output, exit_code_from_status, resolved_command};
use anyhow::{Context, Result};
use std::ffi::OsString;
use std::process::Command;

#[derive(Debug, Clone)]
pub enum GitCommand {
    Diff,
    Log,
    Status,
    Show,
    Add,
    Commit,
    Push,
    Pull,
    Branch,
    Fetch,
    Stash { subcommand: Option<String> },
    Worktree,
}

/// Create a git Command with global options (e.g. -C, -c, --git-dir, --work-tree)
/// prepended before any subcommand arguments.
fn git_cmd(global_args: &[String]) -> Command {
    let mut cmd = resolved_command("git");
    for arg in global_args {
        cmd.arg(arg);
    }
    cmd
}

pub fn run(
    cmd: GitCommand,
    args: &[String],
    max_lines: Option<usize>,
    verbose: u8,
    global_args: &[String],
) -> Result<i32> {
    match cmd {
        GitCommand::Diff => run_diff(args, max_lines, verbose, global_args),
        GitCommand::Log => run_log(args, max_lines, verbose, global_args),
        GitCommand::Status => run_status(args, verbose, global_args),
        GitCommand::Show => run_show(args, max_lines, verbose, global_args),
        GitCommand::Add => run_add(args, verbose, global_args),
        GitCommand::Commit => run_commit(args, verbose, global_args),
        GitCommand::Push => run_push(args, verbose, global_args),
        GitCommand::Pull => run_pull(args, verbose, global_args),
        GitCommand::Branch => run_branch(args, verbose, global_args),
        GitCommand::Fetch => run_fetch(args, verbose, global_args),
        GitCommand::Stash { subcommand } => {
            run_stash(subcommand.as_deref(), args, verbose, global_args)
        }
        GitCommand::Worktree => run_worktree(args, verbose, global_args),
    }
}

fn run_diff(
    args: &[String],
    max_lines: Option<usize>,
    verbose: u8,
    global_args: &[String],
) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    // Check if user wants stat output
    let wants_stat = args
        .iter()
        .any(|arg| arg == "--stat" || arg == "--numstat" || arg == "--shortstat");

    // Check if user wants compact diff (default RTK behavior)
    let wants_compact = !args.iter().any(|arg| arg == "--no-compact");

    if wants_stat || !wants_compact {
        // User wants stat or explicitly no compacting - pass through directly
        let mut cmd = git_cmd(global_args);
        cmd.arg("diff");
        for arg in args {
            if arg == "--no-compact" {
                continue; // RTK flag, not a git flag
            }
            cmd.arg(arg);
        }

        let output = cmd.output().context("Failed to run git diff")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("{}", stderr);
            return Ok(exit_code_from_output(&output, "git"));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        println!("{}", stdout.trim());

        timer.track(
            &format!("git diff {}", args.join(" ")),
            &format!("rtk git diff {} (passthrough)", args.join(" ")),
            &stdout,
            &stdout,
        );

        return Ok(0);
    }

    // Default RTK behavior: stat first, then compacted diff
    let mut cmd = git_cmd(global_args);
    cmd.arg("diff").arg("--stat");

    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run git diff")?;
    let stat_stdout = String::from_utf8_lossy(&output.stdout);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.trim().is_empty() {
            eprint!("{}", stderr);
        }
        let raw = stat_stdout.to_string();
        timer.track(
            &format!("git diff {}", args.join(" ")),
            &format!("rtk git diff {}", args.join(" ")),
            &raw,
            &raw,
        );
        return Ok(exit_code_from_output(&output, "git"));
    }

    if verbose > 0 {
        eprintln!("Git diff summary:");
    }

    // Print stat summary first
    println!("{}", stat_stdout.trim());

    // Now get actual diff but compact it
    let mut diff_cmd = git_cmd(global_args);
    diff_cmd.arg("diff");
    for arg in args {
        diff_cmd.arg(arg);
    }

    let diff_output = diff_cmd.output().context("Failed to run git diff")?;
    let diff_stdout = String::from_utf8_lossy(&diff_output.stdout);

    let mut final_output = stat_stdout.to_string();
    if !diff_stdout.is_empty() {
        println!("\n--- Changes ---");
        let compacted = compact_diff(&diff_stdout, max_lines.unwrap_or(500));
        println!("{}", compacted);
        final_output.push_str("\n--- Changes ---\n");
        final_output.push_str(&compacted);
    }

    timer.track(
        &format!("git diff {}", args.join(" ")),
        &format!("rtk git diff {}", args.join(" ")),
        &format!("{}\n{}", stat_stdout, diff_stdout),
        &final_output,
    );

    Ok(0)
}

fn run_show(
    args: &[String],
    max_lines: Option<usize>,
    verbose: u8,
    global_args: &[String],
) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    // If user wants --stat or --format only, pass through
    let wants_stat_only = args
        .iter()
        .any(|arg| arg == "--stat" || arg == "--numstat" || arg == "--shortstat");

    let wants_format = args
        .iter()
        .any(|arg| arg.starts_with("--pretty") || arg.starts_with("--format"));

    // `git show rev:path` prints a blob, not a commit diff. In this mode we should
    // pass through directly to avoid duplicated output from compact-show steps.
    let wants_blob_show = args.iter().any(|arg| is_blob_show_arg(arg));

    if wants_stat_only || wants_format || wants_blob_show {
        let mut cmd = git_cmd(global_args);
        cmd.arg("show");
        for arg in args {
            cmd.arg(arg);
        }
        let output = cmd.output().context("Failed to run git show")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("{}", stderr);
            return Ok(exit_code_from_output(&output, "git"));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        if wants_blob_show {
            print!("{}", stdout);
        } else {
            println!("{}", stdout.trim());
        }

        timer.track(
            &format!("git show {}", args.join(" ")),
            &format!("rtk git show {} (passthrough)", args.join(" ")),
            &stdout,
            &stdout,
        );

        return Ok(0);
    }

    // Get raw output for tracking
    let mut raw_cmd = git_cmd(global_args);
    raw_cmd.arg("show");
    for arg in args {
        raw_cmd.arg(arg);
    }
    let raw_output = raw_cmd
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    // Step 1: one-line commit summary
    let mut summary_cmd = git_cmd(global_args);
    summary_cmd.args(["show", "--no-patch", "--pretty=format:%h %s (%ar) <%an>"]);
    for arg in args {
        summary_cmd.arg(arg);
    }
    let summary_output = summary_cmd.output().context("Failed to run git show")?;
    if !summary_output.status.success() {
        let stderr = String::from_utf8_lossy(&summary_output.stderr);
        eprintln!("{}", stderr);
        return Ok(exit_code_from_output(&summary_output, "git"));
    }
    let summary = String::from_utf8_lossy(&summary_output.stdout);
    println!("{}", summary.trim());

    // Step 2: --stat summary
    let mut stat_cmd = git_cmd(global_args);
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
    let mut diff_cmd = git_cmd(global_args);
    diff_cmd.args(["show", "--pretty=format:"]);
    for arg in args {
        diff_cmd.arg(arg);
    }
    let diff_output = diff_cmd.output().context("Failed to run git show (diff)")?;
    let diff_stdout = String::from_utf8_lossy(&diff_output.stdout);
    let diff_text = diff_stdout.trim();

    let mut final_output = summary.to_string();
    if !diff_text.is_empty() {
        if verbose > 0 {
            println!("\n--- Changes ---");
        }
        let compacted = compact_diff(diff_text, max_lines.unwrap_or(500));
        println!("{}", compacted);
        final_output.push_str(&format!("\n{}", compacted));
    }

    timer.track(
        &format!("git show {}", args.join(" ")),
        &format!("rtk git show {}", args.join(" ")),
        &raw_output,
        &final_output,
    );

    Ok(0)
}

fn is_blob_show_arg(arg: &str) -> bool {
    // Detect `rev:path` style arguments while ignoring flags like `--pretty=format:...`.
    !arg.starts_with('-') && arg.contains(':')
}

pub(crate) fn compact_diff(diff: &str, max_lines: usize) -> String {
    let mut result = Vec::new();
    let mut current_file = String::new();
    let mut added = 0;
    let mut removed = 0;
    let mut in_hunk = false;
    let mut hunk_shown = 0;
    let mut hunk_skipped = 0usize;
    let max_hunk_lines = 100;
    let mut was_truncated = false;

    for line in diff.lines() {
        if line.starts_with("diff --git") {
            // Flush hunk truncation before starting a new file
            if hunk_skipped > 0 {
                result.push(format!("  ... ({} lines truncated)", hunk_skipped));
                was_truncated = true;
                hunk_skipped = 0;
            }
            if !current_file.is_empty() && (added > 0 || removed > 0) {
                result.push(format!("  +{} -{}", added, removed));
            }
            current_file = line.split(" b/").nth(1).unwrap_or("unknown").to_string();
            result.push(format!("\n{}", current_file));
            added = 0;
            removed = 0;
            in_hunk = false;
            hunk_shown = 0;
        } else if line.starts_with("@@") {
            // Flush hunk truncation before starting a new hunk
            if hunk_skipped > 0 {
                result.push(format!("  ... ({} lines truncated)", hunk_skipped));
                was_truncated = true;
                hunk_skipped = 0;
            }
            in_hunk = true;
            hunk_shown = 0;
            let hunk_info = line.split("@@").nth(1).unwrap_or("").trim();
            result.push(format!("  @@ {} @@", hunk_info));
        } else if in_hunk {
            if line.starts_with('+') && !line.starts_with("+++") {
                added += 1;
                if hunk_shown < max_hunk_lines {
                    result.push(format!("  {}", line));
                    hunk_shown += 1;
                } else {
                    hunk_skipped += 1;
                }
            } else if line.starts_with('-') && !line.starts_with("---") {
                removed += 1;
                if hunk_shown < max_hunk_lines {
                    result.push(format!("  {}", line));
                    hunk_shown += 1;
                } else {
                    hunk_skipped += 1;
                }
            } else if hunk_shown < max_hunk_lines && !line.starts_with("\\") {
                // Context line
                if hunk_shown > 0 {
                    result.push(format!("  {}", line));
                    hunk_shown += 1;
                }
            }
        }

        if result.len() >= max_lines {
            result.push("\n... (more changes truncated)".to_string());
            was_truncated = true;
            break;
        }
    }

    // Flush last hunk
    if hunk_skipped > 0 {
        result.push(format!("  ... ({} lines truncated)", hunk_skipped));
        was_truncated = true;
    }

    if !current_file.is_empty() && (added > 0 || removed > 0) {
        result.push(format!("  +{} -{}", added, removed));
    }

    if was_truncated {
        result.push("[full diff: rtk git diff --no-compact]".to_string());
    }

    result.join("\n")
}

fn run_log(
    args: &[String],
    _max_lines: Option<usize>,
    verbose: u8,
    global_args: &[String],
) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = git_cmd(global_args);
    cmd.arg("log");

    // Check if user provided format flags
    let has_format_flag = args.iter().any(|arg| {
        arg.starts_with("--oneline") || arg.starts_with("--pretty") || arg.starts_with("--format")
    });

    // Check if user provided limit flag (-N, -n N, --max-count=N, --max-count N)
    let has_limit_flag = args.iter().any(|arg| {
        (arg.starts_with('-') && arg.chars().nth(1).is_some_and(|c| c.is_ascii_digit()))
            || arg == "-n"
            || arg.starts_with("--max-count")
    });

    // Apply RTK defaults only if user didn't specify them
    // Use %b (body) to preserve first line of commit body for agent context
    // (BREAKING CHANGE, Closes #xxx, design notes)
    if !has_format_flag {
        cmd.args(["--pretty=format:%h %s (%ar) <%an>%n%b%n---END---"]);
    }

    // Determine limit: respect user's explicit -N flag, use sensible defaults otherwise
    let (limit, user_set_limit) = if has_limit_flag {
        // User explicitly passed -N / -n N / --max-count=N → respect their choice
        let n = parse_user_limit(args).unwrap_or(10);
        (n, true)
    } else if has_format_flag {
        // --oneline / --pretty without -N: user wants compact output, allow more
        cmd.arg("-50");
        (50, false)
    } else {
        // No flags at all: default to 10
        cmd.arg("-10");
        (10, false)
    };

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
        return Ok(exit_code_from_output(&output, "git"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    if verbose > 0 {
        eprintln!("Git log output:");
    }

    // Post-process: truncate long messages, cap lines only if RTK set the default
    let filtered = filter_log_output(&stdout, limit, user_set_limit, has_format_flag);
    println!("{}", filtered);

    timer.track(
        &format!("git log {}", args.join(" ")),
        &format!("rtk git log {}", args.join(" ")),
        &stdout,
        &filtered,
    );

    Ok(0)
}

/// Filter git log output: truncate long messages, cap lines
/// Parse the user-specified limit from git log args.
/// Handles: -20, -n 20, --max-count=20, --max-count 20
fn parse_user_limit(args: &[String]) -> Option<usize> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        // -20 (combined digit form)
        if arg.starts_with('-')
            && arg.len() > 1
            && arg.chars().nth(1).is_some_and(|c| c.is_ascii_digit())
        {
            if let Ok(n) = arg[1..].parse::<usize>() {
                return Some(n);
            }
        }
        // -n 20 (two-token form)
        if arg == "-n" {
            if let Some(next) = iter.next() {
                if let Ok(n) = next.parse::<usize>() {
                    return Some(n);
                }
            }
        }
        // --max-count=20
        if let Some(rest) = arg.strip_prefix("--max-count=") {
            if let Ok(n) = rest.parse::<usize>() {
                return Some(n);
            }
        }
        // --max-count 20 (two-token form)
        if arg == "--max-count" {
            if let Some(next) = iter.next() {
                if let Ok(n) = next.parse::<usize>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// When `user_set_limit` is true, the user explicitly passed `-N` to git log,
/// so we skip line capping (git already returns exactly N commits) and use a
/// wider truncation threshold (120 chars) to preserve commit context that LLMs
/// need for rebase/squash operations.
pub(crate) fn filter_log_output(
    output: &str,
    limit: usize,
    user_set_limit: bool,
    user_format: bool,
) -> String {
    let truncate_width = if user_set_limit { 120 } else { 80 };

    // When user specified their own format (--oneline, --pretty, --format),
    // RTK did not inject ---END--- markers. Use simple line-based truncation.
    if user_format {
        let lines: Vec<&str> = output.lines().collect();
        let max_lines = if user_set_limit { lines.len() } else { limit };
        return lines
            .iter()
            .take(max_lines)
            .map(|l| truncate_line(l, truncate_width))
            .collect::<Vec<_>>()
            .join("\n");
    }

    // RTK injected format: split output into commit blocks separated by ---END---
    let commits: Vec<&str> = output.split("---END---").collect();
    let max_commits = if user_set_limit { commits.len() } else { limit };

    let mut result = Vec::new();
    for block in commits.iter().take(max_commits) {
        let block = block.trim();
        if block.is_empty() {
            continue;
        }
        let mut lines = block.lines();
        // First line is the header: hash subject (date) <author>
        let header = match lines.next() {
            Some(h) => truncate_line(h.trim(), truncate_width),
            None => continue,
        };
        // Remaining lines are the body — keep up to 3 non-empty, non-trailer lines
        let all_body_lines: Vec<&str> = lines
            .map(|l| l.trim())
            .filter(|l| {
                !l.is_empty()
                    && !l.starts_with("Signed-off-by:")
                    && !l.starts_with("Co-authored-by:")
            })
            .collect();
        let body_omitted = all_body_lines.len().saturating_sub(3);
        let body_lines = &all_body_lines[..all_body_lines.len().min(3)];

        if body_lines.is_empty() {
            result.push(header);
        } else {
            let mut entry = header;
            for body in body_lines {
                entry.push_str(&format!("\n  {}", truncate_line(body, truncate_width)));
            }
            if body_omitted > 0 {
                entry.push_str(&format!("\n  [+{} lines omitted]", body_omitted));
            }
            result.push(entry);
        }
    }

    result.join("\n").trim().to_string()
}

/// Truncate a single line to `width` characters, appending "..." if needed
fn truncate_line(line: &str, width: usize) -> String {
    if line.chars().count() > width {
        let truncated: String = line.chars().take(width - 3).collect();
        format!("{}...", truncated)
    } else {
        line.to_string()
    }
}

pub(crate) fn format_status_output(porcelain: &str) -> String {
    let lines: Vec<&str> = porcelain.lines().collect();

    if lines.is_empty() {
        return "Clean working tree".to_string();
    }

    let mut output = String::new();

    // Parse branch info
    if let Some(branch_line) = lines.first() {
        if branch_line.starts_with("##") {
            let branch = branch_line.trim_start_matches("## ");
            output.push_str(&format!("* {}\n", branch));
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
        let status = line.get(0..2).unwrap_or("  ");
        let file = line.get(3..).unwrap_or("");

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

    // Build summary
    let limits = config::limits();
    let max_files = limits.status_max_files;
    let max_untracked = limits.status_max_untracked;

    if staged > 0 {
        output.push_str(&format!("+ Staged: {} files\n", staged));
        for f in staged_files.iter().take(max_files) {
            output.push_str(&format!("   {}\n", f));
        }
        if staged_files.len() > max_files {
            output.push_str(&format!(
                "   ... +{} more\n",
                staged_files.len() - max_files
            ));
        }
    }

    if modified > 0 {
        output.push_str(&format!("~ Modified: {} files\n", modified));
        for f in modified_files.iter().take(max_files) {
            output.push_str(&format!("   {}\n", f));
        }
        if modified_files.len() > max_files {
            output.push_str(&format!(
                "   ... +{} more\n",
                modified_files.len() - max_files
            ));
        }
    }

    if untracked > 0 {
        output.push_str(&format!("? Untracked: {} files\n", untracked));
        for f in untracked_files.iter().take(max_untracked) {
            output.push_str(&format!("   {}\n", f));
        }
        if untracked_files.len() > max_untracked {
            output.push_str(&format!(
                "   ... +{} more\n",
                untracked_files.len() - max_untracked
            ));
        }
    }

    if conflicts > 0 {
        output.push_str(&format!("conflicts: {} files\n", conflicts));
    }

    // When working tree is clean (only branch line, no changes)
    if staged == 0 && modified == 0 && untracked == 0 && conflicts == 0 {
        output.push_str("clean — nothing to commit\n");
    }

    output.trim_end().to_string()
}

/// Minimal filtering for git status with user-provided args
fn filter_status_with_args(output: &str) -> String {
    let mut result = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip empty lines
        if trimmed.is_empty() {
            continue;
        }

        // Skip git hints - can appear at start or within line
        if trimmed.starts_with("(use \"git")
            || trimmed.starts_with("(create/copy files")
            || trimmed.contains("(use \"git add")
            || trimmed.contains("(use \"git restore")
        {
            continue;
        }

        // Special case: clean working tree
        if trimmed.contains("nothing to commit") && trimmed.contains("working tree clean") {
            result.push(trimmed.to_string());
            break;
        }

        result.push(line.to_string());
    }

    if result.is_empty() {
        "ok".to_string()
    } else {
        result.join("\n")
    }
}

fn run_status(args: &[String], verbose: u8, global_args: &[String]) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    // If user provided flags, apply minimal filtering
    if !args.is_empty() {
        let output = git_cmd(global_args)
            .arg("status")
            .args(args)
            .output()
            .context("Failed to run git status")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            if !stderr.trim().is_empty() {
                eprint!("{}", stderr);
            }
            let raw = stdout.to_string();
            timer.track(
                &format!("git status {}", args.join(" ")),
                &format!("rtk git status {}", args.join(" ")),
                &raw,
                &raw,
            );
            return Ok(exit_code_from_output(&output, "git"));
        }

        if verbose > 0 || !stderr.is_empty() {
            eprint!("{}", stderr);
        }

        // Apply minimal filtering: strip ANSI, remove hints, empty lines
        let filtered = filter_status_with_args(&stdout);
        print!("{}", filtered);

        timer.track(
            &format!("git status {}", args.join(" ")),
            &format!("rtk git status {}", args.join(" ")),
            &stdout,
            &filtered,
        );

        return Ok(0);
    }

    // Default RTK compact mode (no args provided)
    // Get raw git status for tracking
    let raw_output = git_cmd(global_args)
        .args(["status"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    let output = git_cmd(global_args)
        .args(["status", "--porcelain", "-b"])
        .output()
        .context("Failed to run git status")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !stderr.is_empty() && stderr.contains("not a git repository") {
        let message = "Not a git repository".to_string();
        eprintln!("{}", message);
        timer.track("git status", "rtk git status", &raw_output, &message);
        return Ok(exit_code_from_output(&output, "git"));
    }

    let formatted = format_status_output(&stdout);

    println!("{}", formatted);

    // Track for statistics
    timer.track("git status", "rtk git status", &raw_output, &formatted);

    Ok(0)
}

fn run_add(args: &[String], verbose: u8, global_args: &[String]) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = git_cmd(global_args);
    cmd.arg("add");

    // Pass all arguments directly to git (flags like -A, -p, --all, etc.)
    if args.is_empty() {
        cmd.arg(".");
    } else {
        for arg in args {
            cmd.arg(arg);
        }
    }

    let output = cmd.output().context("Failed to run git add")?;

    if verbose > 0 {
        eprintln!("git add executed");
    }

    let raw_output = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    if output.status.success() {
        // Count what was added
        let status_output = git_cmd(global_args)
            .args(["diff", "--cached", "--stat", "--shortstat"])
            .output()
            .context("Failed to check staged files")?;

        let stat = String::from_utf8_lossy(&status_output.stdout);
        let compact = if stat.trim().is_empty() {
            "ok (nothing to add)".to_string()
        } else {
            // Parse "1 file changed, 5 insertions(+)" format
            let short = stat.lines().last().unwrap_or("").trim();
            if short.is_empty() {
                "ok".to_string()
            } else {
                format!("ok {}", short)
            }
        };

        println!("{}", compact);

        timer.track(
            &format!("git add {}", args.join(" ")),
            &format!("rtk git add {}", args.join(" ")),
            &raw_output,
            &compact,
        );
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
        return Ok(exit_code_from_output(&output, "git"));
    }

    Ok(0)
}

fn build_commit_command(args: &[String], global_args: &[String]) -> Command {
    let mut cmd = git_cmd(global_args);
    cmd.arg("commit");
    for arg in args {
        cmd.arg(arg);
    }
    cmd
}

fn run_commit(args: &[String], verbose: u8, global_args: &[String]) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let original_cmd = format!("git commit {}", args.join(" "));

    if verbose > 0 {
        eprintln!("{}", original_cmd);
    }

    let output = build_commit_command(args, global_args)
        .output()
        .context("Failed to run git commit")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw_output = format!("{}\n{}", stdout, stderr);

    if output.status.success() {
        // Extract commit hash from output like "[main abc1234] message"
        let compact = if let Some(line) = stdout.lines().next() {
            if let Some(hash_start) = line.find(' ') {
                let hash = line[1..hash_start].split(' ').next_back().unwrap_or("");
                if !hash.is_empty() && hash.len() >= 7 {
                    format!("ok {}", &hash[..7.min(hash.len())])
                } else {
                    "ok".to_string()
                }
            } else {
                "ok".to_string()
            }
        } else {
            "ok".to_string()
        };

        println!("{}", compact);

        timer.track(&original_cmd, "rtk git commit", &raw_output, &compact);
    } else {
        if stderr.contains("nothing to commit") || stdout.contains("nothing to commit") {
            println!("ok (nothing to commit)");
            timer.track(
                &original_cmd,
                "rtk git commit",
                &raw_output,
                "ok (nothing to commit)",
            );
        } else {
            if !stderr.trim().is_empty() {
                eprint!("{}", stderr);
            }
            if !stdout.trim().is_empty() {
                eprint!("{}", stdout);
            }
            timer.track(&original_cmd, "rtk git commit", &raw_output, &raw_output);
            return Ok(exit_code_from_output(&output, "git"));
        }
    }

    Ok(0)
}

fn run_push(args: &[String], verbose: u8, global_args: &[String]) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("git push");
    }

    let mut cmd = git_cmd(global_args);
    cmd.arg("push");
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run git push")?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let raw = format!("{}{}", stdout, stderr);

    if output.status.success() {
        let compact = if stderr.contains("Everything up-to-date") {
            "ok (up-to-date)".to_string()
        } else {
            let mut result = String::new();
            for line in stderr.lines() {
                if line.contains("->") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 3 {
                        result = format!("ok {}", parts[parts.len() - 1]);
                        break;
                    }
                }
            }
            if !result.is_empty() {
                result
            } else {
                "ok".to_string()
            }
        };

        println!("{}", compact);

        timer.track(
            &format!("git push {}", args.join(" ")),
            &format!("rtk git push {}", args.join(" ")),
            &raw,
            &compact,
        );
    } else {
        eprintln!("FAILED: git push");
        if !stderr.trim().is_empty() {
            eprintln!("{}", stderr);
        }
        if !stdout.trim().is_empty() {
            eprintln!("{}", stdout);
        }
        return Ok(exit_code_from_output(&output, "git"));
    }

    Ok(0)
}

fn run_pull(args: &[String], verbose: u8, global_args: &[String]) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("git pull");
    }

    let mut cmd = git_cmd(global_args);
    cmd.arg("pull");
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run git pull")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw_output = format!("{}\n{}", stdout, stderr);

    if output.status.success() {
        let compact =
            if stdout.contains("Already up to date") || stdout.contains("Already up-to-date") {
                "ok (up-to-date)".to_string()
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
                    format!("ok {} files +{} -{}", files, insertions, deletions)
                } else {
                    "ok".to_string()
                }
            };

        println!("{}", compact);

        timer.track(
            &format!("git pull {}", args.join(" ")),
            &format!("rtk git pull {}", args.join(" ")),
            &raw_output,
            &compact,
        );
    } else {
        eprintln!("FAILED: git pull");
        if !stderr.trim().is_empty() {
            eprintln!("{}", stderr);
        }
        if !stdout.trim().is_empty() {
            eprintln!("{}", stdout);
        }
        return Ok(exit_code_from_output(&output, "git"));
    }

    Ok(0)
}

fn run_branch(args: &[String], verbose: u8, global_args: &[String]) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("git branch");
    }

    // Detect write operations: delete, rename, copy, upstream tracking
    let has_action_flag = args.iter().any(|a| {
        a == "-d"
            || a == "-D"
            || a == "-m"
            || a == "-M"
            || a == "-c"
            || a == "-C"
            || a == "--set-upstream-to"
            || a.starts_with("--set-upstream-to=")
            || a == "-u"
            || a == "--unset-upstream"
            || a == "--edit-description"
    });

    // Detect flags that produce specific output (not a branch list)
    let has_show_flag = args.iter().any(|a| a == "--show-current");

    // Detect list-mode flags
    let has_list_flag = args.iter().any(|a| {
        a == "-a"
            || a == "--all"
            || a == "-r"
            || a == "--remotes"
            || a == "--list"
            || a == "--merged"
            || a == "--no-merged"
            || a == "--contains"
            || a == "--no-contains"
            || a == "--format"
            || a.starts_with("--format=")
            || a == "--sort"
            || a.starts_with("--sort=")
            || a == "--points-at"
            || a.starts_with("--points-at=")
    });

    // Detect positional arguments (not flags) — indicates branch creation
    let has_positional_arg = args.iter().any(|a| !a.starts_with('-'));

    // --show-current: passthrough with raw stdout (not "ok ✓")
    if has_show_flag {
        let mut cmd = git_cmd(global_args);
        cmd.arg("branch");
        for arg in args {
            cmd.arg(arg);
        }
        let output = cmd.output().context("Failed to run git branch")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{}{}", stdout, stderr);

        let trimmed = stdout.trim();
        timer.track(
            &format!("git branch {}", args.join(" ")),
            &format!("rtk git branch {}", args.join(" ")),
            &combined,
            trimmed,
        );

        if output.status.success() {
            println!("{}", trimmed);
        } else {
            eprintln!("FAILED: git branch {}", args.join(" "));
            if !stderr.trim().is_empty() {
                eprintln!("{}", stderr);
            }
            return Ok(exit_code_from_output(&output, "git"));
        }
        return Ok(0);
    }

    // Write operation: action flags, or positional args without list flags (= branch creation)
    if has_action_flag || (has_positional_arg && !has_list_flag) {
        let mut cmd = git_cmd(global_args);
        cmd.arg("branch");
        for arg in args {
            cmd.arg(arg);
        }
        let output = cmd.output().context("Failed to run git branch")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{}{}", stdout, stderr);

        let msg = if output.status.success() {
            "ok"
        } else {
            &combined
        };

        timer.track(
            &format!("git branch {}", args.join(" ")),
            &format!("rtk git branch {}", args.join(" ")),
            &combined,
            msg,
        );

        if output.status.success() {
            println!("ok");
        } else {
            eprintln!("FAILED: git branch {}", args.join(" "));
            if !stderr.trim().is_empty() {
                eprintln!("{}", stderr);
            }
            if !stdout.trim().is_empty() {
                eprintln!("{}", stdout);
            }
            return Ok(exit_code_from_output(&output, "git"));
        }
        return Ok(0);
    }

    // List mode: show compact branch list
    let mut cmd = git_cmd(global_args);
    cmd.arg("branch");
    if !has_list_flag {
        cmd.arg("-a");
    }
    cmd.arg("--no-color");
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run git branch")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let raw = stdout.to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.trim().is_empty() {
            eprint!("{}", stderr);
        }
        timer.track(
            &format!("git branch {}", args.join(" ")),
            &format!("rtk git branch {}", args.join(" ")),
            &raw,
            &raw,
        );
        return Ok(exit_code_from_output(&output, "git"));
    }

    let filtered = filter_branch_output(&stdout);
    println!("{}", filtered);

    timer.track(
        &format!("git branch {}", args.join(" ")),
        &format!("rtk git branch {}", args.join(" ")),
        &raw,
        &filtered,
    );

    Ok(0)
}

fn filter_branch_output(output: &str) -> String {
    let mut current = String::new();
    let mut local: Vec<String> = Vec::new();
    let mut remote: Vec<String> = Vec::new();
    let mut seen_remote: std::collections::HashSet<String> = std::collections::HashSet::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(branch) = line.strip_prefix("* ") {
            current = branch.to_string();
        } else if let Some(rest) = line.strip_prefix("remotes/") {
            if let Some(slash_pos) = rest.find('/') {
                let branch = &rest[slash_pos + 1..];
                if branch.starts_with("HEAD ") {
                    continue;
                }
                if seen_remote.insert(branch.to_string()) {
                    remote.push(branch.to_string());
                }
            }
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

fn run_fetch(args: &[String], verbose: u8, global_args: &[String]) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("git fetch");
    }

    let mut cmd = git_cmd(global_args);
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
        return Ok(exit_code_from_output(&output, "git"));
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
    timer.track("git fetch", "rtk git fetch", &raw, &msg);

    Ok(0)
}

fn run_stash(
    subcommand: Option<&str>,
    args: &[String],
    verbose: u8,
    global_args: &[String],
) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("git stash {:?}", subcommand);
    }

    match subcommand {
        Some("list") => {
            let output = git_cmd(global_args)
                .args(["stash", "list"])
                .output()
                .context("Failed to run git stash list")?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let raw = stdout.to_string();

            if stdout.trim().is_empty() {
                let msg = "No stashes";
                println!("{}", msg);
                timer.track("git stash list", "rtk git stash list", &raw, msg);
                return Ok(0);
            }

            let filtered = filter_stash_list(&stdout);
            println!("{}", filtered);
            timer.track("git stash list", "rtk git stash list", &raw, &filtered);
        }
        Some("show") => {
            let mut cmd = git_cmd(global_args);
            cmd.args(["stash", "show", "-p"]);
            for arg in args {
                cmd.arg(arg);
            }
            let output = cmd.output().context("Failed to run git stash show")?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let raw = stdout.to_string();

            let filtered = if stdout.trim().is_empty() {
                let msg = "Empty stash";
                println!("{}", msg);
                msg.to_string()
            } else {
                let compacted = compact_diff(&stdout, 100);
                println!("{}", compacted);
                compacted
            };

            timer.track("git stash show", "rtk git stash show", &raw, &filtered);
        }
        Some("pop") | Some("apply") | Some("drop") | Some("push") => {
            let sub = subcommand.unwrap();
            let mut cmd = git_cmd(global_args);
            cmd.args(["stash", sub]);
            for arg in args {
                cmd.arg(arg);
            }
            let output = cmd.output().context("Failed to run git stash")?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{}{}", stdout, stderr);

            let msg = if output.status.success() {
                let msg = format!("ok stash {}", sub);
                println!("{}", msg);
                msg
            } else {
                eprintln!("FAILED: git stash {}", sub);
                if !stderr.trim().is_empty() {
                    eprintln!("{}", stderr);
                }
                combined.clone()
            };

            timer.track(
                &format!("git stash {}", sub),
                &format!("rtk git stash {}", sub),
                &combined,
                &msg,
            );

            if !output.status.success() {
                return Ok(exit_code_from_output(&output, "git"));
            }
        }
        Some(sub) => {
            // Unrecognized subcommand: passthrough to git stash <sub> [args]
            let mut cmd = git_cmd(global_args);
            cmd.args(["stash", sub]);
            for arg in args {
                cmd.arg(arg);
            }
            let output = cmd.output().context("Failed to run git stash")?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{}{}", stdout, stderr);

            let msg = if output.status.success() {
                let msg = format!("ok stash {}", sub);
                println!("{}", msg);
                msg
            } else {
                eprintln!("FAILED: git stash {}", sub);
                if !stderr.trim().is_empty() {
                    eprintln!("{}", stderr);
                }
                combined.clone()
            };

            timer.track(
                &format!("git stash {}", sub),
                &format!("rtk git stash {}", sub),
                &combined,
                &msg,
            );

            if !output.status.success() {
                return Ok(exit_code_from_output(&output, "git"));
            }
        }
        None => {
            // Default: git stash (push)
            let mut cmd = git_cmd(global_args);
            cmd.arg("stash");
            for arg in args {
                cmd.arg(arg);
            }
            let output = cmd.output().context("Failed to run git stash")?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{}{}", stdout, stderr);

            let msg = if output.status.success() {
                if stdout.contains("No local changes") {
                    let msg = "ok (nothing to stash)";
                    println!("{}", msg);
                    msg.to_string()
                } else {
                    let msg = "ok stashed";
                    println!("{}", msg);
                    msg.to_string()
                }
            } else {
                eprintln!("FAILED: git stash");
                if !stderr.trim().is_empty() {
                    eprintln!("{}", stderr);
                }
                combined.clone()
            };

            timer.track("git stash", "rtk git stash", &combined, &msg);

            if !output.status.success() {
                return Ok(exit_code_from_output(&output, "git"));
            }
        }
    }

    Ok(0)
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

fn run_worktree(args: &[String], verbose: u8, global_args: &[String]) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("git worktree list");
    }

    // If args contain "add", "remove", "prune" etc., pass through
    let has_action = args.iter().any(|a| {
        a == "add" || a == "remove" || a == "prune" || a == "lock" || a == "unlock" || a == "move"
    });

    if has_action {
        let mut cmd = git_cmd(global_args);
        cmd.arg("worktree");
        for arg in args {
            cmd.arg(arg);
        }
        let output = cmd.output().context("Failed to run git worktree")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{}{}", stdout, stderr);

        let msg = if output.status.success() {
            "ok"
        } else {
            &combined
        };

        timer.track(
            &format!("git worktree {}", args.join(" ")),
            &format!("rtk git worktree {}", args.join(" ")),
            &combined,
            msg,
        );

        if output.status.success() {
            println!("ok");
        } else {
            eprintln!("FAILED: git worktree {}", args.join(" "));
            if !stderr.trim().is_empty() {
                eprintln!("{}", stderr);
            }
            return Ok(exit_code_from_output(&output, "git"));
        }
        return Ok(0);
    }

    // Default: list mode
    let output = git_cmd(global_args)
        .args(["worktree", "list"])
        .output()
        .context("Failed to run git worktree list")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let raw = stdout.to_string();

    let filtered = filter_worktree_list(&stdout);
    println!("{}", filtered);
    timer.track("git worktree list", "rtk git worktree", &raw, &filtered);

    Ok(0)
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

/// Runs an unsupported git subcommand by passing it through directly
pub fn run_passthrough(args: &[OsString], global_args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("git passthrough: {:?}", args);
    }
    let status = git_cmd(global_args)
        .args(args)
        .status()
        .context("Failed to run git")?;

    let args_str = tracking::args_display(args);
    timer.track_passthrough(
        &format!("git {}", args_str),
        &format!("rtk git {} (passthrough)", args_str),
    );

    if !status.success() {
        return Ok(exit_code_from_status(&status, "git"));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_cmd_no_global_args() {
        let cmd = git_cmd(&[]);
        let program = cmd.get_program().to_string_lossy().to_string();
        // On Windows, resolved_command returns full path (e.g. "C:\Program Files\Git\bin\git.exe")
        let basename = std::path::Path::new(&program)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(basename, "git");
        let args: Vec<_> = cmd.get_args().collect();
        assert!(args.is_empty());
    }

    #[test]
    fn test_git_cmd_with_directory() {
        let global_args = vec!["-C".to_string(), "/tmp".to_string()];
        let cmd = git_cmd(&global_args);
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, vec!["-C", "/tmp"]);
    }

    #[test]
    fn test_git_cmd_with_multiple_global_args() {
        let global_args = vec![
            "-C".to_string(),
            "/tmp".to_string(),
            "-c".to_string(),
            "user.name=test".to_string(),
            "--git-dir".to_string(),
            "/foo/.git".to_string(),
        ];
        let cmd = git_cmd(&global_args);
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(
            args,
            vec![
                "-C",
                "/tmp",
                "-c",
                "user.name=test",
                "--git-dir",
                "/foo/.git"
            ]
        );
    }

    #[test]
    fn test_git_cmd_with_boolean_flags() {
        let global_args = vec!["--no-pager".to_string(), "--bare".to_string()];
        let cmd = git_cmd(&global_args);
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, vec!["--no-pager", "--bare"]);
    }

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
    fn test_compact_diff_increased_hunk_limit() {
        // Build a hunk with 25 changed lines — should NOT be truncated with limit 30
        let mut diff =
            "diff --git a/big.rs b/big.rs\n--- a/big.rs\n+++ b/big.rs\n@@ -1,25 +1,25 @@\n"
                .to_string();
        for i in 1..=25 {
            diff.push_str(&format!("+line{}\n", i));
        }
        let result = compact_diff(&diff, 500);
        assert!(
            !result.contains("... (truncated)"),
            "25 lines should not be truncated with max_hunk_lines=30"
        );
        assert!(result.contains("+line25"));
    }

    #[test]
    fn test_compact_diff_increased_total_limit() {
        // Build a diff with 150 output result lines across multiple files — should NOT be cut at 100
        let mut diff = String::new();
        for f in 1..=5 {
            diff.push_str(&format!("diff --git a/file{f}.rs b/file{f}.rs\n--- a/file{f}.rs\n+++ b/file{f}.rs\n@@ -1,20 +1,20 @@\n"));
            for i in 1..=20 {
                diff.push_str(&format!("+line{f}_{i}\n"));
            }
        }
        let result = compact_diff(&diff, 500);
        assert!(
            !result.contains("more changes truncated"),
            "5 files × 20 lines should not exceed max_lines=500"
        );
    }

    #[test]
    fn test_is_blob_show_arg() {
        assert!(is_blob_show_arg("develop:modules/pairs_backtest.py"));
        assert!(is_blob_show_arg("HEAD:src/main.rs"));
        assert!(!is_blob_show_arg("--pretty=format:%h"));
        assert!(!is_blob_show_arg("--format=short"));
        assert!(!is_blob_show_arg("HEAD"));
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
    fn test_filter_branch_multi_remote() {
        let output = "* main\n  develop\n  remotes/origin/HEAD -> origin/main\n  remotes/origin/main\n  remotes/origin/feature-x\n  remotes/upstream/main\n  remotes/upstream/release-v3\n  remotes/fork/main\n  remotes/fork/experiment\n";
        let result = filter_branch_output(output);
        assert!(result.contains("* main"));
        assert!(result.contains("develop"));
        assert!(result.contains("feature-x"), "origin branch shown: {}", result);
        assert!(
            result.contains("release-v3"),
            "upstream branch shown: {}",
            result
        );
        assert!(
            result.contains("experiment"),
            "fork branch shown: {}",
            result
        );
        assert!(
            !result.contains("remotes/"),
            "remote prefix stripped: {}",
            result
        );
        let main_count = result.matches("main").count();
        assert!(
            main_count <= 2,
            "main deduplicated across remotes (found {} occurrences): {}",
            main_count,
            result
        );
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

    #[test]
    fn test_format_status_output_clean() {
        let porcelain = "";
        let result = format_status_output(porcelain);
        assert_eq!(result, "Clean working tree");
    }

    #[test]
    fn test_format_status_output_modified_files() {
        let porcelain = "## main...origin/main\n M src/main.rs\n M src/lib.rs\n";
        let result = format_status_output(porcelain);
        assert!(result.contains("* main...origin/main"));
        assert!(result.contains("~ Modified: 2 files"));
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("src/lib.rs"));
        assert!(!result.contains("Staged"));
        assert!(!result.contains("Untracked"));
    }

    #[test]
    fn test_format_status_output_untracked_files() {
        let porcelain = "## feature/new\n?? temp.txt\n?? debug.log\n?? test.sh\n";
        let result = format_status_output(porcelain);
        assert!(result.contains("* feature/new"));
        assert!(result.contains("? Untracked: 3 files"));
        assert!(result.contains("temp.txt"));
        assert!(result.contains("debug.log"));
        assert!(result.contains("test.sh"));
        assert!(!result.contains("Modified"));
    }

    #[test]
    fn test_format_status_output_mixed_changes() {
        let porcelain = r#"## main
M  staged.rs
 M modified.rs
A  added.rs
?? untracked.txt
"#;
        let result = format_status_output(porcelain);
        assert!(result.contains("* main"));
        assert!(result.contains("+ Staged: 2 files"));
        assert!(result.contains("staged.rs"));
        assert!(result.contains("added.rs"));
        assert!(result.contains("~ Modified: 1 files"));
        assert!(result.contains("modified.rs"));
        assert!(result.contains("? Untracked: 1 files"));
        assert!(result.contains("untracked.txt"));
    }

    #[test]
    fn test_format_status_output_truncation() {
        // Test that >15 staged files show "... +N more"
        let mut porcelain = String::from("## main\n");
        for i in 1..=20 {
            porcelain.push_str(&format!("M  file{}.rs\n", i));
        }
        let result = format_status_output(&porcelain);
        assert!(result.contains("+ Staged: 20 files"));
        assert!(result.contains("file1.rs"));
        assert!(result.contains("file15.rs"));
        assert!(result.contains("... +5 more"));
        assert!(!result.contains("file16.rs"));
        assert!(!result.contains("file20.rs"));
    }

    #[test]
    fn test_format_status_modified_truncation() {
        // Test that >15 modified files show "... +N more"
        let mut porcelain = String::from("## main\n");
        for i in 1..=20 {
            porcelain.push_str(&format!(" M file{}.rs\n", i));
        }
        let result = format_status_output(&porcelain);
        assert!(result.contains("~ Modified: 20 files"));
        assert!(result.contains("file1.rs"));
        assert!(result.contains("file15.rs"));
        assert!(result.contains("... +5 more"));
        assert!(!result.contains("file16.rs"));
    }

    #[test]
    fn test_format_status_untracked_truncation() {
        // Test that >10 untracked files show "... +N more"
        let mut porcelain = String::from("## main\n");
        for i in 1..=15 {
            porcelain.push_str(&format!("?? file{}.rs\n", i));
        }
        let result = format_status_output(&porcelain);
        assert!(result.contains("? Untracked: 15 files"));
        assert!(result.contains("file1.rs"));
        assert!(result.contains("file10.rs"));
        assert!(result.contains("... +5 more"));
        assert!(!result.contains("file11.rs"));
    }

    #[test]
    fn test_run_passthrough_accepts_args() {
        // Test that run_passthrough compiles and has correct signature
        let _args: Vec<OsString> = vec![OsString::from("tag"), OsString::from("--list")];
        // Compile-time verification that the function exists with correct signature
    }

    #[test]
    fn test_filter_log_output() {
        let output = "abc1234 This is a commit message (2 days ago) <author>\n\n---END---\ndef5678 Another commit (1 week ago) <other>\n\n---END---\n";
        let result = filter_log_output(output, 10, false, false);
        assert!(result.contains("abc1234"));
        assert!(result.contains("def5678"));
        assert_eq!(result.lines().count(), 2);
    }

    #[test]
    fn test_filter_log_output_with_body() {
        // Commit with body: first non-trailer body line should appear indented
        let output = "abc1234 feat: add feature (2 days ago) <author>\nBREAKING CHANGE: removed old API\nSigned-off-by: Author <a@b.com>\n---END---\ndef5678 fix: typo (1 day ago) <other>\n\n---END---\n";
        let result = filter_log_output(output, 10, false, false);
        assert!(result.contains("abc1234"));
        assert!(result.contains("BREAKING CHANGE: removed old API"));
        assert!(!result.contains("Signed-off-by:"));
        // def5678 has no body — just header
        assert!(result.contains("def5678"));
        // 3 lines: header1, body1 indented, header2
        assert_eq!(result.lines().count(), 3);
    }

    #[test]
    fn test_filter_log_output_skips_trailers() {
        // Body with only trailers should not produce a body line
        let output = "abc1234 chore: bump (1 day ago) <bot>\nSigned-off-by: Bot <bot@ci>\nCo-authored-by: Human <h@b>\n---END---\n";
        let result = filter_log_output(output, 10, false, false);
        assert!(result.contains("abc1234"));
        assert!(!result.contains("Signed-off-by:"));
        assert!(!result.contains("Co-authored-by:"));
        assert_eq!(result.lines().count(), 1);
    }

    #[test]
    fn test_filter_log_output_truncate_long() {
        let long_line = "abc1234 ".to_string() + &"x".repeat(100) + " (2 days ago) <author>";
        let result = filter_log_output(&long_line, 10, false, false);
        assert!(result.chars().count() < long_line.chars().count());
        assert!(result.contains("..."));
        assert!(result.chars().count() <= 80);
    }

    #[test]
    fn test_filter_log_output_cap_lines() {
        let output = (0..20)
            .map(|i| format!("hash{} message {} (1 day ago) <author>\n\n---END---", i, i))
            .collect::<Vec<_>>()
            .join("\n");
        let result = filter_log_output(&output, 5, false, false);
        assert_eq!(result.lines().count(), 5);
    }

    #[test]
    fn test_filter_log_output_user_limit_no_cap() {
        // When user explicitly passes -N, all N lines should be returned (no re-truncation)
        let output = (0..20)
            .map(|i| format!("hash{} message {} (1 day ago) <author>\n\n---END---", i, i))
            .collect::<Vec<_>>()
            .join("\n");
        let result = filter_log_output(&output, 20, true, false);
        assert_eq!(
            result.lines().count(),
            20,
            "User's -20 should return all 20 lines"
        );
    }

    #[test]
    fn test_filter_log_output_user_limit_wider_truncation() {
        // When user explicitly passes -N, lines up to 120 chars should NOT be truncated
        let line_90_chars = format!("abc1234 {} (2 days ago) <author>", "x".repeat(60));
        assert!(line_90_chars.chars().count() > 80);
        assert!(line_90_chars.chars().count() < 120);

        let result_default = filter_log_output(&line_90_chars, 10, false, false);
        let result_user = filter_log_output(&line_90_chars, 10, true, false);

        // Default truncates at 80 chars
        assert!(
            result_default.contains("..."),
            "Default should truncate at 80 chars"
        );
        // User-set limit uses wider threshold (120 chars)
        assert!(
            !result_user.contains("..."),
            "User limit should not truncate 90-char line"
        );
    }

    #[test]
    fn test_parse_user_limit_combined() {
        let args: Vec<String> = vec!["-20".into()];
        assert_eq!(parse_user_limit(&args), Some(20));
    }

    #[test]
    fn test_parse_user_limit_n_space() {
        let args: Vec<String> = vec!["-n".into(), "15".into()];
        assert_eq!(parse_user_limit(&args), Some(15));
    }

    #[test]
    fn test_parse_user_limit_max_count_eq() {
        let args: Vec<String> = vec!["--max-count=30".into()];
        assert_eq!(parse_user_limit(&args), Some(30));
    }

    #[test]
    fn test_parse_user_limit_max_count_space() {
        let args: Vec<String> = vec!["--max-count".into(), "25".into()];
        assert_eq!(parse_user_limit(&args), Some(25));
    }

    #[test]
    fn test_parse_user_limit_none() {
        let args: Vec<String> = vec!["--oneline".into()];
        assert_eq!(parse_user_limit(&args), None);
    }

    #[test]
    fn test_filter_log_output_token_savings() {
        fn count_tokens(text: &str) -> usize {
            text.split_whitespace().count()
        }
        // Simulate verbose git log output (default format with full metadata)
        let input = (0..20)
            .map(|i| {
                format!(
                    "commit abc123{:02x}\nAuthor: User Name <user@example.com>\nDate:   Mon Mar 10 10:00:00 2026 +0000\n\n    fix: commit message number {}\n\n    Extended body with details about the change.\n",
                    i, i
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let output = filter_log_output(&input, 10, false, false);
        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(&input) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Expected ≥60% token savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_status_with_args() {
        let output = r#"On branch main
Your branch is up to date with 'origin/main'.

Changes not staged for commit:
  (use "git add <file>..." to update what will be committed)
  (use "git restore <file>..." to discard changes in working directory)
	modified:   src/main.rs

no changes added to commit (use "git add" and/or "git commit -a")
"#;
        let result = filter_status_with_args(output);
        eprintln!("Result:\n{}", result);
        assert!(result.contains("On branch main"));
        assert!(result.contains("modified:   src/main.rs"));
        assert!(
            !result.contains("(use \"git"),
            "Result should not contain git hints"
        );
    }

    #[test]
    fn test_filter_status_with_args_clean() {
        let output = "nothing to commit, working tree clean\n";
        let result = filter_status_with_args(output);
        assert!(result.contains("nothing to commit"));
    }

    #[test]
    fn test_filter_log_output_multibyte() {
        // Thai characters: each is 3 bytes. A line with >80 bytes but few chars
        let thai_msg = format!("abc1234 {} (2 days ago) <author>", "ก".repeat(30));
        let result = filter_log_output(&thai_msg, 10, false, false);
        // Should not panic
        assert!(result.contains("abc1234"));
        // The line has 30 Thai chars + other text, so > 80 chars total
        // truncate_line now counts chars, not bytes
        // 30 Thai + ~33 other = 63 chars < 80 threshold, so no truncation
        assert!(result.contains("abc1234"));
    }

    #[test]
    fn test_filter_log_output_emoji() {
        let emoji_msg = "abc1234 🎉🎊🎈🎁🎂🎄🎃🎆🎇✨🎉🎊🎈🎁🎂🎄🎃🎆🎇✨ (1 day ago) <user>";
        let result = filter_log_output(emoji_msg, 10, false, false);
        // Should not panic
        // 20 emoji + ~30 other chars = ~50 chars < 80, no truncation needed
        assert!(result.contains("abc1234"));
    }

    #[test]
    fn test_format_status_output_thai_filename() {
        let porcelain = "## main\n M สวัสดี.txt\n?? ทดสอบ.rs\n";
        let result = format_status_output(porcelain);
        // Should not panic
        assert!(result.contains("* main"));
        assert!(result.contains("สวัสดี.txt"));
        assert!(result.contains("ทดสอบ.rs"));
    }

    #[test]
    fn test_format_status_output_emoji_filename() {
        let porcelain = "## main\nA  🎉-party.txt\n M 日本語ファイル.rs\n";
        let result = format_status_output(porcelain);
        assert!(result.contains("* main"));
    }

    /// Regression test: --oneline and other user format flags must preserve all commits.
    /// Before fix, filter_log_output split on ---END--- which doesn't exist when
    /// the user specifies their own format, resulting in only 2 commits surviving.
    #[test]
    fn test_filter_log_output_user_format_oneline() {
        let oneline_output = "abc1234 feat: add feature\n\
                              def5678 fix: typo\n\
                              ghi9012 chore: bump deps\n\
                              jkl3456 docs: update readme\n\
                              mno7890 test: add tests\n";

        let result = filter_log_output(oneline_output, 10, false, true);
        // All 5 lines must survive — no ---END--- splitting
        assert_eq!(result.lines().count(), 5);
        assert!(result.contains("abc1234"));
        assert!(result.contains("mno7890"));
    }

    #[test]
    fn test_filter_log_output_user_format_with_limit() {
        let oneline_output = "abc1234 feat: add feature\n\
                              def5678 fix: typo\n\
                              ghi9012 chore: bump deps\n\
                              jkl3456 docs: update readme\n\
                              mno7890 test: add tests\n";

        // user_set_limit=true means respect all lines (no cap)
        let result = filter_log_output(oneline_output, 3, true, true);
        assert_eq!(result.lines().count(), 5);

        // user_set_limit=false means cap at limit
        let result = filter_log_output(oneline_output, 3, false, true);
        assert_eq!(result.lines().count(), 3);
    }

    /// Regression test: `git branch <name>` must create, not list.
    /// Before fix, positional args fell into list mode which added `-a`,
    /// turning creation into a pattern-filtered listing (silent no-op).
    #[test]
    #[ignore] // Integration test: requires git repo
    fn test_branch_creation_not_swallowed() {
        let branch = "test-rtk-create-branch-regression";
        // Create branch via run_branch
        run_branch(&[branch.to_string()], 0, &[]).expect("run_branch should succeed");
        // Verify it exists
        let output = Command::new("git")
            .args(["branch", "--list", branch])
            .output()
            .expect("git branch --list should work");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(branch),
            "Branch '{}' was not created. run_branch silently swallowed the creation.",
            branch
        );
        // Cleanup
        let _ = Command::new("git").args(["branch", "-d", branch]).output();
    }

    /// Regression test: `git branch <name> <commit>` must create from commit.
    #[test]
    #[ignore] // Integration test: requires git repo
    fn test_branch_creation_from_commit() {
        let branch = "test-rtk-create-from-commit";
        run_branch(&[branch.to_string(), "HEAD".to_string()], 0, &[])
            .expect("run_branch with start-point should succeed");
        let output = Command::new("git")
            .args(["branch", "--list", branch])
            .output()
            .expect("git branch --list should work");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(branch),
            "Branch '{}' was not created from commit.",
            branch
        );
        let _ = Command::new("git").args(["branch", "-d", branch]).output();
    }

    #[test]
    fn test_commit_single_message() {
        let args = vec!["-m".to_string(), "fix: typo".to_string()];
        let cmd = build_commit_command(&args, &[]);
        let cmd_args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(cmd_args, vec!["commit", "-m", "fix: typo"]);
    }

    #[test]
    fn test_commit_multiple_messages() {
        let args = vec![
            "-m".to_string(),
            "feat: add multi-paragraph support".to_string(),
            "-m".to_string(),
            "This allows git commit -m \"title\" -m \"body\".".to_string(),
        ];
        let cmd = build_commit_command(&args, &[]);
        let cmd_args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(
            cmd_args,
            vec![
                "commit",
                "-m",
                "feat: add multi-paragraph support",
                "-m",
                "This allows git commit -m \"title\" -m \"body\"."
            ]
        );
    }

    // #327: git commit -am "msg" must pass -am through to git
    #[test]
    fn test_commit_am_flag() {
        let args = vec!["-am".to_string(), "quick fix".to_string()];
        let cmd = build_commit_command(&args, &[]);
        let cmd_args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(cmd_args, vec!["commit", "-am", "quick fix"]);
    }

    #[test]
    fn test_commit_amend() {
        let args = vec![
            "--amend".to_string(),
            "-m".to_string(),
            "new msg".to_string(),
        ];
        let cmd = build_commit_command(&args, &[]);
        let cmd_args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(cmd_args, vec!["commit", "--amend", "-m", "new msg"]);
    }

    #[test]
    #[ignore] // Requires `cargo build` first — run with `cargo test --ignored`
    fn test_git_status_not_a_repo_exits_nonzero() {
        // Run rtk git status in a directory that is not a git repo
        let tmp = std::env::temp_dir().join("rtk_test_not_a_repo");
        let _ = std::fs::create_dir_all(&tmp);

        // Build the path to the test binary
        let bin_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("debug")
            .join("rtk");
        assert!(
            bin_path.exists(),
            "Debug binary not found at {:?} — run `cargo build` first",
            bin_path
        );
        let output = std::process::Command::new(&bin_path)
            .args(["git", "status"])
            .current_dir(&tmp)
            .output()
            .expect("Failed to run rtk");

        // Should exit with non-zero (128 from git)
        assert!(
            !output.status.success(),
            "Expected non-zero exit code for git status outside a repo, got {:?}",
            output.status.code()
        );

        // Message should be on stderr, not stdout
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stderr.to_lowercase().contains("not a git repository"),
            "Expected 'not a git repository' on stderr, got stderr={:?}, stdout={:?}",
            stderr,
            stdout
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // --- truncation accuracy ---

    #[test]
    fn test_format_status_overflow_count_exact() {
        // 25 staged files, default status_max_files = 15
        // Should show 15, overflow = 25 - 15 = 10, report "+10 more"
        let mut porcelain = String::from("## main...origin/main\n");
        for i in 0..25 {
            porcelain.push_str(&format!("M  staged_file_{}.rs\n", i));
        }
        let result = format_status_output(&porcelain);
        assert!(
            result.contains("+10 more"),
            "Expected '+10 more' for 25 staged files (max_files=15), got:\n{}",
            result
        );
        assert!(
            result.contains("Staged: 25 files"),
            "Expected 'Staged: 25 files', got:\n{}",
            result
        );
    }

    #[test]
    fn test_compact_diff_recovery_hint_present() {
        // A hunk with 110 lines exceeds max_hunk_lines (100), triggers truncation
        // The recovery hint must appear so LLMs can re-fetch the full diff
        let mut diff = String::new();
        diff.push_str("diff --git a/large.rs b/large.rs\n");
        diff.push_str("--- a/large.rs\n");
        diff.push_str("+++ b/large.rs\n");
        diff.push_str("@@ -1,150 +1,150 @@\n");
        for i in 0..110 {
            diff.push_str(&format!("+added line {}\n", i));
        }
        let result = compact_diff(&diff, 500);
        assert!(
            result.contains("[full diff: rtk git diff --no-compact]"),
            "Expected recovery hint when hunk is truncated, got:\n{}",
            result
        );
    }

    #[test]
    fn test_compact_diff_hunk_truncation_count_accurate() {
        // 150 change lines in one hunk: 100 shown, 50 silently dropped
        // Must report the exact count, not just "(truncated)"
        let mut diff = String::from(
            "diff --git a/large.rs b/large.rs\n--- a/large.rs\n+++ b/large.rs\n@@ -1,150 +1,150 @@\n",
        );
        for i in 0..150 {
            diff.push_str(&format!("+line {}\n", i));
        }
        let result = compact_diff(&diff, 500);
        assert!(
            result.contains("50 lines truncated"),
            "Expected '50 lines truncated' (150 - 100 = 50), got:\n{}",
            result
        );
    }

    #[test]
    fn test_filter_log_output_body_omission_indicator() {
        // Commit with 6 meaningful body lines: only 3 shown, must signal "+3 lines omitted"
        let body_lines = (1..=6)
            .map(|i| format!("body line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let output = format!(
            "abc1234 feat: big change (1 day ago) <author>\n{}\n---END---\n",
            body_lines
        );
        let result = filter_log_output(&output, 10, false, false);
        assert!(
            result.contains("+3 lines omitted"),
            "Expected '+3 lines omitted' when 6 body lines truncated to 3, got:\n{}",
            result
        );
    }
}
