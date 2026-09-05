use anyhow::{Context, Result};
use std::borrow::Cow;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;

#[cfg(test)]
use regex::Regex;

/// Read `reader` line by line, decoding each line through the console code
/// page and falling back to lossy UTF-8 (invalid bytes become U+FFFD) instead
/// of erroring.
///
/// `BufRead::lines()` returns `Err` for a non-UTF-8 line, and callers
/// commonly chain `.map_while(Result::ok)` to skip bad lines — but
/// `map_while` stops at the *first* `None`, so one invalid-UTF-8 line (e.g.
/// OEM/ANSI bytes from a non-English-locale Windows tool) silently discards
/// every line after it too, not just the bad one. This reads raw bytes and
/// never fails on the source encoding, so a garbled line still surfaces
/// instead of vanishing along with everything downstream of it.
///
/// The OEM/ANSI lines this guards against are exactly what
/// [`decode_process_output`](super::utils::decode_process_output) exists to
/// read, so the streamed path decodes them the same way the captured path
/// does rather than going straight to U+FFFD.
pub(crate) const TRUNCATED_LINE_MARKER: &str = " [rtk: line truncated]";
const STREAM_CHANNEL_CAP: usize = 32;

struct DecodedLine {
    text: String,
    /// Number of producer bytes consumed for this line, including its newline
    /// when one was present. This remains exact when the retained text is
    /// bounded, so accounting does not mistake a huge line for a tiny result.
    bytes: usize,
}

struct LossyLines<R> {
    reader: BufReader<R>,
    max_line_bytes: Option<usize>,
}

impl<R: Read> LossyLines<R> {
    fn new(reader: R, max_line_bytes: Option<usize>) -> Self {
        Self {
            reader: BufReader::new(reader),
            max_line_bytes,
        }
    }
}

impl<R: Read> Iterator for LossyLines<R> {
    type Item = DecodedLine;

    fn next(&mut self) -> Option<Self::Item> {
        let max_line_bytes = self.max_line_bytes;
        let mut retained = Vec::new();
        let mut consumed = 0usize;
        let mut truncated = false;

        loop {
            let buffer = match self.reader.fill_buf() {
                Ok(buffer) => buffer,
                Err(error) => {
                    eprintln!("rtk: stream read error: {}", error);
                    return None;
                }
            };
            if buffer.is_empty() {
                if consumed == 0 {
                    return None;
                }
                break;
            }

            let buffer_len = buffer.len();
            let before_newline = buffer
                .iter()
                .position(|byte| *byte == b'\n')
                .unwrap_or(buffer_len);
            let retain_limit = max_line_bytes.unwrap_or(usize::MAX);
            let available = retain_limit.saturating_sub(retained.len());
            let retain = before_newline.min(available);
            if retain > 0 {
                retained.extend_from_slice(&buffer[..retain]);
            }
            if retain < before_newline {
                truncated = true;
            }

            let has_newline = before_newline < buffer_len;
            let consumed_now = if has_newline {
                before_newline + 1
            } else {
                buffer_len
            };
            self.reader.consume(consumed_now);
            consumed = consumed.saturating_add(consumed_now);

            if has_newline {
                break;
            }
        }

        if !truncated && retained.last() == Some(&b'\r') {
            retained.pop();
        }
        if truncated {
            retained.extend_from_slice(TRUNCATED_LINE_MARKER.as_bytes());
        }

        Some(DecodedLine {
            text: super::utils::decode_process_output(&retained),
            bytes: consumed,
        })
    }
}

fn read_lines_lossy(reader: impl Read) -> impl Iterator<Item = String> {
    LossyLines::new(reader, None).map(|line| line.text)
}

fn read_decoded_lines(
    reader: impl Read,
    max_line_bytes: Option<usize>,
) -> impl Iterator<Item = DecodedLine> {
    LossyLines::new(reader, max_line_bytes)
}

pub trait StreamFilter {
    fn feed_line(&mut self, line: &str) -> Option<String>;
    fn flush(&mut self) -> String;
    fn on_exit(&mut self, _exit_code: i32, _raw: &str) -> Option<String> {
        None
    }
}

#[allow(dead_code)]
pub trait BlockHandler {
    /// Rewrite a raw line before it is matched or emitted — the place to
    /// strip ANSI escapes for tools that colour by default. Identity unless
    /// a handler opts in, so the rest of the pipeline sees raw bytes.
    fn normalize_line<'a>(&self, line: &'a str) -> Cow<'a, str> {
        Cow::Borrowed(line)
    }
    fn should_skip(&mut self, line: &str) -> bool;
    fn is_block_start(&mut self, line: &str) -> bool;
    fn is_block_continuation(&mut self, line: &str, block: &[String]) -> bool;
    fn format_summary(&self, exit_code: i32, raw: &str) -> Option<String>;
}

#[allow(dead_code)]
pub struct BlockStreamFilter<H: BlockHandler> {
    handler: H,
    in_block: bool,
    current_block: Vec<String>,
    blocks_emitted: usize,
}

#[allow(dead_code)]
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
        let line = self.handler.normalize_line(line);
        let line = line.as_ref();
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
    /// Stream stdout through a filter while preserving stderr byte-for-byte.
    /// This is useful for semantic adapters whose producer diagnostics must
    /// remain on stderr and must never be parsed as stdout records.
    StreamingStdout(Box<dyn StreamFilter + 'a>),
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
    /// True only when every captured byte was retained for semantic parsing.
    /// False means capture failed open and the native byte stream was replayed.
    pub capture_complete: bool,
    raw_stdout_bytes: Vec<u8>,
    raw_stderr_bytes: Vec<u8>,
    observed_output_bytes: usize,
}

impl StreamResult {
    #[cfg(test)]
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }

    pub fn write_captured_stdout(&self) -> io::Result<()> {
        let mut stdout = io::stdout().lock();
        stdout.write_all(&self.raw_stdout_bytes)?;
        stdout.flush()
    }

    pub fn write_captured_stderr(&self) -> io::Result<()> {
        let mut stderr = io::stderr().lock();
        stderr.write_all(&self.raw_stderr_bytes)?;
        stderr.flush()
    }

    /// Native stdout and stderr bytes observed before a capture failed open.
    pub fn observed_output_bytes(&self) -> usize {
        self.observed_output_bytes
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

// ISSUE #897: ChildGuard RAII prevents zombie processes that caused kernel panic
pub const RAW_CAP: usize = 10_485_760; // 10 MiB

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapturedStream {
    Stdout,
    Stderr,
}

struct CaptureChunk {
    stream: CapturedStream,
    bytes: Vec<u8>,
}

struct CompleteCapture {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Retains an ordered capture only while both streams remain within the
/// semantic-memory limit. The first overflow atomically replays everything
/// already retained and permanently forwards all subsequent chunks, so a
/// truncated buffer is never exposed as complete semantic input.
struct CaptureAccumulator {
    per_stream_cap: usize,
    stdout_len: usize,
    stderr_len: usize,
    observed_bytes: usize,
    chunks: Vec<CaptureChunk>,
    replaying: bool,
}

impl CaptureAccumulator {
    fn new(per_stream_cap: usize) -> Self {
        Self {
            per_stream_cap,
            stdout_len: 0,
            stderr_len: 0,
            observed_bytes: 0,
            chunks: Vec::new(),
            replaying: false,
        }
    }

    fn push<F>(&mut self, stream: CapturedStream, bytes: Vec<u8>, replay: &mut F) -> io::Result<()>
    where
        F: FnMut(CapturedStream, &[u8]) -> io::Result<()>,
    {
        self.observed_bytes = self.observed_bytes.saturating_add(bytes.len());
        if self.replaying {
            return replay(stream, &bytes);
        }

        let retained = match stream {
            CapturedStream::Stdout => self.stdout_len,
            CapturedStream::Stderr => self.stderr_len,
        };
        if retained.saturating_add(bytes.len()) > self.per_stream_cap {
            self.replaying = true;
            for chunk in self.chunks.drain(..) {
                replay(chunk.stream, &chunk.bytes)?;
            }
            replay(stream, &bytes)?;
            return Ok(());
        }

        match stream {
            CapturedStream::Stdout => self.stdout_len += bytes.len(),
            CapturedStream::Stderr => self.stderr_len += bytes.len(),
        }
        self.chunks.push(CaptureChunk { stream, bytes });
        Ok(())
    }

    fn fail_open<F>(&mut self, replay: &mut F) -> io::Result<()>
    where
        F: FnMut(CapturedStream, &[u8]) -> io::Result<()>,
    {
        if self.replaying {
            return Ok(());
        }
        self.replaying = true;
        for chunk in self.chunks.drain(..) {
            replay(chunk.stream, &chunk.bytes)?;
        }
        Ok(())
    }

    fn is_complete(&self) -> bool {
        !self.replaying
    }

    fn observed_bytes(&self) -> usize {
        self.observed_bytes
    }

    fn into_complete(self) -> Option<CompleteCapture> {
        if self.replaying {
            return None;
        }
        let mut stdout = Vec::with_capacity(self.stdout_len);
        let mut stderr = Vec::with_capacity(self.stderr_len);
        for chunk in self.chunks {
            match chunk.stream {
                CapturedStream::Stdout => stdout.extend_from_slice(&chunk.bytes),
                CapturedStream::Stderr => stderr.extend_from_slice(&chunk.bytes),
            }
        }
        Some(CompleteCapture { stdout, stderr })
    }
}

enum CaptureMessage {
    Chunk(CaptureChunk),
    ReadFailed(CapturedStream, io::Error),
}

const CAPTURE_QUEUE_DEPTH: usize = 8;

fn capture_channel() -> (
    mpsc::SyncSender<CaptureMessage>,
    mpsc::Receiver<CaptureMessage>,
) {
    mpsc::sync_channel(CAPTURE_QUEUE_DEPTH)
}

fn spawn_capture_thread<R: Read + Send + 'static>(
    mut reader: R,
    stream: CapturedStream,
    sender: mpsc::SyncSender<CaptureMessage>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    let chunk = CaptureChunk {
                        stream,
                        bytes: buffer[..read].to_vec(),
                    };
                    if sender.send(CaptureMessage::Chunk(chunk)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = sender.send(CaptureMessage::ReadFailed(stream, error));
                    break;
                }
            }
        }
    })
}

fn decode_captured_lines(bytes: &[u8]) -> String {
    let mut decoded = String::new();
    for line in read_lines_lossy(bytes) {
        decoded.push_str(&line);
        decoded.push('\n');
    }
    decoded
}

pub fn run_streaming(
    cmd: &mut Command,
    stdin_mode: StdinMode,
    stdout_mode: FilterMode<'_>,
) -> Result<StreamResult> {
    run_streaming_with_line_cap(cmd, stdin_mode, stdout_mode, None)
}

/// Like [`run_streaming`], but bounds the memory used for an individual
/// producer line. The producer is still drained to completion and the returned
/// byte count remains the full observed size. A bounded line gets an explicit
/// marker in the text delivered to the filter, so semantic adapters can report
/// loss without treating the prefix as complete native output.
pub fn run_streaming_with_line_cap(
    cmd: &mut Command,
    stdin_mode: StdinMode,
    stdout_mode: FilterMode<'_>,
    max_line_bytes: Option<usize>,
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
            capture_complete: true,
            raw_stdout_bytes: Vec::new(),
            raw_stderr_bytes: Vec::new(),
            observed_output_bytes: 0,
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

    let is_streaming = matches!(
        stdout_mode,
        FilterMode::Streaming(_) | FilterMode::StreamingStdout(_)
    );
    let filter_stdout_only = matches!(stdout_mode, FilterMode::StreamingStdout(_));
    let is_capture_only = matches!(stdout_mode, FilterMode::CaptureOnly);

    let mut child = ChildGuard(cmd.spawn().context("Failed to spawn process")?);

    let stdin_thread: Option<std::thread::JoinHandle<()>> = match stdin_mode {
        StdinMode::Filter(mut filter) => {
            let child_stdin = child.0.stdin.take().context("No child stdin handle")?;
            Some(std::thread::spawn(move || {
                let mut writer = BufWriter::new(child_stdin);
                let stdin_handle = io::stdin();
                for line in read_lines_lossy(stdin_handle.lock()) {
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
    let mut raw_stdout_bytes = Vec::new();
    let mut raw_stderr_bytes = Vec::new();
    let mut observed_output_bytes: usize = 0;
    let mut filtered = String::new();
    let mut capped_out = false;
    let mut capped_err = false;
    let mut capture_complete = true;
    let mut saved_filter: Option<Box<dyn StreamFilter + '_>> = None;
    let mut filter_fd_is_stderr = false;

    if is_streaming {
        enum StreamLine {
            Stdout(DecodedLine),
            Stderr(DecodedLine),
        }

        let (tx, rx) = mpsc::sync_channel(STREAM_CHANNEL_CAP);
        let tx_out = tx.clone();
        let stdout_thread = std::thread::spawn(move || {
            for line in read_decoded_lines(stdout, max_line_bytes) {
                if tx_out.send(StreamLine::Stdout(line)).is_err() {
                    break;
                }
            }
        });
        let tx_err = tx;
        let stderr_line_cap = if filter_stdout_only {
            None
        } else {
            max_line_bytes
        };
        let stderr_thread = std::thread::spawn(move || {
            for line in read_decoded_lines(stderr, stderr_line_cap) {
                if tx_err.send(StreamLine::Stderr(line)).is_err() {
                    break;
                }
            }
        });

        if let FilterMode::Streaming(mut filter) | FilterMode::StreamingStdout(mut filter) =
            stdout_mode
        {
            let stdout_handle = io::stdout();
            let mut out = stdout_handle.lock();
            let stderr_handle = io::stderr();
            let mut err_out = stderr_handle.lock();

            for msg in rx {
                let (line, line_bytes, is_stderr) = match msg {
                    StreamLine::Stderr(line) => (line.text, line.bytes, true),
                    StreamLine::Stdout(line) => (line.text, line.bytes, false),
                };
                observed_output_bytes = observed_output_bytes.saturating_add(line_bytes);
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
                if filter_stdout_only && is_stderr {
                    if writeln!(err_out, "{}", line).is_err() {
                        break;
                    }
                    continue;
                }

                filter_fd_is_stderr = is_stderr;
                if let Some(output) = filter.feed_line(&line) {
                    filtered.push_str(&output);
                    let dest: &mut dyn Write = if is_stderr { &mut err_out } else { &mut out };
                    match write!(dest, "{}", output) {
                        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => break,
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

        stdout_thread.join().ok();
        stderr_thread.join().ok();
    } else if is_capture_only {
        let (sender, receiver) = capture_channel();
        let stdout_thread = spawn_capture_thread(stdout, CapturedStream::Stdout, sender.clone());
        let stderr_thread = spawn_capture_thread(stderr, CapturedStream::Stderr, sender);
        let stdout_handle = io::stdout();
        let mut native_stdout = stdout_handle.lock();
        let stderr_handle = io::stderr();
        let mut native_stderr = stderr_handle.lock();
        let mut capture = CaptureAccumulator::new(RAW_CAP);

        {
            let mut replay = |stream: CapturedStream, bytes: &[u8]| match stream {
                CapturedStream::Stdout => native_stdout.write_all(bytes),
                CapturedStream::Stderr => native_stderr.write_all(bytes),
            };
            for message in receiver {
                match message {
                    CaptureMessage::Chunk(chunk) => {
                        capture.push(chunk.stream, chunk.bytes, &mut replay)?;
                    }
                    CaptureMessage::ReadFailed(stream, error) => {
                        eprintln!("[rtk] warning: {stream:?} capture failed: {error}");
                        capture.fail_open(&mut replay)?;
                    }
                }
            }
            if stdout_thread.join().is_err() {
                eprintln!("[rtk] warning: stdout reader thread panicked");
                capture.fail_open(&mut replay)?;
            }
            if stderr_thread.join().is_err() {
                eprintln!("[rtk] warning: stderr reader thread panicked");
                capture.fail_open(&mut replay)?;
            }
        }

        capture_complete = capture.is_complete();
        observed_output_bytes = capture.observed_bytes();
        if let Some(complete) = capture.into_complete() {
            raw_stdout = decode_captured_lines(&complete.stdout);
            raw_stderr = decode_captured_lines(&complete.stderr);
            filtered = raw_stdout.clone();
            raw_stdout_bytes = complete.stdout;
            raw_stderr_bytes = complete.stderr;
        } else {
            native_stdout.flush()?;
            native_stderr.flush()?;
        }
    } else {
        let stderr_thread = std::thread::spawn(move || -> String {
            let mut raw_err = String::new();
            let mut capped = false;
            for line in read_lines_lossy(stderr) {
                if raw_err.len() + line.len() < RAW_CAP {
                    raw_err.push_str(&line);
                    raw_err.push('\n');
                } else if !capped {
                    capped = true;
                }
            }
            raw_err
        });

        {
            let stdout_handle = io::stdout();
            let mut out = stdout_handle.lock();

            match stdout_mode {
                FilterMode::Passthrough => unreachable!("handled by early-return above"),
                FilterMode::Streaming(_) | FilterMode::StreamingStdout(_) => {
                    unreachable!("handled by is_streaming branch")
                }
                FilterMode::Buffered(filter_fn) => {
                    for line in read_lines_lossy(stdout) {
                        if raw_stdout.len() + line.len() < RAW_CAP {
                            raw_stdout.push_str(&line);
                            raw_stdout.push('\n');
                        } else if !capped_out {
                            capped_out = true;
                            eprintln!(
                                "[rtk] warning: output exceeds 10 MiB — filter input truncated"
                            );
                        }
                    }
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
                    for line in read_lines_lossy(stdout) {
                        if raw_stdout.len() + line.len() < RAW_CAP {
                            raw_stdout.push_str(&line);
                            raw_stdout.push('\n');
                        } else if !capped_out {
                            capped_out = true;
                            eprintln!(
                                "[rtk] warning: output exceeds 10 MiB — filter input truncated"
                            );
                        }
                    }
                    filtered = raw_stdout.clone();
                }
            }
        }

        raw_stderr = stderr_thread.join().unwrap_or_else(|e| {
            eprintln!("[rtk] warning: stderr reader thread panicked: {:?}", e);
            String::new()
        });
        capture_complete = !(capped_out || capped_err);
        raw_stdout_bytes = raw_stdout.as_bytes().to_vec();
        raw_stderr_bytes = raw_stderr.as_bytes().to_vec();
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
        capture_complete,
        raw_stdout_bytes,
        raw_stderr_bytes,
        observed_output_bytes,
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
    capture(cmd)
}

/// Like [`exec_capture`] but inherits stdin so a wrapped engine can read a piped stdin.
pub fn exec_capture_stdin(cmd: &mut Command) -> Result<CaptureResult> {
    cmd.stdin(Stdio::inherit());
    capture(cmd)
}

/// Run `cmd` to completion, decode what it wrote, and report the exit code.
///
/// A process killed by a signal has no exit code of its own, and returning
/// only the synthesized `128 + signal` hides why it died. `exit_code_from_output`
/// announces that on stderr, so callers moving here from a hand-rolled
/// `.output()` keep the diagnostic instead of losing it. The program name is
/// used as the label so no call site has to pass one.
fn capture(cmd: &mut Command) -> Result<CaptureResult> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    let output = cmd.output().context("Failed to execute command")?;
    let exit_code = super::utils::exit_code_from_output(&output, &program);
    Ok(CaptureResult {
        stdout: super::utils::decode_process_output(&output.stdout),
        stderr: super::utils::decode_process_output(&output.stderr),
        exit_code,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn test_read_lines_lossy_preserves_lines_after_invalid_utf8() {
        // Line 2 contains a lone 0xE3 byte (the cp850 case from the bug report).
        // `BufRead::lines().map_while(Result::ok)` would stop at that Err and
        // silently lose line 3 as well — not just the bad byte.
        let mut data = Vec::new();
        data.extend_from_slice(b"ERRO A: ascii\n");
        data.extend_from_slice(&[b'E', b'R', b'R', b'O', b' ', b'B', b':', b' ', 0xE3, b'\n']);
        data.extend_from_slice(b"ERRO C: ascii again\n");

        let lines: Vec<String> = read_lines_lossy(data.as_slice()).collect();
        assert_eq!(lines.len(), 3, "got: {:?}", lines);
        assert_eq!(lines[0], "ERRO A: ascii");
        assert!(lines[1].starts_with("ERRO B: "), "got: {:?}", lines[1]);
        assert!(lines[1].contains('\u{FFFD}'), "got: {:?}", lines[1]);
        assert_eq!(lines[2], "ERRO C: ascii again");
    }

    #[test]
    fn test_read_lines_lossy_strips_crlf() {
        let lines: Vec<String> = read_lines_lossy(&b"a\r\nb\n"[..]).collect();
        assert_eq!(lines, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn test_read_lines_lossy_no_trailing_newline() {
        let lines: Vec<String> = read_lines_lossy(&b"only line"[..]).collect();
        assert_eq!(lines, vec!["only line".to_string()]);
    }

    #[test]
    fn bounded_lines_keep_the_prefix_marker_and_full_byte_count() {
        let input = format!("prefix{}\nnext\n", "x".repeat(32));
        let mut lines = LossyLines::new(input.as_bytes(), Some(16));

        let first = lines.next().expect("first line");
        assert_eq!(first.bytes, 39);
        assert!(first.text.starts_with("prefixxxxxxxxxxx"));
        assert!(first.text.ends_with(TRUNCATED_LINE_MARKER));

        let second = lines.next().expect("second line");
        assert_eq!(second.text, "next");
        assert_eq!(second.bytes, 5);
        assert!(lines.next().is_none());
    }

    #[test]
    fn test_read_lines_lossy_empty_input() {
        let lines: Vec<String> = read_lines_lossy(&b""[..]).collect();
        assert!(lines.is_empty());
    }

    /// A `Read` that yields some good lines, then a genuine I/O error --
    /// distinct from clean EOF (`Ok(0)`). Before this fix, `Ok(0) | Err(_)`
    /// treated both the same way, silently truncating output on a real read
    /// failure instead of surfacing it.
    struct FailingReader {
        data: std::io::Cursor<Vec<u8>>,
        failed: bool,
    }

    impl Read for FailingReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.data.position() as usize >= self.data.get_ref().len() {
                if !self.failed {
                    self.failed = true;
                    return Err(io::Error::other("simulated read failure"));
                }
                return Ok(0);
            }
            std::io::Read::read(&mut self.data, buf)
        }
    }

    #[test]
    fn test_read_lines_lossy_stops_on_io_error_without_panicking() {
        let reader = FailingReader {
            data: std::io::Cursor::new(b"line one\nline two\n".to_vec()),
            failed: false,
        };
        // The two good lines are still yielded; the simulated failure after
        // them must not panic or hang -- it just ends the iterator, same as
        // clean EOF would, but via the Err(_) arm instead of Ok(0).
        let lines: Vec<String> = read_lines_lossy(reader).collect();
        assert_eq!(lines, vec!["line one".to_string(), "line two".to_string()]);
    }

    struct LineFilter<F: FnMut(&str) -> Option<String>> {
        f: F,
    }

    fn success_command() -> Command {
        #[cfg(windows)]
        {
            let mut command = Command::new("cmd");
            command.args(["/C", "exit", "0"]);
            command
        }
        #[cfg(not(windows))]
        {
            Command::new("true")
        }
    }

    fn failure_command() -> Command {
        #[cfg(windows)]
        {
            let mut command = Command::new("cmd");
            command.args(["/C", "exit", "1"]);
            command
        }
        #[cfg(not(windows))]
        {
            Command::new("false")
        }
    }

    fn echo_command(value: &str) -> Command {
        #[cfg(windows)]
        {
            let mut command = Command::new("cmd");
            command.args(["/C", "echo", value]);
            command
        }
        #[cfg(not(windows))]
        {
            let mut command = Command::new("echo");
            command.arg(value);
            command
        }
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
    fn semantic_capture_queue_applies_backpressure() {
        let (sender, _receiver) = capture_channel();
        for _ in 0..CAPTURE_QUEUE_DEPTH {
            sender
                .try_send(CaptureMessage::Chunk(CaptureChunk {
                    stream: CapturedStream::Stdout,
                    bytes: vec![b'x'],
                }))
                .unwrap();
        }

        assert!(matches!(
            sender.try_send(CaptureMessage::Chunk(CaptureChunk {
                stream: CapturedStream::Stdout,
                bytes: vec![b'x'],
            })),
            Err(mpsc::TrySendError::Full(_))
        ));
    }

    #[test]
    fn capture_over_raw_cap_stdout_replays_every_byte_and_disables_semantics() {
        let mut capture = CaptureAccumulator::new(RAW_CAP);
        let first = vec![b'a'; RAW_CAP];
        let tail = b"stdout-tail".to_vec();
        let mut replayed = Vec::new();

        capture
            .push(CapturedStream::Stdout, first, &mut |stream, bytes| {
                replayed.push((stream, bytes.to_vec()));
                Ok(())
            })
            .unwrap();
        capture
            .push(
                CapturedStream::Stdout,
                tail.clone(),
                &mut |stream, bytes| {
                    replayed.push((stream, bytes.to_vec()));
                    Ok(())
                },
            )
            .unwrap();

        assert!(!capture.is_complete());
        assert!(capture.into_complete().is_none());
        assert_eq!(replayed.len(), 2);
        assert_eq!(replayed[0].0, CapturedStream::Stdout);
        assert_eq!(replayed[0].1.len(), RAW_CAP);
        assert!(replayed[0].1.iter().all(|byte| *byte == b'a'));
        assert_eq!(replayed[1], (CapturedStream::Stdout, tail));
    }

    #[test]
    fn capture_over_raw_cap_stderr_replays_every_byte_and_disables_semantics() {
        let mut capture = CaptureAccumulator::new(RAW_CAP);
        let first = vec![b'e'; RAW_CAP];
        let tail = b"stderr-tail".to_vec();
        let mut replayed = Vec::new();

        capture
            .push(CapturedStream::Stderr, first, &mut |stream, bytes| {
                replayed.push((stream, bytes.to_vec()));
                Ok(())
            })
            .unwrap();
        capture
            .push(
                CapturedStream::Stderr,
                tail.clone(),
                &mut |stream, bytes| {
                    replayed.push((stream, bytes.to_vec()));
                    Ok(())
                },
            )
            .unwrap();

        assert!(!capture.is_complete());
        assert!(capture.into_complete().is_none());
        assert_eq!(replayed.len(), 2);
        assert_eq!(replayed[0].0, CapturedStream::Stderr);
        assert_eq!(replayed[0].1.len(), RAW_CAP);
        assert!(replayed[0].1.iter().all(|byte| *byte == b'e'));
        assert_eq!(replayed[1], (CapturedStream::Stderr, tail));
    }

    #[test]
    fn capture_one_line_over_raw_cap_is_replayed_whole() {
        let mut capture = CaptureAccumulator::new(RAW_CAP);
        let one_long_line = vec![b'L'; RAW_CAP + 1];
        let mut replayed = Vec::new();

        capture
            .push(
                CapturedStream::Stdout,
                one_long_line,
                &mut |stream, bytes| {
                    replayed.push((stream, bytes.to_vec()));
                    Ok(())
                },
            )
            .unwrap();

        assert!(!capture.is_complete());
        assert_eq!(replayed.len(), 1);
        assert_eq!(replayed[0].0, CapturedStream::Stdout);
        assert_eq!(replayed[0].1.len(), RAW_CAP + 1);
        assert!(replayed[0].1.iter().all(|byte| *byte == b'L'));
    }

    #[test]
    fn capture_fail_open_replays_mixed_streams_in_observed_order() {
        let mut capture = CaptureAccumulator::new(4);
        let mut replayed = Vec::new();
        for (stream, bytes) in [
            (CapturedStream::Stdout, b"out".to_vec()),
            (CapturedStream::Stderr, b"err".to_vec()),
            (CapturedStream::Stdout, b"overflow".to_vec()),
            (CapturedStream::Stderr, b"after".to_vec()),
        ] {
            capture
                .push(stream, bytes, &mut |stream, bytes| {
                    replayed.push((stream, bytes.to_vec()));
                    Ok(())
                })
                .unwrap();
        }

        assert_eq!(
            replayed,
            vec![
                (CapturedStream::Stdout, b"out".to_vec()),
                (CapturedStream::Stderr, b"err".to_vec()),
                (CapturedStream::Stdout, b"overflow".to_vec()),
                (CapturedStream::Stderr, b"after".to_vec()),
            ]
        );
        assert!(!capture.is_complete());
        assert!(capture.into_complete().is_none());
    }

    #[test]
    fn capture_read_failure_keeps_count_for_bytes_seen_before_fail_open() {
        let mut capture = CaptureAccumulator::new(64);
        let bytes = b"read-before-error".to_vec();
        let mut replayed = Vec::new();

        capture
            .push(
                CapturedStream::Stdout,
                bytes.clone(),
                &mut |stream, bytes| {
                    replayed.push((stream, bytes.to_vec()));
                    Ok(())
                },
            )
            .unwrap();
        capture
            .fail_open(&mut |stream, bytes| {
                replayed.push((stream, bytes.to_vec()));
                Ok(())
            })
            .unwrap();

        assert_eq!(capture.observed_bytes(), bytes.len());
        assert_eq!(replayed, vec![(CapturedStream::Stdout, bytes)]);
        assert!(!capture.is_complete());
    }

    #[test]
    fn test_exit_code_zero() {
        let status = success_command().status().unwrap();
        assert_eq!(status_to_exit_code(status), 0);
    }

    #[test]
    fn test_exit_code_nonzero() {
        let status = failure_command().status().unwrap();
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
    fn test_exec_capture_decodes_and_reports_exit_code() {
        let captured = exec_capture(&mut failure_command()).expect("spawn");
        assert_eq!(captured.exit_code, 1);
        assert!(!captured.success());
    }

    /// A signal-killed child keeps the `128 + signal` code that
    /// `exit_code_from_output` reports, so callers that moved off a hand-rolled
    /// `.output()` neither lose the code nor the stderr diagnostic with it.
    #[cfg(unix)]
    #[test]
    fn test_exec_capture_reports_signal_exit_code() {
        // `kill -TERM $$` makes the shell terminate itself by signal 15.
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("kill -TERM $$");
        let captured = exec_capture(&mut cmd).expect("spawn");
        assert_eq!(captured.exit_code, 128 + 15);
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
            capture_complete: true,
            raw_stdout_bytes: Vec::new(),
            raw_stderr_bytes: Vec::new(),
            observed_output_bytes: 0,
        };
        assert!(r.success());
        assert!(r.capture_complete);
        r.write_captured_stdout().unwrap();
        r.write_captured_stderr().unwrap();
    }

    #[test]
    fn test_stream_result_failure() {
        let r = StreamResult {
            exit_code: 1,
            raw: String::new(),
            raw_stdout: String::new(),
            raw_stderr: String::new(),
            filtered: String::new(),
            capture_complete: true,
            raw_stdout_bytes: Vec::new(),
            raw_stderr_bytes: Vec::new(),
            observed_output_bytes: 0,
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
            capture_complete: true,
            raw_stdout_bytes: Vec::new(),
            raw_stderr_bytes: Vec::new(),
            observed_output_bytes: 0,
        };
        assert!(!r.success());
    }

    #[test]
    fn test_run_streaming_passthrough_echo() {
        let mut cmd = echo_command("hello");
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::Passthrough).unwrap();
        assert_eq!(result.exit_code, 0);
        // Passthrough inherits TTY — raw/filtered are empty
        assert!(result.raw.is_empty());
    }

    #[cfg(not(windows))]
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
        let mut cmd = success_command();
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::Passthrough).unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.success());
    }

    #[test]
    fn test_run_streaming_exit_code_one() {
        let mut cmd = failure_command();
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

    #[cfg(not(windows))]
    #[test]
    fn test_run_streaming_stdout_over_cap_never_exposes_truncated_semantic_input() {
        // nosemgrep: interpreter-execution
        let mut cmd = Command::new("sh");
        // ~11 MiB of 80-char lines (fast: fewer lines than `yes | head -6M`)
        cmd.args([
            "-c",
            "dd if=/dev/zero bs=1024 count=11264 2>/dev/null | tr '\\0' 'a' | fold -w 80",
        ]);
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::CaptureOnly).unwrap();
        assert!(!result.capture_complete);
        assert!(result.raw.is_empty());
        assert!(result.raw_stdout.is_empty());
    }

    #[cfg(not(windows))]
    #[test]
    fn test_run_streaming_stderr_over_cap_never_exposes_truncated_semantic_input() {
        // nosemgrep: interpreter-execution
        let mut cmd = Command::new("sh");
        // ~11 MiB on stderr, nothing on stdout
        cmd.args([
            "-c",
            "dd if=/dev/zero bs=1024 count=11264 2>/dev/null | tr '\\0' 'a' | fold -w 80 1>&2",
        ]);
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::CaptureOnly).unwrap();
        assert!(!result.capture_complete);
        assert!(result.raw.is_empty());
        assert!(result.raw_stderr.is_empty());
    }

    #[test]
    fn test_child_guard_prevents_zombie() {
        let mut cmd = success_command();
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::CaptureOnly);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().exit_code, 0);
    }

    #[test]
    fn test_run_streaming_null_stdin_cat() {
        let mut cmd = success_command();
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::Passthrough).unwrap();
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn test_run_streaming_raw_contains_stdout() {
        let mut cmd = echo_command("test_output_xyz");
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::CaptureOnly).unwrap();
        assert!(result.raw.contains("test_output_xyz"));
    }

    #[test]
    fn test_run_streaming_capture_only_filtered_equals_raw() {
        let mut cmd = echo_command("check_equality");
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::CaptureOnly).unwrap();
        assert_eq!(result.filtered.trim(), result.raw_stdout.trim());
    }

    #[test]
    fn test_exec_capture_success() {
        let mut cmd = echo_command("hello_capture");
        let result = exec_capture(&mut cmd).unwrap();
        assert!(result.success());
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("hello_capture"));
    }

    #[test]
    fn test_exec_capture_failure() {
        let mut cmd = failure_command();
        let result = exec_capture(&mut cmd).unwrap();
        assert!(!result.success());
        assert_eq!(result.exit_code, 1);
    }

    #[cfg(not(windows))]
    #[test]
    fn test_exec_capture_stderr() {
        // nosemgrep: interpreter-execution
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo err_msg >&2"]);
        let result = exec_capture(&mut cmd).unwrap();
        assert!(result.stderr.contains("err_msg"));
    }

    #[cfg(not(windows))]
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

    struct UpperHandler;

    impl BlockHandler for UpperHandler {
        fn normalize_line<'a>(&self, line: &'a str) -> Cow<'a, str> {
            Cow::Owned(line.to_uppercase())
        }
        fn should_skip(&mut self, _line: &str) -> bool {
            false
        }
        fn is_block_start(&mut self, line: &str) -> bool {
            line.starts_with("ERR")
        }
        fn is_block_continuation(&mut self, line: &str, _block: &[String]) -> bool {
            line.starts_with("  ")
        }
        fn format_summary(&self, _exit_code: i32, _raw: &str) -> Option<String> {
            None
        }
    }

    #[test]
    fn block_handler_normalize_line_feeds_matching_and_emission() {
        // Both the match and the emitted block see the normalized line.
        let mut f = BlockStreamFilter::new(UpperHandler);
        let out = run_block_filter(&mut f, "err: one\n  detail\nnoise\n", 0);
        assert_eq!(out, "ERR: ONE\n  DETAIL\n");
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
}
