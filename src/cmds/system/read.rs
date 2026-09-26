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
        let read_context = || format!("Failed to read file: {}", file.display());
        let mut source = fs::File::open(file).with_context(read_context)?;
        // The window is written as it is read, so what it holds and what reached stdout are
        // the same bytes.
        let window = copy_head_lines(&mut source, head, &mut io::stdout().lock(), read_context)?;
        timer.track_bytes(
            &window_label(Some(head), None, &file.display().to_string()),
            "rtk read",
            window,
            window,
        );
        return Ok(());
    }

    // Read file content
    let bytes =
        fs::read(file).with_context(|| format!("Failed to read file: {}", file.display()))?;
    if level == FilterLevel::None
        && !line_numbers
        && let Some(window) = byte_line_window(&bytes, head_lines, tail_lines)
    {
        io::stdout()
            .lock()
            .write_all(window)
            .context("Failed to write line window")?;
        timer.track_bytes(
            &window_label(head_lines, tail_lines, &file.display().to_string()),
            "rtk read",
            window.len(),
            window.len(),
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
    let (label, baseline) = window_baseline(
        level,
        head_lines,
        tail_lines,
        &file.display().to_string(),
        shown,
    )
    .unwrap_or_else(|| (format!("cat {}", file.display()), raw.as_str()));
    timer.track(&label, "rtk read", baseline, shown);
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

    // `head -n N` stops as soon as it has N lines. Draining stdin first gives the same answer
    // on a producer that ends and no answer at all on one that does not, so the head window is
    // taken straight off the stream.
    if level == FilterLevel::None
        && !line_numbers
        && let Some(head) = head_lines
    {
        let window = copy_head_lines(
            &mut io::stdin().lock(),
            head,
            &mut io::stdout().lock(),
            || "Failed to read from stdin".to_string(),
        )?;
        timer.track_bytes(
            &window_label(Some(head), None, "-"),
            "rtk read -",
            window,
            window,
        );
        return Ok(());
    }

    // Read from stdin
    let mut bytes = Vec::new();
    io::stdin()
        .lock()
        .read_to_end(&mut bytes)
        .context("Failed to read from stdin")?;
    if level == FilterLevel::None
        && !line_numbers
        && let Some(window) = byte_line_window(&bytes, head_lines, tail_lines)
    {
        io::stdout()
            .lock()
            .write_all(window)
            .context("Failed to write line window")?;
        timer.track_bytes(
            &window_label(head_lines, tail_lines, "-"),
            "rtk read -",
            window.len(),
            window.len(),
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

    let (label, baseline) = window_baseline(level, head_lines, tail_lines, "-", shown)
        .unwrap_or_else(|| ("cat - (stdin)".to_string(), raw.as_str()));
    timer.track(&label, "rtk read -", baseline, shown);
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

/// How much is pulled from the source at a time, which bounds both the memory a window of any
/// shape costs and how much of the source is consumed reaching the `n`th newline -- for a pipe
/// or a seekable stdin, how much a later reader loses. On unix that second bound is exact:
/// `StdinLock`'s buffer has the same capacity, and a read that large is handed straight to the
/// OS instead of going through it. On Windows its buffer holds 12 KiB, so a read can fill that
/// much instead.
const READ_CHUNK: usize = 8192;

/// Where the first `n` newline-terminated lines end in `bytes`, and how many of their
/// terminators that prefix holds: the whole slice, and a count short of `n`, when there are
/// fewer lines than asked for.
///
/// This is the only place the head window is defined. [`head_window`] applies it once to bytes
/// already in memory; [`copy_head_lines`] applies it to each chunk with a running count, which
/// is what lets it stop at the `n`th newline instead of draining the source. Scanning for line
/// ends twice would be two places for CRLF endings and unterminated last lines to drift apart.
fn head_prefix(bytes: &[u8], n: usize) -> (usize, usize) {
    if n == 0 {
        return (0, 0);
    }
    let mut seen = 0;
    for (idx, &byte) in bytes.iter().enumerate() {
        if byte == b'\n' {
            seen += 1;
            if seen == n {
                return (idx + 1, seen);
            }
        }
    }
    (bytes.len(), seen)
}

/// Copies the first `n` newline-terminated lines of `source` to `out`, and reports how many
/// bytes that was. Reading stops at the `n`th newline rather than at the source's end, so a
/// source that never ends still returns -- the chunk that newline arrived in is read whole and
/// no further -- and each chunk is written as it is read, so a line of any length costs one
/// chunk of memory rather than its own length. Short input, or input whose last line is
/// unterminated, comes through whole, matching [`head_window`].
///
/// Only the unfiltered head window is served this way: a filter level or `--line-numbers`
/// needs the input whole, and `--tail-lines` inherently so.
fn copy_head_lines(
    source: &mut impl IoRead,
    n: usize,
    out: &mut impl Write,
    read_context: impl Fn() -> String,
) -> Result<usize> {
    let mut chunk = [0u8; READ_CHUNK];
    let mut seen = 0;
    let mut written = 0;
    while seen < n {
        let read = match source.read(&mut chunk) {
            Ok(read) => read,
            // A bare `read` does not retry after a signal, and turning one into a failed
            // read would lose the window entirely.
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error).with_context(read_context),
        };
        if read == 0 {
            break;
        }
        let (taken, found) = head_prefix(&chunk[..read], n - seen);
        out.write_all(&chunk[..taken])
            .context("Failed to write line window")?;
        written += taken;
        seen += found;
    }
    Ok(written)
}

/// The command a window the user asked for stands in for, and the row's name for it. The hook
/// rewrites `head -n N <file>` to `rtk read <file> --head-lines N`, and RTK prints the window
/// that command prints, so the row is measured against that window and not against the rest of
/// the file: crediting RTK with bytes the user never asked for would be crediting it with
/// their own choice of command. Every path here prints the window whole, so every such row
/// books nothing, which is what the rewrite saves.
///
/// A filter level does not come through here: cutting the input down is RTK's own doing, so
/// such a read stands in for `cat` and is measured against the whole of it.
fn window_label(head_lines: Option<usize>, tail_lines: Option<usize>, source: &str) -> String {
    match (head_lines, tail_lines) {
        (Some(n), _) => format!("head -n {n} {source}"),
        (None, Some(n)) => format!("tail -n {n} {source}"),
        (None, None) => format!("cat {source}"),
    }
}

/// How a read that went through the filter pipeline is recorded, when a window is all the
/// user asked RTK to do. `--line-numbers` renders that window rather than printing it, but
/// they asked for the numbering too, so the row is measured against what was shown and books
/// nothing, exactly as the byte-window paths do. `None` for everything else: cutting the input
/// down is RTK's own doing, and the caller keeps its own `cat` baseline for that.
fn window_baseline<'a>(
    level: FilterLevel,
    head_lines: Option<usize>,
    tail_lines: Option<usize>,
    source: &str,
    shown: &'a str,
) -> Option<(String, &'a str)> {
    let window_only = level == FilterLevel::None && (head_lines.is_some() || tail_lines.is_some());
    window_only.then(|| (window_label(head_lines, tail_lines, source), shown))
}

/// First `n` lines, sliced on byte offsets rather than round-tripped through
/// `lines()`, so CRLF endings and an unterminated final line survive verbatim.
/// `\n` is ASCII, so valid UTF-8 input also stays valid after slicing.
fn head_window(content: &[u8], n: usize) -> &[u8] {
    &content[..head_prefix(content, n).0]
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

    /// The context a failing read would carry; the tests never make one fail.
    fn context() -> String {
        "test source".to_string()
    }

    /// [`copy_head_lines`] into a buffer, for assertions that compare whole windows. The
    /// reported size is what tracking books, so it is pinned to what was actually written.
    fn head_lines_of(source: &mut impl IoRead, n: usize) -> Result<Vec<u8>> {
        let mut window = Vec::new();
        let reported = copy_head_lines(source, n, &mut window, context)?;
        assert_eq!(
            reported,
            window.len(),
            "reported size must match the bytes written"
        );
        Ok(window)
    }

    /// A second, independent definition of the head window: one pass over bytes already in
    /// memory. Both ways of reaching the window are asserted against this rather than against
    /// each other, so a shared rule that drifts is still caught.
    fn reference_head_window(content: &[u8], n: usize) -> &[u8] {
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

    /// `copy_head_lines` and `head_window` must both reproduce `reference_head_window`
    /// byte-for-byte on every shape -- CRLF endings and an unterminated last line included --
    /// since between them they serve every unfiltered head window RTK emits.
    ///
    /// The inputs have to span more than one `READ_CHUNK`, because reading across chunks is
    /// the only thing the reader adds: a set that all fits in the first chunk passes just as
    /// happily with the loop stopped after that chunk.
    #[test]
    fn test_head_windows_match_the_reference() -> Result<()> {
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
                let expected = reference_head_window(content.as_bytes(), n);
                assert_eq!(
                    head_lines_of(&mut fs::File::open(file.path())?, n)?,
                    expected,
                    "copy_head_lines, content of {} bytes, n {n}",
                    content.len()
                );
                assert_eq!(
                    head_window(content.as_bytes(), n),
                    expected,
                    "head_window, content of {} bytes, n {n}",
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
    fn test_copy_head_lines_returns_from_an_endless_source() -> Result<()> {
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

        assert_eq!(
            head_lines_of(&mut fs::File::open(&fifo)?, 3)?,
            b"line\nline\nline\n"
        );
        drop(writer);
        Ok(())
    }

    /// What a pipe looks like and a file does not: a few bytes per read, never any end, line
    /// endings split across reads. A source like this is the one the stdin path has to stop
    /// itself on, and `served` pins that it stops without draining -- at the cost of the read
    /// already in flight, and no more.
    #[test]
    fn test_copy_head_lines_stops_on_a_trickling_endless_source() -> Result<()> {
        const READ: usize = 5;
        struct Trickle {
            at: usize,
            served: usize,
        }
        impl IoRead for Trickle {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let take = buf.len().min(READ);
                for slot in &mut buf[..take] {
                    *slot = b"ab\r\n"[self.at % 4];
                    self.at += 1;
                }
                self.served += take;
                Ok(take)
            }
        }

        // The fourth line's terminator starts in one read and ends in the next.
        let expected = b"ab\r\nab\r\nab\r\nab\r\n";
        let mut source = Trickle { at: 0, served: 0 };
        assert_eq!(head_lines_of(&mut source, 4)?, expected);
        // A read already in flight when the `n`th newline arrives still delivers its chunk.
        assert!(
            source.served < expected.len() + READ,
            "consumed {} bytes for a {}-byte window",
            source.served,
            expected.len()
        );

        let mut untouched = Trickle { at: 0, served: 0 };
        assert_eq!(head_lines_of(&mut untouched, 0)?, b"");
        assert_eq!(untouched.served, 0, "n = 0 must not read at all");
        Ok(())
    }

    /// The row has to name the command RTK stands in for, because that is what its numbers
    /// are measured against.
    #[test]
    fn test_window_label_names_the_command_it_stands_in_for() {
        assert_eq!(window_label(Some(5), None, "big.txt"), "head -n 5 big.txt");
        assert_eq!(window_label(None, Some(5), "big.txt"), "tail -n 5 big.txt");
        assert_eq!(window_label(Some(5), None, "-"), "head -n 5 -");
        assert_eq!(window_label(None, None, "big.txt"), "cat big.txt");
        // `byte_line_window` serves the head window when both are given, so the label has to
        // name the same one.
        assert_eq!(
            window_label(Some(5), Some(9), "big.txt"),
            "head -n 5 big.txt"
        );
    }

    /// A signal is not the end of the input. Reading it as one truncates the window, which
    /// no output assertion elsewhere would notice, since the source has more to give.
    #[test]
    fn test_copy_head_lines_retries_after_a_signal() -> Result<()> {
        struct InterruptFirst<'a> {
            interrupted: bool,
            rest: &'a [u8],
        }
        impl IoRead for InterruptFirst<'_> {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if !self.interrupted {
                    self.interrupted = true;
                    return Err(io::Error::from(io::ErrorKind::Interrupted));
                }
                self.rest.read(buf)
            }
        }

        let mut source = InterruptFirst {
            interrupted: false,
            rest: b"a\nb\n",
        };
        assert_eq!(head_lines_of(&mut source, 2)?, b"a\nb\n");
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
