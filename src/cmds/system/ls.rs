//! Filters directory listings into a compact tree format.

use super::constants::NOISE_DIRS;
use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;
use std::io::IsTerminal;

lazy_static! {
    // Parse ls -la lines: extract size and filename regardless of owner/group token count.
    // Pattern: ...SIZE MONTH DAY TIME_OR_YEAR FILENAME
    // Works with any locale (month is matched as \S+) and any number of group tokens.
    static ref LS_ENTRY_RE: Regex = Regex::new(
        r"(\d+)\s+\S+\s+\d{1,2}\s+(?:\d{1,2}:\d{2}|\d{4})\s+(.+)$"
    ).unwrap();
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
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

    let mut cmd = resolved_command("ls");
    cmd.arg("-la");
    for flag in &flags {
        if flag.starts_with("--") {
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

    if paths.is_empty() {
        cmd.arg(".");
    } else {
        for p in &paths {
            cmd.arg(p);
        }
    }

    let target_display = if paths.is_empty() {
        ".".to_string()
    } else {
        paths.join(" ")
    };

    runner::run_filtered(
        cmd,
        "ls",
        &format!("-la {}", target_display),
        |raw| {
            let (entries, summary) = compact_ls(raw, show_all);

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
            filtered
        },
        RunOptions::stdout_only()
            .early_exit_on_failure()
            .no_trailing_newline(),
    )
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

        // Use regex to extract size and filename from ls -la output.
        // This handles any number of owner/group tokens and any locale. (#1084)
        // Falls back to parts[4]/parts[8] for non-standard formats (e.g. --full-time).
        let (file_size, name) = if let Some(caps) = LS_ENTRY_RE.captures(line) {
            let size: u64 = caps[1].parse().unwrap_or(0);
            let name = caps[2].to_string();
            (size, name)
        } else if parts.len() >= 9 {
            let size: u64 = parts[4].parse().unwrap_or(0);
            let name = parts[8..].join(" ");
            (size, name)
        } else {
            continue;
        };

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
            let size = file_size;
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
        assert!(
            !entries.contains("Summary:"),
            "entries must not contain summary"
        );
        assert!(
            summary.contains("Summary:"),
            "summary must contain the icon"
        );
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

    #[test]
    fn test_compact_windows_group_with_space() {
        // Windows: group name may contain a space (e.g. "Domain Users"),
        // shifting all subsequent columns by one. (#1084)
        let input = "total 8\n\
                     drwxr-xr-x  1 WBPC.VN Domain Users     0 Apr  6 15:16 docs\n\
                     -rw-r--r--  1 WBPC.VN Domain Users    59 Apr  6 15:16 projects.json\n\
                     -rw-r--r--  1 WBPC.VN Domain Users  2048 Apr  6 15:16 README.md\n";
        let (entries, _summary) = compact_ls(input, false);
        assert!(entries.contains("docs/"), "should list dirs");
        assert!(
            entries.contains("projects.json  59B"),
            "should show 59B not 0B, got: {}",
            entries
        );
        assert!(
            entries.contains("README.md  2.0K"),
            "should show 2.0K, got: {}",
            entries
        );
    }

    #[test]
    fn test_compact_windows_numeric_group() {
        // Windows Git Bash: group is numeric (e.g. 197121)
        let input = "total 136\n\
                     drwxr-xr-x 1 szk 197121     0 Apr 10 22:05 src\n\
                     -rw-r--r-- 1 szk 197121  1729 Apr 10 22:05 Cargo.toml\n\
                     -rw-r--r-- 1 szk 197121 87474 Apr 10 23:25 main.rs\n";
        let (entries, _summary) = compact_ls(input, false);
        assert!(entries.contains("Cargo.toml  1.7K"), "got: {}", entries);
        assert!(entries.contains("main.rs  85.4K"), "got: {}", entries);
    }

    #[test]
    fn test_compact_owner_named_like_month() {
        // User or group named like a month abbreviation should not
        // confuse the month-detection heuristic (Codex review P3).
        let input = "total 8\n\
                     -rw-r--r--  1 Jan  staff  1234 Feb  1 12:00 data.csv\n\
                     -rw-r--r--  1 user May    5678 Jun 15 09:00 report.md\n";
        let (entries, _summary) = compact_ls(input, false);
        assert!(
            entries.contains("data.csv  1.2K"),
            "owner 'Jan' must not confuse month detection, got: {}",
            entries
        );
        assert!(
            entries.contains("report.md  5.5K"),
            "group 'May' must not confuse month detection, got: {}",
            entries
        );
    }

    #[test]
    fn test_compact_non_english_locale_fallback() {
        // Non-English locale: month names won't match, should fall back
        // to parts[4] for size and parts[8] for name (Codex review P2).
        let input = "total 8\n\
                     -rw-r--r--  1 user  staff  1234 janv.  1 12:00 notes.txt\n";
        let (entries, _summary) = compact_ls(input, false);
        assert!(
            entries.contains("notes.txt  1.2K"),
            "non-English locale should fall back to parts[4], got: {}",
            entries
        );
    }

    #[test]
    fn test_compact_filename_containing_month_name() {
        // Filename contains a month abbreviation — must not confuse
        // the parser (Codex review round 2).
        let input = "total 8\n\
                     -rw-r--r--  1 user  staff  4096 janv.  1 12:00 meeting May notes.txt\n";
        let (entries, _summary) = compact_ls(input, false);
        assert!(
            entries.contains("meeting May notes.txt  4.0K"),
            "month in filename must not shift columns, got: {}",
            entries
        );
    }

    #[test]
    fn test_compact_windows_three_token_group() {
        // Windows: group with 3 tokens (e.g. "Remote Desktop Users")
        // Codex review round 3: must handle >1 extra token.
        let input = "total 8\n\
                     -rw-r--r--  1 user Remote Desktop Users  2048 Apr  6 15:16 data.csv\n\
                     drwxr-xr-x  1 user Remote Desktop Users     0 Apr  6 15:16 docs\n";
        let (entries, _summary) = compact_ls(input, false);
        assert!(
            entries.contains("data.csv  2.0K"),
            "3-token group must work, got: {}",
            entries
        );
        assert!(entries.contains("docs/"), "dirs should still be listed");
    }

    #[test]
    fn test_compact_non_english_locale_with_spaced_group() {
        // Non-English locale + spaced group name: worst-case combo.
        // Codex review round 3.
        let input = "total 8\n\
                     -rw-r--r--  1 user Domain Users  59 avr.  6 15:16 file.txt\n";
        let (entries, _summary) = compact_ls(input, false);
        assert!(
            entries.contains("file.txt  59B"),
            "non-English + spaced group must work, got: {}",
            entries
        );
    }

    #[test]
    fn test_compact_full_time_format() {
        // ls --full-time produces ISO timestamps — regex won't match,
        // should fall back to parts[4]/parts[8]. (Codex review round 4)
        let input = "total 8\n\
                     -rw-r--r--  1 user  staff  1234 2026-04-10 22:05:30.000000000 +0900 notes.txt\n";
        let (entries, _summary) = compact_ls(input, false);
        assert!(
            entries.contains("notes.txt  1.2K"),
            "--full-time fallback must work, got: {}",
            entries
        );
    }
}
