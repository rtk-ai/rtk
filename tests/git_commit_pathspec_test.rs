use std::path::Path;
use std::process::{Command, Output};

fn run_git(dir_repo: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .args(args)
        .current_dir(dir_repo)
        .output()
        .expect("run git")
}

fn assert_git_success(dir_repo: &Path, args: &[&str]) {
    let output = run_git(dir_repo, args);
    assert!(
        output.status.success(),
        "git {args:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn commit_preserves_pathspec_separator() {
    let dir_temp = tempfile::tempdir().expect("tempdir");
    let dir_repo = dir_temp.path().join("repo");
    let path_message = dir_temp.path().join("message.txt");
    std::fs::create_dir(&dir_repo).expect("create repo");
    std::fs::write(dir_repo.join("foo.txt"), "staged content\n").expect("write staged file");
    std::fs::write(&path_message, "must stay a pathspec\n").expect("write message file");

    assert_git_success(&dir_repo, &["init", "-q"]);
    assert_git_success(&dir_repo, &["config", "user.email", "test@example.com"]);
    assert_git_success(&dir_repo, &["config", "user.name", "Test User"]);
    assert_git_success(&dir_repo, &["add", "foo.txt"]);

    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args([
            "git",
            "commit",
            "--",
            "foo.txt",
            "-F",
            path_message.to_str().expect("UTF-8 message path"),
        ])
        .current_dir(&dir_repo)
        .output()
        .expect("run rtk git commit");

    assert_eq!(
        output.status.code(),
        Some(128),
        "rtk must preserve `--` so -F remains a pathspec:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !run_git(&dir_repo, &["rev-parse", "--verify", "HEAD"])
            .status
            .success(),
        "rtk must not create a commit when git rejects the pathspec"
    );
}
