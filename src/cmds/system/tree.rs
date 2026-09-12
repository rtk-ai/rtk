//! tree command - proxy to native tree with token-optimized output
//!
//! This module proxies to the native `tree` command and filters the output
//! to reduce token usage while preserving structure visibility.
//!
//! Token optimization: automatically excludes noise directories via -I pattern
//! unless -a flag is present (respecting user intent).

use super::constants::NOISE_DIRS;
use crate::core::runner::{self, RunOptions};
use crate::core::utils::{resolved_command, tool_exists};
use anyhow::Result;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if !tool_exists("tree") {
        anyhow::bail!(
            "tree command not found. Install it first:\n\
             - macOS: brew install tree\n\
             - Ubuntu/Debian: sudo apt install tree\n\
             - Fedora/RHEL: sudo dnf install tree\n\
             - Arch: sudo pacman -S tree"
        );
    }

    let mut cmd = resolved_command("tree");

    let forwarded_args = tree_args_for_platform(args);
    for arg in &forwarded_args {
        cmd.arg(arg);
    }

    runner::run_filtered(
        cmd,
        "tree",
        &args.join(" "),
        |raw| {
            let filtered = filter_tree_output(raw);
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
            filtered
        },
        RunOptions::stdout_only()
            .early_exit_on_failure()
            .no_trailing_newline(),
    )
}

fn tree_args_for_platform(args: &[String]) -> Vec<String> {
    if cfg!(windows) {
        windows_tree_args(args)
    } else {
        unix_tree_args(args)
    }
}

fn unix_tree_args(args: &[String]) -> Vec<String> {
    let mut forwarded = Vec::new();
    let show_all = args.iter().any(|a| a == "-a" || a == "--all");
    let has_ignore = args.iter().any(|a| a == "-I" || a.starts_with("--ignore="));

    if !show_all && !has_ignore {
        forwarded.push("-I".to_string());
        forwarded.push(NOISE_DIRS.join("|"));
    }

    forwarded.extend(args.iter().cloned());
    forwarded
}

fn windows_tree_args(args: &[String]) -> Vec<String> {
    let mut forwarded = Vec::new();
    let mut has_files = false;
    let mut has_ascii = false;
    let mut skip_next = false;

    // Windows tree.com only accepts slash flags; Unix -I/--all options break it.
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }

        let lower = arg.to_ascii_lowercase();
        match lower.as_str() {
            "/f" => {
                has_files = true;
                forwarded.push(arg.clone());
            }
            "/a" => {
                has_ascii = true;
                forwarded.push(arg.clone());
            }
            "-a" | "--all" => {}
            "-i" => {
                skip_next = true;
            }
            _ if lower.starts_with("--ignore=") || lower.starts_with("-i") => {}
            _ => forwarded.push(arg.clone()),
        }
    }

    if !has_files {
        forwarded.push("/F".to_string());
    }
    if !has_ascii {
        forwarded.push("/A".to_string());
    }

    forwarded
}

fn filter_tree_output(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();

    if lines.is_empty() {
        return "\n".to_string();
    }

    let mut filtered_lines = Vec::new();

    for line in lines {
        if is_windows_tree_header(line) {
            continue;
        }

        // Skip the final summary line (e.g., "5 directories, 23 files")
        if line.contains("director") && line.contains("file") {
            continue;
        }

        // Skip empty lines at the end
        if line.trim().is_empty() && filtered_lines.is_empty() {
            continue;
        }

        filtered_lines.push(line);
    }

    // Remove trailing empty lines
    while filtered_lines.last().is_some_and(|l| l.trim().is_empty()) {
        filtered_lines.pop();
    }

    filtered_lines.join("\n") + "\n"
}

fn is_windows_tree_header(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.eq_ignore_ascii_case("Folder PATH listing")
        || trimmed
            .to_ascii_lowercase()
            .starts_with("volume serial number is ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_removes_summary() {
        let input = ".\n├── src\n│   └── main.rs\n└── Cargo.toml\n\n2 directories, 3 files\n";
        let output = filter_tree_output(input);
        assert!(!output.contains("directories"));
        assert!(!output.contains("files"));
        assert!(output.contains("main.rs"));
        assert!(output.contains("Cargo.toml"));
    }

    #[test]
    fn test_filter_preserves_structure() {
        let input = ".\n├── src\n│   ├── main.rs\n│   └── lib.rs\n└── tests\n    └── test.rs\n";
        let output = filter_tree_output(input);
        assert!(output.contains("├──"));
        assert!(output.contains("│"));
        assert!(output.contains("└──"));
        assert!(output.contains("main.rs"));
        assert!(output.contains("test.rs"));
    }

    #[test]
    fn test_filter_handles_empty() {
        let input = "";
        let output = filter_tree_output(input);
        assert_eq!(output, "\n");
    }

    #[test]
    fn test_filter_removes_windows_headers() {
        let input = "Folder PATH listing\nVolume serial number is 0000014E 7957:0E10\nC:\\PROJECT\n+---src\n|       main.rs\n";
        let output = filter_tree_output(input);
        assert!(!output.contains("Folder PATH listing"));
        assert!(!output.contains("Volume serial number"));
        assert!(output.contains("C:\\PROJECT"));
        assert!(output.contains("main.rs"));
    }

    #[cfg(windows)]
    #[test]
    fn test_windows_tree_args_uses_native_flags() {
        let args = vec![".".to_string()];
        let output = tree_args_for_platform(&args);
        assert_eq!(output, vec![".", "/F", "/A"]);
    }

    #[cfg(windows)]
    #[test]
    fn test_windows_tree_args_drops_unix_ignore_flags() {
        let args = vec![
            "-I".to_string(),
            "target|node_modules".to_string(),
            "--all".to_string(),
            ".".to_string(),
        ];
        let output = tree_args_for_platform(&args);
        assert_eq!(output, vec![".", "/F", "/A"]);
    }

    #[test]
    fn test_filter_removes_trailing_empty_lines() {
        let input = ".\n├── file.txt\n\n\n";
        let output = filter_tree_output(input);
        assert_eq!(output.matches('\n').count(), 2); // Root + file.txt + final newline
    }

    #[test]
    fn test_filter_summary_variations() {
        // Test different summary formats
        let inputs = vec![
            (".\n└── file.txt\n\n0 directories, 1 file\n", "1 file"),
            (".\n└── file.txt\n\n1 directory, 0 files\n", "1 directory"),
            (".\n└── file.txt\n\n10 directories, 25 files\n", "25 files"),
        ];

        for (input, summary_fragment) in inputs {
            let output = filter_tree_output(input);
            assert!(
                !output.contains(summary_fragment),
                "Should remove summary '{}' from output",
                summary_fragment
            );
            assert!(
                output.contains("file.txt"),
                "Should preserve file.txt in output"
            );
        }
    }

    #[test]
    fn test_noise_dirs_constant() {
        // Verify NOISE_DIRS contains expected patterns
        assert!(NOISE_DIRS.contains(&"node_modules"));
        assert!(NOISE_DIRS.contains(&".git"));
        assert!(NOISE_DIRS.contains(&"target"));
        assert!(NOISE_DIRS.contains(&"__pycache__"));
        assert!(NOISE_DIRS.contains(&".next"));
        assert!(NOISE_DIRS.contains(&"dist"));
        assert!(NOISE_DIRS.contains(&"build"));
    }
}
