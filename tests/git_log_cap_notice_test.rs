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
//! where the user did -- along with the `--no-merges` RTK adds to the command and must
//! therefore also ask about, and `--exit-code`, which makes a successful `git log` exit 1.

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
    // A diff-based filter makes the probe count rather than look one past the cap, and the
    // count has to start where the user's `--skip` left off. Measured from the start of
    // history instead, a walk that printed every commit it selected claimed a cap.
    // Exactly the cap left after the skip, so the printed run settles nothing and the probe
    // has to answer.
    let repo = repo_with_a_needle(15, 15);
    assert_eq!(
        native_commit_count(&repo, &["-G", "NEEDLE", "--skip=5"]),
        10
    );

    let out = rtk_log(&repo.path, &repo.home, &["-G", "NEEDLE", "--skip=5"]);
    assert!(out.status.success(), "rtk git log failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(filtered_entries(&stdout), 10, "{stdout:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("[rtk]"),
        "all ten selected commits were shown: {stderr:?}"
    );

    // And still loud when the skip leaves more than the cap.
    assert_eq!(
        native_commit_count(&repo, &["-G", "NEEDLE", "--skip=1"]),
        14
    );
    let out = rtk_log(&repo.path, &repo.home, &["-G", "NEEDLE", "--skip=1"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(NOTICE), "fourteen selected: {stderr:?}");
}

/// A history whose commits are mostly merges: fewer ordinary commits than RTK's cap, more
/// commits overall than it.
fn merge_heavy_repo(sides: usize) -> Repo {
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
        git_ok(&path, &home, &["commit", "-qm", &format!("side {i}")]);
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
    // The filtered path adds `--no-merges` of its own, so the probe has to carry it: eleven
    // commits of which six are ordinary all fit under a cap of ten, and asking about the
    // wider walk claimed a truncation that never happened.
    // Exactly the cap in ordinary commits, nearly twice that overall: the printed run fills
    // the cap and so settles nothing, and the probe has to answer about the walk RTK ran.
    let repo = merge_heavy_repo(9);
    assert_eq!(native_commit_count(&repo, &[]), 19);
    assert_eq!(native_commit_count(&repo, &["--no-merges"]), 10);

    let out = rtk_log(&repo.path, &repo.home, &[]);
    assert!(out.status.success(), "rtk git log failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(filtered_entries(&stdout), 10, "{stdout:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("[rtk]"),
        "ten commits shown of ten selected: {stderr:?}"
    );

    // A user who asks for merges gets RTK's `--no-merges` withheld, and then the wider walk
    // is the right one to ask about.
    let wide = merge_heavy_repo(10);
    assert_eq!(native_commit_count(&wide, &["--no-merges"]), 11);
    let out = rtk_log(&wide.path, &wide.home, &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(NOTICE),
        "eleven ordinary commits: {stderr:?}"
    );

    let out = rtk_log(&wide.path, &wide.home, &["--merges"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("[rtk]"),
        "ten merges, cap of ten: {stderr:?}"
    );
}

#[test]
fn a_multi_line_user_format_is_capped_in_lines_and_says_so() {
    // RTK cannot find the commit boundaries in a shape it did not choose, so it caps lines.
    // A `--pretty` that spends five lines on a commit therefore loses commits well before
    // the fiftieth, and naming the cut in commits would be off by that factor.
    let repo = repo_with(25);
    for shape in ["--pretty=full", "--pretty=medium", "--format=%H%n%s%n%an"] {
        let out = rtk_log(&repo.path, &repo.home, &[shape]);
        assert!(out.status.success(), "rtk git log {shape} failed");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(stdout.lines().count(), 50, "{shape}: {stdout:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("[rtk] capped at 50 lines"),
            "{shape}: {stderr:?}"
        );
    }

    // Exactly the cap is not more than the cap: 25 commits of two lines each fill it and
    // lose nothing.
    let out = rtk_log(&repo.path, &repo.home, &["--format=%H%n%s"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.lines().count(), 50, "{stdout:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("[rtk]"), "fifty lines exactly: {stderr:?}");

    // A history the line cap does not reach stays silent.
    let small = repo_with(3);
    let out = rtk_log(&small.path, &small.home, &["--pretty=full"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("[rtk]"), "three commits: {stderr:?}");
}

#[test]
fn an_exit_code_run_on_the_filtered_path_keeps_its_log_and_its_notice() {
    // `--exit-code` makes a perfectly successful `git log` exit 1. Read as "git refused the
    // command", it threw away the whole filtered walk along with the notice owed on it.
    let repo = repo_with(55);
    for shape in [vec!["--exit-code"], vec!["--exit-code", "--oneline"]] {
        let out = rtk_log(&repo.path, &repo.home, &shape);
        assert_eq!(
            out.status.code(),
            Some(1),
            "{shape:?}: git's code must reach the caller"
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.lines().count() >= 10,
            "{shape:?} printed no walk: {stdout:?}"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("[rtk] capped at"), "{shape:?}: {stderr:?}");
    }
}

#[test]
fn an_empty_user_format_is_not_read_as_a_short_walk() {
    // `--pretty=format:` prints a commit as no bytes at all, so the printed output is no
    // evidence of how many commits came back: fifty of them are forty-nine newlines, and
    // `--format=` is one. Neither may short-circuit the probe.
    let repo = repo_with(55);
    for shape in ["--pretty=format:", "--format="] {
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
fn a_probed_skip_is_measured_from_where_the_user_started() {
    // The probe's counting branch, taken for a diff-based filter, writes no `--skip` of its
    // own -- but the user's is stripped from what is forwarded for the other branch's sake.
    // Counting matches from the start of history made a walk that returned everything it
    // selected claim a cap.
    let repo = repo_with_a_needle(15, 15);
    let target = repo.path.join("out.txt");
    let redirect = format!("--output={}", target.to_string_lossy());

    let out = rtk_log(
        &repo.path,
        &repo.home,
        &[&redirect, "-G", "NEEDLE", "--skip=5"],
    );
    assert!(out.status.success(), "rtk git log failed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("[rtk]"),
        "all ten selected commits were written: {stderr:?}"
    );

    let out = rtk_log(
        &repo.path,
        &repo.home,
        &[&redirect, "-G", "NEEDLE", "--skip=1"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(NOTICE), "fourteen selected: {stderr:?}");
}

#[test]
fn a_nul_separated_walk_is_not_read_as_a_short_walk() {
    // `-z` replaces the newline between commits with a NUL, so the whole capped walk arrives
    // on one line. Counting lines to decide the run was short of the cap silenced it.
    let repo = repo_with(55);
    for shape in [vec!["--pretty=format:%h", "-z"], vec!["--format=%h", "-z"]] {
        let out = rtk_log(&repo.path, &repo.home, &shape);
        assert!(out.status.success(), "rtk git log {shape:?} failed");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("[rtk] capped at 50"),
            "{shape:?}: {stderr:?}"
        );
    }
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
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&path).expect("mkdir repo");
    std::fs::create_dir_all(&home).expect("mkdir home");
    git_ok(&path, &home, &["init", "-q", "-b", "main"]);
    for i in 0..commits {
        std::fs::write(path.join("f.txt"), format!("content {i}\n")).expect("write");
        git_ok(&path, &home, &["add", "f.txt"]);
        if i == 0 {
            git_ok(
                &path,
                &home,
                &["commit", "-qm", "commit 0", "-m", "---END---"],
            );
        } else {
            git_ok(&path, &home, &["commit", "-qm", &format!("commit {i}")]);
        }
    }
    Repo {
        _dir: dir,
        path,
        home,
    }
}

#[test]
fn a_body_quoting_the_entry_marker_does_not_invent_a_cap() {
    // RTK closes every commit with a marker, so a body that quotes one splits that commit in
    // two. Counting markers rather than the blocks of content they delimit announced a cut
    // where the stray marker left nothing but whitespace behind it.
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
    // fifty commits as zero bytes. Reading that as "nothing came back" would silence the cap.
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

#[test]
fn the_entry_fetched_to_measure_the_cap_is_not_counted_as_savings() {
    // RTK asks git for one entry past its limit to learn whether the limit cut. That entry is
    // never rendered, so counting it on the raw side of the ledger would credit RTK with
    // compressing bytes it asked for itself, and `rtk gain` would read high.
    let repo = repo_with(25);
    let db = repo.path.join("gain.sqlite");

    let mut cmd = Command::new(RTK_BIN);
    cmd.args(["git", "log"]).current_dir(&repo.path);
    isolate(&mut cmd, &repo.home);
    cmd.env("RTK_DB_PATH", &db);
    assert!(
        cmd.output().expect("run rtk git log").status.success(),
        "rtk git log failed"
    );

    let mut gain = Command::new(RTK_BIN);
    gain.args(["gain", "--format", "json"])
        .current_dir(&repo.path);
    isolate(&mut gain, &repo.home);
    gain.env("RTK_DB_PATH", &db);
    let report = String::from_utf8_lossy(&gain.output().expect("run rtk gain").stdout).into_owned();
    let tracked: usize = report
        .split("\"total_input\":")
        .nth(1)
        .and_then(|rest| rest.trim_start().split(',').next())
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or_else(|| panic!("no total_input in {report}"));

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
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&path).expect("mkdir repo");
    std::fs::create_dir_all(&home).expect("mkdir home");
    git_ok(&path, &home, &["init", "-q", "-b", "main"]);
    for i in 0..commits {
        std::fs::write(path.join("f.txt"), format!("content {i}\n")).expect("write");
        git_ok(&path, &home, &["add", "f.txt"]);
        git_ok(
            &path,
            &home,
            &[
                "commit",
                "-qm",
                &format!("commit {i}"),
                "-m",
                "one\ntwo\nthree\nfour",
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
fn the_raw_fallback_shows_the_walk_the_notice_describes() {
    // When filtering would cost more than it saves, RTK prints the raw run instead -- and the
    // raw run holds the entry fetched only to measure the cap. Printing eleven commits under
    // a notice that says ten contradicts the notice.
    let repo = repo_with_bodies(60);
    for shape in [vec![], vec!["--graph"]] {
        let out = rtk_log(&repo.path, &repo.home, &shape);
        assert!(out.status.success(), "rtk git log {shape:?} failed");
        let stdout = String::from_utf8_lossy(&out.stdout);
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
fn a_body_ending_on_the_entry_marker_still_announces_what_it_cost() {
    // Two markers in a row give the render an empty block, which fills a slot in its window
    // and pushes a real commit out of it. Counting only the blocks with something in them
    // never saw that overflow.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&path).expect("mkdir repo");
    std::fs::create_dir_all(&home).expect("mkdir home");
    git_ok(&path, &home, &["init", "-q", "-b", "main"]);
    for i in 0..10 {
        std::fs::write(path.join("f.txt"), format!("content {i}\n")).expect("write");
        git_ok(&path, &home, &["add", "f.txt"]);
        if i == 5 {
            git_ok(
                &path,
                &home,
                &["commit", "-qm", "commit 5", "-m", "---END---"],
            );
        } else {
            git_ok(&path, &home, &["commit", "-qm", &format!("commit {i}")]);
        }
    }

    let mut cmd = Command::new(RTK_BIN);
    cmd.args(["git", "log"]).current_dir(&path);
    isolate(&mut cmd, &home);
    let out = cmd.output().expect("run rtk git log");
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

    let mut cmd = Command::new(RTK_BIN);
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
    // not over-fetched, so there is nothing to trim -- and trimming anyway cut at a marker
    // that came from a commit message, losing commits the user had asked for by number.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&path).expect("mkdir repo");
    std::fs::create_dir_all(&home).expect("mkdir home");
    git_ok(&path, &home, &["init", "-q", "-b", "main"]);
    for i in 0..5 {
        std::fs::write(path.join("f.txt"), format!("content {i}\n")).expect("write");
        git_ok(&path, &home, &["add", "f.txt"]);
        if i == 2 {
            git_ok(
                &path,
                &home,
                &["commit", "-qm", "commit 2", "-m", "---END---\ntail"],
            );
        } else {
            git_ok(&path, &home, &["commit", "-qm", &format!("commit {i}")]);
        }
    }

    for shape in [vec!["-n", "5"], vec!["-5"], vec!["--max-count=5"]] {
        let out = rtk_log(&path, &home, &shape);
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
