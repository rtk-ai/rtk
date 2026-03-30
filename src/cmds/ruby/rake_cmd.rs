//! Minitest output filter for `rake test` and `rails test`.
//!
//! Parses the standard Minitest output format produced by both `rake test` and
//! `rails test`, filtering down to failures/errors and the summary line.
//! Uses `ruby_exec("rake")` to auto-detect `bundle exec`.

use crate::core::runner;
use crate::core::utils::{ruby_exec, strip_ansi};
use anyhow::Result;

/// Decide whether to use `rake test` or `rails test` based on args.
///
/// `rake test` only supports a single file via `TEST=path` and ignores positional
/// file args. When any positional test file paths are detected, we switch to
/// `rails test` which handles single files, multiple files, and line-number
/// syntax (`file.rb:15`) natively.
fn select_runner(args: &[String]) -> (&'static str, Vec<String>) {
    let has_test_subcommand = args.first().map_or(false, |a| a == "test");
    if !has_test_subcommand {
        return ("rake", args.to_vec());
    }

    let after_test: Vec<&String> = args[1..].iter().collect();

    let positional_files: Vec<&&String> = after_test
        .iter()
        .filter(|a| !a.contains('=') && !a.starts_with('-'))
        .filter(|a| looks_like_test_path(a))
        .collect();

    let needs_rails = !positional_files.is_empty();

    if needs_rails {
        ("rails", args.to_vec())
    } else {
        ("rake", args.to_vec())
    }
}

fn looks_like_test_path(arg: &str) -> bool {
    let path = arg.split(':').next().unwrap_or(arg);
    path.ends_with(".rb")
        || path.starts_with("test/")
        || path.starts_with("spec/")
        || path.contains("_test.rb")
        || path.contains("_spec.rb")
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let (tool, effective_args) = select_runner(args);
    let mut cmd = ruby_exec(tool);
    for arg in &effective_args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!(
            "Running: {} {}",
            cmd.get_program().to_string_lossy(),
            effective_args.join(" ")
        );
    }

    runner::run_filtered(
        cmd,
        "rake",
        &args.join(" "),
        filter_minitest_output,
        runner::RunOptions::with_tee("rake"),
    )
}

#[derive(Debug, PartialEq)]
enum ParseState {
    Header,
    Running,
    Failures,
    #[allow(dead_code)]
    Summary,
}

/// Parse Minitest output using a state machine.
///
/// Minitest produces output like:
/// ```text
/// Run options: --seed 12345
///
/// # Running:
///
/// ..F..E..
///
/// Finished in 0.123456s, 64.8 runs/s
///
///   1) Failure:
/// TestSomething#test_that_fails [/path/to/test.rb:15]:
/// Expected: true
///   Actual: false
///
/// 8 runs, 7 assertions, 1 failures, 1 errors, 0 skips
/// ```
fn filter_minitest_output(output: &str) -> String {
    let clean = strip_ansi(output);
    let mut state = ParseState::Header;
    let mut failures: Vec<String> = Vec::new();
    let mut current_failure: Vec<String> = Vec::new();
    let mut summary_line = String::new();

    for line in clean.lines() {
        let trimmed = line.trim();

        // Detect summary line anywhere (it's always last meaningful line)
        // Handles both "N runs, N assertions, ..." and "N tests, N assertions, ..."
        if (trimmed.contains(" runs,") || trimmed.contains(" tests,"))
            && trimmed.contains(" assertions,")
        {
            summary_line = trimmed.to_string();
            continue;
        }

        // State transitions — handle both standard Minitest and minitest-reporters
        if trimmed == "# Running:" || trimmed.starts_with("Started with run options") {
            state = ParseState::Running;
            continue;
        }

        if trimmed.starts_with("Finished in ") {
            state = ParseState::Failures;
            continue;
        }

        match state {
            ParseState::Header | ParseState::Running => {
                // Skip seed line, blank lines, progress dots
                continue;
            }
            ParseState::Failures => {
                if is_failure_header(trimmed) {
                    if !current_failure.is_empty() {
                        failures.push(current_failure.join("\n"));
                        current_failure.clear();
                    }
                    current_failure.push(trimmed.to_string());
                } else if trimmed.is_empty() && !current_failure.is_empty() {
                    failures.push(current_failure.join("\n"));
                    current_failure.clear();
                } else if !trimmed.is_empty() {
                    current_failure.push(line.to_string());
                }
            }
            ParseState::Summary => {}
        }
    }

    // Save last failure if any
    if !current_failure.is_empty() {
        failures.push(current_failure.join("\n"));
    }

    build_minitest_summary(&summary_line, &failures)
}

fn is_failure_header(line: &str) -> bool {
    lazy_static::lazy_static! {
        static ref RE_FAILURE: regex::Regex =
            regex::Regex::new(r"^\d+\)\s+(Failure|Error):$").unwrap();
    }
    RE_FAILURE.is_match(line)
}

fn build_minitest_summary(summary: &str, failures: &[String]) -> String {
    let (runs, _assertions, fail_count, error_count, skips) = parse_minitest_summary(summary);

    if runs == 0 && summary.is_empty() {
        return "rake test: no tests ran".to_string();
    }

    if fail_count == 0 && error_count == 0 {
        let mut msg = format!("ok rake test: {} runs, 0 failures", runs);
        if skips > 0 {
            msg.push_str(&format!(", {} skips", skips));
        }
        return msg;
    }

    let mut result = String::new();
    result.push_str(&format!(
        "rake test: {} runs, {} failures, {} errors",
        runs, fail_count, error_count
    ));
    if skips > 0 {
        result.push_str(&format!(", {} skips", skips));
    }
    result.push('\n');

    if failures.is_empty() {
        return result.trim().to_string();
    }

    result.push('\n');

    for (i, failure) in failures.iter().take(10).enumerate() {
        let lines: Vec<&str> = failure.lines().collect();
        // First line is like "  1) Failure:" or "  1) Error:"
        if let Some(header) = lines.first() {
            result.push_str(&format!("{}. {}\n", i + 1, header.trim()));
        }
        // Remaining lines contain test name, file:line, assertion message
        for line in lines.iter().skip(1).take(4) {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                result.push_str(&format!(
                    "   {}\n",
                    crate::core::utils::truncate(trimmed, 120)
                ));
            }
        }
        if i < failures.len().min(10) - 1 {
            result.push('\n');
        }
    }

    if failures.len() > 10 {
        result.push_str(&format!("\n... +{} more failures\n", failures.len() - 10));
    }

    result.trim().to_string()
}

fn parse_minitest_summary(summary: &str) -> (usize, usize, usize, usize, usize) {
    let mut runs = 0;
    let mut assertions = 0;
    let mut failures = 0;
    let mut errors = 0;
    let mut skips = 0;

    for part in summary.split(',') {
        let part = part.trim();
        let words: Vec<&str> = part.split_whitespace().collect();
        if words.len() >= 2 {
            if let Ok(n) = words[0].parse::<usize>() {
                match words[1].trim_end_matches(',') {
                    "runs" | "run" | "tests" | "test" => runs = n,
                    "assertions" | "assertion" => assertions = n,
                    "failures" | "failure" => failures = n,
                    "errors" | "error" => errors = n,
                    "skips" | "skip" => skips = n,
                    _ => {}
                }
            }
        }
    }

    (runs, assertions, failures, errors, skips)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::utils::count_tokens;

    #[test]
    fn test_filter_minitest_all_pass() {
        let output = r#"Run options: --seed 12345

# Running:

........

Finished in 0.123456s, 64.8 runs/s, 72.9 assertions/s.

8 runs, 9 assertions, 0 failures, 0 errors, 0 skips"#;

        let result = filter_minitest_output(output);
        assert!(result.contains("ok rake test"));
        assert!(result.contains("8 runs"));
        assert!(result.contains("0 failures"));
    }

    #[test]
    fn test_filter_minitest_with_failures() {
        let output = r#"Run options: --seed 54321

# Running:

..F....

Finished in 0.234567s, 29.8 runs/s

  1) Failure:
TestSomething#test_that_fails [/path/to/test.rb:15]:
Expected: true
  Actual: false

7 runs, 7 assertions, 1 failures, 0 errors, 0 skips"#;

        let result = filter_minitest_output(output);
        assert!(result.contains("1 failures"));
        assert!(result.contains("test_that_fails"));
        assert!(result.contains("Expected: true"));
    }

    #[test]
    fn test_filter_minitest_with_errors() {
        let output = r#"Run options: --seed 99999

# Running:

.E....

Finished in 0.345678s, 17.4 runs/s

  1) Error:
TestOther#test_boom [/path/to/test.rb:42]:
RuntimeError: something went wrong
    /path/to/test.rb:42:in `test_boom'

6 runs, 5 assertions, 0 failures, 1 errors, 0 skips"#;

        let result = filter_minitest_output(output);
        assert!(result.contains("1 errors"));
        assert!(result.contains("test_boom"));
        assert!(result.contains("RuntimeError"));
    }

    #[test]
    fn test_filter_minitest_empty() {
        let result = filter_minitest_output("");
        assert!(result.contains("no tests ran"));
    }

    #[test]
    fn test_filter_minitest_skip() {
        let output = r#"Run options: --seed 11111

# Running:

..S..

Finished in 0.100000s, 50.0 runs/s

5 runs, 4 assertions, 0 failures, 0 errors, 1 skips"#;

        let result = filter_minitest_output(output);
        assert!(result.contains("ok rake test"));
        assert!(result.contains("1 skips"));
    }

    #[test]
    fn test_token_savings() {
        let mut dots = String::new();
        for _ in 0..20 {
            dots.push_str(
                "......................................................................\n",
            );
        }
        let output = format!(
            "Run options: --seed 12345\n\n\
             # Running:\n\n\
             {}\n\
             Finished in 2.345678s, 213.4 runs/s, 428.7 assertions/s.\n\n\
             500 runs, 1003 assertions, 0 failures, 0 errors, 0 skips",
            dots
        );

        let input_tokens = count_tokens(&output);
        let result = filter_minitest_output(&output);
        let output_tokens = count_tokens(&result);

        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 80.0,
            "Expected >= 80% savings, got {:.1}% (input: {}, output: {})",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_parse_minitest_summary() {
        assert_eq!(
            parse_minitest_summary("8 runs, 9 assertions, 0 failures, 0 errors, 0 skips"),
            (8, 9, 0, 0, 0)
        );
        assert_eq!(
            parse_minitest_summary("5 runs, 4 assertions, 1 failures, 1 errors, 2 skips"),
            (5, 4, 1, 1, 2)
        );
        // minitest-reporters uses "tests" instead of "runs"
        assert_eq!(
            parse_minitest_summary("57 tests, 378 assertions, 0 failures, 0 errors, 0 skips"),
            (57, 378, 0, 0, 0)
        );
    }

    #[test]
    fn test_filter_minitest_multiple_failures() {
        let output = r#"Run options: --seed 77777

# Running:

.FF.E.

Finished in 0.500000s, 12.0 runs/s

  1) Failure:
TestFoo#test_alpha [/test.rb:10]:
Expected: 1
  Actual: 2

  2) Failure:
TestFoo#test_beta [/test.rb:20]:
Expected: "hello"
  Actual: "world"

  3) Error:
TestBar#test_gamma [/test.rb:30]:
NoMethodError: undefined method `blah'

6 runs, 5 assertions, 2 failures, 1 errors, 0 skips"#;

        let result = filter_minitest_output(output);
        assert!(result.contains("2 failures"));
        assert!(result.contains("1 errors"));
        assert!(result.contains("test_alpha"));
        assert!(result.contains("test_beta"));
        assert!(result.contains("test_gamma"));
    }

    #[test]
    fn test_filter_minitest_reporters_format() {
        let output = "Started with run options --seed 37764\n\n\
            Progress: |========================================|\n\n\
            Finished in 5.79938s\n\
            57 tests, 378 assertions, 0 failures, 0 errors, 0 skips";

        let result = filter_minitest_output(output);
        assert!(result.contains("ok rake test"));
        assert!(result.contains("57 runs"));
        assert!(result.contains("0 failures"));
    }

    #[test]
    fn test_filter_minitest_with_ansi() {
        let output = "\x1b[32mRun options: --seed 12345\x1b[0m\n\n\
            # Running:\n\n\
            \x1b[32m....\x1b[0m\n\n\
            Finished in 0.1s, 40.0 runs/s\n\n\
            4 runs, 4 assertions, 0 failures, 0 errors, 0 skips";

        let result = filter_minitest_output(output);
        assert!(result.contains("ok rake test"));
        assert!(result.contains("4 runs"));
    }

    // ── select_runner tests ─────────────────────────────

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn test_select_runner_single_file_uses_rake() {
        let (tool, _) = select_runner(&args("test TEST=test/models/post_test.rb"));
        assert_eq!(tool, "rake");
    }

    #[test]
    fn test_select_runner_no_files_uses_rake() {
        let (tool, _) = select_runner(&args("test"));
        assert_eq!(tool, "rake");
    }

    #[test]
    fn test_select_runner_multiple_files_uses_rails() {
        let (tool, a) = select_runner(&args(
            "test test/models/post_test.rb test/models/user_test.rb",
        ));
        assert_eq!(tool, "rails");
        assert_eq!(
            a,
            args("test test/models/post_test.rb test/models/user_test.rb")
        );
    }

    #[test]
    fn test_select_runner_line_number_uses_rails() {
        let (tool, _) = select_runner(&args("test test/models/post_test.rb:15"));
        assert_eq!(tool, "rails");
    }

    #[test]
    fn test_select_runner_multiple_with_line_numbers() {
        let (tool, _) = select_runner(&args(
            "test test/models/post_test.rb:15 test/models/user_test.rb:30",
        ));
        assert_eq!(tool, "rails");
    }

    #[test]
    fn test_select_runner_non_test_subcommand_uses_rake() {
        let (tool, _) = select_runner(&args("db:migrate"));
        assert_eq!(tool, "rake");
    }

    #[test]
    fn test_select_runner_single_positional_file_uses_rails() {
        let (tool, _) = select_runner(&args("test test/models/post_test.rb"));
        assert_eq!(tool, "rails");
    }

    #[test]
    fn test_select_runner_flags_not_counted_as_files() {
        let (tool, _) = select_runner(&args("test --verbose --seed 12345"));
        assert_eq!(tool, "rake");
    }

    #[test]
    fn test_looks_like_test_path() {
        assert!(looks_like_test_path("test/models/post_test.rb"));
        assert!(looks_like_test_path("test/models/post_test.rb:15"));
        assert!(looks_like_test_path("spec/models/post_spec.rb"));
        assert!(looks_like_test_path("my_file.rb"));
        assert!(!looks_like_test_path("--verbose"));
        assert!(!looks_like_test_path("12345"));
    }
}
