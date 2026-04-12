//! ParaTest runner filter.

use super::test_output::filter_test_runner_output;
use super::utils::php_tool_command;
use crate::core::runner;
use anyhow::Result;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = php_tool_command("paratest");

    let has_no_progress = args.iter().any(|a| a == "--no-progress");
    if !has_no_progress {
        cmd.arg("--no-progress");
    }

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: paratest {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "paratest",
        &args.join(" "),
        filter_test_runner_output,
        runner::RunOptions::default(),
    )
}
