//! tree command - proxy to native tree with token-optimized output
//!
//! This module proxies to the native `tree` command and filters the output
//! to reduce token usage while preserving structure visibility.
//!
//! Token optimization: automatically excludes noise directories via -I pattern
//! unless -a flag is present (respecting user intent).

use super::constants::NOISE_DIRS;
#[cfg(any(target_os = "windows", test))]
use anyhow::anyhow;
use anyhow::Result;
#[cfg(not(target_os = "windows"))]
use crate::core::runner::{self, RunOptions};
#[cfg(not(target_os = "windows"))]
use crate::core::utils::{resolved_command, tool_exists};
#[cfg(any(target_os = "windows", test))]
use std::path::Path;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    #[cfg(target_os = "windows")]
    {
        run_native(args, verbose)
    }

    #[cfg(not(target_os = "windows"))]
    {
        run_external(args, verbose)
    }
}

#[cfg(not(target_os = "windows"))]
fn run_external(args: &[String], verbose: u8) -> Result<i32> {
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

#[cfg(target_os = "windows")]
fn run_native(args: &[String], verbose: u8) -> Result<i32> {
    let (path, options) = match parse_native_args(args) {
        Ok(parsed) => parsed,
        Err(err) => {
            eprintln!("rtk tree: {err}");
            return Ok(2);
        }
    };

    if verbose > 0 {
        eprintln!("Running native tree {}", args.join(" "));
    }

    match native_tree_output(Path::new(&path), &options) {
        Ok(output) => {
            print!("{output}");
            Ok(0)
        }
        Err(err) => {
            eprintln!("rtk tree: {err}");
            Ok(2)
        }
    }
}

#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Default)]
struct TreeOptions {
    show_all: bool,
    ignore: Option<String>,
    max_depth: Option<usize>,
}

#[cfg(any(target_os = "windows", test))]
fn parse_native_args(args: &[String]) -> Result<(String, TreeOptions)> {
    let mut options = TreeOptions::default();
    let mut paths = Vec::new();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "-a" | "--all" => {
                options.show_all = true;
                i += 1;
            }
            "-I" => {
                let Some(pattern) = args.get(i + 1) else {
                    return Err(anyhow!("-I requires a pattern"));
                };
                options.ignore = Some(pattern.clone());
                i += 2;
            }
            "-L" => {
                let Some(depth) = args.get(i + 1) else {
                    return Err(anyhow!("-L requires a depth"));
                };
                options.max_depth = Some(parse_depth(depth)?);
                i += 2;
            }
            _ if arg.starts_with("--ignore=") => {
                options.ignore = Some(arg["--ignore=".len()..].to_string());
                i += 1;
            }
            _ if arg.starts_with('-') => {
                return Err(anyhow!(
                    "unsupported tree flag '{arg}' on Windows native path; use rtk proxy tree ..."
                ));
            }
            _ => {
                paths.push(arg.clone());
                i += 1;
            }
        }
    }

    if paths.len() > 1 {
        return Err(anyhow!("native tree supports one path in the first version"));
    }

    Ok((paths.pop().unwrap_or_else(|| ".".to_string()), options))
}

#[cfg(any(target_os = "windows", test))]
fn parse_depth(raw: &str) -> Result<usize> {
    raw.parse::<usize>()
        .map_err(|_| anyhow!("invalid tree depth '{raw}'"))
}

#[cfg(any(target_os = "windows", test))]
fn native_tree_output(path: &Path, options: &TreeOptions) -> Result<String> {
    let mut lines = vec![path.display().to_string()];
    append_tree_children(path, "", 0, options, &mut lines)?;
    Ok(lines.join("\n") + "\n")
}

#[cfg(any(target_os = "windows", test))]
fn append_tree_children(
    path: &Path,
    prefix: &str,
    depth: usize,
    options: &TreeOptions,
    lines: &mut Vec<String>,
) -> Result<()> {
    if options.max_depth.is_some_and(|max| depth >= max) {
        return Ok(());
    }

    let mut entries = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let metadata = entry.path().symlink_metadata()?;
        if should_skip_entry(&name, is_hidden_name_or_metadata(&name, &metadata), options) {
            continue;
        }
        entries.push((entry, metadata));
    }

    entries.sort_by_key(|(entry, _)| entry.file_name().to_string_lossy().to_lowercase());
    let last_index = entries.len().saturating_sub(1);

    for (idx, (entry, metadata)) in entries.into_iter().enumerate() {
        let name = entry.file_name().to_string_lossy().to_string();
        let is_last = idx == last_index;
        let connector = if is_last { "└──" } else { "├──" };
        lines.push(format!("{prefix}{connector} {name}"));

        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            let child_prefix = format!("{prefix}{}", if is_last { "    " } else { "│   " });
            append_tree_children(&entry.path(), &child_prefix, depth + 1, options, lines)?;
        }
    }

    Ok(())
}

#[cfg(any(target_os = "windows", test))]
fn should_skip_entry(name: &str, hidden: bool, options: &TreeOptions) -> bool {
    if !options.show_all && hidden {
        return true;
    }

    if let Some(pattern) = &options.ignore {
        return matches_any_pattern(name, pattern);
    }

    if options.show_all {
        return false;
    }

    name.starts_with('.') || NOISE_DIRS.iter().any(|pattern| matches_pattern(name, pattern))
}

#[cfg(any(target_os = "windows", test))]
fn is_hidden_name_or_metadata(name: &str, metadata: &std::fs::Metadata) -> bool {
    name.starts_with('.') || metadata_is_hidden(metadata)
}

#[cfg(target_os = "windows")]
fn metadata_is_hidden(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
}

#[cfg(not(target_os = "windows"))]
fn metadata_is_hidden(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(any(target_os = "windows", test))]
fn matches_any_pattern(name: &str, pattern: &str) -> bool {
    pattern.split('|').any(|part| matches_pattern(name, part))
}

#[cfg(any(target_os = "windows", test))]
fn matches_pattern(name: &str, pattern: &str) -> bool {
    wildcard_match(&name.to_lowercase(), &pattern.to_lowercase())
}

#[cfg(any(target_os = "windows", test))]
fn wildcard_match(name: &str, pattern: &str) -> bool {
    let name = name.as_bytes();
    let pattern = pattern.as_bytes();
    let (mut ni, mut pi) = (0, 0);
    let mut star = None;
    let mut star_match = 0;

    while ni < name.len() {
        if pi < pattern.len() && (pattern[pi] == b'?' || pattern[pi] == name[ni]) {
            ni += 1;
            pi += 1;
        } else if pi < pattern.len() && pattern[pi] == b'*' {
            star = Some(pi);
            star_match = ni;
            pi += 1;
        } else if let Some(star_idx) = star {
            pi = star_idx + 1;
            star_match += 1;
            ni = star_match;
        } else {
            return false;
        }
    }

    while pi < pattern.len() && pattern[pi] == b'*' {
        pi += 1;
    }

    pi == pattern.len()
}

#[cfg_attr(target_os = "windows", allow(dead_code))]
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
    fn test_native_tree_filters_default_noise_and_egg_info() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("keep.txt"), "").unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git").join("config"), "").unwrap();
        std::fs::create_dir(dir.path().join("pkg.egg-info")).unwrap();
        std::fs::write(dir.path().join("pkg.egg-info").join("PKG-INFO"), "").unwrap();

        let output = native_tree_output(dir.path(), &TreeOptions::default()).unwrap();

        assert!(output.contains("keep.txt"));
        assert!(!output.contains(".git"));
        assert!(!output.contains("pkg.egg-info"));
    }

    #[test]
    fn test_native_tree_all_disables_default_noise_but_keeps_explicit_ignore() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git").join("config"), "").unwrap();
        std::fs::create_dir(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target").join("artifact"), "").unwrap();

        let show_all = TreeOptions {
            show_all: true,
            ignore: None,
            max_depth: None,
        };
        let all_output = native_tree_output(dir.path(), &show_all).unwrap();
        assert!(all_output.contains(".git"));
        assert!(all_output.contains("target"));

        let explicit_ignore = TreeOptions {
            show_all: true,
            ignore: Some("target".to_string()),
            max_depth: None,
        };
        let ignored_output = native_tree_output(dir.path(), &explicit_ignore).unwrap();
        assert!(ignored_output.contains(".git"));
        assert!(!ignored_output.contains("target"));
    }

    #[test]
    fn test_tree_skip_entry_hides_windows_hidden_by_default() {
        let options = TreeOptions::default();
        assert!(should_skip_entry("hidden_attr.txt", true, &options));

        let show_all = TreeOptions {
            show_all: true,
            ignore: None,
            max_depth: None,
        };
        assert!(!should_skip_entry("hidden_attr.txt", true, &show_all));
    }
}
