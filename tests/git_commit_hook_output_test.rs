//! `rtk git commit` compresses git's summary to `ok <hash>`, but git routes post-commit
//! hook output to its own stderr. Dropping stderr on the success path made every hook
//! message invisible (#1355) — post-commit notices, lint summaries and follow-up
//! instructions were easy to miss, and the user had no signal anything was dropped.
//!
//! Both halves are pinned here: hook output survives, and a plain commit still prints
//! nothing but the compact token. The second is the one that keeps the fix honest —
//! forwarding stderr is only acceptable because a clean commit leaves it empty.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const RTK_BIN: &str = env!("CARGO_BIN_EXE_rtk");

/// Env that isolates git and rtk from the developer's real config, identity and home.
fn isolate(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", home.join("nonexistent-global"))
        .env("GIT_CONFIG_SYSTEM", home.join("nonexistent-system"))
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE");
}

fn git_ok(repo: &Path, home: &Path, args: &[&str]) {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo);
    cmd.args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"]);
    cmd.args(args);
    isolate(&mut cmd, home);
    let out = cmd.output().expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn rtk_commit(repo: &Path, home: &Path, message: &str) -> Output {
    let mut cmd = Command::new(RTK_BIN);
    cmd.arg("git")
        .arg("commit")
        .arg("-m")
        .arg(message)
        .current_dir(repo);
    isolate(&mut cmd, home);
    cmd.output().expect("run rtk git commit")
}

/// stdout and stderr as one string, since rtk forwards the hook text on stderr while the
/// compact token goes to stdout.
fn combined(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

struct Repo {
    _dir: tempfile::TempDir,
    path: PathBuf,
    home: PathBuf,
}

impl Repo {
    fn write(&self, name: &str, contents: &str) {
        std::fs::write(self.path.join(name), contents).expect("write file");
    }

    /// Install a `post-commit` hook with `body` as its script source.
    fn install_post_commit_hook(&self, body: &str) {
        let hooks = self.path.join(".git").join("hooks");
        std::fs::create_dir_all(&hooks).expect("create hooks dir");
        let hook = hooks.join("post-commit");
        std::fs::write(&hook, body).expect("write hook");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))
                .expect("chmod hook");
        }
    }
}

fn new_repo() -> Repo {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&path).expect("create repo dir");
    std::fs::create_dir_all(&home).expect("create home dir");

    let repo = Repo {
        _dir: dir,
        path,
        home,
    };

    let mut init = Command::new("git");
    init.arg("-C")
        .arg(&repo.path)
        .args(["init", "-q", "-b", "main"]);
    isolate(&mut init, &repo.home);
    let out = init.output().expect("git init");
    assert!(out.status.success(), "git init failed");

    repo
}

#[test]
fn post_commit_hook_output_is_visible() {
    let repo = new_repo();
    repo.install_post_commit_hook(
        "#!/bin/sh\necho HOOK_STDOUT_visible\necho HOOK_STDERR_visible >&2\n",
    );
    repo.write("f.txt", "one\n");
    git_ok(&repo.path, &repo.home, &["add", "f.txt"]);

    let out = rtk_commit(&repo.path, &repo.home, "hooked");
    let text = combined(&out);

    assert!(
        out.status.success(),
        "rtk git commit failed: {text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.contains("HOOK_STDOUT_visible"),
        "hook stdout was swallowed: {text:?}"
    );
    assert!(
        text.contains("HOOK_STDERR_visible"),
        "hook stderr was swallowed: {text:?}"
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("ok "),
        "compact token must still be printed: {text:?}"
    );
}

/// The counterpart guarantee: forwarding stderr must not make an ordinary commit chatty.
/// Nothing but the compact token may reach either stream.
#[test]
fn plain_commit_prints_only_the_compact_token() {
    let repo = new_repo();
    repo.write("f.txt", "one\n");
    git_ok(&repo.path, &repo.home, &["add", "f.txt"]);

    let out = rtk_commit(&repo.path, &repo.home, "plain");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "rtk git commit failed: {stderr}\n{stdout}"
    );
    assert!(
        stdout.trim().starts_with("ok"),
        "expected the compact token, got: {stdout:?}"
    );
    assert_eq!(
        stdout.trim().lines().count(),
        1,
        "only the compact token belongs on stdout, got: {stdout:?}"
    );
    assert!(
        stderr.trim().is_empty(),
        "a clean commit has nothing to forward, got: {stderr:?}"
    );
}
