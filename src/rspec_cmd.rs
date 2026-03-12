use crate::tracking;
use crate::utils::truncate;
use anyhow::{Context, Result};
use std::process::Command;

#[derive(Debug, PartialEq)]
enum ParseState {
    Preamble,
    Failures,
    Summary,
    FailedExamples,
}

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let (program, base_args) = detect_rspec_command();

    let mut cmd = Command::new(&program);
    for a in &base_args {
        cmd.arg(a);
    }
    for a in args {
        cmd.arg(a);
    }

    if verbose > 0 {
        eprintln!(
            "Running: {} {} {}",
            program,
            base_args.join(" "),
            args.join(" ")
        );
    }

    let output = cmd
        .output()
        .context("Failed to run rspec. Is it installed? Try: gem install rspec")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = filter_rspec_output(&raw);

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });

    if let Some(hint) = crate::tee::tee_and_hint(&raw, "rspec", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("rspec {}", args.join(" ")),
        &format!("rtk rspec {}", args.join(" ")),
        &raw,
        &filtered,
    );

    // RSpec exit codes: 0=pass, 1=failure, 2=CLI error
    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

/// Detect available rspec command: bundle exec rspec, bin/rspec, or bare rspec
fn detect_rspec_command() -> (String, Vec<String>) {
    // Check for bin/rspec first
    if std::path::Path::new("bin/rspec").exists() {
        return ("bin/rspec".to_string(), vec![]);
    }

    // Check if bundle is available
    if which_command("bundle").is_some() {
        return (
            "bundle".to_string(),
            vec!["exec".to_string(), "rspec".to_string()],
        );
    }

    ("rspec".to_string(), vec![])
}

fn which_command(cmd: &str) -> Option<String> {
    Command::new("which")
        .arg(cmd)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Parse RSpec output using a state machine and produce compact output
pub fn filter_rspec_output(output: &str) -> String {
    let mut state = ParseState::Preamble;
    let mut failures: Vec<Vec<String>> = Vec::new();
    let mut current_failure: Vec<String> = Vec::new();
    let mut failed_examples: Vec<String> = Vec::new();
    let mut summary_line = String::new();
    let mut seed: Option<String> = None;
    let mut duration: Option<String> = None;

    for line in output.lines() {
        let trimmed = line.trim();

        // Extract seed anywhere
        if trimmed.starts_with("Randomized with seed") {
            if let Some(s) = trimmed.split_whitespace().last() {
                seed = Some(s.to_string());
            }
            continue;
        }

        // Extract duration from "Finished in X.XX seconds"
        if trimmed.starts_with("Finished in") {
            // "Finished in 4.03 seconds (files took 2.13 seconds to load)"
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                duration = Some(format!("{}s", parts[2]));
            }
            continue;
        }

        // State transitions
        if trimmed == "Failures:" {
            state = ParseState::Failures;
            // Save any pending failure
            if !current_failure.is_empty() {
                failures.push(current_failure.clone());
                current_failure.clear();
            }
            continue;
        }

        if trimmed == "Failed examples:" {
            state = ParseState::FailedExamples;
            // Save any pending failure
            if !current_failure.is_empty() {
                failures.push(current_failure.clone());
                current_failure.clear();
            }
            continue;
        }

        // Summary line: "N examples, M failures" or "N examples, M failures, P pending"
        if is_summary_line(trimmed) {
            summary_line = trimmed.to_string();
            state = ParseState::Summary;
            continue;
        }

        match state {
            ParseState::Preamble => {
                // Skip preamble (deprecation warnings, run options, etc.)
            }
            ParseState::Failures => {
                // Detect new failure block: "  1) Description"
                if is_failure_header(trimmed) {
                    if !current_failure.is_empty() {
                        failures.push(current_failure.clone());
                        current_failure.clear();
                    }
                    current_failure.push(trimmed.to_string());
                } else if !trimmed.is_empty() {
                    current_failure.push(trimmed.to_string());
                }
            }
            ParseState::Summary => {
                // Skip lines in summary section
            }
            ParseState::FailedExamples => {
                if trimmed.starts_with("rspec ") {
                    failed_examples.push(trimmed.to_string());
                }
            }
        }
    }

    // Save last failure if any
    if !current_failure.is_empty() {
        failures.push(current_failure.clone());
    }

    build_rspec_summary(&summary_line, &failures, &failed_examples, seed, duration)
}

fn is_summary_line(line: &str) -> bool {
    // Matches: "42 examples, 0 failures"
    //          "42 examples, 3 failures, 2 pending"
    //          "1 example, 1 failure"
    let has_examples = line.contains("example");
    let has_failures = line.contains("failure");
    has_examples && has_failures
}

fn is_failure_header(line: &str) -> bool {
    // Matches: "1) Some::Test description"
    let chars = line.chars();
    let mut has_digit = false;
    for c in chars {
        if c.is_ascii_digit() {
            has_digit = true;
        } else if c == ')' && has_digit {
            return true;
        } else {
            break;
        }
    }
    false
}

fn parse_summary_counts(summary: &str) -> (u32, u32, u32) {
    let mut examples = 0u32;
    let mut failures = 0u32;
    let mut pending = 0u32;

    // "42 examples, 3 failures, 2 pending"
    // Split by comma and parse each part
    for part in summary.split(',') {
        let part = part.trim();
        let words: Vec<&str> = part.split_whitespace().collect();
        if words.len() >= 2 {
            if let Ok(n) = words[0].parse::<u32>() {
                let label = words[1];
                if label.starts_with("example") {
                    examples = n;
                } else if label.starts_with("failure") {
                    failures = n;
                } else if label.starts_with("pending") {
                    pending = n;
                }
            }
        }
    }

    (examples, failures, pending)
}

fn build_rspec_summary(
    summary_line: &str,
    failures: &[Vec<String>],
    failed_examples: &[String],
    seed: Option<String>,
    duration: Option<String>,
) -> String {
    let (examples, failure_count, pending) = parse_summary_counts(summary_line);

    let seed_str = seed
        .map(|s| format!(" [seed {}]", s))
        .unwrap_or_default();
    let dur_str = duration
        .map(|d| format!("({})", d))
        .unwrap_or_default();

    if summary_line.is_empty() {
        return "RSpec: No output".to_string();
    }

    if failure_count == 0 {
        let mut line = format!("✓ RSpec: {} example{}", examples, if examples == 1 { "" } else { "s" });
        line.push_str(", 0 failures");
        if pending > 0 {
            line.push_str(&format!(", {} pending", pending));
        }
        if !dur_str.is_empty() {
            line.push_str(&format!(" {} ", dur_str));
        }
        line.push_str(&seed_str);
        return line;
    }

    // There are failures
    let mut result = String::new();
    result.push_str(&format!(
        "RSpec: {} example{}, {} failure{}",
        examples,
        if examples == 1 { "" } else { "s" },
        failure_count,
        if failure_count == 1 { "" } else { "s" }
    ));
    if pending > 0 {
        result.push_str(&format!(", {} pending", pending));
    }
    if !dur_str.is_empty() {
        result.push_str(&format!(" {}", dur_str));
    }
    result.push_str(&seed_str);
    result.push('\n');
    result.push_str("════════════════════════════════════════\n");

    // Show up to 5 failures
    let max_failures = 5;
    for (i, failure_block) in failures.iter().take(max_failures).enumerate() {
        result.push('\n');
        let lines = failure_block.as_slice();

        if let Some(header) = lines.first() {
            result.push_str(&format!("  {}\n", truncate(header, 120)));
        }

        // Extract key error lines from remaining lines
        let mut relevant = 0;
        for line in lines.iter().skip(1) {
            let lower = line.to_lowercase();
            let is_relevant = lower.contains("expected")
                || lower.contains("got:")
                || lower.contains("failure/error")
                || lower.contains("error:")
                || lower.contains("assert")
                || (line.contains("./spec/") && line.contains(".rb:"))
                || line.starts_with('#');

            if is_relevant && relevant < 3 {
                result.push_str(&format!("     {}\n", truncate(line, 120)));
                relevant += 1;
            }
        }

        // Show file location (last line that looks like a spec path)
        for line in lines.iter().rev() {
            if line.contains("./spec/") && line.contains(".rb:") {
                if !result.contains(line) {
                    result.push_str(&format!("     {}\n", truncate(line, 120)));
                }
                break;
            }
        }

        let _ = i; // suppress unused warning
    }

    if failures.len() > max_failures {
        result.push_str(&format!(
            "\n  ... +{} more failure{}\n",
            failures.len() - max_failures,
            if failures.len() - max_failures == 1 { "" } else { "s" }
        ));
    }

    // Show failed examples for re-run
    if !failed_examples.is_empty() {
        result.push('\n');
        result.push_str("Failed examples:\n");
        for ex in failed_examples {
            result.push_str(&format!("  {}\n", ex));
        }
    }

    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_all_pass() {
        let input = "Randomized with seed 40426\n\nFinished in 4.03 seconds (files took 2.13 seconds to load)\n1 example, 0 failures\n\nRandomized with seed 40426\n";
        let result = filter_rspec_output(input);
        assert!(result.contains("✓ RSpec"), "Expected checkmark, got: {}", result);
        assert!(result.contains("0 failures"), "got: {}", result);
        assert!(result.contains("4.03s"), "got: {}", result);
        assert!(result.contains("40426"), "got: {}", result);
    }

    #[test]
    fn test_filter_all_pass_multiple_examples() {
        let input = "Randomized with seed 12345\n\nFinished in 2.50 seconds\n42 examples, 0 failures\n\nRandomized with seed 12345\n";
        let result = filter_rspec_output(input);
        assert!(result.contains("✓ RSpec"), "got: {}", result);
        assert!(result.contains("42 examples"), "got: {}", result);
        assert!(result.contains("0 failures"), "got: {}", result);
    }

    #[test]
    fn test_filter_with_failures() {
        let input = r#"Randomized with seed 12345

Failures:

  1) UsersController GET /users returns http success
     Failure/Error: get :index
     expected: 200
          got: 403
     # ./spec/controllers/users_controller_spec.rb:15

Failed examples:

  rspec ./spec/controllers/users_controller_spec.rb:14

Finished in 4.23 seconds
5 examples, 1 failure

Randomized with seed 12345
"#;
        let result = filter_rspec_output(input);
        assert!(result.contains("1 failure"), "got: {}", result);
        assert!(result.contains("Failed examples:"), "got: {}", result);
        assert!(result.contains("users_controller_spec.rb"), "got: {}", result);
    }

    #[test]
    fn test_filter_with_pending() {
        let input = "Finished in 1.23 seconds\n10 examples, 0 failures, 3 pending\n";
        let result = filter_rspec_output(input);
        assert!(result.contains("✓ RSpec"), "got: {}", result);
        assert!(result.contains("3 pending"), "got: {}", result);
    }

    #[test]
    fn test_filter_empty_output() {
        let result = filter_rspec_output("");
        assert!(result.contains("No output"), "got: {}", result);
    }

    #[test]
    fn test_filter_with_deprecation_warnings() {
        let input = r#"DEPRECATION WARNING: Some warning here
Run options: include {:focus=>true}

All examples were skipped -- no :focus tag was set.
Randomized with seed 99999

Finished in 0.01 seconds
0 examples, 0 failures
"#;
        let result = filter_rspec_output(input);
        // Should not crash and should show summary
        assert!(!result.is_empty(), "got empty result");
    }

    #[test]
    fn test_is_summary_line() {
        assert!(is_summary_line("42 examples, 0 failures"));
        assert!(is_summary_line("1 example, 1 failure"));
        assert!(is_summary_line("10 examples, 3 failures, 2 pending"));
        assert!(!is_summary_line("Failures:"));
        assert!(!is_summary_line("Failed examples:"));
    }

    #[test]
    fn test_parse_summary_counts() {
        assert_eq!(parse_summary_counts("42 examples, 0 failures"), (42, 0, 0));
        assert_eq!(parse_summary_counts("1 example, 1 failure"), (1, 1, 0));
        assert_eq!(
            parse_summary_counts("10 examples, 3 failures, 2 pending"),
            (10, 3, 2)
        );
    }

    #[test]
    fn test_multiple_failures_truncated() {
        let mut input = String::from("Failures:\n\n");
        for i in 1..=7 {
            input.push_str(&format!(
                "  {}) Test {} fails\n     expected: true\n          got: false\n\n",
                i, i
            ));
        }
        input.push_str("Finished in 1.0 seconds\n7 examples, 7 failures\n");

        let result = filter_rspec_output(&input);
        assert!(result.contains("7 failures"), "got: {}", result);
        assert!(result.contains("+2 more"), "got: {}", result);
    }
}
