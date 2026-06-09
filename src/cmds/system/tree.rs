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
use std::fs;
use std::path::{Path, PathBuf};

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if !tool_exists("tree") {
        return run_fallback_tree(args, verbose);
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

#[derive(Debug)]
struct FallbackTreeOptions {
    max_depth: Option<usize>,
    show_all: bool,
    ignore_patterns: Vec<String>,
    paths: Vec<PathBuf>,
}

fn parse_fallback_options(args: &[String]) -> FallbackTreeOptions {
    let mut max_depth = None;
    let mut show_all = false;
    let mut user_ignore_patterns = Vec::new();
    let mut paths = Vec::new();
    let mut has_user_ignore = false;
    let mut idx = 0;

    while idx < args.len() {
        let arg = &args[idx];
        match arg.as_str() {
            "-a" | "--all" => show_all = true,
            "-L" => {
                if let Some(value) = args.get(idx + 1) {
                    max_depth = value.parse::<usize>().ok();
                    idx += 1;
                }
            }
            "-I" => {
                if let Some(value) = args.get(idx + 1) {
                    has_user_ignore = true;
                    user_ignore_patterns.extend(split_ignore_patterns(value));
                    idx += 1;
                }
            }
            _ if arg.starts_with("-L") && arg.len() > 2 => {
                max_depth = arg[2..].parse::<usize>().ok();
            }
            _ if arg.starts_with("--ignore=") => {
                has_user_ignore = true;
                user_ignore_patterns.extend(split_ignore_patterns(&arg["--ignore=".len()..]));
            }
            _ if arg.starts_with('-') => {}
            _ => paths.push(PathBuf::from(arg)),
        }
        idx += 1;
    }

    let ignore_patterns = if show_all {
        Vec::new()
    } else if has_user_ignore {
        user_ignore_patterns
    } else {
        NOISE_DIRS.iter().map(|item| item.to_string()).collect()
    };

    FallbackTreeOptions {
        max_depth,
        show_all,
        ignore_patterns,
        paths: if paths.is_empty() {
            vec![PathBuf::from(".")]
        } else {
            paths
        },
    }
}

fn split_ignore_patterns(value: &str) -> Vec<String> {
    value
        .split('|')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

fn run_fallback_tree(args: &[String], verbose: u8) -> Result<i32> {
    let options = parse_fallback_options(args);
    let mut output = String::new();

    for (idx, path) in options.paths.iter().enumerate() {
        if idx > 0 {
            output.push('\n');
        }
        render_root(path, &options, &mut output)?;
    }

    if verbose > 0 {
        eprintln!("tree command not found; used built-in fallback renderer");
    }

    print!("{}", filter_tree_output(&output));
    Ok(0)
}

fn render_root(path: &Path, options: &FallbackTreeOptions, output: &mut String) -> Result<()> {
    output.push_str(&display_root(path));
    output.push('\n');

    if path.is_dir() && options.max_depth != Some(0) {
        render_dir(path, "", 1, options, output)?;
    }

    Ok(())
}

fn display_root(path: &Path) -> String {
    if path == Path::new(".") {
        ".".to_string()
    } else {
        path.display().to_string()
    }
}

fn render_dir(
    path: &Path,
    prefix: &str,
    depth: usize,
    options: &FallbackTreeOptions,
    output: &mut String,
) -> Result<()> {
    if options.max_depth.is_some_and(|max| depth > max) {
        return Ok(());
    }

    let mut entries = match fs::read_dir(path) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| options.show_all || !is_hidden(entry.path().as_path()))
            .filter(|entry| !is_ignored(entry.path().as_path(), &options.ignore_patterns))
            .collect::<Vec<_>>(),
        Err(_) => return Ok(()),
    };

    entries.sort_by(|a, b| {
        let a_path = a.path();
        let b_path = b.path();
        b_path
            .is_dir()
            .cmp(&a_path.is_dir())
            .then_with(|| a.file_name().cmp(&b.file_name()))
    });

    let total = entries.len();
    for (idx, entry) in entries.into_iter().enumerate() {
        let is_last = idx + 1 == total;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let connector = if is_last { "└── " } else { "├── " };

        output.push_str(prefix);
        output.push_str(connector);
        output.push_str(&name);
        output.push('\n');

        if path.is_dir() {
            let next_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });
            render_dir(&path, &next_prefix, depth + 1, options, output)?;
        }
    }

    Ok(())
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

fn is_ignored(path: &Path, patterns: &[String]) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    patterns.iter().any(|pattern| {
        if let Some(suffix) = pattern.strip_prefix('*') {
            name.ends_with(suffix)
        } else {
            name == pattern
        }
    })
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
    fn test_parse_fallback_options_common_depth() {
        let args = vec!["-L".to_string(), "2".to_string(), ".".to_string()];
        let options = parse_fallback_options(&args);

        assert_eq!(options.max_depth, Some(2));
        assert_eq!(options.paths, vec![PathBuf::from(".")]);
        assert!(options.ignore_patterns.contains(&"node_modules".to_string()));
    }

    #[test]
    fn test_parse_fallback_options_user_ignore_overrides_noise() {
        let args = vec!["-I".to_string(), "target|dist".to_string()];
        let options = parse_fallback_options(&args);

        assert_eq!(options.ignore_patterns, vec!["target", "dist"]);
    }

    #[test]
    fn test_fallback_render_skips_noise_dirs() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src/main.rs"), "").unwrap();
        fs::create_dir(temp.path().join("node_modules")).unwrap();
        fs::write(temp.path().join("node_modules/pkg.js"), "").unwrap();

        let options = parse_fallback_options(&["-L".to_string(), "2".to_string()]);
        let mut output = String::new();
        render_root(temp.path(), &options, &mut output).unwrap();

        assert!(output.contains("src"));
        assert!(output.contains("main.rs"));
        assert!(!output.contains("node_modules"));
    }
}
