use anyhow::{Context, Result};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(test)]
use regex::Regex;

pub trait StreamFilter {
    fn feed_line(&mut self, line: &str) -> Option<String>;
    fn flush(&mut self) -> String;
    fn on_exit(&mut self, _exit_code: i32, _raw: &str) -> Option<String> {
        None
    }
}

pub trait BlockHandler {
    fn should_skip(&mut self, line: &str) -> bool;
    fn is_block_start(&mut self, line: &str) -> bool;
    fn is_block_continuation(&mut self, line: &str, block: &[String]) -> bool;
    fn format_summary(&self, exit_code: i32, raw: &str) -> Option<String>;
}

pub struct BlockStreamFilter<H: BlockHandler> {
    handler: H,
    in_block: bool,
    current_block: Vec<String>,
    blocks_emitted: usize,
}

impl<H: BlockHandler> BlockStreamFilter<H> {
    pub fn new(handler: H) -> Self {
        Self {
            handler,
            in_block: false,
            current_block: Vec::new(),
            blocks_emitted: 0,
        }
    }

    fn emit_block(&mut self) -> Option<String> {
        if self.current_block.is_empty() {
            return None;
        }
        let block = self.current_block.join("\n");
        self.current_block.clear();
        self.blocks_emitted += 1;
        Some(format!("{}\n", block))
    }
}

impl<H: BlockHandler> StreamFilter for BlockStreamFilter<H> {
    fn feed_line(&mut self, line: &str) -> Option<String> {
        if self.handler.should_skip(line) {
            return None;
        }

        if self.handler.is_block_start(line) {
            let prev = self.emit_block();
            self.current_block.push(line.to_string());
            self.in_block = true;
            prev
        } else if self.in_block {
            if self
                .handler
                .is_block_continuation(line, &self.current_block)
            {
                self.current_block.push(line.to_string());
                None
            } else {
                self.in_block = false;
                self.emit_block()
            }
        } else {
            None
        }
    }

    fn flush(&mut self) -> String {
        self.emit_block().unwrap_or_default()
    }

    fn on_exit(&mut self, exit_code: i32, raw: &str) -> Option<String> {
        self.handler.format_summary(exit_code, raw)
    }
}

/// Counterpart to [`BlockHandler`] for line-oriented streams.
///
/// Default behaviour is KEEP — every line is emitted unchanged. Implementors
/// opt in to dropping noise via [`LineHandler::should_skip`] and may capture
/// state for the final summary via [`LineHandler::observe_line`].
pub trait LineHandler {
    fn should_skip(&mut self, _line: &str) -> bool {
        false
    }

    fn observe_line(&mut self, _line: &str) {}

    fn format_summary(&self, exit_code: i32, raw: &str) -> Option<String>;
}

pub struct LineStreamFilter<H: LineHandler> {
    handler: H,
}

impl<H: LineHandler> LineStreamFilter<H> {
    pub fn new(handler: H) -> Self {
        Self { handler }
    }
}

impl<H: LineHandler> StreamFilter for LineStreamFilter<H> {
    fn feed_line(&mut self, line: &str) -> Option<String> {
        if self.handler.should_skip(line) {
            return None;
        }
        self.handler.observe_line(line);
        Some(format!("{}\n", line))
    }

    fn flush(&mut self) -> String {
        String::new()
    }

    fn on_exit(&mut self, exit_code: i32, raw: &str) -> Option<String> {
        self.handler.format_summary(exit_code, raw)
    }
}

#[cfg(test)] // available for command modules; currently used in tests only
pub struct RegexBlockFilter {
    start_re: Regex,
    skip_prefixes: Vec<String>,
    tool_name: String,
    block_count: usize,
}

#[cfg(test)]
impl RegexBlockFilter {
    pub fn new(tool_name: &str, start_pattern: &str) -> Self {
        Self {
            start_re: Regex::new(start_pattern).unwrap_or_else(|e| {
                panic!("RegexBlockFilter: bad pattern '{}': {}", start_pattern, e)
            }),
            skip_prefixes: Vec::new(),
            tool_name: tool_name.to_string(),
            block_count: 0,
        }
    }

    pub fn skip_prefix(mut self, prefix: &str) -> Self {
        self.skip_prefixes.push(prefix.to_string());
        self
    }

    pub fn skip_prefixes(mut self, prefixes: &[&str]) -> Self {
        self.skip_prefixes
            .extend(prefixes.iter().map(|s| s.to_string()));
        self
    }
}

#[cfg(test)]
impl BlockHandler for RegexBlockFilter {
    fn should_skip(&mut self, line: &str) -> bool {
        self.skip_prefixes.iter().any(|p| line.starts_with(p))
    }

    fn is_block_start(&mut self, line: &str) -> bool {
        if self.start_re.is_match(line) {
            self.block_count += 1;
            true
        } else {
            false
        }
    }

    fn is_block_continuation(&mut self, line: &str, _block: &[String]) -> bool {
        line.starts_with(' ') || line.starts_with('\t')
    }

    fn format_summary(&self, _exit_code: i32, _raw: &str) -> Option<String> {
        if self.block_count == 0 {
            Some(format!("{}: no errors found\n", self.tool_name))
        } else {
            Some(format!(
                "{}: {} blocks in output\n",
                self.tool_name, self.block_count
            ))
        }
    }
}

pub trait StdinFilter: Send {
    fn feed_line(&mut self, line: &str) -> Option<String>;
    fn flush(&mut self) -> String;
}

pub enum FilterMode<'a> {
    Streaming(Box<dyn StreamFilter + 'a>),
    #[allow(dead_code)]
    Buffered(Box<dyn Fn(&str) -> String + 'a>),
    CaptureOnly,
    Passthrough,
}

pub enum StdinMode {
    Inherit,
    #[allow(dead_code)] // future API: stdin filtering for interactive commands
    Filter(Box<dyn StdinFilter + Send>),
    Null,
}

pub struct StreamResult {
    pub exit_code: i32,
    pub raw: String,
    pub raw_stdout: String,
    pub raw_stderr: String,
    pub filtered: String,
}

impl StreamResult {
    #[cfg(test)]
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}

pub fn status_to_exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    1
}

/// Collapse single-line terminal redraw controls to the final rendered text before
/// filters parse output. Models a one-line cursor: `\r` returns to column 0, `\b` moves
/// left, printable chars overlay at the cursor, `\n` commits the line.
///
/// CSI escape sequences (`ESC [ … final`) are parsed as a *unit* — this is the fix for the
/// real defect: erase-in-line (`ESC [ K`) is honored against the cursor, so a *shrinking*
/// redraw (`\r` + erase, the way programs clean up a longer previous frame) resolves
/// correctly instead of leaving a stale tail or leaking the literal escape bytes. A bare
/// `\r` overlay alone cannot express that erase, which is why the previous frame's tail
/// survived before this fix.
///
/// Every *other* CSI sequence (SGR colour, cursor motion) is overlaid through the SAME
/// write path as printable text — collapse does not strip colour (that is `strip_ansi`'s
/// separate, config-gated job), and a later `\r` redraw overwrites prior escape bytes cell
/// for cell rather than shifting them, so a same-or-longer coloured redraw resolves cleanly.
///
/// Scope boundary: this models the *cursor*, not per-cell colour *attributes*. So a
/// genuinely *shorter* coloured frame overlaying a longer one can leave a trailing reset
/// (`ESC[0m`) from the prior frame in the collapsed bytes — harmless, and `strip_ansi`
/// removes it downstream where it matters. Tracking that would require a full VT attribute
/// model, which is out of scope (rtk is not a terminal emulator).
fn collapse_terminal_control(text: &str) -> String {
    let mut visible = String::new();
    let mut line: Vec<char> = Vec::new();
    let mut cursor = 0usize;
    let mut chars = text.chars().peekable();

    // Overlay one char at the cursor (shared by printable chars and re-emitted escapes),
    // padding with spaces if the cursor was advanced past the current end (e.g. post-erase).
    let overlay = |line: &mut Vec<char>, cursor: &mut usize, ch: char| {
        while line.len() < *cursor {
            line.push(' ');
        }
        if *cursor < line.len() {
            line[*cursor] = ch;
        } else {
            line.push(ch);
        }
        *cursor += 1;
    };

    while let Some(ch) = chars.next() {
        match ch {
            '\r' if chars.peek() == Some(&'\n') => {
                chars.next();
                visible.extend(line.drain(..));
                visible.push('\n');
                cursor = 0;
            }
            '\r' => {
                cursor = 0;
            }
            '\n' => {
                visible.extend(line.drain(..));
                visible.push('\n');
                cursor = 0;
            }
            '\u{8}' => {
                cursor = cursor.saturating_sub(1);
            }
            '\u{1b}' => {
                // Escape. Parse a CSI sequence (ESC '[' params… final) as a unit. A CSI
                // final byte is 0x40–0x7E; bytes before it are parameter/intermediate bytes.
                if chars.peek() == Some(&'[') {
                    chars.next(); // consume '['
                    let mut params = String::new();
                    let final_byte = loop {
                        match chars.next() {
                            Some(c) if ('\u{40}'..='\u{7e}').contains(&c) => break Some(c),
                            Some(c) => params.push(c),
                            None => break None, // truncated sequence at EOF
                        }
                    };
                    match final_byte {
                        // Erase-in-line — the operation collapse exists to honor.
                        Some('K') => match params.as_str() {
                            "" | "0" => line.truncate(cursor), // cursor → end of line
                            "1" => {
                                for cell in line.iter_mut().take(cursor + 1) {
                                    *cell = ' '; // start of line → cursor
                                }
                            }
                            "2" => line.clear(), // whole line
                            _ => line.truncate(cursor),
                        },
                        // Any other CSI: overlay the sequence verbatim through the SAME write
                        // path as printable text, so colour passes through unchanged (prior
                        // contract) and a later `\r` redraw overwrites it rather than shifting.
                        Some(fb) => {
                            overlay(&mut line, &mut cursor, '\u{1b}');
                            overlay(&mut line, &mut cursor, '[');
                            for c in params.chars() {
                                overlay(&mut line, &mut cursor, c);
                            }
                            overlay(&mut line, &mut cursor, fb);
                        }
                        None => {} // truncated at EOF — drop
                    }
                }
                // A lone ESC (not a CSI) is dropped — zero-width control data.
            }
            ch => {
                overlay(&mut line, &mut cursor, ch);
            }
        }
    }

    visible.extend(line);
    visible
}

// ISSUE #897: ChildGuard RAII prevents zombie processes that caused kernel panic
pub const RAW_CAP: usize = 10_485_760; // 10 MiB

// After the direct child exits, a descendant may still hold the captured pipe open
// (e.g. `ng build` leaves an `esbuild --service` grandchild on node's stderr). Reading
// until pipe EOF would then block forever, so we wait on the direct child and only
// briefly drain whatever is already buffered. See docs/pr_briefs/004-pipe-eof-grandchild-hang.
const STREAM_POST_EXIT_IDLE_GRACE: Duration = Duration::from_millis(200);
const STREAM_POST_EXIT_MAX_DRAIN: Duration = Duration::from_secs(2);

/// Spawn a background thread that reads a child pipe line-by-line into a shared buffer,
/// appending `\n` after each line and honoring [`RAW_CAP`]. Signals `done_tx` once the
/// pipe reaches EOF. The buffer is readable at any time, so a caller can recover output
/// already collected even if the thread is still blocked on a pipe held open by a
/// detached grandchild. Terminal redraw controls are collapsed by the caller on the
/// final buffer (see [`collapse_terminal_control`]).
fn spawn_capture_reader<R: io::Read + Send + 'static>(
    pipe: R,
    label: &'static str,
    done_tx: mpsc::Sender<()>,
) -> Arc<Mutex<String>> {
    let buf = Arc::new(Mutex::new(String::new()));
    let buf_for_thread = Arc::clone(&buf);
    std::thread::spawn(move || {
        let mut capped = false;
        for line in BufReader::new(pipe).lines().map_while(Result::ok) {
            let mut b = buf_for_thread.lock().expect("capture buffer lock poisoned");
            if b.len() + line.len() < RAW_CAP {
                b.push_str(&line);
                b.push('\n');
            } else if !capped {
                capped = true;
                eprintln!("[rtk] warning: {label} exceeds 10 MiB — capture truncated");
            }
        }
        let _ = done_tx.send(());
    });
    buf
}

/// Block until both reader threads report EOF, OR the captured output goes idle for
/// [`STREAM_POST_EXIT_IDLE_GRACE`] after the child has already exited, OR the overall
/// [`STREAM_POST_EXIT_MAX_DRAIN`] cap is hit. Called only after the direct child exits,
/// so a still-open pipe held by a descendant can no longer cause an unbounded wait.
fn drain_capture_readers(
    done_rx: &mpsc::Receiver<()>,
    stdout_buf: &Arc<Mutex<String>>,
    stderr_buf: &Arc<Mutex<String>>,
) {
    let captured_len = |a: &Arc<Mutex<String>>, b: &Arc<Mutex<String>>| {
        a.lock().expect("capture buffer lock poisoned").len()
            + b.lock().expect("capture buffer lock poisoned").len()
    };
    let started = Instant::now();
    let mut completed = 0;
    let mut last_len = captured_len(stdout_buf, stderr_buf);

    while completed < 2 && started.elapsed() < STREAM_POST_EXIT_MAX_DRAIN {
        match done_rx.recv_timeout(STREAM_POST_EXIT_IDLE_GRACE) {
            Ok(()) => completed += 1,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let current_len = captured_len(stdout_buf, stderr_buf);
                if current_len == last_len {
                    break; // no new output during the grace window — descendant holds the pipe
                }
                last_len = current_len;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Join a reader thread only if it has already finished; otherwise leave it detached.
/// A thread still blocked in `pipe.read()` on a pipe held open by a detached descendant
/// would block `join()` indefinitely — the exact hang we are avoiding — so we never wait
/// on an unfinished reader.
fn try_join_finished(handle: std::thread::JoinHandle<()>) {
    if handle.is_finished() {
        handle.join().ok();
    }
}

pub fn run_streaming(
    cmd: &mut Command,
    stdin_mode: StdinMode,
    stdout_mode: FilterMode<'_>,
) -> Result<StreamResult> {
    if matches!(stdout_mode, FilterMode::Passthrough) {
        match &stdin_mode {
            StdinMode::Inherit => {
                cmd.stdin(Stdio::inherit());
            }
            _ => {
                cmd.stdin(Stdio::null());
            }
        };
        cmd.stdout(Stdio::inherit());
        cmd.stderr(Stdio::inherit());
        let status = cmd.status().context("Failed to spawn process")?;
        return Ok(StreamResult {
            exit_code: status_to_exit_code(status),
            raw: String::new(),
            raw_stdout: String::new(),
            raw_stderr: String::new(),
            filtered: String::new(),
        });
    }

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

    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            self.0.wait().ok();
        }
    }

    let is_streaming = matches!(stdout_mode, FilterMode::Streaming(_));

    let mut child = ChildGuard(cmd.spawn().context("Failed to spawn process")?);

    let stdin_thread: Option<std::thread::JoinHandle<()>> = match stdin_mode {
        StdinMode::Filter(mut filter) => {
            let child_stdin = child.0.stdin.take().context("No child stdin handle")?;
            Some(std::thread::spawn(move || {
                let mut writer = BufWriter::new(child_stdin);
                let stdin_handle = io::stdin();
                for line in BufReader::new(stdin_handle.lock())
                    .lines()
                    .map_while(Result::ok)
                {
                    if let Some(out) = filter.feed_line(&line) {
                        if writeln!(writer, "{}", out).is_err() {
                            break;
                        }
                    }
                }
                let tail = filter.flush();
                if !tail.is_empty() {
                    write!(writer, "{}", tail).ok();
                }
            }))
        }
        StdinMode::Null => {
            child.0.stdin.take();
            None
        }
        StdinMode::Inherit => None,
    };

    let stdout = child.0.stdout.take().context("No child stdout handle")?;
    let stderr = child.0.stderr.take().context("No child stderr handle")?;
    let mut raw_stdout = String::new();
    let mut raw_stderr = String::new();
    let mut filtered = String::new();
    let mut capped_out = false;
    let mut capped_err = false;
    let mut saved_filter: Option<Box<dyn StreamFilter + '_>> = None;
    let mut filter_fd_is_stderr = false;

    if is_streaming {
        enum StreamLine {
            Stdout(String),
            Stderr(String),
        }

        let (tx, rx) = mpsc::channel();
        let tx_out = tx.clone();
        let stdout_thread = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx_out.send(StreamLine::Stdout(line)).is_err() {
                    break;
                }
            }
        });
        let tx_err = tx;
        let stderr_thread = std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if tx_err.send(StreamLine::Stderr(line)).is_err() {
                    break;
                }
            }
        });

        if let FilterMode::Streaming(mut filter) = stdout_mode {
            let stdout_handle = io::stdout();
            let mut out = stdout_handle.lock();
            let stderr_handle = io::stderr();
            let mut err_out = stderr_handle.lock();

            // Consume streamed lines, but never block on pipe EOF: once the DIRECT child
            // has exited, only drain briefly. A descendant that inherited the pipe (e.g.
            // `ng build`'s esbuild service) keeps the write-end open forever, so a plain
            // `for msg in rx` would hang. We poll the channel and, on an idle gap, check
            // the child via try_wait (which caches the reaped status for the wait() below).
            let mut child_exited_at: Option<Instant> = None;
            'consume: loop {
                let msg = match rx.recv_timeout(STREAM_POST_EXIT_IDLE_GRACE) {
                    Ok(msg) => msg,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break, // both readers hit EOF
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if child_exited_at.is_none() {
                            if let Some(_status) =
                                child.0.try_wait().context("Failed to poll child")?
                            {
                                child_exited_at = Some(Instant::now());
                            }
                        }
                        // Child exited and the stream went idle for a full grace window →
                        // remaining writers are detached descendants; stop draining.
                        if child_exited_at.is_some() {
                            break;
                        }
                        continue;
                    }
                };
                // Hard cap: even if a descendant keeps emitting after the child exited,
                // don't drain forever.
                if let Some(t) = child_exited_at {
                    if t.elapsed() > STREAM_POST_EXIT_MAX_DRAIN {
                        break;
                    }
                }
                let (line, is_stderr) = match msg {
                    StreamLine::Stderr(l) => (l, true),
                    StreamLine::Stdout(l) => (l, false),
                };
                let line = collapse_terminal_control(&line);
                if is_stderr {
                    if !capped_err {
                        if raw_stderr.len() + line.len() < RAW_CAP {
                            raw_stderr.push_str(&line);
                            raw_stderr.push('\n');
                        } else {
                            capped_err = true;
                            eprintln!("[rtk] warning: stderr exceeds 10 MiB — capture truncated");
                        }
                    }
                } else if !capped_out {
                    if raw_stdout.len() + line.len() < RAW_CAP {
                        raw_stdout.push_str(&line);
                        raw_stdout.push('\n');
                    } else {
                        capped_out = true;
                        eprintln!("[rtk] warning: stdout exceeds 10 MiB — filter input truncated");
                    }
                }
                filter_fd_is_stderr = is_stderr;
                if let Some(output) = filter.feed_line(&line) {
                    filtered.push_str(&output);
                    let dest: &mut dyn Write = if is_stderr { &mut err_out } else { &mut out };
                    match write!(dest, "{}", output) {
                        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => break 'consume,
                        Err(e) => return Err(e.into()),
                        Ok(_) => {}
                    }
                }
            }
            let tail = filter.flush();
            filtered.push_str(&tail);
            let flush_dest: &mut dyn Write = if filter_fd_is_stderr {
                &mut err_out
            } else {
                &mut out
            };
            match write!(flush_dest, "{}", tail) {
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {}
                Err(e) => return Err(e.into()),
                Ok(_) => {}
            }
            saved_filter = Some(filter);
        }

        // Drop our receiver so any reader thread still blocked in `send()` unblocks and
        // exits. A thread blocked in `pipe.read()` on a pipe held open by a detached
        // descendant cannot be force-joined without blocking us again, so we deliberately
        // do NOT join it here — it exits when the pipe finally closes. `try_join_finished`
        // only waits on threads that have already finished.
        drop(rx);
        try_join_finished(stdout_thread);
        try_join_finished(stderr_thread);
    } else {
        // Non-streaming capture (Buffered / CaptureOnly). Read both pipes on background
        // threads, then wait on the DIRECT child and only briefly drain afterwards, so a
        // descendant that inherited the pipe (e.g. esbuild) can't cause an unbounded wait.
        let (done_tx, done_rx) = mpsc::channel();
        let stdout_buf = spawn_capture_reader(stdout, "output", done_tx.clone());
        let stderr_buf = spawn_capture_reader(stderr, "stderr", done_tx);

        let status = child.0.wait().context("Failed to wait for child")?;
        drain_capture_readers(&done_rx, &stdout_buf, &stderr_buf);
        if let Some(t) = stdin_thread {
            t.join().ok();
        }

        // Collapse terminal redraw controls on the final buffers (matches the streaming
        // path and exec_capture); see collapse_terminal_control / upstream #2581.
        raw_stdout = collapse_terminal_control(&std::mem::take(
            &mut *stdout_buf.lock().expect("capture buffer lock poisoned"),
        ));
        raw_stderr = collapse_terminal_control(&std::mem::take(
            &mut *stderr_buf.lock().expect("capture buffer lock poisoned"),
        ));

        let stdout_handle = io::stdout();
        let mut out = stdout_handle.lock();
        match stdout_mode {
            FilterMode::Passthrough => unreachable!("handled by early-return above"),
            FilterMode::Streaming(_) => unreachable!("handled by is_streaming branch"),
            FilterMode::Buffered(filter_fn) => {
                filtered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    filter_fn(&raw_stdout)
                }))
                .unwrap_or_else(|_| {
                    eprintln!("[rtk] warning: filter panicked — passing through raw output");
                    raw_stdout.clone()
                });
                match write!(out, "{}", filtered) {
                    Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {}
                    Err(e) => return Err(e.into()),
                    Ok(_) => {}
                }
            }
            FilterMode::CaptureOnly => {
                filtered = raw_stdout.clone();
            }
        }

        let exit_code = status_to_exit_code(status);
        let raw = format!("{}{}", raw_stdout, raw_stderr);

        if let Some(mut f) = saved_filter {
            if let Some(post) = f.on_exit(exit_code, &raw) {
                filtered.push_str(&post);
                match write!(io::stdout().lock(), "{}", post) {
                    Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {}
                    Err(e) => return Err(e.into()),
                    Ok(_) => {}
                }
            }
        }

        return Ok(StreamResult {
            exit_code,
            raw,
            raw_stdout,
            raw_stderr,
            filtered,
        });
    }
    if let Some(t) = stdin_thread {
        t.join().ok();
    }

    let status = child.0.wait().context("Failed to wait for child")?;
    let exit_code = status_to_exit_code(status);
    let raw = format!("{}{}", raw_stdout, raw_stderr);

    if let Some(mut f) = saved_filter {
        if let Some(post) = f.on_exit(exit_code, &raw) {
            filtered.push_str(&post);
            let mut dest: Box<dyn Write> = if filter_fd_is_stderr {
                Box::new(io::stderr().lock())
            } else {
                Box::new(io::stdout().lock())
            };
            match write!(dest, "{}", post) {
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {}
                Err(e) => return Err(e.into()),
                Ok(_) => {}
            }
        }
    }

    Ok(StreamResult {
        exit_code,
        raw,
        raw_stdout,
        raw_stderr,
        filtered,
    })
}

pub struct CaptureResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl CaptureResult {
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }

    pub fn combined(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

pub fn exec_capture(cmd: &mut Command) -> Result<CaptureResult> {
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    // RAII so a `?` below still reaps the child rather than leaking a zombie.
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            self.0.wait().ok();
        }
    }
    let mut child = ChildGuard(cmd.spawn().context("Failed to execute command")?);

    let stdout_pipe = child.0.stdout.take().context("No child stdout handle")?;
    let stderr_pipe = child.0.stderr.take().context("No child stderr handle")?;

    // Read both pipes on background threads into shared buffers. A descendant that
    // inherited the pipe (e.g. an `esbuild --service` grandchild) keeps the write-end
    // open after the direct child exits, so reading to EOF on this thread — what plain
    // `cmd.output()` does — would block forever. Adopted from #2322's exec_capture half.
    let (done_tx, done_rx) = mpsc::channel();
    let stdout_buf = spawn_capture_reader(stdout_pipe, "stdout", done_tx.clone());
    let stderr_buf = spawn_capture_reader(stderr_pipe, "stderr", done_tx);

    let status = child.0.wait().context("Failed to wait for command")?;
    // Child exited: bounded-drain whatever the readers already buffered, then return even
    // if a detached descendant still holds the pipe open (see drain_capture_readers).
    drain_capture_readers(&done_rx, &stdout_buf, &stderr_buf);

    let stdout = std::mem::take(&mut *stdout_buf.lock().expect("capture buffer lock poisoned"));
    let stderr = std::mem::take(&mut *stderr_buf.lock().expect("capture buffer lock poisoned"));
    Ok(CaptureResult {
        stdout: collapse_terminal_control(&stdout),
        stderr: collapse_terminal_control(&stderr),
        exit_code: status_to_exit_code(status),
    })
}

/// Like [`exec_capture`] but inherits stdin so a wrapped engine can read a piped stdin.
pub fn exec_capture_stdin(cmd: &mut Command) -> Result<CaptureResult> {
    cmd.stdin(Stdio::inherit());
    let output = cmd.output().context("Failed to execute command")?;
    Ok(CaptureResult {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: status_to_exit_code(output.status),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::process::Command;

    struct LineFilter<F: FnMut(&str) -> Option<String>> {
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
        let mut child = Command::new("sleep").arg("60").spawn().unwrap();
        child.kill().unwrap();
        let status = child.wait().unwrap();
        assert_eq!(status_to_exit_code(status), 137);
    }

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

    #[test]
    fn test_stream_result_success() {
        let r = StreamResult {
            exit_code: 0,
            raw: String::new(),
            raw_stdout: String::new(),
            raw_stderr: String::new(),
            filtered: String::new(),
        };
        assert!(r.success());
    }

    #[test]
    fn test_stream_result_failure() {
        let r = StreamResult {
            exit_code: 1,
            raw: String::new(),
            raw_stdout: String::new(),
            raw_stderr: String::new(),
            filtered: String::new(),
        };
        assert!(!r.success());
    }

    #[test]
    fn test_stream_result_signal_not_success() {
        let r = StreamResult {
            exit_code: 137,
            raw: String::new(),
            raw_stdout: String::new(),
            raw_stderr: String::new(),
            filtered: String::new(),
        };
        assert!(!r.success());
    }

    #[test]
    fn test_run_streaming_passthrough_echo() {
        let mut cmd = Command::new("echo");
        cmd.arg("hello");
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::Passthrough).unwrap();
        assert_eq!(result.exit_code, 0);
        // Passthrough inherits TTY — raw/filtered are empty
        assert!(result.raw.is_empty());
    }

    #[test]
    fn test_run_streaming_exit_code_preserved() {
        // nosemgrep: interpreter-execution
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

    #[cfg(not(windows))]
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

    #[cfg(not(windows))]
    #[test]
    fn test_run_streaming_buffered_filter() {
        let mut cmd = Command::new("printf");
        cmd.arg("line1\nline2\nline3\n");
        let result = run_streaming(
            &mut cmd,
            StdinMode::Null,
            FilterMode::Buffered(Box::new(|s: &str| s.to_uppercase())),
        )
        .unwrap();
        assert!(result.filtered.contains("LINE1"));
        assert!(result.filtered.contains("LINE2"));
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn test_run_streaming_raw_cap_at_10mb() {
        // nosemgrep: interpreter-execution
        let mut cmd = Command::new("sh");
        // ~11 MiB of 80-char lines (fast: fewer lines than `yes | head -6M`)
        cmd.args([
            "-c",
            "dd if=/dev/zero bs=1024 count=11264 2>/dev/null | tr '\\0' 'a' | fold -w 80",
        ]);
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::CaptureOnly).unwrap();
        assert!(
            result.raw.len() <= 10_485_760 + 100,
            "raw should be capped at ~10 MiB, got {} bytes",
            result.raw.len()
        );
        assert!(
            result.raw.len() > 1_000_000,
            "Should have captured significant data"
        );
    }

    #[test]
    fn test_run_streaming_stderr_cap_at_10mb() {
        // nosemgrep: interpreter-execution
        let mut cmd = Command::new("sh");
        // ~11 MiB on stderr, nothing on stdout
        cmd.args([
            "-c",
            "dd if=/dev/zero bs=1024 count=11264 2>/dev/null | tr '\\0' 'a' | fold -w 80 1>&2",
        ]);
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::CaptureOnly).unwrap();
        // raw = raw_stdout + raw_stderr; stdout is empty so raw ≈ stderr size
        assert!(
            result.raw.len() <= RAW_CAP + 200,
            "stderr in raw should be capped at ~10 MiB, got {} bytes",
            result.raw.len()
        );
    }

    #[test]
    fn test_child_guard_prevents_zombie() {
        let mut cmd = Command::new("true");
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::CaptureOnly);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().exit_code, 0);
    }

    #[test]
    fn test_run_streaming_null_stdin_cat() {
        let mut cmd = Command::new("cat");
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::Passthrough).unwrap();
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn test_run_streaming_raw_contains_stdout() {
        let mut cmd = Command::new("echo");
        cmd.arg("test_output_xyz");
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::CaptureOnly).unwrap();
        assert!(result.raw.contains("test_output_xyz"));
    }

    #[test]
    fn test_run_streaming_capture_only_filtered_equals_raw() {
        let mut cmd = Command::new("echo");
        cmd.arg("check_equality");
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::CaptureOnly).unwrap();
        assert_eq!(result.filtered.trim(), result.raw_stdout.trim());
    }

    #[test]
    fn test_run_streaming_capture_only_collapses_carriage_return_progress() {
        // nosemgrep: interpreter-execution
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf 'step 1\\rstep 2\\n'"]);
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::CaptureOnly).unwrap();
        assert_eq!(result.raw_stdout, "step 2\n");
        assert_eq!(result.filtered, "step 2\n");
    }

    // ── collapse_terminal_control: terminal-faithful redraw resolution ────────────
    // Ground truth = what a real VT100-ish terminal renders. A bare `\r` moves the
    // cursor to column 0 but does NOT clear the line, so a shorter frame overlaying a
    // longer one leaves the old tail (this is faithful, not a bug). Programs that want a
    // clean shrink emit `\r` + ESC[K (erase to end of line) or pad with spaces.

    #[test]
    fn test_collapse_bare_cr_overlay_is_faithful() {
        // No clear: `Done` overlays `Down`, tail `loading 100%` survives — exactly what a
        // real terminal shows for a bare `\r` with no erase. Must NOT be "fixed" away.
        assert_eq!(
            collapse_terminal_control("Downloading 100%\rDone\n"),
            "Doneloading 100%\n"
        );
    }

    #[test]
    fn test_collapse_progress_bar_keeps_final() {
        assert_eq!(
            collapse_terminal_control("Building 10%\rBuilding 60%\rBuilding 100%\n"),
            "Building 100%\n"
        );
    }

    #[test]
    fn test_collapse_backspace_overwrites() {
        assert_eq!(collapse_terminal_control("abc\u{8}D\n"), "abD\n");
    }

    #[test]
    fn test_collapse_plain_and_crlf_unchanged() {
        assert_eq!(
            collapse_terminal_control("line1\nline2\nline3\n"),
            "line1\nline2\nline3\n"
        );
        assert_eq!(collapse_terminal_control("a\r\nb\r\n"), "a\nb\n");
    }

    // ── The bug: ESC[K (erase to end of line) is not honored ──────────────────────
    // Real shrinking redraws use `\r` + ESC[K. #2581's collapse has no ANSI awareness,
    // so it (a) leaks the literal escape into output and (b) leaves the stale tail.
    // These tests pin the terminal-correct result and FAIL until ESC[K is handled.

    #[test]
    fn test_collapse_cr_then_erase_to_eol_clears_tail() {
        // `Downloading 100%` then `\r`, erase-to-EOL, `Done` → terminal shows `Done`.
        assert_eq!(
            collapse_terminal_control("Downloading 100%\r\x1b[KDone\n"),
            "Done\n"
        );
    }

    #[test]
    fn test_collapse_erase_to_eol_midline() {
        // `abcdef`, `\r`, write `XY`, erase-to-EOL → `XY` (tail `cdef` cleared, no escape leak).
        assert_eq!(collapse_terminal_control("abcdef\rXY\x1b[K\n"), "XY\n");
    }

    #[test]
    fn test_collapse_preserves_sgr_color() {
        // SGR colour is NOT collapse's concern — the pipe capture path passed colour
        // through to the command filters (which strip it themselves where needed), so
        // collapse must not start stripping it. Only erase-in-line (ESC[K) is intercepted.
        assert_eq!(
            collapse_terminal_control("\x1b[32mPASS\x1b[0m\n"),
            "\x1b[32mPASS\x1b[0m\n"
        );
    }

    #[test]
    fn test_collapse_colored_progress_redraw_keeps_final_with_color() {
        // A coloured progress line redrawn via `\r`: the final frame (with its colour
        // codes) survives, earlier frames collapse away. Pins that re-emitted CSI bytes
        // participate in overlay/redraw like normal cells, not a one-way shift.
        assert_eq!(
            collapse_terminal_control("\x1b[33m50%\x1b[0m\r\x1b[32m100%\x1b[0m\n"),
            "\x1b[32m100%\x1b[0m\n"
        );
    }

    #[test]
    fn test_exec_capture_success() {
        let mut cmd = Command::new("echo");
        cmd.arg("hello_capture");
        let result = exec_capture(&mut cmd).unwrap();
        assert!(result.success());
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("hello_capture"));
    }

    #[test]
    fn test_exec_capture_failure() {
        let mut cmd = Command::new("false");
        let result = exec_capture(&mut cmd).unwrap();
        assert!(!result.success());
        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn test_exec_capture_stderr() {
        // nosemgrep: interpreter-execution
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo err_msg >&2"]);
        let result = exec_capture(&mut cmd).unwrap();
        assert!(result.stderr.contains("err_msg"));
    }

    #[test]
    fn test_exec_capture_combined() {
        // nosemgrep: interpreter-execution
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo out_msg; echo err_msg >&2"]);
        let result = exec_capture(&mut cmd).unwrap();
        let combined = result.combined();
        assert!(combined.contains("out_msg"));
        assert!(combined.contains("err_msg"));
    }

    #[test]
    fn test_exec_capture_collapses_carriage_return_progress() {
        // nosemgrep: interpreter-execution
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf 'Downloading... 0%%\\rDownloading... 3%%\\n'"]);
        let result = exec_capture(&mut cmd).unwrap();
        assert_eq!(result.stdout, "Downloading... 3%\n");
    }

    #[test]
    fn test_exec_capture_applies_backspace_overwrites() {
        // nosemgrep: interpreter-execution
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf 'abc\\bD\\n'"]);
        let result = exec_capture(&mut cmd).unwrap();
        assert_eq!(result.stdout, "abD\n");
    }

    #[test]
    fn test_capture_result_combined_empty() {
        let r = CaptureResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        };
        assert_eq!(r.combined(), "");
    }

    pub fn run_block_filter(filter: &mut dyn StreamFilter, input: &str, exit_code: i32) -> String {
        let mut output = String::new();
        for line in input.lines() {
            if let Some(s) = filter.feed_line(line) {
                output.push_str(&s);
            }
        }
        output.push_str(&filter.flush());
        if let Some(post) = filter.on_exit(exit_code, input) {
            output.push_str(&post);
        }
        output
    }

    struct TestHandler;

    impl BlockHandler for TestHandler {
        fn should_skip(&mut self, line: &str) -> bool {
            line.starts_with("SKIP")
        }
        fn is_block_start(&mut self, line: &str) -> bool {
            line.starts_with("ERROR")
        }
        fn is_block_continuation(&mut self, line: &str, _block: &[String]) -> bool {
            line.starts_with("  ")
        }
        fn format_summary(&self, _exit_code: i32, _raw: &str) -> Option<String> {
            Some("DONE\n".to_string())
        }
    }

    #[test]
    fn test_block_filter_emits_blocks() {
        let mut f = BlockStreamFilter::new(TestHandler);
        let input = "SKIP noise\nERROR first\n  detail1\nnon-block\nERROR second\n  detail2\n";
        let result = run_block_filter(&mut f, input, 0);
        assert!(result.contains("ERROR first\n  detail1"), "got: {}", result);
        assert!(
            result.contains("ERROR second\n  detail2"),
            "got: {}",
            result
        );
        assert!(!result.contains("SKIP"), "got: {}", result);
        assert!(result.ends_with("DONE\n"), "got: {}", result);
    }

    #[test]
    fn test_block_filter_no_blocks() {
        let mut f = BlockStreamFilter::new(TestHandler);
        let result = run_block_filter(&mut f, "nothing here\njust text\n", 0);
        assert_eq!(result, "DONE\n");
    }

    #[test]
    fn test_regex_block_filter_emits_blocks() {
        let handler = RegexBlockFilter::new("test", r"^error\[");
        let mut f = BlockStreamFilter::new(handler);
        let input = "ok line\nerror[E0308]: mismatched types\n  expected `u32`\nok again\nerror[E0599]: no method\n  help: try\n";
        let result = run_block_filter(&mut f, input, 1);
        assert!(
            result.contains("error[E0308]: mismatched types\n  expected `u32`"),
            "got: {}",
            result
        );
        assert!(
            result.contains("error[E0599]: no method\n  help: try"),
            "got: {}",
            result
        );
        assert!(
            result.contains("test: 2 blocks in output"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_regex_block_filter_skip_prefix() {
        let handler = RegexBlockFilter::new("test", r"^error").skip_prefix("warning:");
        let mut f = BlockStreamFilter::new(handler);
        let input = "warning: unused var\nerror: bad type\n  detail\nwarning: dead code\n";
        let result = run_block_filter(&mut f, input, 1);
        assert!(result.contains("error: bad type"), "got: {}", result);
        assert!(!result.contains("warning:"), "got: {}", result);
    }

    #[test]
    fn test_regex_block_filter_no_blocks() {
        let handler = RegexBlockFilter::new("mytest", r"^FAIL");
        let mut f = BlockStreamFilter::new(handler);
        let result = run_block_filter(&mut f, "all passed\nok\n", 0);
        assert_eq!(result, "mytest: no errors found\n");
    }

    #[test]
    fn test_regex_block_filter_indent_continuation() {
        let handler = RegexBlockFilter::new("test", r"^ERR");
        let mut f = BlockStreamFilter::new(handler);
        let input = "ERR space indent\n  two spaces\n\ttab indent\nnon-indent\n";
        let result = run_block_filter(&mut f, input, 1);
        assert!(
            result.contains("ERR space indent\n  two spaces\n\ttab indent"),
            "got: {}",
            result
        );
        assert!(!result.contains("non-indent"), "got: {}", result);
    }

    #[test]
    fn test_regex_block_filter_multiple_skip_prefixes() {
        let handler =
            RegexBlockFilter::new("test", r"^error").skip_prefixes(&["note:", "warning:", "help:"]);
        let mut f = BlockStreamFilter::new(handler);
        let input = "note: see docs\nwarning: unused\nhelp: try this\nerror: fatal\n  details\n";
        let result = run_block_filter(&mut f, input, 1);
        assert!(!result.contains("note:"), "got: {}", result);
        assert!(!result.contains("warning:"), "got: {}", result);
        assert!(!result.contains("help:"), "got: {}", result);
        assert!(
            result.contains("error: fatal\n  details"),
            "got: {}",
            result
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn test_streaming_filters_both_fds_and_routes_to_correct_fd() {
        // nosemgrep: interpreter-execution
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo 'error[E0308]: type mismatch'; echo '   Compiling foo v1.0' >&2; echo '   Downloading bar v2.0' >&2; echo '   Finished dev' >&2; echo 'real error on stderr' >&2"]);

        struct CargoLikeHandler;
        impl BlockHandler for CargoLikeHandler {
            fn should_skip(&mut self, line: &str) -> bool {
                let trimmed = line.trim_start();
                trimmed.starts_with("Compiling")
                    || trimmed.starts_with("Downloading")
                    || trimmed.starts_with("Finished")
            }
            fn is_block_start(&mut self, line: &str) -> bool {
                line.starts_with("error")
            }
            fn is_block_continuation(&mut self, line: &str, _block: &[String]) -> bool {
                line.starts_with(' ')
            }
            fn format_summary(&self, _: i32, _: &str) -> Option<String> {
                None
            }
        }

        let filter = BlockStreamFilter::new(CargoLikeHandler);
        let result = run_streaming(
            &mut cmd,
            StdinMode::Null,
            FilterMode::Streaming(Box::new(filter)),
        )
        .unwrap();

        assert!(
            result.filtered.contains("error[E0308]"),
            "filtered should contain stdout errors, got: {}",
            result.filtered
        );
        assert!(
            !result.filtered.contains("Compiling"),
            "cargo noise should be filtered out, got: {}",
            result.filtered
        );
        assert!(
            !result.filtered.contains("Downloading"),
            "cargo noise should be filtered out, got: {}",
            result.filtered
        );
        assert!(
            result.raw_stderr.contains("Compiling"),
            "raw_stderr should capture all stderr lines"
        );
        assert!(
            result.raw_stderr.contains("real error on stderr"),
            "raw_stderr should capture all stderr lines"
        );
    }

    struct CountingLineHandler {
        observed: Vec<String>,
        skip_prefixes: Vec<String>,
        summary_tag: &'static str,
    }

    impl LineHandler for CountingLineHandler {
        fn should_skip(&mut self, line: &str) -> bool {
            self.skip_prefixes.iter().any(|p| line.starts_with(p))
        }

        fn observe_line(&mut self, line: &str) {
            self.observed.push(line.to_string());
        }

        fn format_summary(&self, exit_code: i32, _raw: &str) -> Option<String> {
            Some(format!(
                "{}: {} kept, exit={}\n",
                self.summary_tag,
                self.observed.len(),
                exit_code
            ))
        }
    }

    fn run_line_filter(filter: &mut dyn StreamFilter, input: &str, exit_code: i32) -> String {
        let mut out = String::new();
        for line in input.lines() {
            if let Some(s) = filter.feed_line(line) {
                out.push_str(&s);
            }
        }
        out.push_str(&filter.flush());
        if let Some(post) = filter.on_exit(exit_code, input) {
            out.push_str(&post);
        }
        out
    }

    #[test]
    fn test_line_filter_defaults_keep_all() {
        struct DefaultHandler;
        impl LineHandler for DefaultHandler {
            fn format_summary(&self, _: i32, _: &str) -> Option<String> {
                None
            }
        }
        let mut f = LineStreamFilter::new(DefaultHandler);
        let result = run_line_filter(&mut f, "a\nb\nc\n", 0);
        assert_eq!(result, "a\nb\nc\n");
    }

    #[test]
    fn test_line_filter_skip_drops_matching_lines() {
        let handler = CountingLineHandler {
            observed: Vec::new(),
            skip_prefixes: vec!["NOISE:".to_string()],
            summary_tag: "demo",
        };
        let mut f = LineStreamFilter::new(handler);
        let input = "NOISE: progress 10%\nkeep me\nNOISE: progress 90%\nalso keep\n";
        let result = run_line_filter(&mut f, input, 0);
        assert!(!result.contains("NOISE:"), "got: {}", result);
        assert!(result.contains("keep me\n"));
        assert!(result.contains("also keep\n"));
        assert!(result.contains("demo: 2 kept, exit=0\n"));
    }

    #[test]
    fn test_line_filter_summary_propagates_exit_code() {
        let handler = CountingLineHandler {
            observed: Vec::new(),
            skip_prefixes: Vec::new(),
            summary_tag: "demo",
        };
        let mut f = LineStreamFilter::new(handler);
        let result = run_line_filter(&mut f, "one\n", 42);
        assert!(result.contains("exit=42"), "got: {}", result);
    }

    #[test]
    fn test_line_filter_observe_only_called_for_kept_lines() {
        let handler = CountingLineHandler {
            observed: Vec::new(),
            skip_prefixes: vec!["DROP".to_string()],
            summary_tag: "demo",
        };
        let mut f = LineStreamFilter::new(handler);
        let result = run_line_filter(&mut f, "DROP a\nDROP b\nkeep\n", 0);
        // Only "keep" was observed, so summary says "1 kept"
        assert!(result.contains("demo: 1 kept"), "got: {}", result);
    }

    // ── Regression: pipe held open by a detached grandchild must not hang ──────────
    // Mirrors the `ng build` → `esbuild --service` case: the direct child exits but a
    // background descendant keeps the captured stdout/stderr pipe open. run_streaming
    // must return promptly with the child's output and exit code, not block on pipe EOF.
    // See docs/pr_briefs/004-pipe-eof-grandchild-hang.

    #[cfg(not(windows))]
    #[test]
    fn test_capture_only_returns_when_grandchild_holds_pipe() {
        // A grandchild `sleep 5` inherits stdout/stderr and outlives the direct child.
        // nosemgrep: interpreter-execution
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "(sleep 5) & echo out_msg; echo err_msg >&2; exit 0"]);

        let start = Instant::now();
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::CaptureOnly).unwrap();

        assert!(
            start.elapsed() < Duration::from_secs(4),
            "run_streaming must return after the direct child exits, not wait for the \
             grandchild to close the pipe (took {:?})",
            start.elapsed()
        );
        assert_eq!(result.exit_code, 0);
        assert!(
            result.raw_stdout.contains("out_msg"),
            "stdout: {:?}",
            result.raw_stdout
        );
        assert!(
            result.raw_stderr.contains("err_msg"),
            "stderr: {:?}",
            result.raw_stderr
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn test_streaming_returns_when_grandchild_holds_pipe() {
        // Same scenario but through the live-streaming filter path.
        // nosemgrep: interpreter-execution
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "(sleep 5) & echo keep_me; exit 0"]);

        let filter = LineStreamFilter::new(CountingLineHandler {
            observed: Vec::new(),
            skip_prefixes: vec![],
            summary_tag: "t",
        });
        let start = Instant::now();
        let result = run_streaming(
            &mut cmd,
            StdinMode::Null,
            FilterMode::Streaming(Box::new(filter)),
        )
        .unwrap();

        assert!(
            start.elapsed() < Duration::from_secs(4),
            "streaming run_streaming must not block on a grandchild-held pipe (took {:?})",
            start.elapsed()
        );
        assert_eq!(result.exit_code, 0);
        assert!(
            result.raw_stdout.contains("keep_me"),
            "stdout: {:?}",
            result.raw_stdout
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn test_exec_capture_returns_when_grandchild_holds_pipe() {
        // The buffered capture path (git/docker/wget/dotnet/ccusage all use exec_capture)
        // must bound the post-exit drain too: a `sleep 2` grandchild inherits the pipe and
        // outlives the direct child, so a plain `cmd.output()` would block until the pipe
        // EOFs ~2s later. Adopted from #2322's exec_capture half (the author fixed both
        // sites); our re-derived branch had only fixed run_streaming.
        // nosemgrep: interpreter-execution
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "(sleep 2) & echo out_msg; echo err_msg >&2; exit 0"]);

        let start = Instant::now();
        let result = exec_capture(&mut cmd).unwrap();

        assert!(
            start.elapsed() < Duration::from_secs(1),
            "exec_capture must return after the direct child exits, not wait for the \
             grandchild to close the pipe (took {:?})",
            start.elapsed()
        );
        assert_eq!(result.exit_code, 0);
        assert!(
            result.stdout.contains("out_msg"),
            "stdout: {:?}",
            result.stdout
        );
        assert!(
            result.stderr.contains("err_msg"),
            "stderr: {:?}",
            result.stderr
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn test_capture_only_no_truncation_on_fast_exit() {
        // A command that writes then exits immediately: output must not be lost to the
        // post-exit drain shortcut (the readers must still be drained of buffered bytes).
        // nosemgrep: interpreter-execution
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf 'line1\\nline2\\nline3\\n'; exit 0"]);

        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::CaptureOnly).unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(
            result.raw_stdout.contains("line1"),
            "stdout: {:?}",
            result.raw_stdout
        );
        assert!(result.raw_stdout.contains("line2"));
        assert!(result.raw_stdout.contains("line3"));
    }
}
