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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paratest_strips_banner_seed_and_progress() {
        // ParaTest prints its own banner and a "Random Seed:" line on top of
        // PHPUnit-style dot progress; only the result summary should survive.
        let output = "ParaTest v7.3.0 upon PHPUnit 10.5.0 by Sebastian Bergmann and contributors.\n\
                      Random Seed:   1234567890\n\
                      ..........                                        10 / 10 (100%)\n\n\
                      OK (10 tests, 25 assertions)\n";
        let filtered = filter_test_runner_output(output);
        assert!(!filtered.contains("ParaTest v7.3.0"), "got: {}", filtered);
        assert!(!filtered.contains("Random Seed:"), "got: {}", filtered);
        assert!(!filtered.contains("10 / 10 (100%)"), "got: {}", filtered);
        assert!(
            filtered.contains("OK (10 tests, 25 assertions)"),
            "got: {}",
            filtered
        );
    }

    #[test]
    fn test_paratest_keeps_failures() {
        let output = "ParaTest v7.3.0 upon PHPUnit 10.5.0\n\
                      ..F.\n\
                      There was 1 failure:\n\
                      1) App\\Tests\\UserTest::testEmail\n\
                      Failed asserting that false is true.\n";
        let filtered = filter_test_runner_output(output);
        assert!(!filtered.contains("ParaTest v7.3.0"), "got: {}", filtered);
        assert!(
            filtered.contains("App\\Tests\\UserTest::testEmail"),
            "got: {}",
            filtered
        );
        assert!(
            filtered.contains("Failed asserting that false is true."),
            "got: {}",
            filtered
        );
    }

    #[test]
    fn test_paratest_token_savings() {
        use crate::core::utils::count_tokens;

        // A realistic passing run: banner + config + many parallel-worker
        // progress lines, collapsing to a one-line summary.
        let mut output = String::from(
            "ParaTest v7.3.0 upon PHPUnit 10.5.0 by Sebastian Bergmann and contributors.\n\
             Runtime:       PHP 8.3.10\n\
             Configuration: /var/www/html/phpunit.xml\n\
             Random Seed:   1234567890\n\n",
        );
        for i in 1..=40 {
            output.push_str(&format!(
                "..........................................  {} / 40 ({}%)\n",
                i,
                i * 100 / 40
            ));
        }
        output.push_str("\nOK (400 tests, 1200 assertions)\n");

        let filtered = filter_test_runner_output(&output);
        let savings =
            100.0 - (count_tokens(&filtered) as f64 / count_tokens(&output) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "expected ≥60% savings, got {:.1}%\n{}",
            savings,
            filtered
        );
    }
}
