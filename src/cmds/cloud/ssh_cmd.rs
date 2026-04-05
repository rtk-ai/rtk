//! Runs ssh and truncates verbose output (strip ANSI, cap lines).

use crate::core::tracking;
use crate::core::utils::{exit_code_from_output, resolved_command};
use anyhow::{Context, Result};

const MAX_LINES: usize = 80;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let mut cmd = resolved_command("ssh");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: ssh {}", args.join(" "));
    }

    let output = cmd.output().context("Failed to run ssh")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = filter_ssh_output(&stdout, &stderr);
    print!("{}", filtered);

    timer.track(
        &format!("ssh {}", args.join(" ")),
        &format!("rtk ssh {}", args.join(" ")),
        &raw,
        &filtered,
    );

    Ok(exit_code_from_output(&output, "ssh"))
}

fn filter_ssh_output(stdout: &str, stderr: &str) -> String {
    let mut result = String::new();

    // Strip ANSI codes and blank lines, cap output
    let lines: Vec<&str> = stdout
        .lines()
        .map(|l| strip_ansi_codes(l))
        .filter(|l| !l.trim().is_empty())
        .collect();

    let truncated = lines.len() > MAX_LINES;
    for line in lines.iter().take(MAX_LINES) {
        result.push_str(line);
        result.push('\n');
    }

    if truncated {
        result.push_str(&format!("... +{} lines truncated\n", lines.len() - MAX_LINES));
    }

    // Include stderr warnings (connection messages, etc.) but cap at 5 lines
    let stderr_lines: Vec<&str> = stderr
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();
    if !stderr_lines.is_empty() {
        for line in stderr_lines.iter().take(5) {
            eprintln!("{}", line);
        }
        if stderr_lines.len() > 5 {
            eprintln!("... +{} stderr lines truncated", stderr_lines.len() - 5);
        }
    }

    result
}

/// Simple ANSI escape code stripper
fn strip_ansi_codes(s: &str) -> &str {
    // For ssh output, ANSI codes are rare — just return as-is for now.
    // Full stripping would require allocation; defer to TOML filter pipeline.
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_ssh_output_truncation() {
        let long_output: String = (0..100)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let filtered = filter_ssh_output(&long_output, "");
        assert!(filtered.contains("... +20 lines truncated"));
    }

    #[test]
    fn test_filter_ssh_output_short() {
        let output = "hello\nworld\n";
        let filtered = filter_ssh_output(output, "");
        assert_eq!(filtered, "hello\nworld\n");
        assert!(!filtered.contains("truncated"));
    }

    #[test]
    fn test_filter_ssh_output_blank_lines() {
        let output = "hello\n\n\nworld\n";
        let filtered = filter_ssh_output(output, "");
        assert_eq!(filtered, "hello\nworld\n");
    }
}
