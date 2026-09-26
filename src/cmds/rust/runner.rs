//! Argv and explicit-shell wrappers over the shared err/test command runners in core.

use crate::core::runner::{
    TestEcosystem, run_err_cmd, run_err_unrunnable, run_test_cmd, run_test_unrunnable,
};
use crate::core::shell::{Launch, command_from_args, display_args, program_name, spawn_failure};
use anyhow::{Context, Result};

/// Run a command and filter output to show only errors/warnings.
///
/// Arguments execute directly, preserving every boundary Clap parsed. With
/// `shell`, the single supplied script runs through that shell instead.
pub fn run_err(command: &[String], shell: Option<&str>, verbose: u8) -> Result<i32> {
    let display = display_args(command);
    let program = program_name(command, shell);
    match command_from_args(command, shell).context("Failed to prepare err command")? {
        Launch::Ready(cmd) => match run_err_cmd(cmd, "err", &display, "err", verbose) {
            Ok(code) => Ok(code),
            // Resolution proves the name resolves, not that `execve` accepts the
            // file: a CRLF shebang or a bad binary format fails here instead.
            Err(error) => match spawn_failure(program, &error) {
                Some(outcome) => Ok(run_err_unrunnable("err", &display, &outcome, verbose)),
                None => Err(error),
            },
        },
        Launch::Unrunnable(outcome) => Ok(run_err_unrunnable("err", &display, &outcome, verbose)),
    }
}

/// Run tests and show only failures.
///
/// Arguments execute directly, preserving every boundary Clap parsed. With
/// `shell`, the single supplied script runs through that shell instead.
pub fn run_test(command: &[String], shell: Option<&str>, verbose: u8) -> Result<i32> {
    let display = display_args(command);
    let program = program_name(command, shell);
    let eco = TestEcosystem::detect(&display);
    match command_from_args(command, shell).context("Failed to prepare test command")? {
        Launch::Ready(cmd) => match run_test_cmd(cmd, "test", &display, "test", eco, verbose) {
            Ok(code) => Ok(code),
            Err(error) => match spawn_failure(program, &error) {
                Some(outcome) => Ok(run_test_unrunnable(
                    "test", &display, &outcome, eco, verbose,
                )),
                None => Err(error),
            },
        },
        Launch::Unrunnable(outcome) => Ok(run_test_unrunnable(
            "test", &display, &outcome, eco, verbose,
        )),
    }
}
