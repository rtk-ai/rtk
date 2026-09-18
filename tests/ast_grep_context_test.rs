//! Context output captured with ast-grep 0.45.3:
//! `ast-grep run -p 'make($$$)' -l typescript -C 2 context.ts`.
//! A single contiguous result has no group separator to trigger the generic
//! unparsed-output fallback, so it exercises the context truncation bug.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

const RAW: &str = include_str!("fixtures/ast_grep_context_raw.txt");

fn run(args: &[&str]) -> Output {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("output.txt"), RAW).expect("write fixture");
    let tool = dir.path().join("ast-grep");
    fs::write(&tool, "#!/bin/sh\ncat output.txt\n").expect("write fake ast-grep");
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).expect("chmod");
    let path = std::env::join_paths(std::iter::once(dir.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("join PATH");
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("ast-grep")
        .args(args)
        .current_dir(dir.path())
        .env("PATH", path)
        .env("RTK_DB_PATH", dir.path().join("rtk.db"))
        .output()
        .expect("run rtk")
}

#[test]
fn explicit_context_preserves_the_entire_result() {
    for options in [
        vec!["-C", "2"],
        vec!["-C2"],
        vec!["--context=2"],
        vec!["--context", "2"],
        vec!["-A", "2"],
        vec!["-B2"],
        vec!["--after=2"],
        vec!["--before", "2"],
    ] {
        let mut args = vec!["run", "-p", "make($$$)", "-l", "typescript"];
        args.extend(options);
        args.push("context.ts");
        let out = run(&args);
        assert!(out.status.success(), "{args:?}: {:?}", out.stderr);
        assert_eq!(out.stdout, RAW.as_bytes(), "{args:?}");
    }
}

#[test]
fn ordinary_search_still_filters() {
    let out = run(&["run", "-p", "make($$$)", "-l", "typescript", "context.ts"]);
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).expect("utf8 output");
    assert!(stdout.contains("more match line(s)"), "{stdout}");
    assert!(stdout.len() < RAW.len());
}

#[test]
fn context_like_paths_do_not_disable_filtering() {
    let out = run(&["run", "-p", "make($$$)", "-l", "typescript", "--", "-C2"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("more match line(s)"));
}
