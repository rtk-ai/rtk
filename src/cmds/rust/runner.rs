//! Shell-string wrappers over the shared err/test command runners in core.

use crate::core::runner::{run_err_cmd, run_test_cmd};
use anyhow::Result;
use std::process::Command;

fn build_shell_command(command: &str) -> Command {
    if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    }
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
