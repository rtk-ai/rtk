//! Busted test runner output filter.

use crate::core::runner;
use crate::core::utils::{resolved_command, strip_ansi, truncate};
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;

#[derive(Debug, PartialEq, Eq)]
struct BustedSummary {
    successes: usize,
    failures: usize,
    errors: usize,
    pending: usize,
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("busted");
    cmd.args(args);

    if verbose > 0 {
        eprintln!("Running: busted {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "busted",
        &args.join(" "),
        filter_busted_output,
        runner::RunOptions::stdout_only().tee("busted"),
    )
}

pub(crate) fn filter_busted_output(output: &str) -> String {
    let clean = strip_ansi(output);

    if clean.trim().is_empty() {
        return "busted: no output".to_string();
    }

    let summary = parse_summary(&clean);
    let failures = collect_failures(&clean);

    if let Some(summary) = &summary {
        if summary.failures == 0 && summary.errors == 0 {
            let mut result = format!("ok busted: {} successes", summary.successes);
            if summary.pending > 0 {
                result.push_str(&format!(", {} pending", summary.pending));
            }
            return result;
        }
    }

    let mut result = match summary {
        Some(ref s) => format!(
            "busted: {} successes, {} failures, {} errors",
            s.successes, s.failures, s.errors
        ),
        None => format!("busted: {} failures/errors", failures.len()),
    };

    if let Some(summary) = &summary {
        if summary.pending > 0 {
            result.push_str(&format!(", {} pending", summary.pending));
        }
    }
    result.push('\n');

    if failures.is_empty() {
        return result.trim().to_string();
    }

    for (idx, failure) in failures.iter().take(8).enumerate() {
        result.push_str(&format!("{}. {}\n", idx + 1, failure));
    }

    if failures.len() > 8 {
        result.push_str(&format!("... +{} more failures\n", failures.len() - 8));
    }

    result.trim().to_string()
}

fn parse_summary(output: &str) -> Option<BustedSummary> {
    lazy_static! {
        static ref SUMMARY_RE: Regex = Regex::new(
            r"(\d+)\s+success(?:es)?\s*/\s*(\d+)\s+failures?\s*/\s*(\d+)\s+errors?\s*/\s*(\d+)\s+pending"
        )
        .unwrap();
    }

    SUMMARY_RE.captures(output).map(|caps| BustedSummary {
        successes: caps[1].parse().unwrap_or(0),
        failures: caps[2].parse().unwrap_or(0),
        errors: caps[3].parse().unwrap_or(0),
        pending: caps[4].parse().unwrap_or(0),
    })
}

fn collect_failures(output: &str) -> Vec<String> {
    lazy_static! {
        static ref LOCATION_RE: Regex =
            Regex::new(r"(?i)(?:^|\s)([\w./\\-]+\.lua:\d+(?::\d+)?)").unwrap();
    }

    let mut failures = Vec::new();
    let lines: Vec<&str> = output.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let trimmed = lines[i].trim();
        let lower = trimmed.to_ascii_lowercase();
        let starts_failure = lower.starts_with("failure")
            || lower.starts_with("error")
            || lower.starts_with("failures")
            || lower.starts_with("errors");
        let has_lua_location = LOCATION_RE.is_match(trimmed)
            && (lower.contains("expected")
                || lower.contains("assert")
                || lower.contains("failure")
                || lower.contains("error"));

        if starts_failure || has_lua_location {
            let mut block = Vec::new();
            for line in lines.iter().skip(i).take(5) {
                let t = line.trim();
                if t.is_empty() || t.eq_ignore_ascii_case("stack traceback:") {
                    break;
                }
                block.push(truncate(t, 140));
            }
            if !block.is_empty() {
                failures.push(block.join(" | "));
            }
        }

        i += 1;
    }

    failures
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::utils::count_tokens;

    #[test]
    fn test_filter_busted_success() {
        let output = "++++\n4 successes / 0 failures / 0 errors / 0 pending : 0.012 seconds";
        let result = filter_busted_output(output);
        assert_eq!(result, "ok busted: 4 successes");
    }

    #[test]
    fn test_filter_busted_failure_summary_and_location() {
        let output = r#"-- Output omitted --

Failure -> spec/foo_spec.lua @ 12
Expected objects to be equal.
Passed in:
(number) 1
Expected:
(number) 2

spec/bar_spec.lua:20: assertion failed!
stack traceback:
    spec/bar_spec.lua:20: in function <spec/bar_spec.lua:18>

3 successes / 1 failure / 1 error / 0 pending : 0.020 seconds"#;

        let result = filter_busted_output(output);
        assert!(result.contains("3 successes, 1 failures, 1 errors"));
        assert!(result.contains("spec/foo_spec.lua"));
        assert!(result.contains("Expected objects"));
        assert!(result.contains("spec/bar_spec.lua:20"));
        assert!(!result.contains("stack traceback"));
    }

    #[test]
    fn test_filter_busted_caps_failures() {
        let mut output = String::new();
        for i in 1..=10 {
            output.push_str(&format!("Failure -> spec/file{}_spec.lua @ {}\nboom\n\n", i, i));
        }
        output.push_str("0 successes / 10 failures / 0 errors / 0 pending : 0.1 seconds");

        let result = filter_busted_output(&output);
        assert!(result.contains("8. Failure -> spec/file8_spec.lua"));
        assert!(!result.contains("9. Failure -> spec/file9_spec.lua"));
        assert!(result.contains("+2 more failures"));
    }

    #[test]
    fn test_busted_token_savings() {
        let mut output = String::new();
        output.push_str("++++++++++++++++++++++++++++++++++++++++++++++++++\n");
        for i in 1..=30 {
            output.push_str(&format!("spec/file{}_spec.lua: ok\n", i));
        }
        output.push_str("30 successes / 0 failures / 0 errors / 0 pending : 0.5 seconds");

        let filtered = filter_busted_output(&output);
        let savings =
            100.0 - (count_tokens(&filtered) as f64 / count_tokens(&output) as f64 * 100.0);

        assert!(
            savings >= 80.0,
            "expected >=80% savings, got {:.1}%\n{}",
            savings,
            filtered
        );
    }
}
