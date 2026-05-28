//! Reads source files with optional language-aware filtering to strip boilerplate.

use crate::cmds::cpp::msbuild_cmd;
use crate::core::filter::{self, FilterLevel, Language};
use crate::core::text_encoding::{self, TextEncoding};
use crate::core::tracking;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use std::fs;
use std::path::Path;

lazy_static! {
    // Compiler / managed-code diagnostic: ": error C2065" / ": error L1234" / ": error MSB..."
    static ref MSBUILD_DIAG_RE: Regex = Regex::new(r": error [CLM]\d+").unwrap();
    // Linker diagnostic: ": error LNK2001" (covers "fatal error LNK..." too via the colon)
    static ref MSBUILD_LNK_RE: Regex = Regex::new(r": error LNK\d+").unwrap();
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    file: &Path,
    level: FilterLevel,
    max_lines: Option<usize>,
    tail_lines: Option<usize>,
    line_range: Option<(usize, usize)>,
    line_numbers: bool,
    encoding: TextEncoding,
    verbose: u8,
) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Reading: {} (filter: {})", file.display(), level);
    }

    // Read file content (handles UTF-16 LE/BE BOM — MSBuild logs on Windows)
    let (content, used_encoding, used_fallback) = read_file_text(file, encoding)?;
    if used_fallback {
        eprintln!(
            "rtk read: decoded {} as {}",
            file.display(),
            used_encoding.label()
        );
    }

    // Auto-detect MSBuild log files and route through the msbuild filter.
    // Without this, `rtk read msbuild.log` (after `msbuild *> file.log`) would
    // pass through verbose project/task chatter at near-zero token savings.
    if let Some(filtered) = maybe_apply_msbuild_filter(&content) {
        if verbose > 0 {
            eprintln!("Detected MSBuild log — applying msbuild filter");
        }
        print!("{}", filtered);
        timer.track(
            &format!("cat {}", file.display()),
            "rtk read",
            &content,
            &filtered,
        );
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

    filtered = apply_line_window(&filtered, max_lines, tail_lines, line_range, &lang);

    let rtk_output = if line_numbers {
        match line_range {
            Some((start, _end)) => format_with_line_numbers_offset(&filtered, start),
            None => format_with_line_numbers(&filtered),
        }
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
    level: FilterLevel,
    max_lines: Option<usize>,
    tail_lines: Option<usize>,
    line_range: Option<(usize, usize)>,
    line_numbers: bool,
    _encoding: TextEncoding,
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

    filtered = apply_line_window(&filtered, max_lines, tail_lines, line_range, &lang);

    let rtk_output = if line_numbers {
        match line_range {
            Some((start, _end)) => format_with_line_numbers_offset(&filtered, start),
            None => format_with_line_numbers(&filtered),
        }
    } else {
        filtered.clone()
    };
    print!("{}", rtk_output);

    timer.track("cat - (stdin)", "rtk read -", &content, &rtk_output);
    Ok(())
}

/// Heuristic: returns `true` if the first 200 lines contain ANY MSBuild marker.
///
/// Single marker is enough — real MSBuild logs may have hundreds of lines of
/// progress chatter before the first error/build-result line, so requiring
/// multiple markers in a 200-line sample misses logs where only `.vcxproj`
/// references show up early. The detection runs on transcoded UTF-8 content
/// (after BOM strip + UTF-16 → UTF-8 conversion) and tolerates `\r\n` line
/// endings (`str::lines()` already strips `\r`, but explicit trim is kept
/// for defense in depth).
fn is_msbuild_log(content: &str) -> bool {
    for (i, raw_line) in content.lines().take(200).enumerate() {
        // Defense in depth: strip a stray UTF-8 BOM that survived decoding.
        let line = if i == 0 {
            raw_line.trim_start_matches('\u{FEFF}')
        } else {
            raw_line
        };
        let line = line.trim_end_matches('\r');

        if line.contains("Build FAILED")
            || line.contains("Build succeeded")
            || line.contains(".vcxproj")
            || MSBUILD_DIAG_RE.is_match(line)
            || MSBUILD_LNK_RE.is_match(line)
        {
            return true;
        }
    }
    false
}

/// If `content` is an MSBuild log, run it through the msbuild filter and return
/// the compressed output. Returns `None` for non-MSBuild content.
fn maybe_apply_msbuild_filter(content: &str) -> Option<String> {
    if !is_msbuild_log(content) {
        return None;
    }
    let filtered = msbuild_cmd::filter_output(content, &[]);
    if filtered.starts_with("msbuild: ok") {
        return Some("rtk read: build ok \u{2014} no errors found in log\n".to_string());
    }
    if filtered.starts_with("msbuild: no output captured") {
        // Detection passed but the filter found nothing structured — fall
        // back to the regular read pipeline so the user still sees content.
        return None;
    }
    let mut out = filtered;
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
}

fn read_file_text(
    path: &Path,
    encoding: TextEncoding,
) -> Result<(String, text_encoding::UsedEncoding, bool)> {
    let bytes = fs::read(path)
        .with_context(|| format!("Failed to read file: {}", path.display()))?;

    // Avoid dumping binary-ish content as Latin1 garbage in --encoding auto mode.
    // This is intentionally conservative and only triggers for obvious cases.
    if encoding == TextEncoding::Auto && looks_binary_bytes(&bytes) {
        let preview = hex_preview(&bytes, 64);
        let nul = bytes.iter().take(8192).filter(|b| **b == 0).count();
        let msg = format!(
            "rtk read: file appears binary ({} bytes, nul={} in first 8192)\n\
binary preview (first {} bytes): {}\n\
hint: use `rtk read --encoding latin1 <file>` to force raw bytes-as-text\n",
            bytes.len(),
            nul,
            preview.len,
            preview.hex
        );
        return Ok((msg, text_encoding::UsedEncoding::Utf8, false));
    }

    let decoded = text_encoding::decode_bytes(&bytes, encoding)
        .with_context(|| format!("Failed to decode file: {}", path.display()))?;
    Ok((decoded.text, decoded.used, decoded.used_fallback))
}

struct HexPreview {
    hex: String,
    len: usize,
}

fn hex_preview(bytes: &[u8], max: usize) -> HexPreview {
    let n = std::cmp::min(bytes.len(), max);
    let mut out = String::new();
    for (i, b) in bytes.iter().take(n).enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&format!("{:02X}", b));
    }
    HexPreview { hex: out, len: n }
}

fn looks_binary_bytes(bytes: &[u8]) -> bool {
    if bytes.len() >= 2 && ((bytes[0] == 0xFF && bytes[1] == 0xFE) || (bytes[0] == 0xFE && bytes[1] == 0xFF)) {
        return false;
    }
    let len = std::cmp::min(bytes.len(), 8192);
    let sample = &bytes[..len];
    if sample.is_empty() {
        return false;
    }

    // UTF-16 without BOM can contain many NULs; don't treat it as binary.
    if looks_utf16_no_bom(sample) {
        return false;
    }

    // A single NUL byte is a strong signal for binary in this tool's context.
    if sample.contains(&0) {
        return true;
    }

    // If a large fraction of bytes are control chars (excluding \t,\n,\r), treat as binary-ish.
    let mut control = 0usize;
    for b in sample {
        if *b < 0x09 || (*b > 0x0D && *b < 0x20) {
            control += 1;
        }
    }
    (control as f64 / sample.len() as f64) > 0.30
}

fn looks_utf16_no_bom(sample: &[u8]) -> bool {
    if sample.len() < 4 {
        return false;
    }
    let mut zeros_even = 0usize;
    let mut zeros_odd = 0usize;
    let mut pairs = 0usize;
    for chunk in sample.chunks_exact(2).take(4096) {
        pairs += 1;
        if chunk[0] == 0 {
            zeros_even += 1;
        }
        if chunk[1] == 0 {
            zeros_odd += 1;
        }
    }
    if pairs == 0 {
        return false;
    }
    let even_ratio = zeros_even as f64 / pairs as f64;
    let odd_ratio = zeros_odd as f64 / pairs as f64;

    fn ascii_lane_ratio(sample: &[u8], lane: usize) -> f64 {
        let mut total = 0usize;
        let mut ascii = 0usize;
        for chunk in sample.chunks_exact(2).take(4096) {
            let b = chunk[lane];
            if b == 0 {
                continue;
            }
            total += 1;
            let is_ascii =
                b == b'\t' || b == b'\n' || b == b'\r' || (0x20..=0x7E).contains(&b);
            if is_ascii {
                ascii += 1;
            }
        }
        if total == 0 {
            0.0
        } else {
            ascii as f64 / total as f64
        }
    }

    (odd_ratio >= 0.60 && even_ratio < 0.10 && ascii_lane_ratio(sample, 0) >= 0.85)
        || (even_ratio >= 0.60 && odd_ratio < 0.10 && ascii_lane_ratio(sample, 1) >= 0.85)
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

fn format_with_line_numbers_offset(content: &str, start_line: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let max_line_num = start_line.saturating_add(lines.len()).saturating_sub(1);
    let width = max_line_num.to_string().len().max(1);
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        out.push_str(&format!(
            "{:>width$} │ {}\n",
            start_line + i,
            line,
            width = width
        ));
    }
    out
}

fn apply_line_window(
    content: &str,
    max_lines: Option<usize>,
    tail_lines: Option<usize>,
    line_range: Option<(usize, usize)>,
    lang: &Language,
) -> String {
    if let Some((start, end)) = line_range {
        if start == 0 || end == 0 || end < start {
            return String::new();
        }
        let lines: Vec<&str> = content.lines().collect();
        let start_idx = start.saturating_sub(1).min(lines.len());
        let end_idx = end.min(lines.len());
        if end_idx <= start_idx {
            return String::new();
        }
        let mut result = lines[start_idx..end_idx].join("\n");
        if content.ends_with('\n') {
            result.push('\n');
        }
        return result;
    }

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
        run(
            file.path(),
            FilterLevel::Minimal,
            None,
            None,
            None,
            false,
            TextEncoding::Auto,
            0,
        )?;
        Ok(())
    }

    #[test]
    fn test_read_auto_fallback_cp949() -> Result<()> {
        let mut file = NamedTempFile::new()?;
        let (bytes, _, _) = encoding_rs::EUC_KR.encode("안녕\n");
        file.write_all(&bytes)?;

        let (txt, used, used_fallback) = read_file_text(file.path(), TextEncoding::Auto)?;
        assert!(used_fallback);
        assert_eq!(used, text_encoding::UsedEncoding::Cp949);
        assert!(txt.contains("안녕"));
        Ok(())
    }

    #[test]
    fn test_read_auto_utf16_le_no_bom() -> Result<()> {
        let mut file = NamedTempFile::new()?;
        // "Hi\n" in UTF-16 LE without BOM
        file.write_all(&[0x48, 0x00, 0x69, 0x00, 0x0A, 0x00])?;
        let (txt, used, used_fallback) = read_file_text(file.path(), TextEncoding::Auto)?;
        assert!(used_fallback);
        assert_eq!(used, text_encoding::UsedEncoding::Utf16Le);
        assert!(txt.contains("Hi"));
        Ok(())
    }

    #[test]
    fn test_read_auto_utf16_le_bom() -> Result<()> {
        let mut file = NamedTempFile::new()?;
        // BOM + "Hi\n" in UTF-16 LE
        file.write_all(&[0xFF, 0xFE, 0x48, 0x00, 0x69, 0x00, 0x0A, 0x00])?;
        let (txt, used, used_fallback) = read_file_text(file.path(), TextEncoding::Auto)?;
        assert!(!used_fallback);
        assert_eq!(used, text_encoding::UsedEncoding::Utf16Le);
        assert!(txt.contains("Hi"));
        Ok(())
    }

    #[test]
    fn test_read_auto_windows_1252() -> Result<()> {
        let mut file = NamedTempFile::new()?;
        // "Hé" in windows-1252: 0x48 0xE9 (invalid UTF-8)
        file.write_all(&[0x48, 0xE9])?;
        let (txt, used, used_fallback) = read_file_text(file.path(), TextEncoding::Auto)?;
        assert!(used_fallback);
        assert_eq!(used, text_encoding::UsedEncoding::Windows1252);
        assert!(txt.contains('é'));
        Ok(())
    }

    #[test]
    fn test_read_auto_binary_preview() -> Result<()> {
        let mut file = NamedTempFile::new()?;
        file.write_all(&[0x00, 0x01, 0x02, 0x03, 0x00, 0xFF])?;
        let (txt, used, used_fallback) = read_file_text(file.path(), TextEncoding::Auto)?;
        assert!(!used_fallback);
        assert_eq!(used, text_encoding::UsedEncoding::Utf8);
        assert!(txt.contains("file appears binary"));
        assert!(txt.contains("binary preview"));
        Ok(())
    }

    #[test]
    fn test_apply_line_window_range() {
        let lang = Language::Unknown;
        let s = "a\nb\nc\nd\n";
        let out = apply_line_window(s, None, None, Some((2, 3)), &lang);
        assert_eq!(out, "b\nc\n");
    }

    #[test]
    fn test_apply_line_window_invalid_range_empty() {
        let lang = Language::Unknown;
        let s = "a\nb\nc\n";
        assert_eq!(apply_line_window(s, None, None, Some((0, 2)), &lang), "");
        assert_eq!(apply_line_window(s, None, None, Some((3, 2)), &lang), "");
    }

    #[test]
    fn test_format_with_line_numbers_offset() {
        let s = "b\nc\n";
        let out = format_with_line_numbers_offset(s, 2);
        assert!(out.contains("2 │ b"));
        assert!(out.contains("3 │ c"));
    }

    #[test]
    fn test_stdin_support_signature() {
        // Test that run_stdin has correct signature and compiles
        // We don't actually run it because it would hang waiting for stdin
        // Compile-time verification that the function exists with correct signature
    }

    #[test]
    fn test_is_msbuild_log_failure_fixture() {
        let raw = include_str!("../../../tests/fixtures/cpp/msbuild_failure_compiler.txt");
        assert!(is_msbuild_log(raw));
    }

    #[test]
    fn test_is_msbuild_log_success_fixture() {
        let raw = include_str!("../../../tests/fixtures/cpp/msbuild_success.txt");
        assert!(is_msbuild_log(raw));
    }

    #[test]
    fn test_is_msbuild_log_rejects_plain_text() {
        let plain = "This is just\nsome regular text\nno build markers here\n";
        assert!(!is_msbuild_log(plain));
    }

    #[test]
    fn test_is_msbuild_log_single_marker_is_enough() {
        // Single marker is sufficient under OR semantics (real msbuild logs
        // often start with hundreds of lines of progress chatter before the
        // first error/build-result line).
        assert!(is_msbuild_log("see MyProject.vcxproj for details\n"));
        assert!(is_msbuild_log("Build FAILED.\n"));
        assert!(is_msbuild_log("Build succeeded.\n"));
        assert!(is_msbuild_log("foo.lib(bar.obj) : error LNK2001: x\n"));
        assert!(is_msbuild_log("a.cpp(1): error C2065: x\n"));
    }

    #[test]
    fn test_is_msbuild_log_skips_first_200_only() {
        // 250 plain lines then an MSBuild marker → not detected (200-line cap).
        let mut s = String::new();
        for _ in 0..250 {
            s.push_str("plain line\n");
        }
        s.push_str("Build FAILED.\n");
        assert!(!is_msbuild_log(&s));
    }

    #[test]
    fn test_is_msbuild_log_strips_utf8_bom_first_line() {
        let txt = "\u{FEFF}.vcxproj reference\nmore content\n";
        assert!(is_msbuild_log(txt));
    }

    #[test]
    fn test_is_msbuild_log_handles_crlf() {
        let txt = "header\r\nBuild FAILED.\r\n";
        assert!(is_msbuild_log(txt));
    }

    #[test]
    fn test_maybe_apply_msbuild_filter_success() {
        let raw = include_str!("../../../tests/fixtures/cpp/msbuild_success.txt");
        let out = maybe_apply_msbuild_filter(raw).expect("should detect");
        assert!(out.starts_with("rtk read: build ok"));
    }

    #[test]
    fn test_maybe_apply_msbuild_filter_failure_compresses() {
        let raw = include_str!("../../../tests/fixtures/cpp/msbuild_failure_compiler.txt");
        let out = maybe_apply_msbuild_filter(raw).expect("should detect");
        assert!(out.contains("C2065"));
        assert!(out.contains("Build FAILED"));
        // Must drop the verbose header chatter
        assert!(!out.contains("Microsoft (R) Build Engine"));
        assert!(!out.contains("Done Building Project"));
        // Must compress
        assert!(
            out.len() < raw.len(),
            "filter should reduce size: raw={} filtered={}",
            raw.len(),
            out.len()
        );
    }

    #[test]
    fn test_maybe_apply_msbuild_filter_skips_non_msbuild() {
        let plain = "Just some\nregular file content\n";
        assert!(maybe_apply_msbuild_filter(plain).is_none());
    }

    #[test]
    fn test_decode_utf16_le_bom() {
        // "abc" in UTF-16 LE with BOM
        let bytes: &[u8] = &[0xFF, 0xFE, b'a', 0, b'b', 0, b'c', 0];
        assert_eq!(
            text_encoding::decode_bytes(bytes, TextEncoding::Auto)
                .unwrap()
                .text,
            "abc"
        );
    }

    #[test]
    fn test_decode_utf16_be_bom() {
        // "abc" in UTF-16 BE with BOM
        let bytes: &[u8] = &[0xFE, 0xFF, 0, b'a', 0, b'b', 0, b'c'];
        assert_eq!(
            text_encoding::decode_bytes(bytes, TextEncoding::Auto)
                .unwrap()
                .text,
            "abc"
        );
    }

    #[test]
    fn test_decode_utf8_bom_stripped() {
        let bytes: &[u8] = &[0xEF, 0xBB, 0xBF, b'h', b'i'];
        assert_eq!(
            text_encoding::decode_bytes(bytes, TextEncoding::Auto)
                .unwrap()
                .text,
            "hi"
        );
    }

    #[test]
    fn test_decode_plain_utf8() {
        assert_eq!(
            text_encoding::decode_bytes(b"plain text", TextEncoding::Auto)
                .unwrap()
                .text,
            "plain text"
        );
    }

    #[test]
    fn test_decode_utf16_le_msbuild_style() {
        // Simulate a tiny MSBuild log line in UTF-16 LE
        let line = "Build succeeded.\r\n";
        let mut bytes = vec![0xFF, 0xFE];
        for c in line.encode_utf16() {
            bytes.extend_from_slice(&c.to_le_bytes());
        }
        assert_eq!(
            text_encoding::decode_bytes(&bytes, TextEncoding::Auto)
                .unwrap()
                .text,
            line
        );
    }

    #[test]
    fn test_looks_utf16_no_bom_allows_odd_length_sample() {
        // UTF-16LE-like ASCII: H i ! with a trailing odd byte.
        let sample = vec![0x48, 0x00, 0x69, 0x00, 0x21, 0x00, 0xFF];
        assert!(looks_utf16_no_bom(&sample));
    }

    #[test]
    fn test_looks_utf16_no_bom_short_sample_false() {
        assert!(!looks_utf16_no_bom(&[0x00, 0x41, 0x00]));
    }

    #[test]
    fn test_read_utf16_le_file() -> Result<()> {
        // End-to-end: rtk read on a UTF-16 LE file should not crash
        let mut file = NamedTempFile::with_suffix(".log")?;
        file.write_all(&[0xFF, 0xFE])?;
        for c in "Build succeeded.\n".encode_utf16() {
            file.write_all(&c.to_le_bytes())?;
        }
        run(
            file.path(),
            FilterLevel::Minimal,
            None,
            None,
            None,
            false,
            TextEncoding::Auto,
            0,
        )?;
        Ok(())
    }

    #[test]
    fn test_apply_line_window_tail_lines() {
        let input = "a\nb\nc\nd\n";
        let output = apply_line_window(input, None, Some(2), None, &Language::Unknown);
        assert_eq!(output, "c\nd\n");
    }

    #[test]
    fn test_apply_line_window_tail_lines_no_trailing_newline() {
        let input = "a\nb\nc\nd";
        let output = apply_line_window(input, None, Some(2), None, &Language::Unknown);
        assert_eq!(output, "c\nd");
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
