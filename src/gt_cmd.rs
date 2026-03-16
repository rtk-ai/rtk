use crate::tracking;
use crate::utils::{ok_confirmation, resolved_command, strip_ansi, truncate};
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use std::ffi::OsString;

lazy_static! {
    static ref EMAIL_RE: Regex =
        Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b").unwrap();
    static ref BRANCH_NAME_RE: Regex = Regex::new(
        r#"(?:Created|Pushed|pushed|Deleted|deleted)\s+branch\s+[`"']?([a-zA-Z0-9/_.\-+@]+)"#
    )
    .unwrap();
    static ref PR_LINE_RE: Regex =
        Regex::new(r"(Created|Updated)\s+pull\s+request\s+#(\d+)\s+for\s+([^\s:]+)(?::\s*(\S+))?")
            .unwrap();
}

fn run_gt_filtered(
    subcmd: &[&str],
    args: &[String],
    verbose: u8,
    tee_label: &str,
    filter_fn: fn(&str) -> String,
) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("gt");
    for part in subcmd {
        cmd.arg(part);
    }
    for arg in args {
        cmd.arg(arg);
    }

    let subcmd_str = subcmd.join(" ");
    if verbose > 0 {
        eprintln!("Running: gt {} {}", subcmd_str, args.join(" "));
    }

    let cmd_output = cmd.output().with_context(|| {
        format!(
            "Failed to run gt {}. Is gt (Graphite) installed?",
            subcmd_str
        )
    })?;

    let stdout = String::from_utf8_lossy(&cmd_output.stdout);
    let stderr = String::from_utf8_lossy(&cmd_output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = cmd_output.status.code().unwrap_or(1);

    let clean = strip_ansi(stdout.trim());
    let output = if verbose > 0 {
        clean.clone()
    } else {
        filter_fn(&clean)
    };

    if let Some(hint) = crate::tee::tee_and_hint(&raw, tee_label, exit_code) {
        println!("{}\n{}", output, hint);
    } else {
        println!("{}", output);
    }

    if !stderr.trim().is_empty() {
        eprintln!("{}", stderr.trim());
    }

    let label = if args.is_empty() {
        format!("gt {}", subcmd_str)
    } else {
        format!("gt {} {}", subcmd_str, args.join(" "))
    };
    let rtk_label = format!("rtk {}", label);
    timer.track(&label, &rtk_label, &raw, &output);

    if !cmd_output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

fn filter_identity(input: &str) -> String {
    input.to_string()
}

pub fn run_log(args: &[String], verbose: u8) -> Result<()> {
    match args.first().map(|s| s.as_str()) {
        Some("short") => run_gt_filtered(
            &["log", "short"],
            &args[1..],
            verbose,
            "gt_log_short",
            filter_identity,
        ),
        Some("long") => run_gt_filtered(
            &["log", "long"],
            &args[1..],
            verbose,
            "gt_log_long",
            filter_gt_log_entries,
        ),
        _ => run_gt_filtered(&["log"], args, verbose, "gt_log", filter_gt_log_entries),
    }
}

pub fn run_submit(args: &[String], verbose: u8) -> Result<()> {
    run_gt_filtered(&["submit"], args, verbose, "gt_submit", filter_gt_submit)
}

pub fn run_sync(args: &[String], verbose: u8) -> Result<()> {
    run_gt_filtered(&["sync"], args, verbose, "gt_sync", filter_gt_sync)
}

pub fn run_restack(args: &[String], verbose: u8) -> Result<()> {
    run_gt_filtered(&["restack"], args, verbose, "gt_restack", filter_gt_restack)
}

pub fn run_create(args: &[String], verbose: u8) -> Result<()> {
    run_gt_filtered(&["create"], args, verbose, "gt_create", filter_gt_create)
}

pub fn run_branch(args: &[String], verbose: u8) -> Result<()> {
    run_gt_filtered(&["branch"], args, verbose, "gt_branch", filter_identity)
}

pub fn run_other(args: &[OsString], verbose: u8) -> Result<()> {
    if args.is_empty() {
        anyhow::bail!("gt: no subcommand specified");
    }

    let subcommand = args[0].to_string_lossy();
    let rest: Vec<String> = args[1..]
        .iter()
        .map(|a| a.to_string_lossy().into())
        .collect();

    // gt passes unknown subcommands to git, so "gt status" = "git status".
    // Route known git commands to RTK's git filters for token savings.
    match subcommand.as_ref() {
        "status" => crate::git::run(crate::git::GitCommand::Status, &rest, None, verbose, &[]),
        "diff" => crate::git::run(crate::git::GitCommand::Diff, &rest, None, verbose, &[]),
        "show" => crate::git::run(crate::git::GitCommand::Show, &rest, None, verbose, &[]),
        "add" => crate::git::run(crate::git::GitCommand::Add, &rest, None, verbose, &[]),
        "push" => crate::git::run(crate::git::GitCommand::Push, &rest, None, verbose, &[]),
        "pull" => crate::git::run(crate::git::GitCommand::Pull, &rest, None, verbose, &[]),
        "fetch" => crate::git::run(crate::git::GitCommand::Fetch, &rest, None, verbose, &[]),
        "stash" => {
            let stash_sub = rest.first().cloned();
            let stash_args = rest.get(1..).unwrap_or(&[]);
            crate::git::run(
                crate::git::GitCommand::Stash {
                    subcommand: stash_sub,
                },
                stash_args,
                None,
                verbose,
                &[],
            )
        }
        "worktree" => crate::git::run(crate::git::GitCommand::Worktree, &rest, None, verbose, &[]),
        _ => passthrough_gt(&subcommand, &rest, verbose),
    }
}

fn passthrough_gt(subcommand: &str, args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("gt");
    cmd.arg(subcommand);
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: gt {} {}", subcommand, args.join(" "));
    }

    let status = cmd
        .status()
        .with_context(|| format!("Failed to run gt {}", subcommand))?;

    let args_str = if args.is_empty() {
        subcommand.to_string()
    } else {
        format!("{} {}", subcommand, args.join(" "))
    };
    timer.track_passthrough(
        &format!("gt {}", args_str),
        &format!("rtk gt {} (passthrough)", args_str),
    );

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

const MAX_LOG_ENTRIES: usize = 15;

fn filter_gt_log_entries(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let lines: Vec<&str> = trimmed.lines().collect();
    let mut result = Vec::new();
    let mut entry_count = 0;

    for (i, line) in lines.iter().enumerate() {
        if is_graph_node(line) {
            entry_count += 1;
        }

        let replaced = EMAIL_RE.replace_all(line, "");
        let processed = truncate(replaced.trim_end(), 120);
        result.push(processed);

        if entry_count >= MAX_LOG_ENTRIES {
            let remaining = lines[i + 1..].iter().filter(|l| is_graph_node(l)).count();
            if remaining > 0 {
                result.push(format!("... +{} more entries", remaining));
            }
            break;
        }
    }

    result.join("\n")
}

fn filter_gt_submit(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut pushed = Vec::new();
    let mut prs = Vec::new();

    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if line.contains("pushed") || line.contains("Pushed") {
            pushed.push(extract_branch_name(line));
        } else if let Some(caps) = PR_LINE_RE.captures(line) {
            let action = caps[1].to_lowercase();
            let num = &caps[2];
            let branch = &caps[3];
            if let Some(url) = caps.get(4) {
                prs.push(format!(
                    "{} PR #{} {} {}",
                    action,
                    num,
                    branch,
                    url.as_str()
                ));
            } else {
                prs.push(format!("{} PR #{} {}", action, num, branch));
            }
        }
    }

    let mut summary = Vec::new();

    if !pushed.is_empty() {
        let branch_names: Vec<&str> = pushed
            .iter()
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        if !branch_names.is_empty() {
            summary.push(format!("pushed {}", branch_names.join(", ")));
        } else {
            summary.push(format!("pushed {} branches", pushed.len()));
        }
    }

    summary.extend(prs);

    if summary.is_empty() {
        return truncate(trimmed, 200);
    }

    summary.join("\n")
}

fn filter_gt_sync(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut synced = 0;
    let mut deleted = 0;
    let mut deleted_names = Vec::new();

    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if (line.contains("Synced") && line.contains("branch"))
            || line.starts_with("Synced with remote")
        {
            synced += 1;
        }
        if line.contains("deleted") || line.contains("Deleted") {
            deleted += 1;
            let name = extract_branch_name(line);
            if !name.is_empty() {
                deleted_names.push(name);
            }
        }
    }

    let mut parts = Vec::new();

    if synced > 0 {
        parts.push(format!("{} synced", synced));
    }

    if deleted > 0 {
        if deleted_names.is_empty() {
            parts.push(format!("{} deleted", deleted));
        } else {
            parts.push(format!(
                "{} deleted ({})",
                deleted,
                deleted_names.join(", ")
            ));
        }
    }

    if parts.is_empty() {
        return ok_confirmation("synced", "");
    }

    format!("ok sync: {}", parts.join(", "))
}

fn filter_gt_restack(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut restacked = 0;
    for line in trimmed.lines() {
        let line = line.trim();
        if (line.contains("Restacked") || line.contains("Rebased")) && line.contains("branch") {
            restacked += 1;
        }
    }

    if restacked > 0 {
        ok_confirmation("restacked", &format!("{} branches", restacked))
    } else {
        ok_confirmation("restacked", "")
    }
}

fn filter_gt_create(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let branch_name = trimmed
        .lines()
        .find_map(|line| {
            let line = line.trim();
            if line.contains("Created") || line.contains("created") {
                Some(extract_branch_name(line))
            } else {
                None
            }
        })
        .unwrap_or_default();

    if branch_name.is_empty() {
        let first_line = trimmed.lines().next().unwrap_or("");
        ok_confirmation("created", first_line.trim())
    } else {
        ok_confirmation("created", &branch_name)
    }
}

fn is_graph_node(line: &str) -> bool {
    let stripped = line
        .trim_start_matches('│')
        .trim_start_matches('|')
        .trim_start();
    stripped.starts_with('◉')
        || stripped.starts_with('○')
        || stripped.starts_with('◯')
        || stripped.starts_with('◆')
        || stripped.starts_with('●')
        || stripped.starts_with('@')
        || stripped.starts_with('*')
}

fn extract_branch_name(line: &str) -> String {
    BRANCH_NAME_RE
        .captures(line)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_filter_gt_log_exact_format() {
        let input = r#"◉  abc1234 feat/add-auth 2d ago
│  feat(auth): add login endpoint
│
◉  def5678 feat/add-db 3d ago user@example.com
│  feat(db): add migration system
│
◉  ghi9012 main 5d ago admin@corp.io
│  chore: update dependencies
~
"#;
        let output = filter_gt_log_entries(input);
        let expected = "\
◉  abc1234 feat/add-auth 2d ago
│  feat(auth): add login endpoint
│
◉  def5678 feat/add-db 3d ago
│  feat(db): add migration system
│
◉  ghi9012 main 5d ago
│  chore: update dependencies
~";
        assert_eq!(output, expected);
    }

    #[test]
    fn test_filter_gt_submit_exact_format() {
        let input = r#"Pushed branch feat/add-auth
Created pull request #42 for feat/add-auth
Pushed branch feat/add-db
Updated pull request #40 for feat/add-db
"#;
        let output = filter_gt_submit(input);
        let expected = "\
pushed feat/add-auth, feat/add-db
created PR #42 feat/add-auth
updated PR #40 feat/add-db";
        assert_eq!(output, expected);
    }

    #[test]
    fn test_filter_gt_sync_exact_format() {
        let input = r#"Synced with remote
Deleted branch feat/merged-feature
Deleted branch fix/old-hotfix
"#;
        let output = filter_gt_sync(input);
        assert_eq!(
            output,
            "ok sync: 1 synced, 2 deleted (feat/merged-feature, fix/old-hotfix)"
        );
    }

    #[test]
    fn test_filter_gt_restack_exact_format() {
        let input = r#"Restacked branch feat/add-auth on main
Restacked branch feat/add-db on feat/add-auth
Restacked branch fix/parsing on feat/add-db
"#;
        let output = filter_gt_restack(input);
        assert_eq!(output, "ok restacked 3 branches");
    }

    #[test]
    fn test_filter_gt_create_exact_format() {
        let input = "Created branch feat/new-feature\n";
        let output = filter_gt_create(input);
        assert_eq!(output, "ok created feat/new-feature");
    }

    #[test]
    fn test_filter_gt_log_truncation() {
        let mut input = String::new();
        for i in 0..20 {
            input.push_str(&format!(
                "◉  hash{:02} branch-{} 1d ago dev@example.com\n│  commit message {}\n│\n",
                i, i, i
            ));
        }
        input.push_str("~\n");

        let output = filter_gt_log_entries(&input);
        assert!(output.contains("... +"));
    }

    #[test]
    fn test_filter_gt_log_empty() {
        assert_eq!(filter_gt_log_entries(""), String::new());
        assert_eq!(filter_gt_log_entries("  "), String::new());
    }

    #[test]
    fn test_filter_gt_log_token_savings() {
        let mut input = String::new();
        for i in 0..40 {
            input.push_str(&format!(
                "◉  hash{:02}abc feat/feature-{} {}d ago developer{}@longcompany.example.com\n\
                 │  feat(module-{}): implement feature {} with detailed description of changes\n│\n",
                i,
                i,
                i + 1,
                i,
                i,
                i
            ));
        }
        input.push_str("~\n");

        let output = filter_gt_log_entries(&input);
        let input_tokens = count_tokens(&input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "gt log filter: expected >=60% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_filter_gt_log_long() {
        let input = r#"◉  abc1234 feat/add-auth
│  Author: Dev User <dev@example.com>
│  Date: 2026-02-25 10:30:00 -0800
│
│  feat(auth): add login endpoint with OAuth2 support
│  and session management for web clients
│
◉  def5678 feat/add-db
│  Author: Other Dev <other@example.com>
│  Date: 2026-02-24 14:00:00 -0800
│
│  feat(db): add migration system
~
"#;

        let output = filter_gt_log_entries(input);
        assert!(output.contains("abc1234"));
        assert!(!output.contains("dev@example.com"));
        assert!(!output.contains("other@example.com"));
    }

    #[test]
    fn test_filter_gt_submit_empty() {
        assert_eq!(filter_gt_submit(""), String::new());
    }

    #[test]
    fn test_filter_gt_submit_with_urls() {
        let input =
            "Created pull request #42 for feat/add-auth: https://github.com/org/repo/pull/42\n";
        let output = filter_gt_submit(input);
        assert!(output.contains("PR #42"));
        assert!(output.contains("feat/add-auth"));
        assert!(output.contains("https://github.com/org/repo/pull/42"));
    }

    #[test]
    fn test_filter_gt_submit_token_savings() {
        let input = r#"
  ✅  Pushing to remote...
  Enumerating objects: 15, done.
  Counting objects: 100% (15/15), done.
  Delta compression using up to 10 threads
  Compressing objects: 100% (8/8), done.
  Writing objects: 100% (10/10), 2.50 KiB | 2.50 MiB/s, done.
  Total 10 (delta 5), reused 0 (delta 0), pack-reused 0
  Pushed branch feat/add-auth to origin
  Creating pull request for feat/add-auth...
  Created pull request #42 for feat/add-auth: https://github.com/org/repo/pull/42
  ✅  Pushing to remote...
  Enumerating objects: 8, done.
  Counting objects: 100% (8/8), done.
  Delta compression using up to 10 threads
  Compressing objects: 100% (4/4), done.
  Writing objects: 100% (5/5), 1.20 KiB | 1.20 MiB/s, done.
  Total 5 (delta 3), reused 0 (delta 0), pack-reused 0
  Pushed branch feat/add-db to origin
  Updating pull request for feat/add-db...
  Updated pull request #40 for feat/add-db: https://github.com/org/repo/pull/40
  ✅  Pushing to remote...
  Enumerating objects: 5, done.
  Counting objects: 100% (5/5), done.
  Delta compression using up to 10 threads
  Compressing objects: 100% (3/3), done.
  Writing objects: 100% (3/3), 890 bytes | 890.00 KiB/s, done.
  Total 3 (delta 2), reused 0 (delta 0), pack-reused 0
  Pushed branch fix/parsing to origin
  All branches submitted successfully!
"#;

        let output = filter_gt_submit(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "gt submit filter: expected >=60% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_filter_gt_sync() {
        let input = r#"Synced with remote
Deleted branch feat/merged-feature
Deleted branch fix/old-hotfix
"#;

        let output = filter_gt_sync(input);
        assert!(output.contains("ok sync"));
        assert!(output.contains("synced"));
        assert!(output.contains("deleted"));
    }

    #[test]
    fn test_filter_gt_sync_empty() {
        assert_eq!(filter_gt_sync(""), String::new());
    }

    #[test]
    fn test_filter_gt_sync_no_deletes() {
        let input = "Synced with remote\n";
        let output = filter_gt_sync(input);
        assert!(output.contains("ok sync"));
        assert!(output.contains("synced"));
        assert!(!output.contains("deleted"));
    }

    #[test]
    fn test_filter_gt_restack() {
        let input = r#"Restacked branch feat/add-auth on main
Restacked branch feat/add-db on feat/add-auth
Restacked branch fix/parsing on feat/add-db
"#;

        let output = filter_gt_restack(input);
        assert!(output.contains("ok restacked"));
        assert!(output.contains("3 branches"));
    }

    #[test]
    fn test_filter_gt_restack_empty() {
        assert_eq!(filter_gt_restack(""), String::new());
    }

    #[test]
    fn test_filter_gt_create() {
        let input = "Created branch feat/new-feature\n";
        let output = filter_gt_create(input);
        assert_eq!(output, "ok created feat/new-feature");
    }

    #[test]
    fn test_filter_gt_create_empty() {
        assert_eq!(filter_gt_create(""), String::new());
    }

    #[test]
    fn test_filter_gt_create_no_branch_name() {
        let input = "Some unexpected output\n";
        let output = filter_gt_create(input);
        assert!(output.starts_with("ok created"));
    }

    #[test]
    fn test_is_graph_node() {
        assert!(is_graph_node("◉  abc1234 main"));
        assert!(is_graph_node("○  def5678 feat/x"));
        assert!(is_graph_node("@  ghi9012 (current)"));
        assert!(is_graph_node("*  jkl3456 branch"));
        assert!(is_graph_node("│ ◉  nested node"));
        assert!(!is_graph_node("│  just a message line"));
        assert!(!is_graph_node("~"));
    }

    #[test]
    fn test_extract_branch_name() {
        assert_eq!(
            extract_branch_name("Created branch feat/new-feature"),
            "feat/new-feature"
        );
        assert_eq!(
            extract_branch_name("Pushed branch fix/bug-123"),
            "fix/bug-123"
        );
        assert_eq!(
            extract_branch_name("Pushed branch feat/auth+session"),
            "feat/auth+session"
        );
        assert_eq!(extract_branch_name("Created branch user@fix"), "user@fix");
        assert_eq!(extract_branch_name("no branch here"), "");
    }

    #[test]
    fn test_filter_gt_log_pre_stripped_input() {
        let input = "◉  abc1234 feat/x 1d ago user@test.com\n│  message\n~\n";
        let output = filter_gt_log_entries(input);
        assert!(output.contains("abc1234"));
        assert!(!output.contains("user@test.com"));
    }

    #[test]
    fn test_filter_gt_sync_token_savings() {
        let input = r#"
  ✅ Syncing with remote...
  Pulling latest changes from main...
  Successfully pulled 5 new commits
  Synced branch feat/add-auth with remote
  Synced branch feat/add-db with remote
  Branch feat/merged-feature has been merged
  Deleted branch feat/merged-feature
  Branch fix/old-hotfix has been merged
  Deleted branch fix/old-hotfix
  All branches synced!
"#;

        let output = filter_gt_sync(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "gt sync filter: expected >=60% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_filter_gt_create_token_savings() {
        let input = r#"
  ✅ Creating new branch...
  Checking out from feat/add-auth...
  Created branch feat/new-feature from feat/add-auth
  Tracking branch set up to follow feat/add-auth
  Branch feat/new-feature is ready for development
"#;

        let output = filter_gt_create(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "gt create filter: expected >=60% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_filter_gt_restack_token_savings() {
        let input = r#"
  ✅ Restacking branches...
  Restacked branch feat/add-auth on top of main
  Successfully rebased feat/add-auth (3 commits)
  Restacked branch feat/add-db on top of feat/add-auth
  Successfully rebased feat/add-db (2 commits)
  Restacked branch fix/parsing on top of feat/add-db
  Successfully rebased fix/parsing (1 commit)
  All branches restacked!
"#;

        let output = filter_gt_restack(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "gt restack filter: expected >=60% savings, got {:.1}%",
            savings
        );
    }
}
