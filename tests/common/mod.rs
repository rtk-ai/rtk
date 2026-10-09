//! Shared helpers for the integration tests.
//!
//! Every integration target compiles this module separately and uses only the
//! part it needs, so what one target leaves untouched is dead code there. A
//! target whose only caller sits behind `#[cfg(unix)]` uses none of it on
//! Windows, where `warnings = "deny"` would then fail the build.
#![allow(dead_code)]

/// rtk is a binary crate, so `tests/` cannot import `core::test_isolation`.
/// Its scratch directory is compiled in by path instead.
#[path = "../../src/core/test_isolation/scratch.rs"]
mod scratch;

use std::process::Command;

/// Build a `Command` for the rtk binary, isolated by [`isolate_rtk`]. Use it in
/// place of `Command::new(env!("CARGO_BIN_EXE_rtk"))`.
///
/// A spawned rtk resolves the same data directory a normal invocation would,
/// writing into the contributor's `~/.local/share/rtk/`: rows in their savings
/// history, eviction of the raw output rtk keeps for them under `tee/`, and
/// database corruption when several children write at once.
/// `core::test_isolation` fails the suite on a spawn that skips this.
pub fn rtk_command() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rtk"));
    scratch::isolate_rtk(&mut cmd);
    cmd
}

/// Isolate a child the way [`rtk_command`] does: an empty environment but for
/// what `scratch::kept` lets through, with rtk's data in this test binary's
/// scratch directory. It clears whatever the command set before and starts it
/// in a project directory of its own, so call it before setting the test's own
/// variables or directory.
pub fn isolate_rtk(cmd: &mut Command) {
    scratch::isolate_rtk(cmd);
}

/// This test binary's scratch directory, where an isolated child's data goes.
pub fn scratch_dir() -> &'static std::path::Path {
    scratch::scratch_dir()
}

/// Build a `Command` for a native tool whose output a test compares with
/// rtk's, in the environment an rtk child gets, so a setting only the
/// developer's shell exports, such as `RIPGREP_CONFIG_PATH`, reaches neither
/// side rather than one. It starts in the test's directory, not the child's,
/// so give both the same `current_dir` or absolute paths.
pub fn native_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    scratch::isolate_environment(&mut cmd);
    cmd
}

/// Run git in the environment every rtk child from [`rtk_command`] gets, with
/// no configuration of the developer's and its messages in English, where a
/// child's git passes its output through in the test's locale. It clears
/// whatever the command set before, so call it before setting the test's own
/// variables.
pub fn isolate_git(cmd: &mut Command) {
    scratch::isolate_git(cmd);
}

/// A throwaway git repository with one commit, which works with no user git
/// configuration at all. Keep the `TempDir` alive for the test.
pub fn temp_git_repo() -> tempfile::TempDir {
    scratch::temp_git_repo()
}
