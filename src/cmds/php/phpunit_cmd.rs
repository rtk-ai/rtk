//! PHPUnit output filter.
//!
//! Parses PHPUnit's plain-text runner output and emits a compact summary:
//! aggregate counts from the `Tests: X, Assertions: Y, Failures: Z.` line
//! plus a bounded list of failures, each with its message, diff and stack
//! frames. A long trace keeps only its first and last frames, and details
//! beyond a cap are counted in a marker, except the last frame, which is
//! always kept. Dot-progress lines and headers are stripped entirely.

use super::utils::{php_tool_command, strip_ansi_and_controls};
use crate::core::runner;
use crate::core::shell::display_args;
use anyhow::Result;
use regex::Regex;
use std::borrow::Cow;
use std::sync::LazyLock;

const MAX_FAILURES_SHOWN: usize = 10;
// Room for the message, the `--- Expected` / `+++ Actual` / `@@ @@` header,
// a typical diff hunk and its frames.
const MAX_DETAIL_LINES_PER_FAILURE: usize = 20;
// PHPUnit keeps vendor frames: an exception thrown inside a framework prints
// dozens. Past this, only the throw site and the test line are kept.
const MAX_FRAMES_IN_FULL: usize = 3;

// PHPUnit prints each failure heading as "N) Class::method". Anchor to that
// exact shape so detail lines that merely start with a digit and contain ')'
// (e.g. "5 of 10 assertions passed in Foo::bar()") don't split a block.
static FAILURE_HEADING_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+\) \S").unwrap());
// A stack frame, printed as `/path/to/FooTest.php:42`.
static FRAME_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.php:\d+$").unwrap());

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
        &display_args(args),
        filter_phpunit_output,
        runner::RunOptions::stdout_only().tee("phpunit"),
    )
}

pub(crate) fn filter_phpunit_output(output: &str) -> String {
    // PHPUnit colorizes its result line and progress with ANSI under
    // `--colors=always`; without stripping, the "OK ("/"FAILURES!"/"Tests:"
    // anchors below never match and real counts are lost.
    let cleaned = strip_ansi_and_controls(output);
    let output = cleaned.as_str();

    let mut failures: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut in_failures = false;

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("OK (") {
            return format!("PHPUnit: {}", trimmed);
        }

        if trimmed.starts_with("OK, but") {
            return build_success_with_skipped(output);
        }

        if (trimmed.starts_with("There was") || trimmed.starts_with("There were"))
            && (trimmed.contains("failure") || trimmed.contains("error"))
        {
            in_failures = true;
            continue;
        }

        // `--` separates PHPUnit's result sections: what follows may be risky
        // or skipped tests, which are not failures. Matched untrimmed, since a
        // diff prints the context line `--` as ` --`.
        if trimmed == "FAILURES!" || trimmed == "ERRORS!" || line.trim_end() == "--" {
            if !current.is_empty() {
                failures.push(std::mem::take(&mut current));
            }
            in_failures = false;
            continue;
        }

        if in_failures {
            if is_numbered_failure_heading(trimmed) {
                if !current.is_empty() {
                    failures.push(std::mem::take(&mut current));
                }
                current.push(trimmed.to_string());
            } else if !line.is_empty() {
                // Leading whitespace is the diff's context column and indentation.
                current.push(line.trim_end().to_string());
            }
        }
    }

    if !current.is_empty() {
        failures.push(current);
    }

    if failures.is_empty() {
        let counts = parse_counts(output);
        if counts.tests > 0 {
            return format!(
                "PHPUnit: {} tests, {} assertions",
                counts.tests, counts.assertions
            );
        }
        return "PHPUnit: ok".to_string();
    }

    build_phpunit_summary(output, &failures)
}

fn is_numbered_failure_heading(line: &str) -> bool {
    FAILURE_HEADING_RE.is_match(line)
}

fn build_success_with_skipped(output: &str) -> String {
    let counts = parse_counts(output);
    if counts.skipped > 0 {
        format!(
            "PHPUnit: {} tests, {} assertions, {} skipped",
            counts.tests, counts.assertions, counts.skipped
        )
    } else {
        format!(
            "PHPUnit: {} tests, {} assertions",
            counts.tests, counts.assertions
        )
    }
}

fn build_phpunit_summary(output: &str, failures: &[Vec<String>]) -> String {
    let counts = parse_counts(output);

    // PHPUnit separates failures (assertion mismatches) from errors (thrown
    // exceptions); report them distinctly rather than lumping under "failures".
    let mut result = format!(
        "PHPUnit: {} tests, {} assertions, {} failures",
        counts.tests, counts.assertions, counts.failures
    );
    if counts.errors > 0 {
        result.push_str(&format!(", {} errors", counts.errors));
    }
    result.push('\n');

    for failure_lines in failures.iter().take(MAX_FAILURES_SHOWN) {
        if let Some((heading, details)) = failure_lines.split_first() {
            result.push_str(&format!("\n{}\n", heading));
            push_failure_details(&mut result, details);
        }
    }

    if failures.len() > MAX_FAILURES_SHOWN {
        result.push_str(&format!(
            "\n... +{} more failures\n",
            failures.len() - MAX_FAILURES_SHOWN
        ));
    }

    result.trim().to_string()
}

fn push_failure_details(result: &mut String, details: &[String]) {
    let lines = compact_frames(details);
    let (shown, hidden) = lines.split_at(lines.len().min(MAX_DETAIL_LINES_PER_FAILURE));
    for (line, _) in shown {
        result.push_str(&format!("  {}\n", line));
    }

    // PHPUnit drops its own frames, so the last one is the line of the test
    // method: keep it even when a long diff pushes it past the cap.
    let last_frame = hidden
        .iter()
        .rev()
        .map(|(line, _)| line)
        .find(|line| FRAME_RE.is_match(line));
    let cut = hidden.iter().map(|(_, raw)| raw).sum::<usize>() - usize::from(last_frame.is_some());
    if cut > 0 {
        result.push_str(&format!("  ... +{} more lines\n", cut));
    }
    if let Some(frame) = last_frame {
        result.push_str(&format!("  {}\n", frame));
    }
}

/// Each line comes with the number of raw lines it stands for, so a cut
/// that falls after a compacted trace still reports the real count.
fn compact_frames(details: &[String]) -> Vec<(Cow<'_, str>, usize)> {
    let is_frame = |line: &String| FRAME_RE.is_match(line);
    let mut lines = Vec::with_capacity(details.len());
    for run in details.chunk_by(|a, b| is_frame(a) == is_frame(b)) {
        match run {
            [first, middle @ .., last] if run.len() > MAX_FRAMES_IN_FULL && is_frame(first) => {
                lines.push((Cow::Borrowed(first.as_str()), 1));
                let marker = format!("... +{} more frames", middle.len());
                lines.push((Cow::Owned(marker), middle.len()));
                lines.push((Cow::Borrowed(last.as_str()), 1));
            }
            _ => lines.extend(run.iter().map(|line| (Cow::Borrowed(line.as_str()), 1))),
        }
    }
    lines
}

fn parse_counts(output: &str) -> Counts {
    let mut counts = Counts::default();

    for line in output.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("Tests:") {
            continue;
        }

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
                "Tests:" => counts.tests = val,
                "Assertions:" => counts.assertions = val,
                k if k.starts_with("Failures") => counts.failures += val,
                k if k.starts_with("Errors") => counts.errors += val,
                k if k.starts_with("Skipped") => counts.skipped = val,
                _ => {}
            }
        }
    }

    counts
}

#[derive(Default)]
struct Counts {
    tests: usize,
    assertions: usize,
    failures: usize,
    errors: usize,
    skipped: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_numbered_failure_heading_anchored() {
        // Real PHPUnit failure headings match.
        assert!(is_numbered_failure_heading(
            "1) App\\Tests\\UserTest::testEmail"
        ));
        assert!(is_numbered_failure_heading("12) Foo::bar"));
        // Detail lines that merely start with a digit and contain ')' must not.
        assert!(!is_numbered_failure_heading(
            "5 of 10 assertions passed in Foo::bar()"
        ));
        assert!(!is_numbered_failure_heading("1)")); // no method after ") "
        assert!(!is_numbered_failure_heading(
            "Failed asserting that Array(3) is identical."
        ));
    }

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

    #[test]
    fn test_phpunit_success() {
        let result = filter_phpunit_output(REAL_PHPUNIT_SUCCESS);
        assert!(result.contains("PHPUnit"), "got: {}", result);
        assert!(
            result.contains("OK (9 tests, 20 assertions)"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_phpunit_failure_captures_test_name() {
        let result = filter_phpunit_output(REAL_PHPUNIT_FAILURE);
        assert!(
            result.contains("UserTest::testEmailValidation"),
            "got: {}",
            result
        );
        assert!(
            result.contains("Failed asserting that false is true"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_phpunit_failure_summary_counts() {
        let result = filter_phpunit_output(REAL_PHPUNIT_FAILURE);
        assert!(result.contains("110 tests"), "got: {}", result);
        assert!(result.contains("340 assertions"), "got: {}", result);
        assert!(result.contains("1 failures"), "got: {}", result);
    }

    #[test]
    fn test_phpunit_multiple_failures() {
        let result = filter_phpunit_output(REAL_PHPUNIT_MULTIPLE_FAILURES);
        assert!(result.contains("UserTest::testEmail"), "got: {}", result);
        assert!(result.contains("OrderTest::testTotal"), "got: {}", result);
        assert!(result.contains("2 failures"), "got: {}", result);
    }

    #[test]
    fn test_phpunit_ok_with_skipped() {
        let output = r#"OK, but incomplete, skipped, or risky tests!
Tests: 5, Assertions: 10, Skipped: 2."#;
        let result = filter_phpunit_output(output);
        assert!(result.contains("5 tests"), "got: {}", result);
        assert!(result.contains("2 skipped"), "got: {}", result);
    }

    #[test]
    fn test_phpunit_errors_summary() {
        let output = r#"There was 1 error:

1) FooTest::testBar
RuntimeException: boom

ERRORS!
Tests: 1, Assertions: 0, Errors: 1."#;
        let result = filter_phpunit_output(output);
        assert!(result.contains("FooTest::testBar"), "got: {}", result);
        // Errors are now reported distinctly from failures.
        assert!(result.contains("0 failures, 1 errors"), "got: {}", result);
    }

    #[test]
    fn test_phpunit_failure_truncation() {
        let mut output = String::from("There were 15 failures:\n\n");
        for i in 1..=15 {
            output.push_str(&format!(
                "{}) Suite::test{}\nFailed asserting thing {}.\n\n",
                i, i, i
            ));
        }
        output.push_str("FAILURES!\nTests: 15, Assertions: 15, Failures: 15.\n");

        let result = filter_phpunit_output(&output);
        assert!(result.contains("Suite::test1"), "got: {}", result);
        assert!(result.contains("Suite::test10"), "got: {}", result);
        assert!(!result.contains("Suite::test11"), "got: {}", result);
        assert!(result.contains("+5 more failures"), "got: {}", result);
    }

    #[test]
    fn test_phpunit_strips_ansi_colors() {
        // --colors=always wraps the result line; anchors must still match.
        let colored = "\x1b[30;42mOK\x1b[0m \x1b[32m(9 tests, 20 assertions)\x1b[0m";
        let result = filter_phpunit_output(colored);
        assert!(
            result.contains("OK (9 tests, 20 assertions)"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_phpunit_empty_ok_fallback() {
        let result = filter_phpunit_output("");
        assert_eq!(result, "PHPUnit: ok");
    }

    #[test]
    fn test_phpunit_only_summary_line() {
        let result = filter_phpunit_output("Tests: 4, Assertions: 4.\n");
        assert!(result.contains("4 tests"), "got: {}", result);
    }

    #[test]
    fn test_phpunit_compression() {
        let raw_len = REAL_PHPUNIT_FAILURE.len();
        let filtered_len = filter_phpunit_output(REAL_PHPUNIT_FAILURE).len();
        assert!(
            filtered_len < raw_len / 2,
            "expected >50% reduction, raw={}, filtered={}",
            raw_len,
            filtered_len
        );
    }

    // PHPUnit 13.4.3, `assertSame(['a' => 1, 'b' => 2], ['a' => 1, 'b' => 3])`.
    const REAL_PHPUNIT_DIFF_FAILURE: &str = r#"PHPUnit 13.4.3 by Sebastian Bergmann and contributors.

Runtime:       PHP 8.5.4

.F                                                                  2 / 2 (100%)

Time: 00:00.007, Memory: 8.00 MB

There was 1 failure:

1) FooTest::testFails
Failed asserting that two arrays are identical.
--- Expected
+++ Actual
@@ @@
 Array &0 [
     'a' => 1,
-    'b' => 2,
+    'b' => 3,
 ]

/app/tests/FooTest.php:8

FAILURES!
Tests: 2, Assertions: 2, Failures: 1."#;

    // PHPUnit 13.4.3: a chained exception thrown two calls deep, an assertion
    // in a helper, a 60-line array diff and a test without assertions.
    const REAL_PHPUNIT_ERRORS_FAILURES_RISKY: &str =
        include_str!("../../../tests/fixtures/phpunit_errors_failures_risky_raw.txt");

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    fn failure_block<'a>(output: &'a str, heading: &str) -> &'a str {
        let start = output
            .find(heading)
            .unwrap_or_else(|| panic!("{heading} missing from: {output}"));
        let block = &output[start..];
        block.find("\n\n").map_or(block, |end| &block[..end])
    }

    #[test]
    fn test_phpunit_diff_failure_keeps_diff_and_location() {
        let result = filter_phpunit_output(REAL_PHPUNIT_DIFF_FAILURE);
        assert_eq!(
            result,
            "PHPUnit: 2 tests, 2 assertions, 1 failures

1) FooTest::testFails
  Failed asserting that two arrays are identical.
  --- Expected
  +++ Actual
  @@ @@
   Array &0 [
       'a' => 1,
  -    'b' => 2,
  +    'b' => 3,
   ]
  /app/tests/FooTest.php:8"
        );
    }

    #[test]
    fn test_phpunit_long_diff_marks_cut_and_keeps_location() {
        let result = filter_phpunit_output(REAL_PHPUNIT_ERRORS_FAILURES_RISKY);
        let block = failure_block(&result, "2) BazTest::testLongDiff");
        // 66 detail lines: the first 20 are shown, then the location.
        assert_eq!(block.lines().count(), 1 + 20 + 2, "got: {block}");
        assert!(
            block.ends_with(
                "  -    'k14' => 14,\n  ... +45 more lines\n  /app/tests/BazTest.php:41"
            ),
            "got: {block}"
        );
    }

    #[test]
    fn test_phpunit_short_details_are_shown_in_full() {
        let result = filter_phpunit_output(REAL_PHPUNIT_ERRORS_FAILURES_RISKY);
        assert_eq!(
            failure_block(&result, "1) BazTest::testDeepError"),
            "1) BazTest::testDeepError
  RuntimeException: Outer failure.
  /app/tests/BazTest.php:16
  /app/tests/BazTest.php:8
  /app/tests/BazTest.php:25
  Caused by
  InvalidArgumentException: Inner cause.
  /app/tests/BazTest.php:14
  /app/tests/BazTest.php:8
  /app/tests/BazTest.php:25"
        );
        assert_eq!(
            failure_block(&result, "1) BazTest::testHelperAssertion"),
            "1) BazTest::testHelperAssertion
  Failed asserting that -1 is greater than 0.
  /app/tests/BazTest.php:50
  /app/tests/BazTest.php:30"
        );
    }

    #[test]
    fn test_phpunit_string_diff_keeps_blank_and_dash_lines() {
        // PHPUnit 13.4.3, `assertSame("a\n\n--\nb\n", "a\n\n--\nc\n")`. The
        // `\n` escapes spell the whitespace-only context line ` `.
        let output = "There was 1 failure:

1) StrTest::testMultiline
Failed asserting that two strings are identical.
--- Expected
+++ Actual
@@ @@
 'a
 \n --
-b
+c
 '

/app/tests/StrTest.php:7

FAILURES!
Tests: 1, Assertions: 1, Failures: 1.";
        assert_eq!(
            filter_phpunit_output(output),
            "PHPUnit: 1 tests, 1 assertions, 1 failures

1) StrTest::testMultiline
  Failed asserting that two strings are identical.
  --- Expected
  +++ Actual
  @@ @@
   'a
  \n   --
  -b
  +c
   '
  /app/tests/StrTest.php:7"
        );
    }

    #[test]
    fn test_phpunit_vendor_trace_keeps_throw_site_and_test_line() {
        // PHPUnit 13.4.3, a Symfony container compiled with a missing service.
        let output = r#"There was 1 error:

1) QuxTest::testVendorError
Symfony\Component\DependencyInjection\Exception\ServiceNotFoundException: The service "app.mailer" has a dependency on a non-existent service "app.missing".

/app/vendor/symfony/dependency-injection/Compiler/CheckExceptionOnInvalidReferenceBehaviorPass.php:116
/app/vendor/symfony/dependency-injection/Compiler/CheckExceptionOnInvalidReferenceBehaviorPass.php:123
/app/vendor/symfony/dependency-injection/Compiler/CheckExceptionOnInvalidReferenceBehaviorPass.php:83
/app/vendor/symfony/dependency-injection/Compiler/AbstractRecursivePass.php:80
/app/vendor/symfony/dependency-injection/Compiler/CheckExceptionOnInvalidReferenceBehaviorPass.php:48
/app/vendor/symfony/dependency-injection/Compiler/AbstractRecursivePass.php:89
/app/vendor/symfony/dependency-injection/Compiler/CheckExceptionOnInvalidReferenceBehaviorPass.php:48
/app/vendor/symfony/dependency-injection/Compiler/AbstractRecursivePass.php:80
/app/vendor/symfony/dependency-injection/Compiler/CheckExceptionOnInvalidReferenceBehaviorPass.php:48
/app/vendor/symfony/dependency-injection/Compiler/AbstractRecursivePass.php:43
/app/vendor/symfony/dependency-injection/Compiler/CheckExceptionOnInvalidReferenceBehaviorPass.php:39
/app/vendor/symfony/dependency-injection/Compiler/Compiler.php:73
/app/vendor/symfony/dependency-injection/ContainerBuilder.php:823
/app/tests/QuxTest.php:14

ERRORS!
Tests: 1, Assertions: 0, Errors: 1."#;
        assert_eq!(
            filter_phpunit_output(output),
            r#"PHPUnit: 1 tests, 0 assertions, 0 failures, 1 errors

1) QuxTest::testVendorError
  Symfony\Component\DependencyInjection\Exception\ServiceNotFoundException: The service "app.mailer" has a dependency on a non-existent service "app.missing".
  /app/vendor/symfony/dependency-injection/Compiler/CheckExceptionOnInvalidReferenceBehaviorPass.php:116
  ... +12 more frames
  /app/tests/QuxTest.php:14"#
        );
    }

    #[test]
    fn test_phpunit_cut_counts_compacted_frames() {
        let mut output = String::from("There was 1 failure:\n\n1) FooTest::testBar\n");
        for i in 1..=25 {
            output.push_str(&format!("-    'k{i}' => {i},\n"));
        }
        output.push('\n');
        for i in 1..=10 {
            output.push_str(&format!("/app/src/Frame{i}.php:{i}\n"));
        }
        output.push_str("\nFAILURES!\nTests: 1, Assertions: 1, Failures: 1.\n");

        let result = filter_phpunit_output(&output);
        // 35 raw detail lines: 20 shown, the last frame kept, 14 cut.
        assert!(
            result
                .ends_with("  -    'k20' => 20,\n  ... +14 more lines\n  /app/src/Frame10.php:10"),
            "got: {result}"
        );
    }

    #[test]
    fn test_phpunit_stops_at_section_separator() {
        let result = filter_phpunit_output(REAL_PHPUNIT_ERRORS_FAILURES_RISKY);
        assert!(
            !result.lines().any(|line| line.trim() == "--"),
            "got: {result}"
        );
        assert!(!result.contains("BazTest::testRisky"), "got: {result}");
    }

    #[test]
    fn test_phpunit_errors_failures_risky_savings() {
        let input = REAL_PHPUNIT_ERRORS_FAILURES_RISKY;
        let output = filter_phpunit_output(input);
        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(savings >= 20.0, "expected >=20% savings, got {savings:.1}%");
    }
}
