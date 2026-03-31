use anyhow::{Context, Result};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::process::{Command, Stdio};

pub trait StreamFilter {
    fn feed_line(&mut self, line: &str) -> Option<String>;
    fn flush(&mut self) -> String;
}

pub trait StdinFilter: Send {
    fn feed_line(&mut self, line: &str) -> Option<String>;
    fn flush(&mut self) -> String;
}

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

pub enum FilterMode {
    Streaming(Box<dyn StreamFilter>),
    Buffered(fn(&str) -> String),
    Passthrough,
}

#[allow(dead_code)]
pub enum StdinMode {
    Inherit,
    Filter(Box<dyn StdinFilter + Send>),
    Null,
}

pub struct StreamResult {
    pub exit_code: i32,
    pub raw: String,
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
    stdout_mode: FilterMode,
) -> Result<StreamResult> {
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
        let stderr_out = io::stderr();
        let mut err_out = stderr_out.lock();
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            writeln!(err_out, "{}", line).ok();
            raw_err.push_str(&line);
            raw_err.push('\n');
        }
        raw_err
    });

    let stdout = child.0.stdout.take().context("No child stdout handle")?;
    const RAW_CAP: usize = 1_048_576;
    let mut raw_stdout = String::new();
    let mut filtered = String::new();

    {
        let stdout_handle = io::stdout();
        let mut out = stdout_handle.lock();

        match stdout_mode {
            FilterMode::Passthrough => {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
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
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
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
                match write!(out, "{}", tail) {
                    Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {}
                    Err(e) => return Err(e.into()),
                    Ok(_) => {}
                }
            }
            FilterMode::Buffered(filter_fn) => {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
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
    }

    let raw_stderr = stderr_thread.join().unwrap_or_else(|_| String::new());
    if let Some(t) = stdin_thread {
        t.join().ok();
    }

    let status = child.0.wait().context("Failed to wait for child")?;

    Ok(StreamResult {
        exit_code: status_to_exit_code(status),
        raw: format!("{}{}", raw_stdout, raw_stderr),
        filtered,
    })
}

#[cfg(test)]
mod tests {
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
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "yes | head -600000"]);
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::Passthrough).unwrap();
        assert!(
            result.raw.len() <= 1_048_576 + 100,
            "raw should be capped at ~1 MiB, got {} bytes",
            result.raw.len()
        );
        assert!(
            result.raw.len() > 100_000,
            "Should have captured significant data"
        );
    }

    #[test]
    fn test_child_guard_prevents_zombie() {
        let mut cmd = Command::new("true");
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::Passthrough);
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
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::Passthrough).unwrap();
        assert!(result.raw.contains("test_output_xyz"));
    }

    #[test]
    fn test_run_streaming_filtered_equals_raw_in_passthrough() {
        let mut cmd = Command::new("echo");
        cmd.arg("check_equality");
        let result = run_streaming(&mut cmd, StdinMode::Null, FilterMode::Passthrough).unwrap();
        assert_eq!(result.filtered.trim(), result.raw.trim());
    }
}
