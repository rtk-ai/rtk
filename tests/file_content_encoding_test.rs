//! #4822: on Windows, every non-UTF-8 line a wrapped program printed was
//! decoded through the console code page. That suits tool messages, but file
//! content is not in the console code page: Latin-1 `caf\xe9` came out as
//! `cafÚ` (CP850), a valid-looking character that is not in the file. File
//! content now decodes the way it does on Linux: an invalid byte becomes
//! U+FFFD, never another character.

use std::path::Path;
use std::process::Command;

mod common;

const REPLACED: &str = "caf\u{FFFD}";

fn git(repo: &Path, args: &[&str]) {
    let mut cmd = Command::new("git");
    common::isolate_git(&mut cmd);
    let status = cmd
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .current_dir(repo)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?}");
}

fn rtk(repo: &Path, args: &[&str]) -> String {
    let mut cmd = common::rtk_command();
    let out = cmd
        .env("LC_ALL", "C")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("run rtk");
    String::from_utf8(out.stdout).expect("rtk prints UTF-8")
}

/// A repo whose committed `l1.txt` holds a Latin-1 `é`, and whose working copy
/// changed that line.
fn latin1_repo() -> tempfile::TempDir {
    let dir = common::tempdir();
    git(dir.path(), &["init", "-q", "."]);
    git(dir.path(), &["config", "core.autocrlf", "false"]);
    std::fs::write(dir.path().join("l1.txt"), b"caf\xe9 line\nplain\n").unwrap();
    git(dir.path(), &["add", "l1.txt"]);
    git(dir.path(), &["commit", "-qm", "c"]);
    std::fs::write(dir.path().join("l1.txt"), b"caf\xe9 changed\nplain\n").unwrap();
    dir
}

fn assert_replaced(what: &str, out: &str) {
    assert!(out.contains(REPLACED), "{what}: {out:?}");
    assert!(
        !out.contains("caf\u{DA}") && !out.contains("caf\u{E9}"),
        "{what} turned the byte into a real character: {out:?}"
    );
}

#[test]
fn grep_keeps_a_non_utf8_file_byte_as_a_replacement() {
    let repo = latin1_repo();
    assert_replaced(
        "rtk grep",
        &rtk(repo.path(), &["grep", "-n", "caf", "l1.txt"]),
    );
}

#[test]
fn git_diff_keeps_a_non_utf8_file_byte_as_a_replacement() {
    let repo = latin1_repo();
    assert_replaced("rtk git diff", &rtk(repo.path(), &["git", "diff"]));
}

#[test]
fn git_show_keeps_a_non_utf8_file_byte_as_a_replacement() {
    let repo = latin1_repo();
    assert_replaced("rtk git show", &rtk(repo.path(), &["git", "show", "HEAD"]));
}
