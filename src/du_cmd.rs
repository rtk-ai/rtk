/// Compact filter for `du` — strips padding, sorts by size, shortens paths.
///
/// Compression examples:
/// - `du -sh dir`       → `1.2G dir`
/// - `du -sh *`         → sorted table, common prefix stripped
/// - `du -h -d 1 dir`   → sorted table with Σ total
use crate::tracking;
use anyhow::{Context, Result};
use std::process::Command;

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("du");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: du {}", args.join(" "));
    }

    let output = cmd.output().context("Failed to run du")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        // du often succeeds partially (permission denied on some dirs)
        // Only fail if there's no stdout at all
        if stdout.trim().is_empty() {
            let msg = if stderr.trim().is_empty() {
                "unknown error".to_string()
            } else {
                stderr.trim().to_string()
            };
            eprintln!("FAILED: du {}", msg);
            std::process::exit(output.status.code().unwrap_or(1));
        }
    }

    // Print stderr warnings (permission denied, etc.) but continue
    if !stderr.trim().is_empty() && verbose > 0 {
        eprintln!("{}", stderr.trim());
    }

    let raw = stdout.to_string();
    let filtered = filter_du_output(&raw);
    println!("{}", filtered);

    timer.track(
        &format!("du {}", args.join(" ")),
        &format!("rtk du {}", args.join(" ")),
        &raw,
        &filtered,
    );

    Ok(())
}

/// Parse a human-readable size string into bytes for sorting.
/// Handles: 4.0K, 316K, 1.2M, 1.8G, 500B, 1234 (raw bytes/blocks)
fn parse_size_bytes(size: &str) -> u64 {
    let size = size.trim();
    if size.is_empty() {
        return 0;
    }

    let last = size.chars().last().unwrap_or('0');
    let multiplier: u64 = match last {
        'K' => 1024,
        'M' => 1024 * 1024,
        'G' => 1024 * 1024 * 1024,
        'T' => 1024 * 1024 * 1024 * 1024,
        'P' => 1024 * 1024 * 1024 * 1024 * 1024,
        'B' => 1,
        _ => {
            // Raw number (blocks or bytes without suffix)
            return size.parse::<u64>().unwrap_or(0);
        }
    };

    let num_str = &size[..size.len() - 1];
    let num: f64 = num_str.parse().unwrap_or(0.0);
    (num * multiplier as f64) as u64
}

/// Core filter: strip padding, sort by size desc, shorten paths
pub fn filter_du_output(raw: &str) -> String {
    let lines: Vec<&str> = raw.trim().lines().collect();

    if lines.is_empty() {
        return String::new();
    }

    // Single line — just compact it
    if lines.len() == 1 {
        return format_single_line(lines[0]);
    }

    // Multiple lines — sort and compact
    format_multi_line(&lines)
}

/// Format a single du output line
fn format_single_line(line: &str) -> String {
    let parts: Vec<&str> = line.split('\t').collect();
    if parts.len() >= 2 {
        let size = parts[0].trim();
        let path = parts[1..].join("\t");
        format!("{}\t{}", size, path.trim())
    } else {
        line.trim().to_string()
    }
}

/// Parsed du entry for sorting
struct DuEntry {
    size_str: String,
    size_bytes: u64,
    path: String,
    is_total: bool,
}

/// Format multiple lines: sort by size desc, strip common prefix
fn format_multi_line(lines: &[&str]) -> String {
    let mut entries: Vec<DuEntry> = Vec::new();

    // First pass: parse all entries
    for line in lines {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 2 {
            continue;
        }

        let size_str = parts[0].trim().to_string();
        let path = parts[1..].join("\t").trim().to_string();
        let size_bytes = parse_size_bytes(&size_str);

        entries.push(DuEntry {
            size_str,
            size_bytes,
            path,
            is_total: false,
        });
    }

    // Second pass: detect total — any entry whose path is a prefix of all others
    // (e.g., "." is prefix of "./src", "/foo" is prefix of "/foo/bar")
    // Also handles "." and "./" explicitly
    let total_idx = find_total_entry(&entries);
    let mut total_entry: Option<DuEntry> = None;
    if let Some(idx) = total_idx {
        total_entry = Some(entries.remove(idx));
    }

    // Sort by size descending
    entries.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));

    // Find common directory prefix to strip
    let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
    let common_prefix = find_common_prefix(&paths);

    let mut result: Vec<String> = Vec::new();

    for entry in &entries {
        let display_path = strip_prefix(&entry.path, &common_prefix);
        result.push(format!("{}\t{}", entry.size_str, display_path));
    }

    // Add total at bottom
    if let Some(total) = &total_entry {
        result.push(format!("Σ {}", total.size_str));
    }

    result.join("\n")
}

/// Find the total/root entry: the one whose path is a parent of all others.
/// Returns the index of the total entry, or None.
fn find_total_entry(entries: &[DuEntry]) -> Option<usize> {
    if entries.len() <= 1 {
        return None;
    }

    for (i, candidate) in entries.iter().enumerate() {
        let cp = &candidate.path;
        // Explicit dot paths
        if cp == "." || cp == "./" {
            return Some(i);
        }

        // Check if this path is a parent of all other entries
        let is_parent = entries.iter().enumerate().all(|(j, other)| {
            if i == j {
                return true;
            }
            let op = &other.path;
            // "foo" is parent of "foo/bar" but not "foobar"
            op.starts_with(cp) && op.len() > cp.len() && op.as_bytes()[cp.len()] == b'/'
        });

        if is_parent {
            return Some(i);
        }
    }

    None
}

/// Find common directory prefix among paths
fn find_common_prefix(paths: &[&str]) -> String {
    if paths.len() <= 1 {
        return String::new();
    }

    let first = paths[0];
    let prefix = if let Some(pos) = first.rfind('/') {
        &first[..=pos]
    } else {
        return String::new();
    };

    if paths.iter().all(|p| p.starts_with(prefix)) {
        return prefix.to_string();
    }

    // Try shorter prefixes
    let mut candidate = prefix.to_string();
    while !candidate.is_empty() {
        if paths.iter().all(|p| p.starts_with(&candidate)) {
            return candidate;
        }
        if let Some(pos) = candidate[..candidate.len() - 1].rfind('/') {
            candidate.truncate(pos + 1);
        } else {
            return String::new();
        }
    }
    String::new()
}

/// Strip common prefix from a path
fn strip_prefix<'a>(path: &'a str, prefix: &str) -> &'a str {
    if prefix.is_empty() {
        return path;
    }
    path.strip_prefix(prefix).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_entry() {
        let raw = "3.9M\t/Users/me/projects/rtk\n";
        let result = filter_du_output(raw);
        assert_eq!(result, "3.9M\t/Users/me/projects/rtk");
    }

    #[test]
    fn test_empty_input() {
        let raw = "";
        let result = filter_du_output(raw);
        assert_eq!(result, "");
    }

    #[test]
    fn test_multi_sorted_desc() {
        let raw = "4.0K\t./docs\n1.2M\t./src\n316K\t./scripts\n3.9M\t.\n";
        let result = filter_du_output(raw);
        assert!(
            result.starts_with("1.2M\tsrc"),
            "Should sort largest first, got: {}",
            result
        );
        assert!(result.contains("316K\tscripts"));
        assert!(result.contains("4.0K\tdocs"));
        assert!(result.ends_with("Σ 3.9M"));
    }

    #[test]
    fn test_common_prefix_stripped() {
        let raw = "60K\t/home/user/project/src/main.rs\n28K\t/home/user/project/src/lib.rs\n";
        let result = filter_du_output(raw);
        assert!(
            result.contains("main.rs"),
            "Should strip common prefix, got: {}",
            result
        );
        assert!(result.contains("lib.rs"));
        assert!(!result.contains("/home/user"));
    }

    #[test]
    fn test_du_sh_wildcard() {
        let raw = "\
60K\tARCHITECTURE.md
4.0K\tCargo.toml
1.2M\tsrc
316K\tdocs
84K\tscripts
";
        let result = filter_du_output(raw);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines[0], "1.2M\tsrc", "Largest should be first");
        assert_eq!(lines[1], "316K\tdocs");
        assert_eq!(lines[2], "84K\tscripts");
        assert_eq!(lines[3], "60K\tARCHITECTURE.md");
        assert_eq!(lines[4], "4.0K\tCargo.toml");
    }

    #[test]
    fn test_total_line_dot() {
        let raw = "220K\t./a\n316K\t./b\n536K\t.\n";
        let result = filter_du_output(raw);
        assert!(result.contains("Σ 536K"));
        assert!(result.starts_with("316K"));
    }

    #[test]
    fn test_parse_size_bytes() {
        assert_eq!(parse_size_bytes("4.0K"), 4096);
        assert_eq!(parse_size_bytes("1.2M"), 1258291);
        assert_eq!(parse_size_bytes("3.9G"), 4187593113);
        assert_eq!(parse_size_bytes("500B"), 500);
        assert_eq!(parse_size_bytes("1234"), 1234);
        assert!(parse_size_bytes("1.0T") > parse_size_bytes("999G"));
    }

    #[test]
    fn test_raw_block_numbers() {
        // du without -h outputs raw block counts
        let raw = "8\t./README.md\n2048\t./src\n2056\t.\n";
        let result = filter_du_output(raw);
        assert!(result.starts_with("2048\tsrc"));
        assert!(result.contains("8\tREADME.md"));
        assert!(result.ends_with("Σ 2056"));
    }

    #[test]
    fn test_total_detected_from_parent_path() {
        // du -h -d 1 /Users/me/projects/rtk produces the root as last line
        let raw = "\
1.2G\t/Users/me/rtk/target
1.9M\t/Users/me/rtk/.git
1.2M\t/Users/me/rtk/src
1.2G\t/Users/me/rtk
";
        let result = filter_du_output(raw);
        assert!(
            result.contains("Σ 1.2G"),
            "Total should have Σ marker, got: {}",
            result
        );
        assert!(
            !result.contains("\t/Users/me/rtk\n"),
            "Root path should not appear as regular entry"
        );
        // Entries should be sorted, prefix stripped
        let lines: Vec<&str> = result.lines().collect();
        assert!(
            lines[0].contains("target"),
            "Largest should be first, got: {}",
            lines[0]
        );
        assert!(
            lines.last().unwrap().starts_with("Σ"),
            "Total should be last line"
        );
    }

    #[test]
    fn test_token_savings() {
        let raw = "\
220K\t/home/user/myproject/.claude\n\
316K\t/home/user/myproject/docs\n\
20K\t/home/user/myproject/hooks\n\
4.0K\t/home/user/myproject/Formula\n\
84K\t/home/user/myproject/scripts\n\
36K\t/home/user/myproject/.github\n\
1.8M\t/home/user/myproject/.git\n\
1.2M\t/home/user/myproject/src\n\
3.9M\t/home/user/myproject\n";
        let filtered = filter_du_output(raw);
        let input_chars = raw.len() as f64;
        let output_chars = filtered.len() as f64;
        let savings = 100.0 - (output_chars / input_chars * 100.0);
        assert!(
            savings >= 60.0,
            "Expected >=60% savings, got {:.1}%",
            savings
        );
    }
}
