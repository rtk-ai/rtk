#![cfg(unix)]
//! Two `rtk find` defects, one file.
//!
//! A: `rtk find --help` / `-h` were forwarded to the OS `find`, which has no
//! such flag, so asking for usage printed `illegal option` and exited 1.
//!
//! B: a match under `.git` was dropped by disclosure without a word, so output
//! was empty and exit 0 — the same shape as "no matches", which is a lie the
//! caller cannot detect.

use std::process::Command;

mod common;

fn rtk() -> Command {
    let mut cmd = common::rtk_command();
    cmd.env("LC_ALL", "C");
    cmd
}

fn run(args: &[&str]) -> (Option<i32>, String, String) {
    let out = rtk().args(args).output().unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `rtk help find` works and is what `--help` should print.
#[test]
fn help_flag_prints_own_usage_instead_of_running_native_find() {
    let (_, reference, _) = run(&["help", "find"]);
    assert!(
        reference.contains("Usage: rtk find"),
        "`rtk help find` is the reference text:\n{reference}"
    );

    for flag in ["--help", "-h"] {
        let (code, stdout, stderr) = run(&["find", flag]);
        assert_eq!(
            code,
            Some(0),
            "`rtk find {flag}` must exit 0, got {code:?}\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            stdout.contains("Usage: rtk find"),
            "`rtk find {flag}` must print find's own usage:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            !stderr.contains("illegal option"),
            "`rtk find {flag}` reached the OS find instead of answering itself:\n{stderr}"
        );
        assert!(
            stderr.trim().is_empty(),
            "asking for help is not an error:\n{stderr}"
        );
    }
}

/// A match that disclosure suppresses must still be reported.
#[test]
fn a_match_inside_git_is_not_reported_as_no_match() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("X");
    std::fs::create_dir_all(root.join(".git").join("objects")).unwrap();
    std::fs::write(root.join(".git").join("objects").join("grill-me.md"), "x").unwrap();

    let (code, stdout, stderr) = run(&["find", root.to_str().unwrap(), "-iname", "*grill*"]);
    assert_eq!(code, Some(0), "stderr:\n{stderr}");
    assert!(
        stdout.contains("skipped"),
        "a match existed but was skipped; the output must not read as \"no match\":\nstdout: {stdout:?}\nstderr: {stderr:?}"
    );
    assert!(
        stdout.contains(".git"),
        "the note must name where the match was skipped:\n{stdout}"
    );
    assert!(
        stdout.contains('1'),
        "the note must count the skipped match:\n{stdout}"
    );
}

/// The note rides along with ordinary results instead of replacing them.
#[test]
fn the_skipped_match_note_does_not_replace_real_results() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Y");
    std::fs::create_dir_all(root.join(".git").join("objects")).unwrap();
    std::fs::write(root.join(".git").join("objects").join("grill-me.md"), "x").unwrap();
    std::fs::write(root.join("grill-visible.md"), "x").unwrap();

    let (code, stdout, stderr) = run(&["find", root.to_str().unwrap(), "-iname", "*grill*"]);
    assert_eq!(code, Some(0), "stderr:\n{stderr}");
    assert!(
        stdout.contains("grill-visible.md"),
        "the visible match must survive:\n{stdout}"
    );
    assert!(
        stdout.contains("skipped"),
        "the skipped match must still be disclosed:\n{stdout}"
    );
}
