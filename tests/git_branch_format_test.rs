//! `rtk git branch --format` hands git's listing through byte-identical: with a template, each
//! line is whatever the caller wrote, not an entry of the default layout the compact form reads.

use std::path::Path;
use std::process::{Command, Output};

mod common;

fn git(repo: &Path) -> Command {
    let mut cmd = common::native_command("git");
    common::isolate_git(&mut cmd);
    cmd.current_dir(repo);
    cmd
}

fn run(cmd: &mut Command, what: &str) -> Output {
    let out = cmd.output().unwrap_or_else(|e| panic!("{what}: {e}"));
    assert_eq!(out.status.code(), Some(0), "{what}: {out:?}");
    out
}

fn current_branch(repo: &Path) -> String {
    let out = run(
        git(repo).args(["branch", "--show-current"]),
        "git branch --show-current",
    );
    String::from_utf8(out.stdout)
        .expect("utf-8 name")
        .trim()
        .to_string()
}

/// The bytes git prints and the bytes rtk prints for `git branch <args>` in `repo`.
fn both(repo: &Path, args: &[&str]) -> (Vec<u8>, Vec<u8>) {
    let native = run(git(repo).arg("branch").args(args), "git branch");
    let rtk = run(
        common::rtk_command()
            .env("RTK_NO_TRACK", "1")
            .current_dir(repo)
            .args(["git", "branch"])
            .args(args),
        "rtk git branch",
    );
    (native.stdout, rtk.stdout)
}

/// One branch gives a one-line listing, which a compaction that re-indents every entry would
/// change; two give several lines.
#[test]
fn a_formatted_listing_is_gits_bytes() {
    let one = common::temp_git_repo();
    let two = common::temp_git_repo();
    // `side` is merged into the current branch and `ahead` is not, so `--merged` and
    // `--no-merged` each list a strict subset.
    for args in [
        &["branch", "side"][..],
        &["switch", "-q", "-c", "ahead"],
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "ahead",
        ],
        &["switch", "-q", "-"],
    ] {
        run(git(two.path()).args(args), "git setup");
    }

    for repo in [one.path(), two.path()] {
        let current = current_branch(repo);
        for args in [
            &["--format=%(refname:short)"][..],
            &["--format", "%(refname:short)"],
            &["--format=%(refname:short)", "--contains", "HEAD"],
            &["--format=%(refname:short)", "--points-at", "HEAD"],
            &["--format=%(refname:short)", "-l", &current],
            &["--format=%(HEAD) %(refname:short)"],
            &["--format=%(refname)", "-a"],
            &["--format=%(refname:short)", "--merged", "HEAD"],
            &["--format=%(refname:short)", "--no-merged", "HEAD"],
            &["--format=%(refname:short)", "--sort", "-refname"],
            &["--format=%(refname:short)", "--sort=refname"],
        ] {
            let (native, rtk) = both(repo, args);
            // Only `--no-merged` can list nothing: in the repository with one branch, every
            // branch is merged.
            assert!(
                !native.is_empty() || args.contains(&"--no-merged"),
                "{args:?} lists something"
            );
            assert_eq!(
                String::from_utf8_lossy(&rtk),
                String::from_utf8_lossy(&native),
                "{args:?}"
            );
        }
    }
}

/// `-r` in a repository with no remotes lists nothing, and a template does not change that.
#[test]
fn an_empty_formatted_listing_prints_nothing() {
    let repo = common::temp_git_repo();
    let args = ["--format=%(refname:short)", "-r"];

    let (native, rtk) = both(repo.path(), &args);

    assert!(native.is_empty(), "git lists nothing: {native:?}");
    assert_eq!(rtk, native);
}
