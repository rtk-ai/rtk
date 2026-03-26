use crate::core::tracking;
use crate::core::utils::{resolved_command, tool_exists};
use anyhow::{Context, Result};

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    // Priority: global phpunit → Symfony bin/phpunit → Composer vendor/bin/phpunit
    let phpunit_script = if std::path::Path::new("bin/phpunit").exists() {
        "bin/phpunit"
    } else {
        "vendor/bin/phpunit"
    };
    let mut cmd = if tool_exists("phpunit") {
        resolved_command("phpunit")
    } else {
        let mut c = resolved_command("php");
        c.arg(phpunit_script);
        c
    };

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: phpunit {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run phpunit. Is it installed?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    if raw.to_lowercase().contains("segmentation fault") {
        println!("💥 PHPUnit crashed (segfault)");
        std::process::exit(1);
    }

    if raw.to_lowercase().contains("fatal error") {
        println!("{}", extract_php_fatal(&raw));
        std::process::exit(1);
    }

    let filtered = filter_phpunit_output(&stdout);

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });
    if let Some(hint) = crate::core::tee::tee_and_hint(&raw, "phpunit", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    if !stderr.trim().is_empty() {
        eprintln!("{}", stderr.trim());
    }

    timer.track(
        &format!("phpunit {}", args.join(" ")),
        &format!("rtk phpunit {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

fn extract_php_fatal(output: &str) -> String {
    let mut result = String::from("💥 PHP Fatal Error\n");

    for line in output.lines() {
        if line.contains("Fatal error") {
            result.push_str(line.trim());
            break;
        }
    }

    result
}

fn filter_phpunit_output(output: &str) -> String {
    let mut failures: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut in_failures = false;

    for line in output.lines() {
        let trimmed = line.trim();

        // Standard success: "OK (X tests, Y assertions)"
        if trimmed.starts_with("OK (") {
            return format!("PHPUnit: {}", trimmed);
        }

        // Success with skipped/risky/incomplete: "OK, but incomplete, skipped, or risky tests!"
        if trimmed.starts_with("OK, but") {
            return build_success_with_skipped(output);
        }

        // "There was 1 failure:" / "There were N failures:" / errors variant
        if (trimmed.starts_with("There was") || trimmed.starts_with("There were"))
            && (trimmed.contains("failure") || trimmed.contains("error"))
        {
            in_failures = true;
            continue;
        }

        // FAILURES! or ERRORS! marks end of failure details
        if trimmed == "FAILURES!" || trimmed == "ERRORS!" {
            if !current.is_empty() {
                failures.push(std::mem::take(&mut current));
            }
            in_failures = false;
            continue;
        }

        if in_failures {
            // New numbered failure: "1) TestClass::testMethod"
            if trimmed
                .chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
                && trimmed.contains(')')
            {
                if !current.is_empty() {
                    failures.push(std::mem::take(&mut current));
                }
                current.push(trimmed.to_string());
            } else if !trimmed.is_empty() {
                current.push(trimmed.to_string());
            }
        }
    }

    if failures.is_empty() {
        // No explicit OK line and no failures: show counts from summary line if available
        let (tests, assertions, _, _) = parse_counts(output);
        if tests > 0 {
            return format!("PHPUnit: {} tests, {} assertions", tests, assertions);
        }
        return "PHPUnit: OK".to_string();
    }

    build_phpunit_summary(output, &failures)
}

fn build_success_with_skipped(output: &str) -> String {
    let (tests, assertions, _, skipped) = parse_counts(output);
    if skipped > 0 {
        format!(
            "PHPUnit: {} tests, {} assertions, {} skipped",
            tests, assertions, skipped
        )
    } else {
        format!("PHPUnit: {} tests, {} assertions", tests, assertions)
    }
}

fn build_phpunit_summary(output: &str, failures: &[Vec<String>]) -> String {
    let (tests, assertions, failures_count, _skipped) = parse_counts(output);

    let mut result = format!(
        "PHPUnit: {} tests, {} assertions, {} failures\n",
        tests, assertions, failures_count
    );
    result.push_str("═══════════════════════════════════════\n");
    result.push_str("Failures:\n");

    for failure_lines in failures.iter().take(10) {
        if let Some(first) = failure_lines.first() {
            result.push_str(&format!("{}\n", first));
        }
        // Show up to 2 detail lines (error message + file location)
        for detail in failure_lines.iter().skip(1).take(2) {
            result.push_str(&format!("   {}\n", detail));
        }
    }

    if failures.len() > 10 {
        result.push_str(&format!("... +{} more\n", failures.len() - 10));
    }

    result.trim().to_string()
}

fn parse_counts(output: &str) -> (usize, usize, usize, usize) {
    let mut tests = 0;
    let mut assertions = 0;
    let mut failures = 0;
    let mut skipped = 0;

    for line in output.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("Tests:") {
            continue;
        }

        // "Tests: 10, Assertions: 20, Failures: 1." or "Tests: 9, Assertions: 15, Skipped: 2."
        for part in trimmed.split(',') {
            let mut it = part.split_whitespace();
            let key = it.next().unwrap_or("");
            let val = it
                .next()
                .unwrap_or("")
                .trim_end_matches('.')
                .parse()
                .unwrap_or(0);

            match key {
                "Tests:" => tests = val,
                "Assertions:" => assertions = val,
                k if k.starts_with("Failures") || k.starts_with("Errors") => failures += val,
                k if k.starts_with("Skipped") => skipped = val,
                _ => {}
            }
        }
    }

    (tests, assertions, failures, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real PHPUnit output: FAILURES! comes AFTER the failure details.
    // Uses a large-enough fixture to verify meaningful token savings.
    const REAL_PHPUNIT_FAILURE: &str = r#"PHPUnit 10.5.0 by Sebastian Bergmann and contributors.

Runtime:       PHP 8.2.27 with Xdebug 3.3.1
Configuration: /var/www/html/phpunit.xml

........................................          40 / 40 (100%)
..................................................  80 / 80 (100%)
.F................................................  100 / 100 (100%)
..........                                         110 / 110 (100%)

Time: 00:01:23.456, Memory: 48.00 MB

There was 1 failure:

1) App\Tests\UserTest::testEmailValidation
Failed asserting that false is true.

#0 /var/www/html/src/User.php:142 (App\User::validate)
#1 /var/www/html/tests/UserTest.php:38 (App\Tests\UserTest::testEmailValidation)

FAILURES!
Tests: 110, Assertions: 340, Failures: 1."#;

    const REAL_PHPUNIT_SUCCESS: &str = r#"PHPUnit 10.5.0 by Sebastian Bergmann and contributors.

Runtime:       PHP 8.2.0

.........                                          9 / 9 (100%)

Time: 00:00:00.234, Memory: 6.00 MB

OK (9 tests, 20 assertions)"#;

    const REAL_PHPUNIT_MULTIPLE_FAILURES: &str = r#"PHPUnit 10.5.0 by Sebastian Bergmann and contributors.

FF.......                                         9 / 9 (100%)

Time: 00:00:00.234, Memory: 6.00 MB

There were 2 failures:

1) UserTest::testEmail
Failed asserting that false is true.

/home/user/tests/UserTest.php:42

2) OrderTest::testTotal
Failed asserting that 42 matches expected 100.

/home/user/tests/OrderTest.php:17

FAILURES!
Tests: 9, Assertions: 15, Failures: 2."#;

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    #[test]
    fn test_phpunit_success_real_format() {
        let result = filter_phpunit_output(REAL_PHPUNIT_SUCCESS);
        assert!(result.contains("PHPUnit"), "should contain PHPUnit prefix");
        assert!(result.contains("OK"), "should indicate success");
        assert!(result.contains("9"), "should show test count");
    }

    #[test]
    fn test_phpunit_failure_real_format_captures_test_name() {
        // Bug: current code captures lines AFTER FAILURES!, missing the details
        let result = filter_phpunit_output(REAL_PHPUNIT_FAILURE);
        assert!(
            result.contains("UserTest::testEmail"),
            "must capture test name — got: {}",
            result
        );
    }

    #[test]
    fn test_phpunit_failure_real_format_captures_error_message() {
        let result = filter_phpunit_output(REAL_PHPUNIT_FAILURE);
        assert!(
            result.contains("Failed asserting"),
            "must capture assertion message — got: {}",
            result
        );
    }

    #[test]
    fn test_phpunit_multiple_failures() {
        let result = filter_phpunit_output(REAL_PHPUNIT_MULTIPLE_FAILURES);
        assert!(
            result.contains("UserTest::testEmail"),
            "must capture first failure"
        );
        assert!(
            result.contains("OrderTest::testTotal"),
            "must capture second failure"
        );
    }

    #[test]
    fn test_parse_counts_with_trailing_period() {
        // Bug: "Failures: 1." has trailing period, parse::<usize>() fails → returns 0
        let (_, _, failures, _) = parse_counts(REAL_PHPUNIT_FAILURE);
        assert_eq!(
            failures, 1,
            "must parse failure count despite trailing period"
        );
    }

    #[test]
    fn test_phpunit_token_savings() {
        let result = filter_phpunit_output(REAL_PHPUNIT_FAILURE);
        let input_tokens = count_tokens(REAL_PHPUNIT_FAILURE);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 65.0,
            "expected ≥65% token savings, got {:.1}% (in={}, out={})",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_phpunit_empty_input() {
        let result = filter_phpunit_output("");
        assert!(!result.is_empty(), "should not panic on empty input");
    }

    // --- Success variants ---

    #[test]
    fn test_phpunit_success_shows_counts() {
        // Fallback "PHPUnit: OK" must show counts from the Tests: summary line
        let output = r#"PHPUnit 10.5.0 by Sebastian Bergmann and contributors.

.........                                          9 / 9 (100%)

Time: 00:00:00.234, Memory: 6.00 MB

OK (9 tests, 20 assertions)"#;
        let result = filter_phpunit_output(output);
        assert!(
            result.contains("9"),
            "must show test count — got: {}",
            result
        );
        assert!(
            result.contains("20"),
            "must show assertion count — got: {}",
            result
        );
    }

    #[test]
    fn test_phpunit_success_with_skipped() {
        // PHPUnit shows "OK, but incomplete, skipped, or risky tests!" when tests are skipped.
        // Must show skipped count, not bare "PHPUnit: OK".
        let output = r#"PHPUnit 10.5.0 by Sebastian Bergmann and contributors.

.S.......                                          9 / 9 (100%)

Time: 00:00:00.234, Memory: 6.00 MB

OK, but incomplete, skipped, or risky tests!
Tests: 9, Assertions: 15, Skipped: 2."#;
        let result = filter_phpunit_output(output);
        assert!(
            result.contains("2") && result.contains("skipped"),
            "must show skipped count — got: {}",
            result
        );
    }

    #[test]
    fn test_parse_counts_skipped() {
        let output = "Tests: 9, Assertions: 15, Skipped: 2.";
        let (tests, assertions, failures, skipped) = parse_counts(output);
        assert_eq!(tests, 9);
        assert_eq!(assertions, 15);
        assert_eq!(failures, 0);
        assert_eq!(skipped, 2, "must parse Skipped field");
    }
}
