//! Runs ssh and compacts output (strips blank lines, caps at 80 lines).
//!
//! SSH remote commands can produce verbose output (docker logs, systemctl status,
//! journal dumps) that wastes tokens when piped back to an LLM. This handler
//! passes through the command unchanged but truncates the result.

use crate::core::tracking;
use crate::core::utils::{exit_code_from_output, resolved_command};
use anyhow::{Context, Result};

const MAX_LINES: usize = 80;
const MAX_STDERR_LINES: usize = 5;

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

    // Drop blank lines to reduce noise, then cap total output
    let lines: Vec<&str> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();

    let truncated = lines.len() > MAX_LINES;
    for line in lines.iter().take(MAX_LINES) {
        result.push_str(line);
        result.push('\n');
    }

    if truncated {
        result.push_str(&format!(
            "... +{} lines truncated\n",
            lines.len() - MAX_LINES
        ));
    }

    // Surface stderr (connection warnings, errors) but cap to avoid noise
    let stderr_lines: Vec<&str> = stderr
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();
    if !stderr_lines.is_empty() {
        for line in stderr_lines.iter().take(MAX_STDERR_LINES) {
            eprintln!("{}", line);
        }
        if stderr_lines.len() > MAX_STDERR_LINES {
            eprintln!(
                "... +{} stderr lines truncated",
                stderr_lines.len() - MAX_STDERR_LINES
            );
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_truncates_long_output() {
        let long_output: String = (0..100)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let filtered = filter_ssh_output(&long_output, "");
        assert!(filtered.contains("... +20 lines truncated"));
        assert_eq!(filtered.lines().count(), MAX_LINES + 1); // 80 lines + truncation msg
    }

    #[test]
    fn test_filter_short_output_unchanged() {
        let output = "hello\nworld\n";
        let filtered = filter_ssh_output(output, "");
        assert_eq!(filtered, "hello\nworld\n");
        assert!(!filtered.contains("truncated"));
    }

    #[test]
    fn test_filter_strips_blank_lines() {
        let output = "hello\n\n\nworld\n";
        let filtered = filter_ssh_output(output, "");
        assert_eq!(filtered, "hello\nworld\n");
    }

    #[test]
    fn test_filter_empty_output() {
        let filtered = filter_ssh_output("", "");
        assert_eq!(filtered, "");
    }

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_token_savings_long_output() {
        // Simulate verbose ssh output (200 lines of systemctl-style output)
        let input: String = (0..200)
            .map(|i| format!("    Active: active (running) since Mon; {}d ago", i))
            .collect::<Vec<_>>()
            .join("\n");
        let filtered = filter_ssh_output(&input, "");

        let input_tokens = count_tokens(&input);
        let output_tokens = count_tokens(&filtered);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 50.0,
            "SSH filter: expected ≥50% savings on long output, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_truncated_output_format() {
        let input: String = (0..100)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let filtered = filter_ssh_output(&input, "");
        // First line preserved
        assert!(filtered.starts_with("line 0\n"));
        // Last real line is line 79
        assert!(filtered.contains("line 79\n"));
        // Line 80 is NOT in output
        assert!(!filtered.contains("line 80\n"));
        // Truncation message at the end
        assert!(filtered.trim_end().ends_with("... +20 lines truncated"));
    }

    #[test]
    fn test_stderr_not_in_filtered_output() {
        let stdout = "connected to host\ncommand output line 1\ncommand output line 2\n";
        let stderr = "Warning: Permanently added 'host' to known hosts.\n";
        let filtered = filter_ssh_output(stdout, stderr);
        // stderr goes to eprintln, not into the returned filtered string
        assert!(!filtered.contains("Warning"));
        assert!(filtered.contains("connected to host"));
    }
}
