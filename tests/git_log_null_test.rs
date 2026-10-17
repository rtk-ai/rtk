//! #4577: `rtk git log --format=%H -z` (and `--oneline -z`) must print the same
//! NUL-separated walk git does under RTK's commit cap. The whole walk is one
//! line to a line filter, and truncating that line at 80 characters kept a
//! hash and `...`.

use std::path::Path;
use std::process::{Command, Output};

mod common;

const COMMITS: usize = 60;
const FORMAT_CAP: usize = 50;

fn isolate(cmd: &mut Command, home: &Path) {
    common::isolate_git(cmd);
    cmd.env("HOME", home)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env("GIT_TERMINAL_PROMPT", "0");
}

fn git_ok(repo: &Path, home: &Path, args: &[&str]) {
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
}

fn git_log(repo: &Path, home: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo).arg("log").args(args);
    isolate(&mut cmd, home);
    cmd.output().expect("run git log")
}

fn rtk_log(repo: &Path, home: &Path, args: &[&str]) -> Output {
    let mut cmd = common::rtk_command();
    cmd.arg("git").arg("log").args(args).current_dir(repo);
    isolate(&mut cmd, home);
    cmd.output().expect("run rtk git log")
}

struct Repo {
    _dir: tempfile::TempDir,
    path: std::path::PathBuf,
    home: std::path::PathBuf,
}

fn repo_with(commits: usize) -> Repo {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&path).expect("mkdir repo");
    std::fs::create_dir_all(&home).expect("mkdir home");
    git_ok(&path, &home, &["init", "-q", "-b", "main"]);
    for i in 0..commits {
        let message = format!("commit {i}");
        git_ok(
            &path,
            &home,
            &["commit", "-q", "--allow-empty", "-m", &message],
        );
    }
    Repo {
        _dir: dir,
        path,
        home,
    }
}

fn nuls(bytes: &[u8]) -> usize {
    bytes.iter().filter(|b| **b == 0).count()
}

fn assert_matches_git(repo: &Repo, rtk_args: &[&str], git_args: &[&str]) -> Output {
    let git = git_log(&repo.path, &repo.home, git_args);
    let rtk = rtk_log(&repo.path, &repo.home, rtk_args);
    assert!(
        git.status.success() && rtk.status.success(),
        "git log {git_args:?} -> {} {}\nrtk git log {rtk_args:?} -> {} {}",
        git.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&git.stderr),
        rtk.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&rtk.stderr),
    );
    assert_eq!(
        rtk.stdout, git.stdout,
        "rtk git log {rtk_args:?} diverged from git log {git_args:?}"
    );
    assert!(
        !rtk.stdout.windows(3).any(|w| w == b"..."),
        "NUL walk was truncated: {:?}",
        String::from_utf8_lossy(&rtk.stdout)
    );
    rtk
}

#[test]
fn format_null_walk_matches_git_under_the_format_cap() {
    let repo = repo_with(COMMITS);
    // `--null` is `-z`'s long spelling, added after the `-z` this bug is about.
    // Older git rejects it; the argv unit test still passes the flag through.
    let null = git_log(&repo.path, &repo.home, &["-1", "--format=%H", "--null"])
        .status
        .success();
    let mut cases: Vec<(&[&str], &[&str])> = vec![
        (&["--format=%H", "-z"], &["-50", "--format=%H", "-z"]),
        (&["--oneline", "-z"], &["-50", "--oneline", "-z"]),
    ];
    if null {
        cases.push((
            &["--format=%H", "--null"],
            &["-50", "--format=%H", "--null"],
        ));
        cases.push((&["--oneline", "--null"], &["-50", "--oneline", "--null"]));
    }
    for (rtk_args, git_args) in cases {
        let out = assert_matches_git(&repo, rtk_args, git_args);
        assert_eq!(nuls(&out.stdout), FORMAT_CAP, "{rtk_args:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("[rtk] capped at 50 commits"),
            "{rtk_args:?}: {stderr:?}"
        );
    }
}

#[test]
fn user_limit_null_walk_is_the_whole_limited_log() {
    // Five hashes are 205 bytes. The old user-limit width was 120, so this
    // still ended in `...` once the walk was one line.
    let repo = repo_with(COMMITS);
    let out = assert_matches_git(
        &repo,
        &["-n", "5", "--format=%H", "-z"],
        &["-n", "5", "--format=%H", "-z"],
    );
    assert_eq!(nuls(&out.stdout), 5);
    assert_eq!(out.stdout.len(), 5 * 41);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("[rtk]"), "{stderr:?}");
}

#[test]
fn short_null_walk_is_not_cut_and_says_nothing() {
    // Three `%H` records are 123 bytes, already past the 80-character cut.
    let repo = repo_with(3);
    let out = assert_matches_git(&repo, &["--format=%H", "-z"], &["--format=%H", "-z"]);
    assert_eq!(nuls(&out.stdout), 3);
    assert_eq!(out.stdout.len(), 3 * 41);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("[rtk]"), "{stderr:?}");
}
