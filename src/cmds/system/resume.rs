//! Compact, per-repository execution context for resuming agent work.
//!
//! Context is stored outside the repository in RTK's normal local data directory.
//! Reading is side-effect free; callers must opt in to writes with `--save`.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::core::constants::RTK_DATA_DIR;

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
struct SavedContext {
    active_plan: Option<String>,
    completed_steps: Vec<String>,
    blockers: Vec<String>,
    last_reviewed_commit: Option<String>,
    next_action: Option<String>,
}

#[derive(Debug, Serialize)]
struct ResumeContext {
    worktree: String,
    branch: String,
    base: Option<String>,
    clean: bool,
    head: String,
    #[serde(flatten)]
    saved: SavedContext,
}

pub fn run(
    save: bool,
    plan: Option<String>,
    completed_steps: Vec<String>,
    blockers: Vec<String>,
    last_reviewed: Option<String>,
    next: Option<String>,
    format: &str,
) -> Result<()> {
    if format != "text" && format != "json" {
        bail!("Unsupported format '{format}'. Use text or json.");
    }
    if (!completed_steps.is_empty()
        || !blockers.is_empty()
        || plan.is_some()
        || last_reviewed.is_some()
        || next.is_some())
        && !save
    {
        bail!("Context fields require --save; read-only resume never changes state.");
    }

    let repo = git(&["rev-parse", "--show-toplevel"])?;
    let path = context_path(Path::new(&repo))?;
    let mut saved = load_context(&path)?;
    if save {
        if let Some(value) = plan {
            saved.active_plan = Some(value);
        }
        if !completed_steps.is_empty() {
            saved.completed_steps = completed_steps;
        }
        if !blockers.is_empty() {
            saved.blockers = blockers;
        }
        if let Some(value) = last_reviewed {
            saved.last_reviewed_commit = Some(value);
        }
        if let Some(value) = next {
            saved.next_action = Some(value);
        }
        save_context(&path, &saved)?;
    }

    let state = ResumeContext {
        worktree: repo,
        branch: git(&["branch", "--show-current"])? ,
        base: merge_base().ok(),
        clean: git(&["status", "--porcelain"])?.is_empty(),
        head: git(&["rev-parse", "HEAD"])? ,
        saved,
    };
    print_context(&state, format)
}

fn git(args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .context("Failed to run git")?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn merge_base() -> Result<String> {
    for candidate in ["@{upstream}", "origin/HEAD", "main", "master"] {
        if let Ok(base) = git(&["merge-base", "HEAD", candidate]) {
            return Ok(base);
        }
    }
    bail!("No upstream or conventional base branch found")
}

fn context_path(repo: &Path) -> Result<PathBuf> {
    let canonical = repo.canonicalize().context("Cannot canonicalize repository path")?;
    let hash = format!("{:x}", Sha256::digest(canonical.to_string_lossy().as_bytes()));
    let data = dirs::data_local_dir().context("Cannot determine local data directory")?;
    Ok(data.join(RTK_DATA_DIR).join("resume").join(format!("{hash}.json")))
}

fn load_context(path: &Path) -> Result<SavedContext> {
    if !path.exists() {
        return Ok(SavedContext::default());
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    serde_json::from_str(&content).with_context(|| format!("Failed to parse {}", path.display()))
}

fn save_context(path: &Path, context: &SavedContext) -> Result<()> {
    let parent = path.parent().context("Resume context path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("Failed to create {}", parent.display()))?;
    let content = serde_json::to_string_pretty(context).context("Failed to serialize resume context")?;
    fs::write(path, content + "\n")
        .with_context(|| format!("Failed to write {}", path.display()))
}

fn print_context(context: &ResumeContext, format: &str) -> Result<()> {
    if format == "json" {
        println!("{}", serde_json::to_string(context)?);
        return Ok(());
    }
    println!(
        "repo={} branch={} base={} clean={} head={}",
        context.worktree,
        context.branch,
        context.base.as_deref().unwrap_or("unknown"),
        context.clean,
        context.head
    );
    println!("plan={}", context.saved.active_plan.as_deref().unwrap_or("unset"));
    println!("completed={}", context.saved.completed_steps.join(" | "));
    println!("blockers={}", context.saved.blockers.join(" | "));
    println!("last_reviewed={}", context.saved.last_reviewed_commit.as_deref().unwrap_or("unset"));
    println!("next={}", context.saved.next_action.as_deref().unwrap_or("unset"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_path_is_stable_and_does_not_live_in_the_repository() {
        let repo = std::env::current_dir().unwrap();
        let first = context_path(&repo).unwrap();
        let second = context_path(&repo).unwrap();
        assert_eq!(first, second);
        assert!(!first.starts_with(repo));
    }

    #[test]
    fn saved_context_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("context.json");
        let expected = SavedContext {
            active_plan: Some("docs/plans/a.md".into()),
            completed_steps: vec!["tests".into()],
            blockers: vec!["review".into()],
            last_reviewed_commit: Some("abc123".into()),
            next_action: Some("open PR".into()),
        };
        save_context(&path, &expected).unwrap();
        assert_eq!(load_context(&path).unwrap(), expected);
    }

    #[test]
    fn context_fields_require_explicit_save() {
        let error = run(
            false,
            Some("docs/plans/a.md".into()),
            Vec::new(),
            Vec::new(),
            None,
            None,
            "text",
        )
        .unwrap_err();
        assert!(error.to_string().contains("require --save"));
    }
}
