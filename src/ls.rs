//! ls command - proxy to native ls with token-optimized output
//!
//! This module proxies to the native `ls` command instead of reimplementing
//! directory traversal. This ensures full compatibility with all ls flags
//! like -l, -a, -h, -R, etc.

use crate::tracking;
use anyhow::{Context, Result};
use std::process::Command;

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let mut cmd = Command::new("ls");

    // Default to -la if no args (common case for LLM context)
    if args.is_empty() {
        cmd.args(["-la"]);
    } else {
        for arg in args {
            cmd.arg(arg);
        }
    }

    let output = cmd.output().context("Failed to run ls")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprint!("{}", stderr);
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let filtered = filter_ls_output(&raw);

    if verbose > 0 {
        eprintln!(
            "Lines: {} → {} ({}% reduction)",
            raw.lines().count(),
            filtered.lines().count(),
            if raw.lines().count() > 0 {
                100 - (filtered.lines().count() * 100 / raw.lines().count())
            } else {
                0
            }
        );
    }

    print!("{}", filtered);
    tracking::track("ls", "rtk ls", &raw, &filtered);

    Ok(())
}

fn filter_ls_output(raw: &str) -> String {
    raw.lines()
        .filter(|line| {
            // Skip "total X" line (adds no value for LLM context)
            !line.starts_with("total ")
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_removes_total_line() {
        let input = "total 48\n-rw-r--r--  1 user  staff  1234 Jan  1 12:00 file.txt\n";
        let output = filter_ls_output(input);
        assert!(!output.contains("total "));
        assert!(output.contains("file.txt"));
    }

    #[test]
    fn test_filter_preserves_files() {
        let input = "-rw-r--r--  1 user  staff  1234 Jan  1 12:00 file.txt\ndrwxr-xr-x  2 user  staff  64 Jan  1 12:00 dir\n";
        let output = filter_ls_output(input);
        assert!(output.contains("file.txt"));
        assert!(output.contains("dir"));
    }

    #[test]
    fn test_filter_handles_empty() {
        let input = "";
        let output = filter_ls_output(input);
        assert_eq!(output, "\n");
    }
}
