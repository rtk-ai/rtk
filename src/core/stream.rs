use anyhow::{Context, Result};
use regex::Regex;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::process::{Command, Stdio};

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

#[allow(dead_code)] // available for command modules; currently used in tests only
pub struct RegexBlockFilter {
    start_re: Regex,
    skip_prefixes: Vec<String>,
    tool_name: String,
    block_count: usize,
}

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

    #[allow(dead_code)]
    pub fn skip_prefix(mut self, prefix: &str) -> Self {
        self.skip_prefixes.push(prefix.to_string());
        self
    }

    #[allow(dead_code)]
    pub fn skip_prefixes(mut self, prefixes: &[&str]) -> Self {
        self.skip_prefixes
            .extend(prefixes.iter().map(|s| s.to_string()));
        self
    }
}

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

#[allow(dead_code)] // test utility: wraps closures as StreamFilter
pub struct LineFilter<F: FnMut(&str) -> Option<String>> {
    f: F,
}

#[allow(dead_code)]
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

pub enum FilterMode<'a> {
    Streaming(Box<dyn StreamFilter + 'a>),
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
    pub filtered: String,
}

impl StreamResult {
    #[allow(dead_code)]
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

// ISSUE #897: ChildGuard RAII prevents zombie processes that caused kernel panic
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

    let live_stderr = matches!(stdout_mode, FilterMode::Streaming(_));

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

    let stderr = child.0.stderr.take().context("No child stderr handle")?;
    let stderr_thread = std::thread::spawn(move || -> String {
        let mut raw_err = String::new();
        if live_stderr {
            let stderr_out = io::stderr();
            let mut err_out = stderr_out.lock();
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                writeln!(err_out, "{}", line).ok();
                raw_err.push_str(&line);
                raw_err.push('\n');
            }
        } else {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                raw_err.push_str(&line);
                raw_err.push('\n');
            }
        }
        raw_err
    });

    let stdout = child.0.stdout.take().context("No child stdout handle")?;
    const RAW_CAP: usize = 10_485_760;
    let mut raw_stdout = String::new();
    let mut filtered = String::new();
    let mut capped = false;
    let mut saved_filter: Option<Box<dyn StreamFilter + '_>> = None;

    {
        let stdout_handle = io::stdout();
        let mut out = stdout_handle.lock();

        match stdout_mode {
            FilterMode::Passthrough => unreachable!("handled by early-return above"),
            FilterMode::Streaming(mut filter) => {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if raw_stdout.len() + line.len() < RAW_CAP {
                        raw_stdout.push_str(&line);
                        raw_stdout.push('\n');
                    } else if !capped {
                        capped = true;
                        eprintln!("[rtk] warning: output exceeds 10 MiB — filter input truncated");
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
                match write!(out, "{}", tail) {
                    Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {}
                    Err(e) => return Err(e.into()),
                    Ok(_) => {}
                }
                saved_filter = Some(filter);
            }
            FilterMode::Buffered(filter_fn) => {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if raw_stdout.len() + line.len() < RAW_CAP {
                        raw_stdout.push_str(&line);
                        raw_stdout.push('\n');
                    } else if !capped {
                        capped = true;
                        eprintln!("[rtk] warning: output exceeds 10 MiB — filter input truncated");
                    }
                }
                filtered = filter_fn(&raw_stdout);
                match write!(out, "{}", filtered) {
                    Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {}
                    Err(e) => return Err(e.into()),
                    Ok(_) => {}
                }
            }
            FilterMode::CaptureOnly => {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if raw_stdout.len() + line.len() < RAW_CAP {
                        raw_stdout.push_str(&line);
                        raw_stdout.push('\n');
                    } else if !capped {
                        capped = true;
                        eprintln!("[rtk] warning: output exceeds 10 MiB — filter input truncated");
                    }
                }
                filtered = raw_stdout.clone();
            }
        }
    }

    let raw_stderr = stderr_thread.join().unwrap_or_else(|e| {
        eprintln!("[rtk] warning: stderr reader thread panicked: {:?}", e);
        String::new()
    });
    if let Some(t) = stdin_thread {
        t.join().ok();
    }

    let status = child.0.wait().context("Failed to wait for child")?;
    let exit_code = status_to_exit_code(status);
    let raw = format!("{}{}", raw_stdout, raw_stderr);

    if let Some(mut f) = saved_filter {
        if let Some(post) = f.on_exit(exit_code, &raw) {
            filtered.push_str(&post);
            print!("{}", post);
        }
    }

    Ok(StreamResult {
        exit_code,
        raw,
        raw_stdout,
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
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo err_msg >&2"]);
        let result = exec_capture(&mut cmd).unwrap();
        assert!(result.stderr.contains("err_msg"));
    }

    #[test]
    fn test_exec_capture_combined() {
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
}
