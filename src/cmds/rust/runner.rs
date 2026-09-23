//! Shell-string and argv wrappers over the shared err/test command runners in core.

use crate::core::runner::{run_err_cmd, run_test_cmd};
use crate::core::tracking::args_display;
use crate::core::utils::{ChildArgExt, resolved_command};
use anyhow::Result;
use std::ffi::OsString;
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

/// A shell re-tokenizes its string on whitespace, so an argv the shell already
/// split has to reach the child as a vector rather than be joined and re-split.
fn build_argv_command(argv: &[String]) -> Command {
    let mut c = resolved_command(&argv[0]);
    c.child_args(&argv[1..]);
    c
}

/// `rtk err <cmd> [args...]` reaches us tokenized. A lone argument is a shell
/// string (`rtk err 'a && b'`) and has to keep the shell to compose.
pub fn err_uses_argv(command: &[String]) -> bool {
    command.len() > 1
}

/// Run a command via the shell and filter output to show only errors/warnings.
pub fn run_err(command: &str, verbose: u8) -> Result<i32> {
    run_err_cmd(build_shell_command(command), "err", command, "err", verbose)
}

/// Run an already-tokenized argv and filter output to show only errors/warnings.
///
/// Joining that argv back into a shell string re-split quoted arguments (#2389):
/// `rtk err sh -c 'exit 7'` reached `sh` as `-c exit 7`, which runs the one-word
/// script `exit` with `$0=7` and exits 0 — a failure reported as success. `rtk
/// proxy` already hands its argv over verbatim; this is that route with the err
/// filter on it.
pub fn run_err_argv(command: &[String], verbose: u8) -> Result<i32> {
    let argv: Vec<OsString> = command.iter().map(OsString::from).collect();
    let display = args_display(&argv);
    run_err_cmd(build_argv_command(command), "err", &display, "err", verbose)
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
