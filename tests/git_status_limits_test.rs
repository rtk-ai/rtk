//! `[limits] status_max_files` and `status_max_untracked` bound what `rtk git status` lists,
//! and what they leave off stays recoverable.
//!
//! An integration test because the failure it guards is a wiring one: the caps were parsed and
//! documented for a long time without anything reading them, and the formatter's own unit tests
//! pass explicit numbers, so they cannot notice the config no longer reaches it.
//!
//! Unix only: the config file is found through `$HOME`, which only Unix honours.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const RTK_BIN: &str = env!("CARGO_BIN_EXE_rtk");

/// A repo with five modified tracked files (`a1`..`a5`) and four untracked ones (`u1`..`u4`),
/// under a home directory whose rtk config holds `limits`.
struct Sandbox {
    _root: tempfile::TempDir,
    home: PathBuf,
    repo: PathBuf,
}

impl Sandbox {
    fn new(limits: &str) -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let home = root.path().join("home");
        let repo = root.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        // `dirs::config_dir` is `$XDG_CONFIG_HOME` on Linux but `~/Library/Application Support`
        // on macOS, so write the config where either would look.
        for dir in [
            home.join(".config").join("rtk"),
            home.join("Library").join("Application Support").join("rtk"),
        ] {
            std::fs::create_dir_all(&dir).expect("config dir");
            std::fs::write(dir.join("config.toml"), limits).expect("config");
        }
        let sandbox = Sandbox {
            _root: root,
            home,
            repo,
        };

        sandbox.git(&["init", "-q", "-b", "main"]);
        for i in 1..=5 {
            std::fs::write(sandbox.repo.join(format!("a{i}.txt")), "one\n").expect("write");
        }
        sandbox.git(&["add", "."]);
        sandbox.git(&["commit", "-qm", "init"]);
        for i in 1..=5 {
            std::fs::write(sandbox.repo.join(format!("a{i}.txt")), "two\n").expect("write");
        }
        for i in 1..=4 {
            std::fs::write(sandbox.repo.join(format!("u{i}.txt")), "new\n").expect("write");
        }
        sandbox
    }

    /// Pins every directory and identity the binaries resolve from the environment.
    fn isolate(&self, cmd: &mut Command) {
        let root: &Path = self.home.parent().expect("root");
        cmd.current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_DATA_HOME", self.home.join(".local").join("share"))
            .env("RTK_DB_PATH", root.join("rtk.db"))
            .env("RTK_RECALL_DB", root.join("recall.db"))
            .env("RTK_TEE_DIR", root.join("tee"))
            .env_remove("RTK_RECALL")
            .env_remove("RTK_TEE")
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_GLOBAL", self.home.join("nonexistent-global"))
            .env("GIT_CONFIG_SYSTEM", self.home.join("nonexistent-system"))
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE");
    }

    fn git(&self, args: &[&str]) {
        let mut cmd = Command::new("git");
        cmd.args(["-c", "commit.gpgsign=false"]).args(args);
        self.isolate(&mut cmd);
        let out = cmd.output().expect("run git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn rtk(&self, args: &[&str], recall: bool) -> Output {
        let mut cmd = Command::new(RTK_BIN);
        cmd.args(args);
        self.isolate(&mut cmd);
        if !recall {
            cmd.env("RTK_RECALL", "0");
        }
        let out = cmd.output().expect("run rtk");
        assert!(out.status.success(), "rtk {args:?} failed: {out:?}");
        out
    }

    fn status(&self, recall: bool) -> Vec<String> {
        lines(&self.rtk(&["git", "status"], recall))
    }
}

fn lines(out: &Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

/// A section that sets only the two status caps, the way the config is documented to be written.
const STATUS_CAPS: &str = "[limits]\nstatus_max_files = 2\nstatus_max_untracked = 1\n";

#[test]
fn the_configured_caps_bound_the_listing() {
    let sandbox = Sandbox::new(STATUS_CAPS);
    assert_eq!(
        sandbox.status(false),
        [
            "* main",
            " M a1.txt",
            " M a2.txt",
            "?? u1.txt",
            "... +6 more"
        ],
        "two tracked and one untracked entry are shown, the other six are counted"
    );
}

#[test]
fn a_larger_cap_lists_everything_it_covers() {
    let sandbox = Sandbox::new("[limits]\nstatus_max_files = 5\nstatus_max_untracked = 4\n");
    let listing = sandbox.status(false);
    assert_eq!(
        listing.len(),
        10,
        "branch + 5 tracked + 4 untracked: {listing:?}"
    );
    assert!(
        listing.iter().all(|l| !l.contains("more")),
        "nothing was cut, so there is nothing to count: {listing:?}"
    );
}

#[test]
fn what_the_caps_hide_is_recoverable() {
    let sandbox = Sandbox::new(STATUS_CAPS);
    let listing = sandbox.status(true);
    let note = listing.last().expect("a listing");
    assert!(note.starts_with("... +6 more"), "{listing:?}");

    let hash = note
        .split("rtk recall ")
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .unwrap_or_else(|| panic!("no recall hint in {note:?}"));
    let recalled = lines(&sandbox.rtk(&["recall", hash], true));
    assert_eq!(
        recalled,
        [
            " M a3.txt",
            " M a4.txt",
            " M a5.txt",
            "?? u2.txt",
            "?? u3.txt",
            "?? u4.txt"
        ],
        "recall must return exactly the entries the listing left off"
    );
}
