//! Test-only helpers that keep integration tests independent of the machine they run on.
//!
//! Tests that spawn the built binary must not read the contributor's own repository,
//! git config or locale: a test that mutates the working repo is hostile, and one that
//! matches a tool's English wording either fails or silently stops asserting anything in
//! another language.

use std::path::PathBuf;
use std::process::Command;

/// Path to the freshly built debug binary. Panics with the fix if it isn't there.
pub fn rtk_bin() -> PathBuf {
    let bin = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("debug")
        .join("rtk");
    assert!(bin.exists(), "Run `cargo build` first: {}", bin.display());
    bin
}

/// The binary, with the environment pinned: `LC_ALL`/`LANGUAGE` so a child tool's messages
/// are the ones the assertions were written against, and `GIT_CONFIG_*` so the
/// contributor's own git config (aliases, `init.defaultBranch`, custom pagers) can't reach
/// into the run.
pub fn rtk_command() -> Command {
    // Test-only, and the path is this crate's own freshly built binary, not user input.
    // nosemgrep: dynamic-command-execution
    let mut cmd = Command::new(rtk_bin());
    pin_environment(&mut cmd);
    cmd
}

/// Same pinning for a directly spawned tool, so both sides of a comparison speak one
/// language.
pub fn pin_environment(cmd: &mut Command) -> &mut Command {
    cmd.env("LC_ALL", "C")
        .env("LANGUAGE", "")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
}

/// A throwaway git repo with one commit, its identity set locally so it works with no user
/// git config at all. The `TempDir` deletes it on drop, so keep it alive for the test.
pub fn temp_git_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "rtk-test@example.invalid"][..],
        &["config", "user.name", "rtk test"][..],
        &["config", "commit.gpgsign", "false"][..],
        &["commit", "-q", "--allow-empty", "-m", "init"][..],
    ] {
        let ok = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false);
        assert!(ok, "git setup failed: {args:?}");
    }
    dir
}
