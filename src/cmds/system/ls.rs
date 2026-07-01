//! Filters directory listings into a compact tree format.

use super::constants::NOISE_DIRS;
use crate::core::runner::{self, RunOptions};
use crate::core::truncate::{reduced, CAP_WARNINGS};
use crate::core::utils::resolved_command;
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;
use std::io::IsTerminal;

lazy_static! {
    /// Matches the date+time portion in `ls -la` output, which serves as a
    /// stable anchor regardless of owner/group column width.
    /// E.g.: " Mar 31 16:18 " or " Dec 25  2024 "
    static ref LS_DATE_RE: Regex = Regex::new(
        r"\s+(Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)\s+\d{1,2}\s+(?:\d{4}|\d{2}:\d{2})\s+"
    )
    .unwrap();
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let show_all = args
        .iter()
        .any(|a| (a.starts_with('-') && !a.starts_with("--") && a.contains('a')) || a == "--all");

    // Per `man ls`, the long listing is triggered by `-l` and also implied by
    // `-g`, `-n`, `-o`, `--full-time` or GNU `--format=long` and `--format=verbose`.
    // In any of those cases we preserve permission info as octal.
    let show_long = args.iter().any(|a| {
        if a == "--full-time" || a == "--format=long" || a == "--format=verbose" {
            return true;
        }
        if a.starts_with('-') && !a.starts_with("--") {
            return a.chars().any(|c| matches!(c, 'l' | 'g' | 'n' | 'o'));
        }
        false
    });

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
    cmd.env("LC_ALL", "C");
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
            let (entries, summary, parsed_count) = compact_ls(raw, show_all, show_long);

            // If no lines were parsed (e.g., unrecognized locale), fall back to raw output.
            // This is safer than returning "(empty)" for a non-empty directory.
            let has_real_content = raw
                .lines()
                .any(|l| !l.starts_with("total ") && !l.is_empty() && !is_dotdir(l));
            if parsed_count == 0 && has_real_content {
                return raw.to_string();
            }

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

/// Parse a single `ls -la` line, returning `(file_type_char, perms, size, name)`.
///
/// `perms` is the raw 10-char string from ls (e.g. `-rw-r--r--`); use
/// [`perms_to_octal`] to render it.
///
/// Uses the date field as a stable anchor — the date format in `ls -la` is
/// always three tokens (`Mon DD HH:MM` or `Mon DD  YYYY`), so we locate it
/// with a regex, then extract size (rightmost number before the date) and
/// filename (everything after the date). This handles owner/group names that
/// contain spaces, which break the old fixed-column approach.
fn parse_ls_line(line: &str) -> Option<(char, String, u64, String)> {
    // Skip . and .. entries before date parsing (works for non-English locales too)
    if is_dotdir(line) {
        return None;
    }

    let date_match = LS_DATE_RE.find(line)?;
    let name = line[date_match.end()..].to_string();

    let before_date = &line[..date_match.start()];
    let before_parts: Vec<&str> = before_date.split_whitespace().collect();
    if before_parts.len() < 4 {
        return None;
    }

    let perms = before_parts[0].to_string();
    let file_type = perms.chars().next()?;

    // Size is the rightmost parseable number before the date.
    // nlinks is also numeric but appears earlier; scanning from the end
    // guarantees we hit the size field first.
    let mut size: u64 = 0;
    for part in before_parts.iter().rev() {
        if let Ok(s) = part.parse::<u64>() {
            size = s;
            break;
        }
    }

    Some((file_type, perms, size, name))
}

/// Returns true if the line represents a . or .. directory entry.
///
/// POSIX.1-2017 (IEEE Std 1003.1) specifies that each directory contains
/// entries for "." (the directory itself) and ".." (its parent). These entries
/// always appear in `ls -la` output and are skipped during parsing since they
/// carry no meaningful content for token reduction.
fn is_dotdir(line: &str) -> bool {
    line.trim().ends_with('.') || line.trim().ends_with("..")
}

/// Convert an `ls`-style permission string (e.g. `-rw-r--r--`, `drwxr-xr-x`,
/// `-rwsr-xr-t`) into octal notation (e.g. `644`, `755`, `4755`).
///
/// Returns `None` if the input does not look like a permission field.
/// Special bits (setuid/setgid/sticky) are encoded as a leading 4th digit when
/// any are set; otherwise we emit a 3-digit value to stay compact.
fn perms_to_octal(perms: &str) -> Option<String> {
    if perms.len() < 10 || !perms.is_ascii() {
        return None;
    }
    let b = perms.as_bytes();

    fn perm_value(read: bool, write: bool, exec: bool) -> u32 {
        ((read as u32) << 2) | ((write as u32) << 1) | (exec as u32)
    }

    let owner_x = matches!(b[3], b'x' | b's');
    let group_x = matches!(b[6], b'x' | b's');
    let other_x = matches!(b[9], b'x' | b't');

    let owner = perm_value(b[1] == b'r', b[2] == b'w', owner_x);
    let group = perm_value(b[4] == b'r', b[5] == b'w', group_x);
    let other = perm_value(b[7] == b'r', b[8] == b'w', other_x);

    let setuid = matches!(b[3], b's' | b'S');
    let setgid = matches!(b[6], b's' | b'S');
    let sticky = matches!(b[9], b't' | b'T');
    let special = perm_value(setuid, setgid, sticky);

    if special > 0 {
        Some(format!("{}{}{}{}", special, owner, group, other))
    } else {
        Some(format!("{}{}{}", owner, group, other))
    }
}

/// Parse ls -la output into compact format.
///
/// Without `show_long`:
///   name/        (dirs)
///   name  size   (files)
///
/// With `show_long` (user passed `-l`):
///   755  name/        (dirs)
///   644  name  size   (files)
///
/// Returns (entries, summary, parsed_count) so caller can suppress summary when piped.
/// parsed_count tracks how many non-header lines were successfully parsed.
/// If parsed_count == 0 but raw had content, caller should fall back to raw output.
fn compact_ls(raw: &str, show_all: bool, show_long: bool) -> (String, String, usize) {
    use std::collections::HashMap;

    let mut dirs: Vec<(String, Option<String>)> = Vec::new(); // (name, octal_perms)
    let mut files: Vec<(String, String, Option<String>)> = Vec::new(); // (name, size, octal_perms)
    let mut by_ext: HashMap<String, usize> = HashMap::new();
    let mut lines_seen: usize = 0;
    let mut parsed_count: usize = 0;
    let mut dotdirs: usize = 0;

    for line in raw.lines() {
        if line.starts_with("total ") || line.is_empty() {
            continue;
        }
        lines_seen += 1;

        let Some((file_type, perms, size, name)) = parse_ls_line(line) else {
            if is_dotdir(line) {
                dotdirs += 1;
            }
            continue;
        };
        parsed_count += 1;

        // Filter noise dirs unless -a
        if !show_all && NOISE_DIRS.iter().any(|noise| name == *noise) {
            continue;
        }

        // Only parse perms when the user actually wants the long listing —
        // skip the work otherwise.
        let octal = if show_long {
            perms_to_octal(&perms)
        } else {
            None
        };

        if file_type == 'd' {
            dirs.push((name, octal));
        } else {
            // Regular files, symlinks, character/block devices, pipes, sockets
            let ext = if let Some(pos) = name.rfind('.') {
                name[pos..].to_string()
            } else {
                "no ext".to_string()
            };
            *by_ext.entry(ext).or_insert(0) += 1;
            files.push((name, human_size(size), octal));
        }
    }

    if dirs.is_empty() && files.is_empty() {
        if lines_seen > 0 && parsed_count == 0 {
            if dotdirs == lines_seen {
                // Only . and .. entries (empty directory)
                return ("(empty)\n".to_string(), String::new(), 0);
            }
            // Real content that couldn't be parsed (e.g., non-English locale)
            return (String::new(), String::new(), 0);
        }
        return ("(empty)\n".to_string(), String::new(), 0);
    }

    let mut entries = String::new();

    // Dirs first, compact
    for (name, octal) in &dirs {
        if let Some(octal) = octal {
            entries.push_str(octal);
            entries.push_str("  ");
        }
        entries.push_str(name);
        entries.push_str("/\n");
    }

    // Files with size
    for (name, size, octal) in &files {
        if let Some(octal) = octal {
            entries.push_str(octal);
            entries.push_str("  ");
        }
        entries.push_str(name);
        entries.push_str("  ");
        entries.push_str(size);
        entries.push('\n');
    }

    // Summary line (separate so caller can suppress when piped)
    let mut summary = format!("\nSummary: {} files, {} dirs", files.len(), dirs.len());
    if !by_ext.is_empty() {
        // inline single-line summary — fewer entries to avoid wrapping.
        const MAX_EXT_SUMMARY: usize = reduced(CAP_WARNINGS, 5);
        let mut ext_counts: Vec<_> = by_ext.iter().collect();
        ext_counts.sort_by(|a, b| b.1.cmp(a.1));
        let ext_parts: Vec<String> = ext_counts
            .iter()
            .take(MAX_EXT_SUMMARY)
            .map(|(ext, count)| format!("{} {}", count, ext))
            .collect();
        summary.push_str(" (");
        summary.push_str(&ext_parts.join(", "));
        if ext_counts.len() > MAX_EXT_SUMMARY {
            summary.push_str(&format!(", +{} more", ext_counts.len() - MAX_EXT_SUMMARY));
        }
        summary.push(')');
    }
    summary.push('\n');

    (entries, summary, parsed_count)
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
        let (entries, _summary, _parsed) = compact_ls(input, false, false);
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
        let (entries, _summary, _parsed) = compact_ls(input, false, false);
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
        let (entries, _summary, _parsed) = compact_ls(input, true, false);
        assert!(entries.contains(".git/"));
        assert!(entries.contains("src/"));
    }

    #[test]
    fn test_compact_empty() {
        let input = "total 0\n";
        let (entries, summary, _parsed) = compact_ls(input, false, false);
        assert_eq!(entries, "(empty)\n");
        assert!(summary.is_empty());
    }

    #[test]
    fn test_compact_empty_chinese_locale() {
        let input = "total 8\n\
                     drwxr-xr-x  2 user user  4096  1月  1 12:00 .\n\
                     drwxr-xr-x 16 user user 20480  1月  1 12:00 ..\n";
        let (entries, summary, parsed_count) = compact_ls(input, false, false);
        assert_eq!(parsed_count, 0);
        assert_eq!(entries, "(empty)\n");
        assert!(summary.is_empty());
    }

    #[test]
    fn test_compact_empty_english_locale() {
        let input = "total 0\n\
                     drwxr-xr-x  2 lumin  wheel  64 Apr 23 00:37 .\n\
                     drwxr-xr-x 16 root  wheel 164576 Apr 23 00:37 ..\n";
        let (entries, summary, parsed_count) = compact_ls(input, false, false);
        assert_eq!(parsed_count, 0);
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
        let (_entries, summary, _parsed) = compact_ls(input, false, false);
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
        let (entries, _summary, _parsed) = compact_ls(input, false, false);
        assert!(entries.contains("my file.txt"));
    }

    #[test]
    fn test_compact_symlinks() {
        let input = "total 8\n\
                     lrwxr-xr-x  1 user  staff  10 Jan  1 12:00 link -> target\n";
        let (entries, _summary, _parsed) = compact_ls(input, false, false);
        assert!(entries.contains("link -> target"));
    }

    #[test]
    fn test_entries_no_summary() {
        // Entries should never contain the summary line
        let input = "total 48\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 main.rs\n";
        let (entries, summary, _parsed) = compact_ls(input, false, false);
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
        let (entries, _summary, _parsed) = compact_ls(input, false, false);
        let line_count = entries.lines().count();
        assert_eq!(
            line_count, 3,
            "pipe should see exactly 3 lines (1 dir + 2 files), got {}",
            line_count
        );
    }

    // Regression test for #948: owner/group with spaces breaks fixed-column parsing
    #[test]
    fn test_compact_multiline_group() {
        let input = "total 8\n\
                     -rw-r--r--  1 fjeanne utilisa. du domaine    0 Mar 31 16:18 empty.txt\n\
                     -rw-r--r--  1 fjeanne utilisa. du domaine 1234 Mar 31 16:18 data.json\n";
        let (entries, _summary, _parsed) = compact_ls(input, false, false);
        assert!(
            entries.contains("empty.txt"),
            "should contain 'empty.txt', got: {entries}"
        );
        assert!(
            entries.contains("data.json"),
            "should contain 'data.json', got: {entries}"
        );
        assert!(
            !entries.contains("16:18"),
            "time should not leak into filename, got: {entries}"
        );
        assert!(
            entries.contains("0B"),
            "empty.txt should show 0B, got: {entries}"
        );
        assert!(
            entries.contains("1.2K"),
            "data.json should show 1.2K (1234 bytes), got: {entries}"
        );
    }

    #[test]
    fn test_compact_year_format_date() {
        // Some systems show year instead of time for old files
        let input = "total 8\n\
                     -rw-r--r--  1 user staff  5678 Dec 25  2024 archive.tar\n";
        let (entries, _summary, _parsed) = compact_ls(input, false, false);
        assert!(
            entries.contains("archive.tar"),
            "should contain filename, got: {entries}"
        );
        assert!(entries.contains("5.5K"), "should show 5.5K, got: {entries}");
    }

    #[test]
    fn test_parse_ls_line_basic() {
        let (ft, perms, size, name) =
            parse_ls_line("-rw-r--r--  1 user staff 1234 Jan  1 12:00 file.txt").unwrap();
        assert_eq!(ft, '-');
        assert_eq!(perms, "-rw-r--r--");
        assert_eq!(size, 1234);
        assert_eq!(name, "file.txt");
    }

    #[test]
    fn test_parse_ls_line_multiline_group() {
        let (ft, perms, size, name) =
            parse_ls_line("-rw-r--r--  1 fjeanne utilisa. du domaine 0 Mar 31 16:18 empty.txt")
                .unwrap();
        assert_eq!(ft, '-');
        assert_eq!(perms, "-rw-r--r--");
        assert_eq!(size, 0);
        assert_eq!(name, "empty.txt");
    }

    #[test]
    fn test_parse_ls_line_dir_with_space_in_group() {
        let (ft, perms, size, name) =
            parse_ls_line("drwxr-xr-x  2 fjeanne utilisa. du domaine 64 Mar 31 16:18 my dir")
                .unwrap();
        assert_eq!(ft, 'd');
        assert_eq!(perms, "drwxr-xr-x");
        assert_eq!(size, 64);
        assert_eq!(name, "my dir");
    }

    #[test]
    fn test_parse_ls_line_symlink() {
        let (ft, perms, size, name) =
            parse_ls_line("lrwxr-xr-x  1 user staff 10 Jan  1 12:00 link -> target").unwrap();
        assert_eq!(ft, 'l');
        assert_eq!(perms, "lrwxr-xr-x");
        assert_eq!(size, 10);
        assert_eq!(name, "link -> target");
    }

    #[test]
    fn test_compact_device_files() {
        // Regression test for #844: `rtk ls /dev/ttyACM*` returned "(empty)"
        // because character devices (type 'c') were not handled by compact_ls.
        let input = "crw-rw----  1 root  dialout  166, 0 Apr 22 09:46 /dev/ttyACM0\n";
        let (entries, _summary, _parsed) = compact_ls(input, false, false);
        assert!(
            entries.contains("/dev/ttyACM0"),
            "should contain device file, got: {entries}"
        );
        assert!(!entries.contains("(empty)"), "should not be empty");
    }

    #[test]
    fn test_compact_device_files_macos_hex_size() {
        // macOS shows device major/minor as hex (e.g. 0x2000000)
        let input = "crw-rw-rw-  1 root  wheel  0x2000000 Mar 31 19:25 /dev/tty\n";
        let (entries, _summary, _parsed) = compact_ls(input, false, false);
        assert!(
            entries.contains("/dev/tty"),
            "should contain device file, got: {entries}"
        );
    }

    #[test]
    fn test_compact_block_device() {
        let input = "brw-rw----  1 root  disk  8, 0 Apr 22 09:46 /dev/sda\n";
        let (entries, _summary, _parsed) = compact_ls(input, false, false);
        assert!(
            entries.contains("/dev/sda"),
            "should contain block device, got: {entries}"
        );
    }

    #[test]
    fn test_parse_ls_line_returns_none_for_total() {
        assert!(parse_ls_line("total 48").is_none());
    }

    #[test]
    fn test_parse_ls_line_year_format() {
        let (ft, perms, size, name) =
            parse_ls_line("-rw-r--r--  1 user staff 5678 Dec 25  2024 old.tar.gz").unwrap();
        assert_eq!(ft, '-');
        assert_eq!(perms, "-rw-r--r--");
        assert_eq!(size, 5678);
        assert_eq!(name, "old.tar.gz");
    }

    #[test]
    fn test_perms_to_octal_common() {
        assert_eq!(perms_to_octal("-rw-r--r--").as_deref(), Some("644"));
        assert_eq!(perms_to_octal("-rwxr-xr-x").as_deref(), Some("755"));
        assert_eq!(perms_to_octal("drwxr-xr-x").as_deref(), Some("755"));
        assert_eq!(perms_to_octal("-rw-------").as_deref(), Some("600"));
        assert_eq!(perms_to_octal("-rwxrwxrwx").as_deref(), Some("777"));
        assert_eq!(perms_to_octal("----------").as_deref(), Some("000"));
        assert_eq!(perms_to_octal("lrwxr-xr-x").as_deref(), Some("755"));
    }

    #[test]
    fn test_perms_to_octal_special_bits() {
        // setuid + 755 -> 4755
        assert_eq!(perms_to_octal("-rwsr-xr-x").as_deref(), Some("4755"));
        // setuid without execute -> 4644
        assert_eq!(perms_to_octal("-rwSr--r--").as_deref(), Some("4644"));
        // setgid + 755 -> 2755
        assert_eq!(perms_to_octal("-rwxr-sr-x").as_deref(), Some("2755"));
        // sticky bit on /tmp-style dir -> 1777
        assert_eq!(perms_to_octal("drwxrwxrwt").as_deref(), Some("1777"));
        // setuid + setgid + sticky
        assert_eq!(perms_to_octal("-rwsrwsrwt").as_deref(), Some("7777"));
    }

    #[test]
    fn test_perms_to_octal_garbage() {
        assert_eq!(perms_to_octal(""), None);
        assert_eq!(perms_to_octal("short"), None);
    }

    #[test]
    fn test_compact_long_format_includes_octal() {
        let input = "total 48\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 Cargo.toml\n\
                     -rwxr-xr-x  1 user  staff   500 Jan  1 12:00 build.sh\n";
        let (entries, _summary, _parsed) = compact_ls(input, false, true);
        assert!(
            entries.contains("755  src/"),
            "dir should be prefixed with octal perms, got: {entries}"
        );
        assert!(
            entries.contains("644  Cargo.toml  1.2K"),
            "file should be prefixed with octal perms, got: {entries}"
        );
        assert!(
            entries.contains("755  build.sh  500B"),
            "executable should show 755, got: {entries}"
        );
    }

    #[test]
    fn test_compact_short_format_omits_octal() {
        // Without -l, no octal prefix even though we still parse `ls -la`
        // under the hood.
        let input = "total 48\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 Cargo.toml\n";
        let (entries, _summary, _parsed) = compact_ls(input, false, false);
        assert!(
            !entries.contains("644"),
            "short format must not include octal perms, got: {entries}"
        );
        assert!(entries.contains("Cargo.toml"));
    }

    #[test]
    fn test_compact_chinese_locale_fallback() {
        let input = "total 8\n\
                      drwxr-xr-x  2 user staff  64  1月  1 12:00 src\n\
                      -rw-r--r--  1 user staff 1234  1月  1 12:00 main.rs\n";
        let (entries, summary, parsed_count) = compact_ls(input, false, false);
        assert_eq!(parsed_count, 0);
        assert!(entries.is_empty());
        assert!(summary.is_empty());
    }
}
