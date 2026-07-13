//! Filters directory listings into a compact tree format.

use super::ls_format::synthesize_output;
use super::ls_unix::compact_ls;
use crate::core::runner::{self, RunOptions};
use crate::core::utils::{resolved_command, tool_exists};
use anyhow::Result;
use std::io::IsTerminal;
use std::process::Command;

/// Represents a single directory entry for token-optimized listing.
#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum LsRecordType {
    FILE,
    DIRECTORY,
    SYMBOLINK,
    UNKNOWN,
}

#[derive(Debug, Clone)]
pub struct LsRecord {
    pub name: String,
    pub file_type: LsRecordType,
    pub size: u64,
    pub extension: String,
    pub timestamp: Option<u64>,
    pub octal_permissions: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct FormatOptions {
    pub show_all: bool,
    pub show_long: bool,
    pub sort_by_time: bool,
    pub reverse: bool,
}

pub fn parser(args: &[String]) -> (Vec<String>, Vec<String>, FormatOptions) {
    let show_all = args
        .iter()
        .any(|a| (a.starts_with('-') && !a.starts_with("--") && a.contains('a')) || a == "--all");

    let show_long = args.iter().any(|f| {
        f == "-l"
            || f == "-g"
            || f == "-n"
            || f == "-o"
            || f == "--full-time"
            || f == "--format=long"
            || f == "--format=verbose"
            || (f.starts_with('-') && !f.starts_with("--") && (f.chars().skip(1).any(|c| c == 'l' || c == 'g' || c == 'n' || c == 'o')))
    });

    let flags: Vec<String> = args
        .iter()
        .filter(|a| a.starts_with('-'))
        .cloned()
        .collect();
    let paths: Vec<String> = args
        .iter()
        .filter(|a| !a.starts_with('-'))
        .cloned()
        .collect();

    let sort_by_time = flags.iter().any(|f| f.starts_with('-') && !f.starts_with("--") && f.contains('t'));
    let reverse = flags.iter().any(|f| f.starts_with('-') && !f.starts_with("--") && f.contains('r'));

    (paths, flags, FormatOptions { show_all, show_long, sort_by_time, reverse })
}


pub fn cmd_builder(paths: &[String], flags: &[String], _show_all: bool) -> Command {
    let mut cmd = resolved_command("ls");
    cmd.arg("-la");
    for flag in flags {
        if flag.starts_with("--") {
            if flag != "--all" {
                cmd.arg(flag);
            }
        } else {
            let stripped = flag.trim_start_matches('-');
            let extra: String = stripped
                .chars()
                .filter(|c| *c != 'l' && *c != 'a' && *c != 'h')
                .collect();
            if !extra.is_empty() {
                cmd.arg(format!("-{}", extra));
            }
        }
    }

    if paths.is_empty() {
        cmd.arg(".");
    } else {
        for p in paths {
            cmd.arg(p);
        }
    }

    cmd
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let (paths, flags, options) = parser(args);

    #[cfg(windows)]
    {
        if !tool_exists("ls") {
            let timer = crate::core::tracking::TimedExecution::start();

            let (exit_code, output, raw_estimate) = super::ls_win::run_native(paths.clone(), options.clone(), flags.clone())?;
            print!("{}", output);

            timer.track(
                "ls -la",
                "rtk ls (native win)",
                &raw_estimate,
                &output,
            );

            return Ok(exit_code);
        }
    }

    let cmd = cmd_builder(&paths, &flags, options.show_all);

    let target_display = if paths.is_empty() {
        ".".to_string()
    } else {
        paths.join(" ")
    };

    runner::run_filtered(
        cmd,
        "ls",
        &format!("-la {}", target_display),
        |raw| {
            let result = compact_ls(raw, &options);
            let (entries, summary) = synthesize_output(result, &options);

            // Only show summary in interactive mode (not when piped)
            let is_tty = std::io::stdout().is_terminal();
            let filtered = if is_tty {
                format!("{}{}", entries, summary)
            } else {
                entries
            };

            if verbose > 0 {
                eprintln!(
                    "Chars: {} → {} ({}% reduction)",
                    raw.len(),
                    filtered.len(),
                    if !raw.is_empty() {
                        100 - (filtered.len() * 100 / raw.len())
                    } else {
                        0
                    }
                );
            }
            filtered
        },
        RunOptions::stdout_only()
            .early_exit_on_failure()
            .no_trailing_newline(),
    )
}

pub fn get_extension(name: &str) -> String {
    if let Some(pos) = name.rfind('.') {
        name[pos..].to_string()
    } else {
        "no ext".to_string()
    }
}


