//! Filters Bun command output while preserving the Bun/Bunx executable.

use crate::core::runner;
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::Result;

/// Run `bun <args>` through the Bun output filter.
pub fn run(args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    run_filtered("bun", args, verbose, skip_env)
}

/// Run `bunx <args>` through the Bun output filter.
pub fn exec(args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    run_filtered("bunx", args, verbose, skip_env)
}

fn run_filtered(name: &str, args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    let mut cmd = resolved_command(name);
    for arg in args {
        cmd.arg(arg);
    }

    if skip_env {
        cmd.env("SKIP_ENV_VALIDATION", "1");
    }

    let args_display = args.join(" ");
    if verbose > 0 {
        eprintln!("Running: {} {}", name, args_display);
    }

    runner::run_filtered(
        cmd,
        name,
        &args_display,
        filter_bun_output,
        runner::RunOptions::default(),
    )
}

/// Remove Bun's version banner and the script command it immediately echoes.
fn filter_bun_output(output: &str) -> String {
    let mut result = Vec::new();
    let mut skip_script_echo = false;

    for line in output.lines() {
        let clean = strip_ansi(line);
        let trimmed = clean.trim_start();

        if is_bun_banner(trimmed) {
            skip_script_echo = trimmed.starts_with("bun run v");
            continue;
        }

        if skip_script_echo {
            if trimmed.is_empty() {
                continue;
            }
            skip_script_echo = false;
            if trimmed.starts_with("$ ") {
                continue;
            }
        }

        if trimmed.is_empty() {
            continue;
        }

        result.push(line.to_string());
    }

    if result.is_empty() {
        "ok".to_string()
    } else {
        result.join("\n")
    }
}

fn is_bun_banner(line: &str) -> bool {
    (line.starts_with("bun run v") || line.starts_with("bun test v"))
        && line.contains(|c: char| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_run_banner_and_immediate_script_echo() {
        let output = "bun run v1.2.3 (abc123)\n\n$ tsc --noEmit\nsrc/index.ts(1,1): error TS1234: broken\n";

        assert_eq!(
            filter_bun_output(output),
            "src/index.ts(1,1): error TS1234: broken"
        );
    }

    #[test]
    fn filters_test_banner_but_keeps_test_output() {
        let output = "bun test v1.2.3 (abc123)\n\npass  tests/example.test.ts\n 1 pass\n";

        assert_eq!(
            filter_bun_output(output),
            "pass  tests/example.test.ts\n 1 pass"
        );
    }

    #[test]
    fn keeps_dollar_prefixed_program_output() {
        let output = "bun run v1.2.3\n$ node script.js\nstarted\n$ 10.00 total\n";

        assert_eq!(filter_bun_output(output), "started\n$ 10.00 total");
    }

    #[test]
    fn recognizes_ansi_colored_banners() {
        let output = "\u{1b}[1mbun run v1.2.3\u{1b}[0m\n\u{1b}[2m$ echo ready\u{1b}[0m\nready\n";

        assert_eq!(filter_bun_output(output), "ready");
    }

    #[test]
    fn returns_ok_for_boilerplate_only_output() {
        assert_eq!(filter_bun_output("bun run v1.2.3\n$ true\n"), "ok");
    }
}
