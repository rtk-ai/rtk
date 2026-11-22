#![cfg(unix)]
//! Exit-code faithfulness across hook rewrites (#3492, #2974, #2735).
//!
//! Agents gate on exit status (`&&` chains, "did the tests pass?"), and hooks
//! silently replace the command with whatever `rtk rewrite` returns. So the
//! rewritten command must exit with exactly the code the raw command would
//! have — compressing the output may never change the status.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let dir = common::temp_git_repo();
        let root = dir.path().to_path_buf();
        std::fs::write(root.join("a.txt"), "alpha\nbeta\n").expect("write");
        for args in [
            &["add", "a.txt"][..],
            &["commit", "-q", "-m", "add fixture"][..],
        ] {
            let mut git = Command::new("git");
            common::isolate_git(&mut git);
            let ok = git
                .args(args)
                .current_dir(&root)
                .status()
                .expect("git")
                .success();
            assert!(ok, "git {args:?} failed");
        }
        // A tracked change, left unstaged: `git diff --exit-code` reports it and
        // `git diff --cached --exit-code` has nothing to report.
        std::fs::write(root.join("a.txt"), "alpha\nbeta\ngamma\n").expect("modify");
        Sandbox { _dir: dir, root }
    }

    fn repo(&self) -> PathBuf {
        self.root.clone()
    }

    /// Isolate rtk's config, tracking DB and tee output from the developer's machine,
    /// and put the freshly built `rtk` first on PATH so rewritten commands resolve it.
    fn command(&self, program: &str) -> Command {
        let rtk = common::rtk_command();
        let bin_dir = Path::new(rtk.get_program())
            .parent()
            .expect("rtk binary has a parent directory")
            .to_path_buf();
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let path =
            std::env::join_paths(std::iter::once(bin_dir).chain(std::env::split_paths(&inherited)))
                .expect("PATH entries join");
        let mut cmd = Command::new(program);
        common::isolate_rtk(&mut cmd);
        cmd.current_dir(self.repo())
            .env("PATH", path)
            .env("LC_ALL", "C");
        cmd
    }

    fn sh_exit(&self, script: &str) -> Option<i32> {
        self.command("sh")
            .args(["-c", script])
            .output()
            .expect("sh")
            .status
            .code()
    }

    /// `rtk rewrite` prints the rewrite and exits 0 (allow) or 3 (ask); anything
    /// else means the hook would run the command unchanged.
    fn rewrite(&self, script: &str) -> Option<String> {
        let mut cmd = common::rtk_command();
        let out = cmd
            .current_dir(self.repo())
            .env("LC_ALL", "C")
            .args(["rewrite", script])
            .output()
            .expect("rtk rewrite");
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        match out.status.code() {
            Some(0) | Some(3) if !text.is_empty() => Some(text),
            _ => None,
        }
    }
}

/// Asked in the sandbox's own environment, where the Python cases run: a
/// version manager's shim can answer for the caller's home and not for it.
fn python3_available(sb: &Sandbox) -> bool {
    sb.command("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Each case puts a recognized tool (the part rtk rewrites) alone, succeeding
/// or failing, or ahead of an `&&`, `||` or `;`, so both its own status and the
/// list's are checked. The number is the raw command's exit code; `LS_MISSING`
/// stands for the one a missing operand gets from the `ls` first on PATH.
const CASES: &[(&str, i32)] = &[
    ("git status", 0),
    ("git log -1 && sh -c 'exit 9'", 9),
    ("git log --no-such-flag", 128),
    ("git diff --exit-code", 1),
    ("git diff --exit-code HEAD && sh -c 'exit 7'", 1),
    ("git diff --cached --exit-code && sh -c 'exit 7'", 7),
    ("git show no-such-rev && sh -c 'exit 9'", 128),
    ("ls /nonexistent-rtk-path", LS_MISSING),
    ("ls /nonexistent-rtk-path && sh -c 'exit 13'", LS_MISSING),
    ("ls /nonexistent-rtk-path || sh -c 'exit 13'", 13),
    ("ls a.txt || sh -c 'exit 13'", 0),
    ("ls a.txt && sh -c 'exit 13'", 13),
    ("grep -r not-present-anywhere a.txt", 1),
    ("grep -r alpha a.txt && sh -c 'exit 14'", 14),
    ("wc -l /nonexistent-rtk-path", 1),
    ("cat /nonexistent-rtk-path", 1),
    ("cat a.txt && sh -c 'exit 11'", 11),
    ("head -n 1 a.txt && sh -c 'exit 15'", 15),
    ("find . -name a.txt -exec false {} +", 1),
    ("git status; sh -c 'exit 12'", 12),
];

/// Replaced by `cases` with the code a missing operand gets from whichever `ls`
/// is first on PATH: 2 from GNU, 1 from BSD and busybox.
const LS_MISSING: i32 = -1;

const PYTHON_CASES: &[(&str, i32)] = &[
    // Bare `python3 -c` one-liners are intentional passthrough commands.
    // These compound cases exercise a rewritten git/ls segment while keeping
    // the Python step's exit status intact.
    ("git status && python3 -c 'import sys; sys.exit(8)'", 8),
    ("ls a.txt && python3 -c 'import sys; sys.exit(6)'", 6),
];

fn cases(sb: &Sandbox) -> Vec<(&'static str, i32)> {
    let probe = sb.sh_exit("ls /nonexistent-rtk-path");
    let Some(ls_missing @ (1 | 2)) = probe else {
        panic!("ls on a missing path exited {probe:?}, expected 1 or 2");
    };
    let python = if python3_available(sb) {
        PYTHON_CASES
    } else {
        &[]
    };
    CASES
        .iter()
        .chain(python)
        .map(|&(case, code)| (case, if code == LS_MISSING { ls_missing } else { code }))
        .collect()
}

#[test]
fn rewritten_commands_keep_the_raw_exit_code() {
    let sb = Sandbox::new();
    let mut mismatches = Vec::new();
    for (case, expected) in cases(&sb) {
        let rewritten = sb
            .rewrite(case)
            .unwrap_or_else(|| panic!("case must exercise the rewrite path: {case}"));
        let raw = sb.sh_exit(case);
        assert_eq!(raw, Some(expected), "raw fixture exit changed: {case}");
        let via_rtk = sb.sh_exit(&rewritten);
        if raw != via_rtk {
            mismatches.push(format!(
                "  {case:?} exited {raw:?}, but its rewrite {rewritten:?} exited {via_rtk:?}"
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "rewrite changed the exit code:\n{}",
        mismatches.join("\n")
    );
}

#[test]
fn every_case_actually_goes_through_a_rewrite() {
    // The test above stops at the first case that is not rewritten; this one
    // reports rule drift under its own name. It only asks `rtk rewrite`, which
    // never runs a case, so it checks the Python rows without python3 too.
    let sb = Sandbox::new();
    for (case, _) in CASES.iter().chain(PYTHON_CASES) {
        assert!(
            sb.rewrite(case).is_some(),
            "case must exercise the rewrite path: {case}"
        );
    }
}
