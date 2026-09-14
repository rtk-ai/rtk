//! Runs ssh and filters output for non-interactive (remote-command) invocations.
//!
//! Interactive sessions (no remote command after the hostname) are passed through
//! with inherited stdio so PTY allocation, password prompts, and control sequences
//! all work as normal. Only invocations that run a remote command and exit are
//! eligible for output filtering.

use crate::core::tee::force_tee_hint;
use crate::core::tracking;
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};
use std::io::Write;

const MAX_OUTPUT_SIZE: usize = 2000;

/// SSH flags that consume the next token as a separate value argument.
/// Flags with an embedded value (e.g. `-p22`) still only advance by one.
const VALUE_FLAGS: &[char] = &[
    'b', 'c', 'D', 'E', 'e', 'F', 'I', 'i', 'J', 'l', 'L', 'm', 'o', 'p', 'Q', 'R', 'S', 'W',
    'w',
];

/// Returns true when the argument list contains a remote command after the
/// hostname, meaning the session is non-interactive and its output is capturable.
fn has_remote_command(args: &[String]) -> bool {
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            // Everything after `--` is: hostname [remote_command...]
            return args.len() > i + 2;
        }
        if arg.starts_with('-') && arg.len() >= 2 {
            let flag_char = arg.chars().nth(1).unwrap_or('\0');
            // Two-char flag whose value is the NEXT argument (e.g. `-i ~/.ssh/id_rsa`)
            if arg.len() == 2 && VALUE_FLAGS.contains(&flag_char) {
                i += 2;
            } else {
                i += 1;
            }
        } else {
            // First non-flag token is the hostname; anything after it is the remote command.
            return args.len() > i + 1;
        }
    }
    false
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if !has_remote_command(args) {
        return run_passthrough(args);
    }

    let timer = tracking::TimedExecution::start();
    let mut cmd = resolved_command("ssh");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: ssh {}", args.join(" "));
    }

    let output = cmd.output().context("Failed to run ssh")?;
    let exit_code = output.status.code().unwrap_or(1);

    // On failure emit everything and return; don't truncate error output.
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.trim().is_empty() {
            eprint!("{}", stderr);
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.trim().is_empty() {
            print!("{}", stdout);
        }
        return Ok(exit_code);
    }

    // Always forward stderr (connection banners, warnings) verbatim.
    if !output.stderr.is_empty() {
        let _ = std::io::stderr().lock().write_all(&output.stderr);
    }

    let raw = String::from_utf8_lossy(&output.stdout).into_owned();
    let filtered = filter_output(&raw);

    print!("{}", filtered.content);
    if let Some(hint) = &filtered.tee_hint {
        println!("\n{}", hint);
    }

    timer.track(
        &format!("ssh {}", args.join(" ")),
        &format!("rtk ssh {}", args.join(" ")),
        &raw,
        &filtered.content,
    );

    Ok(exit_code)
}

fn run_passthrough(args: &[String]) -> Result<i32> {
    let status = resolved_command("ssh")
        .args(args)
        .status()
        .context("Failed to run ssh")?;
    Ok(status.code().unwrap_or(1))
}

struct FilterResult {
    content: String,
    tee_hint: Option<String>,
}

fn filter_output(raw: &str) -> FilterResult {
    let trimmed = raw.trim();
    if trimmed.len() <= MAX_OUTPUT_SIZE {
        return FilterResult {
            content: trimmed.to_string(),
            tee_hint: None,
        };
    }

    let Some(hint) = force_tee_hint(raw, "ssh") else {
        return FilterResult {
            content: trimmed.to_string(),
            tee_hint: None,
        };
    };

    let mut end = MAX_OUTPUT_SIZE;
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    FilterResult {
        content: format!("{}... ({} bytes total)", &trimmed[..end], trimmed.len()),
        tee_hint: Some(hint),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn test_has_remote_command_with_command() {
        assert!(has_remote_command(&args("myserver ls /opt")));
        assert!(has_remote_command(&args("-i ~/.ssh/id_rsa user@host find /opt -name '*.log'")));
        assert!(has_remote_command(&args("-A -J jump user@10.0.0.1 journalctl -u app")));
        assert!(has_remote_command(&args("-p 2222 host cat /etc/hosts")));
    }

    #[test]
    fn test_no_remote_command_interactive() {
        assert!(!has_remote_command(&args("myserver")));
        assert!(!has_remote_command(&args("-A myserver")));
        assert!(!has_remote_command(&args("-i key user@host")));
        assert!(!has_remote_command(&args("")));
    }

    #[test]
    fn test_double_dash_separator() {
        assert!(has_remote_command(&args("-- host ls /tmp")));
        assert!(!has_remote_command(&args("-- host")));
    }

    #[test]
    fn test_filter_output_short_passthrough() {
        let raw = "total 4\ndrwxr-xr-x 2 root root 4096 Jan 1 00:00 .\n";
        let result = filter_output(raw);
        assert!(!result.content.contains("bytes total"));
        assert!(result.tee_hint.is_none());
    }

    #[test]
    fn test_filter_output_long_truncates() {
        let raw = "x".repeat(5000);
        let result = filter_output(&raw);
        assert!(result.content.contains("bytes total"));
        assert!(result.content.len() < 2200);
    }

    #[test]
    fn test_filter_output_multibyte_boundary() {
        let raw = "a".repeat(1999) + "é";
        let result = filter_output(&raw);
        // "é" is 2 bytes; boundary adjustment must not split it
        assert!(result.content.is_char_boundary(result.content.len()));
    }
}
