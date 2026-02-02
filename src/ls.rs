//! ls command - proxy to native ls with token-optimized output
//!
//! This module proxies to the native `ls` command instead of reimplementing
//! directory traversal. This ensures full compatibility with all ls flags
//! like -l, -a, -h, -R, etc.
//!
//! Token optimization: filters noise directories (node_modules, .git, target, etc.)
//! unless -a flag is present (respecting user intent).

use crate::tracking;
use anyhow::{Context, Result};
use std::process::Command;

/// Noise directories commonly excluded from LLM context
const NOISE_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "__pycache__",
    ".next",
    "dist",
    "build",
    ".cache",
    ".turbo",
    ".vercel",
    ".pytest_cache",
    ".mypy_cache",
    ".tox",
    ".venv",
    "venv",
    "env",
    ".env",
    "coverage",
    ".nyc_output",
    ".DS_Store",
    "Thumbs.db",
    ".idea",
    ".vscode",
    ".vs",
    "*.egg-info",
    ".eggs",
];

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("ls");

    // Determine if user wants all files or default behavior
    let show_all = args.iter().any(|a| a == "-a" || a == "--all");
    let has_args = !args.is_empty();

    // Default to -la if no args (upstream behavior)
    if !has_args {
        cmd.arg("-la");
    } else {
        // Pass all user args
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
    let filtered = filter_ls_output(&raw, show_all);

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
    timer.track("ls", "rtk ls", &raw, &filtered);

    Ok(())
}

fn filter_ls_output(raw: &str, show_all: bool) -> String {
    let lines: Vec<&str> = raw
        .lines()
        .filter(|line| {
            // Always skip "total X" line (adds no value for LLM context)
            if line.starts_with("total ") {
                return false;
            }

            // If -a flag present, show everything (user intent)
            if show_all {
                return true;
            }

            // Filter noise directories
            let trimmed = line.trim();
            !NOISE_DIRS.iter().any(|noise| {
                // Check if line ends with noise dir (handles various ls formats)
                trimmed.ends_with(noise) || trimmed.contains(&format!(" {}", noise))
            })
        })
        .collect();

    if lines.is_empty() {
        "\n".to_string()
    } else {
        let mut output = lines.join("\n");

        // Add summary with file type grouping
        let summary = generate_summary(&lines);
        if !summary.is_empty() {
            output.push_str("\n\n");
            output.push_str(&summary);
        }

        output.push('\n');
        output
    }
}

/// Generate summary of files by extension
fn generate_summary(lines: &[&str]) -> String {
    use std::collections::HashMap;

    let mut by_ext: HashMap<String, usize> = HashMap::new();
    let mut total_files = 0;
    let mut total_dirs = 0;

    for line in lines {
        // Parse ls -la format: permissions user group size date time filename
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        // Check if it's a directory (starts with 'd')
        if parts[0].starts_with('d') {
            total_dirs += 1;
            continue;
        }

        // Check if it's a regular file (starts with '-')
        if !parts[0].starts_with('-') {
            continue;
        }

        // Get filename (last part, handle spaces in filenames)
        if parts.len() < 9 {
            continue;
        }

        let filename = parts[8..].join(" ");

        // Extract extension
        let ext = if let Some(pos) = filename.rfind('.') {
            filename[pos..].to_string()
        } else {
            "no ext".to_string()
        };

        *by_ext.entry(ext).or_insert(0) += 1;
        total_files += 1;
    }

    if total_files == 0 && total_dirs == 0 {
        return String::new();
    }

    // Sort by count descending
    let mut ext_counts: Vec<_> = by_ext.iter().collect();
    ext_counts.sort_by(|a, b| b.1.cmp(a.1));

    // Build summary
    let mut summary = format!("📊 {} files", total_files);
    if total_dirs > 0 {
        summary.push_str(&format!(", {} dirs", total_dirs));
    }

    if !ext_counts.is_empty() {
        summary.push_str(" (");
        let ext_parts: Vec<String> = ext_counts
            .iter()
            .take(5) // Top 5 extensions
            .map(|(ext, count)| format!("{} {}", count, ext))
            .collect();
        summary.push_str(&ext_parts.join(", "));

        if ext_counts.len() > 5 {
            summary.push_str(&format!(", +{} more", ext_counts.len() - 5));
        }
        summary.push(')');
    }

    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_removes_total_line() {
        let input = "total 48\n-rw-r--r--  1 user  staff  1234 Jan  1 12:00 file.txt\n";
        let output = filter_ls_output(input, false);
        assert!(!output.contains("total "));
        assert!(output.contains("file.txt"));
    }

    #[test]
    fn test_filter_preserves_files() {
        let input = "-rw-r--r--  1 user  staff  1234 Jan  1 12:00 file.txt\ndrwxr-xr-x  2 user  staff  64 Jan  1 12:00 dir\n";
        let output = filter_ls_output(input, false);
        assert!(output.contains("file.txt"));
        assert!(output.contains("dir"));
    }

    #[test]
    fn test_filter_handles_empty() {
        let input = "";
        let output = filter_ls_output(input, false);
        assert_eq!(output, "\n");
    }

    #[test]
    fn test_summary_generation() {
        let lines = vec![
            "-rw-r--r--  1 user  staff  1234 Jan  1 12:00 file.rs",
            "-rw-r--r--  1 user  staff  1234 Jan  1 12:00 main.rs",
            "-rw-r--r--  1 user  staff  1234 Jan  1 12:00 lib.rs",
            "-rw-r--r--  1 user  staff  1234 Jan  1 12:00 Cargo.toml",
            "-rw-r--r--  1 user  staff  1234 Jan  1 12:00 README.md",
            "drwxr-xr-x  2 user  staff    64 Jan  1 12:00 src",
        ];
        let summary = generate_summary(&lines);
        assert!(summary.contains("5 files"));
        assert!(summary.contains("1 dirs"));
        assert!(summary.contains(".rs"));
    }

    #[test]
    fn test_filter_with_summary() {
        let input = "total 48\n-rw-r--r--  1 user  staff  1234 Jan  1 12:00 file.rs\n-rw-r--r--  1 user  staff  1234 Jan  1 12:00 main.rs\n";
        let output = filter_ls_output(input, false);
        assert!(!output.contains("total "));
        assert!(output.contains("file.rs"));
        assert!(output.contains("main.rs"));
        assert!(output.contains("📊"));
        assert!(output.contains("2 files"));
    }

    #[test]
    fn test_filter_removes_noise_dirs() {
        let input = "drwxr-xr-x  2 user  staff  64 Jan  1 12:00 node_modules\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 .git\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 target\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 file.txt\n";
        let output = filter_ls_output(input, false);
        assert!(!output.contains("node_modules"));
        assert!(!output.contains(".git"));
        assert!(!output.contains("target"));
        assert!(output.contains("src"));
        assert!(output.contains("file.txt"));
    }

    #[test]
    fn test_filter_shows_all_with_a_flag() {
        let input = "drwxr-xr-x  2 user  staff  64 Jan  1 12:00 node_modules\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 .git\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 src\n";
        let output = filter_ls_output(input, true);
        assert!(output.contains("node_modules"));
        assert!(output.contains(".git"));
        assert!(output.contains("src"));
    }

    #[test]
    fn test_filter_removes_pycache() {
        let input = "drwxr-xr-x  2 user  staff  64 Jan  1 12:00 __pycache__\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 main.py\n";
        let output = filter_ls_output(input, false);
        assert!(!output.contains("__pycache__"));
        assert!(output.contains("main.py"));
    }

    #[test]
    fn test_filter_removes_next_and_build_dirs() {
        let input = "drwxr-xr-x  2 user  staff  64 Jan  1 12:00 .next\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 dist\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 build\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 src\n";
        let output = filter_ls_output(input, false);
        assert!(!output.contains(".next"));
        assert!(!output.contains("dist"));
        assert!(!output.contains("build"));
        assert!(output.contains("src"));
    }
}
