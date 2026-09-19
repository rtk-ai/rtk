//! Shell-string wrappers over the shared err/test command runners in core.

use crate::core::child_command::ChildCommand;
use crate::core::runner::{run_err_cmd, run_test_cmd};
use anyhow::Result;
use std::process::Command;

/// The shell receives one command *string*, not an argument vector, so
/// `ChildCommand`'s per-argument encoding must not be applied to it.
fn build_shell_command(command: &str) -> ChildCommand {
    let shell = if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    };
    ChildCommand::from(shell)
}

/// Run a command via the shell and filter output to show only errors/warnings.
pub fn run_err(command: &str, verbose: u8) -> Result<i32> {
    run_err_cmd(build_shell_command(command), "err", command, "err", verbose)
}

/// Run tests via the shell and show only failures.
pub fn run_test(command: &str, verbose: u8) -> Result<i32> {
    run_test_cmd(
        build_shell_command(command),
        "test",
        command,
        "test",
        crate::core::runner::TestEcosystem::detect(command),
        verbose,
    )
}
