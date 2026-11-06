//! `fnm` (Fast Node Manager) handler.
//!
//! `fnm env` prints shell code that the caller evaluates, so its output is
//! passed through byte-for-byte. Every other subcommand is lightly compacted:
//! ANSI escapes and blank lines are dropped, and `fnm use` no longer repeats
//! the installed-version table after the `Using Node ...` result.

use crate::core::shell::{display_args, with_args};
use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::{Context, Result};

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let tracked = with_args("fnm", &display_args(args));

    if verbose > 0 {
        eprintln!("fnm {}", display_args(args));
    }

    let mut cmd = resolved_command("fnm");
    for arg in args {
        cmd.arg(arg);
    }

    let result = exec_capture(&mut cmd).context("Failed to run fnm")?;
    let combined = result.combined();

    if !result.success() {
        if !result.stderr.trim().is_empty() {
            eprint!("{}", result.stderr);
        }
        if !result.stdout.trim().is_empty() {
            print!("{}", result.stdout);
        }
        timer.track(&tracked, &format!("rtk {tracked}"), &combined, &combined);
        return Ok(result.exit_code);
    }

    // `eval "$(fnm env)"` consumes this output as shell code: removing a line
    // would break the environment it installs.
    let output = if args.first().is_some_and(|arg| arg == "env") {
        result.stdout.clone()
    } else {
        let drop_version_table =
            matches!(args.first().map(String::as_str), Some("use" | "default"));
        filter_fnm_output(&result.stdout, drop_version_table)
    };

    if !output.is_empty() {
        println!("{}", output);
    }

    timer.track(&tracked, &format!("rtk {tracked}"), &combined, &output);
    Ok(0)
}

/// Compact a successful `fnm` listing.
///
/// `drop_version_table` is set for `fnm use`/`fnm default`, where fnm may print
/// the installed versions after the `Using Node ...` line that actually answers
/// the command. It is not set for `fnm list`, whose whole output is that table.
pub(crate) fn filter_fnm_output(output: &str, drop_version_table: bool) -> String {
    let cleaned = strip_ansi(output);
    let mut result: Vec<&str> = Vec::new();

    for line in cleaned.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if drop_version_table && is_installed_version_row(trimmed) {
            continue;
        }
        if result.last().is_some_and(|last| last.trim() == trimmed) {
            continue;
        }
        result.push(line);
    }

    result.join("\n")
}

/// `* v20.11.0`, `  v18.20.4 default`, or `system` — the rows `fnm use` repeats.
fn is_installed_version_row(line: &str) -> bool {
    let rest = line.trim_start_matches(['*', ' ', '\t']);
    if rest == "system" || rest == "default" {
        return true;
    }
    let first = rest.strip_prefix('v').unwrap_or(rest);
    let Some(version) = first.split_whitespace().next() else {
        return false;
    };
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_drops_the_installed_version_table() {
        let output = "Using Node v20.11.0\n\n* v18.20.4\n* v20.11.0 default\n  system\n";
        assert_eq!(
            filter_fnm_output(output, true),
            "Using Node v20.11.0",
            "the switch result is kept, the table after it is not"
        );
    }

    #[test]
    fn list_keeps_the_version_table() {
        let output = "* v18.20.4\n* v20.11.0 default\n  system\n";
        assert_eq!(filter_fnm_output(output, false), output.trim_end());
    }

    #[test]
    fn filter_drops_blank_lines_and_consecutive_duplicates() {
        let output = "Using Node v20.11.0\n\nUsing Node v20.11.0\n";
        assert_eq!(filter_fnm_output(output, true), "Using Node v20.11.0");
    }

    #[test]
    fn filter_keeps_unrecognized_output() {
        let output = "error: version 99 is not installed\nrun `fnm install 99`\n";
        assert_eq!(
            filter_fnm_output(output, true),
            "error: version 99 is not installed\nrun `fnm install 99`"
        );
    }
}
