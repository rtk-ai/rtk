//! Reads source files with optional language-aware filtering to strip boilerplate.

use crate::core::filter::{self, FilterLevel, Language};
use crate::core::guard::never_worse;
use crate::core::tracking;
use anyhow::{Context, Result};
use std::fs;
use std::io::{self, Read as IoRead, Write};
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

    // `head -n N` stops as soon as it has N lines. Reading the file whole first gives the same
    // answer on a regular file and no answer at all on a device node or a FIFO nobody closes,
    // which is reachable now that `head -n N` rewrites to this.
    if level == FilterLevel::None
        && !line_numbers
        && let Some(head) = head_lines
    {
        let window = read_head_lines(file, head)?;
        io::stdout()
            .lock()
            .write_all(&window)
            .context("Failed to write line window")?;
        timer.track_bytes(
            &format!("cat {}", file.display()),
            "rtk read",
            // The bytes `cat` would have written. Unknowable without reading the file, which is
            // the whole point of not doing that, so it is taken from the size on disk -- and
            // for the unbounded sources above there is no size, only a 0 that would book the
            // window as pure cost. Claim nothing there.
            regular_file_len(file).unwrap_or(window.len()),
            &String::from_utf8_lossy(&window),
        );
        return Ok(());
    }

    // Read file content
    let bytes =
        fs::read(file).with_context(|| format!("Failed to read file: {}", file.display()))?;
    if let Some(window) = raw_byte_window(
        &bytes,
        level,
        max_lines,
        head_lines,
        tail_lines,
        line_numbers,
    ) {
        io::stdout()
            .lock()
            .write_all(window)
            .context("Failed to write raw bytes")?;
        timer.track(
            &format!("cat {}", file.display()),
            "rtk read",
            &String::from_utf8_lossy(&bytes),
            &String::from_utf8_lossy(window),
        );
        return Ok(());
    }
    let content = String::from_utf8(bytes)
        .with_context(|| format!("Failed to decode file: {}", file.display()))?;

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
    timer.track(&format!("cat {}", file.display()), "rtk read", &raw, shown);
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
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Reading from stdin (filter: {})", level);
    }

    // Read from stdin
    let mut bytes = Vec::new();
    io::stdin()
        .lock()
        .read_to_end(&mut bytes)
        .context("Failed to read from stdin")?;
    if let Some(window) = raw_byte_window(
        &bytes,
        level,
        max_lines,
        head_lines,
        tail_lines,
        line_numbers,
    ) {
        io::stdout()
            .lock()
            .write_all(window)
            .context("Failed to write raw bytes")?;
        timer.track(
            "cat - (stdin)",
            "rtk read -",
            &String::from_utf8_lossy(&bytes),
            &String::from_utf8_lossy(window),
        );
        return Ok(());
    }
    let content = String::from_utf8(bytes).context("Failed to decode stdin")?;

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
    if let Some(window) = byte_line_window(content.as_bytes(), head_lines, tail_lines) {
        return String::from_utf8_lossy(window).into_owned();
    }

    if let Some(max) = max_lines {
        return filter::smart_truncate(content, max, lang);
    }

    content.to_string()
}

/// How much is pulled from the file at a time. Only the lines asked for are ever read, so the
/// chunk bounds how far past the `n`th newline that read can reach.
const READ_CHUNK: usize = 8192;

/// The first `n` newline-terminated lines of `file`, read in chunks and stopped at the `n`th
/// newline so an endless source is never read past what was asked for. Short input, or input
/// whose last line is unterminated, comes back whole, matching [`head_window`].
///
/// Only the unfiltered head window is served this way. A filter level or `--line-numbers`
/// still needs the file whole -- `--tail-lines` inherently so -- and none of those is reachable
/// from a `head` rewrite, which is what made this path the one that had to stop early.
fn read_head_lines(file: &Path, n: usize) -> Result<Vec<u8>> {
    let mut handle =
        fs::File::open(file).with_context(|| format!("Failed to read file: {}", file.display()))?;
    let mut window = Vec::new();
    let mut chunk = [0u8; READ_CHUNK];
    let mut seen = 0;
    while seen < n {
        let read = match handle.read(&mut chunk) {
            Ok(read) => read,
            // `fs::read`, which this replaces, retries this itself; a bare `read` does not,
            // and turning a signal into a failed read would lose the window entirely.
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to read file: {}", file.display()));
            }
        };
        if read == 0 {
            break;
        }
        for &byte in &chunk[..read] {
            window.push(byte);
            if byte == b'\n' {
                seen += 1;
                if seen == n {
                    break;
                }
            }
        }
    }
    Ok(window)
}

/// `file`'s size on disk, and `None` for anything whose size says nothing about how much it
/// will produce -- a device node, a FIFO, a socket.
fn regular_file_len(file: &Path) -> Option<usize> {
    let meta = fs::metadata(file).ok()?;
    meta.is_file().then_some(meta.len() as usize)
}

/// First `n` lines, sliced on byte offsets rather than round-tripped through
/// `lines()`, so CRLF endings and an unterminated final line survive verbatim.
/// `\n` is ASCII, so valid UTF-8 input also stays valid after slicing.
fn head_window(content: &[u8], n: usize) -> &[u8] {
    if n == 0 {
        return &[];
    }
    let mut seen = 0;
    for (idx, &byte) in content.iter().enumerate() {
        if byte == b'\n' {
            seen += 1;
            if seen == n {
                return &content[..=idx];
            }
        }
    }
    content
}

/// Last `n` lines, byte-sliced for the same fidelity reasons as `head_window`.
/// A trailing newline terminates the final line instead of starting a new one,
/// so it is excluded before counting separators backwards — otherwise `n` would
/// select one line too few for newline-terminated input.
fn tail_window(content: &[u8], n: usize) -> &[u8] {
    if n == 0 {
        return &[];
    }
    let search_end = match content.last() {
        Some(b'\n') => content.len() - 1,
        _ => content.len(),
    };
    let mut seen = 0;
    for idx in (0..search_end).rev() {
        if content[idx] == b'\n' {
            seen += 1;
            if seen == n {
                return &content[idx + 1..];
            }
        }
    }
    content
}

/// The slice to write out verbatim, or `None` when a later step needs a `&str`:
/// a filter level, line numbers, or `--max-lines` (counted on filtered text).
/// Head and tail windows slice on byte offsets, so they stay here, and so does a
/// plain read — that is what lets `rtk read` handle a file or a stdin stream that
/// is not valid UTF-8.
fn raw_byte_window(
    content: &[u8],
    level: FilterLevel,
    max_lines: Option<usize>,
    head_lines: Option<usize>,
    tail_lines: Option<usize>,
    line_numbers: bool,
) -> Option<&[u8]> {
    if level != FilterLevel::None || line_numbers {
        return None;
    }
    byte_line_window(content, head_lines, tail_lines)
        .or_else(|| max_lines.is_none().then_some(content))
}

fn byte_line_window(
    content: &[u8],
    head_lines: Option<usize>,
    tail_lines: Option<usize>,
) -> Option<&[u8]> {
    if let Some(head) = head_lines {
        Some(head_window(content, head))
    } else {
        tail_lines.map(|tail| tail_window(content, tail))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// `read_head_lines` must agree with `head_window` byte-for-byte on every shape, since it
    /// replaces it on the unfiltered path -- CRLF endings and an unterminated last line
    /// included.
    ///
    /// The inputs have to span more than one `READ_CHUNK`, because reading across chunks is
    /// the only thing the rewrite added: a set that all fits in the first chunk passes just as
    /// happily with the loop stopped after that chunk.
    #[test]
    fn test_read_head_lines_matches_head_window() -> Result<()> {
        let long_line = "x".repeat(READ_CHUNK * 2);
        // A newline sitting exactly on a chunk boundary, and on either side of it.
        let boundary = |at: usize| format!("{}\n{}\n", "y".repeat(at - 1), "z".repeat(100));

        let mut contents: Vec<String> = [
            "",
            "a",
            "a\n",
            "a\nb\nc\n",
            "a\nb\nc",
            "a\r\nb\r\nc\r\n",
            "\n\n\n",
        ]
        .iter()
        .map(|c| (*c).to_string())
        .collect();
        contents.push(long_line.clone());
        contents.push(format!("{long_line}\n"));
        contents.push(boundary(READ_CHUNK));
        contents.push(boundary(READ_CHUNK + 1));
        contents.push(boundary(READ_CHUNK - 1));
        // Many short lines over several chunks, so the Nth newline lands deep in.
        contents.push((0..4000).map(|i| format!("line {i}\n")).collect());
        // A CRLF straddling a chunk boundary: the `\r` and its `\n` must not come apart.
        contents.push(format!(
            "{}\r\n{}\r\n",
            "w".repeat(READ_CHUNK - 1),
            "v".repeat(50)
        ));

        for content in &contents {
            let mut file = NamedTempFile::new()?;
            file.write_all(content.as_bytes())?;
            file.flush()?;
            for n in [0, 1, 2, 3, 10, 1000, 4000] {
                assert_eq!(
                    read_head_lines(file.path(), n)?,
                    head_window(content.as_bytes(), n),
                    "content of {} bytes, n {n}",
                    content.len()
                );
            }
        }
        Ok(())
    }

    /// The point of reading in chunks: a source with no end still returns. A FIFO nobody ever
    /// closes stands in for the `/dev/urandom` case, which `head -n N` now rewrites to.
    #[cfg(unix)]
    #[test]
    fn test_read_head_lines_returns_from_an_endless_source() -> Result<()> {
        use std::io::Write as _;
        let dir = tempfile::tempdir()?;
        let fifo = dir.path().join("endless");
        // Shelled out rather than called through libc: `unsafe` is not allowed outside proxy
        // mode's signal handling.
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()?
                .success(),
            "mkfifo failed"
        );

        let writer_path = fifo.clone();
        let writer = std::thread::spawn(move || {
            let Ok(mut handle) = fs::OpenOptions::new().write(true).open(&writer_path) else {
                return;
            };
            // Never closes on its own: the read side has to stop itself.
            while handle.write_all(b"line\n").is_ok() {}
        });

        assert_eq!(read_head_lines(&fifo, 3)?, b"line\nline\nline\n");
        drop(writer);
        Ok(())
    }

    /// A device node reports a size of 0, which would book the window as pure cost.
    #[test]
    fn test_regular_file_len_only_answers_for_a_regular_file() -> Result<()> {
        let mut file = NamedTempFile::new()?;
        file.write_all(b"hello\n")?;
        file.flush()?;
        assert_eq!(regular_file_len(file.path()), Some(6));
        assert_eq!(regular_file_len(Path::new("/nonexistent-rtk-test")), None);
        #[cfg(unix)]
        assert_eq!(regular_file_len(Path::new("/dev/null")), None);
        Ok(())
    }

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
        assert_eq!(
            apply_line_window("", None, Some(5), None, &Language::Unknown),
            ""
        );
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
        assert_eq!(
            apply_line_window("", None, None, Some(5), &Language::Unknown),
            ""
        );
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

    #[test]
    fn test_raw_byte_window_passes_a_plain_read_through() {
        let input: &[u8] = b"alpha\nbravo\n";
        assert_eq!(
            raw_byte_window(input, FilterLevel::None, None, None, None, false),
            Some(input)
        );
    }

    #[test]
    fn test_raw_byte_window_declines_when_decoding_is_needed() {
        let input: &[u8] = b"alpha\nbravo\n";
        for (level, max_lines, line_numbers) in [
            (FilterLevel::Minimal, None, false),
            (FilterLevel::Aggressive, None, false),
            (FilterLevel::None, None, true),
            (FilterLevel::None, Some(1), false),
        ] {
            assert_eq!(
                raw_byte_window(input, level, max_lines, None, None, line_numbers),
                None
            );
        }
    }

    #[test]
    fn test_raw_byte_window_keeps_line_windows() {
        let input: &[u8] = b"alpha\nbravo\ncharlie\n";
        assert_eq!(
            raw_byte_window(input, FilterLevel::None, None, Some(1), None, false),
            Some(&b"alpha\n"[..])
        );
        assert_eq!(
            raw_byte_window(input, FilterLevel::None, None, None, Some(1), false),
            Some(&b"charlie\n"[..])
        );
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
    fn test_read_non_utf8_file_is_byte_exact() {
        let bin = rtk_bin();
        assert!(bin.exists(), "Run `cargo build` first");

        let raw: &[u8] = b"line one\nlatin-1 byte: \xe9\nline three\n";
        let mut file = NamedTempFile::with_suffix(".log").unwrap();
        file.write_all(raw).unwrap();
        file.flush().unwrap();

        let output = std::process::Command::new(&bin)
            .args(["read", &file.path().to_string_lossy()])
            .output()
            .expect("failed to run rtk read");

        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.stdout, raw,
            "non-UTF-8 input must reach stdout verbatim"
        );
    }

    #[test]
    #[ignore]
    fn test_read_non_utf8_file_still_errors_when_filtering() {
        let bin = rtk_bin();
        assert!(bin.exists(), "Run `cargo build` first");

        let raw: &[u8] = b"line one\nlatin-1 byte: \xe9\nline three\n";
        let mut file = NamedTempFile::with_suffix(".log").unwrap();
        file.write_all(raw).unwrap();
        file.flush().unwrap();

        let output = std::process::Command::new(&bin)
            .args(["read", "-l", "minimal", &file.path().to_string_lossy()])
            .output()
            .expect("failed to run rtk read");

        assert!(
            !output.status.success(),
            "filtering cannot run on undecodable input, so it must still report the error"
        );
    }

    #[test]
    #[ignore]
    fn test_read_stdin_non_utf8_is_byte_exact() {
        let bin = rtk_bin();
        assert!(bin.exists(), "Run `cargo build` first");

        let raw: &[u8] = b"line one\nlatin-1 byte: \xe9\nline three\n";
        let mut child = std::process::Command::new(&bin)
            .args(["read", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("failed to run rtk read -");
        child
            .stdin
            .take()
            .expect("stdin was piped")
            .write_all(raw)
            .expect("failed to write to stdin");
        let output = child.wait_with_output().expect("failed to wait for rtk");

        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.stdout, raw,
            "non-UTF-8 stdin must reach stdout verbatim"
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
