//! rtk-managed shell: filters eligible commands transparently, forwards the
//! rest to the backing shell unmodified.
//!
//! This module tree is shared by two entry points:
//! - `rtk shell` — the Clap subcommand in the main `rtk` binary
//!   ([`src/main.rs`](crate)), routed here via [`dispatch`].
//! - `rtk-shell` — a distinct, minimal binary
//!   ([`src/bin/rtk_shell.rs`](crate)) that inspects `argv` itself (no Clap)
//!   and calls into [`oneshot`] or [`session`] directly.
//!
//! Submodules:
//! - [`dispatch`]: pure command-line routing/classification logic (no I/O).
//! - [`oneshot`]: `-c "<line>"` one-shot execution mode.
//! - [`session`]: persistent, interactive session mode.

pub mod dispatch;
pub mod oneshot;
pub mod session;

use anyhow::Result;

use crate::core::config::Config;

/// Entry point for the `rtk shell` Clap subcommand.
///
/// `args` are the raw trailing arguments captured by
/// `Commands::Shell { args }` in `src/main.rs`, exactly as the user typed
/// them after `rtk shell`. Mirrors `rtk-shell`'s own argv handling: a
/// leading `-c` followed by one command string means one-shot mode; no args
/// means persistent-session mode.
///
/// Returns the process exit code to propagate to the OS.
pub fn dispatch(args: &[String]) -> Result<i32> {
    let config = Config::load().map(|c| c.shell).unwrap_or_default();

    match args {
        [] => session::run(),
        [flag, line] if flag == "-c" => oneshot::run_line(line, &config, None),
        _ => anyhow::bail!(
            "rtk shell: unsupported arguments {:?}\nUsage: rtk shell [-c <command>]",
            args
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dispatch_rejects_unsupported_args() {
        let result = dispatch(&["--bogus".to_string()]);
        assert!(result.is_err());
    }

    #[test]
    fn test_dispatch_oneshot_reaches_implementation() {
        // Should reach oneshot::run_line (not the arg-parsing error) and
        // actually execute the command now that oneshot mode is implemented.
        let result = dispatch(&["-c".to_string(), "echo hi".to_string()]);
        assert!(result.is_ok());
    }
}
