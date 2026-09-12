//! Reads source files with optional language-aware filtering to strip boilerplate.

use crate::core::filter::{self, FilterLevel, Language};
use crate::core::guard::never_worse;
use crate::core::tracking;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

pub fn run(
    file: &Path,
    level: FilterLevel,
    max_lines: Option<usize>,
    head_lines: Option<usize>,
    tail_lines: Option<usize>,
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

    filtered = apply_line_window(&filtered, max_lines, head_lines, tail_lines, &lang);

    let (raw, rtk_output) = if line_numbers {
        (
            format_with_line_numbers(&content),
            format_with_line_numbers(&filtered),
        )
    } else {
        (content.clone(), filtered.clone())
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
    head_lines: Option<usize>,
    tail_lines: Option<usize>,
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
    let mut filtered = filter.filter(&content, &lang);

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

    filtered = apply_line_window(&filtered, max_lines, head_lines, tail_lines, &lang);

    let (raw, rtk_output) = if line_numbers {
        (
            format_with_line_numbers(&content),
            format_with_line_numbers(&filtered),
        )
    } else {
        (content.clone(), filtered.clone())
    };
    let shown = never_worse(&raw, &rtk_output);
    print!("{}", shown);

    timer.track("cat - (stdin)", "rtk read -", &raw, shown);
    Ok(())
}

fn format_with_line_numbers(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let width = lines.len().to_string().len();
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        out.push_str(&format!("{:>width$} │ {}\n", i + 1, line, width = width));
    }
    out
}

fn apply_line_window(
    content: &str,
    max_lines: Option<usize>,
    head_lines: Option<usize>,
    tail_lines: Option<usize>,
    lang: &Language,
) -> String {
    if let Some(head) = head_lines {
        return head_window(content, head);
    }

    if let Some(tail) = tail_lines {
        return tail_window(content, tail);
    }

    if let Some(max) = max_lines {
        return filter::smart_truncate(content, max, lang);
    }

    content.to_string()
}

/// First `n` lines, sliced on byte offsets rather than round-tripped through
/// `lines()`, so CRLF endings and an unterminated final line survive verbatim.
/// `\n` is ASCII, so slicing just past one always lands on a char boundary.
fn head_window(content: &str, n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    let mut seen = 0;
    for (idx, byte) in content.bytes().enumerate() {
        if byte == b'\n' {
            seen += 1;
            if seen == n {
                return content[..=idx].to_string();
            }
        }
    }
    content.to_string()
}

/// Last `n` lines, byte-sliced for the same fidelity reasons as `head_window`.
/// A trailing newline terminates the final line instead of starting a new one,
/// so it is excluded before counting separators backwards — otherwise `n` would
/// select one line too few for newline-terminated input.
fn tail_window(content: &str, n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    let bytes = content.as_bytes();
    let search_end = match bytes.last() {
        Some(b'\n') => bytes.len() - 1,
        _ => bytes.len(),
    };
    let mut seen = 0;
    for idx in (0..search_end).rev() {
        if bytes[idx] == b'\n' {
            seen += 1;
            if seen == n {
                return content[idx + 1..].to_string();
            }
        }
    }
    content.to_string()
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
        run(file.path(), FilterLevel::Minimal, None, None, None, false, 0)?;
        Ok(())
    }

    #[test]
    fn test_stdin_support_signature() {
        // Test that run_stdin has correct signature and compiles
        // We don't actually run it because it would hang waiting for stdin
        // Compile-time verification that the function exists with correct signature
    }

    #[test]
    fn test_apply_line_window_tail_lines() {
        let input = "a\nb\nc\nd\n";
        let output = apply_line_window(input, None, None, Some(2), &Language::Unknown);
        assert_eq!(output, "c\nd\n");
    }

    #[test]
    fn test_apply_line_window_tail_lines_no_trailing_newline() {
        let input = "a\nb\nc\nd";
        let output = apply_line_window(input, None, None, Some(2), &Language::Unknown);
        assert_eq!(output, "c\nd");
    }

    #[test]
    fn test_head_window_matches_native_head() {
        let input = "1\n2\n3\n4\n5\n";
        assert_eq!(
            apply_line_window(input, None, Some(3), None, &Language::Unknown),
            "1\n2\n3\n"
        );
    }

    /// The defect this window exists to fix: `--max-lines N` keeps only about
    /// N/2 lines, so it could never stand in for `head -N`.
    #[test]
    fn test_head_window_keeps_all_n_lines_unlike_max_lines() {
        let input = (1..=200)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let head = apply_line_window(&input, None, Some(10), None, &Language::Unknown);
        assert_eq!(head.lines().count(), 10);
        assert_eq!(head.lines().last(), Some("10"));
    }

    #[test]
    fn test_head_window_single_line() {
        let input = "1\n2\n3\n";
        assert_eq!(
            apply_line_window(input, None, Some(1), None, &Language::Unknown),
            "1\n"
        );
    }

    #[test]
    fn test_head_window_zero_is_empty() {
        assert_eq!(
            apply_line_window("a\nb\n", None, Some(0), None, &Language::Unknown),
            ""
        );
    }

    #[test]
    fn test_head_window_n_exceeds_line_count() {
        let input = "a\nb\n";
        assert_eq!(
            apply_line_window(input, None, Some(99), None, &Language::Unknown),
            input
        );
    }

    #[test]
    fn test_head_window_empty_input() {
        assert_eq!(apply_line_window("", None, Some(5), None, &Language::Unknown), "");
    }

    #[test]
    fn test_head_window_unterminated_final_line() {
        assert_eq!(
            apply_line_window("a\nb\nc", None, Some(3), None, &Language::Unknown),
            "a\nb\nc"
        );
    }

    #[test]
    fn test_head_window_preserves_crlf() {
        assert_eq!(
            apply_line_window("a\r\nb\r\nc\r\n", None, Some(2), None, &Language::Unknown),
            "a\r\nb\r\n"
        );
    }

    #[test]
    fn test_tail_window_preserves_crlf() {
        assert_eq!(
            apply_line_window("a\r\nb\r\nc\r\n", None, None, Some(2), &Language::Unknown),
            "b\r\nc\r\n"
        );
    }

    /// Without discounting the terminal newline, counting separators backwards
    /// selects one line too few for newline-terminated input.
    #[test]
    fn test_tail_window_unterminated_single_line() {
        assert_eq!(
            apply_line_window("a\nb\nc", None, None, Some(1), &Language::Unknown),
            "c"
        );
    }

    #[test]
    fn test_tail_window_n_exceeds_line_count() {
        let input = "a\nb\n";
        assert_eq!(
            apply_line_window(input, None, None, Some(99), &Language::Unknown),
            input
        );
    }

    #[test]
    fn test_tail_window_empty_input() {
        assert_eq!(apply_line_window("", None, None, Some(5), &Language::Unknown), "");
    }

    #[test]
    fn test_max_lines_zero_is_empty() {
        assert_eq!(
            apply_line_window("a\nb\nc\n", Some(0), None, None, &Language::Unknown),
            ""
        );
    }

    #[test]
    fn test_head_window_mixed_line_endings() {
        assert_eq!(
            apply_line_window("a\r\nb\nc\r\n", None, Some(2), None, &Language::Unknown),
            "a\r\nb\n"
        );
    }

    #[test]
    fn test_tail_window_mixed_line_endings() {
        assert_eq!(
            apply_line_window("a\r\nb\nc\r\n", None, None, Some(2), &Language::Unknown),
            "b\nc\r\n"
        );
    }

    #[test]
    fn test_windows_preserve_multibyte_utf8() {
        let input = "héllo\n日本語\nثالث\n";
        assert_eq!(
            apply_line_window(input, None, Some(2), None, &Language::Unknown),
            "héllo\n日本語\n"
        );
        assert_eq!(
            apply_line_window(input, None, None, Some(2), &Language::Unknown),
            "日本語\nثالث\n"
        );
    }

    #[test]
    fn test_windows_on_blank_lines_only() {
        assert_eq!(
            apply_line_window("\n\n\n", None, Some(2), None, &Language::Unknown),
            "\n\n"
        );
        assert_eq!(
            apply_line_window("\n\n\n", None, None, Some(2), &Language::Unknown),
            "\n\n"
        );
    }

    #[test]
    fn test_tail_window_zero_is_empty() {
        assert_eq!(
            apply_line_window("a\nb\n", None, None, Some(0), &Language::Unknown),
            ""
        );
    }

    #[test]
    fn test_apply_line_window_max_lines_still_works() {
        let input = "a\nb\nc\nd\n";
        let output = apply_line_window(input, Some(2), None, None, &Language::Unknown);
        assert!(output.starts_with("a\n"));
        assert!(output.contains("more lines"));
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
            .args(["read", &f1.path().to_string_lossy(), &f2.path().to_string_lossy()])
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
            .args(["read", &f1.path().to_string_lossy(), "/tmp/rtk_nonexistent_file.txt"])
            .output()
            .expect("failed to run rtk read");

        assert!(!output.status.success(), "should exit non-zero on missing file");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stdout.contains("valid content"), "valid file should still be printed");
        assert!(stderr.contains("rtk_nonexistent_file"), "should report missing file on stderr");
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
