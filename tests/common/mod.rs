//! Shared helpers for the integration tests.

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
