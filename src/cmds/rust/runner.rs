//! Argv wrappers over the shared err/test command runners in core.

use crate::core::runner::{run_err_cmd, run_test_cmd};
use crate::core::utils::user_command;
use anyhow::Result;

/// Run a command and filter output to show only errors/warnings.
///
/// `parts` are clap trailing varargs; they are joined only for display and
/// tracking labels, never to build the child process (see [`user_command`]).
pub fn run_err(parts: &[String], verbose: u8) -> Result<i32> {
    let label = parts.join(" ");
    run_err_cmd(user_command(parts), "err", &label, "err", verbose)
}

/// Run tests and show only failures.
///
/// `parts` are clap trailing varargs; see [`run_err`] for the join caveat.
pub fn run_test(parts: &[String], verbose: u8) -> Result<i32> {
    let label = parts.join(" ");
    run_test_cmd(
        user_command(parts),
        "test",
        &label,
        "test",
        crate::core::runner::TestEcosystem::detect(&label),
        verbose,
    )
}
