//! Shared command execution skeleton for filter modules.

use anyhow::{Context, Result};
use std::process::Command;

use crate::core::stream::{self, FilterMode, StdinMode, StreamFilter};
use crate::core::tracking;

pub fn print_with_hint(filtered: &str, raw: &str, tee_label: &str, exit_code: i32) {
    if let Some(hint) = crate::core::tee::tee_and_hint(raw, tee_label, exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }
}

#[derive(Default)]
pub struct RunOptions<'a> {
    pub tee_label: Option<&'a str>,
    pub filter_stdout_only: bool,
    pub skip_filter_on_failure: bool,
    pub no_trailing_newline: bool,
}

impl<'a> RunOptions<'a> {
    pub fn with_tee(label: &'a str) -> Self {
        Self {
            tee_label: Some(label),
            ..Default::default()
        }
    }

    pub fn stdout_only() -> Self {
        Self {
            filter_stdout_only: true,
            ..Default::default()
        }
    }

    pub fn tee(mut self, label: &'a str) -> Self {
        self.tee_label = Some(label);
        self
    }

    pub fn early_exit_on_failure(mut self) -> Self {
        self.skip_filter_on_failure = true;
        self
    }

    pub fn no_trailing_newline(mut self) -> Self {
        self.no_trailing_newline = true;
        self
    }
}

pub enum RunMode<'a> {
    Filtered(Box<dyn Fn(&str) -> String + 'a>),
    Streamed(Box<dyn StreamFilter + 'a>),
    Passthrough,
}

/// Detect long-running invocations before `RunMode::Filtered` captures output.
///
/// `args_display` must be structured command arguments, not user-controlled prose.
/// `RunMode::Streamed` and `RunMode::Passthrough` do not call this helper.
fn is_streaming_invocation(tool_name: &str, args_display: &str) -> bool {
    let tool = tool_name.split_whitespace().next().unwrap_or(tool_name);
    let args: Vec<&str> = args_display.split_whitespace().collect();

    if args
        .iter()
        .any(|arg| matches!(*arg, "--watch" | "--watchAll"))
    {
        return true;
    }

    if args.contains(&"-w") && matches!(tool, "jest" | "vitest" | "webpack" | "nodemon") {
        return true;
    }

    let follows = args.iter().any(|arg| matches!(*arg, "--follow" | "-f"));
    if follows && matches!(tool, "docker" | "kubectl" | "tail" | "journalctl") {
        return true;
    }

    (tool == "go" && args.contains(&"run"))
        || (tool == "playwright" && args.contains(&"codegen"))
        || matches!(tool, "tail" | "nodemon" | "watchman")
        || args.iter().any(|arg| {
            matches!(
                *arg,
                "watch" | "dev" | "serve" | "start" | "codegen" | "show-report"
            )
        })
}

pub fn run(
    mut cmd: Command,
    tool_name: &str,
    args_display: &str,
    mode: RunMode<'_>,
    opts: RunOptions<'_>,
) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let cmd_label = format!("{} {}", tool_name, args_display);

    match mode {
        RunMode::Filtered(filter_fn) => {
            if is_streaming_invocation(tool_name, args_display) {
                let result =
                    stream::run_streaming(&mut cmd, StdinMode::Inherit, FilterMode::Passthrough)
                        .with_context(|| format!("Failed to run {}", tool_name))?;

                timer.track_passthrough(&cmd_label, &format!("rtk {} (streaming)", cmd_label));
                return Ok(result.exit_code);
            }

            let result = stream::run_streaming(&mut cmd, StdinMode::Null, FilterMode::CaptureOnly)
                .with_context(|| format!("Failed to run {}", tool_name))?;

            let exit_code = result.exit_code;
            let raw = &result.raw;
            let raw_stdout = &result.raw_stdout;

            if opts.skip_filter_on_failure && exit_code != 0 {
                if !result.raw_stdout.trim().is_empty() {
                    print!("{}", result.raw_stdout);
                }
                if !result.raw_stderr.trim().is_empty() {
                    eprint!("{}", result.raw_stderr);
                }
                timer.track(&cmd_label, &format!("rtk {}", cmd_label), raw, raw);
                return Ok(exit_code);
            }

            let text_to_filter = if opts.filter_stdout_only {
                raw_stdout
            } else {
                raw
            };
            let filtered = filter_fn(text_to_filter);

            if let Some(label) = opts.tee_label {
                print_with_hint(&filtered, raw, label, exit_code);
            } else if opts.no_trailing_newline {
                print!("{}", filtered);
            } else {
                println!("{}", filtered);
            }

            let raw_for_tracking = if opts.filter_stdout_only {
                raw_stdout
            } else {
                raw
            };
            timer.track(
                &cmd_label,
                &format!("rtk {}", cmd_label),
                raw_for_tracking,
                &filtered,
            );
            Ok(exit_code)
        }
        RunMode::Streamed(filter) => {
            let result =
                stream::run_streaming(&mut cmd, StdinMode::Null, FilterMode::Streaming(filter))
                    .with_context(|| format!("Failed to run {}", tool_name))?;

            if let Some(label) = opts.tee_label {
                if let Some(hint) =
                    crate::core::tee::tee_and_hint(&result.raw, label, result.exit_code)
                {
                    println!("{}", hint);
                }
            }

            timer.track(
                &cmd_label,
                &format!("rtk {}", cmd_label),
                &result.raw,
                &result.filtered,
            );
            Ok(result.exit_code)
        }
        RunMode::Passthrough => {
            let result =
                stream::run_streaming(&mut cmd, StdinMode::Inherit, FilterMode::Passthrough)
                    .with_context(|| format!("Failed to run {}", tool_name))?;

            timer.track_passthrough(&cmd_label, &format!("rtk {} (passthrough)", cmd_label));
            Ok(result.exit_code)
        }
    }
}

pub fn run_filtered<F>(
    cmd: Command,
    tool_name: &str,
    args_display: &str,
    filter_fn: F,
    opts: RunOptions<'_>,
) -> Result<i32>
where
    F: Fn(&str) -> String,
{
    run(
        cmd,
        tool_name,
        args_display,
        RunMode::Filtered(Box::new(filter_fn)),
        opts,
    )
}

pub fn run_passthrough(tool: &str, args: &[std::ffi::OsString], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("{} passthrough: {:?}", tool, args);
    }
    let mut cmd = crate::core::utils::resolved_command(tool);
    cmd.args(args);
    let args_str = tracking::args_display(args);
    run(
        cmd,
        tool,
        &args_str,
        RunMode::Passthrough,
        RunOptions::default(),
    )
}

pub fn run_streamed(
    cmd: Command,
    tool_name: &str,
    args_display: &str,
    filter: Box<dyn StreamFilter + '_>,
    opts: RunOptions<'_>,
) -> Result<i32> {
    run(
        cmd,
        tool_name,
        args_display,
        RunMode::Streamed(filter),
        opts,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_streaming_invocation_detects_watch_flags() {
        assert!(is_streaming_invocation("prettier", "--watch ."));
        assert!(is_streaming_invocation("gh", "pr checks 123 --watch"));
        assert!(is_streaming_invocation("jest", "run -w"));
    }

    #[test]
    fn test_is_streaming_invocation_scopes_short_watch_flag() {
        assert!(is_streaming_invocation("vitest", "run -w"));
        assert!(!is_streaming_invocation("wc", "-w src/main.rs"));
        assert!(!is_streaming_invocation("cargo", "test -w"));
    }

    #[test]
    fn test_is_streaming_invocation_detects_follow_flags_for_log_tools() {
        assert!(is_streaming_invocation("docker", "logs -f web"));
        assert!(is_streaming_invocation("kubectl", "logs --follow web"));
        assert!(!is_streaming_invocation("git", "log -f"));
    }

    #[test]
    fn test_is_streaming_invocation_detects_dev_and_watch_subcommands() {
        assert!(is_streaming_invocation("npm", "run dev"));
        assert!(is_streaming_invocation("pnpm", "run dev"));
        assert!(is_streaming_invocation("npm", "start"));
        assert!(is_streaming_invocation("playwright", "codegen"));
        assert!(is_streaming_invocation("dotnet", "watch run"));
        assert!(is_streaming_invocation("go", "run ."));
    }

    #[test]
    fn test_is_streaming_invocation_keeps_finite_commands_filterable() {
        assert!(!is_streaming_invocation("npm", "run build"));
        assert!(!is_streaming_invocation("gh", "pr checks 123"));
        assert!(!is_streaming_invocation("go", "test ./..."));
        assert!(!is_streaming_invocation("go test", "./..."));
        assert!(!is_streaming_invocation("dotnet", "build"));
    }
}
