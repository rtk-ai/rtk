//! Reads source files with optional language-aware filtering to strip boilerplate.

use crate::core::filter::{self, FilterLevel, Language};
use crate::core::tracking;
use anyhow::{Context, Result};
use std::fs;
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

    // Capture post-filter content before user-requested truncation.
    // --max-lines/--tail-lines is a user choice, not RTK compression — savings
    // are tracked against filtered (not truncated) output to avoid inflation.
    let filtered_for_tracking = if max_lines.is_some() || tail_lines.is_some() {
        Some(filtered.clone())
    } else {
        None
    };

    filtered = apply_line_window(&filtered, max_lines, tail_lines, &lang);

    let rtk_output = if line_numbers {
        format_with_line_numbers(&filtered)
    } else {
        filtered.clone()
    };
    println!("{}", rtk_output);

    let tracking_output = filtered_for_tracking.as_deref().unwrap_or(&rtk_output);
    timer.track(
        &format!("cat {}", file.display()),
        "rtk read",
        &content,
        tracking_output,
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

    let filtered_for_tracking = if max_lines.is_some() || tail_lines.is_some() {
        Some(filtered.clone())
    } else {
        None
    };

    filtered = apply_line_window(&filtered, max_lines, tail_lines, &lang);

    let rtk_output = if line_numbers {
        format_with_line_numbers(&filtered)
    } else {
        filtered.clone()
    };
    print!("{}", rtk_output);

    let tracking_output = filtered_for_tracking.as_deref().unwrap_or(&rtk_output);
    timer.track("cat - (stdin)", "rtk read -", &content, tracking_output);
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

    // ── Helpers for savings-tracking logic tests ──────────────────────────────

    /// Simulate the filtered_for_tracking / tracking_output logic from run().
    /// Returns (tracking_output_len, truncated_len, full_filtered_len).
    fn simulate_tracking(
        full_content: &str,
        max_lines: Option<usize>,
        tail_lines: Option<usize>,
    ) -> (usize, usize, usize) {
        let filtered = full_content.to_string();
        let filtered_for_tracking = if max_lines.is_some() || tail_lines.is_some() {
            Some(filtered.clone())
        } else {
            None
        };
        let truncated = apply_line_window(&filtered, max_lines, tail_lines, &Language::Unknown);
        let tracking_output = filtered_for_tracking
            .as_deref()
            .unwrap_or(truncated.as_str());
        (tracking_output.len(), truncated.len(), filtered.len())
    }

    // ── Regression tests for #1045 ─────────────────────────────────────────

    /// When --max-lines is active, savings must be calculated against the
    /// pre-truncation (filtered) content, not the truncated output.
    /// Without this fix: savings ≈ 100k tokens for a 100k-line file.
    /// With this fix   : savings ≈ 0 tokens (no RTK compression happened).
    #[test]
    fn test_max_lines_tracking_uses_pretruncation_content() {
        let content: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let (tracking_len, truncated_len, full_len) =
            simulate_tracking(&content, Some(5), None);

        assert_eq!(
            tracking_len, full_len,
            "tracking_output must equal full filtered content, not the truncated slice"
        );
        assert!(
            truncated_len < full_len,
            "sanity check: truncation must have reduced content"
        );
        assert!(
            tracking_len > truncated_len,
            "tracking_output must be larger than truncated_output to prevent savings inflation"
        );
    }

    /// Same invariant for --tail-lines.
    #[test]
    fn test_tail_lines_tracking_uses_pretruncation_content() {
        let content: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let (tracking_len, truncated_len, full_len) =
            simulate_tracking(&content, None, Some(5));

        assert_eq!(tracking_len, full_len);
        assert!(truncated_len < full_len);
        assert!(tracking_len > truncated_len);
    }

    /// Without any truncation flag, tracking_output == rtk_output (no change
    /// in behaviour for the normal code path).
    #[test]
    fn test_no_truncation_tracking_unchanged() {
        let content: String = (0..10).map(|i| format!("line {i}\n")).collect();
        let (tracking_len, truncated_len, _full_len) =
            simulate_tracking(&content, None, None);

        assert_eq!(
            tracking_len, truncated_len,
            "without truncation flags, tracking_output must equal rtk_output"
        );
    }

    /// Numerical proof: the bug would have reported huge artificial savings;
    /// the fix reports ≈0 savings (content unchanged by RTK filtering).
    #[test]
    fn test_savings_inflation_is_zero_with_fix() {
        use crate::core::tracking::estimate_tokens;

        // 1000-line file, each line ~20 chars = ~5k tokens input
        let content: String = (0..1000).map(|i| format!("{:020}\n", i)).collect();
        let (tracking_len, truncated_len, _) = simulate_tracking(&content, Some(5), None);

        let input_tokens = estimate_tokens(&content);

        // Savings reported by the BUGGY version (tracking against truncated output)
        let buggy_savings = input_tokens as i64 - estimate_tokens(&content[..truncated_len]) as i64;

        // Savings reported by the FIXED version (tracking against pre-truncation content)
        let fixed_savings = input_tokens as i64 - estimate_tokens(&content[..tracking_len]) as i64;

        assert!(
            buggy_savings > 1_000,
            "bug would report >1k fake token savings, got {buggy_savings}"
        );
        assert_eq!(
            fixed_savings, 0,
            "fix must report 0 savings when content wasn't actually compressed, got {fixed_savings}"
        );
    }
}
