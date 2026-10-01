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

/// Build a `Command` for the rtk binary with its tracking database and tee
/// spool redirected to this test binary's scratch directory. Use it in place
/// of `Command::new(env!("CARGO_BIN_EXE_rtk"))`.
///
/// A spawned rtk resolves the same data directory a normal invocation would,
/// writing into the contributor's `~/.local/share/rtk/`: rows in their savings
/// history, eviction of the raw output rtk keeps for them under `tee/`, and
/// database corruption when several children write at once.
/// `core::test_isolation` fails the suite on a spawn that skips this.
pub fn rtk_command() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rtk"));
    scratch::redirect_rtk_data(&mut cmd);
    cmd
}

/// Run git isolated from the developer's configuration and repository
/// variables, as every rtk child from [`rtk_command`] runs it, and with its
/// messages in English, which a child's git is not.
pub fn isolate_git(cmd: &mut Command) {
    scratch::isolate_git(cmd);
}

/// A throwaway git repository with one commit, which works with no user git
/// configuration at all. Keep the `TempDir` alive for the test.
pub fn temp_git_repo() -> tempfile::TempDir {
    scratch::temp_git_repo()
}
