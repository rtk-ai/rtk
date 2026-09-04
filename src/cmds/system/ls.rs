//! Filters directory listings into a compact tree format.

use super::constants::NOISE_DIRS;
use crate::core::runner::{self, RunOptions};
use crate::core::truncate::CAP_INVENTORY;
use crate::core::utils::resolved_command;
use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

/// Matches the date+time portion in `ls -la` output, which serves as a
/// stable anchor regardless of owner/group column width.
/// E.g.: " Mar 31 16:18 " or " Dec 25  2024 "
static LS_DATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\s+(Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)\s+\d{1,2}\s+(?:\d{4}|\d{2}:\d{2})\s+"
    )
    .unwrap()
});

fn is_short_flag(arg: &str) -> bool {
    arg.starts_with('-') && !arg.starts_with("--")
}

/// `-a`/`--all` and `-A`/`--almost-all` both make ls print dotfiles;
/// in either case RTK must show everything the child listed.
fn shows_dotfiles(args: &[String]) -> bool {
    args.iter().any(|a| {
        (is_short_flag(a) && a.chars().any(|c| matches!(c, 'a' | 'A')))
            || a == "--all"
            || a == "--almost-all"
    })
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let show_all = shows_dotfiles(args);

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
    let wants_all = args
        .iter()
        .any(|a| (is_short_flag(a) && a.contains('a')) || a == "--all");
    cmd.arg(if wants_all { "-la" } else { "-l" });
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

    let label = if args.is_empty() {
        ".".to_string()
    } else {
        args.join(" ")
    };

    runner::run_filtered(
        cmd,
        "ls",
        &label,
        |raw| {
            let (entries, parsed_count, truncated, filtered) = compact_ls(raw, show_all, show_long);

            // If no lines were parsed (e.g., unrecognized locale), fall back to raw output.
            // This is safer than returning "(empty)" for a non-empty directory.
            let has_real_content = raw
                .lines()
                .any(|l| !l.starts_with("total ") && !l.is_empty() && !is_dotdir(l));
            if parsed_count == 0 && has_real_content {
                return raw.to_string();
            }

            let mut out = entries;

            if let Some(hint) = hidden_hint(&truncated, &filtered) {
                out.push_str(&hint);
                out.push('\n');
            }

            if verbose > 0 {
                eprintln!(
                    "Chars: {} → {} ({}% reduction)",
                    raw.len(),
                    out.len(),
                    if !raw.is_empty() {
                        100usize.saturating_sub(out.len() * 100 / raw.len())
                    } else {
                        0
                    }
                );
            }
            out
        },
        RunOptions::stdout_only()
            .early_exit_on_failure()
            .no_trailing_newline(),
    )
}

/// Build the recovery hint for entries dropped from the listing —
/// truncated past the display cap and/or RTK-filtered noise.
///
/// Standard RTK pattern: a truncation note plus a one-shot command to
/// retrieve the remaining. The tee file contains ONLY the dropped
/// entries (truncated first, then filtered), so `tail -n +1` (whole
/// file) retrieves nothing the agent has already seen.
fn hidden_hint(truncated: &[String], filtered: &[String]) -> Option<String> {
    if truncated.is_empty() && filtered.is_empty() {
        return None;
    }
    let note = match (truncated.len(), filtered.len()) {
        (0, f) => format!("... ({} filtered)", f),
        (t, 0) => format!("... ({} more)", t),
        (t, f) => format!("... ({} more, {} filtered)", t, f),
    };
    let mut hidden_only = String::new();
    for line in truncated.iter().chain(filtered) {
        hidden_only.push_str(line);
        hidden_only.push('\n');
    }
    match crate::core::tee::force_tee_tail_hint(&hidden_only, "ls-hidden", 1) {
        Some(tee_hint) => Some(format!("{}\n{}", note, tee_hint)),
        None => Some(note),
    }
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
/// Returns (entries, parsed_count, truncated, filtered) so caller can emit
/// a recovery hint when anything was dropped.
/// parsed_count tracks how many non-header lines were successfully parsed.
/// truncated holds compact lines beyond the CAP_INVENTORY display cap.
/// filtered holds the display name of each entry RTK removed from view
/// (noise dirs without -a/-A as `name/`, plus raw unparsable non-dotdir
/// lines).
/// If parsed_count == 0 but raw had content, caller should fall back to raw output.
fn compact_ls(
    raw: &str,
    show_all: bool,
    show_long: bool,
) -> (String, usize, Vec<String>, Vec<String>) {
    let mut dirs: Vec<(String, Option<String>)> = Vec::new(); // (name, octal_perms)
    let mut files: Vec<(String, String, Option<String>)> = Vec::new(); // (name, size, octal_perms)
    let mut lines_seen: usize = 0;
    let mut parsed_count: usize = 0;
    let mut dotdirs: usize = 0;
    let mut filtered: Vec<String> = Vec::new();

    for line in raw.lines() {
        if line.starts_with("total ") || line.is_empty() {
            continue;
        }
        lines_seen += 1;

        let Some((file_type, perms, size, name)) = parse_ls_line(line) else {
            if is_dotdir(line) {
                dotdirs += 1;
            } else {
                filtered.push(line.trim().to_string());
            }
            continue;
        };
        parsed_count += 1;

        // Filter noise dirs unless dotfiles were requested; every entry the
        // child printed and RTK drops is recorded so the hint can recover it.
        if !show_all && NOISE_DIRS.iter().any(|noise| name == *noise) {
            filtered.push(format!("{}/", name));
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
            files.push((name, human_size(size), octal));
        }
    }

    if dirs.is_empty() && files.is_empty() {
        if lines_seen > 0 && parsed_count == 0 {
            if dotdirs == lines_seen {
                // Only . and .. entries (empty directory)
                return ("(empty)\n".to_string(), 0, Vec::new(), Vec::new());
            }
            // Real content that couldn't be parsed (e.g., non-English locale)
            return (String::new(), 0, Vec::new(), Vec::new());
        }
        // Everything parsed was filtered out (e.g., only noise dirs) —
        // keep filtered so the caller can still emit a recovery hint.
        return ("(empty)\n".to_string(), parsed_count, Vec::new(), filtered);
    }

    // Dirs first, then files — one compact line each
    let mut all_lines: Vec<String> = Vec::with_capacity(dirs.len() + files.len());
    for (name, octal) in &dirs {
        all_lines.push(match octal {
            Some(octal) => format!("{}  {}/", octal, name),
            None => format!("{}/", name),
        });
    }
    for (name, size, octal) in &files {
        all_lines.push(match octal {
            Some(octal) => format!("{}  {}  {}", octal, name, size),
            None => format!("{}  {}", name, size),
        });
    }

    // Cap the displayed listing; the rest is recoverable via the tee hint.
    let truncated = if all_lines.len() > CAP_INVENTORY {
        all_lines.split_off(CAP_INVENTORY)
    } else {
        Vec::new()
    };

    let mut entries = String::new();
    for line in &all_lines {
        entries.push_str(line);
        entries.push('\n');
    }

    (entries, parsed_count, truncated, filtered)
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
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
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
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
        assert!(!entries.contains("node_modules"));
        assert!(!entries.contains(".git"));
        assert!(!entries.contains("target"));
        assert!(entries.contains("src/"));
        assert!(entries.contains("main.rs"));
    }

    #[test]
    fn test_compact_hidden_noise_dirs() {
        let input = "total 8\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 node_modules\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 .git\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 target\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  100 Jan  1 12:00 main.rs\n";
        let (_entries, _parsed, _truncated, hidden) = compact_ls(input, false, false);
        assert_eq!(
            hidden,
            vec!["node_modules/", ".git/", "target/"],
            "every noise dir the child printed is recorded"
        );

        let (_entries, _parsed, _truncated, hidden_all) = compact_ls(input, true, false);
        assert!(hidden_all.is_empty(), "-a shows noise dirs, nothing hidden");
    }

    #[test]
    fn test_compact_hidden_unparsable_line() {
        // A non-dotdir line the date regex can't parse is silently dropped —
        // it must be collected as hidden so the caller emits a recovery hint.
        let input = "total 8\n\
                     -rw-r--r--  1 user  staff  100 Jan  1 12:00 main.rs\n\
                     garbage line without date anchor\n";
        let (entries, _parsed, _truncated, hidden) = compact_ls(input, false, false);
        assert!(entries.contains("main.rs"));
        assert_eq!(hidden, vec!["garbage line without date anchor"]);
    }

    #[test]
    fn test_compact_hidden_clean_listing() {
        let input = "total 8\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  100 Jan  1 12:00 main.rs\n";
        let (_entries, _parsed, _truncated, hidden) = compact_ls(input, false, false);
        assert!(hidden.is_empty(), "nothing dropped, no hint expected");
    }

    #[test]
    fn test_compact_only_noise_dirs_keeps_hidden() {
        // Directory containing only noise dirs: output collapses to (empty),
        // but RTK-filtered entries must survive so the agent knows they exist.
        let input = "total 8\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 node_modules\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 .git\n";
        let (entries, parsed, _truncated, hidden) = compact_ls(input, false, false);
        assert_eq!(entries, "(empty)\n");
        assert_eq!(parsed, 2);
        assert_eq!(hidden, vec!["node_modules/", ".git/"]);
    }

    #[test]
    fn test_shows_dotfiles_flags() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(shows_dotfiles(&s(&["-a"])));
        assert!(shows_dotfiles(&s(&["-A"])));
        assert!(shows_dotfiles(&s(&["-lA", "."])));
        assert!(shows_dotfiles(&s(&["--all"])));
        assert!(shows_dotfiles(&s(&["--almost-all"])));
        assert!(!shows_dotfiles(&s(&["-l", "."])));
        assert!(!shows_dotfiles(&s(&["--author"])));
    }

    #[test]
    fn test_compact_records_dot_noise_dir_when_child_printed_it() {
        // Child ran with -A but RTK was told not to show all: the dot noise
        // dir must be recorded, never silently vanish.
        let input = "total 8\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 .git\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 node_modules\n\
                     -rw-r--r--  1 user  staff  100 Jan  1 12:00 README.md\n";
        let (entries, _parsed, _truncated, filtered) = compact_ls(input, false, false);
        assert!(!entries.contains(".git"));
        assert_eq!(filtered, vec![".git/", "node_modules/"]);
    }

    #[test]
    fn test_hidden_hint_none_when_empty() {
        assert!(hidden_hint(&[], &[]).is_none());
    }

    #[test]
    fn test_hidden_hint_truncation_note_and_one_shot_command() {
        let noise = vec!["node_modules/".to_string(), "target/".to_string()];
        let hint = hidden_hint(&[], &noise).expect("hint for hidden entries");
        assert!(hint.starts_with("... (2 filtered)"));
        assert!(
            !hint.contains("use -a"),
            "standard ls flags are not RTK's job to teach: {hint}"
        );
        assert!(
            !hint.contains("full output"),
            "must not point at already-seen output: {hint}"
        );
        // Tee availability depends on environment; when present the hint is
        // the standard one-shot retrieval command over the hidden-only file.
        if hint.lines().count() > 1 {
            assert!(
                hint.contains("[see remaining: tail -n +1 "),
                "tee hint must be the standard tail form: {hint}"
            );
        }
    }

    #[test]
    fn test_hidden_hint_note_variants() {
        let t = vec!["x  1B".to_string()];
        let f = vec!["target/".to_string()];
        assert!(
            hidden_hint(&t, &[])
                .expect("hint")
                .starts_with("... (1 more)")
        );
        assert!(
            hidden_hint(&[], &f)
                .expect("hint")
                .starts_with("... (1 filtered)")
        );
        assert!(
            hidden_hint(&t, &f)
                .expect("hint")
                .starts_with("... (1 more, 1 filtered)")
        );
    }

    #[test]
    fn test_compact_truncates_past_cap() {
        let mut input = String::from("total 0\n");
        for i in 0..60 {
            input.push_str(&format!(
                "-rw-r--r--  1 user  staff  100 Jan  1 12:00 file{:02}.txt\n",
                i
            ));
        }
        let (entries, _parsed, truncated, _hidden) = compact_ls(&input, false, false);
        assert_eq!(entries.lines().count(), CAP_INVENTORY);
        assert_eq!(truncated.len(), 60 - CAP_INVENTORY);
        assert!(entries.contains("file00.txt"));
        assert!(!entries.contains("file59.txt"));
        assert!(
            truncated.iter().any(|l| l.contains("file59.txt")),
            "overflow entries must be recoverable via the tee file"
        );
    }

    #[test]
    fn test_compact_no_truncation_under_cap() {
        let input = "total 8\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  100 Jan  1 12:00 main.rs\n";
        let (_entries, _parsed, truncated, _hidden) = compact_ls(input, false, false);
        assert!(truncated.is_empty());
    }

    #[test]
    fn test_compact_show_all() {
        let input = "total 8\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 .git\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 src\n";
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, true, false);
        assert!(entries.contains(".git/"));
        assert!(entries.contains("src/"));
    }

    #[test]
    fn test_compact_empty() {
        let input = "total 0\n";
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
        assert_eq!(entries, "(empty)\n");
    }

    #[test]
    fn test_compact_empty_chinese_locale() {
        let input = "total 8\n\
                     drwxr-xr-x  2 user user  4096  1月  1 12:00 .\n\
                     drwxr-xr-x 16 user user 20480  1月  1 12:00 ..\n";
        let (entries, parsed_count, _truncated, _hidden) = compact_ls(input, false, false);
        assert_eq!(parsed_count, 0);
        assert_eq!(entries, "(empty)\n");
    }

    #[test]
    fn test_compact_empty_english_locale() {
        let input = "total 0\n\
                     drwxr-xr-x  2 lumin  wheel  64 Apr 23 00:37 .\n\
                     drwxr-xr-x 16 root  wheel 164576 Apr 23 00:37 ..\n";
        let (entries, parsed_count, _truncated, _hidden) = compact_ls(input, false, false);
        assert_eq!(parsed_count, 0);
        assert_eq!(entries, "(empty)\n");
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
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
        assert!(entries.contains("my file.txt"));
    }

    #[test]
    fn test_compact_symlinks() {
        let input = "total 8\n\
                     lrwxr-xr-x  1 user  staff  10 Jan  1 12:00 link -> target\n";
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
        assert!(entries.contains("link -> target"));
    }

    #[test]
    fn test_entries_no_summary() {
        // No summary line anywhere — pure entries (agent-first output)
        let input = "total 48\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 main.rs\n";
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
        assert!(
            !entries.contains("Summary:"),
            "entries must not contain summary"
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
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
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
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
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
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
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
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
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
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
        assert!(
            entries.contains("/dev/tty"),
            "should contain device file, got: {entries}"
        );
    }

    #[test]
    fn test_compact_block_device() {
        let input = "brw-rw----  1 root  disk  8, 0 Apr 22 09:46 /dev/sda\n";
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
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
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, true);
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
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
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
        let (entries, parsed_count, _truncated, _hidden) = compact_ls(input, false, false);
        assert_eq!(parsed_count, 0);
        assert!(entries.is_empty());
    }
}
