//! Argv and explicit-shell wrappers over the shared err/test command runners in core.

use crate::core::runner::{
    TestEcosystem, run_err_cmd, run_err_not_found, run_test_cmd, run_test_not_found,
};
use crate::core::shell::{Launch, command_from_args, display_args};
use anyhow::{Context, Result};

/// Run a command and filter output to show only errors/warnings.
///
/// Arguments execute directly, preserving every boundary Clap parsed. With
/// `shell`, the single supplied script runs through that shell instead.
pub fn run_err(command: &[String], shell: Option<&str>, verbose: u8) -> Result<i32> {
    let display = display_args(command);
    match command_from_args(command, shell).context("Failed to prepare err command")? {
        Launch::Ready(cmd) => run_err_cmd(cmd, "err", &display, "err", verbose),
        Launch::NotFound(program) => Ok(run_err_not_found("err", &display, &program)),
    }
}

/// Run tests and show only failures.
///
/// Arguments execute directly, preserving every boundary Clap parsed. With
/// `shell`, the single supplied script runs through that shell instead.
pub fn run_test(command: &[String], shell: Option<&str>, verbose: u8) -> Result<i32> {
    let display = display_args(command);
    let eco = TestEcosystem::detect(&display);
    match command_from_args(command, shell).context("Failed to prepare test command")? {
        Launch::Ready(cmd) => run_test_cmd(cmd, "test", &display, "test", eco, verbose),
        Launch::NotFound(program) => Ok(run_test_not_found("test", &display, &program, eco)),
    }
}
