//! Both of `rtk git log`'s paths bound the walk by RTK's own limit rather than by what the
//! user asked for. This pins the one guarantee that makes that acceptable: the cap is never
//! applied in silence, and never announced when it took nothing.
//!
//! An integration test rather than a unit one because every failure it guards lives in the
//! gap between what RTK printed and what git actually walked, which only a real repository
//! shows. On the raw-shape path that gap is the output's shape: `--oneline` leaves no commit
//! header to count, `log.decorate` and `--graph` disfigure the one that exists, `-z` runs the
//! walk onto a single line, `--line-prefix` puts something in front of it. On the filtered
//! path it is the unit the cap is counted in -- commits where RTK chose the format, lines
//! where the user did -- along with what git prints after a commit that is not another one
//! and the `--no-merges` RTK adds to the command and must therefore also ask about.
//!
//! The shapes that narrow a walk by what its commits changed get their own tests, because
//! those take a different probe: `--skip` is applied before a diff-based filter, so the cap
//! has to be measured by counting rather than by asking for the commit past it, and counting
//! is what every mis-read record above breaks.

use std::path::Path;
use std::process::{Command, Output};

mod common;

const NOTICE: &str = "[rtk] capped at 10 commits";

/// Env that isolates git and rtk from the developer's real config, identity and home.
fn isolate(cmd: &mut Command, home: &Path) {
    common::isolate_git(cmd);
    cmd.env("HOME", home)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env("GIT_TERMINAL_PROMPT", "0");
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
    let mut cmd = common::rtk_command();
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
    repo_with_subjects(commits, |i| format!("commit {i}"))
}

/// A history whose commit `i` has the subject `subject(i)`.
fn repo_with_subjects(commits: usize, subject: impl Fn(usize) -> String) -> Repo {
    repo_with_messages(commits, |i| vec![subject(i)])
}

/// A history whose commit `i` carries the message paragraphs `message(i)`, the subject first.
fn repo_with_messages(commits: usize, message: impl Fn(usize) -> Vec<String>) -> Repo {
    repo_committing(commits, |_| vec!["f.txt".to_string()], message)
}

/// A history whose commit `i` writes the files `files(i)` and carries the message paragraphs
/// `message(i)`, one `-m` each.
fn repo_committing(
    commits: usize,
    files: impl Fn(usize) -> Vec<String>,
    message: impl Fn(usize) -> Vec<String>,
) -> Repo {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&path).expect("mkdir repo");
    std::fs::create_dir_all(&home).expect("mkdir home");
    git_ok(&path, &home, &["init", "-q", "-b", "main"]);
    for i in 0..commits {
        for file in files(i) {
            let target = path.join(&file);
            std::fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
            std::fs::write(target, format!("content {i}\n")).expect("write");
        }
        git_ok(&path, &home, &["add", "-A"]);
        let paragraphs = message(i);
        let mut args = vec!["commit", "-q"];
        for paragraph in &paragraphs {
            args.extend(["-m", paragraph.as_str()]);
        }
        git_ok(&path, &home, &args);
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
        let mut cmd = common::rtk_command();
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
        let mut cmd = common::rtk_command();
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

    // The filtered path reads the object name out of its own format, whatever length it has.
    for (range, owed) in [("HEAD~10..HEAD", false), ("HEAD~11..HEAD", true)] {
        let out = rtk_log(&path, &home, &[range]);
        assert!(out.status.success(), "rtk git log {range} failed");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(filtered_entries(&stdout), 10, "{range}: {stdout:?}");
        assert!(!stdout.contains('\u{1}'), "{range}: {stdout:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(stderr.contains(NOTICE), owed, "{range}: {stderr:?}");
    }
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

/// Entries in RTK's own log format: one header line per commit, hash first.
fn filtered_entries(stdout: &str) -> usize {
    stdout
        .lines()
        .filter(|l| {
            let head = l.split(' ').next().unwrap_or("");
            head.len() >= 7 && head.chars().all(|c| c.is_ascii_hexdigit())
        })
        .count()
}

#[test]
fn the_filtered_walk_announces_the_default_cap() {
    // No raw-shape flag, so RTK filters the log itself and injects a limit of ten. The
    // entries it prints carry no sign that anything is missing.
    let repo = repo_with(25);
    let out = rtk_log(&repo.path, &repo.home, &[]);
    assert!(out.status.success(), "rtk git log failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(filtered_entries(&stdout), 10, "{stdout:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(NOTICE), "{stderr:?}");
}

#[test]
fn a_compact_filtered_walk_announces_the_fifty_it_was_capped_at() {
    // A format flag raises RTK's limit to 50, and the notice has to name that number rather
    // than the default the other path applies.
    let repo = repo_with(55);
    for shape in [
        vec!["--oneline"],
        vec!["--pretty=format:%h %s"],
        vec!["--format=%h %s"],
    ] {
        let out = rtk_log(&repo.path, &repo.home, &shape);
        assert!(out.status.success(), "rtk git log {shape:?} failed");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(stdout.lines().count(), 50, "{shape:?}: {stdout:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("[rtk] capped at 50 commits"),
            "{shape:?}: {stderr:?}"
        );
    }

    // Exactly the cap is not more than the cap; one past it is. `--skip` sizes the remaining
    // walk without a second repository, and RTK's own skip has to absorb the user's for the
    // probe to land on the right commit.
    let out = rtk_log(&repo.path, &repo.home, &["--oneline", "--skip=5"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("[rtk]"), "fifty commits left: {stderr:?}");

    let out = rtk_log(&repo.path, &repo.home, &["--oneline", "--skip=4"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[rtk] capped at 50 commits"),
        "fifty-one commits left: {stderr:?}"
    );
}

#[test]
fn a_short_filtered_walk_says_nothing() {
    let repo = repo_with(3);
    for shape in [vec![], vec!["--oneline"]] {
        let out = rtk_log(&repo.path, &repo.home, &shape);
        assert!(out.status.success(), "rtk git log {shape:?} failed");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("[rtk]"),
            "three commits, nothing cut, but `git log {}` said {stderr:?}",
            shape.join(" ")
        );
    }

    let exact = repo_with(10);
    let out = rtk_log(&exact.path, &exact.home, &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("[rtk]"),
        "ten commits, cap of ten: {stderr:?}"
    );

    let over = repo_with(11);
    let out = rtk_log(&over.path, &over.home, &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(NOTICE),
        "eleven commits, cap of ten: {stderr:?}"
    );
}

#[test]
fn a_user_count_on_the_filtered_walk_says_nothing() {
    // The user asked for that many and got them, so nothing was taken from them.
    let repo = repo_with(25);
    for shape in [
        vec!["-n", "20"],
        vec!["-20"],
        vec!["--max-count=20"],
        vec!["--oneline", "-n", "20"],
    ] {
        let out = rtk_log(&repo.path, &repo.home, &shape);
        assert!(out.status.success(), "rtk git log {shape:?} failed");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("[rtk]"),
            "cap notice on a walk the user counted {shape:?}: {stderr:?}"
        );
    }
}

#[test]
fn a_revision_range_is_capped_on_the_filtered_path_and_says_so() {
    // A range leaves the raw-shape path uncapped, so it carries no notice there. This path
    // caps it all the same, and twenty commits asked for coming back as ten is exactly the
    // truncation the notice exists to report.
    let repo = repo_with(25);

    let out = rtk_log(&repo.path, &repo.home, &["HEAD~20..HEAD"]);
    assert!(out.status.success(), "rtk git log HEAD~20..HEAD failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(filtered_entries(&stdout), 10, "{stdout:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(NOTICE), "{stderr:?}");

    // A range the cap does not reach stays silent.
    let out = rtk_log(&repo.path, &repo.home, &["HEAD~5..HEAD"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("[rtk]"), "five commits: {stderr:?}");
}

#[test]
fn a_skipped_pickaxe_walk_that_returned_everything_says_nothing() {
    // The count of a diff-based filter starts where the user's `--skip` left off, so a walk
    // that printed every commit it selected says nothing.
    // Exactly the cap left after the skip, so the printed run settles nothing.
    let repo = repo_with_a_needle(15, 15);
    assert_eq!(
        native_commit_count(&repo, &["-G", "NEEDLE", "--skip=5"]),
        10
    );
    assert_eq!(
        native_commit_count(&repo, &["-G", "NEEDLE", "--skip=1"]),
        14
    );

    // The default format is fetched one commit past the cap, so the run itself answers.
    // `--reverse` is not fetched that way, so the counting probe has to answer, and it has to
    // start its count where the skip left off.
    for shape in [&[][..], &["--reverse"][..]] {
        let args = |skip: &'static str| {
            let mut args = vec!["-G", "NEEDLE", skip];
            args.extend_from_slice(shape);
            args
        };
        let out = rtk_log(&repo.path, &repo.home, &args("--skip=5"));
        assert!(out.status.success(), "rtk git log {shape:?} failed");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(filtered_entries(&stdout), 10, "{shape:?}: {stdout:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("[rtk]"),
            "{shape:?}: all ten selected commits were shown: {stderr:?}"
        );

        // And still loud when the skip leaves more than the cap.
        let out = rtk_log(&repo.path, &repo.home, &args("--skip=1"));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(NOTICE),
            "{shape:?}: fourteen selected: {stderr:?}"
        );
    }
}

/// A history whose commits are mostly merges: fewer ordinary commits than RTK's cap, more
/// commits overall than it.
fn merge_heavy_repo(sides: usize) -> Repo {
    merge_heavy_repo_with_bodies(sides, None)
}

/// [`merge_heavy_repo`] whose side commits carry `body` under their subject.
fn merge_heavy_repo_with_bodies(sides: usize, body: Option<&str>) -> Repo {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&path).expect("mkdir repo");
    std::fs::create_dir_all(&home).expect("mkdir home");
    git_ok(&path, &home, &["init", "-q", "-b", "main"]);
    std::fs::write(path.join("base.txt"), "base\n").expect("write");
    git_ok(&path, &home, &["add", "-A"]);
    git_ok(&path, &home, &["commit", "-qm", "base"]);
    for i in 0..sides {
        let branch = format!("s{i}");
        git_ok(&path, &home, &["checkout", "-q", "-b", &branch, "main"]);
        std::fs::write(path.join(format!("{branch}.txt")), "side\n").expect("write");
        git_ok(&path, &home, &["add", "-A"]);
        let subject = format!("side {i}");
        let mut commit = vec!["commit", "-q", "-m", subject.as_str()];
        if let Some(body) = body {
            commit.extend(["-m", body]);
        }
        git_ok(&path, &home, &commit);
        git_ok(&path, &home, &["checkout", "-q", "main"]);
        git_ok(
            &path,
            &home,
            &[
                "merge",
                "-q",
                "--no-ff",
                "-m",
                &format!("merge {i}"),
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
fn merges_rtk_drops_are_not_counted_as_a_cut() {
    // The filtered path adds `--no-merges` of its own, so a cap is measured on the walk it
    // ran: eleven ordinary commits among merges are more than the cap, ten are not.
    // Exactly the cap in ordinary commits, nearly twice that overall.
    let repo = merge_heavy_repo(9);
    assert_eq!(native_commit_count(&repo, &[]), 19);
    assert_eq!(native_commit_count(&repo, &["--no-merges"]), 10);
    let wide = merge_heavy_repo(10);
    assert_eq!(native_commit_count(&wide, &["--no-merges"]), 11);

    // The default format is fetched one commit past the cap, so the run itself answers.
    // `--reverse` is not fetched that way, so the probe has to answer about the walk RTK ran,
    // which means carrying RTK's `--no-merges`.
    for shape in [&[][..], &["--reverse"][..]] {
        let out = rtk_log(&repo.path, &repo.home, shape);
        assert!(out.status.success(), "rtk git log {shape:?} failed");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(filtered_entries(&stdout), 10, "{shape:?}: {stdout:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("[rtk]"),
            "{shape:?}: ten commits shown of ten selected: {stderr:?}"
        );

        let out = rtk_log(&wide.path, &wide.home, shape);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(NOTICE),
            "{shape:?}: eleven ordinary commits: {stderr:?}"
        );

        // A user who asks for merges gets RTK's `--no-merges` withheld, and then the wider
        // walk is the right one to ask about.
        let mut merges = vec!["--merges"];
        merges.extend_from_slice(shape);
        let out = rtk_log(&wide.path, &wide.home, &merges);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("[rtk]"),
            "{shape:?}: ten merges, cap of ten: {stderr:?}"
        );
    }
}

/// What git prints for `args` on this repository, in the environment the rtk child gets.
fn git_stdout(repo: &Repo, args: &[&str]) -> String {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(&repo.path).args(args);
    isolate(&mut cmd, &repo.home);
    let out = cmd.output().expect("run git");
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8(out.stdout).expect("utf-8")
}

/// `rtk git log <args>` with the recall store pinned to this repository's own directory, so
/// the test can tell whether the run wrote one, and `extra` set on the child.
fn rtk_log_with_recall_in(repo: &Repo, args: &[&str], extra: &[(&str, &str)]) -> Output {
    let mut cmd = common::rtk_command();
    cmd.arg("git").arg("log").args(args).current_dir(&repo.path);
    isolate(&mut cmd, &repo.home);
    cmd.env("RTK_RECALL_DB", recall_db(repo));
    for (key, value) in extra {
        cmd.env(key, value);
    }
    cmd.output().expect("run rtk git log")
}

fn recall_db(repo: &Repo) -> std::path::PathBuf {
    repo.home.join("recall.db")
}

/// `rtk recall <args>` against the store [`rtk_log_with_recall_in`] wrote.
fn rtk_recall(repo: &Repo, args: &[&str]) -> Output {
    let mut cmd = common::rtk_command();
    cmd.arg("recall").args(args).current_dir(&repo.path);
    isolate(&mut cmd, &repo.home);
    cmd.env("RTK_RECALL_DB", recall_db(repo));
    cmd.output().expect("run rtk recall")
}

/// Splits `[+N hidden: rtk recall <id>]` into N and the id.
fn parse_recall_hint(line: &str) -> (usize, String) {
    let inner = line
        .strip_prefix("[+")
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or_else(|| panic!("not a recall hint: {line:?}"));
    let (hidden, id) = inner
        .split_once(" hidden: rtk recall ")
        .unwrap_or_else(|| panic!("not a recall hint: {line:?}"));
    (hidden.parse().expect("hidden count"), id.to_string())
}

/// A line as the filtered path shows it under a format the user chose: cut to 80 characters.
fn shown_form(line: &str) -> String {
    if line.chars().count() > 80 {
        let head: String = line.chars().take(77).collect();
        format!("{head}...")
    } else {
        line.to_string()
    }
}

#[test]
fn a_multi_line_user_format_hands_back_the_lines_it_cut_through_recall() {
    // RTK cannot find the commit boundaries in a shape it did not choose, so it caps lines,
    // and a `--pretty` that spends several lines on a commit loses commits well before the
    // fiftieth. RTK holds every line it drops, so the hint points at them instead of at a
    // second run of git. What recall returns is what the log would have shown: the same
    // lines, cut to the same width.
    let repo = repo_with_subjects(25, |i| format!("commit {i}: {}", "long subject ".repeat(8)));
    for shape in ["--pretty=full", "--pretty=medium", "--format=%H%n%s%n%an"] {
        let git_args = ["log", "-50", "--no-merges", shape];
        let raw = git_stdout(&repo, &git_args);
        let raw_lines: Vec<&str> = raw.lines().collect();
        assert!(raw_lines.len() > 50, "{shape}: the walk must pass the cap");
        assert!(
            raw_lines[50..].iter().any(|l| l.chars().count() > 80),
            "{shape}: the hidden tail must hold a line the display would cut"
        );
        let shown: Vec<String> = raw_lines.iter().map(|l| shown_form(l)).collect();

        let out = rtk_log_with_recall_in(&repo, &[shape], &[]);
        assert!(out.status.success(), "rtk git log {shape} failed");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines.len(), 51, "{shape}: fifty lines and the hint");
        assert_eq!(&lines[..50], &shown[..50], "{shape}: shown lines");
        let (hidden, id) = parse_recall_hint(lines[50]);
        assert_eq!(hidden, raw_lines.len() - 50, "{shape}: hidden count");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.trim().is_empty(),
            "{shape}: the hint is the whole announcement, got {stderr:?}"
        );

        // Recall returns the lines the log did not print, in the form it prints them, and
        // `--full` the whole walk in that form.
        let hidden_tail = rtk_recall(&repo, &[&id]);
        assert!(hidden_tail.status.success(), "{shape}: rtk recall failed");
        assert_eq!(
            String::from_utf8_lossy(&hidden_tail.stdout).trim_end(),
            shown[50..].join("\n"),
            "{shape}: recalled tail"
        );
        let whole = rtk_recall(&repo, &[&id, "--full"]);
        assert_eq!(
            String::from_utf8_lossy(&whole.stdout).trim_end(),
            shown.join("\n"),
            "{shape}: recalled walk"
        );
    }
}

/// The directory holding the first `git` on `PATH`, so a wrapper placed ahead of it can run
/// the real one.
#[cfg(unix)]
fn real_git() -> std::path::PathBuf {
    let path = std::env::var_os("PATH").expect("PATH");
    std::env::split_paths(&path)
        .map(|dir| dir.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("git on PATH")
}

/// `rtk git log <args>` with `stub` ahead of everything else on `PATH`.
#[cfg(unix)]
fn rtk_log_with_stub(repo: &Repo, stub: &Path, args: &[&str], extra: &[(&str, &str)]) -> Output {
    let mut cmd = common::rtk_command();
    cmd.arg("git").arg("log").args(args).current_dir(&repo.path);
    isolate(&mut cmd, &repo.home);
    cmd.env("RTK_RECALL_DB", recall_db(repo));
    let path = std::env::var_os("PATH").expect("PATH");
    let mut dirs = vec![stub.to_path_buf()];
    dirs.extend(std::env::split_paths(&path));
    cmd.env("PATH", std::env::join_paths(dirs).expect("join PATH"));
    for (key, value) in extra {
        cmd.env(key, value);
    }
    cmd.output().expect("run rtk git log")
}

/// Writes an executable `git` into `dir` that runs `body`.
#[cfg(unix)]
fn stub_git(dir: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("git");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write stub git");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

/// `rtk git log <args>` through a `git` that records its calls, and how many of them were a
/// `log`.
#[cfg(unix)]
fn rtk_log_counting_walks(repo: &Repo, args: &[&str]) -> (Output, usize) {
    let stub = tempfile::tempdir().expect("tempdir");
    let calls = stub.path().join("calls.log");
    stub_git(
        stub.path(),
        &format!(
            "echo \"$@\" >> {}\nexec {} \"$@\"",
            calls.display(),
            real_git().display()
        ),
    );
    let out = rtk_log_with_stub(repo, stub.path(), args, &[]);
    let walks = std::fs::read_to_string(&calls)
        .expect("calls log")
        .lines()
        .filter(|call| call.split(' ').any(|arg| arg == "log"))
        .count();
    (out, walks)
}

#[cfg(unix)]
#[test]
fn a_multi_line_cut_is_announced_by_the_hint_alone_and_costs_no_second_walk() {
    // Past the walk recall holds there is more history, and the hint is still the whole
    // announcement: a notice naming `-n` would send the agent back to git for what RTK
    // already read, and the probe behind it walks the history a second time.
    let repo = repo_with(60);
    let (out, walks) = rtk_log_counting_walks(&repo, &["--pretty=full"]);
    assert!(out.status.success(), "rtk git log failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    parse_recall_hint(stdout.lines().last().unwrap_or_default());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.trim().is_empty(),
        "sixty commits past the walk recall holds: {stderr:?}"
    );
    assert_eq!(walks, 1, "the history was walked {walks} times");
}

#[cfg(unix)]
#[test]
fn a_walk_one_short_of_the_cap_is_settled_without_asking_git_again() {
    // Forty-nine commits, each closed by a newline, are forty-nine and not fifty: the run
    // ended before the cap could bite, and the separator count says so.
    let repo = repo_with(60);
    let shape = "--pretty=tformat:%h %s";
    let (out, walks) = rtk_log_counting_walks(&repo, &[shape, "--skip=11"]);
    assert!(out.status.success(), "rtk git log {shape} failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.lines().count(), 49, "{stdout:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.trim().is_empty(), "{stderr:?}");
    assert_eq!(walks, 1, "the history was walked {walks} times");

    // At the cap the separators cannot say, and git is asked: no notice at fifty, one at fifty-one.
    for (skip, owed) in [("--skip=10", false), ("--skip=9", true)] {
        let (out, _) = rtk_log_counting_walks(&repo, &[shape, skip]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            stderr.contains("[rtk] capped at 50 commits"),
            owed,
            "{skip}: {stderr:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_built_in_format_that_closes_each_commit_is_settled_like_tformat() {
    // `--pretty=reference` is one line per commit, each ended by a newline: forty-nine commits
    // are forty-nine and the run says so, where treating the newline as a separator would
    // read them as fifty and ask git.
    let repo = repo_with(60);
    for shape in ["--pretty=reference", "--oneline"] {
        let (out, walks) = rtk_log_counting_walks(&repo, &[shape, "--skip=11"]);
        assert!(out.status.success(), "rtk git log {shape} failed");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(stdout.lines().count(), 49, "{shape}: {stdout:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.trim().is_empty(), "{shape}: {stderr:?}");
        if shape == "--pretty=reference" {
            assert_eq!(walks, 1, "{shape}: the history was walked {walks} times");
        }
    }
    // At the cap git is asked, and answers.
    for (skip, owed) in [("--skip=10", false), ("--skip=9", true)] {
        let (out, _) = rtk_log_counting_walks(&repo, &["--pretty=reference", skip]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            stderr.contains("[rtk] capped at 50 commits"),
            owed,
            "{skip}: {stderr:?}"
        );
    }
}

#[test]
fn every_format_git_builds_in_ends_each_commit_with_a_newline() {
    // The classification RTK reads the length of a run by is git's behaviour, so it is checked
    // against git: one commit and three commits of each built-in format end on a newline,
    // while `format:` leaves the last commit open.
    let repo = repo_with(3);
    for name in [
        "oneline",
        "short",
        "medium",
        "full",
        "fuller",
        "reference",
        "email",
        "mboxrd",
        "raw",
    ] {
        for count in ["-1", "-3"] {
            let walk = git_stdout(&repo, &["log", count, &format!("--pretty={name}")]);
            assert!(walk.ends_with('\n'), "{name} {count}: {walk:?}");
        }
    }
    let walk = git_stdout(&repo, &["log", "-3", "--pretty=format:%h"]);
    assert!(!walk.ends_with('\n'), "{walk:?}");
    let walk = git_stdout(&repo, &["log", "-3", "--pretty=tformat:%h"]);
    assert!(walk.ends_with('\n'), "{walk:?}");
}

#[cfg(unix)]
#[test]
fn in_tee_mode_the_hint_names_the_file_and_the_line_after_the_fiftieth() {
    // The hint is `tail -n +<first hidden line>`: the file holds the whole walk in the form
    // the log shows it, so the fifty printed lines come first and the hidden ones from line 51.
    let repo = repo_with_subjects(25, |i| format!("commit {i}: {}", "long subject ".repeat(8)));
    let tee_dir = repo.home.join("tee");
    for config_dir in [
        repo.home.join(".config").join("rtk"),
        repo.home.join("Library/Application Support/rtk"),
    ] {
        std::fs::create_dir_all(&config_dir).expect("mkdir config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            "[retriever]\nmode = \"tee\"\n",
        )
        .expect("write config");
    }
    let xdg = repo.home.join(".config");
    for shape in ["--pretty=full", "--format=%H%n%s%n%an"] {
        let _ = std::fs::remove_dir_all(&tee_dir);
        let raw = git_stdout(&repo, &["log", "-50", "--no-merges", shape]);
        let shown: Vec<String> = raw.lines().map(shown_form).collect();
        assert!(shown.len() > 50, "{shape}: the walk must pass the cap");

        let out = rtk_log_with_recall_in(
            &repo,
            &[shape],
            &[
                ("XDG_CONFIG_HOME", xdg.to_str().expect("utf-8 path")),
                ("RTK_TEE_DIR", tee_dir.to_str().expect("utf-8 path")),
            ],
        );
        assert!(out.status.success(), "rtk git log {shape} failed");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines.len(), 51, "{shape}: fifty lines and the hint");
        assert_eq!(&lines[..50], &shown[..50], "{shape}: shown lines");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.trim().is_empty(),
            "{shape}: the hint is the whole announcement, got {stderr:?}"
        );

        let (first, path) = lines[50]
            .strip_prefix("[see remaining: tail -n +")
            .and_then(|rest| rest.strip_suffix(']'))
            .and_then(|rest| rest.split_once(' '))
            .unwrap_or_else(|| panic!("{shape}: not a tee hint: {:?}", lines[50]));
        let first: usize = first.parse().expect("the line `tail` starts from");
        let files: Vec<_> = std::fs::read_dir(&tee_dir)
            .expect("the tee directory")
            .map(|entry| entry.expect("entry").path())
            .collect();
        assert_eq!(files.len(), 1, "{shape}: {files:?}");
        let name = files[0].file_name().and_then(|n| n.to_str()).expect("name");
        assert!(path.ends_with(name), "{shape}: {path} is not {name}");

        let stored = std::fs::read_to_string(&files[0]).expect("read the tee file");
        let stored: Vec<&str> = stored.lines().collect();
        assert_eq!(&stored[..50], &shown[..50], "{shape}: the printed lines");
        assert_eq!(
            &stored[first - 1..],
            &shown[50..],
            "{shape}: from line {first}, which is where `tail` starts"
        );
    }
}

#[test]
fn a_multi_line_user_format_with_recall_off_says_so_in_lines() {
    // Recall is off, so nothing stores the lines and the only way back to them is git, with
    // the count set aside.
    let repo = repo_with(25);
    for shape in ["--pretty=full", "--pretty=medium", "--format=%H%n%s%n%an"] {
        let out = rtk_log_with_recall_in(&repo, &[shape], &[("RTK_RECALL", "0")]);
        assert!(out.status.success(), "rtk git log {shape} failed");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(stdout.lines().count(), 50, "{shape}: {stdout:?}");
        assert!(!stdout.contains("rtk recall"), "{shape}: {stdout:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("[rtk] capped at 50 lines; pass -n <count> for more"),
            "{shape}: {stderr:?}"
        );
        assert!(
            !recall_db(&repo).exists(),
            "{shape}: recall is off, nothing may be stored"
        );
    }
}

#[test]
fn a_multi_line_user_format_the_store_cannot_hold_says_so_in_lines() {
    // Recall is on but the store cannot be written, so there is no hint to give and the
    // lines are as unreachable as with recall off.
    let repo = repo_with(25);
    let blocker = repo.home.join("blocker");
    std::fs::write(&blocker, "a file where the store's directory should be").expect("write");
    let unusable = blocker.join("recall.db");
    let out = rtk_log_with_recall_in(
        &repo,
        &["--pretty=full"],
        &[("RTK_RECALL_DB", &unusable.to_string_lossy())],
    );
    assert!(out.status.success(), "rtk git log failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.lines().count(), 50, "{stdout:?}");
    assert!(!stdout.contains("rtk recall"), "{stdout:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[rtk] capped at 50 lines; pass -n <count> for more"),
        "{stderr:?}"
    );
}

/// A `git` that answers the cap probe as a walk with a commit past the limit and dies, with
/// `death`, in any other call after printing part of a walk.
#[cfg(unix)]
fn stub_git_dying_mid_walk(dir: &Path, death: &str) {
    stub_git(
        dir,
        &format!(
            "case \"$*\" in *--skip=*) echo 0000000000000000000000000000000000000000; exit 0;; esac\n\
             echo 'part of a walk'\n\
             echo 'fatal: simulated' >&2\n\
             {death}"
        ),
    );
}

#[cfg(unix)]
#[test]
fn a_raw_shape_walk_git_dies_in_owes_no_cap_notice() {
    // git can print part of a walk and then die. What it printed is the walk of a command git
    // did not carry out, so the cap notice may not sit beside its `fatal:` line, even though a
    // probe asked about the history would say a commit lay past the limit.
    let repo = repo_with(1);
    for death in ["exit 128", "exit 129", "kill -9 $$"] {
        let stub = tempfile::tempdir().expect("tempdir");
        stub_git_dying_mid_walk(stub.path(), death);
        let out = rtk_log_with_stub(&repo, stub.path(), &["-p"], &[]);
        assert!(!out.status.success(), "{death}: git's failure is rtk's");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("part of a walk"), "{death}: {stdout:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains("capped at"), "{death}: {stderr:?}");
    }
}

#[cfg(unix)]
#[test]
fn a_filtered_walk_git_dies_in_ends_with_gits_error_and_its_code() {
    // The filtered path has always ended a run on a nonzero exit with git's error, before it
    // renders anything: no notice, no recall entry.
    let repo = repo_with(1);
    let lines = "i=0; while [ $i -lt 120 ]; do echo \"line $i\"; i=$((i+1)); done";
    for code in [128, 129] {
        for shape in [vec![], vec!["--pretty=full"]] {
            let stub = tempfile::tempdir().expect("tempdir");
            stub_git(
                stub.path(),
                &format!("{lines}\necho 'fatal: simulated' >&2\nexit {code}"),
            );
            let _ = std::fs::remove_file(recall_db(&repo));
            let out = rtk_log_with_stub(&repo, stub.path(), &shape, &[]);
            assert_eq!(out.status.code(), Some(code), "{shape:?}: git's code");
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(stderr.contains("fatal: simulated"), "{shape:?}: {stderr:?}");
            assert!(!stderr.contains("[rtk]"), "{shape:?}: {stderr:?}");
            assert!(!recall_db(&repo).exists(), "{shape:?}: the recall store");
        }
    }
}

#[test]
fn a_line_cap_that_took_nothing_stays_silent_and_stores_nothing() {
    // Exactly the cap is not more than the cap: 25 commits of two lines each fill it and
    // lose nothing.
    let repo = repo_with(25);
    let out = rtk_log_with_recall_in(&repo, &["--format=%H%n%s"], &[]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.lines().count(), 50, "{stdout:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("[rtk]"), "fifty lines exactly: {stderr:?}");
    assert!(
        !recall_db(&repo).exists(),
        "nothing was cut, nothing stored"
    );

    // A history the line cap does not reach stays silent.
    let small = repo_with(3);
    let out = rtk_log_with_recall_in(&small, &["--pretty=full"], &[]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("rtk recall"), "three commits: {stdout:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("[rtk]"), "three commits: {stderr:?}");
    assert!(
        !recall_db(&small).exists(),
        "nothing was cut, nothing stored"
    );
}

#[test]
fn an_empty_user_format_is_not_read_as_a_short_walk() {
    // `--pretty=format:` prints a commit as no bytes at all, so the printed output is no
    // evidence of how many commits came back: fifty of them are forty-nine newlines, and
    // `--format=` is one. `--format=format:<template>` is the same format as
    // `--pretty=format:<template>`, so a template that renders empty is the same
    // forty-nine. None of them may short-circuit the probe.
    let repo = repo_with(55);
    for shape in [
        "--pretty=format:",
        "--format=",
        "--format=format:",
        "--format=format:%b",
    ] {
        let out = rtk_log(&repo.path, &repo.home, &[shape]);
        assert!(out.status.success(), "rtk git log {shape} failed");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("[rtk] capped at 50 commits"),
            "{shape}: {stderr:?}"
        );
    }
}

#[test]
fn a_filtered_output_file_still_carries_the_notice() {
    // `--output=<file>` sends the walk to a file, so RTK's stdout is empty and proves
    // nothing about how many commits came back.
    let repo = repo_with(25);
    let target = repo.path.join("out.txt");
    let redirect = format!("--output={}", target.to_string_lossy());

    let out = rtk_log(&repo.path, &repo.home, &[&redirect]);
    assert!(out.status.success(), "rtk git log {redirect} failed");
    let written = std::fs::read_to_string(&target).expect("the output file");
    assert_eq!(
        written.matches("---END---").count(),
        10,
        "the output file holds the capped walk: {written:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(NOTICE), "{stderr:?}");
}

#[test]
fn a_bare_pathspec_on_the_filtered_walk_is_capped_and_says_so() {
    // A pathspec is where the order of RTK's own flags stops being cosmetic: git refuses an
    // option that follows a non-option argument, so anything RTK adds behind one takes the
    // whole command down with it.
    let repo = repo_with(25);
    let out = rtk_log(&repo.path, &repo.home, &["f.txt"]);
    assert!(out.status.success(), "rtk git log f.txt failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(filtered_entries(&stdout), 10, "{stdout:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(NOTICE), "{stderr:?}");

    let out = rtk_log(&repo.path, &repo.home, &["--", "f.txt"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(NOTICE), "behind a --: {stderr:?}");

    // And silent where the pathspec selects fewer commits than the cap.
    let small = repo_with(3);
    for shape in [vec!["f.txt"], vec!["--", "f.txt"]] {
        let out = rtk_log(&small.path, &small.home, &shape);
        assert!(out.status.success(), "rtk git log {shape:?} failed");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains("[rtk]"), "{shape:?}: {stderr:?}");
    }
}

#[test]
fn a_probed_walk_carries_rtks_no_merges_and_the_users_pathspec() {
    // A user-chosen format leaves RTK unable to count commits in what it printed, so the
    // question goes to git -- and that question has to be asked of the same walk that ran,
    // with RTK's `--no-merges` on it and in front of the user's pathspec.
    let repo = repo_with(55);
    let out = rtk_log(&repo.path, &repo.home, &["--oneline", "f.txt"]);
    assert!(out.status.success(), "rtk git log --oneline f.txt failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.lines().count(), 50, "{stdout:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("[rtk] capped at 50 commits"), "{stderr:?}");

    // Merges RTK dropped from the command must be dropped from the question too.
    let merges = merge_heavy_repo(9);
    let target = merges.path.join("out.txt");
    let redirect = format!("--output={}", target.to_string_lossy());
    let out = rtk_log(&merges.path, &merges.home, &[&redirect]);
    assert!(out.status.success(), "rtk git log {redirect} failed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("[rtk]"),
        "ten ordinary commits of nineteen, cap of ten: {stderr:?}"
    );

    let wide = merge_heavy_repo(10);
    let target = wide.path.join("out.txt");
    let redirect = format!("--output={}", target.to_string_lossy());
    let out = rtk_log(&wide.path, &wide.home, &[&redirect]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(NOTICE),
        "eleven ordinary commits: {stderr:?}"
    );
}

#[test]
fn a_compact_output_file_still_carries_the_notice() {
    // `--output=<file>` with a format flag leaves RTK's own stdout empty, which is no
    // evidence at all about how many commits came back.
    let repo = repo_with(55);
    let target = repo.path.join("out.txt");
    let redirect = format!("--output={}", target.to_string_lossy());

    let out = rtk_log(&repo.path, &repo.home, &["--oneline", &redirect]);
    assert!(
        out.status.success(),
        "rtk git log --oneline {redirect} failed"
    );
    let written = std::fs::read_to_string(&target).expect("the output file");
    assert_eq!(
        written.lines().count(),
        50,
        "the file holds the capped walk"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("[rtk] capped at 50 commits"), "{stderr:?}");
}

#[test]
fn a_reversed_walk_keeps_its_newest_commit_and_announces_the_cap() {
    // `--reverse` flips the order after the limit has chosen the commits, so an eleventh
    // commit fetched to measure the cap arrives first and the overflow that gets dropped is
    // the tip of the branch.
    let repo = repo_with(15);
    let out = rtk_log(&repo.path, &repo.home, &["--reverse"]);
    assert!(out.status.success(), "rtk git log --reverse failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(filtered_entries(&stdout), 10, "{stdout:?}");
    assert!(
        stdout.contains("commit 14") && !stdout.contains("commit 4"),
        "the ten newest, oldest first: {stdout:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(NOTICE), "{stderr:?}");

    let small = repo_with(3);
    let out = rtk_log(&small.path, &small.home, &["--reverse"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(filtered_entries(&stdout), 3, "{stdout:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("[rtk]"), "three commits: {stderr:?}");
}

/// A history whose oldest commit quotes RTK's own entry marker in its message body.
fn repo_quoting_the_marker(commits: usize) -> Repo {
    repo_with_messages(commits, |i| {
        let mut message = vec![format!("commit {i}")];
        if i == 0 {
            message.push("---END---".to_string());
        }
        message
    })
}

#[test]
fn a_body_quoting_the_entry_marker_does_not_invent_a_cap() {
    // RTK closes every commit with a marker, so a body that quotes one splits that commit in
    // two. The stray marker leaves nothing but whitespace behind it, which is no cut.
    let exact = repo_quoting_the_marker(10);
    let out = rtk_log(&exact.path, &exact.home, &[]);
    assert!(out.status.success(), "rtk git log failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("commit 0") && stdout.contains("commit 9"),
        "all ten commits are shown: {stdout:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("[rtk]"), "nothing was cut: {stderr:?}");

    // And still loud when the history really does run past the cap.
    let over = repo_quoting_the_marker(15);
    let out = rtk_log(&over.path, &over.home, &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(NOTICE), "fifteen commits: {stderr:?}");
}

#[test]
fn a_user_format_that_prints_nothing_still_reaches_the_probe() {
    // Empty output is evidence of an empty walk for every format but one: `--format=` renders
    // fifty commits as zero bytes, so it says nothing about the walk.
    let repo = repo_with(55);
    for shape in ["--format=", "--pretty=format:", "--pretty=tformat:"] {
        let out = rtk_log(&repo.path, &repo.home, &[shape]);
        assert!(out.status.success(), "rtk git log {shape} failed");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("[rtk] capped at 50 commits"),
            "{shape}: {stderr:?}"
        );
    }

    // A format that does print something, over a filter that selects nothing, is an empty
    // walk and says so without asking git a second time.
    let out = rtk_log(&repo.path, &repo.home, &["--oneline", "-S", "nosuchneedle"]);
    assert!(out.status.success(), "rtk git log failed");
    assert!(out.stdout.iter().all(u8::is_ascii_whitespace), "no matches");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("[rtk]"), "nothing selected: {stderr:?}");
}

fn ledger_runs() -> usize {
    static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// What `rtk git <global> log <args>` printed on `repo`, with the raw side of the savings ledger
/// it left, in the tokens `rtk gain` reports.
fn ledger_input(
    repo: &Repo,
    global: &[&str],
    args: &[&str],
    env: &[(&str, &str)],
) -> (Output, usize) {
    // A ledger of this run alone: earlier runs would add to what it reads back.
    let db = repo.home.join(format!("gain-{}.sqlite", ledger_runs()));

    let mut cmd = common::rtk_command();
    cmd.arg("git")
        .args(global)
        .arg("log")
        .args(args)
        .current_dir(&repo.path);
    isolate(&mut cmd, &repo.home);
    cmd.env("RTK_DB_PATH", &db);
    for (key, value) in env {
        cmd.env(key, value);
    }
    let out = cmd.output().expect("run rtk git log");
    assert!(
        out.status.success(),
        "rtk git {global:?} log {args:?} failed"
    );

    let mut gain = common::rtk_command();
    gain.args(["gain", "--format", "json"])
        .current_dir(&repo.path);
    isolate(&mut gain, &repo.home);
    gain.env("RTK_DB_PATH", &db);
    let report = String::from_utf8_lossy(&gain.output().expect("run rtk gain").stdout).into_owned();
    let tracked = report
        .split("\"total_input\":")
        .nth(1)
        .and_then(|rest| rest.trim_start().split(',').next())
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or_else(|| panic!("no total_input in {report}"));
    (out, tracked)
}

#[test]
fn the_entry_fetched_to_measure_the_cap_is_not_counted_as_savings() {
    // RTK asks git for one entry past its limit to learn whether the limit cut. That entry is
    // never rendered, so counting it on the raw side of the ledger would credit RTK with
    // compressing bytes it asked for itself, and `rtk gain` would read high.
    let repo = repo_with(25);
    let (_, tracked) = ledger_input(&repo, &[], &[], &[]);

    // What the same walk weighs at ten entries and at eleven. One entry is worth far more
    // than the couple of bytes a relative date can drift by between the two runs.
    let weigh = |n: &str| {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(&repo.path);
        cmd.args([
            "log",
            n,
            "--no-merges",
            "--pretty=format:%h %s (%ar) <%an>%n%b%n---END---",
        ]);
        isolate(&mut cmd, &repo.home);
        let out = cmd.output().expect("run git log");
        out.stdout.len().div_ceil(4)
    };
    let (ten, eleven) = (weigh("-10"), weigh("-11"));
    assert!(ten < eleven, "the eleventh entry must weigh something");
    assert!(
        tracked < eleven,
        "the ledger counted the entry RTK only fetched to measure itself: \
         {tracked} tokens against {ten} for the ten shown and {eleven} for eleven"
    );
    assert!(
        tracked + 4 >= ten,
        "the ledger lost part of what was shown: {tracked} against {ten}"
    );
}

/// A history whose commits carry a multi-line body, which is what makes RTK's own rendering
/// larger than the raw walk and sends it down the raw fallback.
fn repo_with_bodies(commits: usize) -> Repo {
    repo_with_messages(commits, |i| {
        vec![format!("commit {i}"), "one\ntwo\nthree\nfour".to_string()]
    })
}

#[test]
fn the_raw_fallback_shows_the_walk_the_notice_describes() {
    // When filtering would cost more than it saves, RTK prints the raw run instead -- and the
    // raw run holds the entry fetched only to measure the cap. Printing eleven commits under
    // a notice that says ten contradicts the notice.
    let repo = repo_with_bodies(60);
    for shape in [vec![], vec!["--graph"]] {
        let out = rtk_log(&repo.path, &repo.home, &shape);
        assert!(out.status.success(), "rtk git log {shape:?} failed");
        let stdout = String::from_utf8_lossy(&out.stdout);
        // The entry start opens each commit in the run and is stripped from all of it.
        assert!(!stdout.contains('\u{1}'), "{shape:?}: {stdout:?}");
        // The raw fallback keeps RTK's own markers; the filtered render strips them.
        let shown = match stdout.matches("---END---").count() {
            0 => filtered_entries(&stdout),
            markers => markers,
        };
        assert_eq!(
            shown, 10,
            "{shape:?} printed {shown} entries under a cap of ten: {stdout:?}"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(NOTICE), "{shape:?}: {stderr:?}");
    }
}

#[test]
fn the_raw_fallback_keeps_what_git_printed_after_the_last_commit_shown() {
    // The summary of `-X` follows the marker of the commit it belongs to, and the fallback
    // prints the raw walk: the last commit shown keeps its summary, as it does when git is
    // asked for ten commits and nothing more.
    let repo = repo_committing(
        15,
        |i| vec![format!("d{}/f.txt", i % 3)],
        |i| vec![format!("commit {i}"), "one\ntwo\nthree\nfour".to_string()],
    );
    let flag = "-X";
    let direct = git_stdout(
        &repo,
        &[
            "log",
            "-10",
            "--no-merges",
            "--pretty=format:%h %s (%ar) <%an>%n%b%n---END---",
            flag,
        ],
    );
    let out = rtk_log(&repo.path, &repo.home, &[flag]);
    assert!(out.status.success(), "rtk git log {flag} failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.matches("---END---").count(), 10, "{stdout:?}");
    let after_last = |walk: &str| {
        walk.rsplit("---END---")
            .next()
            .unwrap_or_default()
            .to_string()
    };
    assert!(!after_last(&direct).trim().is_empty(), "{direct:?}");
    assert_eq!(
        after_last(&stdout),
        format!("{}\n", after_last(&direct)),
        "{stdout:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(NOTICE), "{stderr:?}");
}

#[test]
fn a_body_ending_on_the_entry_marker_still_announces_what_it_cost() {
    // Two markers in a row give the render an empty block, which fills a slot in its window
    // and pushes a real commit out of it: that commit is dropped, and the notice owes a word.
    let repo = repo_with_messages(10, |i| {
        let mut message = vec![format!("commit {i}")];
        if i == 5 {
            message.push("---END---".to_string());
        }
        message
    });

    let out = rtk_log(&repo.path, &repo.home, &[]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        stdout.contains("commit 0"),
        !stderr.contains("[rtk]"),
        "a commit dropped by the render and no word about it: {stdout:?} / {stderr:?}"
    );
}

#[test]
fn a_pretty_alias_is_not_read_as_an_empty_walk() {
    // `--pretty=<name>` resolves through the user's config, where it can name a template that
    // prints nothing at all. RTK cannot see that from the arguments, so an empty run under a
    // bare name is not evidence of an empty history.
    let repo = repo_with(55);
    let config = repo.home.join("gitconfig");
    std::fs::write(&config, "[pretty]\n\ttblank = tformat:\n").expect("write config");

    let mut cmd = common::rtk_command();
    cmd.args(["git", "log", "--pretty=tblank"])
        .current_dir(&repo.path);
    isolate(&mut cmd, &repo.home);
    cmd.env("GIT_CONFIG_GLOBAL", &config);
    let out = cmd.output().expect("run rtk git log");
    assert!(out.status.success(), "rtk git log --pretty=tblank failed");
    assert!(
        out.stdout.iter().all(u8::is_ascii_whitespace),
        "prints nothing"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[rtk] capped at 50 commits"),
        "an alias to an empty template: {stderr:?}"
    );
}

#[test]
fn a_user_limit_over_a_marker_quoting_body_keeps_every_commit() {
    // RTK trims the entry it fetched past its own limit back off the run. A user-set count is
    // not over-fetched, so nothing is trimmed: a marker that came from a commit message is no
    // boundary, and every commit the user asked for by number is kept.
    let repo = repo_with_messages(5, |i| {
        let mut message = vec![format!("commit {i}")];
        if i == 2 {
            message.push("---END---\ntail".to_string());
        }
        message
    });

    for shape in [vec!["-n", "5"], vec!["-5"], vec!["--max-count=5"]] {
        let out = rtk_log(&repo.path, &repo.home, &shape);
        assert!(out.status.success(), "rtk git log {shape:?} failed");
        let stdout = String::from_utf8_lossy(&out.stdout);
        for i in 0..5 {
            assert!(
                stdout.contains(&format!("commit {i}")),
                "{shape:?} lost commit {i}: {stdout:?}"
            );
        }
    }
}

#[test]
fn trailing_blank_lines_are_not_a_cut() {
    // Seventeen commits of three lines each come back as 51 lines, the last one blank. What
    // lies past the 50th is nothing, so nothing is announced and nothing is stored.
    let repo = repo_with(17);
    for shape in ["--format=%s%n%n", "--format=%h%n%s%n"] {
        let raw = git_stdout(&repo, &["log", "-50", "--no-merges", shape]);
        let raw_lines: Vec<&str> = raw.lines().collect();
        assert!(raw_lines.len() > 50, "{shape}: the walk must pass the cap");
        assert!(
            raw_lines[50..].iter().all(|l| l.trim().is_empty()),
            "{shape}: only blank lines may lie past the cap"
        );

        let out = rtk_log_with_recall_in(&repo, &[shape], &[]);
        assert!(out.status.success(), "rtk git log {shape} failed");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(!stdout.contains("rtk recall"), "{shape}: {stdout:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.trim().is_empty(), "{shape}: {stderr:?}");
        assert!(
            !recall_db(&repo).exists(),
            "{shape}: nothing was cut, nothing stored"
        );
    }
}

#[test]
fn a_blank_last_line_is_counted_among_the_hidden_ones() {
    // Each commit ends on a blank line, so the walk's last line is blank too. It is still a
    // line of the walk, and the hint counts it.
    let repo = repo_with(60);
    let shape = "--format=%h%n%s%n";
    let raw = git_stdout(&repo, &["log", "-50", "--no-merges", shape]);
    let raw_lines = raw.lines().count();
    assert!(raw.ends_with("\n\n"), "the walk must end on a blank line");

    let out = rtk_log_with_recall_in(&repo, &[shape], &[]);
    assert!(out.status.success(), "rtk git log {shape} failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let (hidden, _) = parse_recall_hint(stdout.lines().last().unwrap_or_default());
    assert_eq!(hidden, raw_lines - 50, "{stdout:?}");
}

/// Flags that make git print something after a commit: a directory summary, a stat line, a
/// patch for a line range. What follows a commit is not another commit.
const PRINTS_AFTER_A_COMMIT: &[&str] = &[
    "--dd",
    "-X",
    "--dirstat-by-file",
    "--cumulative",
    "--stat-count=1",
    "--stat-width=40",
    "--stat-name-width=20",
    "--stat-graph-width=10",
    "-L1,1:f.txt",
];

#[test]
fn text_printed_after_a_commit_is_not_a_commit_past_the_cap() {
    // Every commit touches a file in a directory of its own, so each of these flags has
    // something to print after it. Ten commits are the cap exactly and eleven are more,
    // whatever follows each of them.
    let history = |commits| {
        repo_committing(
            commits,
            |i| vec![format!("d{}/sub/f{i}.txt", i % 3), "f.txt".to_string()],
            |i| vec![format!("commit {i}")],
        )
    };
    let exact = history(10);
    let over = history(11);
    for flag in PRINTS_AFTER_A_COMMIT {
        let out = rtk_log(&exact.path, &exact.home, &[flag]);
        assert!(out.status.success(), "rtk git log {flag} failed");
        assert!(!out.stdout.is_empty(), "{flag}: nothing printed");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("[rtk]"),
            "{flag}: ten commits, cap of ten: {stderr:?}"
        );

        let out = rtk_log(&over.path, &over.home, &[flag]);
        assert!(out.status.success(), "rtk git log {flag} failed");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(NOTICE),
            "{flag}: eleven commits: {stderr:?}"
        );
    }
}

/// A history whose commit `i` rewrites f.txt to `content(i)`.
fn repo_rewriting_f_txt(commits: usize, content: impl Fn(usize) -> String) -> Repo {
    let repo = repo_committing(0, |_| Vec::new(), |_| Vec::new());
    for i in 0..commits {
        std::fs::write(repo.path.join("f.txt"), content(i)).expect("write");
        git_ok(&repo.path, &repo.home, &["add", "-A"]);
        git_ok(
            &repo.path,
            &repo.home,
            &["commit", "-q", "-m", &format!("commit {i}")],
        );
    }
    repo
}

#[test]
fn the_character_that_opens_a_commit_in_a_files_content_is_not_a_commit() {
    // `-L` prints the lines of the file after every commit, and a line may hold anything. Ten
    // commits are the cap exactly and eleven are more, whatever those lines contain and
    // wherever in a line the control character stands. `-L` puts a space ahead of a context
    // line and a minus ahead of a removed one, which are also what `--graph` draws, so a line
    // that starts with it is the case a bare character could not tell from a commit. The
    // history is longer than the range, so the last commit of the range is a change with a
    // patch of its own, printed after its marker.
    type Content = fn(usize) -> String;
    let shapes: [(&str, Content); 3] = [
        // Mid-line, in the lines that change.
        ("mid-line", |i| format!("v{i}\u{1}x\nstable\n")),
        // At the start of a line that stays put, printed as context.
        ("context", |i| format!("\u{1}a\nv{i}\n")),
        // At the start of lines that change, printed as removed and added.
        ("removed", |i| format!("\u{1}v{i}\n\u{1}w{i}\n")),
    ];
    for (shape, content) in shapes {
        let repo = repo_rewriting_f_txt(13, content);
        for (range, owed) in [("HEAD~10..HEAD", false), ("HEAD~11..HEAD", true)] {
            let out = rtk_log(&repo.path, &repo.home, &["-L1,2:f.txt", range]);
            assert!(out.status.success(), "{shape}: rtk git log -L failed");
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(
                stderr.contains(NOTICE),
                owed,
                "{shape}, {range}: {stderr:?}"
            );
        }
    }
}

#[test]
fn a_boundary_commit_is_not_a_commit_past_the_cap() {
    // `--boundary` appends the commit just outside the range to the walk, after the limit has
    // chosen the commits it counts. Ten selected commits fit the cap, whatever git adds.
    let repo = repo_with(15);
    for (range, owed) in [("HEAD~10..HEAD", false), ("HEAD~11..HEAD", true)] {
        let out = rtk_log(&repo.path, &repo.home, &["--boundary", range]);
        assert!(
            out.status.success(),
            "rtk git log --boundary {range} failed"
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(filtered_entries(&stdout), 10, "{range}: {stdout:?}");
        assert!(!stdout.contains('\u{1}'), "{range}: {stdout:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(stderr.contains(NOTICE), owed, "{range}: {stderr:?}");
    }

    // Short of the cap the boundary commit is one of the commits shown, as it always was.
    let out = rtk_log(&repo.path, &repo.home, &["--boundary", "HEAD~5..HEAD"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(filtered_entries(&stdout), 6, "{stdout:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("[rtk]"), "{stderr:?}");
}

#[test]
fn a_boundary_walk_prints_the_render_not_the_raw_run() {
    // `--boundary` makes git append the commits just outside the walk to the run RTK compares
    // its render against, and with them the render is the cheaper of the two. A run fetched
    // with one commit more names other boundary commits and leaves these out of the
    // comparison, so `--boundary` is fetched with the exact limit.
    let repo = merge_heavy_repo_with_bodies(12, Some("one\ntwo\nthree\nfour"));
    assert_eq!(native_commit_count(&repo, &["--no-merges"]), 13);
    let out = rtk_log(&repo.path, &repo.home, &["--boundary"]);
    assert!(out.status.success(), "rtk git log --boundary failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("---END---"), "the raw run: {stdout:?}");
    assert!(!stdout.contains('\u{1}'), "{stdout:?}");
    assert_eq!(filtered_entries(&stdout), 10, "{stdout:?}");
    assert_eq!(
        stdout.matches("[+1 lines omitted]").count(),
        10,
        "{stdout:?}"
    );
    assert_eq!(stdout.lines().count(), 50, "{stdout:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(NOTICE), "{stderr:?}");

    // A cap that took nothing is silent under `--boundary` as well.
    let short = merge_heavy_repo_with_bodies(9, Some("one\ntwo\nthree\nfour"));
    assert_eq!(native_commit_count(&short, &["--no-merges"]), 10);
    let out = rtk_log(&short.path, &short.home, &["--boundary"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("[rtk]"), "{stderr:?}");
}

#[test]
fn the_character_that_opens_a_commit_in_a_message_is_left_as_it_is() {
    // A message line may start with it, or hold it further in. Neither is a commit, and
    // neither is stripped.
    let history = |commits| {
        repo_with_messages(commits, |i| {
            vec![
                format!("commit {i}"),
                "first\u{1}x\n\u{1}second".to_string(),
            ]
        })
    };
    let exact = history(10);
    let out = rtk_log(&exact.path, &exact.home, &[]);
    assert!(out.status.success(), "rtk git log failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(filtered_entries(&stdout), 10, "{stdout:?}");
    assert!(stdout.contains("first\u{1}x"), "{stdout:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("[rtk]"),
        "ten commits, cap of ten: {stderr:?}"
    );

    let over = history(11);
    let out = rtk_log(&over.path, &over.home, &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(NOTICE), "eleven commits: {stderr:?}");
}

#[test]
fn a_nul_separated_walk_in_rtks_format_is_cut_at_its_commits() {
    // `-z` ends a record with a NUL where the format would put a newline, and the commit that
    // opens past the limit is still the cut. The character that opened it is not printed.
    for (commits, owed) in [(10, false), (11, true)] {
        let repo = repo_with(commits);
        let out = rtk_log(&repo.path, &repo.home, &["-z"]);
        assert!(out.status.success(), "rtk git log -z failed");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(!stdout.contains('\u{1}'), "{commits}: {stdout:?}");
        assert_eq!(stdout.matches("commit ").count(), 10, "{commits}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(stderr.contains(NOTICE), owed, "{commits}: {stderr:?}");
    }
}

/// The sizes on the `log size <n>` lines of `output`, in order.
fn log_sizes(output: &str) -> Vec<usize> {
    output
        .lines()
        .filter_map(|line| line.strip_prefix("log size "))
        .map(|size| size.trim().parse().expect("a size"))
        .collect()
}

#[test]
fn a_log_size_run_reports_the_size_of_the_message_git_renders() {
    // git counts the bytes of each message it renders and prints the count ahead of it. The
    // sizes must be those of the format the user sees, whatever RTK asked git for to learn
    // whether the cap cut.
    let repo = repo_with(11);
    let out = rtk_log(&repo.path, &repo.home, &["--log-size"]);
    assert!(out.status.success(), "rtk git log --log-size failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains('\u{1}'), "{stdout:?}");
    let native = git_stdout(
        &repo,
        &[
            "log",
            "-10",
            "--no-merges",
            "--log-size",
            "--pretty=format:%h %s (%ar) <%an>%n%b%n---END---",
        ],
    );
    let (shown, expected) = (log_sizes(&stdout), log_sizes(&native));
    assert_eq!(shown.len(), 10, "{stdout:?}");
    assert_eq!(shown.len(), expected.len(), "{native:?}");
    for (shown, expected) in shown.iter().zip(&expected) {
        // An age may tick over to another width between the two runs. An opening is 43 bytes.
        assert!(shown.abs_diff(*expected) < 10, "{shown} against {expected}");
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(NOTICE), "eleven commits: {stderr:?}");

    let exact = repo_with(10);
    let out = rtk_log(&exact.path, &exact.home, &["--log-size"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("[rtk]"),
        "ten commits, cap of ten: {stderr:?}"
    );
}

/// `rtk git log <args>` with `input` piped to its standard input.
fn rtk_log_fed(repo: &Repo, args: &[&str], input: &str) -> Output {
    use std::io::Write;
    use std::process::Stdio;
    let mut cmd = common::rtk_command();
    cmd.arg("git").arg("log").args(args).current_dir(&repo.path);
    isolate(&mut cmd, &repo.home);
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk git log");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("run rtk git log")
}

#[test]
fn a_piped_stdin_does_not_change_which_walk_the_cap_is_measured_on() {
    // Both of RTK's git runs have no standard input, so `--stdin` reads nothing and the walk
    // is the history's own. The probe asking about it must not be given the revisions the user
    // piped: a range of one commit would answer that nothing lay past the cap.
    let repo = repo_with(12);
    for shape in [vec!["--stdin"], vec!["-p", "--stdin"]] {
        let plain = rtk_log(&repo.path, &repo.home, &shape);
        assert!(plain.status.success(), "rtk git log {shape:?} failed");
        for input in ["HEAD~2..HEAD~1\n", ""] {
            let out = rtk_log_fed(&repo, &shape, input);
            assert!(out.status.success(), "{shape:?} failed on {input:?}");
            assert_eq!(
                with_ages_masked(&out.stdout),
                with_ages_masked(&plain.stdout),
                "{shape:?}, {input:?}"
            );
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(stderr.contains(NOTICE), "{shape:?}, {input:?}: {stderr:?}");
        }
    }
}

/// What `git <global> log -<limit> --no-merges <args>` prints in the format RTK asks for: the
/// walk the limit alone gives.
fn limit_alone(repo: &Repo, global: &[&str], args: &[&str], limit: usize) -> String {
    let limit = format!("-{limit}");
    let mut all: Vec<&str> = global.to_vec();
    all.extend([
        "log",
        limit.as_str(),
        "--no-merges",
        "--pretty=format:%h %s (%ar) <%an>%n%b%n---END---",
    ]);
    all.extend(args);
    git_stdout(repo, &all)
}

/// [`limit_alone`] in tokens.
fn limit_alone_weight(repo: &Repo, global: &[&str], args: &[&str], limit: usize) -> usize {
    limit_alone(repo, global, args, limit).len().div_ceil(4)
}

/// `walk` without the lines that carry a relative date, which tick over between two runs.
fn without_ages(walk: &str) -> String {
    walk.lines()
        .filter(|line| !line.contains(" ago) <"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `walk` with each relative date read as `(age)`, so two runs over the same commits compare
/// equal when a date ticked over between them.
fn with_ages_masked(walk: &[u8]) -> String {
    String::from_utf8_lossy(walk)
        .lines()
        .map(|line| {
            let Some(end) = line.find(" ago) <") else {
                return line.to_string();
            };
            match line[..end].rfind(" (") {
                Some(start) => format!("{} (age){}", &line[..start], &line[end + 5..]),
                None => line.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A history of `commits` commits signed with an SSH key made for it in the repository's own
/// directory, so every commit has a verdict on record and nothing of the developer's gpg or
/// ssh setup is read. `None`, with the reason printed, when `ssh-keygen` is not installed.
fn signed_repo(commits: usize) -> Option<Repo> {
    if Command::new("ssh-keygen").arg("-?").output().is_err() {
        eprintln!("skipped: ssh-keygen is not installed, so there is no signature to print");
        return None;
    }
    let repo = repo_committing(0, |_| Vec::new(), |_| Vec::new());
    let key = repo.home.join("signing-key");
    let keygen = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "t@example.com", "-f"])
        .arg(&key)
        .output()
        .expect("run ssh-keygen");
    assert!(keygen.status.success(), "ssh-keygen failed");
    let public = std::fs::read_to_string(repo.home.join("signing-key.pub")).expect("public key");
    let allowed = repo.home.join("allowed-signers");
    std::fs::write(&allowed, format!("t@example.com {public}")).expect("write signers");
    for (name, value) in [
        ("gpg.format", "ssh"),
        ("user.signingKey", key.to_str().expect("utf-8 path")),
        (
            "gpg.ssh.allowedSignersFile",
            allowed.to_str().expect("utf-8 path"),
        ),
    ] {
        git_ok(&repo.path, &repo.home, &["config", name, value]);
    }
    for i in 0..commits {
        std::fs::write(repo.path.join("f.txt"), format!("content {i}\n")).expect("write");
        git_ok(&repo.path, &repo.home, &["add", "-A"]);
        // Old and fixed dates, so a relative date reads the same in every run.
        let date = format!("--date=2020-01-01T00:{i:02}:00Z");
        let subject = format!("commit {i}");
        git_ok(
            &repo.path,
            &repo.home,
            &[
                "commit",
                "-q",
                "-S",
                date.as_str(),
                "-m",
                subject.as_str(),
                "-m",
                "one\ntwo\nthree\nfour",
            ],
        );
    }
    Some(repo)
}

#[test]
fn the_signature_of_the_commit_past_the_cap_is_not_part_of_the_walk() {
    // Git prints a commit's verdict ahead of the commit, so the one past the cap has its
    // verdict between the last commit shown and its own opening. It is not part of the ten
    // that were asked for, and the ledger must not count it as part of the raw walk.
    let Some(repo) = signed_repo(15) else {
        return;
    };
    let verdict = ["-c", "log.showSignature=true"];
    let (ten, eleven) = (
        limit_alone_weight(&repo, &verdict, &[], 10),
        limit_alone_weight(&repo, &verdict, &[], 11),
    );
    assert!(ten + 4 < eleven, "the eleventh entry must weigh something");

    type Shape<'a> = (&'a [&'a str], &'a [&'a str], &'a [(&'a str, &'a str)]);
    let environment = [
        ("GIT_CONFIG_COUNT", "1"),
        ("GIT_CONFIG_KEY_0", "log.showSignature"),
        ("GIT_CONFIG_VALUE_0", "true"),
    ];
    let shapes: [Shape; 3] = [
        (&[], &["--show-signature"], &[]),
        (&verdict, &[], &[]),
        (&[], &[], &environment),
    ];
    for (global, args, env) in shapes {
        let (out, tracked) = ledger_input(&repo, global, args, env);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(NOTICE), "{global:?} {args:?}: {stderr:?}");
        assert!(
            tracked.abs_diff(ten) <= 4,
            "{global:?} {args:?} {env:?}: the ledger counted {tracked} tokens for a walk of \
             {ten} ({eleven} with the commit past the cap)"
        );
    }
}

#[test]
fn the_bar_ahead_of_the_commit_past_the_cap_is_not_part_of_the_walk() {
    // `--show-linear-break` prints a bar ahead of a commit that is not the parent of the one
    // printed before it, which is every other commit here once `--no-merges` has taken the
    // merges out. The bar of the commit past the cap is not part of the ten asked for.
    let repo = merge_heavy_repo_with_bodies(12, Some("one\ntwo\nthree\nfour"));
    let long_bar = format!("--show-linear-break={}", "-".repeat(60));
    for flag in ["--show-linear-break", long_bar.as_str()] {
        let (ten, eleven) = (
            limit_alone_weight(&repo, &[], &[flag], 10),
            limit_alone_weight(&repo, &[], &[flag], 11),
        );
        assert!(ten + 4 < eleven, "the eleventh entry must weigh something");

        let (out, tracked) = ledger_input(&repo, &[], &[flag], &[]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(NOTICE), "{flag}: {stderr:?}");
        assert!(
            tracked.abs_diff(ten) <= 4,
            "{flag}: the ledger counted {tracked} tokens for a walk of {ten} \
             ({eleven} with the commit past the cap)"
        );
        // A bar this long makes the raw walk the cheaper of the two, so it is what is printed.
        if flag == long_bar {
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(stdout.contains("---END---"), "{flag}: {stdout:?}");
            assert_eq!(
                without_ages(&stdout),
                without_ages(&limit_alone(&repo, &[], &[flag], 10)),
                "{flag}"
            );
        }
    }
}

/// A history of ordinary commits with a criss-cross merge in the middle of it and `tail`
/// commits on top, so that the cap falls on every row of the graph drawn around the merge.
fn criss_cross_repo(tail: usize) -> Repo {
    let repo = repo_committing(0, |_| Vec::new(), |_| Vec::new());
    let (path, home) = (&repo.path, &repo.home);
    let commit = |name: &str| {
        std::fs::write(path.join(name), "content\n").expect("write");
        git_ok(path, home, &["add", "-A"]);
        git_ok(
            path,
            home,
            &["commit", "-q", "-m", name, "-m", "one\ntwo\nthree\nfour"],
        );
    };
    for i in 0..10 {
        commit(&format!("pad{i}"));
    }
    git_ok(path, home, &["checkout", "-q", "-b", "side"]);
    commit("side");
    git_ok(path, home, &["checkout", "-q", "main"]);
    commit("main");
    git_ok(path, home, &["checkout", "-q", "-b", "first"]);
    git_ok(
        path,
        home,
        &["merge", "-q", "--no-ff", "-m", "first", "side"],
    );
    git_ok(path, home, &["checkout", "-q", "side"]);
    git_ok(
        path,
        home,
        &["merge", "-q", "--no-ff", "-m", "second", "main"],
    );
    git_ok(path, home, &["checkout", "-q", "-b", "both", "first"]);
    git_ok(
        path,
        home,
        &["merge", "-q", "--no-ff", "-m", "both", "side"],
    );
    for i in 0..tail {
        commit(&format!("tail{i}"));
    }
    repo
}

#[test]
fn a_graph_over_a_criss_cross_walk_holds_the_limits_own_rows_only() {
    // The rails git draws belong to the commit they sit beside, so a run cut where the next
    // commit opens holds the rails a run asked for the limit holds, wherever the cap falls on
    // the merge. What is printed is the ten commits and the rails beside them, and the raw walk
    // the ledger records is the same.
    let repo = criss_cross_repo(7);
    for back in 0..=7 {
        let revision = format!("HEAD~{back}");
        let args = ["--graph", revision.as_str()];
        let ten = limit_alone(&repo, &[], &args, 10);
        let (out, tracked) = ledger_input(&repo, &[], &args, &[]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(NOTICE), "{revision}: {stderr:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(
            without_ages(&stdout),
            without_ages(&ten),
            "{revision}: the walk shown"
        );
        // The relative dates of ten commits tick over together, ten bytes at most.
        let weight = ten.len().div_ceil(4);
        assert!(
            tracked.abs_diff(weight) <= 3,
            "{revision}: the ledger counted {tracked} tokens for a walk of {weight}"
        );
    }
}
