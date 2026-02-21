//! Streaming process execution infrastructure for RTK.
//!
//! Provides a bidirectional process shim that preserves all process state
//! (exit codes, signals, SIGPIPE) while filtering stdout progressively.
//!
//! # Process Transparency
//! RTK inserts itself between a subprocess and the OS environment.
//! All process expectations are preserved:
//! - Exit codes 0–254 propagated exactly via [`StreamResult::exit_code`].
//! - Signal-killed exit becomes `128 + signal_num` per POSIX convention.
//! - SIGPIPE (broken downstream pipe) breaks the output loop cleanly.
//! - SIGINT reaches both RTK and child via shared process group.
//! - stdin inherited by default (`StdinMode::Inherit`).
//!
//! # RAII Guarantees
//! Three resources follow RAII patterns (mirrors `RtkActiveGuard` in cmd/exec.rs):
//! - `ChildGuard`: ensures `wait()` is called even on early `?` returns → no zombies.
//! - `io::stdout().lock()`: released in scoped block before any joins.
//! - Stdin `JoinHandle`: stored and joined (not detached) to surface panics.

use anyhow::{Context, Result};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::process::{Command, Stdio};

// ─── Traits ──────────────────────────────────────────────────────────────────

/// A filter that processes command output incrementally, line by line.
///
/// Implement this to stream-filter subprocess output as it's produced,
/// rather than buffering the entire output first.
pub trait StreamFilter {
    /// Process one line from subprocess stdout.
    /// Returns `Some(output)` to emit that text downstream, `None` to suppress.
    /// The output string should include a trailing newline if needed.
    fn feed_line(&mut self, line: &str) -> Option<String>;

    /// Called at end-of-stream to flush any buffered state.
    /// Returns any final output (e.g., a summary block).
    fn flush(&mut self) -> String;
}

/// A filter for stdin transformation. Requires [`Send`] because it runs in a thread.
pub trait StdinFilter: Send {
    /// Transform one line of stdin before forwarding to the child process.
    fn feed_line(&mut self, line: &str) -> Option<String>;

    /// Flush any remaining buffered state at stdin EOF.
    fn flush(&mut self) -> String;
}

// ─── LineFilter ──────────────────────────────────────────────────────────────

/// Generic per-line stateless filter adapter (Category A filters).
///
/// Wraps a closure for simple line-by-line transformations without state.
/// The closure receives each line (without trailing newline) and returns
/// `Some(output)` to emit or `None` to suppress.
///
/// # Example
/// ```rust,ignore
/// let f = LineFilter::new(|l| Some(format!("{}\n", l.to_uppercase())));
/// ```
pub struct LineFilter<F: FnMut(&str) -> Option<String>> {
    f: F,
}

impl<F: FnMut(&str) -> Option<String>> LineFilter<F> {
    pub fn new(f: F) -> Self {
        Self { f }
    }
}

impl<F: FnMut(&str) -> Option<String>> StreamFilter for LineFilter<F> {
    fn feed_line(&mut self, line: &str) -> Option<String> {
        (self.f)(line)
    }

    fn flush(&mut self) -> String {
        String::new()
    }
}

// ─── FilterMode / StdinMode ──────────────────────────────────────────────────

/// How subprocess stdout is processed.
pub enum FilterMode {
    /// Line-by-line filtering, emitting output progressively as the subprocess produces it.
    /// Use for commands with predictable per-line output (go test NDJSON, cargo test, etc.).
    Streaming(Box<dyn StreamFilter>),

    /// Buffer all stdout, apply a function, emit at end.
    /// Use only for filters that genuinely require complete input (e.g., full JSON doc).
    Buffered(fn(&str) -> String),

    /// Emit raw lines immediately without filtering (ANSI codes preserved).
    Passthrough,
}

/// How subprocess stdin is handled.
// Filter and Null are used in tests and reserved for bidirectional shim use.
#[allow(dead_code)]
pub enum StdinMode {
    /// Pass RTK's stdin directly to the child (default, zero overhead, no pipe).
    Inherit,

    /// Transform input lines through a filter before forwarding to child.
    /// The filter runs in a dedicated thread to prevent deadlock.
    Filter(Box<dyn StdinFilter + Send>),

    /// Immediately send EOF to child (child sees no stdin input).
    Null,
}

// ─── StreamResult ─────────────────────────────────────────────────────────────

/// Result of [`run_streaming`], carrying the full POSIX exit code.
pub struct StreamResult {
    /// POSIX exit code: 0 = success, 1–127 = app error, 128+N = killed by signal N.
    pub exit_code: i32,

    /// Raw stdout + stderr combined (capped at 1 MiB) for tee recovery.
    pub raw: String,

    /// Filtered stdout content for token savings tracking.
    pub filtered: String,
}

impl StreamResult {
    /// Returns `true` if `exit_code == 0`.
    // Used in tests and by future callers; suppress dead_code for binary target.
    #[allow(dead_code)]
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}

// ─── status_to_exit_code ──────────────────────────────────────────────────────

/// Convert `ExitStatus` to a POSIX-compatible integer exit code.
///
/// - Normal exit via `exit(N)`: returns `N`.
/// - Signal-killed (Unix): returns `128 + signal_number` per POSIX convention.
///   Example: SIGKILL (9) → 137, SIGTERM (15) → 143.
/// - Unknown state: returns `1` (generic failure fallback).
pub fn status_to_exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    // Process was killed by a signal — POSIX convention: 128 + signal_num
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    1 // fallback: unknown failure
}

// ─── run_streaming ────────────────────────────────────────────────────────────

/// Execute `cmd` as a bidirectional process shim with streaming stdout filtering.
///
/// Spawns the command and concurrently:
/// - **(stdin thread)** Optionally transforms stdin before forwarding to child.
/// - **(stderr thread)** Streams stderr directly to `io::stderr()` (responsive).
/// - **(main thread)** Reads stdout, applies `stdout_mode` filter, writes to `io::stdout()`.
///
/// # Exit Code Transparency
/// The returned [`StreamResult::exit_code`] is the child's exact POSIX exit code.
/// Callers should propagate it via `std::process::exit(result.exit_code)` if non-zero.
///
/// # SIGPIPE Handling
/// If the downstream pipe closes (e.g., `rtk go test | head -5`), writing to stdout
/// returns `BrokenPipe`. The output loop breaks cleanly, child is drained, and the
/// child's exit code is returned unchanged.
///
/// # Memory Bound
/// Raw stdout+stderr is capped at 1 MiB (matches `tee.rs` `DEFAULT_MAX_FILE_SIZE`).
/// Output beyond the cap is streamed to stdout but not accumulated in `raw`.
pub fn run_streaming(
    cmd: &mut Command,
    stdin_mode: StdinMode,
    stdout_mode: FilterMode,
) -> Result<StreamResult> {
    // ── Configure pipes ────────────────────────────────────────────────────────
    match &stdin_mode {
        StdinMode::Inherit => {
            cmd.stdin(Stdio::inherit());
        }
        StdinMode::Filter(_) | StdinMode::Null => {
            cmd.stdin(Stdio::piped());
        }
    }
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    // ── RAII child guard ──────────────────────────────────────────────────────
    // Nested type definition (valid in Rust). Mirrors `RtkActiveGuard` in
    // src/cmd/exec.rs:14-28. Ensures child.wait() is always called, preventing zombies.
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            // Reap zombie. Ignores ECHILD error when child was already waited explicitly.
            self.0.wait().ok();
        }
    }

    let mut child = ChildGuard(cmd.spawn().context("Failed to spawn process")?);

    // ── Stdin thread ──────────────────────────────────────────────────────────
    // JoinHandle stored (not detached) — avoids clippy `unused_must_use` warning
    // and ensures panics in the stdin thread are not silently lost.
    let stdin_thread: Option<std::thread::JoinHandle<()>> = match stdin_mode {
        StdinMode::Filter(mut filter) => {
            let child_stdin = child.0.stdin.take().context("No child stdin handle")?;
            Some(std::thread::spawn(move || {
                let mut writer = BufWriter::new(child_stdin);
                // Bind stdin to variable first — avoids temporary lifetime issue with lock().
                let stdin_handle = io::stdin();
                for line in BufReader::new(stdin_handle.lock())
                    .lines()
                    .filter_map(Result::ok)
                {
                    if let Some(out) = filter.feed_line(&line) {
                        if writeln!(writer, "{}", out).is_err() {
                            break; // child closed its stdin — stop sending
                        }
                    }
                }
                let tail = filter.flush();
                if !tail.is_empty() {
                    write!(writer, "{}", tail).ok();
                }
                // writer drop → BufWriter flushes → ChildStdin drops → EOF to child
            }))
        }
        StdinMode::Null => {
            child.0.stdin.take(); // drop ChildStdin immediately → child gets EOF
            None
        }
        StdinMode::Inherit => None, // stdin already configured as Stdio::inherit()
    };

    // ── Stderr thread ─────────────────────────────────────────────────────────
    // Streams stderr directly to io::stderr() (responsive, not buffered).
    // Accumulates raw string for tee recovery.
    let stderr = child.0.stderr.take().context("No child stderr handle")?;
    let stderr_thread = std::thread::spawn(move || -> String {
        let mut raw_err = String::new();
        let stderr_out = io::stderr();
        let mut err_out = stderr_out.lock(); // RAII: released when closure completes
        for line in BufReader::new(stderr).lines().filter_map(Result::ok) {
            writeln!(err_out, "{}", line).ok(); // emit immediately (responsive)
            raw_err.push_str(&line);
            raw_err.push('\n');
        }
        raw_err
    });

    // ── Stdout: main thread ───────────────────────────────────────────────────
    let stdout = child.0.stdout.take().context("No child stdout handle")?;
    const RAW_CAP: usize = 1_048_576; // 1 MiB, matches tee.rs DEFAULT_MAX_FILE_SIZE
    let mut raw_stdout = String::new();
    let mut filtered = String::new();

    {
        // Scoped block: stdout lock held ONLY here.
        // RAII-dropped before joining threads or calling child.wait(),
        // preventing lock contention during blocking operations.
        let stdout_handle = io::stdout();
        let mut out = stdout_handle.lock();

        match stdout_mode {
            FilterMode::Passthrough => {
                for line in BufReader::new(stdout).lines().filter_map(Result::ok) {
                    if raw_stdout.len() < RAW_CAP {
                        raw_stdout.push_str(&line);
                        raw_stdout.push('\n');
                    }
                    match writeln!(out, "{}", line) {
                        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => break,
                        Err(e) => return Err(e.into()),
                        Ok(_) => {}
                    }
                }
                filtered = raw_stdout.clone();
            }
            FilterMode::Streaming(mut filter) => {
                for line in BufReader::new(stdout).lines().filter_map(Result::ok) {
                    if raw_stdout.len() < RAW_CAP {
                        raw_stdout.push_str(&line);
                        raw_stdout.push('\n');
                    }
                    if let Some(output) = filter.feed_line(&line) {
                        filtered.push_str(&output);
                        match write!(out, "{}", output) {
                            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => break,
                            Err(e) => return Err(e.into()),
                            Ok(_) => {}
                        }
                    }
                }
                let tail = filter.flush();
                filtered.push_str(&tail);
                // Guard against BrokenPipe (loop may have broken early above)
                match write!(out, "{}", tail) {
                    Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {}
                    Err(e) => return Err(e.into()),
                    Ok(_) => {}
                }
            }
            FilterMode::Buffered(filter_fn) => {
                for line in BufReader::new(stdout).lines().filter_map(Result::ok) {
                    if raw_stdout.len() < RAW_CAP {
                        raw_stdout.push_str(&line);
                        raw_stdout.push('\n');
                    }
                }
                let result = filter_fn(&raw_stdout);
                filtered = result.clone();
                match write!(out, "{}", result) {
                    Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {}
                    Err(e) => return Err(e.into()),
                    Ok(_) => {}
                }
            }
        }
    } // stdout lock RAII-dropped here — before any blocking joins or child.wait()

    // ── Join threads ──────────────────────────────────────────────────────────
    // stderr_thread finishes when child's stderr pipe closes (child exits).
    let raw_stderr = stderr_thread.join().unwrap_or_else(|_| String::new());
    // stdin_thread finishes when our stdin closes or child closed its stdin end.
    if let Some(t) = stdin_thread {
        t.join().ok();
    }

    // ── Wait for child ────────────────────────────────────────────────────────
    // Explicit wait captures the actual exit status.
    // ChildGuard.drop() will also call wait() as a safety net (ECHILD error ignored).
    let status = child.0.wait().context("Failed to wait for child")?;

    Ok(StreamResult {
        exit_code: status_to_exit_code(status),
        raw: format!("{}{}", raw_stdout, raw_stderr),
        filtered,
    })
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    // ── status_to_exit_code ────────────────────────────────────────────────────

    #[test]
    fn test_exit_code_zero() {
        let status = Command::new("true").status().unwrap();
        assert_eq!(status_to_exit_code(status), 0);
    }

    #[test]
    fn test_exit_code_nonzero() {
        let status = Command::new("false").status().unwrap();
        assert_eq!(status_to_exit_code(status), 1);
    }

    #[cfg(unix)]
    #[test]
    fn test_exit_code_signal_kill() {
        // kill() sends SIGKILL (9); POSIX exit code = 128 + 9 = 137
        let mut child = Command::new("sleep").arg("60").spawn().unwrap();
        child.kill().unwrap();
        let status = child.wait().unwrap();
        assert_eq!(status_to_exit_code(status), 137);
    }

    // ── LineFilter ─────────────────────────────────────────────────────────────

    #[test]
    fn test_line_filter_passes_lines() {
        let mut f = LineFilter::new(|l| Some(format!("{}\n", l.to_uppercase())));
        assert_eq!(f.feed_line("hello"), Some("HELLO\n".to_string()));
    }

    #[test]
    fn test_line_filter_drops_lines() {
        let mut f = LineFilter::new(|l| {
            if l.starts_with('#') {
                None
            } else {
                Some(l.to_string())
            }
        });
        assert_eq!(f.feed_line("# comment"), None);
        assert_eq!(f.feed_line("code"), Some("code".to_string()));
    }

    #[test]
    fn test_line_filter_flush_empty() {
        let mut f = LineFilter::new(|l| Some(l.to_string()));
        assert_eq!(f.flush(), String::new());
    }

    // ── StreamResult ───────────────────────────────────────────────────────────

    #[test]
    fn test_stream_result_success() {
        let r = StreamResult {
            exit_code: 0,
            raw: String::new(),
            filtered: String::new(),
        };
        assert!(r.success());
    }

    #[test]
    fn test_stream_result_failure() {
        let r = StreamResult {
            exit_code: 1,
            raw: String::new(),
            filtered: String::new(),
        };
        assert!(!r.success());
    }

    #[test]
    fn test_stream_result_signal_not_success() {
        let r = StreamResult {
            exit_code: 137,
            raw: String::new(),
            filtered: String::new(),
        };
        assert!(!r.success());
    }

    // ── run_streaming integration ──────────────────────────────────────────────

    #[test]
    fn test_run_streaming_passthrough_echo() {
        let mut cmd = Command::new("echo");
        cmd.arg("hello");
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::Passthrough).unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.raw.contains("hello"));
    }

    #[test]
    fn test_run_streaming_exit_code_preserved() {
        // sh -c "exit 42" → exit_code must be exactly 42, not 0 or 1
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "exit 42"]);
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::Passthrough).unwrap();
        assert_eq!(result.exit_code, 42);
    }

    #[test]
    fn test_run_streaming_exit_code_zero() {
        let mut cmd = Command::new("true");
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::Passthrough).unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.success());
    }

    #[test]
    fn test_run_streaming_exit_code_one() {
        let mut cmd = Command::new("false");
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::Passthrough).unwrap();
        assert_eq!(result.exit_code, 1);
        assert!(!result.success());
    }

    #[test]
    fn test_run_streaming_streaming_filter_drops_lines() {
        let mut cmd = Command::new("printf");
        cmd.arg("a\nb\nc\n");
        let filter = LineFilter::new(|l| {
            if l == "b" {
                None
            } else {
                Some(format!("{}\n", l))
            }
        });
        let result = run_streaming(
            &mut cmd,
            StdinMode::Null,
            FilterMode::Streaming(Box::new(filter)),
        )
        .unwrap();
        assert!(result.filtered.contains('a'));
        assert!(!result.filtered.contains('b'));
        assert!(result.filtered.contains('c'));
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn test_run_streaming_buffered_filter() {
        let mut cmd = Command::new("printf");
        cmd.arg("line1\nline2\nline3\n");
        fn upper(s: &str) -> String {
            s.to_uppercase()
        }
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::Buffered(upper)).unwrap();
        assert!(result.filtered.contains("LINE1"));
        assert!(result.filtered.contains("LINE2"));
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn test_run_streaming_raw_cap_at_1mb() {
        // Generate >1 MiB: 'yes | head -600000' ≈ 1.2 MiB of "y\n" lines.
        // raw must be capped at 1 MiB, not OOM.
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "yes | head -600000"]);
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::Passthrough).unwrap();
        // Allow small overshoot: last partial line may push us 1-2 bytes over cap.
        assert!(
            result.raw.len() <= 1_048_576 + 100,
            "raw should be capped at ~1 MiB, got {} bytes",
            result.raw.len()
        );
        // Must still have captured significant data.
        assert!(
            result.raw.len() > 100_000,
            "Should have captured significant data"
        );
    }

    #[test]
    fn test_child_guard_prevents_zombie() {
        // Verifies ChildGuard returns cleanly on fast-exiting commands.
        let mut cmd = Command::new("true");
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::Passthrough);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().exit_code, 0);
    }

    #[test]
    fn test_run_streaming_null_stdin_cat() {
        // With StdinMode::Null, cat gets EOF and exits 0 with empty output.
        let mut cmd = Command::new("cat");
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::Passthrough).unwrap();
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn test_run_streaming_raw_contains_stdout() {
        let mut cmd = Command::new("echo");
        cmd.arg("test_output_xyz");
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::Passthrough).unwrap();
        assert!(result.raw.contains("test_output_xyz"));
    }

    #[test]
    fn test_run_streaming_filtered_equals_raw_in_passthrough() {
        // In passthrough mode, filtered content matches raw stdout.
        let mut cmd = Command::new("echo");
        cmd.arg("check_equality");
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::Passthrough).unwrap();
        assert_eq!(result.filtered.trim(), result.raw.trim());
    }
}
