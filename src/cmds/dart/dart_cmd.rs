//! Dart command filters.

use anyhow::Result;
use crate::core::runner;
use std::ffi::OsString;

use crate::cmds::flutter::flutter_cmd::{filter_analyze_output, run_flutter_filtered};

pub fn run_analyze(args: &[String], verbose: u8) -> Result<i32> {
    run_flutter_filtered(
        &["analyze"],
        args,
        "dart analyze",
        verbose,
        "dart_analyze",
        |raw| filter_analyze_output(raw, "dart"),
    )
}

pub fn run_other(args: &[OsString], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("Running: dart {}", format_os_args(args));
    }
    runner::run_passthrough("dart", args, verbose)
}

fn format_os_args(args: &[OsString]) -> String {
    args.iter()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_dart_analyze_shares_flutter_parser() {
        let input = "Analyzing dart package...\ninfo • Example warning • lib/foo.dart:1:1 • prefer_final_locals\n1 issue found. (ran in 0.1s)";
        let output = filter_analyze_output(input, "dart");
        assert!(output.contains("1 issues found in 1 files"));
        assert!(output.contains("lib/foo.dart"));
    }

    #[test]
    fn test_dart_analyze_savings_on_synthetic_input() {
        let input = "Analyzing dart package...\ninfo • Example warning • lib/foo.dart:1:1 • prefer_final_locals\nwarning • Example warning • lib/foo.dart:2:1 • prefer_final_locals\n2 issues found. (ran in 0.1s)";
        let output = filter_analyze_output(input, "dart");
        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(savings >= 8.0, "Expected savings, got {:.1}%", savings);
    }
}