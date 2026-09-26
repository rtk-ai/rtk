//! `rtk git commit` names the branch that took the commit, next to the hash.
//!
//! An integration test because the branch is read from git's own summary line, and only real
//! git can show how that line reads for a root commit and for a detached HEAD: the unit tests
//! feed the parser what we believe git prints.

use std::path::Path;
use std::process::Command;

const RTK_BIN: &str = env!("CARGO_BIN_EXE_rtk");

/// Env that isolates git and rtk from the developer's real config, identity and locale, so the
/// branch a commit lands on and the words git prints for it are the same on every machine.
fn isolate(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home)
        .env("RTK_DB_PATH", home.join("rtk.db"))
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_GLOBAL", home.join("nonexistent-global"))
        .env("GIT_CONFIG_SYSTEM", home.join("nonexistent-system"))
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE");
}

fn git(repo: &Path, home: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo);
    cmd.args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"]);
    cmd.args(args);
    isolate(&mut cmd, home);
    let out = cmd.output().expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Stage a new file, run `rtk git commit` on it, and return what rtk printed.
fn rtk_commit(repo: &Path, home: &Path, file: &str) -> String {
    std::fs::write(repo.join(file), format!("{file}\n")).expect("write");
    git(repo, home, &["add", file]);
    let mut cmd = Command::new(RTK_BIN);
    cmd.args(["git", "commit", "-m", &format!("add {file}")])
        .current_dir(repo);
    isolate(&mut cmd, home);
    let out = cmd.output().expect("run rtk git commit");
    assert!(
        out.status.success(),
        "rtk git commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The first seven characters of HEAD, which is all rtk shows of a hash.
fn short_head(repo: &Path, home: &Path) -> String {
    git(repo, home, &["rev-parse", "HEAD"])[..7].to_string()
}

#[test]
fn a_commit_names_the_branch_it_landed_on() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = dir.path().join("repo");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    std::fs::create_dir_all(&home).expect("mkdir home");
    git(&repo, &home, &["init", "-q", "-b", "main"]);

    // The root commit: git also prints "(root-commit)" between the branch and the hash.
    let shown = rtk_commit(&repo, &home, "a.txt");
    assert_eq!(shown, format!("[main] ok {}", short_head(&repo, &home)));

    let shown = rtk_commit(&repo, &home, "b.txt");
    assert_eq!(shown, format!("[main] ok {}", short_head(&repo, &home)));

    git(&repo, &home, &["checkout", "-q", "-b", "feature/login"]);
    let shown = rtk_commit(&repo, &home, "c.txt");
    assert_eq!(
        shown,
        format!("[feature/login] ok {}", short_head(&repo, &home))
    );
}

#[test]
fn a_detached_head_is_not_reported_as_a_branch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = dir.path().join("repo");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    std::fs::create_dir_all(&home).expect("mkdir home");
    git(&repo, &home, &["init", "-q", "-b", "main"]);
    rtk_commit(&repo, &home, "a.txt");

    git(&repo, &home, &["checkout", "-q", "--detach"]);
    let shown = rtk_commit(&repo, &home, "b.txt");
    assert_eq!(
        shown,
        format!("[detached HEAD] ok {}", short_head(&repo, &home))
    );
}
