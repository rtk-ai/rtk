use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const RTK_BIN: &str = env!("CARGO_BIN_EXE_rtk");

fn isolate(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", home.join("nonexistent-global"))
        .env("GIT_CONFIG_SYSTEM", home.join("nonexistent-system"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE");
}

fn git_output(repo: &Path, home: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(repo);
    isolate(&mut cmd, home);
    cmd.output().expect("run git")
}

fn git_ok(repo: &Path, home: &Path, args: &[&str]) {
    let output = git_output(repo, home, args);
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_with_stdin(program: &str, args: &[&str], repo: &Path, home: &Path, input: &str) -> Output {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate(&mut cmd, home);
    let mut child = cmd.spawn().expect("spawn command");
    child
        .stdin
        .take()
        .expect("command stdin")
        .write_all(input.as_bytes())
        .expect("write command stdin");
    child.wait_with_output().expect("wait for command")
}

#[test]
fn git_log_stdin_matches_native_git() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = dir.path().join("repo");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&repo).expect("create repo");
    std::fs::create_dir_all(&home).expect("create home");

    git_ok(&repo, &home, &["init", "-q", "-b", "main"]);
    for i in 1..=3 {
        std::fs::write(repo.join("f.txt"), format!("{i}\n")).expect("write file");
        git_ok(&repo, &home, &["add", "f.txt"]);
        git_ok(
            &repo,
            &home,
            &[
                "-c",
                "user.email=a@b.c",
                "-c",
                "user.name=t",
                "commit",
                "-qm",
                &format!("commit {i}"),
            ],
        );
    }

    let old = git_output(&repo, &home, &["rev-list", "--max-parents=0", "HEAD"]);
    assert!(old.status.success(), "git rev-list failed");
    let old = String::from_utf8(old.stdout).expect("revision is UTF-8");

    for options in [
        &["--stdin", "--no-walk", "--oneline"][..],
        &["--stdin", "--no-walk", "--patch"][..],
    ] {
        let mut native_args = vec!["log"];
        native_args.extend_from_slice(options);
        let native = run_with_stdin("git", &native_args, &repo, &home, &old);

        let mut rtk_args = vec!["git", "log"];
        rtk_args.extend_from_slice(options);
        let wrapped = run_with_stdin(RTK_BIN, &rtk_args, &repo, &home, &old);

        assert_eq!(
            wrapped.status.code(),
            native.status.code(),
            "exit status mismatch for git log {options:?}"
        );
        assert_eq!(
            wrapped.stdout,
            native.stdout,
            "stdout mismatch for git log {options:?}: rtk={} native={}",
            String::from_utf8_lossy(&wrapped.stdout),
            String::from_utf8_lossy(&native.stdout)
        );
    }
}
