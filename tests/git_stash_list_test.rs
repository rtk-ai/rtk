//! End-to-end checks for `rtk git stash list` argument forwarding, error propagation, and
//! raw passthrough when formatting or diff flags are requested.

use std::path::Path;
use std::process::{Command, Output};

mod common;

fn git(repo: &Path) -> Command {
    let mut cmd = common::native_command("git");
    common::isolate_git(&mut cmd);
    cmd.current_dir(repo);
    cmd
}

fn rtk_in(dir: &Path) -> Command {
    let mut cmd = common::rtk_command();
    cmd.env("LC_ALL", "C")
        .env_remove("LANGUAGE")
        .env("RTK_SUPPRESS_HOOK_WARNING", "1")
        .current_dir(dir);
    cmd
}

fn run_ok(cmd: &mut Command, what: &str) -> Output {
    let out = cmd.output().unwrap_or_else(|e| panic!("{what}: {e}"));
    assert_eq!(out.status.code(), Some(0), "{what}: {out:?}");
    out
}

fn repo_with_stashes(messages: &[&str]) -> tempfile::TempDir {
    let repo = common::temp_git_repo();
    std::fs::write(repo.path().join("tracked.txt"), "base\n").expect("write base");
    run_ok(
        git(repo.path()).args(["add", "tracked.txt"]),
        "git add tracked.txt",
    );
    run_ok(
        git(repo.path()).args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "add tracked",
        ]),
        "git commit tracked.txt",
    );

    for (idx, msg) in messages.iter().enumerate() {
        std::fs::write(
            repo.path().join("tracked.txt"),
            format!("url: http://example.test: 808{idx} ({msg})\n"),
        )
        .expect("modify tracked");
        run_ok(
            git(repo.path()).args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "stash",
                "push",
                "-q",
                "-m",
                msg,
            ]),
            "git stash push",
        );
    }

    repo
}

fn run_both(repo: &Path, args: &[&str]) -> (Output, Output) {
    let native = git(repo)
        .args(["stash", "list"])
        .args(args)
        .output()
        .expect("spawn native git stash list");
    let rtk = rtk_in(repo)
        .args(["git", "stash", "list"])
        .args(args)
        .output()
        .expect("spawn rtk git stash list");
    (native, rtk)
}

#[test]
fn git_stash_list_honors_count_and_filter_flags() {
    let repo = repo_with_stashes(&["stash-1", "stash-2", "stash-3"]);

    for (args, expected_lines) in [
        (&["-1"][..], &["stash@{0}: stash-3"][..]),
        (
            &["-n", "2"][..],
            &["stash@{0}: stash-3", "stash@{1}: stash-2"][..],
        ),
        (&["--max-count=1"][..], &["stash@{0}: stash-3"][..]),
        (&["--max-count", "1"][..], &["stash@{0}: stash-3"][..]),
        (
            &["--skip=1"][..],
            &["stash@{1}: stash-2", "stash@{2}: stash-1"][..],
        ),
        (
            &["--skip", "1"][..],
            &["stash@{1}: stash-2", "stash@{2}: stash-1"][..],
        ),
        (&["--grep=stash-2"][..], &["stash@{1}: stash-2"][..]),
    ] {
        let (native, rtk) = run_both(repo.path(), args);
        assert_eq!(native.status.code(), Some(0), "native {args:?}: {native:?}");
        assert_eq!(rtk.status.code(), Some(0), "rtk {args:?}: {rtk:?}");
        assert!(
            rtk.stderr.is_empty(),
            "unexpected stderr for {args:?}: {:?}",
            String::from_utf8_lossy(&rtk.stderr)
        );

        let native_lines: Vec<&str> = std::str::from_utf8(&native.stdout)
            .expect("native utf-8")
            .lines()
            .collect();
        let rtk_lines: Vec<&str> = std::str::from_utf8(&rtk.stdout)
            .expect("rtk utf-8")
            .lines()
            .collect();
        assert_eq!(
            rtk_lines.len(),
            native_lines.len(),
            "entry count mismatch for {args:?}: rtk={rtk_lines:?}, native={native_lines:?}"
        );
        assert_eq!(rtk_lines, expected_lines, "filtered lines for {args:?}");
    }
}

#[test]
fn git_stash_list_propagates_invalid_option_failure() {
    let repo = repo_with_stashes(&["stash-1", "stash-2", "stash-3"]);

    for args in [
        &["--rtk-invalid"][..],
        &["--not-a-real-option"][..],
        &["--graph"][..],
    ] {
        let (native, rtk) = run_both(repo.path(), args);
        assert_ne!(
            native.status.code(),
            Some(0),
            "native git should reject {args:?}"
        );
        assert_eq!(
            rtk.status.code(),
            native.status.code(),
            "exit code mismatch for {args:?}: rtk={rtk:?}, native={native:?}"
        );
        assert!(
            rtk.stdout.is_empty(),
            "failed git stash list {args:?} must not emit stdout: {:?}",
            String::from_utf8_lossy(&rtk.stdout)
        );
        assert_eq!(
            String::from_utf8_lossy(&rtk.stderr),
            String::from_utf8_lossy(&native.stderr),
            "stderr mismatch for {args:?}"
        );
    }
}

#[test]
fn git_stash_list_passes_through_format_and_diff_flags() {
    let repo = repo_with_stashes(&["stash-1", "stash-2", "stash-3"]);

    for args in [
        &["--format=%gd|%s"][..],
        &["--oneline"][..],
        &["--stat", "-1"][..],
        &["-p", "-1"][..],
        &["--name-only", "-1"][..],
        &["--log-size", "-1"][..],
        &["-z", "-1"][..],
        &["--", "-p", "-1"][..],
        &["--", "-z"][..],
    ] {
        let (native, rtk) = run_both(repo.path(), args);
        assert_eq!(native.status.code(), Some(0), "native {args:?}: {native:?}");
        assert_eq!(rtk.status.code(), Some(0), "rtk {args:?}: {rtk:?}");
        assert_eq!(
            rtk.stdout,
            native.stdout,
            "stdout mismatch for {args:?}:\nrtk: {}\nnative: {}",
            String::from_utf8_lossy(&rtk.stdout),
            String::from_utf8_lossy(&native.stdout)
        );
    }
}

#[test]
fn git_stash_list_works_with_global_dash_c() {
    let repo = repo_with_stashes(&["stash-1", "stash-2"]);
    let elsewhere = common::tempdir();
    let repo_str = repo.path().to_string_lossy().into_owned();

    let filtered = rtk_in(elsewhere.path())
        .args(["git", "-C", &repo_str, "stash", "list", "-1"])
        .output()
        .expect("spawn rtk -C stash list -1");
    assert_eq!(filtered.status.code(), Some(0), "{filtered:?}");
    assert_eq!(
        String::from_utf8_lossy(&filtered.stdout).trim(),
        "stash@{0}: stash-2"
    );

    let native_fmt = git(elsewhere.path())
        .args(["-C", &repo_str, "stash", "list", "--format=%gd|%s"])
        .output()
        .expect("spawn native -C stash list --format");
    let rtk_fmt = rtk_in(elsewhere.path())
        .args(["git", "-C", &repo_str, "stash", "list", "--format=%gd|%s"])
        .output()
        .expect("spawn rtk -C stash list --format");
    assert_eq!(rtk_fmt.status.code(), Some(0), "{rtk_fmt:?}");
    assert_eq!(rtk_fmt.stdout, native_fmt.stdout);
}

#[test]
fn git_stash_list_empty_filter_result_emits_empty() {
    let repo = repo_with_stashes(&["stash-1", "stash-2"]);

    for args in [&["--grep=no-such-stash"][..], &["-0"][..]] {
        let (native, rtk) = run_both(repo.path(), args);
        assert_eq!(native.status.code(), Some(0), "native {args:?}");
        assert_eq!(rtk.status.code(), Some(0), "rtk {args:?}");
        assert!(rtk.stdout.is_empty(), "expected empty stdout for {args:?}");
        assert!(rtk.stderr.is_empty(), "expected empty stderr for {args:?}");
    }
}
