//! End-to-end proof of the never-worse guard (src/core/guard.rs).

use std::io::Write;
use std::process::{Command, Stdio};

fn rtk_stdin(args: &[&str], input: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn rtk");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait rtk");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn guard_shows_raw_when_filter_would_bloat_tiny_input() {
    let input = "{\"a\":1,\"b\":2,\"c\":3,\"d\":4}";
    let out = rtk_stdin(&["json", "-"], input);

    assert_eq!(
        out.trim(),
        input,
        "guard should emit the raw minified JSON, not a larger pretty-printed form"
    );
    assert!(
        out.trim().len() <= input.len(),
        "never-worse violated: {} chars emitted for a {}-char raw input",
        out.trim().len(),
        input.len()
    );
}

#[test]
fn guard_does_not_block_real_compression() {
    let mut input = String::from("{");
    for i in 0..60 {
        input.push_str(&format!("\"key_{i}\":\"value_{i}\","));
    }
    input.push_str("\"last\":1}");

    let out = rtk_stdin(&["json", "-"], &input);
    assert!(
        out.len() < input.len(),
        "filter should compress large input (guard must not over-trigger): {} vs {}",
        out.len(),
        input.len()
    );
}

fn rtk_in_dir(dir: &std::path::Path, args: &[&str]) -> (String, Option<i32>) {
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(args)
        .current_dir(dir)
        .stderr(Stdio::null())
        .output()
        .expect("spawn rtk");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code(),
    )
}

fn rg_available() -> bool {
    Command::new("rg")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn init_git_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for args in [
        &["init", "-q"][..],
        &["config", "user.email", "t@t.t"][..],
        &["config", "user.name", "t"][..],
        &["commit", "-q", "--allow-empty", "-m", "init"][..],
    ] {
        let ok = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(ok, "git setup failed: {args:?}");
    }
    dir
}

#[test]
fn grep_no_match_emits_empty_not_a_message() {
    if !rg_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.txt"), "hello world\n").expect("write");
    // Faithful grep needs -r to descend a directory; we no longer force recursion
    // by routing through rg (the engine-faithful contract).
    let (out, code) = rtk_in_dir(dir.path(), &["grep", "-r", "zzz_no_match_xyz", "."]);
    assert!(
        out.trim().is_empty(),
        "no-match grep must emit empty, not a '0 matches' line: {out:?}"
    );
    assert_eq!(code, Some(1), "grep no-match must preserve exit 1");
}

#[test]
fn find_no_results_emits_empty() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.txt"), "x").expect("write");
    let (out, _) = rtk_in_dir(dir.path(), &["find", ".", "-name", "zzz_no_match_xyz"]);
    assert!(
        out.trim().is_empty(),
        "no-result find must emit empty, not a '0 for' line: {out:?}"
    );
}

#[test]
fn git_stash_list_no_stashes_emits_empty() {
    let dir = init_git_repo();
    let (out, code) = rtk_in_dir(dir.path(), &["git", "stash", "list"]);
    assert!(
        out.trim().is_empty(),
        "no-stashes must emit empty, not 'No stashes': {out:?}"
    );
    assert_eq!(code, Some(0));
}

#[test]
fn git_stash_show_no_stash_emits_empty_and_propagates_failure() {
    // Regression: previously printed "Empty stash" and returned Ok(0), masking
    // the underlying `git stash show` failure.
    let dir = init_git_repo();
    let (out, code) = rtk_in_dir(dir.path(), &["git", "stash", "show"]);
    assert!(
        out.trim().is_empty(),
        "must emit empty, not 'Empty stash': {out:?}"
    );
    assert_ne!(
        code,
        Some(0),
        "a real git stash show failure must not be masked as exit 0"
    );
}
