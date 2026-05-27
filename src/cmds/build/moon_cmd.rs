//! Filter for the moon (moonrepo) task runner.
//!
//! Strips moon's chrome (banners, hash suffixes, decoration) and routes each
//! task's stdout/stderr through the matching rtk filter for its underlying
//! command. See issue #1877.

use anyhow::Result;

use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;

/// Subcommands that should bypass filtering — structured output (DOT graphs,
/// JSON), info / setup commands, or long-running servers where filtering
/// would corrupt the stream. Verified against `moon --help` for moon 2.0.4.
const PASSTHROUGH_SUBCOMMANDS: &[&str] = &[
    "action-graph",
    "bin",
    "completions",
    "docker",
    "ext",
    "extension",
    "hash",
    "init",
    "mcp",
    "migrate",
    "project",
    "project-graph",
    "projects",
    "query",
    "setup",
    "task",
    "task-graph",
    "tasks",
    "teardown",
    "template",
    "templates",
    "toolchain",
    "upgrade",
];

/// Entry point for `rtk moon <args>`.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    // Subcommand check: if the first arg is one of the passthrough subcommands,
    // execute moon without filtering. Track usage but apply no transform.
    if let Some(first) = args.first() {
        if PASSTHROUGH_SUBCOMMANDS.contains(&first.as_str()) {
            let os_args: Vec<std::ffi::OsString> =
                args.iter().map(std::ffi::OsString::from).collect();
            return runner::run_passthrough("moon", &os_args, verbose);
        }
    }

    // For run/ci/check (and anything else not in passthrough list), we'll
    // filter in later tasks. For now: passthrough so the command works
    // end-to-end while we iterate.
    let mut cmd = resolved_command("moon");
    for arg in args {
        cmd.arg(arg);
    }
    if verbose > 0 {
        eprintln!("rtk moon: passthrough (skeleton — filtering added in later tasks)");
    }

    runner::run_filtered(
        cmd,
        "moon",
        &args.join(" "),
        // Identity filter — returns input unchanged.
        |s| s.to_string(),
        RunOptions::default(),
    )
}
