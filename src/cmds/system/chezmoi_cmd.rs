//! Filters chezmoi dotfile-manager output.

use crate::core::guard::never_worse;
use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::truncate::{reduced, CAP_LIST};
use crate::core::utils::{resolved_command, strip_ansi, truncate};
use anyhow::{Context, Result};
use std::ffi::OsString;

const MAX_FILES: usize = reduced(CAP_LIST, 3);

#[derive(Debug, Clone)]
pub enum ChezmoiCommand {
    Diff,
    Apply,
    Status,
    Managed,
    Add,
    ReAdd,
    Update,
    Unmanaged,
    Doctor,
}

impl ChezmoiCommand {
    fn as_str(&self) -> &'static str {
        match self {
            ChezmoiCommand::Diff => "diff",
            ChezmoiCommand::Apply => "apply",
            ChezmoiCommand::Status => "status",
            ChezmoiCommand::Managed => "managed",
            ChezmoiCommand::Add => "add",
            ChezmoiCommand::ReAdd => "re-add",
            ChezmoiCommand::Update => "update",
            ChezmoiCommand::Unmanaged => "unmanaged",
            ChezmoiCommand::Doctor => "doctor",
        }
    }
}

pub fn run(cmd: ChezmoiCommand, args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let subcommand = cmd.as_str();

    let mut command = resolved_command("chezmoi");
    command.arg(subcommand).args(args);

    if verbose > 0 {
        eprintln!("Running: chezmoi {} {}", subcommand, args.join(" "));
    }

    let result = exec_capture(&mut command)
        .with_context(|| format!("Failed to run chezmoi {}. Is chezmoi installed?", subcommand))?;
    let raw = format!("{}\n{}", result.stdout, result.stderr);
    let clean = strip_ansi(result.stdout.trim());
    let filtered = match cmd {
        ChezmoiCommand::Diff => filter_chezmoi_diff(&clean),
        ChezmoiCommand::Apply => filter_chezmoi_apply(&clean),
        ChezmoiCommand::Status => filter_chezmoi_status(&clean),
        ChezmoiCommand::Managed => filter_path_list(&clean, "managed"),
        ChezmoiCommand::Add => filter_chezmoi_add(&clean),
        ChezmoiCommand::ReAdd => filter_chezmoi_add(&clean),
        ChezmoiCommand::Update => filter_chezmoi_update(&clean),
        ChezmoiCommand::Unmanaged => filter_path_list(&clean, "unmanaged"),
        ChezmoiCommand::Doctor => filter_chezmoi_doctor(&clean),
    };

    let shown = never_worse(&result.stdout, &filtered);
    println!("{}", shown);

    if !result.stderr.trim().is_empty() {
        eprintln!("{}", result.stderr.trim());
    }

    let label = if args.is_empty() {
        format!("chezmoi {}", subcommand)
    } else {
        format!("chezmoi {} {}", subcommand, args.join(" "))
    };
    timer.track(&label, &format!("rtk {}", label), &raw, shown);

    Ok(result.exit_code)
}

pub fn run_passthrough(args: &[OsString], verbose: u8) -> Result<i32> {
    crate::core::runner::run_passthrough("chezmoi", args, verbose)
}

pub fn run_git(args: &[String], verbose: u8) -> Result<i32> {
    let Some((global, subcommand, rest)) = split_git_invocation(args) else {
        let os_args = prepend_git(args);
        return run_passthrough(&os_args, verbose);
    };

    let Some(source_path) = chezmoi_source_path(verbose) else {
        let os_args = prepend_git(args);
        return run_passthrough(&os_args, verbose);
    };

    let mut global_args = vec!["-C".to_string(), source_path];
    global_args.extend_from_slice(global);

    match subcommand {
        "status" => crate::git::run(
            crate::git::GitCommand::Status,
            rest,
            None,
            verbose,
            &global_args,
        ),
        "diff" => crate::git::run(
            crate::git::GitCommand::Diff,
            rest,
            None,
            verbose,
            &global_args,
        ),
        "log" => crate::git::run(
            crate::git::GitCommand::Log,
            rest,
            None,
            verbose,
            &global_args,
        ),
        "show" => crate::git::run(
            crate::git::GitCommand::Show,
            rest,
            None,
            verbose,
            &global_args,
        ),
        "add" => crate::git::run(
            crate::git::GitCommand::Add,
            rest,
            None,
            verbose,
            &global_args,
        ),
        "commit" => crate::git::run(
            crate::git::GitCommand::Commit,
            rest,
            None,
            verbose,
            &global_args,
        ),
        "push" => crate::git::run(
            crate::git::GitCommand::Push,
            rest,
            None,
            verbose,
            &global_args,
        ),
        "pull" => crate::git::run(
            crate::git::GitCommand::Pull,
            rest,
            None,
            verbose,
            &global_args,
        ),
        "branch" => crate::git::run(
            crate::git::GitCommand::Branch,
            rest,
            None,
            verbose,
            &global_args,
        ),
        "fetch" => crate::git::run(
            crate::git::GitCommand::Fetch,
            rest,
            None,
            verbose,
            &global_args,
        ),
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
                &global_args,
            )
        }
        "worktree" => crate::git::run(
            crate::git::GitCommand::Worktree,
            rest,
            None,
            verbose,
            &global_args,
        ),
        _ => {
            let os_args = prepend_git(args);
            run_passthrough(&os_args, verbose)
        }
    }
}

fn chezmoi_source_path(verbose: u8) -> Option<String> {
    let mut cmd = resolved_command("chezmoi");
    cmd.arg("source-path");

    let result = match exec_capture(&mut cmd) {
        Ok(result) => result,
        Err(err) => {
            if verbose > 0 {
                eprintln!("chezmoi source-path failed: {}", err);
            }
            return None;
        }
    };

    if !result.success() {
        if verbose > 0 && !result.stderr.trim().is_empty() {
            eprintln!("chezmoi source-path: {}", result.stderr.trim());
        }
        return None;
    }

    let source_path = result.stdout.trim();
    if source_path.is_empty() {
        None
    } else {
        Some(source_path.to_string())
    }
}

fn split_git_invocation(args: &[String]) -> Option<(&[String], &str, &[String])> {
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--" {
            return None;
        }
        if !arg.starts_with('-') {
            return Some((&args[..i], arg, &args[i + 1..]));
        }
        if git_global_arg_takes_value(arg) {
            i += 1;
            if i >= args.len() {
                return None;
            }
        }
        i += 1;
    }
    None
}

fn git_global_arg_takes_value(arg: &str) -> bool {
    matches!(arg, "-C" | "-c" | "--git-dir" | "--work-tree")
}

fn prepend_git(args: &[String]) -> Vec<OsString> {
    let mut os_args = vec![OsString::from("git")];
    os_args.extend(args.iter().map(OsString::from));
    os_args
}

pub fn filter_chezmoi_diff(input: &str) -> String {
    crate::cmds::git::git::compact_diff(input, 60)
}

pub fn filter_chezmoi_apply(input: &str) -> String {
    filter_action_lines(input, "apply")
}

pub fn filter_chezmoi_status(input: &str) -> String {
    let lines: Vec<&str> = input.lines().filter(|line| !line.trim().is_empty()).collect();
    if lines.is_empty() {
        return String::new();
    }

    let mut out = format!("{} changes\n", lines.len());
    for line in lines.iter().take(MAX_FILES) {
        out.push_str(&format!("{}\n", truncate(line.trim(), 120)));
    }
    if lines.len() > MAX_FILES {
        out.push_str(&format!("+{} more\n", lines.len() - MAX_FILES));
    }
    out.trim_end().to_string()
}

pub fn filter_path_list(input: &str, label: &str) -> String {
    let paths: Vec<&str> = input.lines().filter(|line| !line.trim().is_empty()).collect();
    if paths.is_empty() {
        return String::new();
    }

    let mut out = format!("{} {} paths\n", paths.len(), label);
    for path in paths.iter().take(MAX_FILES) {
        out.push_str(&format!("{}\n", truncate(path.trim(), 120)));
    }
    if paths.len() > MAX_FILES {
        out.push_str(&format!("+{} more\n", paths.len() - MAX_FILES));
    }
    out.trim_end().to_string()
}

pub fn filter_chezmoi_add(input: &str) -> String {
    filter_action_lines(input, "add")
}

pub fn filter_chezmoi_update(input: &str) -> String {
    filter_action_lines(input, "update")
}

pub fn filter_chezmoi_doctor(input: &str) -> String {
    let lines: Vec<&str> = input.lines().filter(|line| !line.trim().is_empty()).collect();
    if lines.is_empty() {
        return String::new();
    }

    let failing: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("error")
                || lower.contains("warning")
                || lower.contains("fail")
                || lower.contains("not ok")
        })
        .collect();

    if failing.is_empty() {
        return format!("doctor: ok ({} checks)", lines.len());
    }

    let mut out = format!("doctor: {} issue(s)\n", failing.len());
    for line in failing.iter().take(MAX_FILES) {
        out.push_str(&format!("{}\n", truncate(line.trim(), 120)));
    }
    if failing.len() > MAX_FILES {
        out.push_str(&format!("+{} more\n", failing.len() - MAX_FILES));
    }
    out.trim_end().to_string()
}

fn filter_action_lines(input: &str, label: &str) -> String {
    let lines: Vec<&str> = input.lines().filter(|line| !line.trim().is_empty()).collect();
    if lines.is_empty() {
        return String::new();
    }

    let mut out = format!("{}: {} line(s)\n", label, lines.len());
    for line in lines.iter().take(MAX_FILES) {
        out.push_str(&format!("{}\n", truncate(line.trim(), 120)));
    }
    if lines.len() > MAX_FILES {
        out.push_str(&format!("+{} more\n", lines.len() - MAX_FILES));
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn savings(raw: &str, filtered: &str) -> f64 {
        100.0 - (filtered.len() as f64 / raw.len() as f64 * 100.0)
    }

    #[test]
    fn test_filter_chezmoi_diff_compacts_large_diff() {
        let raw = "\
diff --git a/dot_config/app/config.toml b/dot_config/app/config.toml
index 1111111..2222222 100644
--- a/dot_config/app/config.toml
+++ b/dot_config/app/config.toml
@@ -1,80 +1,80 @@
";
        let mut input = raw.to_string();
        for i in 0..120 {
            input.push_str(&format!("-old_setting_{} = false\n+new_setting_{} = true\n", i, i));
        }

        let filtered = filter_chezmoi_diff(&input);
        assert!(filtered.contains("dot_config/app/config.toml"));
        assert!(savings(&input, &filtered) >= 60.0);
    }

    #[test]
    fn test_filter_chezmoi_apply_compacts_output() {
        let raw = repeated_lines("updated /home/me/.config/app/config.toml\n", 80);
        let filtered = filter_chezmoi_apply(&raw);
        assert!(filtered.contains("apply: 80 line(s)"));
        assert!(filtered.contains("+"));
        assert!(savings(&raw, &filtered) >= 60.0);
    }

    #[test]
    fn test_filter_chezmoi_status_compacts_output() {
        let raw = repeated_lines("MM .config/app/config.toml\n", 80);
        let filtered = filter_chezmoi_status(&raw);
        assert!(filtered.contains("80 changes"));
        assert!(savings(&raw, &filtered) >= 60.0);
    }

    #[test]
    fn test_filter_chezmoi_managed_compacts_output() {
        let raw = repeated_lines("/home/me/.config/app/config.toml\n", 80);
        let filtered = filter_path_list(&raw, "managed");
        assert!(filtered.contains("80 managed paths"));
        assert!(savings(&raw, &filtered) >= 60.0);
    }

    #[test]
    fn test_filter_chezmoi_add_compacts_output() {
        let raw = repeated_lines("added /home/me/.config/app/config.toml\n", 80);
        let filtered = filter_chezmoi_add(&raw);
        assert!(filtered.contains("add: 80 line(s)"));
        assert!(savings(&raw, &filtered) >= 60.0);
    }

    #[test]
    fn test_filter_chezmoi_update_compacts_output() {
        let raw = repeated_lines("updated /home/me/.config/app/config.toml\n", 80);
        let filtered = filter_chezmoi_update(&raw);
        assert!(filtered.contains("update: 80 line(s)"));
        assert!(savings(&raw, &filtered) >= 60.0);
    }

    #[test]
    fn test_filter_chezmoi_unmanaged_compacts_output() {
        let raw = repeated_lines("/home/me/.cache/app/generated-file.toml\n", 80);
        let filtered = filter_path_list(&raw, "unmanaged");
        assert!(filtered.contains("80 unmanaged paths"));
        assert!(savings(&raw, &filtered) >= 60.0);
    }

    #[test]
    fn test_filter_chezmoi_doctor_compacts_output() {
        let mut raw = repeated_lines("ok check passed\n", 80);
        raw.push_str("warning config file has weak permissions\n");
        raw.push_str("error source path is not a git repository\n");
        let filtered = filter_chezmoi_doctor(&raw);
        assert!(filtered.contains("doctor: 2 issue(s)"));
        assert!(savings(&raw, &filtered) >= 60.0);
    }

    #[test]
    fn test_split_git_invocation_preserves_global_args() {
        let args = vec![
            "--no-pager".to_string(),
            "-c".to_string(),
            "color.ui=false".to_string(),
            "status".to_string(),
            "-sb".to_string(),
        ];
        let (global, subcommand, rest) = split_git_invocation(&args).unwrap();
        assert_eq!(global, &args[..3]);
        assert_eq!(subcommand, "status");
        assert_eq!(rest, &args[4..]);
    }

    fn repeated_lines(line: &str, count: usize) -> String {
        let mut out = String::new();
        for _ in 0..count {
            out.push_str(line);
        }
        out
    }
}
