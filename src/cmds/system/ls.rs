//! Filters directory listings into a compact tree format.

use crate::core::tracking;
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};
use std::io::IsTerminal;

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

    // Separate flags from paths
    let show_all = args
        .iter()
        .any(|a| (a.starts_with('-') && !a.starts_with("--") && a.contains('a')) || a == "--all");

    let flags: Vec<&str> = args
        .iter()
        .filter(|a| a.starts_with('-'))
        .map(|s| s.as_str())
        .collect();
    let paths: Vec<&str> = args
        .iter()
        .filter(|a| !a.starts_with('-'))
        .map(|s| s.as_str())
        .collect();

    // Build ls -la + any extra flags the user passed (e.g. -R)
    // Strip -l, -a, -h (we handle all of these ourselves)
    let mut cmd = resolved_command("ls");
    cmd.arg("-la");
    for flag in &flags {
        if flag.starts_with("--") {
            // Long flags: skip --all (already handled)
            if *flag != "--all" {
                cmd.arg(flag);
            }
        } else {
            let stripped = flag.trim_start_matches('-');
            let extra: String = stripped
                .chars()
                .filter(|c| *c != 'l' && *c != 'a' && *c != 'h')
                .collect();
            if !extra.is_empty() {
                cmd.arg(format!("-{}", extra));
            }
        }
    }

    // Add paths (default to "." if none)
    if paths.is_empty() {
        cmd.arg(".");
    } else {
        for p in &paths {
            cmd.arg(p);
        }
    }

    let output = cmd.output().context("Failed to run ls")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprint!("{}", stderr);
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let (entries, summary) = compact_ls(&raw, show_all);

    // Only show summary in interactive mode (not when piped)
    let is_tty = std::io::stdout().is_terminal();
    let filtered = if is_tty {
        format!("{}{}", entries, summary)
    } else {
        entries
    };

    if verbose > 0 {
        eprintln!(
            "Chars: {} → {} ({}% reduction)",
            raw.len(),
            filtered.len(),
            if !raw.is_empty() {
                100 - (filtered.len() * 100 / raw.len())
            } else {
                0
            }
        );
    }

    let target_display = if paths.is_empty() {
        ".".to_string()
    } else {
        paths.join(" ")
    };
    print!("{}", filtered);
    timer.track(
        &format!("ls -la {}", target_display),
        "rtk ls",
        &raw,
        &filtered,
    );

    Ok(())
}

/// Format bytes into human-readable size
fn human_size(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1}M", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{}B", bytes)
    }
}

/// Parse ls -la output into compact format:
///   name/  (dirs)
///   name  size  (files)
/// Returns (entries, summary) so caller can suppress summary when piped.
fn compact_ls(raw: &str, show_all: bool) -> (String, String) {
    use std::collections::HashMap;

    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<(String, String)> = Vec::new(); // (name, size)
    let mut by_ext: HashMap<String, usize> = HashMap::new();

    for line in raw.lines() {
        // Skip total, empty, . and ..
        if line.starts_with("total ") || line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 9 {
            continue;
        }

        // Filename is everything from column 9 onward (handles spaces)
        let name = parts[8..].join(" ");

        // Skip . and ..
        if name == "." || name == ".." {
            continue;
        }

        // Filter noise dirs unless -a
        if !show_all && NOISE_DIRS.iter().any(|noise| name == *noise) {
            continue;
        }

        let is_dir = parts[0].starts_with('d');

        if is_dir {
            dirs.push(name);
        } else if parts[0].starts_with('-') || parts[0].starts_with('l') {
            let size: u64 = parts[4].parse().unwrap_or(0);
            let ext = if let Some(pos) = name.rfind('.') {
                name[pos..].to_string()
            } else {
                "no ext".to_string()
            };
            *by_ext.entry(ext).or_insert(0) += 1;
            files.push((name, human_size(size)));
        }
    }

    if dirs.is_empty() && files.is_empty() {
        return ("(empty)\n".to_string(), String::new());
    }

    let mut entries = String::new();

    // Dirs first, compact
    for d in &dirs {
        entries.push_str(d);
        entries.push_str("/\n");
    }

    // Files with size
    for (name, size) in &files {
        entries.push_str(name);
        entries.push_str("  ");
        entries.push_str(size);
        entries.push('\n');
    }

    // Summary line (separate so caller can suppress when piped)
    let mut summary = format!("\nSummary: {} files, {} dirs", files.len(), dirs.len());
    if !by_ext.is_empty() {
        let mut ext_counts: Vec<_> = by_ext.iter().collect();
        ext_counts.sort_by(|a, b| b.1.cmp(a.1));
        let ext_parts: Vec<String> = ext_counts
            .iter()
            .take(5)
            .map(|(ext, count)| format!("{} {}", count, ext))
            .collect();
        summary.push_str(" (");
        summary.push_str(&ext_parts.join(", "));
        if ext_counts.len() > 5 {
            summary.push_str(&format!(", +{} more", ext_counts.len() - 5));
        }
        summary.push(')');
    }
    summary.push('\n');

    (entries, summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compact_basic() {
        let input = "total 48\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 .\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 ..\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 Cargo.toml\n\
                     -rw-r--r--  1 user  staff  5678 Jan  1 12:00 README.md\n";
        let (entries, _summary) = compact_ls(input, false);
        assert!(entries.contains("src/"));
        assert!(entries.contains("Cargo.toml"));
        assert!(entries.contains("README.md"));
        assert!(entries.contains("1.2K")); // 1234 bytes
        assert!(entries.contains("5.5K")); // 5678 bytes
        assert!(!entries.contains("drwx")); // no permissions
        assert!(!entries.contains("staff")); // no group
        assert!(!entries.contains("total")); // no total
        assert!(!entries.contains("\n.\n")); // no . entry
        assert!(!entries.contains("\n..\n")); // no .. entry
    }

    #[test]
    fn test_compact_filters_noise() {
        let input = "total 8\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 node_modules\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 .git\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 target\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  100 Jan  1 12:00 main.rs\n";
        let (entries, _summary) = compact_ls(input, false);
        assert!(!entries.contains("node_modules"));
        assert!(!entries.contains(".git"));
        assert!(!entries.contains("target"));
        assert!(entries.contains("src/"));
        assert!(entries.contains("main.rs"));
    }

    #[test]
    fn test_compact_show_all() {
        let input = "total 8\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 .git\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 src\n";
        let (entries, _summary) = compact_ls(input, true);
        assert!(entries.contains(".git/"));
        assert!(entries.contains("src/"));
    }

    #[test]
    fn test_compact_empty() {
        let input = "total 0\n";
        let (entries, summary) = compact_ls(input, false);
        assert_eq!(entries, "(empty)\n");
        assert!(summary.is_empty());
    }

    #[test]
    fn test_compact_summary() {
        let input = "total 48\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 main.rs\n\
                     -rw-r--r--  1 user  staff  5678 Jan  1 12:00 lib.rs\n\
                     -rw-r--r--  1 user  staff   100 Jan  1 12:00 Cargo.toml\n";
        let (_entries, summary) = compact_ls(input, false);
        assert!(summary.contains("Summary: 3 files, 1 dirs"));
        assert!(summary.contains(".rs"));
        assert!(summary.contains(".toml"));
    }

    #[test]
    fn test_human_size() {
        assert_eq!(human_size(0), "0B");
        assert_eq!(human_size(500), "500B");
        assert_eq!(human_size(1024), "1.0K");
        assert_eq!(human_size(1234), "1.2K");
        assert_eq!(human_size(1_048_576), "1.0M");
        assert_eq!(human_size(2_500_000), "2.4M");
    }

    #[test]
    fn test_compact_handles_filenames_with_spaces() {
        let input = "total 8\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 my file.txt\n";
        let (entries, _summary) = compact_ls(input, false);
        assert!(entries.contains("my file.txt"));
    }

    #[test]
    fn test_compact_symlinks() {
        let input = "total 8\n\
                     lrwxr-xr-x  1 user  staff  10 Jan  1 12:00 link -> target\n";
        let (entries, _summary) = compact_ls(input, false);
        assert!(entries.contains("link -> target"));
    }

    #[test]
    fn test_entries_no_summary() {
        // Entries should never contain the summary line
        let input = "total 48\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 main.rs\n";
        let (entries, summary) = compact_ls(input, false);
        assert!(!entries.contains("Summary:"), "entries must not contain summary");
        assert!(summary.contains("Summary:"), "summary must contain the icon");
    }

    #[test]
    fn test_pipe_line_count() {
        // Simulates: rtk ls | wc -l
        // Entries should have exactly 1 line per file/dir, no extra blank or summary
        let input = "total 48\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 main.rs\n\
                     -rw-r--r--  1 user  staff  5678 Jan  1 12:00 lib.rs\n";
        let (entries, _summary) = compact_ls(input, false);
        let line_count = entries.lines().count();
        assert_eq!(
            line_count, 3,
            "pipe should see exactly 3 lines (1 dir + 2 files), got {}",
            line_count
        );
    }
}
