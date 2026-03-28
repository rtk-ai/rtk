//! Shared command execution skeleton for filter modules.

use anyhow::{Context, Result};
use std::process::Command;

use crate::core::tracking;
use crate::core::utils::exit_code_from_output;

pub fn capture_output(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("{}\n{}", stdout, stderr)
}

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

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run {}", tool_name))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let text_to_filter = if opts.filter_stdout_only {
        &stdout
    } else {
        raw.as_str()
    };
    let filtered = filter_fn(text_to_filter);

    let exit_code = exit_code_from_output(&output, tool_name);

    if let Some(label) = opts.tee_label {
        print_with_hint(&filtered, &raw, label, exit_code);
    } else {
        println!("{}", filtered);
    }

    if opts.filter_stdout_only && !stderr.trim().is_empty() {
        eprintln!("{}", stderr.trim());
    }

    timer.track(
        &format!("{} {}", tool_name, args_display),
        &format!("rtk {} {}", tool_name, args_display),
        &raw,
        &filtered,
    );

    Ok(exit_code)
}
