//! `rtk git log` in its raw-shape path injects RTK's default limit, so the walk it prints is
//! bounded by RTK rather than by what the user asked for. This pins the one guarantee that
//! makes that acceptable: the cap is never applied in silence.
//!
//! An integration test rather than a unit one because the failure it guards lives in the
//! interaction between the user's own output format and RTK's ability to see what it printed:
//! `--oneline` leaves no commit header to count, `log.decorate` and `--graph` disfigure the
//! one that exists, `-z` separates records with a NUL instead of a newline, and
//! `--line-prefix` puts its own text in front of them. Every one of those once silenced the
//! notice.
//!
//! The shapes that narrow a walk by what its commits changed get their own tests, because
//! those take a different probe: `--skip` is applied before a diff-based filter, so the cap
//! has to be measured by counting rather than by asking for the commit past it, and counting
//! is what every mis-read record above breaks.

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
    &["--cc"],
    &["-c"],
    &["--diff-merges=c"],
    &["--diff-merges", "c"],
    &["--oneline", "-p"],
    &["--graph", "-p"],
    &["--stat", "-p"],
    &["--raw", "-z"],
    &["--name-only", "-z"],
    &["--reverse", "-p"],
    &["--line-prefix=zz", "-p"],
    &["-p", "--", "f.txt"],
    &["-p", "f.txt"],
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
        vec!["--binary", "--ext-diff"],
        vec!["--cc", "--ext-diff"],
        vec!["-c", "--ext-diff"],
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
///
/// `--no-patch`, and one line counted per object name rather than per line of output: the
/// merge-diff flags print a combined patch that `--oneline` does not suppress, and counting
/// raw lines read 14 selected commits as 149.
fn native_commit_count(repo: &Repo, args: &[&str]) -> usize {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(&repo.path).arg("log");
    cmd.args(["--no-patch", "--pretty=format:%H"]);
    cmd.args(args);
    isolate(&mut cmd, &repo.home);
    let out = cmd.output().expect("run git");
    assert!(out.status.success(), "git log {args:?} failed");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|line| matches!(line.len(), 40 | 64) && line.chars().all(|c| c.is_ascii_hexdigit()))
        .count()
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

/// A repo where the first `needles` commits each *add* an occurrence of the string, so a
/// pickaxe selects one commit per occurrence rather than only the two that change its
/// presence -- the shape needed to put a diff-selected walk past the cap.
fn repo_growing_a_needle(commits: usize, needles: usize) -> Repo {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&path).expect("mkdir repo");
    std::fs::create_dir_all(&home).expect("mkdir home");
    git_ok(&path, &home, &["init", "-q", "-b", "main"]);
    let mut body = String::new();
    for i in 0..commits {
        if i < needles {
            body.push_str(&format!("NEEDLE {i}\n"));
        } else {
            body.push_str(&format!("plain {i}\n"));
        }
        std::fs::write(path.join("f.txt"), &body).expect("write");
        git_ok(&path, &home, &["add", "f.txt"]);
        git_ok(&path, &home, &["commit", "-qm", &format!("c{i}")]);
    }
    Repo {
        _dir: dir,
        path,
        home,
    }
}

#[test]
fn a_nul_separated_diff_selected_walk_announces_the_cap() {
    // `-z` terminates each record with a NUL instead of a newline, so a walk read by line is
    // one line holding no commit -- and the diff-selected probe counts commits rather than
    // asking whether one came back.
    let repo = repo_growing_a_needle(25, 17);
    for filter in [vec!["-S", "NEEDLE"], vec!["-G", "NEEDLE"]] {
        let native = native_commit_count(&repo, &filter);
        assert!(native > 10, "{filter:?} must select past the cap: {native}");
        for shape in [
            vec!["-p", "-z"],
            vec!["--name-only", "-z"],
            vec!["-z", "-p"],
        ] {
            let mut args = shape.clone();
            args.extend_from_slice(&filter);
            let out = rtk_log(&repo.path, &repo.home, &args);
            assert!(out.status.success(), "rtk git log {args:?} failed");
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(
                stderr.contains(NOTICE),
                "{args:?} selects {native} commits; stderr was {stderr:?}"
            );
        }
    }

    // `-z` written inside the cluster that carries the pickaxe: the probe needs `-S` and so
    // cannot drop the argument the two share.
    let out = rtk_log(&repo.path, &repo.home, &["-p", "-zSNEEDLE"]);
    assert!(out.status.success(), "rtk git log -p -zSNEEDLE failed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(NOTICE), "clustered -zS: {stderr:?}");

    // Still silent when the filter selects less than the cap: the NUL split must not invent
    // commits any more than the newline one did.
    let shallow = repo_growing_a_needle(25, 3);
    let out = rtk_log(&shallow.path, &shallow.home, &["-p", "-z", "-S", "NEEDLE"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("[rtk]"), "three matches: {stderr:?}");
}

#[test]
fn a_walk_git_refused_carries_no_cap_notice() {
    // The probe drops `--pretty`/`--format`/`--output`, so it answers for a command git may
    // have rejected outright -- and the notice would then describe a walk nobody saw.
    let repo = repo_with(25);
    // A redirect into a directory that does not exist: git refuses it, and the probe drops
    // `--output` and would not.
    let unwritable = format!(
        "--output={}",
        repo.path.join("nodir").join("o.txt").display()
    );
    for shape in [
        vec!["-p", "--pretty=bogus"],
        vec!["-p", "--format=bogus"],
        vec!["-p", unwritable.as_str()],
    ] {
        let out = rtk_log(&repo.path, &repo.home, &shape);
        assert!(!out.status.success(), "git must refuse {shape:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("[rtk]"),
            "cap notice on a refused command {shape:?}: {stderr:?}"
        );
        assert!(
            !stderr.trim().is_empty(),
            "git's own error must reach the user for {shape:?}"
        );
    }
}

/// A repo whose every commit adds a line with trailing whitespace, so `--check` has something
/// to report and exits non-zero on a walk git ran in full.
fn repo_with_whitespace_errors(commits: usize) -> Repo {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&path).expect("mkdir repo");
    std::fs::create_dir_all(&home).expect("mkdir home");
    git_ok(&path, &home, &["init", "-q", "-b", "main"]);
    let mut body = String::new();
    for i in 0..commits {
        body.push_str(&format!("line {i} \n"));
        std::fs::write(path.join("f.txt"), &body).expect("write");
        git_ok(&path, &home, &["add", "f.txt"]);
        git_ok(&path, &home, &["commit", "-qm", &format!("c{i}")]);
    }
    Repo {
        _dir: dir,
        path,
        home,
    }
}

#[test]
fn a_check_run_still_announces_the_cap() {
    // `--check` reports the whitespace errors it found as a non-zero exit, on a walk git ran
    // and printed in full -- neither the probe nor the notice may read that as a refusal.
    let repo = repo_with_whitespace_errors(25);
    let out = rtk_log(&repo.path, &repo.home, &["-p", "--check"]);
    assert!(!out.status.success(), "--check must report the whitespace");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(NOTICE), "{stderr:?}");

    // And still says nothing when the cap cut nothing.
    let short = repo_with_whitespace_errors(3);
    let out = rtk_log(&short.path, &short.home, &["-p", "--check"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("[rtk]"), "three commits: {stderr:?}");
}

/// A repo whose tracked file is a list of full-length object names, the way a checksum file
/// or a lockfile carries them, with a needle in three of its twenty commits.
fn repo_of_checksums() -> Repo {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&path).expect("mkdir repo");
    std::fs::create_dir_all(&home).expect("mkdir home");
    git_ok(&path, &home, &["init", "-q", "-b", "main"]);

    let checksum = |n: usize| format!("{n:040x}");
    let mut body = String::new();
    for i in 0..10 {
        body.push_str(&format!("{}\n", checksum(0xdead_0000 + i)));
    }
    let commit = |body: &str, message: &str| {
        std::fs::write(path.join("f.txt"), body).expect("write");
        git_ok(&path, &home, &["add", "f.txt"]);
        git_ok(&path, &home, &["commit", "-qm", message]);
    };
    commit(&body, "base");
    for i in 0..3 {
        body.push_str(&format!("NEEDLE {i}\n"));
        for j in 1..4 {
            body.push_str(&format!("{}\n", checksum(0xcafe_0000 + i * 16 + j)));
        }
        commit(&body, &format!("n{i}"));
    }
    for i in 0..16 {
        body.push_str(&format!("plain {i}\n"));
        commit(&body, &format!("p{i}"));
    }
    Repo {
        _dir: dir,
        path,
        home,
    }
}

#[test]
fn a_file_of_bare_hex_words_is_not_counted_as_a_walk() {
    // A diff-based filter keeps the merge-diff flags in what the probe forwards, because the
    // walk is selecting on them, so the probe still prints a patch here -- and its context
    // lines are these checksums with one space in front. Three commits selected out of
    // twenty must not read as more than the cap.
    let repo = repo_of_checksums();
    let path = &repo.path;
    let home = &repo.home;

    for shape in [
        vec!["--binary", "-S", "NEEDLE"],
        vec!["--cc", "-S", "NEEDLE"],
        vec!["-c", "-S", "NEEDLE"],
        vec!["-p", "-S", "NEEDLE"],
    ] {
        let out = rtk_log(path, home, &shape);
        assert!(out.status.success(), "rtk git log {shape:?} failed");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("[rtk]"),
            "three commits selected, cap of ten: {shape:?} said {stderr:?}"
        );
    }
}

/// A repo whose merges carry content that is in neither parent, so the merge-diff flags
/// change which commits a diff-based filter selects rather than only how they are printed.
fn repo_with_evil_merges(rounds: usize) -> Repo {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&path).expect("mkdir repo");
    std::fs::create_dir_all(&home).expect("mkdir home");
    git_ok(&path, &home, &["init", "-q", "-b", "main"]);
    let commit = |message: &str| {
        git_ok(&path, &home, &["add", "-A"]);
        git_ok(&path, &home, &["commit", "-qm", message]);
    };
    std::fs::write(path.join("f.txt"), "base\n").expect("write");
    commit("base");
    for i in 0..rounds {
        let branch = format!("side{i}");
        git_ok(&path, &home, &["checkout", "-q", "-b", &branch, "main"]);
        std::fs::write(path.join("f.txt"), format!("base\nside {i}\n")).expect("write");
        commit(&format!("s{i}"));
        git_ok(&path, &home, &["checkout", "-q", "main"]);
        std::fs::write(path.join("g.txt"), format!("main {i}\n")).expect("write");
        commit(&format!("m{i}"));
        git_ok(
            &path,
            &home,
            &["merge", "-q", "--no-commit", "--no-ff", &branch],
        );
        // The evil part: content the merge introduces that neither parent has, which only a
        // walk that diffs merges can see.
        std::fs::write(path.join("h.txt"), format!("evil {i}\n")).expect("write");
        commit(&format!("merge s{i}"));
    }
    Repo {
        _dir: dir,
        path,
        home,
    }
}

#[test]
fn a_merge_diff_walk_is_measured_with_its_merges_still_in_it() {
    // `-c`, `--cc`, `--remerge-diff` and `--diff-merges=<anything but off>` hand merge commits
    // a diff of their own, and a diff-based filter selects on the diffs the walk produces --
    // so these decide which commits come back, not just how they look. The counting probe's
    // answer *is* that number, so it has to run the walk with them still in it.
    let repo = repo_with_evil_merges(14);
    let plain = native_commit_count(&repo, &["-S", "evil"]);
    assert_eq!(plain, 0, "without a merge diff the needle is invisible");

    let mut past_the_cap = 0;
    for flag in [
        vec!["-c"],
        vec!["--cc"],
        vec!["--remerge-diff"],
        vec!["--diff-merges=c"],
        vec!["--diff-merges=remerge"],
        vec!["--diff-merges", "combined"],
    ] {
        for filter in [vec!["-S", "evil"], vec!["--diff-filter=A"]] {
            let mut selecting = flag.clone();
            selecting.extend_from_slice(&filter);
            let native = native_commit_count(&repo, &selecting);
            assert!(native > 0, "{selecting:?} must select something");
            past_the_cap += usize::from(native > 10);

            let mut args = vec!["-p"];
            args.extend_from_slice(&selecting);
            let out = rtk_log(&repo.path, &repo.home, &args);
            assert!(out.status.success(), "rtk git log {args:?} failed");
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(
                stderr.contains(NOTICE),
                native > 10,
                "{args:?} selects {native} commits; stderr was {stderr:?}"
            );
        }
    }
    assert!(
        past_the_cap >= 8,
        "only {past_the_cap} of the twelve combinations reached past the cap, so the \
         blocker they pin would go unnoticed"
    );

    // And still silent when those same flags leave the selection under the cap.
    let small = repo_with_evil_merges(2);
    let native = native_commit_count(&small, &["-c", "-S", "evil"]);
    assert!(native <= 10, "two rounds must stay under the cap: {native}");
    let out = rtk_log(&small.path, &small.home, &["-p", "-c", "-S", "evil"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("[rtk]"), "{native} selected: {stderr:?}");
}

#[test]
fn a_path_that_looks_like_an_object_name_is_not_counted_as_a_commit() {
    // Under `-z` a name-listing shape puts every path in a record of its own at column 0,
    // where no indent marks it out from a commit name. Only its length does: `%H` never
    // abbreviates, so anything shorter than a whole object name came from beside the walk.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&path).expect("mkdir repo");
    std::fs::create_dir_all(&home).expect("mkdir home");
    git_ok(&path, &home, &["init", "-q", "-b", "main"]);

    let hex_names: Vec<String> = (1..=5)
        .map(|i| format!("cafebabe0{i}"))
        .chain(std::iter::once("deadbeef01".to_string()))
        .collect();
    let commit = |message: &str| {
        git_ok(&path, &home, &["add", "-A"]);
        git_ok(&path, &home, &["commit", "-qm", message]);
    };
    for name in &hex_names {
        std::fs::write(path.join(name), "seed\n").expect("write");
    }
    commit("base");
    for i in 0..3 {
        // Every hex-named file in the same commit: `--pickaxe-all` then lists all of them,
        // so one selected commit yields a whole run of path records.
        for name in &hex_names {
            let target = path.join(name);
            let body = std::fs::read_to_string(&target).expect("read") + &format!("NEEDLE {i}\n");
            std::fs::write(&target, body).expect("write");
        }
        commit(&format!("n{i}"));
    }
    for i in 0..16 {
        let target = path.join(&hex_names[5]);
        let body = std::fs::read_to_string(&target).expect("read") + &format!("plain {i}\n");
        std::fs::write(&target, body).expect("write");
        commit(&format!("p{i}"));
    }

    let repo = Repo {
        _dir: dir,
        path,
        home,
    };
    let filter = ["-S", "NEEDLE", "--pickaxe-all"];
    let native = native_commit_count(&repo, &filter);
    assert_eq!(native, 3, "the needle must select three commits");

    for shape in [
        vec!["--name-only", "-z"],
        vec!["--name-status", "-z"],
        vec!["--raw", "-z"],
    ] {
        let mut args = shape.clone();
        args.extend_from_slice(&filter);
        let out = rtk_log(&repo.path, &repo.home, &args);
        assert!(out.status.success(), "rtk git log {args:?} failed");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("[rtk]"),
            "{native} commits selected, cap of ten: {args:?} said {stderr:?}"
        );
    }
}

#[test]
fn a_line_prefix_does_not_hide_a_diff_selected_cap() {
    // `--line-prefix` puts its own text in front of every record the probe reads, so the
    // commit names came back unreadable behind it and the cap went unannounced.
    let repo = repo_growing_a_needle(25, 17);
    let filter = ["-S", "NEEDLE"];
    let native = native_commit_count(&repo, &filter);
    assert!(native > 10, "{filter:?} must select past the cap: {native}");

    for prefix in [vec!["--line-prefix=zz"], vec!["--line-prefix", "zz"]] {
        let mut args = vec!["-p"];
        args.extend_from_slice(&prefix);
        args.extend_from_slice(&filter);
        let out = rtk_log(&repo.path, &repo.home, &args);
        assert!(out.status.success(), "rtk git log {args:?} failed");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(NOTICE),
            "{args:?} selects {native} commits; stderr was {stderr:?}"
        );
    }
}

#[test]
fn a_graphed_diff_selected_walk_is_measured_like_any_other() {
    // `--graph` draws its rail with the characters a patch also starts its lines with, so no
    // trimming rule tells a commit from diff content behind one. The probe drops it instead,
    // which it can because the rail only reorders the walk.
    let repo = repo_growing_a_needle(25, 17);
    let filter = ["-S", "NEEDLE"];
    let native = native_commit_count(&repo, &filter);
    assert!(native > 10, "{filter:?} must select past the cap: {native}");

    for shape in [
        vec!["--graph", "-p"],
        vec!["--graph", "--cc"],
        vec!["-c", "--graph"],
    ] {
        let mut args = shape.clone();
        args.extend_from_slice(&filter);
        let out = rtk_log(&repo.path, &repo.home, &args);
        assert!(out.status.success(), "rtk git log {args:?} failed");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(NOTICE),
            "{args:?} selects {native} commits; stderr was {stderr:?}"
        );
    }

    // And a graphed walk whose filter selects under the cap still says nothing, even when the
    // file it walks is full of things shaped like object names.
    let checksums = repo_of_checksums();
    let shallow = ["-S", "NEEDLE"];
    let few = native_commit_count(&checksums, &shallow);
    assert_eq!(few, 3, "the needle must select three commits");
    for shape in [vec!["--graph", "--cc"], vec!["--graph", "-c"]] {
        let mut args = shape.clone();
        args.extend_from_slice(&shallow);
        let out = rtk_log(&checksums.path, &checksums.home, &args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("[rtk]"),
            "{few} selected, cap of ten: {args:?} said {stderr:?}"
        );
    }
}

#[test]
fn a_diff_selected_walk_under_a_user_skip_is_measured_from_where_they_started() {
    // git counts `--skip` before applying a diff-based filter, so the counting probe has to
    // reproduce the user's own skip: without it the count covers commits they had already
    // skipped past, and a walk the cap never touched announced one.
    let repo = repo_growing_a_needle(25, 17);
    for (skip, want_notice) in [("20", false), ("5", true)] {
        let filter = vec!["--skip", skip, "-S", "NEEDLE"];
        let native = native_commit_count(&repo, &filter);
        assert_eq!(
            native > 10,
            want_notice,
            "skip={skip} leaves {native} selected"
        );

        let mut args = vec!["-p"];
        args.extend_from_slice(&filter);
        let out = rtk_log(&repo.path, &repo.home, &args);
        assert!(out.status.success(), "rtk git log {args:?} failed");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            stderr.contains(NOTICE),
            want_notice,
            "{args:?} leaves {native} selected; stderr was {stderr:?}"
        );
    }
}

/// A repo whose merges leave one path untouched, so those merges are TREESAME to both their
/// parents for it. Under `--full-history` they are kept only when parent rewriting is on.
fn repo_with_treesame_merges(rounds: usize) -> Repo {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&path).expect("mkdir repo");
    std::fs::create_dir_all(&home).expect("mkdir home");
    git_ok(&path, &home, &["init", "-q", "-b", "main"]);
    let commit = |message: &str| {
        git_ok(&path, &home, &["add", "-A"]);
        git_ok(&path, &home, &["commit", "-qm", message]);
    };
    std::fs::write(path.join("p.txt"), "p\n").expect("write");
    commit("base");
    for i in 0..rounds {
        let branch = format!("side{i}");
        git_ok(&path, &home, &["checkout", "-q", "-b", &branch, "main"]);
        std::fs::write(path.join(format!("side{i}.txt")), "s\n").expect("write");
        commit(&format!("s{i}"));
        git_ok(&path, &home, &["checkout", "-q", "main"]);
        std::fs::write(path.join(format!("main{i}.txt")), "m\n").expect("write");
        commit(&format!("m{i}"));
        git_ok(
            &path,
            &home,
            &[
                "merge",
                "-q",
                "--no-ff",
                "-m",
                &format!("merge s{i}"),
                &branch,
            ],
        );
    }
    Repo {
        _dir: dir,
        path,
        home,
    }
}

#[test]
fn a_graphed_walk_keeps_the_commits_its_parent_rewriting_adds() {
    // `--graph` is not only a rail: it turns on parent rewriting, which decides which commits
    // the walk returns. Dropping it for the probe without asking for that back measured a
    // different walk -- one path here holds a single commit without the rewriting and
    // fifteen with it.
    let repo = repo_with_treesame_merges(14);
    let plain = native_commit_count(&repo, &["--full-history", "--", "p.txt"]);
    let rewritten = native_commit_count(&repo, &["--full-history", "--parents", "--", "p.txt"]);
    assert_eq!(plain, 1, "without parent rewriting the path has one commit");
    assert!(
        rewritten > 10,
        "with it the merges come back too: {rewritten}"
    );

    let out = rtk_log(
        &repo.path,
        &repo.home,
        &["--graph", "--full-history", "--stat", "--", "p.txt"],
    );
    assert!(out.status.success(), "rtk git log failed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(NOTICE),
        "the graphed walk holds {rewritten} commits; stderr was {stderr:?}"
    );

    // And without the rail the same path is one commit, which the cap never touches.
    let out = rtk_log(
        &repo.path,
        &repo.home,
        &["--full-history", "--stat", "--", "p.txt"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("[rtk]"), "one commit: {stderr:?}");
}

#[test]
fn a_reversed_diff_selected_walk_is_still_measured_by_what_it_selects() {
    // `--reverse` makes git drain the whole walk before it shows anything, so `--max-count`
    // bounds the walk rather than what came through the filter -- the counting probe asked
    // for eleven and was handed a number with no relation to the selection.
    let repo = repo_growing_a_needle(25, 17);
    let filter = ["-S", "NEEDLE"];
    let native = native_commit_count(&repo, &filter);
    assert!(native > 10, "{filter:?} must select past the cap: {native}");

    for shape in [vec!["-p", "--reverse"], vec!["--reverse", "--stat"]] {
        let mut args = shape.clone();
        args.extend_from_slice(&filter);
        let out = rtk_log(&repo.path, &repo.home, &args);
        assert!(out.status.success(), "rtk git log {args:?} failed");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(NOTICE),
            "{args:?} selects {native} commits; stderr was {stderr:?}"
        );
    }
}
