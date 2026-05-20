use std::fs;
use std::process::{Command, Output};

fn rtk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
}

fn git(dir: &std::path::Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git command should run")
}

fn assert_success(output: Output, context: &str) {
    assert!(
        output.status.success(),
        "{context} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn git_diff_exit_code_preserves_stdout_on_nonzero() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert_success(git(dir.path(), &["init"]), "git init");

    let file = dir.path().join("file.txt");
    fs::write(&file, "one\n").expect("write initial file");
    assert_success(git(dir.path(), &["add", "file.txt"]), "git add");
    fs::write(&file, "two\n").expect("write changed file");

    let output = rtk()
        .current_dir(dir.path())
        .args(["git", "diff", "--exit-code"])
        .output()
        .expect("rtk git diff should run");

    assert_eq!(
        output.status.code(),
        Some(1),
        "git diff --exit-code must preserve git's nonzero exit"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("-one") && stdout.contains("+two"),
        "nonzero diff must still emit patch stdout, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("Changes:"),
        "machine-friendly diff output must not include RTK decoration:\n{stdout}"
    );
}
