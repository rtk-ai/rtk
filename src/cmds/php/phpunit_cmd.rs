//! PHPUnit output filter.

use super::test_output::filter_test_runner_output;
use super::utils::php_tool_command;
use crate::core::runner;
use anyhow::Result;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = php_tool_command("phpunit");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: phpunit {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "phpunit",
        &args.join(" "),
        filter_test_runner_output,
        runner::RunOptions::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phpunit_pass_output() {
        let output = "PHPUnit 12.2.0 by Sebastian Bergmann and contributors.\n\n..............\n\nTime: 00:00.321, Memory: 12.00 MB\n\nOK (14 tests, 22 assertions)\n";
        let filtered = filter_test_runner_output(output);
        assert!(!filtered.contains("PHPUnit 12.2.0"));
        assert!(!filtered.contains(".............."));
        assert!(filtered.contains("OK (14 tests, 22 assertions)"));
    }

    #[test]
    fn test_phpunit_failure_kept() {
        let output = "..F\n\nThere was 1 failure:\n\n1) Tests\\Feature\\FooTest::it_fails\nFailed asserting that false is true.\n\nFAILURES!\nTests: 3, Assertions: 3, Failures: 1.\n";
        let filtered = filter_test_runner_output(output);
        assert!(filtered.contains("There was 1 failure"));
        assert!(filtered.contains("Failed asserting that false is true."));
        assert!(filtered.contains("Tests: 3"));
    }
}
