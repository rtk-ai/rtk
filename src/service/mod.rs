//! Integration-facing RTK services.

pub mod mcp;

use anyhow::{Context, Result};
use regex::Regex;
use serde::Serialize;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::Duration;

use crate::core::config::Config;
use crate::discover::registry::rewrite_command;

static SECRET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:(?:token|password|secret|api[_-]?key|authorization)\s*[:=]\s*\S+(?:\s+\S+)?|bearer\s+\S+)"
    )
    .expect("secret redaction regex must compile")
});

/// Maximum command output returned by an integration request by default.
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 1_048_576;
/// Maximum command runtime returned by an integration request by default.
pub const DEFAULT_TIMEOUT_MS: u64 = 120_000;

#[derive(Debug, Serialize)]
pub struct RewriteResult {
    pub matched: bool,
    pub rewritten_command: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RunResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
    pub rtk_args: Vec<String>,
    pub rewritten_command: Option<String>,
    pub filtered: bool,
    pub tee_path: Option<String>,
    /// Per-run savings require the raw producer stream, which is intentionally
    /// not exposed by the child command path.
    pub metrics_available: bool,
    /// Savings are unavailable because the child RTK process exposes only its
    /// filtered streams; the raw-output baseline stays inside that process.
    pub input_tokens: Option<usize>,
    pub output_tokens: Option<usize>,
    pub saved_tokens: Option<usize>,
}

/// Rewrite a shell command using the same configuration as the hooks.
pub fn rewrite(raw_command: &str) -> RewriteResult {
    let (excluded, transparent_prefixes) = Config::load()
        .map(|config| {
            (
                config.hooks.exclude_commands,
                config.hooks.transparent_prefixes,
            )
        })
        .unwrap_or_default();

    let rewritten = rewrite_command(raw_command, &excluded, &transparent_prefixes);
    RewriteResult {
        matched: rewritten.is_some(),
        rewritten_command: rewritten.map(|command| redact_sensitive(&command)),
    }
}

/// Validate an integration working directory without executing anything.
pub fn validate_cwd(cwd: Option<&Path>) -> Result<Option<PathBuf>> {
    let Some(cwd) = cwd else {
        return Ok(None);
    };

    let absolute = if cwd.is_absolute() {
        cwd.to_path_buf()
    } else {
        std::env::current_dir()
            .context("Failed to resolve current directory")?
            .join(cwd)
    };
    let canonical = absolute
        .canonicalize()
        .with_context(|| format!("Working directory does not exist: {}", absolute.display()))?;
    if !canonical.is_dir() {
        anyhow::bail!(
            "Working directory is not a directory: {}",
            canonical.display()
        );
    }
    Ok(Some(canonical))
}

/// Execute an RTK command through the current binary using typed argv.
///
/// This deliberately uses a child-process boundary: every existing command
/// router and filter keeps its current stdout/stderr and exit-code behavior,
/// while integrations receive bounded captured output.
pub fn run_filtered(
    rtk_args: &[String],
    cwd: Option<&Path>,
    timeout: Duration,
    max_output_bytes: usize,
    tee_on_failure: bool,
) -> Result<RunResult> {
    validate_rtk_args(rtk_args)?;
    let cwd = validate_cwd(cwd)?;
    let executable = std::env::current_exe().context("Failed to locate the RTK executable")?;

    // The executable comes only from current_exe(); validate_rtk_args rejects meta commands.
    // nosemgrep: dynamic-command-execution
    let mut command = Command::new(executable);
    command
        .args(rtk_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !tee_on_failure {
        command.env("RTK_TEE", "0");
    }
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }

    if debug_enabled() {
        eprintln!(
            "[rtk-debug] service.run_filtered decision=spawn args={} timeout_ms={} max_output_bytes={}",
            redact_sensitive(&rtk_args.join(" ")),
            timeout.as_millis(),
            max_output_bytes
        );
    }

    let child = command.spawn().context("Failed to spawn RTK command")?;
    let output = wait_with_timeout(child, timeout, max_output_bytes)?;
    let (stdout, stdout_truncated) = bounded_text(&output.stdout, max_output_bytes);
    let (stderr, stderr_truncated) = bounded_text(&output.stderr, max_output_bytes);
    let stdout = redact_sensitive(&stdout);
    let stderr = redact_sensitive(&stderr);
    let raw_command = rtk_args.join(" ");
    let rewritten = rewrite(&raw_command);
    let tee_path = stdout
        .lines()
        .chain(stderr.lines())
        .find_map(|line| {
            line.strip_prefix("[full output: ")
                .and_then(|value| value.strip_suffix(']'))
        })
        .map(str::to_string);

    Ok(RunResult {
        exit_code: output.status.code().unwrap_or(1),
        stdout,
        stderr,
        truncated: stdout_truncated || stderr_truncated,
        rtk_args: rtk_args.iter().map(|arg| redact_sensitive(arg)).collect(),
        rewritten_command: rewritten
            .rewritten_command
            .map(|command| redact_sensitive(&command)),
        filtered: rewritten.matched,
        tee_path,
        metrics_available: false,
        // The raw producer output is intentionally not reconstructed from the
        // argv string. RTK's tracking database remains the authoritative source
        // for savings; returning None avoids misleading per-call metrics.
        input_tokens: None,
        output_tokens: None,
        saved_tokens: None,
    })
}

fn validate_rtk_args(args: &[String]) -> Result<()> {
    let Some(first) = args.first() else {
        anyhow::bail!("rtk_args must contain at least one command");
    };
    if first.starts_with('-') {
        anyhow::bail!("rtk_args must begin with an RTK subcommand");
    }
    if matches!(
        first.as_str(),
        "mcp" | "hook" | "init" | "telemetry" | "run" | "proxy" | "pipe"
    ) {
        anyhow::bail!("rtk_args subcommand '{first}' is not supported through MCP execution");
    }
    if args.iter().any(|arg| arg.contains('\0')) {
        anyhow::bail!("rtk_args cannot contain NUL bytes");
    }
    Ok(())
}

fn bounded_text(bytes: &[u8], max_bytes: usize) -> (String, bool) {
    let limit = max_bytes.max(1);
    if bytes.len() <= limit {
        return (String::from_utf8_lossy(bytes).into_owned(), false);
    }
    (
        format!(
            "{}\n[RTK:TRUNCATED] output exceeded {} bytes",
            String::from_utf8_lossy(&bytes[..limit]),
            limit
        ),
        true,
    )
}

fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: Duration,
    max_output_bytes: usize,
) -> Result<std::process::Output> {
    let stdout_reader = child
        .stdout
        .take()
        .map(|reader| std::thread::spawn(move || drain_bounded(reader, max_output_bytes)));
    let stderr_reader = child
        .stderr
        .take()
        .map(|reader| std::thread::spawn(move || drain_bounded(reader, max_output_bytes)));
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().context("Failed waiting for RTK command")? {
            let stdout = join_pipe_reader(stdout_reader, "stdout")?;
            let stderr = join_pipe_reader(stderr_reader, "stderr")?;
            return Ok(std::process::Output {
                status,
                stdout,
                stderr,
            });
        }
        if start.elapsed() >= timeout {
            if debug_enabled() {
                eprintln!(
                    "[rtk-debug] service.run_filtered decision=timeout elapsed_ms={}",
                    start.elapsed().as_millis()
                );
            }
            let _ = child.kill();
            let _ = child.wait();
            // Killing closes the child-side handles, so both drain threads can
            // finish before the timeout error crosses the integration boundary.
            let _ = join_pipe_reader(stdout_reader, "stdout");
            let _ = join_pipe_reader(stderr_reader, "stderr");
            anyhow::bail!("RTK command timed out after {} ms", timeout.as_millis());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn drain_bounded<R: Read>(mut reader: R, max_output_bytes: usize) -> std::io::Result<Vec<u8>> {
    let retain_limit = max_output_bytes.max(1).saturating_add(1);
    let mut retained = Vec::with_capacity(retain_limit.min(64 * 1024));
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if retained.len() < retain_limit {
            let keep = (retain_limit - retained.len()).min(read);
            retained.extend_from_slice(&buffer[..keep]);
        }
    }
    Ok(retained)
}

fn join_pipe_reader(
    reader: Option<std::thread::JoinHandle<std::io::Result<Vec<u8>>>>,
    stream: &str,
) -> Result<Vec<u8>> {
    let Some(reader) = reader else {
        return Ok(Vec::new());
    };
    reader
        .join()
        .map_err(|_| anyhow::anyhow!("RTK {stream} reader thread panicked"))?
        .with_context(|| format!("Failed reading RTK {stream}"))
}

pub fn debug_enabled() -> bool {
    matches!(
        std::env::var("RTK_DEBUG").ok().as_deref(),
        Some("1" | "true" | "yes")
    )
}

/// Redact common secret-bearing argument and output forms before they cross an
/// integration boundary. The child process still receives the original argv.
pub fn redact_sensitive(value: &str) -> String {
    SECRET_RE.replace_all(value, "[REDACTED]").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_and_meta_command_argv() {
        assert!(validate_rtk_args(&[]).is_err());
        assert!(validate_rtk_args(&["mcp".to_string()]).is_err());
        assert!(validate_rtk_args(&["git".to_string(), "status".to_string()]).is_ok());
    }

    #[test]
    fn bounded_text_marks_output_truncation() {
        let (text, truncated) = bounded_text(b"abcdef", 3);
        assert!(truncated);
        assert!(text.contains("[RTK:TRUNCATED]"));
    }

    #[test]
    fn bounded_text_preserves_content_after_invalid_utf8() {
        let (text, truncated) = bounded_text(b"ab\xffcd-tail", 5);
        assert!(truncated);
        assert!(text.starts_with("ab\u{fffd}cd"));
    }

    #[test]
    fn bounded_pipe_drain_keeps_prefix_and_consumes_remainder() {
        let input = vec![b'x'; 256 * 1024];
        let retained = drain_bounded(input.as_slice(), 1024).expect("drain");
        assert_eq!(retained.len(), 1025);
        assert!(retained.iter().all(|byte| *byte == b'x'));
    }

    #[test]
    fn wait_with_timeout_drains_large_child_output_while_running() {
        #[cfg(windows)]
        let mut command = {
            let mut command = std::process::Command::new("powershell.exe");
            command.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[Console]::Out.Write('x' * 262144)",
            ]);
            command
        };
        #[cfg(unix)]
        let mut command = {
            let mut command = std::process::Command::new("sh");
            command.args(["-c", "head -c 262144 /dev/zero | tr '\\0' x"]);
            command
        };

        command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let child = command.spawn().expect("spawn large-output child");
        let output = wait_with_timeout(child, Duration::from_secs(10), 1024)
            .expect("large-output child must not deadlock on a full pipe");

        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 1025);
        assert!(output.stdout.iter().all(|byte| *byte == b'x'));
    }

    #[test]
    fn redacts_common_secret_forms() {
        let value = redact_sensitive("token=abc password=xyz Authorization=Bearer secret");
        assert!(!value.contains("abc"));
        assert!(!value.contains("xyz"));
        assert!(!value.contains("secret"));
        assert!(value.contains("[REDACTED]"));
    }

    #[test]
    fn validate_cwd_rejects_files() {
        let file = std::env::current_exe().expect("current executable");
        assert!(validate_cwd(Some(&file)).is_err());
    }
}
