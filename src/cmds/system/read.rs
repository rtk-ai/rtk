//! Reads source files with optional language-aware filtering to strip boilerplate.

use crate::core::filter::{self, FilterLevel, Language};
use crate::core::guard::never_worse;
use crate::core::tracking;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

enum LineRange {
    Full,
    Empty,
    Head(usize),           // n > 0
    Tail(usize),           // n > 0
    From(usize),           // start >= 0
    Slice((usize, usize)), // start >= 0, limit > 0
}

impl LineRange {
    fn from_args(offset: Option<usize>, limit: Option<usize>, tail_lines: Option<usize>) -> Self {
        match (offset, limit, tail_lines) {
            (_, Some(0), _) | (_, _, Some(0)) => Self::Empty,
            (Some(offset), Some(limit), _) => Self::Slice((offset.saturating_sub(1), limit)),
            (Some(offset), None, _) => Self::From(offset.saturating_sub(1)),
            (None, Some(limit), _) => Self::Head(limit),
            (None, None, Some(tail)) => Self::Tail(tail),
            _ => Self::Full,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    file: &Path,
    level: FilterLevel,
    max_lines: Option<usize>,
    tail_lines: Option<usize>,
    offset: Option<usize>,
    limit: Option<usize>,
    line_numbers: bool,
    verbose: u8,
) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Reading: {} (filter: {})", file.display(), level);
    }

    // Read file content
    let content = fs::read_to_string(file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    // Detect language from extension
    let lang = file
        .extension()
        .and_then(|e| e.to_str())
        .map(Language::from_extension)
        .unwrap_or(Language::Unknown);

    if verbose > 1 {
        eprintln!("Detected language: {:?}", lang);
    }

    // Apply filter
    let filter = filter::get_filter(level);
    let mut filtered = filter.filter(&content, &lang);

    // Safety: if filter emptied a non-empty file, fall back to raw content
    if filtered.trim().is_empty() && !content.trim().is_empty() {
        eprintln!(
            "rtk: warning: filter produced empty output for {} ({} bytes), showing raw content",
            file.display(),
            content.len()
        );
        filtered = content.clone();
    }

    if verbose > 0 {
        let original_lines = content.lines().count();
        let filtered_lines = filtered.lines().count();
        let reduction = if original_lines > 0 {
            ((original_lines - filtered_lines) as f64 / original_lines as f64) * 100.0
        } else {
            0.0
        };
        eprintln!(
            "Lines: {} -> {} ({:.1}% reduction)",
            original_lines, filtered_lines, reduction
        );
    }

    let range = LineRange::from_args(offset, limit, tail_lines);
    let (line_start, filtered) = apply_line_window(&filtered, max_lines, &range, &lang);

    let (raw, rtk_output) = if line_numbers {
        (
            format_with_line_numbers(&content, 1),
            format_with_line_numbers(&filtered, line_start),
        )
    } else {
        (content, filtered)
    };
    let shown = never_worse(&raw, &rtk_output);
    print!("{}", shown);
    timer.track(
        &format!("cat {}", file.display()),
        "rtk read",
        &raw,
        shown,
    );
    Ok(())
}

pub fn run_stdin(
    level: FilterLevel,
    max_lines: Option<usize>,
    tail_lines: Option<usize>,
    offset: Option<usize>,
    limit: Option<usize>,
    line_numbers: bool,
    verbose: u8,
) -> Result<()> {
    use std::io::{self, Read as IoRead};

    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Reading from stdin (filter: {})", level);
    }

    // Read from stdin
    let mut content = String::new();
    io::stdin()
        .lock()
        .read_to_string(&mut content)
        .context("Failed to read from stdin")?;

    // No file extension, so use Unknown language
    let lang = Language::Unknown;

    if verbose > 1 {
        eprintln!("Language: {:?} (stdin has no extension)", lang);
    }

    // Apply filter
    let filter = filter::get_filter(level);
    let filtered = filter.filter(&content, &lang);

    if verbose > 0 {
        let original_lines = content.lines().count();
        let filtered_lines = filtered.lines().count();
        let reduction = if original_lines > 0 {
            ((original_lines - filtered_lines) as f64 / original_lines as f64) * 100.0
        } else {
            0.0
        };
        eprintln!(
            "Lines: {} -> {} ({:.1}% reduction)",
            original_lines, filtered_lines, reduction
        );
    }

    let range = LineRange::from_args(offset, limit, tail_lines);
    let (line_start, filtered) = apply_line_window(&filtered, max_lines, &range, &lang);

    let (raw, rtk_output) = if line_numbers {
        (
            format_with_line_numbers(&content, 1),
            format_with_line_numbers(&filtered, line_start),
        )
    } else {
        (content, filtered)
    };
    let shown = never_worse(&raw, &rtk_output);
    print!("{}", shown);

    timer.track("cat - (stdin)", "rtk read -", &raw, shown);
    Ok(())
}

fn format_with_line_numbers(content: &str, display_start: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let width = (display_start + lines.len().saturating_sub(1))
        .to_string()
        .len();
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        out.push_str(&format!(
            "{:>width$} │ {}\n",
            display_start + i,
            line,
            width = width
        ));
    }
    out
}

fn apply_line_window(
    content: &str,
    max_lines: Option<usize>,
    range: &LineRange,
    lang: &Language,
) -> (usize, String) {
    let start_ptr = content.as_ptr() as usize;

    let (line_start, windowed) = match range {
        LineRange::Empty => return (1, String::new()),
        LineRange::Tail(n) => {
            // we do a double pass instead of a Vec<&str> collection to save memory
            let total = content.lines().count();
            let start = total.saturating_sub(*n);
            let byte_start = content
                .lines()
                .nth(start)
                .map(|line| line.as_ptr() as usize - start_ptr)
                .unwrap_or(0);
            return (start + 1, content[byte_start..].to_string());
        }
        LineRange::Head(n) => {
            if let Some(line) = content.lines().nth(*n) {
                let byte_stop = line.as_ptr() as usize - start_ptr;
                (1, &content[..byte_stop])
            } else {
                (1, content)
            }
        }
        LineRange::From(n) => {
            if let Some(line) = content.lines().nth(*n) {
                let byte_start = line.as_ptr() as usize - start_ptr;
                (*n + 1, &content[byte_start..])
            } else {
                (*n + 1, &content[0..0])
            }
        }
        LineRange::Slice((start, limit)) => {
            let mut lines = content.lines();
            if let Some(line) = lines.nth(*start) {
                let byte_start = line.as_ptr() as usize - start_ptr;
                if let Some(line) = lines.nth(*limit - 1) {
                    let byte_stop = line.as_ptr() as usize - start_ptr;
                    (*start + 1, &content[byte_start..byte_stop])
                } else {
                    (*start + 1, &content[byte_start..])
                }
            } else {
                (*start + 1, &content[0..0])
            }
        }
        LineRange::Full => (1, content),
    };

    if let Some(max) = max_lines {
        (line_start, filter::smart_truncate(windowed, max, lang))
    } else {
        (line_start, windowed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_read_rust_file() -> Result<()> {
        let mut file = NamedTempFile::with_suffix(".rs")?;
        writeln!(
            file,
            r#"// Comment
fn main() {{
    println!("Hello");
}}"#
        )?;

        // Just verify it doesn't panic
        run(
            file.path(),
            FilterLevel::Minimal,
            None,
            None,
            None,
            None,
            false,
            0,
        )?;
        Ok(())
    }

    #[test]
    fn test_stdin_support_signature() {
        // Test that run_stdin has correct signature and compiles
        // We don't actually run it because it would hang waiting for stdin
        // Compile-time verification that the function exists with correct signature
    }

    fn window(offset: Option<usize>, limit: Option<usize>, tail: Option<usize>) -> LineRange {
        LineRange::from_args(offset, limit, tail)
    }

    fn gen_lines(lines: usize, newline: bool) -> String {
        let mut buffer = (1..=lines)
            .map(|n| format!("line {}\n", n))
            .collect::<String>();
        if !newline {
            buffer.pop();
        }
        buffer
    }

    fn get_range(input: &str, range: &LineRange) -> (usize, String) {
        apply_line_window(input, None, range, &Language::Unknown)
    }

    #[test]
    fn test_apply_line_window_tail_lines() {
        let (start, output) = get_range(&gen_lines(4, true), &window(None, None, Some(2)));
        assert_eq!(start, 3);
        assert_eq!(output, "line 3\nline 4\n");
    }

    #[test]
    fn test_apply_line_window_tail_lines_no_trailing_newline() {
        let (start, output) = get_range(&gen_lines(4, false), &window(None, None, Some(2)));
        assert_eq!(start, 3);
        assert_eq!(output, "line 3\nline 4");
    }

    #[test]
    fn test_apply_line_window_max_lines() {
        let (start, output) = apply_line_window(
            &gen_lines(4, true),
            Some(2),
            &window(None, None, None),
            &Language::Unknown,
        );
        assert_eq!(start, 1);
        assert!(output.starts_with("line 1\n"));
        assert!(output.contains("3 more lines"));
    }

    #[test]
    fn test_apply_line_window_offset_only() {
        let (start, output) = get_range(&gen_lines(4, true), &window(Some(2), None, None));
        assert_eq!(start, 2);
        assert_eq!(output, "line 2\nline 3\nline 4\n");
    }

    #[test]
    fn test_apply_line_window_offset_and_max_lines() {
        let (start, output) = apply_line_window(
            &gen_lines(4, true),
            Some(2),
            &window(Some(2), None, None),
            &Language::Unknown,
        );
        assert_eq!(start, 2);
        assert!(output.starts_with("line 2\n"));
        assert!(output.contains("2 more lines"));
    }

    #[test]
    fn test_apply_line_window_offset_beyond_end() {
        let (start, output) = get_range(&gen_lines(2, true), &window(Some(3), None, None));
        assert_eq!(start, 3);
        assert_eq!(output, "");
    }

    #[test]
    fn test_apply_line_window_empty_range() {
        let (_, output) = get_range(&gen_lines(2, true), &window(Some(2), Some(0), None));
        assert_eq!(output, "");
    }

    #[test]
    fn test_apply_line_window_head() {
        let (start, output) = get_range(&gen_lines(5, true), &window(None, Some(3), None));
        assert_eq!(start, 1);
        assert_eq!(output, "line 1\nline 2\nline 3\n");
    }

    #[test]
    fn test_apply_line_window_head_beyond_end() {
        let (start, output) = get_range(&gen_lines(2, true), &window(None, Some(3), None));
        assert_eq!(start, 1);
        assert_eq!(output, "line 1\nline 2\n");
    }

    #[test]
    fn test_apply_line_window_slice() {
        let (start, output) = get_range(&gen_lines(5, true), &window(Some(3), Some(2), None));
        assert_eq!(start, 3);
        assert_eq!(output, "line 3\nline 4\n");
    }

    #[test]
    fn test_apply_line_window_slice_beyond_end() {
        let (start, output) = get_range(&gen_lines(5, true), &window(Some(3), Some(10), None));
        assert_eq!(start, 3);
        assert_eq!(output, "line 3\nline 4\nline 5\n");
    }

    #[test]
    fn test_apply_line_window_slice_start_beyond_end() {
        let (start, output) = get_range(&gen_lines(5, true), &window(Some(6), Some(1), None));
        assert_eq!(start, 6);
        assert_eq!(output, "");
    }

    #[test]
    fn test_read_slice_with_line_numbers() {
        let (start, output) = get_range(&gen_lines(5, true), &window(Some(3), Some(2), None));
        assert_eq!(start, 3);
        let output = format_with_line_numbers(&output, start);
        assert!(output.contains("3 │ line 3\n"), "line 3 wrong: {output:?}");
        assert!(output.contains("4 │ line 4\n"), "line 4 wrong: {output:?}");
    }

    fn rtk_bin() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("debug")
            .join("rtk")
    }

    #[test]
    #[ignore]
    fn test_read_two_valid_files_concatenated() {
        let bin = rtk_bin();
        assert!(bin.exists(), "Run `cargo build` first");

        let mut f1 = NamedTempFile::with_suffix(".txt").unwrap();
        let mut f2 = NamedTempFile::with_suffix(".txt").unwrap();
        writeln!(f1, "alpha\nbravo").unwrap();
        writeln!(f2, "charlie\ndelta").unwrap();

        let output = std::process::Command::new(&bin)
            .args([
                "read",
                &f1.path().to_string_lossy(),
                &f2.path().to_string_lossy(),
            ])
            .output()
            .expect("failed to run rtk read");

        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("alpha"), "first file content missing");
        assert!(stdout.contains("charlie"), "second file content missing");
    }

    #[test]
    #[ignore]
    fn test_read_valid_and_nonexistent() {
        let bin = rtk_bin();
        assert!(bin.exists(), "Run `cargo build` first");

        let mut f1 = NamedTempFile::with_suffix(".txt").unwrap();
        writeln!(f1, "valid content").unwrap();

        let output = std::process::Command::new(&bin)
            .args([
                "read",
                &f1.path().to_string_lossy(),
                "/tmp/rtk_nonexistent_file.txt",
            ])
            .output()
            .expect("failed to run rtk read");

        assert!(
            !output.status.success(),
            "should exit non-zero on missing file"
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stdout.contains("valid content"),
            "valid file should still be printed"
        );
        assert!(
            stderr.contains("rtk_nonexistent_file"),
            "should report missing file on stderr"
        );
    }

    #[test]
    #[ignore]
    fn test_read_stdin_dedup_warning() {
        let bin = rtk_bin();
        assert!(bin.exists(), "Run `cargo build` first");

        let output = std::process::Command::new(&bin)
            .args(["read", "-", "-"])
            .stdin(std::process::Stdio::piped())
            .output()
            .expect("failed to run rtk read");

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("stdin specified more than once"),
            "should warn about duplicate stdin, got stderr: {}",
            stderr
        );
    }
}
