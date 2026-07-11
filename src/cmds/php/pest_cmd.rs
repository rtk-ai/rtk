//! Pest test runner filter.

use super::test_output::filter_test_runner_output;
use super::utils::php_tool_command;
use crate::core::runner;
use anyhow::Result;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = php_tool_command("pest");

    let has_no_progress = args.iter().any(|a| a == "--no-progress");
    if !has_no_progress {
        cmd.arg("--no-progress");
    }

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: pest {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "pest",
        &args.join(" "),
        filter_test_runner_output,
        pest_run_options(),
    )
}

fn pest_run_options() -> runner::RunOptions<'static> {
    runner::RunOptions::stdout_only()
        .tee("pest")
        .early_exit_on_failure()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pest_failure_options_preserve_raw_diagnostics() {
        let options = pest_run_options();

        assert!(options.filter_stdout_only);
        assert!(options.skip_filter_on_failure);
        assert_eq!(options.tee_label, Some("pest"));
    }

    #[test]
    fn test_pest_filters_progress_noise() {
        let output = "Pest 5.0.0\n.....\nPASS  Tests\\Unit\\ExampleTest\n";
        let filtered = filter_test_runner_output(output);
        assert!(!filtered.contains("Pest 5.0.0"));
        assert!(!filtered.contains("....."));
        assert!(filtered.contains("PASS  Tests\\Unit\\ExampleTest"));
    }
}
