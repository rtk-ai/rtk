use crate::core::runner::{self, RunOptions};
use crate::core::stream::StreamFilter;
use crate::core::truncate::CAP_ERRORS;
use crate::core::utils::{resolved_command, strip_ansi, truncate};
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;
use std::ffi::OsString;
use std::process::Command;

const MAX_BAZEL_LINES: usize = CAP_ERRORS;
const MAX_BAZEL_LINE_CHARS: usize = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BazelSubcommand {
    Build,
    Test,
    Lint,
    Run,
    Query,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DetectedSubcommand {
    subcommand: BazelSubcommand,
}

pub fn run(tool: &str, args: &[String], verbose: u8) -> Result<i32> {
    let args_display = args.join(" ");
    if verbose > 0 {
        eprintln!("Running: {} {}", tool, args_display);
    }

    match detect_subcommand(args).map(|detected| detected.subcommand) {
        Some(BazelSubcommand::Test) => runner::run_filtered_with_exit(
            new_command(tool, args),
            tool,
            &args_display,
            filter_test_output,
            RunOptions::with_tee("bazel_test"),
        ),
        Some(BazelSubcommand::Lint) => runner::run_filtered_with_exit(
            new_command(tool, args),
            tool,
            &args_display,
            filter_lint_output,
            RunOptions::with_tee("bazel_lint"),
        ),
        Some(BazelSubcommand::Build) => runner::run_filtered_with_exit(
            new_command(tool, args),
            tool,
            &args_display,
            filter_build_output,
            RunOptions::with_tee("bazel_build"),
        ),
        Some(BazelSubcommand::Run) => runner::run_streamed(
            new_command(tool, args),
            tool,
            &args_display,
            Box::new(BazelRunFilter::default()),
            RunOptions::with_tee("bazel_run"),
        ),
        Some(BazelSubcommand::Query) | Some(BazelSubcommand::Other) | None => {
            let osargs: Vec<OsString> = args.iter().map(OsString::from).collect();
            runner::run_passthrough(tool, &osargs, verbose)
        }
    }
}

fn new_command(tool: &str, args: &[String]) -> Command {
    let mut cmd = resolved_command(tool);
    cmd.args(args);
    cmd
}

fn detect_subcommand(args: &[String]) -> Option<DetectedSubcommand> {
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--" {
            return None;
        }

        if arg.starts_with("--") {
            let flag_name = arg.split_once('=').map(|(name, _)| name).unwrap_or(arg);
            if !arg.contains('=') && startup_option_takes_value(flag_name) {
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }

        if arg.starts_with('-') {
            i += 1;
            continue;
        }

        return Some(DetectedSubcommand {
            subcommand: match arg {
                "build" => BazelSubcommand::Build,
                "test" => BazelSubcommand::Test,
                "lint" => BazelSubcommand::Lint,
                "run" => BazelSubcommand::Run,
                "query" => BazelSubcommand::Query,
                _ => BazelSubcommand::Other,
            },
        });
    }
    None
}

fn startup_option_takes_value(flag: &str) -> bool {
    matches!(
        flag,
        "--bazelrc"
            | "--batch_cpu_scheduling"
            | "--command_port"
            | "--connect_timeout_secs"
            | "--digest_function"
            | "--failure_detail_out"
            | "--host_platform"
            | "--host_jvm_args"
            | "--host_jvm_profile"
            | "--host_jvm_startup_time"
            | "--install_base"
            | "--invocation_policy"
            | "--io_nice_level"
            | "--local_startup_timeout_secs"
            | "--max_idle_secs"
            | "--output_base"
            | "--output_user_root"
            | "--server_javabase"
    )
}

fn filter_test_output(output: &str, exit_code: i32) -> String {
    filter_validation_output(output, exit_code, "bazel test")
}

fn filter_lint_output(output: &str, exit_code: i32) -> String {
    filter_validation_output(output, exit_code, "bazel lint")
}

fn filter_build_output(output: &str, exit_code: i32) -> String {
    filter_validation_output(output, exit_code, "bazel build")
}

fn filter_validation_output(output: &str, exit_code: i32, label: &str) -> String {
    let cleaned = strip_ansi(output);
    let mut kept = Vec::new();
    let mut truncated = false;

    for line in cleaned.lines() {
        if should_keep_validation_line(line) {
            if kept.len() < MAX_BAZEL_LINES {
                kept.push(truncate(line, MAX_BAZEL_LINE_CHARS));
            } else {
                truncated = true;
            }
        }
    }

    if kept.is_empty() {
        let status = if exit_code == 0 { "ok" } else { "failed" };
        return format!("{label}: {status}\n");
    }

    let mut result = kept.join("\n");
    result.push('\n');
    if truncated {
        result.push_str(&format!(
            "... ({}+ actionable lines hidden)\n",
            MAX_BAZEL_LINES
        ));
    }
    result
}

fn should_keep_validation_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }

    lazy_static! {
        static ref KEEP: Regex = Regex::new(
            r"(?i)(^ERROR:|^WARNING:|^FAILED:|^FAIL:|^FAIL\b|error:|warning:|exception|panic|traceback|compilation failed|build did NOT complete|build completed|build successful|test summary|tests? failed|tests? passed|executed \d+ out of \d+ tests?|see .*test\.log|test\.log|INFO: From |Target .* failed to build|Action failed|Use --verbose_failures)"
        )
        .unwrap();
        static ref NOISE: Regex = Regex::new(
            r"(?i)(^Loading:|^Analyzing:|^INFO: Analyzed|^INFO: Found|^INFO: Elapsed time:|^INFO: .* processes:|^INFO: Build Event Protocol files produced|^INFO: Streaming build results|^Starting local Bazel server|^\[\d+ / \d+\])"
        )
        .unwrap();
    }

    KEEP.is_match(trimmed) && !NOISE.is_match(trimmed)
}

#[derive(Default)]
struct BazelRunFilter {
    command_started: bool,
}

impl StreamFilter for BazelRunFilter {
    fn feed_line(&mut self, line: &str) -> Option<String> {
        if is_bazel_run_sentinel(line) {
            self.command_started = true;
            return None;
        }

        if self.command_started {
            return Some(format!("{line}\n"));
        }

        if should_keep_run_preamble_line(line) {
            Some(format!("{line}\n"))
        } else {
            None
        }
    }

    fn flush(&mut self) -> String {
        String::new()
    }
}

fn is_bazel_run_sentinel(line: &str) -> bool {
    line.starts_with("INFO: Running command line:")
}

fn should_keep_run_preamble_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }

    if should_keep_validation_line(trimmed) {
        return true;
    }

    !is_bazel_noise_line(trimmed)
}

fn is_bazel_noise_line(line: &str) -> bool {
    lazy_static! {
        static ref NOISE: Regex = Regex::new(
            r"(?i)(^Loading:|^Analyzing:|^INFO: Analyzed|^INFO: Found|^INFO: Elapsed time:|^INFO: .* processes:|^INFO: Build completed successfully|^Starting local Bazel server|^\[\d+ / \d+\])"
        )
        .unwrap();
    }
    NOISE.is_match(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn detects_subcommand_after_startup_options() {
        let detected = detect_subcommand(&args(&[
            "--output_base",
            "/tmp/bazel-output",
            "--bazelrc=/tmp/bazelrc",
            "test",
            "//...",
        ]))
        .unwrap();
        assert_eq!(detected.subcommand, BazelSubcommand::Test);
    }

    #[test]
    fn detects_subcommand_after_boolean_startup_option() {
        let detected = detect_subcommand(&args(&["--batch", "test", "//..."])).unwrap();
        assert_eq!(detected.subcommand, BazelSubcommand::Test);
    }

    #[test]
    fn filter_test_keeps_failures_and_summaries() {
        let input = "\
Loading:
Analyzing: target //pkg:test
INFO: Analyzed target //pkg:test (0 packages loaded)
FAIL: //pkg:test (see /tmp/test.log)
INFO: From Testing //pkg:test:
panic: bad things
Executed 1 out of 1 test: 1 fails locally.
INFO: Elapsed time: 1.23s
";

        let out = filter_test_output(input, 1);

        assert!(out.contains("FAIL: //pkg:test"));
        assert!(out.contains("test.log"));
        assert!(out.contains("panic: bad things"));
        assert!(out.contains("Executed 1 out of 1 test"));
        assert!(!out.contains("Loading:"));
        assert!(!out.contains("Analyzing:"));
    }

    #[test]
    fn filter_build_success_compacts_noise() {
        let input = "\
Loading:
Analyzing: target //pkg:lib
INFO: Found 1 target...
INFO: Elapsed time: 1.23s
INFO: Build completed successfully, 1 total action
";

        let out = filter_build_output(input, 0);

        assert_eq!(out, "INFO: Build completed successfully, 1 total action\n");
    }

    #[test]
    fn filter_lint_keeps_violations() {
        let input = "\
Loading:
ERROR: /workspace/pkg/BUILD:12:1: in proto_library rule //pkg:pb_proto: lint failed
WARNING: /workspace/pkg/file.proto: field name should be snake_case
FAILED: Build did NOT complete successfully
";

        let out = filter_lint_output(input, 1);

        assert!(out.contains("ERROR: /workspace/pkg/BUILD"));
        assert!(out.contains("WARNING: /workspace/pkg/file.proto"));
        assert!(out.contains("FAILED: Build did NOT complete successfully"));
    }

    #[test]
    fn run_filter_preserves_stdout_before_stderr_sentinel() {
        let mut filter = BazelRunFilter::default();

        assert_eq!(
            filter.feed_line("hello from target stdout"),
            Some("hello from target stdout\n".to_string())
        );
        assert_eq!(
            filter.feed_line("INFO: Running command line: /tmp/app"),
            None
        );
        assert_eq!(
            filter.feed_line("hello from target stderr"),
            Some("hello from target stderr\n".to_string())
        );
    }

    #[test]
    fn run_filter_drops_bazel_build_noise() {
        let mut filter = BazelRunFilter::default();

        assert_eq!(filter.feed_line("Loading:"), None);
        assert_eq!(filter.feed_line("Analyzing: target //app:app"), None);
        assert_eq!(
            filter.feed_line("ERROR: /workspace/BUILD:1: bad target"),
            Some("ERROR: /workspace/BUILD:1: bad target\n".to_string())
        );
    }
}
