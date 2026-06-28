//! Shared command execution skeleton for filter modules.

use anyhow::{Context, Result};
use std::process::Command;

use crate::core::config;
use crate::core::stream::{self, FilterMode, StdinMode, StreamFilter};
use crate::core::tracking;

/// Compose `filtered` with an optional recovery `hint`, cap the total at `raw`
/// (never emit more tokens than the command), print it, and return what was
/// emitted so the caller tracks exactly that.
pub fn emit_guarded(filtered: &str, hint: Option<&str>, raw: &str) -> String {
    let body = match hint {
        Some(h) => format!("{}\n{}", filtered, h),
        None => filtered.to_string(),
    };
    let shown = crate::core::guard::never_worse(raw, &body).to_string();
    println!("{}", shown);
    shown
}

pub fn print_with_hint(
    filtered: &str,
    tee_raw: &str,
    guard_raw: &str,
    tee_label: &str,
    exit_code: i32,
) -> String {
    let hint = crate::core::tee::tee_and_hint(tee_raw, tee_label, exit_code);
    emit_guarded(filtered, hint.as_deref(), guard_raw)
}

#[derive(Default)]
pub struct RunOptions<'a> {
    pub tee_label: Option<&'a str>,
    pub filter_stdout_only: bool,
    pub skip_filter_on_failure: bool,
    pub no_trailing_newline: bool,
    /// Forward rtk's own stdin to the child process. Needed for commands that
    /// can read from a pipe (e.g. `cat file | rtk wc`); without it the child
    /// gets an empty stdin and reports zero.
    pub inherit_stdin: bool,
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

    pub fn inherit_stdin(mut self) -> Self {
        self.inherit_stdin = true;
        self
    }
}

pub type CaptureFilter<'a> = Box<dyn Fn(&str) -> String + 'a>;
pub type ExitAwareCaptureFilter<'a> = Box<dyn Fn(&str, i32) -> String + 'a>;

/// Whether the raw output is short enough that compression adds no value.
/// When true, the caller should emit the raw output unchanged.
///
/// Both conditions must be met (AND): short lines AND small bytes.
fn should_auto_passthrough(output: &str, cfg: &config::PassthroughConfig) -> bool {
    let line_threshold = cfg.effective_line_threshold();
    let byte_threshold = cfg.effective_byte_threshold();

    let line_count = output.lines().count();
    line_count <= line_threshold && output.len() <= byte_threshold
}

pub enum RunMode<'a> {
    Filtered(CaptureFilter<'a>),
    FilteredWithExit(ExitAwareCaptureFilter<'a>),
    Streamed(Box<dyn StreamFilter + 'a>),
    Passthrough,
}

fn run_captured_filter<F>(
    mut cmd: Command,
    tool_name: &str,
    cmd_label: &str,
    filter_fn: F,
    opts: RunOptions<'_>,
    timer: tracking::TimedExecution,
) -> Result<i32>
where
    F: Fn(&str, i32) -> String,
{
    let stdin_mode = if opts.inherit_stdin {
        StdinMode::Inherit
    } else {
        StdinMode::Null
    };
    let result = stream::run_streaming(&mut cmd, stdin_mode, FilterMode::CaptureOnly)
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
        timer.track(cmd_label, &format!("rtk {}", cmd_label), raw, raw);
        return Ok(exit_code);
    }

    let text_to_filter = if opts.filter_stdout_only {
        raw_stdout
    } else {
        raw
    };

    // Short-output auto-passthrough: skip compression when output is already minimal.
    let passthrough_cfg = config::passthrough();
    if should_auto_passthrough(text_to_filter, &passthrough_cfg) {
        if opts.no_trailing_newline {
            print!("{}", text_to_filter);
        } else {
            println!("{}", text_to_filter);
        }
        // Emit stderr if it exists and wasn't already included in the filtered text.
        if !opts.filter_stdout_only && !result.raw_stderr.trim().is_empty() {
            eprint!("{}", result.raw_stderr);
        }
        timer.track_passthrough(cmd_label, &format!("rtk {} (auto-passthrough)", cmd_label));
        return Ok(exit_code);
    }

    let filtered = filter_fn(text_to_filter, exit_code);

    let raw_for_tracking = if opts.filter_stdout_only {
        raw_stdout
    } else {
        raw
    };

    let shown = if let Some(label) = opts.tee_label {
        print_with_hint(&filtered, raw, raw_for_tracking, label, exit_code)
    } else {
        let guarded = crate::core::guard::never_worse(raw_for_tracking, &filtered).to_string();
        if opts.no_trailing_newline {
            print!("{}", guarded);
        } else {
            println!("{}", guarded);
        }
        guarded
    };

    timer.track(
        cmd_label,
        &format!("rtk {}", cmd_label),
        raw_for_tracking,
        &shown,
    );
    Ok(exit_code)
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
        RunMode::Filtered(filter_fn) => run_captured_filter(
            cmd,
            tool_name,
            &cmd_label,
            move |text, _| filter_fn(text),
            opts,
            timer,
        ),
        RunMode::FilteredWithExit(filter_fn) => run_captured_filter(
            cmd,
            tool_name,
            &cmd_label,
            move |text, exit_code| filter_fn(text, exit_code),
            opts,
            timer,
        ),
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

pub fn run_filtered_with_exit<F>(
    cmd: Command,
    tool_name: &str,
    args_display: &str,
    filter_fn: F,
    opts: RunOptions<'_>,
) -> Result<i32>
where
    F: Fn(&str, i32) -> String,
{
    run(
        cmd,
        tool_name,
        args_display,
        RunMode::FilteredWithExit(Box::new(filter_fn)),
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

    /// Default config: both thresholds 0 → effective defaults 5/500 kick in.
    fn default_cfg() -> config::PassthroughConfig {
        config::PassthroughConfig::default()
    }

    #[test]
    fn test_should_auto_passthrough_short_output() {
        assert!(should_auto_passthrough("ok\n", &default_cfg()));
        assert!(should_auto_passthrough(
            "line1\nline2\nline3\n",
            &default_cfg()
        ));
        assert!(should_auto_passthrough(
            "To github.com:user/repo.git\n   abc..def main -> main\n",
            &default_cfg()
        ));
    }

    #[test]
    fn test_should_auto_passthrough_long_output() {
        let six_lines = "1\n2\n3\n4\n5\n6\n";
        assert!(!should_auto_passthrough(six_lines, &default_cfg()));
    }

    #[test]
    fn test_should_auto_passthrough_large_bytes() {
        // ≤5 lines but >500 bytes → no passthrough
        let long_line = format!("{}\n", "x".repeat(501));
        assert!(!should_auto_passthrough(&long_line, &default_cfg()));
    }

    #[test]
    fn test_should_auto_passthrough_empty_output() {
        assert!(should_auto_passthrough("", &default_cfg()));
    }

    #[test]
    fn test_should_auto_passthrough_exact_threshold() {
        let five_lines = "1\n2\n3\n4\n5\n";
        assert!(should_auto_passthrough(five_lines, &default_cfg()));

        let six_lines = "1\n2\n3\n4\n5\n6\n";
        assert!(!should_auto_passthrough(six_lines, &default_cfg()));
    }

    #[test]
    fn test_should_auto_passthrough_both_conditions_required() {
        // 3 lines (≤5) but >500 bytes → no passthrough
        let fat = format!("a\nb\n{}\n", "x".repeat(500));
        assert!(!should_auto_passthrough(&fat, &default_cfg()));
    }

    #[test]
    fn test_should_auto_passthrough_custom_thresholds() {
        let cfg = config::PassthroughConfig {
            short_line_threshold: 2,
            short_byte_threshold: 100,
        };
        assert!(should_auto_passthrough("ok\n", &cfg));
        assert!(!should_auto_passthrough("1\n2\n3\n", &cfg)); // 3 lines > 2
    }

    #[test]
    fn test_should_auto_passthrough_opt_out_via_tiny_thresholds() {
        // To opt out, set short_line_threshold = 1 and short_byte_threshold = 1.
        let cfg = config::PassthroughConfig {
            short_line_threshold: 1,
            short_byte_threshold: 1,
        };
        // "ok\n" is 3 bytes > 1 → no passthrough
        assert!(!should_auto_passthrough("ok\n", &cfg));
        // "" is 0 bytes ≤ 1 and 0 lines ≤ 1 → passthrough (empty is always safe)
        assert!(should_auto_passthrough("", &cfg));
    }
}
