//! tree command - proxy to native tree with token-optimized output
//!
//! This module proxies to the native `tree` command and filters the output
//! to reduce token usage while preserving structure visibility.
//!
//! Token optimization: automatically excludes noise directories via `-I`
//! unless `-a` is present. Storage metadata remains excluded in every mode.

use super::constants::{NOISE_DIRS, SYSTEM_METADATA_PATTERNS};
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

    let show_all = args.iter().any(|a| a == "-a" || a == "--all");
    let has_ignore = args.iter().any(|a| a == "-I" || a.starts_with("--ignore="));

    if !has_ignore {
        let ignore_pattern = default_ignore_pattern(show_all);
        cmd.arg("-I").arg(&ignore_pattern);
    }

    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "-I" {
            cmd.arg(arg);
            if let Some(pattern) = args.get(index + 1) {
                cmd.arg(append_metadata_ignore(pattern));
                index += 2;
                continue;
            }
        } else if let Some(pattern) = arg.strip_prefix("--ignore=") {
            cmd.arg(format!(
                "--ignore={}",
                append_metadata_ignore(pattern)
            ));
            index += 1;
            continue;
        } else {
            cmd.arg(arg);
        }
        index += 1;
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

fn metadata_ignore_pattern() -> String {
    SYSTEM_METADATA_PATTERNS.join("|")
}

fn append_metadata_ignore(pattern: &str) -> String {
    if pattern.is_empty() {
        metadata_ignore_pattern()
    } else {
        format!("{}|{}", pattern, metadata_ignore_pattern())
    }
}

fn default_ignore_pattern(show_all: bool) -> String {
    if show_all {
        metadata_ignore_pattern()
    } else {
        NOISE_DIRS
            .iter()
            .copied()
            .chain(SYSTEM_METADATA_PATTERNS.iter().copied())
            .collect::<Vec<_>>()
            .join("|")
    }
}

fn filter_tree_output(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();

    if lines.is_empty() {
        return "\n".to_string();
    }

    let mut filtered_lines = Vec::new();

    for line in lines {
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

    #[test]
    fn test_show_all_still_excludes_storage_metadata() {
        assert_eq!(default_ignore_pattern(true), "._*|@eaDir");
    }

    #[test]
    fn test_default_ignore_includes_noise_and_storage_metadata() {
        let pattern = default_ignore_pattern(false);
        assert!(pattern.split('|').any(|part| part == "node_modules"));
        assert!(pattern.split('|').any(|part| part == "._*"));
        assert!(pattern.split('|').any(|part| part == "@eaDir"));
    }

    #[test]
    fn test_user_ignore_is_augmented_with_storage_metadata() {
        assert_eq!(
            append_metadata_ignore("vendor|tmp"),
            "vendor|tmp|._*|@eaDir"
        );
        assert_eq!(append_metadata_ignore(""), "._*|@eaDir");
    }
}
