//! Filters bun output, mirroring the npm/npx filters which don't cover bun.
//!
//! `bun run <script>` / `bun test` / `bun x <tool>` and `bunx <tool>` all share
//! the same boilerplate: a `bun run v1.x.x` banner and a `$ <command>` echo line
//! that add no signal once Claude already knows the command it ran.

use crate::core::runner;
use crate::core::utils::resolved_command;
use anyhow::Result;

/// Run `bun <args>` (e.g. `bun run build`, `bun test`, `bun x tsc`) filtered.
///
/// The rewrite layer only routes `bun run|x|test` here, so `args` already starts
/// with the bun subcommand — no "run" injection is needed (and would be wrong,
/// since `bun build` is bun's bundler, not a script run).
pub fn run(args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    run_filtered("bun", args, verbose, skip_env)
}

/// Run a `bunx <tool>` invocation through the same filtered pipeline.
///
/// Used for unrouted tools in the `Commands::Bunx` fallback so that
/// `rtk bunx cowsay hello` dispatches to `bunx`, not `bun`.
pub fn exec(args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    run_filtered("bunx", args, verbose, skip_env)
}

/// Shared command-execution path for `run` (bun) and `exec` (bunx).
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

/// Filter bun output - strip the `bun run v…` banner, the `$ <cmd>` echo, blanks.
fn filter_bun_output(output: &str) -> String {
    let mut result = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim_start();
        // Skip bun's version banner: "bun run v1.1.0", "bun test v1.1.0"
        if (trimmed.starts_with("bun run v") || trimmed.starts_with("bun test v"))
            && trimmed.contains(|c: char| c.is_ascii_digit())
        {
            continue;
        }
        // Skip the echoed script command: "$ tsc --noEmit"
        if trimmed.starts_with("$ ") {
            continue;
        }
        // Skip empty lines
        if line.trim().is_empty() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_bun_output() {
        let output = r#"bun run v1.1.0
$ tsc --noEmit

src/index.ts(4,7): error TS2322: Type 'number' is not assignable.

"#;
        let result = filter_bun_output(output);
        assert!(!result.contains("bun run v"));
        assert!(!result.contains("$ tsc"));
        assert!(result.contains("error TS2322"));
    }

    #[test]
    fn test_filter_bun_test_banner() {
        let output = "bun test v1.1.0\n\n 5 pass\n 0 fail\n";
        let result = filter_bun_output(output);
        assert!(!result.contains("bun test v"));
        assert!(result.contains("5 pass"));
    }

    #[test]
    fn test_filter_bun_output_empty() {
        let output = "bun run v1.1.0\n$ true\n\n\n";
        let result = filter_bun_output(output);
        assert_eq!(result, "ok");
    }
}
