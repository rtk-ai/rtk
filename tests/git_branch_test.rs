#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn rtk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
}

fn git(repo: impl AsRef<Path>, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn setup_remote_repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let origin = dir.path().join("origin.git");
    let seed = dir.path().join("seed");
    let client = dir.path().join("client");

    let output = Command::new("git")
        .args(["init", "--bare", origin.to_str().unwrap()])
        .output()
        .expect("git init --bare");
    assert!(output.status.success());

    let output = Command::new("git")
        .args(["init", seed.to_str().unwrap()])
        .output()
        .expect("git init seed");
    assert!(output.status.success());

    git(&seed, &["config", "user.name", "Test"]);
    git(&seed, &["config", "user.email", "test@example.com"]);
    fs::write(seed.join("README.md"), "hello\n").expect("write README");
    git(&seed, &["add", "README.md"]);
    git(&seed, &["commit", "-m", "init"]);
    git(&seed, &["branch", "-M", "main"]);
    git(
        &seed,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&seed, &["push", "-u", "origin", "main"]);

    git(&seed, &["checkout", "-b", "feature"]);
    fs::write(seed.join("feature.txt"), "feature\n").expect("write feature");
    git(&seed, &["add", "feature.txt"]);
    git(&seed, &["commit", "-m", "feature"]);
    git(&seed, &["push", "-u", "origin", "feature"]);

    let output = Command::new("git")
        .arg("--git-dir")
        .arg(&origin)
        .args(["symbolic-ref", "HEAD", "refs/heads/main"])
        .output()
        .expect("set origin HEAD");
    assert!(output.status.success());

    let output = Command::new("git")
        .args(["clone", origin.to_str().unwrap(), client.to_str().unwrap()])
        .output()
        .expect("git clone");
    assert!(
        output.status.success(),
        "git clone failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    (dir, client)
}

#[test]
fn branch_all_preserves_remote_tracking_refs() {
    let (_dir, client) = setup_remote_repo();

    let output = rtk()
        .current_dir(client)
        .args(["git", "branch", "-a"])
        .output()
        .expect("rtk git branch -a");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "rtk git branch -a failed\nstdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("remotes/origin/HEAD -> origin/main"),
        "missing origin HEAD in branch -a output:\n{stdout}"
    );
    assert!(
        stdout.contains("remotes/origin/main"),
        "missing origin/main in branch -a output:\n{stdout}"
    );
    assert!(
        stdout.contains("remotes/origin/feature"),
        "missing origin/feature in branch -a output:\n{stdout}"
    );
    assert!(
        !stdout.contains("remote-only"),
        "explicit -a should not collapse remotes into a summary:\n{stdout}"
    );
}

#[test]
fn branch_remotes_preserves_native_remote_listing() {
    let (_dir, client) = setup_remote_repo();

    let output = rtk()
        .current_dir(client)
        .args(["git", "branch", "-r"])
        .output()
        .expect("rtk git branch -r");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "rtk git branch -r failed\nstdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !stdout.starts_with("* \n"),
        "explicit -r should not inject a blank current branch:\n{stdout}"
    );
    assert!(
        stdout.contains("origin/HEAD -> origin/main"),
        "missing origin HEAD in branch -r output:\n{stdout}"
    );
    assert!(
        stdout.contains("origin/main"),
        "missing origin/main in branch -r output:\n{stdout}"
    );
    assert!(
        stdout.contains("origin/feature"),
        "missing origin/feature in branch -r output:\n{stdout}"
    );
}
