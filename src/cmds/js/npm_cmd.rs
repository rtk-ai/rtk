//! Filters npm output — strips boilerplate, progress bars, and warnings.

use crate::core::runner;
use crate::core::utils::resolved_command;
use anyhow::Result;
use std::io::IsTerminal;

/// npm's own argv is forwarded verbatim: `rtk npm <anything>` runs
/// `npm <anything>`. RTK does not guess whether the first argument is a
/// subcommand or a package.json script, so `npm build` fails exactly as it
/// would without RTK and every subcommand npm gains keeps working (#2663).
pub fn run(args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    run_filtered("npm", args, verbose, skip_env, false)
}

/// Run an npx tool through the same filtered pipeline as `npm`.
///
/// Used for unrouted tools in the `Commands::Npx` fallback so that
/// `rtk npx cowsay hello` dispatches to `npx`, not `npm`. Honors `--skip-env`
/// the same way `run` does.
pub fn exec(args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    run_filtered("npx", args, verbose, skip_env, false)
}

/// Run an arbitrary package runner (`npx`, `bunx`) under the npm output filter.
///
/// This is the light line filter, not the errors-only one: these runners host
/// tools whose entire value is their stdout, so dropping non-error lines would
/// return nothing.
pub fn exec_with(runner: &str, args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    run_filtered(runner, args, verbose, skip_env, true)
}

/// Shared command-execution path for `run` (npm) and `exec` (npx).
///
/// Builds the resolved command, appends args, applies `SKIP_ENV_VALIDATION`,
/// emits the verbose log line, and routes through `runner::run_filtered` with
/// the npm output filter.
/// `forward_piped_stdin` sends the caller's stdin to the child when it is a
/// pipe rather than a terminal. The filtered path buffers output until exit, so
/// inheriting a terminal would let an interactive tool prompt invisibly and wait
/// forever; a pipe already holds its data and cannot do that.
///
/// `bunx` opts in, `npx` does not: npx's behaviour here predates this path and
/// is tracked on its own as rtk-ai/rtk#2431, which covers every filtered entry
/// point rather than this one.
fn run_filtered(
    name: &str,
    args: &[String],
    verbose: u8,
    skip_env: bool,
    forward_piped_stdin: bool,
) -> Result<i32> {
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

    let mut opts = runner::RunOptions::default();
    if forward_piped_stdin && !std::io::stdin().is_terminal() {
        opts = opts.inherit_stdin();
    }

    runner::run_filtered(cmd, name, &args_display, filter_npm_output, opts)
}

/// Filter npm run output - strip boilerplate, progress bars, npm WARN
fn filter_npm_output(output: &str) -> String {
    let mut result = Vec::new();

    for line in output.lines() {
        // Skip npm boilerplate
        if line.starts_with('>') && line.contains('@') {
            continue;
        }
        // Skip npm lifecycle scripts
        if line.trim_start().starts_with("npm WARN") {
            continue;
        }
        if line.trim_start().starts_with("npm notice") {
            continue;
        }
        // Skip progress indicators
        if line.contains("⸩") || line.contains("⸨") || line.contains("...") && line.len() < 10 {
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
    fn test_filter_npm_output() {
        let output = r#"
> project@1.0.0 build
> next build

npm WARN deprecated inflight@1.0.6: This module is not supported
npm notice

   Creating an optimized production build...
   ✓ Build completed
"#;
        let result = filter_npm_output(output);
        assert!(!result.contains("npm WARN"));
        assert!(!result.contains("npm notice"));
        assert!(!result.contains("> project@"));
        assert!(result.contains("Build completed"));
    }

    #[test]
    fn test_filter_npm_output_empty() {
        let output = "\n\n\n";
        let result = filter_npm_output(output);
        assert_eq!(result, "ok");
    }
}
