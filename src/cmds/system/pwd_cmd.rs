//! Reports execution context: physical cwd, git root/worktree linkage, branch.
//!
//! Agent harnesses (Claude Code, Cursor, ...) track their shell cwd
//! out-of-band and can silently drift to the wrong checkout when git
//! worktrees are involved (see issue #2148: commands believed to run in a
//! linked worktree actually ran in the main checkout). `rtk pwd` prints the
//! physical cwd plus the git context in one compact block so an agent (or a
//! hook) can validate where commands will actually execute before writing.
//!
//! Output examples:
//! ```text
//! /repo/.worktrees/feature
//! branch: feature
//! worktree of: /repo (branch develop)
//! ```
//!
//! A shell's `pwd` builtin prints the logical, symlink-preserving `$PWD`;
//! child processes only ever see the physical `getcwd()`. When the two
//! disagree, a `note:`/`warn:` line makes the discrepancy explicit instead
//! of leaving the agent to misdiagnose it as misrouting.

use crate::core::tracking;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub fn run(verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let physical = std::env::current_dir().context("Failed to read current directory")?;
    if verbose > 0 {
        eprintln!("Resolving git context for {}", physical.display());
    }

    let logical = std::env::var_os("PWD").map(PathBuf::from);
    let output = describe(&physical, logical.as_deref());
    print!("{}", output);

    timer.track("pwd", "rtk pwd", &output, &output);
    Ok(())
}

/// What the enclosing `.git` entry says about this directory.
enum GitContext {
    /// `.git` is a directory — a main checkout.
    Main { root: PathBuf, branch: String },
    /// `.git` is a file pointing into `<main>/.git/worktrees/<name>` — a linked worktree.
    Worktree {
        root: PathBuf,
        branch: String,
        main_root: Option<PathBuf>,
        main_branch: Option<String>,
    },
}

fn describe(physical: &Path, logical: Option<&Path>) -> String {
    let mut out = format!("{}\n", physical.display());

    match find_git_context(physical) {
        Some(GitContext::Main { root, branch }) => {
            out.push_str(&format!("branch: {}\n", branch));
            if root != physical {
                out.push_str(&format!("repo root: {}\n", root.display()));
            }
        }
        Some(GitContext::Worktree {
            root,
            branch,
            main_root,
            main_branch,
        }) => {
            out.push_str(&format!("branch: {}\n", branch));
            if root != physical {
                out.push_str(&format!("worktree root: {}\n", root.display()));
            }
            match (main_root, main_branch) {
                (Some(main), Some(b)) => out.push_str(&format!(
                    "worktree of: {} (branch {})\n",
                    main.display(),
                    b
                )),
                (Some(main), None) => {
                    out.push_str(&format!("worktree of: {}\n", main.display()))
                }
                _ => out.push_str("worktree of: (unresolved gitdir pointer)\n"),
            }
        }
        None => out.push_str("(not in a git repository)\n"),
    }

    if let Some(note) = logical.and_then(|l| pwd_mismatch_note(physical, l)) {
        out.push_str(&note);
        out.push('\n');
    }
    out
}

/// Note when the shell's logical `$PWD` disagrees with the physical cwd.
///
/// Resolving to the same directory (symlinked path) is benign but worth
/// surfacing; resolving elsewhere means the shell and this process genuinely
/// disagree about where they are.
fn pwd_mismatch_note(physical: &Path, logical: &Path) -> Option<String> {
    if logical == physical {
        return None;
    }
    match fs::canonicalize(logical) {
        Ok(resolved) if resolved == physical => Some(format!(
            "note: shell $PWD is a symlinked path to the same directory: {}",
            logical.display()
        )),
        _ => Some(format!(
            "warn: shell $PWD differs from physical cwd: {}",
            logical.display()
        )),
    }
}

/// Walk up from `start` until a `.git` entry is found.
fn find_git_context(start: &Path) -> Option<GitContext> {
    let mut dir = start;
    loop {
        let dot_git = dir.join(".git");
        if dot_git.is_dir() {
            return Some(GitContext::Main {
                root: dir.to_path_buf(),
                branch: read_branch(&dot_git),
            });
        }
        if dot_git.is_file() {
            return Some(worktree_context(dir, &dot_git));
        }
        dir = dir.parent()?;
    }
}

fn worktree_context(root: &Path, dot_git_file: &Path) -> GitContext {
    let (branch, main_root, main_branch) = match read_gitdir_pointer(dot_git_file, root) {
        Some(gitdir) => {
            let common = common_git_dir(&gitdir);
            let main_root = common
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf);
            let main_branch = common.as_deref().map(read_branch);
            (read_branch(&gitdir), main_root, main_branch)
        }
        None => ("(unknown)".to_string(), None, None),
    };
    GitContext::Worktree {
        root: root.to_path_buf(),
        branch,
        main_root,
        main_branch,
    }
}

/// A worktree's `.git` file contains `gitdir: <path/to/main/.git/worktrees/name>`.
fn read_gitdir_pointer(dot_git_file: &Path, base: &Path) -> Option<PathBuf> {
    let content = fs::read_to_string(dot_git_file).ok()?;
    let raw = content.strip_prefix("gitdir:")?.trim();
    if raw.is_empty() {
        return None;
    }
    let path = PathBuf::from(raw);
    let abs = if path.is_absolute() {
        path
    } else {
        base.join(path)
    };
    Some(fs::canonicalize(&abs).unwrap_or(abs))
}

/// The shared `.git` dir of the main checkout: `<gitdir>/commondir` points at
/// it (usually `../..`); fall back to stripping `worktrees/<name>` from the path.
fn common_git_dir(gitdir: &Path) -> Option<PathBuf> {
    if let Ok(raw) = fs::read_to_string(gitdir.join("commondir")) {
        let p = PathBuf::from(raw.trim());
        let abs = if p.is_absolute() { p } else { gitdir.join(p) };
        return Some(fs::canonicalize(&abs).unwrap_or(abs));
    }
    let parent = gitdir.parent()?;
    if parent.file_name()? == "worktrees" {
        parent.parent().map(Path::to_path_buf)
    } else {
        None
    }
}

/// Branch name from `<gitdir>/HEAD`, or a short hash for detached HEAD.
fn read_branch(gitdir: &Path) -> String {
    match fs::read_to_string(gitdir.join("HEAD")) {
        Ok(head) => {
            let head = head.trim();
            match head.strip_prefix("ref: refs/heads/") {
                Some(branch) => branch.to_string(),
                None => format!("detached @ {}", head.chars().take(9).collect::<String>()),
            }
        }
        Err(_) => "(unknown)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Canonicalized tempdir (macOS `/var` is a symlink to `/private/var`).
    fn tmpdir() -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("create tempdir");
        let path = fs::canonicalize(dir.path()).expect("canonicalize tempdir");
        (dir, path)
    }

    /// Build a fake main checkout: `<root>/.git/` with HEAD on `branch`.
    fn make_main_checkout(root: &Path, branch: &str) {
        let git = root.join(".git");
        fs::create_dir_all(&git).expect("create .git");
        fs::write(git.join("HEAD"), format!("ref: refs/heads/{}\n", branch)).expect("write HEAD");
    }

    /// Build a fake linked worktree of `main_root` at `wt_root` on `branch`.
    fn make_worktree(main_root: &Path, wt_root: &Path, name: &str, branch: &str) {
        let wt_gitdir = main_root.join(".git").join("worktrees").join(name);
        fs::create_dir_all(&wt_gitdir).expect("create worktree gitdir");
        fs::write(
            wt_gitdir.join("HEAD"),
            format!("ref: refs/heads/{}\n", branch),
        )
        .expect("write worktree HEAD");
        fs::write(wt_gitdir.join("commondir"), "../..\n").expect("write commondir");

        fs::create_dir_all(wt_root).expect("create worktree root");
        fs::write(
            wt_root.join(".git"),
            format!("gitdir: {}\n", wt_gitdir.display()),
        )
        .expect("write .git pointer file");
    }

    #[test]
    fn test_main_checkout() {
        let (_guard, tmp) = tmpdir();
        let root = tmp.join("repo");
        fs::create_dir_all(&root).expect("mkdir repo");
        make_main_checkout(&root, "develop");

        let out = describe(&root, None);
        assert_eq!(
            out,
            format!("{}\nbranch: develop\n", root.display()),
            "main checkout at root: cwd + branch only"
        );
    }

    #[test]
    fn test_subdirectory_shows_repo_root() {
        let (_guard, tmp) = tmpdir();
        let root = tmp.join("repo");
        let sub = root.join("src").join("cmds");
        fs::create_dir_all(&sub).expect("mkdir subdir");
        make_main_checkout(&root, "main");

        let out = describe(&sub, None);
        assert!(out.contains("branch: main\n"));
        assert!(
            out.contains(&format!("repo root: {}\n", root.display())),
            "subdir must surface the repo root: {}",
            out
        );
    }

    #[test]
    fn test_linked_worktree() {
        let (_guard, tmp) = tmpdir();
        let main = tmp.join("main-checkout");
        let wt = tmp.join("wt");
        fs::create_dir_all(&main).expect("mkdir main");
        make_main_checkout(&main, "develop");
        make_worktree(&main, &wt, "wt", "feature");

        let out = describe(&wt, None);
        assert!(out.contains("branch: feature\n"), "worktree branch: {}", out);
        assert!(
            out.contains(&format!("worktree of: {} (branch develop)\n", main.display())),
            "worktree must link back to main checkout + its branch: {}",
            out
        );
    }

    #[test]
    fn test_worktree_without_commondir_falls_back() {
        let (_guard, tmp) = tmpdir();
        let main = tmp.join("main-checkout");
        let wt = tmp.join("wt");
        fs::create_dir_all(&main).expect("mkdir main");
        make_main_checkout(&main, "develop");
        make_worktree(&main, &wt, "wt", "feature");
        fs::remove_file(main.join(".git/worktrees/wt/commondir")).expect("drop commondir");

        let out = describe(&wt, None);
        assert!(
            out.contains(&format!("worktree of: {}", main.display())),
            "path-based fallback must still resolve the main checkout: {}",
            out
        );
    }

    #[test]
    fn test_not_a_repository() {
        let (_guard, tmp) = tmpdir();
        let dir = tmp.join("plain");
        fs::create_dir_all(&dir).expect("mkdir plain");

        let out = describe(&dir, None);
        assert!(out.contains("(not in a git repository)"), "{}", out);
    }

    #[test]
    fn test_detached_head() {
        let (_guard, tmp) = tmpdir();
        let root = tmp.join("repo");
        let git = root.join(".git");
        fs::create_dir_all(&git).expect("mkdir .git");
        fs::write(git.join("HEAD"), "0123456789abcdef0123456789abcdef01234567\n")
            .expect("write detached HEAD");

        let out = describe(&root, None);
        assert!(out.contains("branch: detached @ 012345678\n"), "{}", out);
    }

    #[test]
    fn test_pwd_note_symlinked_same_dir() {
        let (_guard, tmp) = tmpdir();
        let real = tmp.join("real");
        let link = tmp.join("link");
        fs::create_dir_all(&real).expect("mkdir real");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        #[cfg(not(unix))]
        return;

        let note = pwd_mismatch_note(&real, &link).expect("mismatch note expected");
        assert!(note.starts_with("note: shell $PWD is a symlinked path"), "{}", note);
    }

    #[test]
    fn test_pwd_warn_on_genuinely_different_dir() {
        let (_guard, tmp) = tmpdir();
        let a = tmp.join("a");
        let b = tmp.join("b");
        fs::create_dir_all(&a).expect("mkdir a");
        fs::create_dir_all(&b).expect("mkdir b");

        let note = pwd_mismatch_note(&a, &b).expect("mismatch note expected");
        assert!(note.starts_with("warn: shell $PWD differs"), "{}", note);
    }

    #[test]
    fn test_pwd_note_absent_when_equal() {
        let (_guard, tmp) = tmpdir();
        assert!(pwd_mismatch_note(&tmp, &tmp).is_none());
    }

    #[test]
    fn test_malformed_gitdir_pointer() {
        let (_guard, tmp) = tmpdir();
        let wt = tmp.join("wt");
        fs::create_dir_all(&wt).expect("mkdir wt");
        fs::write(wt.join(".git"), "not a gitdir line\n").expect("write bogus .git");

        let out = describe(&wt, None);
        assert!(
            out.contains("branch: (unknown)"),
            "malformed pointer must degrade, not panic: {}",
            out
        );
    }
}
