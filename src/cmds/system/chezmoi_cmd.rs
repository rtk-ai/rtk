use crate::core::tracking;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::process::Command;

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

pub fn run(cmd: ChezmoiCommand, args: &[String], verbose: u8) -> Result<()> {
    match cmd {
        ChezmoiCommand::Diff => run_diff(args, verbose),
        ChezmoiCommand::Apply => run_apply(args, verbose),
        ChezmoiCommand::Status => run_status(args),
        ChezmoiCommand::Managed => run_managed(args),
        ChezmoiCommand::Add => run_add_or_readd("add", args, verbose),
        ChezmoiCommand::ReAdd => run_add_or_readd("re-add", args, verbose),
        ChezmoiCommand::Update => run_update(args, verbose),
        ChezmoiCommand::Unmanaged => run_unmanaged(args),
        ChezmoiCommand::Doctor => run_doctor(args),
    }
}

pub fn run_passthrough(args: &[OsString], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let _ = verbose;

    let status = Command::new("chezmoi")
        .args(args)
        .status()
        .context("Failed to run chezmoi")?;

    let args_str = tracking::args_display(args);
    timer.track_passthrough(
        &format!("chezmoi {}", args_str),
        &format!("rtk chezmoi {} (passthrough)", args_str),
    );

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

fn run_diff(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("chezmoi");
    cmd.arg("diff");
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run chezmoi diff")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.is_empty() {
            eprint!("{}", stderr);
        }
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    if stdout.trim().is_empty() {
        let msg = "chezmoi: up to date";
        println!("{}", msg);
        timer.track("chezmoi diff", "rtk chezmoi diff", &stdout, msg);
        return Ok(());
    }

    let filtered = filter_chezmoi_diff(&stdout, verbose);
    println!("{}", filtered);

    let raw_cmd = format!("chezmoi diff {}", args.join(" "));
    let rtk_cmd = format!("rtk chezmoi diff {}", args.join(" "));
    timer.track(raw_cmd.trim_end(), rtk_cmd.trim_end(), &stdout, &filtered);

    Ok(())
}

fn run_apply(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("chezmoi");
    cmd.arg("apply");
    // Always pass -v to capture what was applied
    if !args.iter().any(|a| a == "-v" || a == "--verbose") {
        cmd.arg("-v");
    }
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run chezmoi apply")?;

    // chezmoi apply prints applied files to stderr with -v
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        if !stderr.is_empty() {
            eprint!("{}", stderr);
        }
        if !stdout.is_empty() {
            print!("{}", stdout);
        }
        std::process::exit(output.status.code().unwrap_or(1));
    }

    // Combine stdout+stderr for filtering (chezmoi uses stderr for verbose output)
    let combined = format!("{}{}", stdout, stderr);
    let filtered = filter_chezmoi_apply(&combined, verbose);
    println!("{}", filtered);

    let raw_cmd = format!("chezmoi apply {}", args.join(" "));
    let rtk_cmd = format!("rtk chezmoi apply {}", args.join(" "));
    timer.track(raw_cmd.trim_end(), rtk_cmd.trim_end(), &combined, &filtered);

    Ok(())
}

fn run_status(args: &[String]) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("chezmoi");
    cmd.arg("status");
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run chezmoi status")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.is_empty() {
            eprint!("{}", stderr);
        }
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    if stdout.trim().is_empty() {
        let msg = "chezmoi: up to date";
        println!("{}", msg);
        timer.track("chezmoi status", "rtk chezmoi status", &stdout, msg);
        return Ok(());
    }

    let filtered = filter_chezmoi_status(&stdout);
    println!("{}", filtered);
    timer.track("chezmoi status", "rtk chezmoi status", &stdout, &filtered);

    Ok(())
}

fn run_managed(args: &[String]) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("chezmoi");
    cmd.arg("managed");
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run chezmoi managed")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.is_empty() {
            eprint!("{}", stderr);
        }
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let filtered = filter_chezmoi_managed(&stdout);
    println!("{}", filtered);
    timer.track("chezmoi managed", "rtk chezmoi managed", &stdout, &filtered);

    Ok(())
}

fn run_add_or_readd(subcmd: &str, args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("chezmoi");
    cmd.arg(subcmd);
    // Always pass -v to capture what was processed
    if !args.iter().any(|a| a == "-v" || a == "--verbose") {
        cmd.arg("-v");
    }
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run chezmoi {}", subcmd))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        if !stderr.is_empty() {
            eprint!("{}", stderr);
        }
        if !stdout.is_empty() {
            print!("{}", stdout);
        }
        std::process::exit(output.status.code().unwrap_or(1));
    }

    // chezmoi prints processed files to stderr with -v
    let combined = format!("{}{}", stdout, stderr);
    let action = if subcmd == "re-add" {
        "re-added"
    } else {
        "added"
    };
    let filtered = filter_chezmoi_add(&combined, action, verbose);
    println!("{}", filtered);

    let raw_cmd = format!("chezmoi {} {}", subcmd, args.join(" "));
    let rtk_cmd = format!("rtk chezmoi {} {}", subcmd, args.join(" "));
    timer.track(raw_cmd.trim_end(), rtk_cmd.trim_end(), &combined, &filtered);

    Ok(())
}

fn run_update(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("chezmoi");
    cmd.arg("update");
    // -v lets us see which files were applied
    if !args.iter().any(|a| a == "-v" || a == "--verbose") {
        cmd.arg("-v");
    }
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run chezmoi update")?;

    // chezmoi update writes git output to stderr and apply output to stderr too
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        if !stderr.is_empty() {
            eprint!("{}", stderr);
        }
        if !stdout.is_empty() {
            print!("{}", stdout);
        }
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let combined = format!("{}{}", stdout, stderr);
    let filtered = filter_chezmoi_update(&combined, verbose);
    println!("{}", filtered);

    let raw_cmd = format!("chezmoi update {}", args.join(" "));
    let rtk_cmd = format!("rtk chezmoi update {}", args.join(" "));
    timer.track(raw_cmd.trim_end(), rtk_cmd.trim_end(), &combined, &filtered);

    Ok(())
}

fn run_unmanaged(args: &[String]) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("chezmoi");
    cmd.arg("unmanaged");
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run chezmoi unmanaged")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.is_empty() {
            eprint!("{}", stderr);
        }
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let filtered = filter_chezmoi_unmanaged(&stdout);
    println!("{}", filtered);
    timer.track(
        "chezmoi unmanaged",
        "rtk chezmoi unmanaged",
        &stdout,
        &filtered,
    );

    Ok(())
}

fn run_doctor(args: &[String]) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("chezmoi");
    cmd.arg("doctor");
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run chezmoi doctor")?;

    // doctor exits non-zero when warnings/errors are found — still show filtered output
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);

    let filtered = filter_chezmoi_doctor(&combined);
    println!("{}", filtered);

    timer.track("chezmoi doctor", "rtk chezmoi doctor", &combined, &filtered);

    // Preserve exit code so callers can detect issues
    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

// --- Filters ---

lazy_static! {
    static ref DIFF_FILE_RE: Regex = Regex::new(r"^diff --git a/(.+) b/.+$").unwrap();
    static ref DIFF_HUNK_RE: Regex = Regex::new(r"^@@ -\d+(?:,\d+)? \+\d+(?:,\d+)? @@").unwrap();
    // "1 file changed, 2 insertions(+), 3 deletions(-)" or "2 files changed, ..."
    static ref GIT_STAT_RE: Regex =
        Regex::new(r"(\d+) files? changed").unwrap();
    // "install /path/to/file" or "update /path/to/file" from chezmoi -v apply output
    static ref APPLY_LINE_RE: Regex =
        Regex::new(r"^(?:install|update|remove|mkdir|chmod|run)\s+").unwrap();
    // chezmoi doctor line: "ok    check (detail)" / "warning    ..." / "error    ..."
    static ref DOCTOR_LINE_RE: Regex =
        Regex::new(r"^(ok|warning|error)\s+(.+)$").unwrap();
}

struct FileSummary {
    path: String,
    added: usize,
    removed: usize,
    is_new: bool,
    is_deleted: bool,
}

pub fn filter_chezmoi_diff(output: &str, _verbose: u8) -> String {
    let mut files: Vec<FileSummary> = Vec::new();
    let mut current: Option<FileSummary> = None;
    let mut in_hunk = false;

    for line in output.lines() {
        if let Some(cap) = DIFF_FILE_RE.captures(line) {
            if let Some(prev) = current.take() {
                files.push(prev);
            }
            current = Some(FileSummary {
                path: cap[1].to_string(),
                added: 0,
                removed: 0,
                is_new: false,
                is_deleted: false,
            });
            in_hunk = false;
        } else if line.starts_with("new file") {
            if let Some(ref mut f) = current {
                f.is_new = true;
            }
        } else if line.starts_with("deleted file") {
            if let Some(ref mut f) = current {
                f.is_deleted = true;
            }
        } else if DIFF_HUNK_RE.is_match(line) {
            in_hunk = true;
        } else if in_hunk {
            if line.starts_with('+') && !line.starts_with("+++") {
                if let Some(ref mut f) = current {
                    f.added += 1;
                }
            } else if line.starts_with('-') && !line.starts_with("---") {
                if let Some(ref mut f) = current {
                    f.removed += 1;
                }
            }
        }
    }

    if let Some(prev) = current.take() {
        files.push(prev);
    }

    if files.is_empty() {
        return "chezmoi: up to date".to_string();
    }

    let mut out = format!(
        "chezmoi diff: {} file{}\n",
        files.len(),
        if files.len() == 1 { "" } else { "s" }
    );

    for f in &files {
        let status = if f.is_new {
            "A"
        } else if f.is_deleted {
            "D"
        } else {
            "M"
        };
        let stats = if f.is_new {
            format!("+{}", f.added)
        } else if f.is_deleted {
            format!("-{}", f.removed)
        } else {
            format!("+{}/-{}", f.added, f.removed)
        };
        out.push_str(&format!("  {} {:<50} {}\n", status, f.path, stats));
    }

    out.trim_end().to_string()
}

pub fn filter_chezmoi_apply(output: &str, verbose: u8) -> String {
    let lines: Vec<&str> = output
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();

    if lines.is_empty() {
        return "ok ✓ (no changes)".to_string();
    }

    if verbose > 0 {
        let mut out = format!(
            "ok ✓ {} file{} applied\n",
            lines.len(),
            if lines.len() == 1 { "" } else { "s" }
        );
        for line in &lines {
            out.push_str(&format!("  {}\n", line));
        }
        return out.trim_end().to_string();
    }

    format!(
        "ok ✓ {} file{} applied",
        lines.len(),
        if lines.len() == 1 { "" } else { "s" }
    )
}

pub fn filter_chezmoi_add(output: &str, action: &str, verbose: u8) -> String {
    let lines: Vec<&str> = output
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();

    if lines.is_empty() {
        return format!("ok ✓ (nothing to {})", action);
    }

    if verbose > 0 {
        let mut out = format!(
            "ok ✓ {} file{} {}\n",
            lines.len(),
            if lines.len() == 1 { "" } else { "s" },
            action
        );
        for line in &lines {
            out.push_str(&format!("  {}\n", line));
        }
        return out.trim_end().to_string();
    }

    format!(
        "ok ✓ {} file{} {}",
        lines.len(),
        if lines.len() == 1 { "" } else { "s" },
        action
    )
}

pub fn filter_chezmoi_update(output: &str, verbose: u8) -> String {
    // Fast path: nothing was pulled
    if output.lines().any(|l| l.trim() == "Already up to date.") {
        return "ok ✓ already up to date".to_string();
    }

    // Count files changed reported by git (stat summary line)
    let git_files: usize = GIT_STAT_RE
        .captures_iter(output)
        .filter_map(|c| c[1].parse::<usize>().ok())
        .sum();

    // Count apply lines (files chezmoi actually wrote)
    let applied: Vec<&str> = output
        .lines()
        .filter(|l| APPLY_LINE_RE.is_match(l.trim()))
        .collect();

    let mut summary = match git_files {
        0 => "ok ✓ updated".to_string(),
        n => format!(
            "ok ✓ updated ({} source file{} changed)",
            n,
            if n == 1 { "" } else { "s" }
        ),
    };

    if !applied.is_empty() {
        summary.push_str(&format!(
            ", {} dotfile{} applied",
            applied.len(),
            if applied.len() == 1 { "" } else { "s" }
        ));
    }

    if verbose > 0 && !applied.is_empty() {
        summary.push('\n');
        for line in &applied {
            summary.push_str(&format!("  {}\n", line.trim()));
        }
        return summary.trim_end().to_string();
    }

    summary
}

pub fn filter_chezmoi_unmanaged(output: &str) -> String {
    let files: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();

    if files.is_empty() {
        return "chezmoi unmanaged: none".to_string();
    }

    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    for file in &files {
        let dir = if let Some(idx) = file.find('/') {
            file[..idx].to_string()
        } else {
            ".".to_string()
        };
        *groups.entry(dir).or_insert(0) += 1;
    }

    let mut out = format!(
        "chezmoi unmanaged: {} file{}\n",
        files.len(),
        if files.len() == 1 { "" } else { "s" }
    );
    for (dir, count) in &groups {
        let label = if dir == "." {
            format!("  .  ({})", count)
        } else {
            format!("  {}/  ({})", dir, count)
        };
        out.push_str(&label);
        out.push('\n');
    }

    out.trim_end().to_string()
}

pub fn filter_chezmoi_doctor(output: &str) -> String {
    let mut warnings: Vec<&str> = Vec::new();
    let mut errors: Vec<&str> = Vec::new();
    let mut ok_count = 0usize;

    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(cap) = DOCTOR_LINE_RE.captures(trimmed) {
            match &cap[1] {
                "ok" => ok_count += 1,
                "warning" => warnings.push(line),
                "error" => errors.push(line),
                _ => {}
            }
        }
    }

    // All clear
    if warnings.is_empty() && errors.is_empty() {
        return format!("chezmoi doctor: ok ({} checks passed)", ok_count);
    }

    let mut out = format!(
        "chezmoi doctor: {} ok, {} warning{}, {} error{}\n",
        ok_count,
        warnings.len(),
        if warnings.len() == 1 { "" } else { "s" },
        errors.len(),
        if errors.len() == 1 { "" } else { "s" },
    );
    for line in &errors {
        out.push_str(line.trim());
        out.push('\n');
    }
    for line in &warnings {
        out.push_str(line.trim());
        out.push('\n');
    }

    out.trim_end().to_string()
}

pub fn filter_chezmoi_status(output: &str) -> String {
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();

    if lines.is_empty() {
        return "chezmoi: up to date".to_string();
    }

    let mut added = 0usize;
    let mut modified = 0usize;
    let mut deleted = 0usize;
    let mut other = 0usize;
    let mut file_lines: Vec<String> = Vec::new();

    for line in &lines {
        let mut chars = line.chars();
        let code = chars.next().unwrap_or(' ');
        let path = if line.len() >= 3 {
            line[2..].trim()
        } else {
            line.trim()
        };

        match code {
            'A' => {
                added += 1;
                file_lines.push(format!("  A {}", path));
            }
            'M' => {
                modified += 1;
                file_lines.push(format!("  M {}", path));
            }
            'D' => {
                deleted += 1;
                file_lines.push(format!("  D {}", path));
            }
            _ => {
                other += 1;
                file_lines.push(format!("  {} {}", code, path));
            }
        }
    }

    let mut parts: Vec<String> = Vec::new();
    if added > 0 {
        parts.push(format!("{} added", added));
    }
    if modified > 0 {
        parts.push(format!("{} modified", modified));
    }
    if deleted > 0 {
        parts.push(format!("{} deleted", deleted));
    }
    if other > 0 {
        parts.push(format!("{} other", other));
    }

    let mut out = format!("chezmoi status: {}\n", parts.join(", "));
    for fl in &file_lines {
        out.push_str(fl);
        out.push('\n');
    }

    out.trim_end().to_string()
}

pub fn filter_chezmoi_managed(output: &str) -> String {
    let files: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();

    if files.is_empty() {
        return "chezmoi: no managed files".to_string();
    }

    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    for file in &files {
        let dir = if let Some(idx) = file.find('/') {
            file[..idx].to_string()
        } else {
            ".".to_string()
        };
        *groups.entry(dir).or_insert(0) += 1;
    }

    let mut out = format!("chezmoi managed: {} files\n", files.len());
    for (dir, count) in &groups {
        let label = if dir == "." {
            format!("  .  ({})", count)
        } else {
            format!("  {}/  ({})", dir, count)
        };
        out.push_str(&label);
        out.push('\n');
    }

    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_filter_diff_empty() {
        assert_eq!(filter_chezmoi_diff("", 0), "chezmoi: up to date");
    }

    #[test]
    fn test_filter_diff_modified_file() {
        let input = "diff --git a/.zshrc b/.zshrc\nindex abc..def 100644\n--- a/.zshrc\n+++ b/.zshrc\n@@ -10,6 +10,7 @@ export PATH\n alias gs=\"git status\"\n+alias gd=\"git diff\"\n alias gc=\"git commit\"\n";
        let output = filter_chezmoi_diff(input, 0);
        assert!(output.contains(".zshrc"), "should show file path");
        assert!(output.contains("M"), "should show M for modified");
        assert!(output.contains("+1"), "should show added lines");
        assert!(count_tokens(&output) < count_tokens(input));
    }

    #[test]
    fn test_filter_diff_new_file() {
        let input = "diff --git a/.config/nvim/new.lua b/.config/nvim/new.lua\nnew file mode 100644\nindex 0000000..abc1234\n--- /dev/null\n+++ b/.config/nvim/new.lua\n@@ -0,0 +1,4 @@\n+-- New config\n+local M = {}\n+M.setup = function() end\n+return M\n";
        let output = filter_chezmoi_diff(input, 0);
        assert!(output.contains("A"), "should show A for new file");
        assert!(output.contains(".config/nvim/new.lua"));
    }

    #[test]
    fn test_filter_apply_empty() {
        assert_eq!(filter_chezmoi_apply("", 0), "ok ✓ (no changes)");
    }

    #[test]
    fn test_filter_apply_files() {
        let input = "install /home/user/.zshrc\ninstall /home/user/.config/nvim/init.lua\n";
        let output = filter_chezmoi_apply(input, 0);
        assert!(output.contains("2 files applied"));
    }

    #[test]
    fn test_filter_apply_verbose() {
        let input = "install /home/user/.zshrc\n";
        let output = filter_chezmoi_apply(input, 1);
        assert!(output.contains("install /home/user/.zshrc"));
    }

    #[test]
    fn test_filter_status_empty() {
        assert_eq!(filter_chezmoi_status(""), "chezmoi: up to date");
    }

    #[test]
    fn test_filter_status_changes() {
        let input = "M  .zshrc\nA  .config/nvim/init.lua\n";
        let output = filter_chezmoi_status(input);
        assert!(output.contains("1 added"));
        assert!(output.contains("1 modified"));
        assert!(output.contains(".zshrc"));
        assert!(output.contains(".config/nvim/init.lua"));
    }

    #[test]
    fn test_filter_managed_empty() {
        assert_eq!(filter_chezmoi_managed(""), "chezmoi: no managed files");
    }

    #[test]
    fn test_filter_managed_groups_by_dir() {
        let input = ".zshrc\n.bashrc\n.config/nvim/init.lua\n.config/starship.toml\n";
        let output = filter_chezmoi_managed(input);
        assert!(output.contains("4 files"));
        assert!(output.contains(".config/"));
    }

    #[test]
    fn test_filter_unmanaged_empty() {
        assert_eq!(filter_chezmoi_unmanaged(""), "chezmoi unmanaged: none");
    }

    #[test]
    fn test_filter_unmanaged_groups() {
        let input = "Downloads/file1.txt\nDownloads/file2.txt\nDesktop/readme.txt\n";
        let output = filter_chezmoi_unmanaged(input);
        assert!(output.contains("3 files"));
        assert!(output.contains("Downloads/"));
    }

    #[test]
    fn test_filter_doctor_all_ok() {
        let input = "ok    chezmoi binary found\nok    git binary found\nok    source directory exists\n";
        let output = filter_chezmoi_doctor(input);
        assert!(output.contains("ok (3 checks passed)"));
    }

    #[test]
    fn test_filter_doctor_with_warnings() {
        let input = "ok    chezmoi binary found\nwarning    chezmoi version outdated\nok    git binary found\n";
        let output = filter_chezmoi_doctor(input);
        assert!(output.contains("1 warning"));
        assert!(output.contains("chezmoi version outdated"));
    }

    #[test]
    fn test_filter_update_already_up_to_date() {
        let input = "Already up to date.\n";
        assert_eq!(filter_chezmoi_update(input, 0), "ok ✓ already up to date");
    }

    #[test]
    fn test_filter_update_with_changes() {
        let input = "From github.com:user/dotfiles\n   abc123..def456  main -> origin/main\n1 file changed, 5 insertions(+)\ninstall /home/user/.zshrc\n";
        let output = filter_chezmoi_update(input, 0);
        assert!(output.contains("ok ✓ updated"));
        assert!(output.contains("1 source file"));
        assert!(output.contains("1 dotfile"));
    }

    #[test]
    fn test_filter_add_empty() {
        assert_eq!(
            filter_chezmoi_add("", "added", 0),
            "ok ✓ (nothing to added)"
        );
    }

    #[test]
    fn test_filter_add_files() {
        let input = "add /home/user/.zshrc\n";
        let output = filter_chezmoi_add(input, "added", 0);
        assert!(output.contains("1 file added"));
    }

    #[test]
    fn test_token_savings_diff() {
        let input = "diff --git a/.zshrc b/.zshrc\nindex abc..def 100644\n--- a/.zshrc\n+++ b/.zshrc\n@@ -10,6 +10,7 @@ export PATH=\"$HOME/bin:$PATH\"\n alias gs='git status'\n alias gl='git log'\n+alias gd='git diff'\n alias gc='git commit'\n alias gp='git push'\n \n";
        let output = filter_chezmoi_diff(input, 0);
        let savings = 100.0
            - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "chezmoi diff filter: expected >= 60% savings, got {:.1}%",
            savings
        );
    }
}
