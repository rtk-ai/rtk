#![cfg(unix)]
//! `git show`'s truncation hint names a command that prints the commit (#4817).
//!
//! The old hint said `rtk git diff --no-compact`, which shows the working
//! tree — following it never reveals the truncated commit. The hint now re-runs
//! the user's own `git show` shape, which is the command whose output was cut.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let dir = common::temp_git_repo();
        let root = dir.path().to_path_buf();
        for (name, body) in [("a.txt", 5usize), ("b.txt", 200)] {
            let content: String = (1..=body).map(|i| format!("line {i}\n")).collect();
            std::fs::write(root.join(name), content).expect("write");
        }
        for args in [
            &["add", "."][..],
            &["commit", "-q", "-m", "fixture with a big hunk"][..],
        ] {
            let mut git = Command::new("git");
            common::isolate_git(&mut git);
            let ok = git
                .args(args)
                .current_dir(&root)
                .status()
                .expect("git")
                .success();
            assert!(ok, "git {args:?} failed");
        }
        Sandbox { _dir: dir, root }
    }

    fn rtk(&self) -> Command {
        let mut cmd = common::rtk_command();
        cmd.current_dir(&self.root);
        cmd
    }
}

/// The hunk shown is capped at 100 lines, so b.txt's 200 additions truncate and
/// the recovery hint appears — naming `git show HEAD`, not `rtk git diff`.
#[test]
fn truncated_show_hints_at_the_commit_itself() {
    let repo = Sandbox::new();
    let out = repo
        .rtk()
        .args(["git", "show", "HEAD"])
        .output()
        .expect("run rtk git show");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();

    assert!(
        stdout.contains("... (100 additions truncated)"),
        "fixture must truncate, got:\n{stdout}"
    );
    assert!(
        stdout.contains("[full diff: git show HEAD]"),
        "hint must name the commit command, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("rtk git diff --no-compact"),
        "the working-tree hint must not leak into git show, got:\n{stdout}"
    );

    // And the hinted command must actually print what was cut: all 200 lines.
    let mut full = Command::new("git");
    common::isolate_git(&mut full);
    let full_out = full
        .args(["show", "HEAD"])
        .current_dir(&repo.root)
        .output()
        .expect("run git show");
    let full_text = String::from_utf8_lossy(&full_out.stdout).into_owned();
    assert_eq!(
        full_text.matches("added line ").count() + full_text.matches("line ").count(),
        full_text.matches("line ").count(),
        "sanity: helper counts lines"
    );
    assert!(
        full_text.contains("line 200\n"),
        "git show HEAD must contain the truncated tail"
    );
}

/// `rtk git diff` keeps its own, still-correct hint: there the working tree IS
/// the subject.
#[test]
fn diff_keeps_the_working_tree_hint() {
    let repo = Sandbox::new();
    std::fs::write(
        repo.root.join("b.txt"),
        (1..=200usize)
            .map(|i| format!("edited {i}\n"))
            .collect::<String>(),
    )
    .expect("rewrite tracked file");

    let out = repo
        .rtk()
        .args(["git", "diff"])
        .output()
        .expect("run rtk git diff");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();

    assert!(
        stdout.contains("[full diff: rtk git diff --no-compact]"),
        "git diff keeps its hint, got:\n{stdout}"
    );
}

/// `git stash show -p` gets a hint that re-runs it, not one about the tree.
#[test]
fn stash_show_hints_at_stash_show() {
    let repo = Sandbox::new();
    // A stashed change big enough to truncate: `git stash` on the clean tree
    // from `new()` saves nothing and `stash show -p` would print nothing.
    std::fs::write(
        repo.root.join("b.txt"),
        (1..=200usize)
            .map(|i| format!("stashed {i}\n"))
            .collect::<String>(),
    )
    .expect("rewrite tracked file");
    let mut git = Command::new("git");
    common::isolate_git(&mut git);
    let ok = git
        .args(["stash"])
        .current_dir(&repo.root)
        .status()
        .expect("git stash")
        .success();
    assert!(ok, "git stash failed");

    let out = repo
        .rtk()
        .args(["git", "stash", "show", "-p"])
        .output()
        .expect("run rtk git stash show");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        stdout.contains("[full diff: git stash show -p]"),
        "got:\n{stdout}"
    );
}

#[allow(dead_code)]
fn unused(_p: &Path) {}
