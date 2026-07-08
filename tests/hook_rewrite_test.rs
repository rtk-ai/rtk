#![cfg(unix)]

use std::process::Command;

fn rtk(args: &[&str]) -> (String, String, Option<i32>) {
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(args)
        .output()
        .expect("rtk");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code(),
    )
}

#[test]
fn hook_check_preserves_grep_regex_alternation_as_one_arg() {
    let (stdout, _stderr, code) = rtk(&[
        "hook",
        "check",
        "grep",
        "-r",
        "taper|ended|15m",
        ".",
        "--include=*.py",
    ]);

    assert_eq!(code, Some(0));
    assert_eq!(
        stdout.trim(),
        "rtk grep -r 'taper|ended|15m' . '--include=*.py'"
    );
}

#[test]
fn rewrite_multi_arg_preserves_grep_regex_alternation_as_one_arg() {
    let (stdout, _stderr, code) = rtk(&[
        "rewrite",
        "grep",
        "-rnE",
        "\\.set\\(|\\.setex\\(|\\.setnx\\(",
        "/tmp/code",
    ]);

    assert!(
        matches!(code, Some(0) | Some(3)),
        "unexpected exit code: {code:?}"
    );
    assert_eq!(
        stdout.trim(),
        "rtk grep -rnE '\\.set\\(|\\.setex\\(|\\.setnx\\(' /tmp/code"
    );
}
