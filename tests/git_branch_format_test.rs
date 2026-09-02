//! Explicit branch formats are user-owned output, including whitespace and NULs.

use std::path::Path;
use std::process::{Command, Output};

fn git_command(repo: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", repo.join("no-global-config"))
        .env("LC_ALL", "C");
    command
}

fn git(repo: &Path, args: &[&str]) -> Output {
    git_command(repo).args(args).output().expect("run git")
}

fn repository() -> tempfile::TempDir {
    let repo = tempfile::tempdir().expect("temporary repository");
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec![
            "-c",
            "user.name=RTK Test",
            "-c",
            "user.email=rtk@example.invalid",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "initial",
        ],
        vec!["branch", "feature"],
        vec!["update-ref", "refs/remotes/origin/main", "HEAD"],
        vec!["update-ref", "refs/remotes/origin/remote-only", "HEAD"],
    ] {
        let output = git(repo.path(), &args);
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    repo
}

fn assert_matches_git(repo: &Path, args: &[&str]) -> Output {
    let native = git(repo, args);
    let rtk = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", repo.join("no-global-config"))
        .env("LC_ALL", "C")
        .env("RTK_DB_PATH", repo.join("rtk.db"))
        .output()
        .expect("run rtk git");

    assert_eq!(rtk.status.code(), native.status.code(), "args: {args:?}");
    assert_eq!(rtk.stdout, native.stdout, "stdout for {args:?}");
    assert_eq!(rtk.stderr, native.stderr, "stderr for {args:?}");
    native
}

#[test]
fn branch_format_matches_git_for_both_option_spellings() {
    let repo = repository();
    for args in [
        vec!["branch", "--format=%(refname:short)"],
        vec!["branch", "--format", "%(refname:short)"],
        vec![
            "branch",
            "--all",
            "--sort=-refname",
            "--format=%09  %(refname:short) %00%0a",
        ],
        vec![
            "branch",
            "--color=always",
            "--format=%(color:red)%(refname:short)%(color:reset)",
        ],
    ] {
        assert!(assert_matches_git(repo.path(), &args).status.success());
    }
}

#[test]
fn branch_format_preserves_empty_output_and_operand_boundary() {
    let repo = repository();
    let output = assert_matches_git(
        repo.path(),
        &[
            "branch",
            "--format=%(refname:short)",
            "--list",
            "--",
            "--format=does-not-match",
        ],
    );
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn branch_format_preserves_git_errors() {
    let repo = repository();
    for args in [
        vec!["branch", "--format=%(not-a-valid-field)"],
        vec!["branch", "--format"],
    ] {
        assert!(!assert_matches_git(repo.path(), &args).status.success());
    }
}
