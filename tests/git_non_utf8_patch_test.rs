//! `rtk git diff` / `show` / `log -p` on a file that is not UTF-8 (#4815).
//!
//! A Latin-1 `é` is the single byte `0xE9`. Decoded into a `String`, it came back as U+FFFD
//! (`EF BF BD`), so the agent read a line that exists nowhere on disk and an edit built from
//! it failed to match. These run the real `git` and the built `rtk` on a hermetic repo and
//! check the bytes: git's `0xE9` reaches stdout and no U+FFFD is invented.

use std::path::Path;
use std::process::Output;

mod common;

const FILE: &str = "menu.txt";
const LATIN1_E_ACUTE: u8 = 0xE9;
const REPLACEMENT_CHAR: &[u8] = "\u{FFFD}".as_bytes();

fn git(repo: &Path, args: &[&str]) -> Output {
    let mut cmd = common::native_command("git");
    common::isolate_git(&mut cmd);
    cmd.args(args).current_dir(repo);
    let out = cmd.output().expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn rtk_git(repo: &Path, args: &[&str]) -> Output {
    let mut cmd = common::rtk_command();
    cmd.arg("git").args(args).current_dir(repo);
    cmd.output().expect("run rtk git")
}

/// A committed Latin-1 line, then an uncommitted change to it: `caf\xE9` -> `caf\xE9 au lait`.
fn latin1_repo() -> tempfile::TempDir {
    let dir = common::temp_git_repo();
    let repo = dir.path();
    std::fs::write(repo.join(FILE), b"caf\xE9\n").expect("write Latin-1 file");
    git(repo, &["add", FILE]);
    git(repo, &["commit", "-q", "-m", "latin1"]);
    std::fs::write(repo.join(FILE), b"caf\xE9 au lait\n").expect("rewrite Latin-1 file");
    dir
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// rtk exits 0, keeps every `0xE9` git printed and adds no U+FFFD.
fn assert_keeps_latin1_bytes(repo: &Path, args: &[&str]) {
    let native = git(repo, args);
    let rtk = rtk_git(repo, args);
    assert!(
        rtk.status.success(),
        "rtk git {args:?} failed: {}",
        String::from_utf8_lossy(&rtk.stderr)
    );
    let count = |bytes: &[u8]| bytes.iter().filter(|&&b| b == LATIN1_E_ACUTE).count();
    assert!(
        count(&native.stdout) > 0,
        "fixture: git {args:?} prints 0xE9"
    );
    assert_eq!(
        count(&rtk.stdout),
        count(&native.stdout),
        "rtk git {args:?} lost Latin-1 bytes:\n{}",
        String::from_utf8_lossy(&rtk.stdout)
    );
    assert!(
        !contains(&rtk.stdout, REPLACEMENT_CHAR),
        "rtk git {args:?} printed U+FFFD:\n{}",
        String::from_utf8_lossy(&rtk.stdout)
    );
}

#[test]
fn git_diff_keeps_latin1_bytes() {
    let dir = latin1_repo();
    assert_keeps_latin1_bytes(dir.path(), &["diff"]);
}

#[test]
fn git_diff_no_compact_is_byte_identical_to_git() {
    let dir = latin1_repo();
    let native = git(dir.path(), &["diff"]);
    let rtk = rtk_git(dir.path(), &["diff", "--no-compact"]);
    assert!(rtk.status.success());
    assert_eq!(rtk.stdout, native.stdout);
}

#[test]
fn git_show_keeps_latin1_bytes() {
    let dir = latin1_repo();
    assert_keeps_latin1_bytes(dir.path(), &["show", "HEAD"]);
}

#[test]
fn git_log_patch_keeps_latin1_bytes() {
    let dir = latin1_repo();
    assert_keeps_latin1_bytes(dir.path(), &["log", "-p"]);
}

/// The fallback is for output that is not UTF-8 only: a UTF-8 change is still compacted.
#[test]
fn utf8_diff_is_still_compacted() {
    let dir = common::temp_git_repo();
    let repo = dir.path();
    std::fs::write(repo.join(FILE), "café\n").expect("write UTF-8 file");
    git(repo, &["add", FILE]);
    git(repo, &["commit", "-q", "-m", "utf8"]);
    std::fs::write(repo.join(FILE), "café au lait\n").expect("rewrite UTF-8 file");

    let native = git(repo, &["diff"]);
    let rtk = rtk_git(repo, &["diff"]);
    assert!(rtk.status.success());
    assert_ne!(rtk.stdout, native.stdout, "UTF-8 diff should be compacted");
    assert!(contains(&rtk.stdout, "café au lait".as_bytes()));
}
