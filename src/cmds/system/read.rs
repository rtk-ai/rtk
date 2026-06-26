//! Reads source files with optional language-aware filtering to strip boilerplate.

use crate::core::filter::{self, FilterLevel, FilterStrategy, Language};
use crate::core::guard::never_worse;
use crate::core::tracking;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

const DEFAULT_AUTO_MAX_LINES: usize = 240;

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

    // For code files with no explicit filter, apply comment stripping.
    // Comments are not functional code — this is lossless compression.
    // Only activates when user didn't specify a filter level (default "none").
    if level == filter::FilterLevel::None
        && !matches!(lang, Language::Data | Language::Unknown)
    {
        let minimal = filter::MinimalFilter;
        let stripped = minimal.filter(&content, &lang);
        // Only use stripped version if it's actually smaller (safeguard)
        if stripped.len() < filtered.len() {
            filtered = stripped;
        }
    }

    // Always compactify indentation for data formats (JSON, YAML, XML, TOML)
    // regardless of filter level. This is lossless — only whitespace reduced.
    if matches!(lang, Language::Data | Language::Unknown) {
        filtered = filter::compactify_indent(filtered);
    }

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

    filtered = apply_line_window_with_original_count(
        &filtered,
        max_lines,
        tail_lines,
        &lang,
        content.lines().count(),
    );

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

    filtered = apply_line_window_with_original_count(
        &filtered,
        max_lines,
        tail_lines,
        &lang,
        content.lines().count(),
    );

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
    tail_lines: Option<usize>,
    lang: &Language,
) -> String {
    apply_line_window_with_original_count(content, max_lines, tail_lines, lang, content.lines().count())
}

fn apply_line_window_with_original_count(
    content: &str,
    max_lines: Option<usize>,
    tail_lines: Option<usize>,
    lang: &Language,
    original_line_count: usize,
) -> String {
    let auto_window = max_lines.is_none() && tail_lines.is_none();
    let filtered_line_count = content.lines().count();
    let content = if auto_window && content.lines().count() > DEFAULT_AUTO_MAX_LINES {
        compact_large_read_noise(content)
    } else {
        content.to_string()
    };

    if let Some(tail) = tail_lines {
        if tail == 0 {
            return String::new();
        }
        let lines: Vec<&str> = content.lines().collect();
        let start = lines.len().saturating_sub(tail);
        let mut result = lines[start..].join("\n");
        if content.ends_with('\n') {
            result.push('\n');
        }
        return result;
    }

    if let Some(max) = max_lines {
        return filter::smart_truncate(content.as_str(), max, lang);
    }

    let line_count = content.lines().count();
    if line_count > DEFAULT_AUTO_MAX_LINES {
        let truncated = filter::smart_truncate(content.as_str(), DEFAULT_AUTO_MAX_LINES, lang);
        return prepend_auto_read_header(
            &truncated,
            original_line_count,
            filtered_line_count,
            line_count,
        );
    }

    content.to_string()
}

fn prepend_auto_read_header(
    excerpt: &str,
    original_line_count: usize,
    filtered_line_count: usize,
    compacted_line_count: usize,
) -> String {
    format!(
        "[rtk read: auto-compressed large file; showing {} agent-selected lines from {} original lines ({} after filtering, {} after blank/repeat compaction); excerpt may be non-contiguous]\n{}",
        excerpt.lines().count(),
        original_line_count,
        filtered_line_count,
        compacted_line_count,
        excerpt
    )
}

fn compact_large_read_noise(content: &str) -> String {
    let without_blank_lines = content
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !is_large_read_noise_line(trimmed)
        })
        .collect::<Vec<_>>()
        .join("\n");
    compact_consecutive_repeated_lines(&without_blank_lines)
}

fn is_large_read_noise_line(trimmed: &str) -> bool {
    is_visual_separator_line(trimmed) || is_progress_noise_line(trimmed)
}

fn is_visual_separator_line(trimmed: &str) -> bool {
    let chars: Vec<char> = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    chars.len() >= 8 && chars.iter().all(|c| "-_=*#~.|+".contains(*c))
}

fn is_progress_noise_line(trimmed: &str) -> bool {
    if !trimmed.contains('%') || trimmed.chars().count() > 160 {
        return false;
    }

    let alpha_count = trimmed.chars().filter(|c| c.is_alphabetic()).count();
    let digit_count = trimmed.chars().filter(|c| c.is_ascii_digit()).count();
    let bar_count = trimmed
        .chars()
        .filter(|c| "[]()<>#=->._|/\\ ".contains(*c))
        .count();

    digit_count > 0 && bar_count >= 4 && alpha_count <= 12
}

fn compact_consecutive_repeated_lines(content: &str) -> String {
    let mut out = Vec::new();
    let mut current: Option<&str> = None;
    let mut repeat_count = 0usize;

    for line in content.lines() {
        if current == Some(line) {
            repeat_count += 1;
            continue;
        }
        flush_repeated_line(&mut out, current, repeat_count);
        current = Some(line);
        repeat_count = 1;
    }
    flush_repeated_line(&mut out, current, repeat_count);
    out.join("\n")
}

fn flush_repeated_line(out: &mut Vec<String>, line: Option<&str>, repeat_count: usize) {
    let Some(line) = line else {
        return;
    };
    if repeat_count >= 4 && !line.trim().is_empty() {
        out.push(line.to_string());
        out.push(format!(
            "[rtk read: previous line repeated {} more times]",
            repeat_count - 1
        ));
    } else {
        for _ in 0..repeat_count {
            out.push(line.to_string());
        }
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
    fn test_apply_line_window_max_lines_still_works() {
        let input = "a\nb\nc\nd\n";
        let output = apply_line_window(input, Some(2), None, &Language::Unknown);
        assert!(output.starts_with("a\n"));
        assert!(output.contains("more lines"));
    }

    #[test]
    fn test_apply_line_window_small_default_preserves_full_content() {
        let input = "a\nb\nc\n";
        let output = apply_line_window(input, None, None, &Language::JavaScript);
        assert_eq!(output, input);
    }

    #[test]
    fn test_apply_line_window_large_default_auto_truncates() {
        let input = (0..400)
            .map(|i| format!("const value{} = {};", i, i))
            .collect::<Vec<_>>()
            .join("\n");

        let output = apply_line_window(&input, None, None, &Language::JavaScript);

        assert!(output.lines().count() <= DEFAULT_AUTO_MAX_LINES + 1);
        assert!(output.contains("auto-compressed large file"));
        assert!(output.contains("agent-selected lines"));
        assert!(output.contains("more lines"));
        assert!(output.len() < input.len() / 2);
    }

    #[test]
    fn test_apply_line_window_large_default_removes_blank_noise() {
        let input = (0..300)
            .map(|i| format!("line {}\n", i))
            .collect::<Vec<_>>()
            .join("\n");

        let output = apply_line_window(&input, None, None, &Language::JavaScript);

        assert!(!output.contains("\n\n"));
        assert!(output.contains("blank/repeat compaction"));
        assert!(output.contains("more lines"));
    }

    #[test]
    fn test_compact_consecutive_repeated_lines_counts_noise() {
        let input = "same\nsame\nsame\nsame\nsame\nnext";
        let output = compact_consecutive_repeated_lines(input);

        assert_eq!(
            output,
            "same\n[rtk read: previous line repeated 4 more times]\nnext"
        );
    }

    #[test]
    fn test_compact_large_read_noise_removes_visual_progress_but_keeps_signals() {
        let input = "==========\n\
                     [=====>     ] 45%\n\
                     warning: retrying request\n\
                     warning: retrying request\n\
                     warning: retrying request\n\
                     warning: retrying request\n\
                     error: failed to open config\n\
                     ---\n\
                     done\n";
        let output = compact_large_read_noise(input);

        assert!(!output.contains("=========="));
        assert!(!output.contains("[=====>     ] 45%"));
        assert!(output.contains("warning: retrying request"));
        assert!(output.contains("[rtk read: previous line repeated 3 more times]"));
        assert!(output.contains("error: failed to open config"));
        assert!(output.contains("---"));
        assert!(output.contains("done"));
    }

    #[test]
    fn test_apply_line_window_explicit_tail_overrides_default_auto_truncate() {
        let input = (0..400)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");

        let output = apply_line_window(&input, None, Some(2), &Language::Unknown);

        assert_eq!(output, "line 398\nline 399");
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
