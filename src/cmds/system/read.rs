//! Reads source files with optional language-aware filtering to strip boilerplate.

use crate::core::filter::{self, FilterLevel, Language};
use crate::core::tracking;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const SMALL_FILE_LINES: usize = 160;
const MEDIUM_FILE_LINES: usize = 600;
const MEDIUM_COMPACT_MAX_LINES: usize = 200;
const LARGE_COMPACT_MAX_LINES: usize = 120;
const DATA_COMPACT_MAX_LINES: usize = 80;
const SYMBOL_CONTEXT_LINES: usize = 12;

#[derive(Default, Deserialize, Serialize)]
struct ReadHashCache {
    files: HashMap<String, String>,
}

pub struct ReadOptions<'a> {
    pub level: FilterLevel,
    pub max_lines: Option<usize>,
    pub tail_lines: Option<usize>,
    pub line_numbers: bool,
    pub symbol: Option<&'a str>,
    pub changed: bool,
}

pub fn run(
    file: &Path,
    options: ReadOptions<'_>,
    verbose: u8,
) -> Result<()> {
    let ReadOptions {
        level,
        max_lines,
        tail_lines,
        line_numbers,
        symbol,
        changed,
    } = options;
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Reading: {} (filter: {})", file.display(), level);
    }

    // Read file content
    let content = fs::read_to_string(file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    if changed && !record_changed(file, &content)? {
        println!("[unchanged: {}]", file.display());
        return Ok(());
    }

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

    if let Some(symbol) = symbol {
        filtered = extract_symbol_context(&filtered, symbol);
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

    filtered = apply_default_line_window(&filtered, level, max_lines, tail_lines, &lang);

    let rtk_output = if line_numbers {
        format_with_line_numbers(&filtered)
    } else {
        filtered.clone()
    };
    print!("{}", rtk_output);
    timer.track(
        &format!("cat {}", file.display()),
        "rtk read",
        &content,
        &rtk_output,
    );
    Ok(())
}

pub fn run_stdin(
    options: ReadOptions<'_>,
    verbose: u8,
) -> Result<()> {
    use std::io::{self, Read as IoRead};

    let ReadOptions {
        level,
        max_lines,
        tail_lines,
        line_numbers,
        symbol,
        ..
    } = options;
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
    if let Some(symbol) = symbol {
        filtered = extract_symbol_context(&filtered, symbol);
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

    filtered = apply_default_line_window(&filtered, level, max_lines, tail_lines, &lang);

    let rtk_output = if line_numbers {
        format_with_line_numbers(&filtered)
    } else {
        filtered.clone()
    };
    print!("{}", rtk_output);

    timer.track("cat - (stdin)", "rtk read -", &content, &rtk_output);
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

fn apply_default_line_window(
    content: &str,
    level: FilterLevel,
    max_lines: Option<usize>,
    tail_lines: Option<usize>,
    lang: &Language,
) -> String {
    let effective_max_lines =
        if tail_lines.is_some() || max_lines.is_some() || level == FilterLevel::None {
            max_lines
        } else {
            adaptive_max_lines(content, lang)
        };

    apply_line_window(content, effective_max_lines, tail_lines, lang)
}

fn adaptive_max_lines(content: &str, lang: &Language) -> Option<usize> {
    let lines = content.lines().count();
    if lines <= SMALL_FILE_LINES {
        None
    } else if *lang == Language::Data {
        Some(DATA_COMPACT_MAX_LINES)
    } else if lines <= MEDIUM_FILE_LINES {
        Some(MEDIUM_COMPACT_MAX_LINES)
    } else {
        Some(LARGE_COMPACT_MAX_LINES)
    }
}

fn extract_symbol_context(content: &str, symbol: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let matches: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, line)| line.contains(symbol).then_some(i))
        .collect();
    if matches.is_empty() {
        return format!("[symbol not found: {symbol}]\n");
    }

    let mut keep = vec![false; lines.len()];
    for index in matches {
        let start = index.saturating_sub(SYMBOL_CONTEXT_LINES);
        let end = (index + SYMBOL_CONTEXT_LINES + 1).min(lines.len());
        keep[start..end].fill(true);
    }

    let mut output = String::new();
    let mut previous_kept = false;
    for (line, keep_line) in lines.iter().zip(keep) {
        if keep_line {
            if !previous_kept && !output.is_empty() {
                output.push_str("[...]\n");
            }
            output.push_str(line);
            output.push('\n');
        }
        previous_kept = keep_line;
    }
    output
}

fn read_hash_cache_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("rtk")
        .join("read-hashes.json")
}

fn record_changed(file: &Path, content: &str) -> Result<bool> {
    let cache_path = read_hash_cache_path();
    let mut cache: ReadHashCache = fs::read_to_string(&cache_path)
        .ok()
        .and_then(|value| serde_json::from_str(&value).ok())
        .unwrap_or_default();
    let key = fs::canonicalize(file)
        .unwrap_or_else(|_| file.to_path_buf())
        .to_string_lossy()
        .into_owned();
    let hash = format!("{:x}", Sha256::digest(content.as_bytes()));
    let changed = cache.files.get(&key) != Some(&hash);
    cache.files.insert(key, hash);
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(cache_path, serde_json::to_vec(&cache)?)?;
    Ok(changed)
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
            ReadOptions {
                level: FilterLevel::Minimal,
                max_lines: None,
                tail_lines: None,
                line_numbers: false,
                symbol: None,
                changed: false,
            },
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
    fn test_default_compact_window_truncates_filtered_large_reads() {
        let input = (0..500)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");

        let output =
            apply_default_line_window(&input, FilterLevel::Minimal, None, None, &Language::Unknown);

        assert!(output.contains("more lines"));
        assert!(
            output.lines().count() < 260,
            "default compact read should stay bounded, got {} lines",
            output.lines().count()
        );
    }

    #[test]
    fn test_adaptive_window_keeps_small_files_complete() {
        let input = (0..100)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            apply_default_line_window(&input, FilterLevel::Minimal, None, None, &Language::Rust),
            input
        );
    }

    #[test]
    fn test_adaptive_window_is_tighter_for_large_data_files() {
        let input = (0..500)
            .map(|i| format!("key{i}=value"))
            .collect::<Vec<_>>()
            .join("\n");
        let output =
            apply_default_line_window(&input, FilterLevel::Minimal, None, None, &Language::Data);
        assert!(output.lines().count() <= DATA_COMPACT_MAX_LINES + 1);
    }

    #[test]
    fn test_extract_symbol_context_omits_unrelated_regions() {
        let input = (0..100)
            .map(|i| {
                if i == 70 {
                    "fn wanted() {}".to_string()
                } else {
                    format!("line {i}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let output = extract_symbol_context(&input, "wanted");
        assert!(output.contains("fn wanted() {}"));
        assert!(!output.contains("line 1\n"));
        assert!(output.lines().count() <= SYMBOL_CONTEXT_LINES * 2 + 1);
    }

    #[test]
    fn test_default_compact_window_does_not_truncate_raw_reads() {
        let input = (0..500)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");

        let output =
            apply_default_line_window(&input, FilterLevel::None, None, None, &Language::Unknown);

        assert_eq!(output, input);
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
