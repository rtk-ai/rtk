use lazy_static::lazy_static;
use regex::Regex;
use super::constants::NOISE_DIRS;
use super::ls::{get_extension, LsRecord, LsRecordType, FormatOptions};

lazy_static! {
    /// Matches the date+time portion in `ls -la` output, which serves as a
    /// stable anchor regardless of owner/group column width.
    /// E.g.: " Mar 31 16:18 " or " Dec 25  2024 "
    pub static ref LS_DATE_RE: Regex = Regex::new(
        r"\s+(Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)\s+\d{1,2}\s+(?:\d{4}|\d{2}:\d{2})\s+"
    )
    .unwrap();
}

/// Helper to convert a Unix permissions string (e.g. "rwxr-xr-x") to octal.
pub fn unix_perms_to_octal(perms: &str) -> String {
    let chars: Vec<char> = perms.chars().collect();
    if chars.len() < 9 {
        return "000".to_string();
    }

    let mut modes = [0u32; 3];
    for i in 0..3 {
        let r = chars[i * 3] == 'r';
        let w = chars[i * 3 + 1] == 'w';
        let x_char = chars[i * 3 + 2];
        let x = x_char == 'x' || x_char == 's' || x_char == 't';

        let mut val = 0;
        if r { val += 4; }
        if w { val += 2; }
        if x { val += 1; }
        modes[i] = val;
    }

    // Special bits
    let mut special = 0;
    if chars[2] == 's' || chars[2] == 'S' {
        special += 4;
    }
    if chars[5] == 's' || chars[5] == 'S' {
        special += 2;
    }
    if chars[8] == 't' || chars[8] == 'T' {
        special += 1;
    }

    if special > 0 {
        format!("{}{}{}{}", special, modes[0], modes[1], modes[2])
    } else {
        format!("{}{}{}", modes[0], modes[1], modes[2])
    }
}

/// Parse a single `ls -la` line, returning `(file_type_char, size, name, perms_part)`.
///
/// Uses the date field as a stable anchor — the date format in `ls -la` is
/// always three tokens (`Mon DD HH:MM` or `Mon DD  YYYY`), so we locate it
/// with a regex, then extract size (rightmost number before the date) and
/// filename (everything after the date). This handles owner/group names that
/// contain spaces, which break the old fixed-column approach.
pub fn parse_ls_line(line: &str) -> Option<(char, u64, String, String)> {
    let date_match = LS_DATE_RE.find(line)?;
    let name = line[date_match.end()..].to_string();

    let before_date = &line[..date_match.start()];
    let before_parts: Vec<&str> = before_date.split_whitespace().collect();
    if before_parts.len() < 4 {
        return None;
    }

    let perms = before_parts[0];
    let file_type = perms.chars().next()?;
    let perms_part = if perms.len() > 1 {
        perms[1..].to_string()
    } else {
        "".to_string()
    };

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

    Some((file_type, size, name, perms_part))
}

/// Parse ls -la output into compact records.
pub fn compact_ls(raw: &str, options: &FormatOptions) -> Vec<LsRecord> {
    let mut records = Vec::new();

    for line in raw.lines() {
        if line.starts_with("total ") || line.is_empty() {
            continue;
        }

        let Some((file_type_ch, size, name, perms_part)) = parse_ls_line(line) else {
            continue;
        };

        // Skip . and ..
        if name == "." || name == ".." {
            continue;
        }

        // Filter noise dirs unless -a
        if !options.show_all && NOISE_DIRS.iter().any(|noise| name == *noise) {
            continue;
        }
        let file_type = match file_type_ch {
            'd' => LsRecordType::DIRECTORY,
            'l' => LsRecordType::SYMBOLINK,
            'f' | '-' => LsRecordType::FILE,
            _ => LsRecordType::UNKNOWN,
        };

        let octal_permissions = if options.show_long {
            Some(unix_perms_to_octal(&perms_part))
        } else {
            None
        };

        records.push(LsRecord {
            extension: get_extension(&name),
            file_type,
            size,
            name,
            timestamp: None,
            octal_permissions,
        });
    }

    records
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmds::system::ls_format::synthesize_output;

    #[test]
    fn test_compact_basic() {
        let input = "total 48\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 .\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 ..\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 Cargo.toml\n\
                     -rw-r--r--  1 user  staff  5678 Jan  1 12:00 README.md\n";
        let options = FormatOptions { show_all: false, show_long: false, sort_by_time: false, reverse: false };
        let records = compact_ls(input, &options);
        let (entries, _summary) = synthesize_output(records, &options);
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
    fn test_compact_show_long() {
        let input = "total 48\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 Cargo.toml\n";
        let options = FormatOptions { show_all: false, show_long: true, sort_by_time: false, reverse: false };
        let records = compact_ls(input, &options);
        let (entries, _summary) = synthesize_output(records, &options);
        assert!(entries.contains("755  src/"));
        assert!(entries.contains("644  Cargo.toml  1.2K"));
    }

    #[test]
    fn test_compact_filters_noise() {
        let input = "total 8\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 node_modules\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 .git\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 target\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  100 Jan  1 12:00 main.rs\n";
        let options = FormatOptions { show_all: false, show_long: false, sort_by_time: false, reverse: false };
        let records = compact_ls(input, &options);
        let (entries, _summary) = synthesize_output(records, &options);
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
        let options = FormatOptions { show_all: true, show_long: false, sort_by_time: false, reverse: false };
        let records = compact_ls(input, &options);
        let (entries, _summary) = synthesize_output(records, &options);
        assert!(entries.contains(".git/"));
        assert!(entries.contains("src/"));
    }

    #[test]
    fn test_compact_empty() {
        let input = "total 0\n";
        let options = FormatOptions { show_all: false, show_long: false, sort_by_time: false, reverse: false };
        let records = compact_ls(input, &options);
        let (entries, summary) = synthesize_output(records, &options);
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
        let options = FormatOptions { show_all: false, show_long: false, sort_by_time: false, reverse: false };
        let records = compact_ls(input, &options);
        let (_entries, summary) = synthesize_output(records, &options);
        assert!(summary.contains("Summary: 3 files, 1 dirs"));
        assert!(summary.contains(".rs"));
        assert!(summary.contains(".toml"));
    }

    #[test]
    fn test_compact_handles_filenames_with_spaces() {
        let input = "total 8\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 my file.txt\n";
        let options = FormatOptions { show_all: false, show_long: false, sort_by_time: false, reverse: false };
        let records = compact_ls(input, &options);
        let (entries, _summary) = synthesize_output(records, &options);
        assert!(entries.contains("my file.txt"));
    }

    #[test]
    fn test_compact_symlinks() {
        let input = "total 8\n\
                     lrwxr-xr-x  1 user  staff  10 Jan  1 12:00 link -> target\n";
        let options = FormatOptions { show_all: false, show_long: false, sort_by_time: false, reverse: false };
        let records = compact_ls(input, &options);
        let (entries, _summary) = synthesize_output(records, &options);
        assert!(entries.contains("link -> target"));
    }

    #[test]
    fn test_entries_no_summary() {
        let input = "total 48\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 main.rs\n";
        let options = FormatOptions { show_all: false, show_long: false, sort_by_time: false, reverse: false };
        let records = compact_ls(input, &options);
        let (entries, summary) = synthesize_output(records, &options);
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
        let input = "total 48\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 main.rs\n\
                     -rw-r--r--  1 user  staff  5678 Jan  1 12:00 lib.rs\n";
        let options = FormatOptions { show_all: false, show_long: false, sort_by_time: false, reverse: false };
        let records = compact_ls(input, &options);
        let (entries, _summary) = synthesize_output(records, &options);
        let line_count = entries.lines().count();
        assert_eq!(
            line_count, 3,
            "pipe should see exactly 3 lines (1 dir + 2 files), got {}",
            line_count
        );
    }

    #[test]
    fn test_compact_multiline_group() {
        let input = "total 8\n\
                     -rw-r--r--  1 fjeanne utilisa. du domaine    0 Mar 31 16:18 empty.txt\n\
                     -rw-r--r--  1 fjeanne utilisa. du domaine 1234 Mar 31 16:18 data.json\n";
        let options = FormatOptions { show_all: false, show_long: false, sort_by_time: false, reverse: false };
        let records = compact_ls(input, &options);
        let (entries, _summary) = synthesize_output(records, &options);
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
        let input = "total 8\n\
                     -rw-r--r--  1 user staff  5678 Dec 25  2024 archive.tar\n";
        let options = FormatOptions { show_all: false, show_long: false, sort_by_time: false, reverse: false };
        let records = compact_ls(input, &options);
        let (entries, _summary) = synthesize_output(records, &options);
        assert!(
            entries.contains("archive.tar"),
            "should contain filename, got: {entries}"
        );
        assert!(entries.contains("5.5K"), "should show 5.5K, got: {entries}");
    }

    #[test]
    fn test_parse_ls_line_basic() {
        let (ft, size, name, perms) =
            parse_ls_line("-rw-r--r--  1 user staff 1234 Jan  1 12:00 file.txt").unwrap();
        assert_eq!(ft, '-');
        assert_eq!(size, 1234);
        assert_eq!(name, "file.txt");
        assert_eq!(perms, "rw-r--r--");
    }

    #[test]
    fn test_parse_ls_line_multiline_group() {
        let (ft, size, name, perms) =
            parse_ls_line("-rw-r--r--  1 fjeanne utilisa. du domaine 0 Mar 31 16:18 empty.txt")
                .unwrap();
        assert_eq!(ft, '-');
        assert_eq!(size, 0);
        assert_eq!(name, "empty.txt");
        assert_eq!(perms, "rw-r--r--");
    }

    #[test]
    fn test_parse_ls_line_dir_with_space_in_group() {
        let (ft, size, name, perms) =
            parse_ls_line("drwxr-xr-x  2 fjeanne utilisa. du domaine 64 Mar 31 16:18 my dir")
                .unwrap();
        assert_eq!(ft, 'd');
        assert_eq!(size, 64);
        assert_eq!(name, "my dir");
        assert_eq!(perms, "rwxr-xr-x");
    }

    #[test]
    fn test_parse_ls_line_symlink() {
        let (ft, size, name, perms) =
            parse_ls_line("lrwxr-xr-x  1 user staff 10 Jan  1 12:00 link -> target").unwrap();
        assert_eq!(ft, 'l');
        assert_eq!(size, 10);
        assert_eq!(name, "link -> target");
        assert_eq!(perms, "rwxr-xr-x");
    }

    #[test]
    fn test_parse_ls_line_returns_none_for_total() {
        assert!(parse_ls_line("total 48").is_none());
    }

    #[test]
    fn test_parse_ls_line_year_format() {
        let (ft, size, name, perms) =
            parse_ls_line("-rw-r--r--  1 user staff 5678 Dec 25  2024 old.tar.gz").unwrap();
        assert_eq!(ft, '-');
        assert_eq!(size, 5678);
        assert_eq!(name, "old.tar.gz");
        assert_eq!(perms, "rw-r--r--");
    }
}
