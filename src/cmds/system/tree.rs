//! tree command - proxy to native tree with token-optimized output
//!
//! This module proxies to the native `tree` command and filters the output
//! to reduce token usage while preserving structure visibility.
//!
//! Token optimization: automatically excludes noise directories via -I pattern
//! unless -a flag is present (respecting user intent).

use super::constants::NOISE_DIRS;
use crate::core::runner::{self, RunOptions};
use crate::core::tracking::TimedExecution;
use crate::core::utils::{resolved_command, tool_exists};
use anyhow::Result;
use std::path::Path;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    // Windows ships `tree.com`, which is NOT the Unix `tree` and rejects the
    // flags rtk uses (e.g. `-I <pattern>` → "Too many parameters"). Use a
    // native Rust walk on Windows or whenever the Unix binary is unavailable.
    if cfg!(windows) || !tool_exists("tree") {
        return run_native(args, verbose);
    }

    let mut cmd = resolved_command("tree");

    let show_all = args.iter().any(|a| a == "-a" || a == "--all");
    let has_ignore = args.iter().any(|a| a == "-I" || a.starts_with("--ignore="));

    if !show_all && !has_ignore {
        let ignore_pattern = NOISE_DIRS.join("|");
        cmd.arg("-I").arg(&ignore_pattern);
    }

    for arg in args {
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

/// Options parsed from native `tree` args.
struct TreeOpts {
    show_all: bool,
    dirs_only: bool,
    max_depth: Option<usize>,
    root: String,
}

fn parse_tree_args(args: &[String]) -> TreeOpts {
    let mut show_all = false;
    let mut dirs_only = false;
    let mut max_depth = None;
    let mut positionals: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "-a" || a == "--all" {
            show_all = true;
        } else if a == "-d" {
            dirs_only = true;
        } else if a == "-L" {
            if let Some(n) = args.get(i + 1).and_then(|v| v.parse::<usize>().ok()) {
                max_depth = Some(n);
                i += 1;
            }
        } else if let Some(rest) = a.strip_prefix("-L") {
            if let Ok(n) = rest.parse::<usize>() {
                max_depth = Some(n);
            }
        } else if a.starts_with('-') {
            // Ignore other flags (e.g. -I value pairs) for the native path.
            if a == "-I" {
                i += 1; // skip its pattern argument
            }
        } else {
            positionals.push(a.clone());
        }
        i += 1;
    }

    TreeOpts {
        show_all,
        dirs_only,
        max_depth,
        root: positionals.first().cloned().unwrap_or_else(|| ".".to_string()),
    }
}

/// Native `tree` implementation (cross-platform) used on Windows or when the
/// Unix `tree` binary is unavailable. Prunes [`NOISE_DIRS`] unless `-a`.
fn run_native(args: &[String], verbose: u8) -> Result<i32> {
    let timer = TimedExecution::start();
    let opts = parse_tree_args(args);

    let mut out = String::new();
    out.push_str(&opts.root);
    out.push('\n');

    let mut dirs = 0usize;
    let mut files = 0usize;
    render_dir(
        Path::new(&opts.root),
        "",
        &opts,
        1,
        &mut out,
        &mut dirs,
        &mut files,
    );

    out.push('\n');
    out.push_str(&format!("{dirs} directories, {files} files\n"));

    let filtered = filter_tree_output(&out);

    if verbose > 0 {
        eprintln!("tree (native): {dirs} dirs, {files} files");
    }

    print!("{filtered}");
    timer.track(
        &format!("tree {}", args.join(" ")),
        "rtk tree",
        &out,
        &filtered,
    );
    Ok(0)
}

#[allow(clippy::too_many_arguments)]
fn render_dir(
    dir: &Path,
    prefix: &str,
    opts: &TreeOpts,
    depth: usize,
    out: &mut String,
    dirs: &mut usize,
    files: &mut usize,
) {
    if let Some(max) = opts.max_depth {
        if depth > max {
            return;
        }
    }

    let mut entries: Vec<(String, bool)> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if !opts.show_all && name.starts_with('.') {
                    return None;
                }
                if !opts.show_all && is_dir && NOISE_DIRS.contains(&name.as_str()) {
                    return None;
                }
                if opts.dirs_only && !is_dir {
                    return None;
                }
                Some((name, is_dir))
            })
            .collect(),
        Err(_) => return,
    };

    entries.sort();
    let last_idx = entries.len().saturating_sub(1);

    for (idx, (name, is_dir)) in entries.iter().enumerate() {
        let is_last = idx == last_idx;
        let connector = if is_last { "└── " } else { "├── " };
        out.push_str(prefix);
        out.push_str(connector);
        out.push_str(name);
        out.push('\n');

        if *is_dir {
            *dirs += 1;
            let child_prefix = format!("{prefix}{}", if is_last { "    " } else { "│   " });
            render_dir(
                &dir.join(name),
                &child_prefix,
                opts,
                depth + 1,
                out,
                dirs,
                files,
            );
        } else {
            *files += 1;
        }
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
    fn test_parse_tree_args_defaults() {
        let opts = parse_tree_args(&[]);
        assert!(!opts.show_all);
        assert!(!opts.dirs_only);
        assert_eq!(opts.max_depth, None);
        assert_eq!(opts.root, ".");
    }

    #[test]
    fn test_parse_tree_args_flags_and_path() {
        let args: Vec<String> = vec!["-a".into(), "-L".into(), "2".into(), "src".into()];
        let opts = parse_tree_args(&args);
        assert!(opts.show_all);
        assert_eq!(opts.max_depth, Some(2));
        assert_eq!(opts.root, "src");
    }

    #[test]
    fn test_parse_tree_args_dirs_only_and_inline_depth() {
        let args: Vec<String> = vec!["-d".into(), "-L3".into()];
        let opts = parse_tree_args(&args);
        assert!(opts.dirs_only);
        assert_eq!(opts.max_depth, Some(3));
    }

    #[test]
    fn test_parse_tree_args_skips_ignore_pattern() {
        let args: Vec<String> = vec!["-I".into(), "node_modules".into(), "mydir".into()];
        let opts = parse_tree_args(&args);
        assert_eq!(opts.root, "mydir");
    }
}
