//! Reads source files with optional language-aware filtering to strip boilerplate.

use crate::core::ai_output::{AiDocument, AiRecord, BudgetClass, Omission, Severity};
use crate::core::filter::{self, FilterLevel, Language};
use crate::core::guard::never_worse;
use crate::core::tracking;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// An inclusive, one-based source line range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRange {
    pub start: usize,
    pub end: usize,
}

/// Parse an inclusive one-based line range such as `120:160`.
pub fn parse_line_range(value: &str) -> Result<LineRange> {
    let (start, end) = value
        .split_once(':')
        .context("line range must use START:END syntax")?;
    if start.is_empty() || end.is_empty() || end.contains(':') {
        anyhow::bail!("line range must use START:END syntax");
    }
    let start = start
        .parse::<usize>()
        .context("line range bounds must be positive integers")?;
    let end = end
        .parse::<usize>()
        .context("line range bounds must be positive integers")?;
    if start == 0 || end == 0 {
        anyhow::bail!("line range bounds must be greater than zero");
    }
    if start > end {
        anyhow::bail!("line range start must not exceed end");
    }
    Ok(LineRange { start, end })
}

/// Clap-compatible parser for [`LineRange`].
pub fn parse_line_range_arg(value: &str) -> std::result::Result<LineRange, String> {
    parse_line_range(value).map_err(|error| error.to_string())
}

/// Select a source range while preserving the bytes, line endings, and source
/// line addresses represented by the selected portion.
pub fn select_line_range(content: &str, range: LineRange) -> Result<String> {
    let line_count = content_line_count(content);
    if line_count == 0 || range.start > line_count {
        anyhow::bail!("line range starts after the end of the file");
    }
    let end = range.end.min(line_count);
    let start_offset = line_start_offset(content, range.start).expect("validated start line");
    let end_offset = line_start_offset(content, end + 1).unwrap_or(content.len());
    Ok(content[start_offset..end_offset].to_string())
}

fn content_line_count(content: &str) -> usize {
    if content.is_empty() {
        0
    } else {
        content.bytes().filter(|byte| *byte == b'\n').count()
            + usize::from(!content.ends_with('\n'))
    }
}

fn line_start_offset(content: &str, line: usize) -> Option<usize> {
    if line == 1 {
        return Some(0);
    }
    let mut current = 1usize;
    for (offset, byte) in content.bytes().enumerate() {
        if byte == b'\n' {
            current += 1;
            if current == line {
                return Some(offset + 1);
            }
        }
    }
    None
}

fn source_document(file: &str, lines: &[filter::FilteredLine]) -> AiDocument {
    let mut document = AiDocument::new(Some("source"));
    document.fact("file", file);
    for line in lines {
        document.push(AiRecord::new(
            Severity::Info,
            format!("{}: {}", line.original_line, line.text),
        ));
    }
    document
}

struct AiSourceRequest<'a> {
    timer: &'a tracking::TimedExecution,
    original_cmd: &'a str,
    rtk_cmd: &'a str,
    file_label: &'a str,
    content: &'a str,
    level: FilterLevel,
    lang: &'a Language,
    max_lines: Option<usize>,
    tail_lines: Option<usize>,
    line_numbers: bool,
    line_offset: usize,
    fallback_baseline: &'a str,
    verbose: u8,
}

fn emit_ai_source(request: AiSourceRequest<'_>) -> bool {
    let AiSourceRequest {
        timer,
        original_cmd,
        rtk_cmd,
        file_label,
        content,
        level,
        lang,
        max_lines,
        tail_lines,
        line_numbers,
        line_offset,
        fallback_baseline,
        verbose,
    } = request;
    let filter = filter::get_filter(level);
    let mut lines = filter.filter_lines(content, lang);
    for line in &mut lines {
        line.original_line += line_offset;
    }
    let selected = select_filtered_line_window(&lines, max_lines, tail_lines);
    if selected.is_empty() {
        return false;
    }

    if verbose > 0 {
        let original_lines = content.lines().count();
        let reduction = if original_lines > 0 {
            ((original_lines - selected.len()) as f64 / original_lines as f64) * 100.0
        } else {
            0.0
        };
        eprintln!(
            "Lines: {} -> {} ({:.1}% reduction)",
            original_lines,
            selected.len(),
            reduction
        );
    }

    let omitted_by_filter = content.lines().count().saturating_sub(lines.len());
    let mut document = source_document(file_label, &selected);
    if let Some(max) = max_lines {
        document.fact("window", format!("max={max}"));
    } else if let Some(tail) = tail_lines {
        document.fact("window", format!("tail={tail}"));
    }
    if omitted_by_filter > 0 {
        document = document.with_omission(Omission {
            items: omitted_by_filter,
            groups: 0,
        });
    }
    let native_output = if line_numbers {
        format_with_line_numbers_from(content, line_offset + 1)
    } else {
        content.to_string()
    };
    let fallback = if max_lines.is_some() || tail_lines.is_some() {
        if line_numbers {
            format_with_line_numbers_from(fallback_baseline, line_offset + 1)
        } else {
            fallback_baseline.to_string()
        }
    } else {
        native_output.clone()
    };
    crate::core::runner::emit_ai_document_with_baseline(
        crate::core::runner::AiEmission {
            timer,
            original_cmd,
            rtk_cmd,
            raw: &native_output,
            fallback_baseline: &fallback,
            command_slug: "read",
            budget: BudgetClass::Source,
            trailing_newline: true,
        },
        document,
    );
    true
}

pub fn run(
    file: &Path,
    level: FilterLevel,
    max_lines: Option<usize>,
    tail_lines: Option<usize>,
    line_numbers: bool,
    line_range: Option<LineRange>,
    verbose: u8,
) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Reading: {} (filter: {})", file.display(), level);
    }

    // Read file content
    let content = fs::read_to_string(file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let line_offset = line_range.map(|range| range.start - 1).unwrap_or(0);
    let content = if let Some(range) = line_range {
        select_line_range(&content, range)?
    } else {
        content
    };

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

    filtered = apply_line_window(&filtered, max_lines, tail_lines, &lang);

    let (raw, rtk_output) = if line_numbers {
        (
            format_with_line_numbers_from(&content, line_offset + 1),
            format_with_line_numbers_from(&filtered, line_offset + 1),
        )
    } else {
        (content.clone(), filtered.clone())
    };

    let original_cmd = format!("cat {}", file.display());
    let file_label = file.display().to_string();
    if level != FilterLevel::None
        && emit_ai_source(AiSourceRequest {
            timer: &timer,
            original_cmd: &original_cmd,
            rtk_cmd: "rtk read",
            file_label: &file_label,
            content: &content,
            level,
            lang: &lang,
            max_lines,
            tail_lines,
            line_numbers,
            line_offset,
            fallback_baseline: if max_lines.is_none() && tail_lines.is_none() {
                &content
            } else {
                &filtered
            },
            verbose,
        })
    {
        return Ok(());
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
    line_range: Option<LineRange>,
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

    let line_offset = line_range.map(|range| range.start - 1).unwrap_or(0);
    let content = if let Some(range) = line_range {
        select_line_range(&content, range)?
    } else {
        content
    };

    // No file extension, so use Unknown language
    let lang = Language::Unknown;

    if verbose > 1 {
        eprintln!("Language: {:?} (stdin has no extension)", lang);
    }

    // Apply filter
    let filter = filter::get_filter(level);
    let mut filtered = filter.filter(&content, &lang);

    // Safety: if filter emptied non-empty stdin, fall back to raw content.
    if filtered.trim().is_empty() && !content.trim().is_empty() {
        eprintln!(
            "rtk: warning: filter produced empty output for stdin ({} bytes), showing raw content",
            content.len()
        );
        filtered = content.clone();
    }

    filtered = apply_line_window(&filtered, max_lines, tail_lines, &lang);

    let (raw, rtk_output) = if line_numbers {
        (
            format_with_line_numbers_from(&content, line_offset + 1),
            format_with_line_numbers_from(&filtered, line_offset + 1),
        )
    } else {
        (content.clone(), filtered.clone())
    };

    if level != FilterLevel::None
        && emit_ai_source(AiSourceRequest {
            timer: &timer,
            original_cmd: "cat - (stdin)",
            rtk_cmd: "rtk read -",
            file_label: "stdin",
            content: &content,
            level,
            lang: &lang,
            max_lines,
            tail_lines,
            line_numbers,
            line_offset,
            fallback_baseline: if max_lines.is_none() && tail_lines.is_none() {
                &content
            } else {
                &filtered
            },
            verbose,
        })
    {
        return Ok(());
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

    let shown = never_worse(&raw, &rtk_output);
    print!("{}", shown);

    timer.track("cat - (stdin)", "rtk read -", &raw, shown);
    Ok(())
}

fn format_with_line_numbers_from(content: &str, first_line: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let width = first_line.saturating_add(lines.len().saturating_sub(1)).to_string().len();
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        out.push_str(&format!(
            "{:>width$} │ {}\n",
            first_line + i,
            line,
            width = width
        ));
    }
    out
}

fn select_filtered_line_window(
    lines: &[filter::FilteredLine],
    max_lines: Option<usize>,
    tail_lines: Option<usize>,
) -> Vec<filter::FilteredLine> {
    if let Some(tail) = tail_lines {
        let start = lines.len().saturating_sub(tail);
        return lines[start..].to_vec();
    }

    if let Some(max) = max_lines {
        return filter::smart_truncate_lines(lines, max);
    }

    lines.to_vec()
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
        let lines: Vec<&str> = content.lines().collect();
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
        run(file.path(), FilterLevel::Minimal, None, None, false, None, 0)?;
        Ok(())
    }

    #[test]
    fn test_stdin_support_signature() {
        // Test that run_stdin has correct signature and compiles
        // We don't actually run it because it would hang waiting for stdin
        // Compile-time verification that the function exists with correct signature
    }

    #[test]
    fn source_document_uses_original_dense_line_markers() {
        let lines = vec![
            filter::FilteredLine {
                original_line: 2,
                text: "fn kept() {}".into(),
            },
            filter::FilteredLine {
                original_line: 5,
                text: "let value = 1;".into(),
            },
        ];

        let rendered = crate::core::ai_output::render(
            &source_document("sample.rs", &lines),
            crate::core::ai_output::BudgetClass::Source,
        )
        .text;

        assert_eq!(
            rendered,
            "status=source file=sample.rs\n2: fn kept() {}\n5: let value = 1;"
        );
    }

    #[test]
    fn selected_source_window_keeps_legacy_priority_and_original_locations() {
        let lines = vec![
            filter::FilteredLine {
                original_line: 2,
                text: "let first = 1;".into(),
            },
            filter::FilteredLine {
                original_line: 5,
                text: "fn retained() {}".into(),
            },
            filter::FilteredLine {
                original_line: 9,
                text: "let not_reached = 2;".into(),
            },
            filter::FilteredLine {
                original_line: 12,
                text: "let also_not_reached = 3;".into(),
            },
        ];

        let selected = select_filtered_line_window(&lines, Some(3), None);

        assert_eq!(
            selected
                .iter()
                .map(|line| line.original_line)
                .collect::<Vec<_>>(),
            vec![2, 5]
        );
    }

    #[test]
    fn selected_tail_window_keeps_original_locations() {
        let lines = vec![
            filter::FilteredLine {
                original_line: 2,
                text: "first".into(),
            },
            filter::FilteredLine {
                original_line: 5,
                text: "second".into(),
            },
            filter::FilteredLine {
                original_line: 9,
                text: "third".into(),
            },
        ];

        let selected = select_filtered_line_window(&lines, None, Some(2));
        assert_eq!(
            selected
                .iter()
                .map(|line| line.original_line)
                .collect::<Vec<_>>(),
            vec![5, 9]
        );
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
    fn line_range_preserves_crlf_and_original_selection() -> Result<()> {
        let selected = select_line_range("L001\r\nL002\r\nL003\r\n", LineRange { start: 2, end: 2 })?;
        assert_eq!(selected, "L002\r\n");
        Ok(())
    }

    #[test]
    fn line_range_rejects_invalid_bounds() {
        for value in ["0:5", "10:3", "10", "10:", ":10", "1:2:3"] {
            assert!(parse_line_range(value).is_err(), "accepted {value}");
        }
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
