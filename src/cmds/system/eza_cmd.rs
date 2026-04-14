//! eza filter - proxy to eza with token-optimized output
//!
//! Handles flat, long (-l), and tree (-T) modes.
//! Strips icons, noise dirs, permissions, timestamps.
//! Token savings: ~65% flat, ~75% long, ~70% tree.

use super::constants::NOISE_DIRS;
use crate::core::runner::{self, RunOptions};
use crate::core::utils::{resolved_command, tool_exists};
use anyhow::Result;

const MAX_FLAT_ENTRIES: usize = 30;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if !tool_exists("eza") {
        anyhow::bail!(
            "eza not found. Install:\n\
             - cargo: cargo install eza\n\
             - macOS: brew install eza\n\
             - Ubuntu/Debian: apt install eza\n\
             - Arch: pacman -S eza"
        );
    }

    let show_all = args.iter().any(|a| {
        a == "--all"
            || a == "--almost-all"
            || (a.starts_with('-') && !a.starts_with("--") && (a.contains('a') || a.contains('A')))
    });

    let is_long = args
        .iter()
        .any(|a| a == "--long" || (a.starts_with('-') && !a.starts_with("--") && a.contains('l')));

    let is_tree = args
        .iter()
        .any(|a| a == "--tree" || (a.starts_with('-') && !a.starts_with("--") && a.contains('T')));

    let has_ignore = args
        .iter()
        .any(|a| a == "--ignore-glob" || a.starts_with("--ignore-glob="));

    let mut cmd = resolved_command("eza");

    if is_tree && !show_all && !has_ignore {
        let ignore_pattern = NOISE_DIRS.join("|");
        cmd.arg("--ignore-glob").arg(&ignore_pattern);
    }

    for arg in args {
        cmd.arg(arg);
    }

    runner::run_filtered(
        cmd,
        "eza",
        &args.join(" "),
        |raw| {
            let filtered = if is_tree {
                filter_tree(raw)
            } else if is_long {
                filter_long(raw, show_all)
            } else {
                filter_flat(raw, show_all)
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

fn strip_icon(s: &str) -> &str {
    let s = s.trim_start();
    let first_ascii = s.find(|c: char| c.is_ascii()).unwrap_or(s.len());
    if first_ascii == 0 {
        return s;
    }
    s[first_ascii..].trim_start_matches(' ')
}

fn is_size_like(s: &str) -> bool {
    if s == "-" {
        return true;
    }
    let trimmed = s.trim_end_matches(['k', 'M', 'G', 'T', 'B', 'i']);
    !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit() || c == '.')
}

fn filter_flat(raw: &str, show_all: bool) -> String {
    let mut entries: Vec<String> = Vec::new();

    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let name = strip_icon(line.trim());
        if name.is_empty() {
            continue;
        }
        if name == "." || name == ".." {
            continue;
        }
        if !show_all && NOISE_DIRS.iter().any(|n| name == *n) {
            continue;
        }
        entries.push(name.to_string());
    }

    if entries.is_empty() {
        return "(empty)\n".to_string();
    }

    let total = entries.len();
    let mut out = String::new();
    for e in entries.iter().take(MAX_FLAT_ENTRIES) {
        out.push_str(e);
        out.push('\n');
    }
    if total > MAX_FLAT_ENTRIES {
        out.push_str(&format!(
            "... ({} more entries)\n",
            total - MAX_FLAT_ENTRIES
        ));
    }
    out
}

fn filter_long(raw: &str, show_all: bool) -> String {
    let mut out = String::new();

    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // skip header line
        if line.trim_start().starts_with("Permissions") {
            continue;
        }
        if let Some(entry) = parse_long_line(line, show_all) {
            out.push_str(&entry);
            out.push('\n');
        }
    }

    if out.is_empty() {
        "(empty)\n".to_string()
    } else {
        out
    }
}

fn parse_long_line(line: &str, show_all: bool) -> Option<String> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 7 {
        return None;
    }

    let perms = parts[0];
    if perms.len() != 10 {
        return None;
    }
    let first_char = perms.chars().next()?;
    if !matches!(first_char, 'd' | '.' | '-' | 'l' | 'c' | 'b' | 'p' | 's') {
        return None;
    }

    let is_dir = first_char == 'd';

    // size at idx 1 unless git column present at idx 1
    let size_idx = if is_size_like(parts[1]) { 1 } else { 2 };

    // size + user + day + month + time = 5 fields before name
    let name_start_idx = size_idx + 5;
    if name_start_idx >= parts.len() {
        return None;
    }

    let raw_name = parts[name_start_idx..].join(" ");
    let name = strip_icon(&raw_name);
    let name = name.to_string();

    if name == "." || name == ".." {
        return None;
    }
    if !show_all && NOISE_DIRS.iter().any(|n| name == *n) {
        return None;
    }

    if is_dir {
        Some(format!("{}/", name))
    } else {
        Some(format!("{}  {}", name, parts[size_idx]))
    }
}

fn filter_tree(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();

    if lines.is_empty() {
        return "\n".to_string();
    }

    let mut out: Vec<&str> = lines
        .iter()
        .filter(|l| !(l.contains("director") && l.contains("file")))
        .copied()
        .collect();

    while out.last().is_some_and(|l: &&str| l.trim().is_empty()) {
        out.pop();
    }

    out.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- strip_icon ---

    #[test]
    fn test_strip_icon_no_icon() {
        assert_eq!(strip_icon("Cargo.toml"), "Cargo.toml");
    }

    #[test]
    fn test_strip_icon_with_icon() {
        // U+E5FB (nerd font) + space + filename
        let icon = "\u{e5fb} Cargo.toml";
        assert_eq!(strip_icon(icon), "Cargo.toml");
    }

    #[test]
    fn test_strip_icon_leading_space() {
        assert_eq!(strip_icon("  main.rs"), "main.rs");
    }

    // --- is_size_like ---

    #[test]
    fn test_is_size_like_dash() {
        assert!(is_size_like("-"));
    }

    #[test]
    fn test_is_size_like_human() {
        assert!(is_size_like("4.0k"));
        assert!(is_size_like("1.2M"));
        assert!(is_size_like("100"));
        assert!(is_size_like("3.5G"));
    }

    #[test]
    fn test_is_size_like_git_status_not_size() {
        assert!(!is_size_like("--"));
        assert!(!is_size_like("M-"));
        assert!(!is_size_like("N-"));
    }

    // --- filter_flat ---

    #[test]
    fn test_flat_basic() {
        let input = "Cargo.toml\nmain.rs\nREADME.md\n";
        let out = filter_flat(input, false);
        assert!(out.contains("Cargo.toml"));
        assert!(out.contains("main.rs"));
        assert!(out.contains("README.md"));
    }

    #[test]
    fn test_flat_filters_noise() {
        let input = "src\nnode_modules\n.git\ntarget\nmain.rs\n";
        let out = filter_flat(input, false);
        assert!(!out.contains("node_modules"));
        assert!(!out.contains(".git"));
        assert!(!out.contains("target"));
        assert!(out.contains("src"));
        assert!(out.contains("main.rs"));
    }

    #[test]
    fn test_flat_show_all_includes_noise() {
        let input = "src\nnode_modules\n.git\n";
        let out = filter_flat(input, true);
        assert!(out.contains("node_modules"));
        assert!(out.contains(".git"));
    }

    #[test]
    fn test_flat_truncates_at_30() {
        let input: String = (0..40).map(|i| format!("file{}.rs\n", i)).collect();
        let out = filter_flat(&input, false);
        let lines: Vec<&str> = out.lines().collect();
        // 30 entries + 1 truncation line
        assert_eq!(lines.len(), 31);
        assert!(out.contains("... (10 more entries)"));
    }

    #[test]
    fn test_flat_empty() {
        let input = "\n\n";
        let out = filter_flat(input, false);
        assert_eq!(out, "(empty)\n");
    }

    #[test]
    fn test_flat_strips_icon() {
        let input = "\u{e5fb} Cargo.toml\n\u{f115} src\n";
        let out = filter_flat(input, false);
        assert!(out.contains("Cargo.toml"));
        assert!(out.contains("src"));
        assert!(!out.contains('\u{e5fb}'));
    }

    // --- filter_long ---

    #[test]
    fn test_long_basic_file() {
        // eza long: .rw-r--r-- size user day mon time name
        let input = ".rw-r--r--  4.0k user  8 Apr 12:34  Cargo.toml\n";
        let out = filter_long(input, false);
        assert!(out.contains("Cargo.toml"));
        assert!(out.contains("4.0k"));
        assert!(!out.contains("user"));
        assert!(!out.contains("Apr"));
    }

    #[test]
    fn test_long_dir() {
        let input = "drwxr-xr-x     - user  8 Apr 12:34  src\n";
        let out = filter_long(input, false);
        assert!(out.contains("src/"));
        assert!(!out.contains("user"));
    }

    #[test]
    fn test_long_filters_noise() {
        let input = "drwxr-xr-x  - user  8 Apr 12:34  node_modules\n\
                     drwxr-xr-x  - user  8 Apr 12:34  src\n\
                     .rw-r--r--  1.0k user  8 Apr 12:34  main.rs\n";
        let out = filter_long(input, false);
        assert!(!out.contains("node_modules"));
        assert!(out.contains("src/"));
        assert!(out.contains("main.rs"));
    }

    #[test]
    fn test_long_with_git_column() {
        // with --git: .rw-r--r-- M- 4.0k user day mon time name
        let input = ".rw-r--r-- M- 4.0k user  8 Apr 12:34  changed.rs\n";
        let out = filter_long(input, false);
        assert!(out.contains("changed.rs"));
        assert!(out.contains("4.0k"));
    }

    #[test]
    fn test_long_skips_header() {
        let input = "Permissions Size User Date Modified Name\n\
                     .rw-r--r--  1.0k user  8 Apr 12:34  file.rs\n";
        let out = filter_long(input, false);
        assert!(!out.contains("Permissions"));
        assert!(out.contains("file.rs"));
    }

    #[test]
    fn test_long_empty() {
        let out = filter_long("", false);
        assert_eq!(out, "(empty)\n");
    }

    // --- filter_tree ---

    #[test]
    fn test_tree_removes_summary() {
        let input = ".\n├── src\n│   └── main.rs\n└── Cargo.toml\n\n2 directories, 3 files\n";
        let out = filter_tree(input);
        assert!(!out.contains("directories"));
        assert!(!out.contains("files"));
        assert!(out.contains("main.rs"));
    }

    #[test]
    fn test_tree_preserves_structure() {
        let input = ".\n├── src\n│   └── main.rs\n└── Cargo.toml\n";
        let out = filter_tree(input);
        assert!(out.contains("├──"));
        assert!(out.contains("└──"));
        assert!(out.contains("src"));
    }

    #[test]
    fn test_tree_empty() {
        let out = filter_tree("");
        assert_eq!(out, "\n");
    }
}
