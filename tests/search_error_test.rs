#![cfg(unix)]
//! Error/exit faithfulness: rtk surfaces the engine's own stderr and propagates
//! its exit code, adding nothing (no synthetic "search failed" line). An engine
//! error (exit >=2) must never look like a silent no-match (#2465 / #1436).

use std::process::Command;

fn rtk(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(args)
        .output()
        .expect("rtk")
}

fn grep_exit(args: &[&str]) -> Option<i32> {
    Command::new("grep")
        .args(args)
        .output()
        .expect("grep")
        .status
        .code()
}

fn rg_available() -> bool {
    Command::new("rg")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn write_temp(content: &str) -> (tempfile::TempDir, String) {
    let d = tempfile::tempdir().expect("tempdir");
    let p = d.path().join("ok.txt");
    std::fs::write(&p, content).expect("write");
    let s = p.to_str().unwrap().to_string();
    (d, s)
}

#[test]
fn file_not_found_propagates_exit_and_surfaces_error() {
    let d = tempfile::tempdir().unwrap();
    let missing = d.path().join("nope.txt");
    let m = missing.to_str().unwrap();
    let out = rtk(&["grep", "fn", m]);
    assert_eq!(
        out.status.code(),
        grep_exit(&["fn", m]),
        "exit must match grep"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("No such file"),
        "the engine's error must be surfaced:\n{stderr}"
    );
    assert!(
        !stderr.contains("search failed"),
        "no synthetic line:\n{stderr}"
    );
}

#[test]
fn bad_regex_surfaces_error_never_silent_no_match() {
    let (_d, f) = write_temp("fn alpha\n");
    let out = rtk(&["grep", "[", &f]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "a bad regex must propagate exit 2, not a silent exit-1 no-match"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.is_empty(), "the engine error must surface");
    assert!(
        !stderr.contains("search failed"),
        "nothing added:\n{stderr}"
    );
}

#[test]
fn partial_match_with_error_surfaces_both() {
    let (_d, f) = write_temp("fn alpha\nfn beta\n");
    let d2 = tempfile::tempdir().unwrap();
    let missing = d2.path().join("nope.txt");
    let out = rtk(&["grep", "fn", &f, missing.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2), "the error exit must propagate");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("No such file"),
        "the file error must surface alongside the matches"
    );
    assert!(
        !out.stdout.is_empty(),
        "the matches from the readable file must still be shown"
    );
}

#[test]
fn no_match_stays_exit_1_and_empty() {
    let (_d, f) = write_temp("fn alpha\n");
    let out = rtk(&["grep", "zzzz_no_match", &f]);
    assert_eq!(out.status.code(), Some(1), "no-match is exit 1");
    assert!(
        out.stdout.is_empty() && out.stderr.is_empty(),
        "no-match emits nothing"
    );
}

#[test]
fn rg_bad_regex_surfaces_error_exit_2() {
    if !rg_available() {
        return;
    }
    let (_d, f) = write_temp("GET /objects/{path}\n");
    let out = rtk(&["rg", "a|{path}", &f]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "rg's regex parse error must propagate exit 2"
    );
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("search failed"),
        "nothing added"
    );
}
