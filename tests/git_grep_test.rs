#![cfg(unix)]
//! `rtk git grep`: git's own engine, the agent's args verbatim, output grouped and capped
//! like `rtk grep`. Compared against native `git grep` in the same isolated repository.

use std::path::Path;
use std::process::Command;

mod common;

fn commit_files(repo: &Path, files: &[(&str, String)]) {
    for (name, content) in files {
        std::fs::write(repo.join(name), content).expect("write fixture");
    }
    for args in [&["add", "-A"][..], &["commit", "-q", "-m", "fixture"][..]] {
        let mut git = Command::new("git");
        common::isolate_git(&mut git);
        let status = git.args(args).current_dir(repo).status().expect("run git");
        assert!(status.success(), "git {args:?} failed");
    }
}

fn rtk_git_grep(repo: &Path, args: &[&str]) -> (String, Option<i32>) {
    let out = common::rtk_command()
        .args(["git", "grep"])
        .args(args)
        .current_dir(repo)
        .output()
        .expect("run rtk git grep");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code(),
    )
}

fn native_git_grep(repo: &Path, args: &[&str]) -> String {
    let out = common::native_command("git")
        .arg("grep")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("run git grep");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn small_result_matches_native_output() {
    let repo = common::temp_git_repo();
    commit_files(
        repo.path(),
        &[
            ("a.txt", "needle one\nother\n".into()),
            ("b.txt", "x: needle two\n".into()),
        ],
    );
    for args in [&["needle"][..], &["-n", "needle"][..]] {
        let (out, code) = rtk_git_grep(repo.path(), args);
        assert_eq!(code, Some(0));
        assert_eq!(out, native_git_grep(repo.path(), args), "args {args:?}");
    }
}

#[test]
fn large_result_is_grouped_and_capped() {
    let repo = common::temp_git_repo();
    let many: String = (0..400).map(|i| format!("needle {i}\n")).collect();
    commit_files(repo.path(), &[("big.txt", many)]);
    let (out, code) = rtk_git_grep(repo.path(), &["-n", "needle"]);
    let native = native_git_grep(repo.path(), &["-n", "needle"]);
    assert_eq!(code, Some(0));
    assert!(out.starts_with("400 matches in 1 files:"), "got:\n{out}");
    assert!(out.contains("big.txt:1:needle 0"), "got:\n{out}");
    assert!(
        out.len() * 5 < native.len(),
        "expected >=80% reduction: {} vs {} bytes",
        out.len(),
        native.len()
    );
}

#[test]
fn revisions_and_pathspecs_reach_git_verbatim() {
    let repo = common::temp_git_repo();
    commit_files(repo.path(), &[("a.txt", "needle\n".into())]);
    let args = ["-n", "needle", "HEAD", "--", "a.txt"];
    let (out, code) = rtk_git_grep(repo.path(), &args);
    assert_eq!(code, Some(0));
    assert_eq!(out, native_git_grep(repo.path(), &args));
    assert!(out.starts_with("HEAD:a.txt:1:"), "got:\n{out}");
}

#[test]
fn shape_flags_pass_through_unchanged() {
    let repo = common::temp_git_repo();
    commit_files(
        repo.path(),
        &[("a.txt", "needle\n".into()), ("b.txt", "needle\n".into())],
    );
    for args in [
        &["-l", "needle"][..],
        &["-c", "needle"][..],
        &["-C1", "needle"][..],
    ] {
        let (out, _) = rtk_git_grep(repo.path(), args);
        assert_eq!(out, native_git_grep(repo.path(), args), "args {args:?}");
    }
}

#[test]
fn no_match_keeps_git_exit_code() {
    let repo = common::temp_git_repo();
    commit_files(repo.path(), &[("a.txt", "hay\n".into())]);
    let (out, code) = rtk_git_grep(repo.path(), &["needle"]);
    assert_eq!(code, Some(1));
    assert!(out.is_empty(), "got:\n{out}");
}
