//! Argv fidelity: a pattern must reach the engine exactly as the agent typed it.
//!
//! Deliberately NOT `#![cfg(unix)]`. The failure this guards against is
//! Windows-only: Git for Windows ships MSYS builds of `grep`, and an MSYS child
//! re-parses the Windows command line and glob-expands argv, eating the
//! backslashes of a POSIX basic-regex pattern. `\[x\]` arrives as `[x]` (a
//! character class — matches almost every line) and `a\|\.b(` arrives as
//! `a|.b(` (a literal — matches nothing, reported as exit 1 "no match").
//!
//! The silent direction is the dangerous one: an agent greps to decide whether
//! a symbol exists, reads "no match", and concludes absence.
//!
//! Expectations are hardcoded from POSIX BRE semantics rather than taken from a
//! live `grep` run. A spawned baseline is not usable here: the test harness is
//! itself a native Windows process, so its own `grep` call is mangled the same
//! way and would "confirm" whatever rtk did.

use std::process::Command;

const LINES: [&str; 6] = [
    "alpha [energia] um",
    "beta energia dois",
    "gamma outro",
    "delta in_waiting",
    "call .read(x)",
    "sem parenteses",
];

/// `(pattern, line numbers a POSIX BRE engine must report)`.
const CASES: &[(&str, &[usize])] = &[
    // No backslash: matches even when argv is mangled — the control.
    ("energia", &[1, 2]),
    // Bare character class: every line has one of e/n/r/g/i/a.
    ("[energia]", &[1, 2, 3, 4, 5, 6]),
    // Literal brackets. Mangled argv degrades this into the class above.
    (r"\[energia\]", &[1]),
    // BRE alternation.
    (r"alpha\|gamma", &[1, 3]),
    // In BRE a bare `|` is literal, so this must NOT match.
    ("alpha|gamma", &[]),
    // The silent false negative: `(` is literal in BRE, the alternation is real.
    (r"in_waiting\|\.read(", &[4, 5]),
    (r"\.read(", &[5]),
    // A genuine no-match, so the shape of "found nothing" is covered too.
    ("nao_existe_mesmo", &[]),
];

fn rtk(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("LC_ALL", "C")
        .args(args)
        .output()
        .expect("rtk")
}

fn grep_available() -> bool {
    Command::new("grep")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn fixture() -> (tempfile::TempDir, String) {
    let d = tempfile::tempdir().expect("tempdir");
    let p = d.path().join("t.txt");
    std::fs::write(&p, format!("{}\n", LINES.join("\n"))).expect("write");
    let s = p.to_str().unwrap().to_string();
    (d, s)
}

fn expected_stdout(nums: &[usize]) -> String {
    nums.iter()
        .map(|n| format!("{n}:{}\n", LINES[n - 1]))
        .collect()
}

#[test]
fn escaped_bre_patterns_reach_the_engine_verbatim() {
    if !grep_available() {
        eprintln!("skipping: no `grep` on PATH");
        return;
    }
    let (_d, file) = fixture();

    for (pattern, nums) in CASES {
        let out = rtk(&["grep", "-n", pattern, &file]);
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            expected_stdout(nums),
            "wrong lines for pattern {pattern:?}"
        );
        assert_eq!(
            out.status.code(),
            Some(if nums.is_empty() { 1 } else { 0 }),
            "exit code must follow grep's convention for pattern {pattern:?}"
        );
    }
}

/// The headline failure, asserted alone so a break names itself: the pattern
/// matches two lines, and rtk must never produce the empty/exit-1 shape that an
/// agent reads as "searched, found nothing".
#[test]
fn alternation_with_literal_paren_is_not_reported_as_no_match() {
    if !grep_available() {
        eprintln!("skipping: no `grep` on PATH");
        return;
    }
    let (_d, file) = fixture();

    let out = rtk(&["grep", "-n", r"in_waiting\|\.read(", &file]);
    assert_ne!(
        out.status.code(),
        Some(1),
        "exit 1 here reads as 'no match' for a pattern that matches two lines"
    );
    assert!(
        !String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        "empty output for a matching pattern is a silent false negative"
    );
}
