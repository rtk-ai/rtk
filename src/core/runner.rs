//! Shared command execution skeleton for filter modules.

use anyhow::{Context, Result};
use std::process::Command;

use crate::core::stream::{self, FilterMode, StdinMode};
use crate::core::tracking;
use crate::core::utils::exit_code_from_status;

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

pub fn run_filtered<F>(
    mut cmd: Command,
    tool_name: &str,
    args_display: &str,
    filter_fn: F,
    opts: RunOptions<'_>,
) -> Result<i32>
where
    F: Fn(&str) -> String,
{
    let timer = tracking::TimedExecution::start();

    // CaptureOnly: stderr streams live, stdout buffered silently.
    // result.filtered = raw_stdout, result.raw = stdout + stderr
    let result = stream::run_streaming(&mut cmd, StdinMode::Null, FilterMode::CaptureOnly)
        .with_context(|| format!("Failed to run {}", tool_name))?;

    let exit_code = result.exit_code;
    let raw_stdout = &result.filtered;
    let raw = &result.raw;

    if opts.skip_filter_on_failure && exit_code != 0 {
        timer.track(
            &format!("{} {}", tool_name, args_display),
            &format!("rtk {} {}", tool_name, args_display),
            raw,
            raw,
        );
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
        &format!("{} {}", tool_name, args_display),
        &format!("rtk {} {}", tool_name, args_display),
        raw_for_tracking,
        &filtered,
    );

    Ok(exit_code)
}

pub fn run_passthrough(tool: &str, args: &[std::ffi::OsString], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    if verbose > 0 {
        eprintln!("{} passthrough: {:?}", tool, args);
    }
    let status = crate::core::utils::resolved_command(tool)
        .args(args)
        .status()
        .with_context(|| format!("Failed to run {}", tool))?;
    let args_str = tracking::args_display(args);
    timer.track_passthrough(
        &format!("{} {}", tool, args_str),
        &format!("rtk {} {} (passthrough)", tool, args_str),
    );
    Ok(exit_code_from_status(&status, tool))
}
