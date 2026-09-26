//! `rtk git log` in its raw-shape path injects RTK's default limit, so the walk it prints is
//! bounded by RTK rather than by what the user asked for. This pins the one guarantee that
//! makes that acceptable: the cap is never applied in silence.
//!
//! An integration test rather than a unit one because the failure it guards lives in the
//! interaction between the user's own output format and RTK's ability to see what it printed:
//! `--oneline` leaves no commit header to count, `log.decorate` and `--graph` disfigure the
//! one that exists, `-z` runs the whole walk onto one line, and `--line-prefix` puts something
//! in front of it. Every one of those once silenced the notice.

use std::path::Path;
use std::process::{Command, Output};

const RTK_BIN: &str = env!("CARGO_BIN_EXE_rtk");
const NOTICE: &str = "[rtk] capped at 10 commits";

/// Env that isolates git and rtk from the developer's real config, identity and home.
fn isolate(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home)
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

fn rtk_log(repo: &Path, home: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(RTK_BIN);
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
        std::fs::write(path.join("f.txt"), format!("content {i}\n")).expect("write");
        git_ok(&path, &home, &["add", "f.txt"]);
        git_ok(&path, &home, &["commit", "-qm", &format!("commit {i}")]);
    }
    Repo {
        _dir: dir,
        path,
        home,
    }
}

/// Every output shape that once swallowed the notice.
const CAPPED_SHAPES: &[&[&str]] = &[
    &["-p"],
    &["--name-only"],
    &["--name-status"],
    &["--patch-with-stat"],
    &["--patch-with-raw"],
    &["--binary"],
    &["--oneline", "-p"],
    &["--graph", "-p"],
    &["--stat", "-p"],
    &["--raw", "-z"],
    &["--name-only"],
    &["--name-status"],
    &["--patch-with-stat"],
    &["--line-prefix=zz", "-p"],
    &["-p", "--", "f.txt"],
    &["-p", "f.txt"],
    &["--patch-with-stat"],
];

#[test]
fn every_capped_shape_announces_the_cap() {
    let repo = repo_with(25);
    for shape in CAPPED_SHAPES {
        let out = rtk_log(&repo.path, &repo.home, shape);
        assert!(out.status.success(), "rtk git log {shape:?} failed");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(NOTICE),
            "no cap notice for `git log {}`; stderr was {stderr:?}",
            shape.join(" ")
        );
    }
}

#[test]
fn a_coloured_decorated_walk_announces_the_cap() {
    // Config the user sets rather than flags they pass: both once disfigured the commit
    // header the notice used to be counted out of.
    let repo = repo_with(25);
    for globals in [
        vec!["-c", "log.decorate=short"],
        vec!["-c", "color.ui=always"],
    ] {
        let mut cmd = Command::new(RTK_BIN);
        cmd.arg("git")
            .args(&globals)
            .args(["log", "--graph", "-p"])
            .current_dir(&repo.path);
        isolate(&mut cmd, &repo.home);
        let out = cmd.output().expect("run rtk git log");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(NOTICE),
            "no cap notice under {globals:?}; stderr was {stderr:?}"
        );
    }
}

#[test]
fn a_walk_shorter_than_the_cap_says_nothing() {
    // The notice must not claim a truncation that did not happen, whatever shape the user
    // asked for -- the probe has to be as silent here as it is loud above.
    let repo = repo_with(3);
    for shape in CAPPED_SHAPES {
        let out = rtk_log(&repo.path, &repo.home, shape);
        assert!(out.status.success(), "rtk git log {shape:?} failed");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("[rtk]"),
            "three commits, cap of ten: nothing was cut, but `git log {}` said {stderr:?}",
            shape.join(" ")
        );
    }

    // Exactly the cap is not more than the cap.
    let exact = repo_with(10);
    let out = rtk_log(&exact.path, &exact.home, &["-p"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("[rtk]"),
        "ten commits, cap of ten: {stderr:?}"
    );

    // One past it is.
    let over = repo_with(11);
    let out = rtk_log(&over.path, &over.home, &["-p"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(NOTICE),
        "eleven commits, cap of ten: {stderr:?}"
    );
}

#[test]
fn a_user_skip_is_measured_from_where_the_user_started() {
    // RTK's own `--skip` has to absorb the user's, or the probe asks about a commit the user
    // is already past. 25 commits skipping 20 leaves 5, which the cap does not reach.
    let repo = repo_with(25);
    let out = rtk_log(&repo.path, &repo.home, &["-p", "--skip=20"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("[rtk]"), "five commits left: {stderr:?}");

    let out = rtk_log(&repo.path, &repo.home, &["-p", "--skip", "5"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(NOTICE), "twenty commits left: {stderr:?}");
}

#[test]
fn an_empty_user_format_does_not_read_as_an_empty_walk() {
    // `--pretty=format:` prints a commit as no bytes at all, so forwarding it would make the
    // probe's "did git print anything" read as "nothing left".
    //
    // `--summary` and `--dirstat` are the shapes that prove it: every other raw-shape flag
    // prints something of its own for a plain modification, which masks the empty format.
    let repo = repo_with(25);
    for shape in [
        vec!["--summary", "--pretty=format:"],
        vec!["--dirstat", "--format="],
        vec!["-p", "--pretty=format:"],
        vec!["--oneline", "-p"],
    ] {
        let out = rtk_log(&repo.path, &repo.home, &shape);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(NOTICE), "{shape:?}: {stderr:?}");
    }
}

#[test]
fn a_user_output_file_keeps_what_the_command_wrote() {
    // `--output=<file>` is a redirect, not a format: left in what the probe forwards, the
    // probe reruns it and truncates the file the capped command has just written.
    let repo = repo_with(25);
    for redirect in [vec!["--output=OUT"], vec!["--output", "OUT"]] {
        let target = repo.path.join(format!("out{}.txt", redirect.len()));
        let shape: Vec<String> = redirect
            .iter()
            .map(|a| a.replace("OUT", &target.to_string_lossy()))
            .collect();
        let mut args: Vec<&str> = shape.iter().map(String::as_str).collect();
        args.push("-p");

        let out = rtk_log(&repo.path, &repo.home, &args);
        assert!(out.status.success(), "rtk git log {args:?} failed");

        let written = std::fs::read_to_string(&target).expect("the output file");
        let commits = written.lines().filter(|l| l.starts_with("commit ")).count();
        assert_eq!(
            commits, 10,
            "{args:?} left {commits} commits in the output file, not the 10 the cap shows"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(NOTICE), "{args:?}: {stderr:?}");
    }
}

#[cfg(unix)]
#[test]
fn the_probe_does_not_rerun_the_users_diff_program() {
    // The probe reads only whether git printed anything, so a patch is work with no answer in
    // it -- and with `diff.external` configured it is the user's own program, run once more
    // than they asked for.
    use std::os::unix::fs::PermissionsExt;

    let repo = repo_with(25);
    let counter = repo.path.join("calls.log");
    let driver = repo.path.join("driver.sh");
    std::fs::write(
        &driver,
        format!(
            "#!/bin/sh\necho x >> {}\nexit 0\n",
            counter.to_string_lossy()
        ),
    )
    .expect("write driver");
    std::fs::set_permissions(&driver, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    // Every spelling that asks for a patch, including the two the shape strip once missed.
    for shape in [
        vec!["-p", "--ext-diff"],
        vec!["--patch-with-stat", "--ext-diff"],
        vec!["--patch-with-raw", "--ext-diff"],
    ] {
        let _ = std::fs::remove_file(&counter);
        let mut cmd = Command::new(RTK_BIN);
        cmd.args(["git", "log"])
            .args(&shape)
            .current_dir(&repo.path);
        isolate(&mut cmd, &repo.home);
        cmd.env("GIT_EXTERNAL_DIFF", &driver);
        let out = cmd.output().expect("run rtk git log");
        assert!(out.status.success(), "{shape:?}");

        let calls = std::fs::read_to_string(&counter).map_or(0, |c| c.lines().count());
        assert_eq!(
            calls, 10,
            "{shape:?}: the diff program ran {calls} times for a 10-commit window"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(NOTICE), "{shape:?}: {stderr:?}");
    }
}

#[test]
fn a_sha256_repo_announces_the_cap() {
    // A SHA-256 object name is 64 hex characters, not 40.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&path).expect("mkdir repo");
    std::fs::create_dir_all(&home).expect("mkdir home");
    git_ok(
        &path,
        &home,
        &["init", "-q", "-b", "main", "--object-format=sha256"],
    );
    for i in 0..15 {
        std::fs::write(path.join("f.txt"), format!("content {i}\n")).expect("write");
        git_ok(&path, &home, &["add", "f.txt"]);
        git_ok(&path, &home, &["commit", "-qm", &format!("commit {i}")]);
    }
    let out = rtk_log(&path, &home, &["-p"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(NOTICE), "sha256 repo: {stderr:?}");
}

#[test]
fn a_user_limit_or_a_revision_range_is_not_capped() {
    // RTK caps only a walk the user left unbounded, so these must not carry the notice --
    // and must return everything asked for.
    let repo = repo_with(25);
    for shape in [
        vec!["-p", "-n", "20"],
        vec!["-p", "-n20"],
        vec!["-p", "-20"],
        vec!["-p", "--max-count=20"],
        vec!["-p", "HEAD~20..HEAD"],
    ] {
        let out = rtk_log(&repo.path, &repo.home, &shape);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains(NOTICE),
            "cap notice on an explicitly bounded walk {shape:?}: {stderr:?}"
        );
        let commits = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| l.starts_with("commit "))
            .count();
        assert_eq!(commits, 20, "{shape:?} must return all 20 commits");
    }
}

#[test]
fn a_glued_n_limit_is_the_users_limit_on_the_compact_listing() {
    // #2665: `-n20` was not read as a limit, so RTK's own cap of 10 cut the compact listing
    // short of the 20 asked for, without a word. Every spelling of the count must agree.
    let repo = repo_with(25);
    let listed = |args: &[&str]| {
        String::from_utf8_lossy(&rtk_log(&repo.path, &repo.home, args).stdout)
            .lines()
            .filter(|l| {
                l.split_whitespace()
                    .next()
                    .is_some_and(|h| h.len() >= 7 && h.bytes().all(|b| b.is_ascii_hexdigit()))
            })
            .count()
    };

    assert_eq!(
        listed(&[]),
        10,
        "the cap on an unbounded walk, for contrast"
    );
    for shape in [vec!["-n20"], vec!["-n", "20"], vec!["-20"]] {
        assert_eq!(listed(&shape), 20, "{shape:?} must return all 20 commits");
    }
}

#[test]
fn an_exit_code_run_still_announces_the_cap() {
    // `--exit-code` makes a perfectly successful `git log` exit 1. Reading that as "git
    // refused the command" and returning early left the cap unannounced.
    let repo = repo_with(25);
    let out = rtk_log(&repo.path, &repo.home, &["--exit-code", "-p"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(NOTICE), "{stderr:?}");
}

/// A repo where only the first few commits touch a distinctive string, so a diff-based filter
/// selects far fewer commits than the walk holds.
fn repo_with_a_needle(commits: usize, needles: usize) -> Repo {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&path).expect("mkdir repo");
    std::fs::create_dir_all(&home).expect("mkdir home");
    git_ok(&path, &home, &["init", "-q", "-b", "main"]);
    for i in 0..commits {
        let body = if i < needles {
            format!("NEEDLE{i}\n")
        } else {
            format!("plain {i}\n")
        };
        std::fs::write(path.join("f.txt"), body).expect("write");
        git_ok(&path, &home, &["add", "f.txt"]);
        git_ok(&path, &home, &["commit", "-qm", &format!("c{i}")]);
    }
    Repo {
        _dir: dir,
        path,
        home,
    }
}

/// How many commits `git log` itself selects for these arguments.
fn native_commit_count(repo: &Repo, args: &[&str]) -> usize {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(&repo.path).arg("log").arg("--oneline");
    cmd.args(args);
    isolate(&mut cmd, &repo.home);
    let out = cmd.output().expect("run git");
    assert!(out.status.success(), "git log --oneline {args:?} failed");
    String::from_utf8_lossy(&out.stdout).lines().count()
}

#[test]
fn a_diff_selected_walk_announces_a_cap_only_when_the_cap_cut() {
    // git counts `--skip` where the walk starts and applies a diff-based filter after it, so
    // asking for the commit past the cap by skipping reports on commits the filter would have
    // dropped -- a walk matching twice claimed a cap of ten.
    let repo = repo_with_a_needle(25, 3);
    for filter in [
        vec!["-S", "NEEDLE"],
        vec!["-S", "plain"],
        vec!["-G", "NEEDLE"],
        vec!["-G", "plain"],
        vec!["--diff-filter=A"],
        vec!["--diff-filter=M"],
    ] {
        let native = native_commit_count(&repo, &filter);
        let mut args = vec!["-p"];
        args.extend_from_slice(&filter);

        let out = rtk_log(&repo.path, &repo.home, &args);
        assert!(out.status.success(), "rtk git log {args:?} failed");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            stderr.contains(NOTICE),
            native > 10,
            "{filter:?} selects {native} commits; stderr was {stderr:?}"
        );
    }
}

#[test]
fn a_command_git_refuses_reports_gits_error_and_no_cap_notice() {
    let repo = repo_with(3);
    let out = rtk_log(&repo.path, &repo.home, &["-p", "nosuchref"]);
    assert!(!out.status.success(), "git must refuse an unknown revision");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains(NOTICE), "stderr was {stderr:?}");
    assert!(
        !stderr.trim().is_empty(),
        "git's own error must reach the user"
    );
}
