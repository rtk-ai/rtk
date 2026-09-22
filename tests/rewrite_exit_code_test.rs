#![cfg(unix)]
//! Exit-code faithfulness across hook rewrites (#3492, #2974, #2735).
//!
//! Agents gate on exit status (`&&` chains, "did the tests pass?"), and hooks
//! silently replace the command with whatever `rtk rewrite` returns. So the
//! rewritten command must exit with exactly the code the raw command would
//! have — compressing the output may never change the status.

use std::path::{Path, PathBuf};
use std::process::Command;

struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        for sub in ["repo", "home", "tee"] {
            std::fs::create_dir_all(root.join(sub)).expect("mkdir");
        }
        let repo = root.join("repo");
        std::fs::write(repo.join("a.txt"), "alpha\nbeta\n").expect("write");
        for args in [
            &["init", "-q", "."][..],
            &["add", "a.txt"][..],
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "-m",
                "init",
            ][..],
        ] {
            let ok = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .env("HOME", root.join("home"))
                .status()
                .expect("git")
                .success();
            assert!(ok, "git {args:?} failed");
        }
        Sandbox { _dir: dir, root }
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    /// Isolate rtk's config, tracking DB and tee output from the developer's machine,
    /// and put the freshly built `rtk` first on PATH so rewritten commands resolve it.
    fn command(&self, program: &str) -> Command {
        let bin_dir = Path::new(env!("CARGO_BIN_EXE_rtk")).parent().unwrap();
        let path = format!(
            "{}:{}",
            bin_dir.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut cmd = Command::new(program);
        cmd.current_dir(self.repo())
            .env("PATH", path)
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("home/.config"))
            .env("RTK_DB_PATH", self.root.join("rtk.db"))
            .env("RTK_TEE_DIR", self.root.join("tee"))
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
        let out = self
            .command(env!("CARGO_BIN_EXE_rtk"))
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

fn python3_available() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Each case pairs a recognized tool (the part rtk rewrites) with a failing or
/// succeeding step, so both the tool's own status and a status flowing through
/// an `&&` chain are covered.
const CASES: &[&str] = &[
    "git status",
    "git log -1 && sh -c 'exit 9'",
    "git diff --exit-code HEAD && sh -c 'exit 7'",
    "ls /nonexistent-rtk-path",
    "ls a.txt && sh -c 'exit 13'",
    "grep -r not-present-anywhere .",
    "grep -r alpha . && sh -c 'exit 14'",
    "wc -l /nonexistent-rtk-path",
    "cat /nonexistent-rtk-path",
    "cat a.txt && sh -c 'exit 11'",
    "head -n 1 a.txt && sh -c 'exit 15'",
    "find . -name a.txt -exec false {} +",
    "true; sh -c 'exit 12'",
    "sh -c 'exit 5'",
];

const PYTHON_CASES: &[&str] = &[
    "python3 -c 'import sys; sys.exit(3)'",
    "python3 -c 'raise SystemExit(4)'",
    "git status && python3 -c 'import sys; sys.exit(8)'",
    "ls a.txt && python3 -c 'import sys; sys.exit(6)'",
];

fn cases() -> Vec<&'static str> {
    let mut all = CASES.to_vec();
    if python3_available() {
        all.extend_from_slice(PYTHON_CASES);
    }
    all
}

#[test]
fn rewritten_commands_keep_the_raw_exit_code() {
    let sb = Sandbox::new();
    let mut mismatches = Vec::new();
    for case in cases() {
        let Some(rewritten) = sb.rewrite(case) else {
            continue;
        };
        let raw = sb.sh_exit(case);
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
fn most_cases_actually_go_through_a_rewrite() {
    // Guards the test above: if the rewrite rules drift so that these commands
    // stop being rewritten, the exit-code check would silently compare `sh`
    // with itself and keep passing.
    let sb = Sandbox::new();
    let rewritten: Vec<_> = CASES.iter().filter(|c| sb.rewrite(c).is_some()).collect();
    assert!(
        rewritten.len() * 2 >= CASES.len(),
        "only {} of {} cases are rewritten; update CASES so they exercise the rewrite path: {rewritten:?}",
        rewritten.len(),
        CASES.len()
    );
}
