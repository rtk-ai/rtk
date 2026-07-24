//! Reads source files with optional language-aware filtering to strip boilerplate.

use crate::core::filter::{self, FilterLevel, Language};
use crate::core::guard::never_worse;
use crate::core::tracking;
use anyhow::{Context, Result};
use std::fs;
use std::io::{Read as IoRead, Seek, SeekFrom};
use std::path::Path;

pub fn run(
    file: &Path,
    level: FilterLevel,
    max_lines: Option<usize>,
    tail_lines: Option<usize>,
    line_numbers: bool,
    verbose: u8,
) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Reading: {} (filter: {})", file.display(), level);
    }

    // For --tail-lines, read only the needed slice from the end of the file
    // instead of the whole thing: on a large file, `fs::read_to_string`
    // materializes the entire file (and its later `.clone()`s) in memory
    // before the tail window is ever applied, which turned a bounded
    // `tail -n 3000` into a 49GB RSS OOM on a 161GB log (rtk-ai/rtk#3107).
    // `max_lines` (smart_truncate) isn't bounded here: it intentionally scans
    // the whole file for structurally important lines anywhere in it, so a
    // prefix-only read would silently change its output — a separate,
    // harder problem left for a follow-up.
    let content = if let Some(tail) = tail_lines {
        read_tail_lines_bounded(file, tail)
            .with_context(|| format!("Failed to read file: {}", file.display()))?
    } else {
        fs::read_to_string(file)
            .with_context(|| format!("Failed to read file: {}", file.display()))?
    };

    // Stateful filters (`--level minimal|aggressive`) track context (open
    // block comments, in-progress docstrings) as they scan lines from the
    // top of the file. `--tail-lines` only reads the tail slice, so the
    // filter would start mid-context with no way to know it — e.g. a block
    // comment opened before the tail window would leak its closing
    // fragment as if it were code. Force `none` in that combination instead
    // of risking silently corrupted output.
    let effective_level = effective_filter_level(level, tail_lines);
    if effective_level != level {
        eprintln!(
            "rtk: warning: --level {} has no effect with --tail-lines (the tail slice doesn't carry enough file context for stateful filtering)",
            level
        );
    }
    let level = effective_level;

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

    filtered = apply_line_window(&filtered, max_lines, tail_lines, &lang);

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

    filtered = apply_line_window(&filtered, max_lines, tail_lines, &lang);

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

/// `--tail-lines` reads only the tail slice of a file, so a stateful filter
/// (`minimal`/`aggressive`) has no way to see context from earlier in the
/// file. Returns `none` in that combination; returns `level` unchanged
/// otherwise.
fn effective_filter_level(level: FilterLevel, tail_lines: Option<usize>) -> FilterLevel {
    if tail_lines.is_some() && level != FilterLevel::None {
        FilterLevel::None
    } else {
        level
    }
}

/// Splits `text` into lines like `str::lines()`, but keeps any trailing `\r`
/// on each line intact. `str::lines()` strips `\r` before `\n`, which is
/// exactly right for display but corrupts CRLF files when the split-and-join
/// round trip is used to extract a byte-for-byte tail window.
fn split_lines_preserve_cr(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    // split('\n') on a trailing newline yields one extra "" element that
    // str::lines() doesn't produce; drop it to keep the same line count.
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// Reads roughly the last `tail_lines` lines of `path` without loading the
/// whole file into memory: seeks backward from the end in fixed-size chunks,
/// stopping once enough newlines have been seen (or the start of the file is
/// reached). Memory use is bounded by the tail content itself plus one
/// chunk, not file size.
fn read_tail_lines_bounded(path: &Path, tail_lines: usize) -> Result<String> {
    const CHUNK_SIZE: u64 = 64 * 1024;

    if tail_lines == 0 {
        return Ok(String::new());
    }

    let mut file = fs::File::open(path)?;
    let file_len = file.metadata()?.len();
    if file_len == 0 {
        return Ok(String::new());
    }

    let mut pos = file_len;
    let mut newline_count = 0usize;
    // Chunks are read newest-to-oldest; reversed once collection stops.
    let mut chunks: Vec<Vec<u8>> = Vec::new();

    while pos > 0 {
        let read_size = CHUNK_SIZE.min(pos);
        pos -= read_size;
        file.seek(SeekFrom::Start(pos))?;
        let mut chunk = vec![0u8; read_size as usize];
        file.read_exact(&mut chunk)?;
        newline_count += chunk.iter().filter(|&&b| b == b'\n').count();
        chunks.push(chunk);
        if newline_count > tail_lines {
            break;
        }
    }

    chunks.reverse();
    let bytes: Vec<u8> = chunks.into_iter().flatten().collect();
    let text = String::from_utf8_lossy(&bytes).into_owned();

    let lines = split_lines_preserve_cr(&text);
    let start = lines.len().saturating_sub(tail_lines);
    let mut result = lines[start..].join("\n");
    if text.ends_with('\n') {
        result.push('\n');
    }
    Ok(result)
}

fn apply_line_window(
    content: &str,
    max_lines: Option<usize>,
    tail_lines: Option<usize>,
    lang: &Language,
) -> String {
    if let Some(tail) = tail_lines {
        if tail == 0 {
            return String::new();
        }
        let lines = split_lines_preserve_cr(content);
        let start = lines.len().saturating_sub(tail);
        let mut result = lines[start..].join("\n");
        if content.ends_with('\n') {
            result.push('\n');
        }
        return result;
    }

    if let Some(max) = max_lines {
        return filter::smart_truncate(content, max, lang);
    }

    content.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_read_tail_lines_bounded_matches_naive_window() -> Result<()> {
        let mut file = NamedTempFile::new()?;
        write!(file, "a\nb\nc\nd\ne\n")?;
        let bounded = read_tail_lines_bounded(file.path(), 2)?;
        assert_eq!(bounded, "d\ne\n");
        Ok(())
    }

    #[test]
    fn test_read_tail_lines_bounded_tail_exceeds_file() -> Result<()> {
        let mut file = NamedTempFile::new()?;
        write!(file, "a\nb\n")?;
        let bounded = read_tail_lines_bounded(file.path(), 100)?;
        assert_eq!(bounded, "a\nb\n");
        Ok(())
    }

    #[test]
    fn test_read_tail_lines_bounded_no_trailing_newline() -> Result<()> {
        let mut file = NamedTempFile::new()?;
        write!(file, "a\nb\nc")?;
        let bounded = read_tail_lines_bounded(file.path(), 2)?;
        assert_eq!(bounded, "b\nc");
        Ok(())
    }

    #[test]
    fn test_read_tail_lines_bounded_preserves_crlf() -> Result<()> {
        let mut file = NamedTempFile::new()?;
        write!(file, "a\r\nb\r\nc\r\n")?;
        let bounded = read_tail_lines_bounded(file.path(), 2)?;
        assert_eq!(bounded, "b\r\nc\r\n");
        Ok(())
    }

    #[test]
    fn test_read_tail_lines_bounded_empty_file() -> Result<()> {
        let file = NamedTempFile::new()?;
        let bounded = read_tail_lines_bounded(file.path(), 5)?;
        assert_eq!(bounded, "");
        Ok(())
    }

    #[test]
    fn test_read_tail_lines_bounded_zero_tail() -> Result<()> {
        let mut file = NamedTempFile::new()?;
        write!(file, "a\nb\nc\n")?;
        let bounded = read_tail_lines_bounded(file.path(), 0)?;
        assert_eq!(bounded, "");
        Ok(())
    }

    #[test]
    fn test_read_tail_lines_bounded_spans_multiple_chunks() -> Result<()> {
        // Force the backward reader across several 64KB chunk boundaries and
        // confirm the extracted tail still matches an in-memory reference
        // computed the naive way.
        let mut file = NamedTempFile::new()?;
        let mut expected_lines = Vec::new();
        for i in 0..20_000 {
            let line = format!("line-{i:06}-{}", "x".repeat(20));
            writeln!(file, "{line}")?;
            expected_lines.push(line);
        }
        file.flush()?;

        let tail_n = 500;
        let bounded = read_tail_lines_bounded(file.path(), tail_n)?;
        let expected = expected_lines[expected_lines.len() - tail_n..].join("\n") + "\n";
        assert_eq!(bounded, expected);
        Ok(())
    }
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
        run(file.path(), FilterLevel::Minimal, None, None, false, 0)?;
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
        let output = apply_line_window(input, None, Some(2), &Language::Unknown);
        assert_eq!(output, "c\nd\n");
    }

    #[test]
    fn test_apply_line_window_tail_lines_no_trailing_newline() {
        let input = "a\nb\nc\nd";
        let output = apply_line_window(input, None, Some(2), &Language::Unknown);
        assert_eq!(output, "c\nd");
    }

    #[test]
    fn test_apply_line_window_tail_lines_preserves_crlf() {
        let input = "a\r\nb\r\nc\r\nd\r\n";
        let output = apply_line_window(input, None, Some(2), &Language::Unknown);
        assert_eq!(output, "c\r\nd\r\n");
    }

    #[test]
    fn test_effective_filter_level_forces_none_with_tail_lines() {
        assert_eq!(
            effective_filter_level(FilterLevel::Minimal, Some(3)),
            FilterLevel::None
        );
        assert_eq!(
            effective_filter_level(FilterLevel::Aggressive, Some(3)),
            FilterLevel::None
        );
    }

    #[test]
    fn test_effective_filter_level_unchanged_without_tail_lines() {
        assert_eq!(
            effective_filter_level(FilterLevel::Minimal, None),
            FilterLevel::Minimal
        );
        assert_eq!(
            effective_filter_level(FilterLevel::None, Some(3)),
            FilterLevel::None
        );
    }

    #[test]
    fn test_apply_line_window_max_lines_still_works() {
        let input = "a\nb\nc\nd\n";
        let output = apply_line_window(input, Some(2), None, &Language::Unknown);
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
