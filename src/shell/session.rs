//! Persistent session mode: `rtk-shell` invoked with no arguments.
//!
//! Starts an interactive, rtk-managed shell session: rtk owns the
//! read-eval-print loop (prompt, line editing/history are delegated to the
//! backing shell/PTY as appropriate), classifies each line the user types
//! via [`shell::dispatch`](crate::shell::dispatch), and routes
//! [`Filterable`](crate::shell::dispatch::SegmentClassification::Filterable)
//! segments through rtk's filtered-execution path while
//! [`Forward`](crate::shell::dispatch::SegmentClassification::Forward)
//! segments run unmodified in the backing shell — all commands in the
//! session share one `session_id` for tracking/correlation
//! (see [`core::tracking`](crate::core::tracking)).
//!
//! # Execution model
//!
//! Exactly one backing shell process (bash/zsh/sh) is spawned for the
//! lifetime of the session, with piped stdin/stdout/stderr. Every classified
//! segment — whether [`Filterable`] or [`Forward`] — sends its *original,
//! untouched* text to that same backing shell process to execute, so cwd,
//! shell variables, aliases, and job control state carry over between
//! commands exactly as they would in a normal interactive shell. For
//! [`Filterable`] segments, the rewritten `rtk ...` form is used only to
//! select which RTK filter to apply to the captured output afterwards — see
//! [`Session::exec_filterable_in_backing_shell`] and
//! [`Session::apply_ecosystem_filter`].
//!
//! Completion of each command is detected via a fresh, randomly generated
//! nonce sentinel (never a fixed string, since fixed text can collide with
//! a command's own output) echoed by the backing shell immediately after the
//! command, carrying the command's exit code. stdout/stderr are drained on
//! dedicated threads so a chatty stderr stream can never block a quiet
//! stdout stream (or vice versa) — see [`drain_until_sentinel`]. Bytes read
//! by those threads are buffered only (never written to the real
//! stdout/stderr) until the sentinel has been recognized and stripped, so
//! the internal completion marker can never leak to the terminal; this
//! trades true real-time incremental streaming (e.g. watching `docker
//! build` progress line-by-line as it happens) for correctness — a v1
//! tradeoff, not a bug — see [`spawn_drain_thread`].
//!
//! SIGINT/SIGTERM received by the rtk-shell process are forwarded to the
//! backing shell's process group so Ctrl-C interrupts the running command
//! rather than the rtk-shell wrapper itself (mirroring
//! `Commands::Proxy`'s signal-forwarding pattern in `src/main.rs`).
//!
//! Multi-line constructs (heredocs, unterminated quotes, trailing backslash
//! continuations) are detected via [`discover::lexer`](crate::discover::lexer)
//! and buffered raw, unfiltered and untokenized, until the construct closes;
//! only then is the whole buffered block forwarded to the backing shell.

use std::fmt::Write as _;
use std::io::{BufRead, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::core::config::{Config, ShellConfig};
use crate::core::tracking::TimedExecution;
use crate::discover::lexer;
use crate::shell::dispatch::{self, SegmentClassification};

/// Default per-command hang-detection timeout. Generous enough not to fire
/// on legitimately slow builds/tests, but bounded so a wedged backing shell
/// (or a command that never terminates and never gets interactive input)
/// can't block the session forever in automated contexts.
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(300);

/// Cap on captured stdout/stderr bytes retained for tracking purposes,
/// matching `Commands::Proxy`'s `CAP` in `src/main.rs`. Output beyond this is
/// still streamed to the real stdout/stderr live, just not retained.
const CAPTURE_CAP: usize = 1_048_576;

/// A single persistent rtk-shell session.
///
/// Owns the session id used to correlate every command executed within this
/// session in the tracking database, plus the resolved shell configuration
/// for the session's lifetime.
pub struct Session {
    /// Unique id correlating every command tracked during this session (see
    /// `commands.session_id` in [`core::tracking`](crate::core::tracking)).
    pub session_id: String,
    /// Resolved shell configuration for this session (backing shell
    /// override, minimal PS1, mode-3 swap heuristics).
    pub config: ShellConfig,

    /// The backing shell child process, kept alive for the session's
    /// lifetime. `None` until the first command is run (lazily spawned so
    /// constructing a `Session` never has side effects).
    backing: Option<BackingShell>,
    /// Exit code of the last command executed in the session.
    last_exit_code: i32,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("session_id", &self.session_id)
            .field("config", &self.config)
            .field("last_exit_code", &self.last_exit_code)
            .finish_non_exhaustive()
    }
}

/// A live backing shell process plus the plumbing needed to drive it:
/// buffered stdin writer, and channels fed by the dedicated stdout/stderr
/// draining threads.
struct BackingShell {
    child: Child,
    stdin: ChildStdin,
    stdout_rx: mpsc::Receiver<DrainEvent>,
    stderr_rx: mpsc::Receiver<DrainEvent>,
}

/// One event from a stdout/stderr draining thread: either a captured chunk
/// or EOF. Chunks are buffered only — see [`spawn_drain_thread`] for why
/// nothing is written to the real stdout/stderr from the drain thread
/// itself.
enum DrainEvent {
    Chunk(Vec<u8>),
    Eof,
}

/// PID of the currently-running backing shell, used by the SIGINT/SIGTERM
/// handler to forward signals (mirrors `Commands::Proxy`'s
/// `PROXY_CHILD_PID` in `src/main.rs`).
static BACKING_SHELL_PID: AtomicU32 = AtomicU32::new(0);

#[cfg(unix)]
#[allow(unsafe_code)]
fn install_signal_forwarding() {
    unsafe extern "C" fn handle_signal(sig: libc::c_int) {
        let pid = BACKING_SHELL_PID.load(Ordering::SeqCst);
        if pid != 0 {
            libc::kill(pid as libc::pid_t, sig);
        }
    }
    // nosemgrep: unsafe-block
    unsafe {
        libc::signal(
            libc::SIGINT,
            handle_signal as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGTERM,
            handle_signal as *const () as libc::sighandler_t,
        );
    }
}

#[cfg(not(unix))]
fn install_signal_forwarding() {}

impl Session {
    /// Create a new session with a freshly generated `session_id`, using the
    /// given resolved [`ShellConfig`].
    ///
    /// Id generation is real (not a stub): it is plumbing shared by every
    /// consumer of this struct, not part of the one-shot/session execution
    /// logic deferred to [`run`](Self::run)/[`run_line`](Self::run_line).
    pub fn new(config: ShellConfig) -> Self {
        Self {
            session_id: generate_session_id(),
            config,
            backing: None,
            last_exit_code: 0,
        }
    }

    /// Run the interactive read-eval-print loop until the user exits the
    /// session (e.g. `exit`, Ctrl-D) or the backing shell terminates.
    ///
    /// Returns the process exit code to propagate to the OS (the exit code
    /// of the last command executed in the session, following
    /// [`core::utils::exit_code_from_status`](crate::core::utils::exit_code_from_status)
    /// conventions).
    pub fn run(&mut self) -> Result<i32> {
        install_signal_forwarding();
        self.ensure_backing_shell()?;

        let stdin = std::io::stdin();
        let mut lines = stdin.lock().lines();
        let mut pending: Option<String> = None;

        loop {
            let line = match lines.next() {
                Some(Ok(l)) => l,
                Some(Err(e)) => return Err(e).context("Failed to read rtk-shell stdin"),
                None => break, // EOF (Ctrl-D)
            };

            let combined = match pending.take() {
                Some(mut buf) => {
                    buf.push('\n');
                    buf.push_str(&line);
                    buf
                }
                None => line,
            };

            if needs_more_input(&combined) {
                pending = Some(combined);
                continue;
            }

            if combined.trim() == "exit" || combined.trim() == "logout" {
                break;
            }

            self.last_exit_code = self.run_line(&combined)?;
        }

        self.shutdown_backing_shell();
        Ok(self.last_exit_code)
    }

    /// Classify and execute a single line typed at the session prompt,
    /// without exiting the session. Exposed separately from [`run`](Self::run)
    /// so callers (and tests) can drive the loop one line at a time.
    ///
    /// Returns the exit code of the executed line.
    pub fn run_line(&mut self, line: &str) -> Result<i32> {
        self.ensure_backing_shell()?;

        // Multi-line constructs (heredocs, unterminated quotes, trailing
        // backslash continuations) must reach the backing shell raw and
        // whole — never tokenized/classified/split. This also covers lines
        // handed to run_line() directly (e.g. by tests) that already
        // contain embedded newlines from an assembled heredoc block.
        if needs_more_input(line) {
            let code = self.exec_raw_in_backing_shell(line)?;
            self.last_exit_code = code;
            return Ok(code);
        }

        let segments = dispatch::classify_line(line);
        if segments.is_empty() {
            return Ok(0);
        }

        let mut code = 0;
        for segment in segments {
            code = match segment {
                SegmentClassification::Filterable {
                    original,
                    rewritten,
                } => self.exec_filterable_in_backing_shell(&original, &rewritten, line)?,
                SegmentClassification::Forward(raw) => {
                    self.exec_forward_in_backing_shell(&raw, line)?
                }
            };
        }
        self.last_exit_code = code;
        Ok(code)
    }

    /// Lazily spawn the backing shell (bash/zsh/sh) the first time it's
    /// needed, applying `minimal_ps1` at session start.
    fn ensure_backing_shell(&mut self) -> Result<()> {
        if self.backing.is_some() {
            return Ok(());
        }

        let shell_path = resolve_backing_shell(&self.config)?;

        let mut child = Command::new(&shell_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to spawn backing shell: {}", shell_path))?;

        BACKING_SHELL_PID.store(child.id(), Ordering::SeqCst);

        let stdin = child
            .stdin
            .take()
            .context("Failed to capture shell stdin")?;
        let stdout = child
            .stdout
            .take()
            .context("Failed to capture shell stdout")?;
        let stderr = child
            .stderr
            .take()
            .context("Failed to capture shell stderr")?;

        let (stdout_tx, stdout_rx) = mpsc::channel();
        let (stderr_tx, stderr_rx) = mpsc::channel();
        spawn_drain_thread(stdout, stdout_tx);
        spawn_drain_thread(stderr, stderr_tx);

        let mut backing = BackingShell {
            child,
            stdin,
            stdout_rx,
            stderr_rx,
        };

        if let Some(ps1) = &self.config.minimal_ps1 {
            // Escaped so a hostile/unexpected PS1 string can't break out into
            // a second shell command; POSIX-safe for sh/bash/zsh alike.
            let escaped = ps1.replace('\'', r"'\''");
            writeln!(backing.stdin, "PS1='{}'", escaped)
                .context("Failed to set PS1 on backing shell")?;
            backing.stdin.flush().ok();
        }

        self.backing = Some(backing);
        Ok(())
    }

    fn shutdown_backing_shell(&mut self) {
        if let Some(mut backing) = self.backing.take() {
            let _ = writeln!(backing.stdin, "exit");
            let _ = backing.stdin.flush();
            let _ = backing.child.wait();
        }
        BACKING_SHELL_PID.store(0, Ordering::SeqCst);
    }

    /// Execute a [`Forward`](SegmentClassification::Forward) segment in the
    /// backing shell: captured stdout/stderr are printed to the real
    /// stdout/stderr verbatim (never filtered — that's the whole point of
    /// `Forward`), and tracked as raw-in/raw-out (correctly 0% savings).
    /// `original_line` is the untouched line the user typed, used for
    /// tracking's `original_cmd` field.
    fn exec_forward_in_backing_shell(&mut self, text: &str, original_line: &str) -> Result<i32> {
        let timer = TimedExecution::start();
        let (code, stdout, stderr) = self.exec_capturing(text)?;

        if !stdout.is_empty() {
            print!("{}", stdout);
            let _ = std::io::stdout().flush();
        }
        if !stderr.is_empty() {
            eprint!("{}", stderr);
            let _ = std::io::stderr().flush();
        }

        let full_output = format!("{}{}", stdout, stderr);
        timer.track_with_session(
            original_line,
            text,
            &full_output,
            &full_output,
            Some(&self.session_id),
        );
        Ok(code)
    }

    /// Execute a [`Filterable`](SegmentClassification::Filterable) segment.
    ///
    /// `original` (e.g. `"git status"`) is sent to the backing shell — never
    /// `rewritten` — so `cd`/`export`/env state changes it makes are applied
    /// to the real persistent session state (it truly executes in the
    /// backing shell, unlike one-shot mode). `rewritten` (e.g.
    /// `"rtk git status"`) is used only to select which RTK ecosystem filter
    /// applies to the captured raw output, via [`apply_ecosystem_filter`].
    ///
    /// Only a subset of ecosystems have a real pure filter wired up here
    /// (currently: `git status`, `cargo test`, `cargo build`, `cargo
    /// clippy`) — see [`apply_ecosystem_filter`] for the exact list and the
    /// fallback-to-raw rationale for everything else.
    fn exec_filterable_in_backing_shell(
        &mut self,
        original: &str,
        rewritten: &str,
        original_line: &str,
    ) -> Result<i32> {
        let timer = TimedExecution::start();
        let (code, stdout, stderr) = self.exec_capturing(original)?;
        let raw_output = format!("{}{}", stdout, stderr);

        let filtered = self
            .apply_ecosystem_filter(rewritten, &stdout, &stderr, code)?
            .unwrap_or_else(|| raw_output.clone());
        // Fallback pattern: never emit more tokens than the raw output would
        // have cost, regardless of which branch produced `filtered`.
        let shown = crate::core::guard::never_worse(&raw_output, &filtered);

        if !shown.is_empty() {
            print!("{}", shown);
            let _ = std::io::stdout().flush();
        }

        timer.track_with_session(
            original_line,
            rewritten,
            &raw_output,
            shown,
            Some(&self.session_id),
        );
        Ok(code)
    }

    /// Execute `text` in the backing shell without tracking or printing —
    /// used for raw multi-line construct bodies (heredocs etc.), which must
    /// reach the shell untouched and are not meaningfully "filtered" either
    /// way. Captured stdout/stderr are still printed verbatim so the user
    /// sees the command's real output.
    fn exec_raw_in_backing_shell(&mut self, text: &str) -> Result<i32> {
        let (code, stdout, stderr) = self.exec_capturing(text)?;
        if !stdout.is_empty() {
            print!("{}", stdout);
            let _ = std::io::stdout().flush();
        }
        if !stderr.is_empty() {
            eprint!("{}", stderr);
            let _ = std::io::stderr().flush();
        }
        Ok(code)
    }

    /// Core execution primitive: send `text` to the backing shell, wait for
    /// the nonce-sentinel-delimited completion marker, and return
    /// `(exit_code, captured_stdout, captured_stderr)`.
    ///
    /// Unlike an earlier version of this code, stdout/stderr are **not**
    /// streamed live to the real stdout/stderr from here (or from the drain
    /// threads) — see [`spawn_drain_thread`] for why. Callers decide what to
    /// print (raw passthrough for `Forward`, filtered for `Filterable`)
    /// after this returns the sentinel-stripped captured text.
    fn exec_capturing(&mut self, text: &str) -> Result<(i32, String, String)> {
        let nonce = generate_nonce();
        let sentinel = format!("__RTK_DONE_{}__", nonce);

        let backing = self
            .backing
            .as_mut()
            .context("Backing shell not initialized")?;

        // Emit the command, then an unmistakable sentinel carrying the
        // command's real exit code ($?), on both stdout and stderr so we
        // can detect completion regardless of which stream drains last.
        writeln!(backing.stdin, "{}", text).context("Failed to write command to backing shell")?;
        writeln!(
            backing.stdin,
            "__rtk_ec=$?; echo \"{sentinel}:$__rtk_ec\"; echo \"{sentinel}:$__rtk_ec\" >&2"
        )
        .context("Failed to write sentinel to backing shell")?;
        backing
            .stdin
            .flush()
            .context("Failed to flush backing shell stdin")?;

        drain_until_sentinel(backing, &sentinel, DEFAULT_COMMAND_TIMEOUT)
    }
}

/// Drain stdout/stderr from `backing` (via the channels fed by the drain
/// threads) until both streams have reported the completion sentinel (or
/// EOF), or `timeout` elapses. Returns `(exit_code, stdout, stderr)`.
///
/// Draining both channels with a bounded `recv_timeout` poll (rather than
/// blocking on one channel at a time) is what prevents a command that only
/// writes to stderr — while stdout stays silent — from ever stalling: both
/// channels are serviced on every loop iteration regardless of which one
/// has data ready.
fn drain_until_sentinel(
    backing: &mut BackingShell,
    sentinel: &str,
    timeout: Duration,
) -> Result<(i32, String, String)> {
    let mut stdout_buf: Vec<u8> = Vec::new();
    let mut stderr_buf: Vec<u8> = Vec::new();
    let mut exit_code: Option<i32> = None;
    let mut stdout_done = false;
    let mut stderr_done = false;
    // Independently track whether *each* stream has seen its own copy of the
    // sentinel line, since the command's sentinel is echoed to both stdout
    // and stderr (see exec_capturing). Stopping as soon as either stream
    // reports it (the previous behavior) leaves the other stream's sentinel
    // line unread in its channel — which then gets incorrectly consumed as
    // literal output by the *next* command's exec_capturing call. Both
    // streams must reach their own sentinel (or EOF) before we stop.
    let mut stdout_sentinel_seen = false;
    let mut stderr_sentinel_seen = false;

    let deadline = Instant::now() + timeout;
    let poll_interval = Duration::from_millis(50);

    loop {
        let stdout_settled = stdout_done || stdout_sentinel_seen;
        let stderr_settled = stderr_done || stderr_sentinel_seen;
        if stdout_settled && stderr_settled {
            break;
        }

        if Instant::now() >= deadline {
            // Hang detected: forward SIGINT to try to reclaim the backing
            // shell for the next command, and surface a clear timeout error
            // rather than blocking the session forever.
            let pid = BACKING_SHELL_PID.load(Ordering::SeqCst);
            if pid != 0 {
                #[cfg(unix)]
                #[allow(unsafe_code)]
                // nosemgrep: unsafe-block
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGINT);
                }
            }
            anyhow::bail!(
                "rtk-shell: command timed out after {:?} with no completion sentinel (possible hang)",
                timeout
            );
        }

        if !stdout_settled {
            match backing.stdout_rx.recv_timeout(poll_interval) {
                Ok(DrainEvent::Chunk(bytes)) => {
                    if stdout_buf.len() < CAPTURE_CAP {
                        let take = bytes.len().min(CAPTURE_CAP - stdout_buf.len());
                        stdout_buf.extend_from_slice(&bytes[..take]);
                    }
                    if let Some(code) = extract_sentinel_code(&stdout_buf, sentinel) {
                        exit_code.get_or_insert(code);
                        stdout_sentinel_seen = true;
                    }
                }
                Ok(DrainEvent::Eof) => stdout_done = true,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => stdout_done = true,
            }
        }

        if !stderr_settled {
            match backing.stderr_rx.recv_timeout(poll_interval) {
                Ok(DrainEvent::Chunk(bytes)) => {
                    if stderr_buf.len() < CAPTURE_CAP {
                        let take = bytes.len().min(CAPTURE_CAP - stderr_buf.len());
                        stderr_buf.extend_from_slice(&bytes[..take]);
                    }
                    if let Some(code) = extract_sentinel_code(&stderr_buf, sentinel) {
                        exit_code.get_or_insert(code);
                        stderr_sentinel_seen = true;
                    }
                }
                Ok(DrainEvent::Eof) => stderr_done = true,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => stderr_done = true,
            }
        }

        if stdout_done && stderr_done && exit_code.is_none() {
            // Backing shell died without ever emitting the sentinel.
            anyhow::bail!("rtk-shell: backing shell exited before command completed");
        }
    }

    let stdout_text = strip_sentinel(&String::from_utf8_lossy(&stdout_buf), sentinel);
    let stderr_text = strip_sentinel(&String::from_utf8_lossy(&stderr_buf), sentinel);

    Ok((exit_code.unwrap_or(1), stdout_text, stderr_text))
}

/// Look for `"{sentinel}:<code>"` in `buf` and return `<code>` if found and
/// the line is complete (terminated by a newline), so we never key off a
/// partially-flushed sentinel line.
fn extract_sentinel_code(buf: &[u8], sentinel: &str) -> Option<i32> {
    let text = String::from_utf8_lossy(buf);
    let marker = format!("{}:", sentinel);
    let start = text.find(&marker)?;
    let after = &text[start + marker.len()..];
    let end = after.find('\n')?; // require a full line — no partial reads
    after[..end].trim().parse::<i32>().ok()
}

/// Remove the sentinel line(s) from captured output before it's used for
/// tracking/token-savings purposes.
fn strip_sentinel(text: &str, sentinel: &str) -> String {
    text.lines()
        .filter(|line| !line.contains(sentinel))
        .collect::<Vec<_>>()
        .join("\n")
}

impl Session {
    /// Apply the RTK filter identified by `rewritten` (e.g. `"rtk git
    /// status"`, `"rtk cargo test"`) to the raw `stdout`/`stderr` captured
    /// from actually running the original segment in the backing shell,
    /// returning the filtered text to show the user in place of the raw
    /// output, or `None` to fall back to raw passthrough.
    ///
    /// Only a deliberately narrow set of ecosystems are wired up here.
    /// Currently:
    ///
    /// - `git status` → mirrors [`cmds::git::git`]'s own `run_status` logic:
    ///   for the default/compact-eligible arg shapes (see
    ///   [`cmds::git::git::uses_compact_status_path`]) this issues one
    ///   additional, read-only `git status --porcelain -b` in the same
    ///   backing shell (safe to re-run — it does not mutate any state) to
    ///   get the porcelain text the real formatter needs, then applies
    ///   [`cmds::git::git::format_status_output`] /
    ///   `format_status_output_detached`, exactly matching one-shot mode's
    ///   output shape. For explicit args that disqualify the compact path,
    ///   it instead applies
    ///   [`cmds::git::git::filter_status_with_args`] directly to the
    ///   already-captured plain-text output (no extra round-trip needed).
    /// - `cargo test` → [`cmds::rust::runner::extract_test_summary`] (pure).
    /// - `cargo build` / `cargo check` → [`cmds::rust::cargo_cmd::filter_cargo_build_labeled`] (pure).
    /// - `cargo clippy` → [`cmds::rust::cargo_cmd::filter_cargo_clippy`] (pure).
    ///
    /// Every other recognized command (e.g. `git log`, `git diff`, `gh pr
    /// view`, `pnpm`/`npm`/`pytest`/...) falls back to `None` here, and the
    /// caller shows the raw captured text unfiltered — consistent with
    /// RTK's documented fallback pattern (never block or degrade the user's
    /// workflow). Extending this list is future work, not a silent gap:
    /// each of those filters' `run_*` entry points re-invokes the
    /// underlying tool with its own tuned flags rather than exposing a pure
    /// `&str -> String` function over plain command output, so wiring them
    /// in here needs either a refactor of those modules or a
    /// session-mode-specific reimplementation — out of scope for this fix.
    fn apply_ecosystem_filter(
        &mut self,
        rewritten: &str,
        stdout: &str,
        stderr: &str,
        exit_code: i32,
    ) -> Result<Option<String>> {
        let argv = lexer::shell_split(rewritten);
        // argv[0] is always the literal "rtk" (see hooks::hook_cmd::get_rewritten);
        // argv[1] is the ecosystem ("git", "cargo", ...); argv[2] is the
        // subcommand ("status", "test", ...); anything after that is the
        // subcommand's own args.
        let ecosystem = argv.first().map(String::as_str);
        let subcommand = argv.get(1).map(String::as_str);
        let action = argv.get(2).map(String::as_str);

        if ecosystem != Some("rtk") {
            return Ok(None);
        }

        let filtered = match (subcommand, action) {
            (Some("git"), Some("status")) => {
                let extra_args: Vec<String> = argv.iter().skip(3).cloned().collect();
                Some(self.filter_git_status(&extra_args, stdout, stderr, exit_code)?)
            }
            (Some("cargo"), Some("test")) => {
                let combined = format!("{}{}", stdout, stderr);
                Some(crate::cmds::rust::runner::extract_test_summary(
                    &combined,
                    "cargo test",
                ))
            }
            (Some("cargo"), Some(sub @ ("build" | "check"))) => {
                let combined = format!("{}{}", stdout, stderr);
                let label: &'static str = if sub == "build" { "build" } else { "check" };
                Some(crate::cmds::rust::cargo_cmd::filter_cargo_build_labeled(
                    &combined, label, exit_code,
                ))
            }
            (Some("cargo"), Some("clippy")) => {
                let combined = format!("{}{}", stdout, stderr);
                Some(crate::cmds::rust::cargo_cmd::filter_cargo_clippy(&combined))
            }
            _ => None,
        };

        Ok(filtered)
    }

    /// `git status` filtering, mirroring `cmds::git::git::run_status`'s two
    /// branches. `extra_args` are the args following `status` in the
    /// rewritten command (e.g. `["--short"]` for `rtk git status --short`).
    fn filter_git_status(
        &mut self,
        extra_args: &[String],
        plain_stdout: &str,
        plain_stderr: &str,
        exit_code: i32,
    ) -> Result<String> {
        use crate::cmds::git::git;

        if !git::uses_compact_status_path(extra_args) {
            // Matches run_status's `!uses_compact_status_path` branch: it
            // filters the plain-text output directly, no porcelain needed.
            if exit_code != 0 {
                return Ok(plain_stderr.to_string());
            }
            return Ok(git::filter_status_with_args(plain_stdout));
        }

        // Compact-eligible: run_status re-runs `git status --porcelain -b`
        // itself and formats *that* — plain `git status` text (which is all
        // the backing shell produced for `original`) isn't enough on its
        // own. Issue one additional, read-only git invocation in the same
        // backing shell to get it; this cannot desync session state since
        // it neither reads stdin nor mutates anything.
        let porcelain_cmd = if extra_args.is_empty() {
            "git status --porcelain -b".to_string()
        } else {
            format!("git status --porcelain -b {}", extra_args.join(" "))
        };
        let (porcelain_code, porcelain_stdout, _porcelain_stderr) =
            self.exec_capturing(&porcelain_cmd)?;

        if exit_code != 0 {
            if plain_stderr.contains("not a git repository") {
                return Ok("Not a git repository".to_string());
            }
            return Ok(plain_stderr.trim().to_string());
        }

        if porcelain_code != 0 {
            // Porcelain re-run failed even though the original succeeded
            // (shouldn't normally happen) — fall back to the plain filter
            // rather than showing nothing.
            return Ok(git::filter_status_with_args(plain_stdout));
        }

        let formatted = match git::extract_detached_head(plain_stdout) {
            Some(detached_ref) => {
                git::format_status_output_detached(&porcelain_stdout, &detached_ref)
            }
            None => git::format_status_output(&porcelain_stdout),
        };

        let final_output = match git::extract_state_header(plain_stdout) {
            Some(state) => format!("{}\n{}", state, formatted),
            None => formatted,
        };

        Ok(final_output)
    }
}

/// Spawn a thread that reads `reader` to EOF, forwarding every chunk over
/// `tx` for sentinel detection/capture. Mirrors `Commands::Proxy`'s
/// stdout/stderr draining threads in `src/main.rs` in its concurrent-polling
/// shape (draining both stdout and stderr on their own threads is what
/// prevents a chatty stream on one fd from ever blocking a quiet one on the
/// other — see [`drain_until_sentinel`] and
/// `test_large_stderr_quiet_stdout_no_deadlock`).
///
/// Deliberately does **not** write anything to the real stdout/stderr here:
/// an earlier version of this code wrote each chunk to the real
/// stdout/stderr as bytes arrived, before the sentinel line could even be
/// recognized as such — which leaked the internal `__RTK_DONE_<nonce>__`
/// completion marker to the terminal on every command, and made it
/// impossible for a caller to filter a `Filterable` segment's output (by
/// the time the sentinel was stripped from the buffered copy, the
/// unfiltered raw bytes had already been shown to the user). Buffering only
/// and letting the caller of [`drain_until_sentinel`] decide what to print
/// (raw for `Forward`, filtered for `Filterable`) fixes both problems, at
/// the accepted cost of no incremental/real-time streaming for
/// long-running, chatty commands (e.g. `docker build` progress) — output is
/// only shown once the whole command completes. That's a deliberate v1
/// scope boundary, not something to re-litigate here.
fn spawn_drain_thread<R: Read + Send + 'static>(
    mut reader: R,
    tx: mpsc::Sender<DrainEvent>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    let _ = tx.send(DrainEvent::Eof);
                    break;
                }
                Ok(n) => {
                    let _ = tx.send(DrainEvent::Chunk(buf[..n].to_vec()));
                }
                Err(_) => {
                    let _ = tx.send(DrainEvent::Eof);
                    break;
                }
            }
        }
    })
}

/// True if `line` contains an unclosed heredoc marker, an unterminated
/// quote, or a trailing backslash continuation — i.e. the backing shell
/// would still be waiting for more input if `line` were typed at a real
/// interactive prompt. Detected via [`discover::lexer`](crate::discover::lexer)
/// token/quote scanning rather than reimplementing shell grammar here.
fn needs_more_input(line: &str) -> bool {
    has_unclosed_heredoc(line)
        || has_trailing_backslash_continuation(line)
        || has_unterminated_quote(line)
}

/// A trailing (non-escaped) backslash at the very end of the line, outside
/// of any quoting, signals a line continuation.
fn has_trailing_backslash_continuation(line: &str) -> bool {
    if has_unterminated_quote(line) {
        // Let quote-state own the decision; a backslash inside a still-open
        // quote isn't a line continuation.
        return false;
    }
    let mut chars = line.chars().rev();
    let mut backslashes = 0;
    for c in chars.by_ref() {
        if c == '\\' {
            backslashes += 1;
        } else {
            break;
        }
    }
    backslashes % 2 == 1
}

/// Scan for an odd number of unescaped, unquoted single/double quote
/// characters — i.e. a quote opened on this (possibly already-joined)
/// line/block that has not yet been closed.
fn has_unterminated_quote(line: &str) -> bool {
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for c in line.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if !in_single => escaped = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            _ => {}
        }
    }
    in_single || in_double
}

/// True if `line` opens a heredoc (`<<EOF`, `<<-EOF`, `<<'EOF'`, `<<"EOF"`)
/// whose terminator has not yet appeared as a line of its own in `line`.
/// Uses [`lexer::tokenize`] to find the `<<`/`<<-` redirect token and its
/// following word rather than hand-rolling redirect parsing.
fn has_unclosed_heredoc(line: &str) -> bool {
    let tokens = lexer::tokenize(line);
    for (i, tok) in tokens.iter().enumerate() {
        if tok.kind != lexer::TokenKind::Redirect || tok.value != "<<" {
            continue;
        }
        let Some(delim_tok) = tokens.get(i + 1) else {
            // `<<` with nothing after it on the line yet — still open.
            return true;
        };

        // The tokenizer has no concept of `<<-` as a distinct redirect value
        // (only `<<`/`<`): for `<<-EOF` the `-` is lexed as the leading
        // character of the following Arg token (`-EOF`), not part of the
        // redirect. Detect the dash-strip variant there instead.
        let raw_delim = &delim_tok.value;
        let strip_tabs = raw_delim.starts_with('-');
        let delim = raw_delim
            .strip_prefix('-')
            .unwrap_or(raw_delim)
            .trim_matches(|c| c == '\'' || c == '"')
            .to_string();

        // Terminator must appear as its own line (optionally preceded by
        // leading tabs when using `<<-`), *after* the opening line itself.
        let body_start = delim_tok.offset + delim_tok.value.len();
        let body = &line[body_start.min(line.len())..];
        let closed = body.lines().skip(1).any(|l| {
            let candidate = if strip_tabs {
                l.trim_start_matches('\t')
            } else {
                l
            };
            candidate == delim
        });
        if !closed {
            return true;
        }
    }
    false
}

/// Resolve the backing shell binary to spawn, in priority order:
/// 1. `ShellConfig::backing_shell` (from `config.toml`'s `[shell]` section).
/// 2. `RTK_BACKING_SHELL` environment variable.
/// 3. Auto-detect via `which`: bash, then zsh, then sh.
fn resolve_backing_shell(config: &ShellConfig) -> Result<String> {
    if let Some(shell) = &config.backing_shell {
        return Ok(shell.clone());
    }
    if let Ok(shell) = std::env::var("RTK_BACKING_SHELL") {
        if !shell.trim().is_empty() {
            return Ok(shell);
        }
    }
    for candidate in ["bash", "zsh", "sh"] {
        if which::which(candidate).is_ok() {
            return Ok(candidate.to_string());
        }
    }
    anyhow::bail!("rtk-shell: no backing shell found (tried bash, zsh, sh) — set [shell].backing_shell in config.toml or RTK_BACKING_SHELL")
}

/// Entry point for `rtk-shell` with no arguments, as invoked from
/// [`bin/rtk_shell`](crate) argv handling: loads the resolved [`ShellConfig`],
/// starts a new persistent [`Session`], and runs it to completion.
///
/// Returns the process exit code to propagate to the OS.
pub fn run() -> Result<i32> {
    let config = Config::load().map(|c| c.shell).unwrap_or_default();
    let mut session = Session::new(config);
    session.run()
}

/// Generate a probabilistically-unique session id used to correlate every
/// command tracked during one rtk-shell session. Falls back to a
/// timestamp+pid-derived value if the OS RNG is unavailable (matching the
/// fallback pattern used by [`core::telemetry`](crate::core::telemetry)).
fn generate_session_id() -> String {
    let mut buf = [0u8; 16];
    if getrandom::fill(&mut buf).is_err() {
        return format!("{:?}:{}", std::time::SystemTime::now(), std::process::id());
    }
    buf.iter().fold(String::new(), |mut output, b| {
        let _ = write!(output, "{b:02x}");
        output
    })
}

/// Generate a fresh random nonce for one command's completion sentinel.
/// Never a fixed marker string: fixed text can collide with a command's own
/// output (e.g. a test suite that legitimately prints `"DONE"`), causing
/// premature completion detection. Falls back to a timestamp+pid-derived
/// value if the OS RNG is unavailable.
fn generate_nonce() -> String {
    let mut buf = [0u8; 12];
    if getrandom::fill(&mut buf).is_err() {
        return format!(
            "{:x}{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default(),
            std::process::id()
        );
    }
    buf.iter().fold(String::new(), |mut output, b| {
        let _ = write!(output, "{b:02x}");
        output
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> ShellConfig {
        ShellConfig {
            backing_shell: Some(default_test_shell()),
            minimal_ps1: Some("$ ".to_string()),
            enable_mode3_swap: false,
        }
    }

    /// Pick a shell guaranteed present in CI/dev environments for tests
    /// that need a real backing shell.
    fn default_test_shell() -> String {
        for candidate in ["bash", "sh", "zsh"] {
            if which::which(candidate).is_ok() {
                return candidate.to_string();
            }
        }
        "sh".to_string()
    }

    #[test]
    fn test_session_new_generates_session_id() {
        let session = Session::new(ShellConfig::default());
        assert!(!session.session_id.is_empty());
    }

    #[test]
    fn test_session_new_generates_distinct_ids() {
        let a = Session::new(ShellConfig::default());
        let b = Session::new(ShellConfig::default());
        assert_ne!(a.session_id, b.session_id);
    }

    #[test]
    fn test_nonce_generation_is_distinct() {
        let a = generate_nonce();
        let b = generate_nonce();
        assert_ne!(a, b);
        assert!(!a.is_empty());
    }

    #[test]
    fn test_needs_more_input_simple_line_false() {
        assert!(!needs_more_input("echo hi"));
        assert!(!needs_more_input("git status"));
    }

    #[test]
    fn test_needs_more_input_unterminated_single_quote() {
        assert!(needs_more_input("echo 'unterminated"));
    }

    #[test]
    fn test_needs_more_input_unterminated_double_quote() {
        assert!(needs_more_input(r#"echo "unterminated"#));
    }

    #[test]
    fn test_needs_more_input_closed_quote_false() {
        assert!(!needs_more_input("echo 'closed'"));
        assert!(!needs_more_input(r#"echo "closed""#));
    }

    #[test]
    fn test_needs_more_input_trailing_backslash() {
        assert!(needs_more_input("echo one \\"));
    }

    #[test]
    fn test_needs_more_input_escaped_backslash_not_continuation() {
        // Two trailing backslashes = one escaped backslash, not a continuation.
        assert!(!needs_more_input(r"echo one \\"));
    }

    #[test]
    fn test_needs_more_input_unclosed_heredoc() {
        assert!(needs_more_input("cat <<EOF"));
    }

    #[test]
    fn test_needs_more_input_heredoc_closed_across_lines() {
        let block = "cat <<EOF\nhello\nEOF";
        assert!(!needs_more_input(block));
    }

    #[test]
    fn test_needs_more_input_heredoc_dash_variant_strips_tabs() {
        let block = "cat <<-EOF\n\thello\n\tEOF";
        assert!(!needs_more_input(block));
    }

    #[test]
    fn test_needs_more_input_quoted_heredoc_delim() {
        assert!(needs_more_input("cat <<'EOF'"));
        let block = "cat <<'EOF'\nliteral $HOME\nEOF";
        assert!(!needs_more_input(block));
    }

    #[test]
    fn test_resolve_backing_shell_from_config() {
        let config = ShellConfig {
            backing_shell: Some("/bin/nonexistent-shell-xyz".to_string()),
            ..ShellConfig::default()
        };
        // Config value takes priority even if not validated against PATH —
        // resolution failure surfaces later at spawn time.
        assert_eq!(
            resolve_backing_shell(&config).unwrap(),
            "/bin/nonexistent-shell-xyz"
        );
    }

    #[test]
    fn test_resolve_backing_shell_env_var_override() {
        // SAFETY: test-only env mutation, restored immediately after.
        std::env::set_var("RTK_BACKING_SHELL", "/bin/test-shell-from-env");
        let config = ShellConfig {
            backing_shell: None,
            ..ShellConfig::default()
        };
        let resolved = resolve_backing_shell(&config).unwrap();
        std::env::remove_var("RTK_BACKING_SHELL");
        assert_eq!(resolved, "/bin/test-shell-from-env");
    }

    #[test]
    fn test_resolve_backing_shell_auto_detect_finds_something() {
        std::env::remove_var("RTK_BACKING_SHELL");
        let config = ShellConfig {
            backing_shell: None,
            ..ShellConfig::default()
        };
        // At least one of bash/zsh/sh must exist on any dev/CI box.
        assert!(resolve_backing_shell(&config).is_ok());
    }

    // --- Real backing-shell integration tests -------------------------------
    //
    // These spawn a real shell process; skip gracefully if none is available
    // (should never happen on macOS/Linux CI, but keeps the suite robust).

    #[test]
    fn test_sentinel_survives_output_matching_naive_fixed_marker() {
        let mut session = Session::new(test_config());
        // A command whose own output contains text that would collide with a
        // naive *fixed* sentinel like "DONE" or "RTK_DONE" — must not trigger
        // premature completion detection because our sentinel is a fresh
        // random nonce each time, not a fixed string.
        let code = session
            .run_line("echo '__RTK_DONE__ this looks like a marker but is just output'")
            .expect("command should complete, not hang or misdetect");
        assert_eq!(code, 0);
        // Session must still be usable afterwards (i.e. we didn't desync the
        // stream by matching mid-output and leaving fragments behind).
        let code2 = session.run_line("echo after").expect("session still alive");
        assert_eq!(code2, 0);
    }

    #[test]
    fn test_large_stderr_quiet_stdout_no_deadlock() {
        let mut session = Session::new(test_config());
        // Enough stderr output to exceed typical pipe buffer sizes (64KiB on
        // Linux, 16KiB on macOS) while stdout stays completely silent — a
        // classic deadlock shape if stderr isn't drained on its own thread.
        // Bounded (self-terminating) unlike `yes`, which never exits on its
        // own once its stdout pipe has no reader.
        let code = session
            .run_line("i=0; while [ $i -lt 20000 ]; do echo err line $i >&2; i=$((i+1)); done")
            .unwrap_or_else(|e| panic!("must not deadlock on quiet stdout: {e}"));
        assert_eq!(code, 0);
    }

    #[test]
    fn test_heredoc_across_multiple_stdin_lines_reaches_shell_intact() {
        let mut session = Session::new(test_config());
        // Simulate what Session::run()'s stdin loop does: feed the heredoc
        // opener, then each subsequent physical line, buffering until the
        // construct closes, then dispatch the whole raw block unfiltered.
        let mut pending = String::from("cat <<'EOF'");
        for extra in [
            "line one && not-an-operator",
            "line two | not-a-pipe",
            "EOF",
        ] {
            assert!(
                needs_more_input(&pending),
                "heredoc must still be considered open before terminator"
            );
            pending.push('\n');
            pending.push_str(extra);
        }
        assert!(!needs_more_input(&pending), "terminator line must close it");

        let code = session
            .run_line(&pending)
            .expect("heredoc body must reach backing shell raw, not be tokenized/split");
        assert_eq!(code, 0);
    }

    #[test]
    fn test_cwd_state_persists_across_sequential_lines() {
        // "cd /tmp" and "pwd" as two sequential run_line() calls against the
        // *same* Session must share one backing shell process, so a cwd
        // change made by the first line is visible to the second — proving
        // state (cwd, in this case) persists across commands in a session,
        // unlike one-shot mode where each -c invocation is independent.
        let mut session = Session::new(test_config());

        let cd_code = session.run_line("cd /tmp").expect("cd should succeed");
        assert_eq!(cd_code, 0);

        let (pwd_code, stdout, _stderr) = session
            .exec_capturing("pwd")
            .expect("pwd should run in the same backing shell");
        assert_eq!(pwd_code, 0);

        // macOS may report /tmp as /private/tmp (symlink resolution), so
        // check for either canonical form rather than an exact match.
        let trimmed = stdout.trim();
        assert!(
            trimmed == "/tmp" || trimmed.ends_with("/tmp"),
            "expected pwd to reflect the earlier `cd /tmp`, got: {trimmed:?}"
        );
    }

    #[test]
    fn test_hung_command_times_out_instead_of_blocking_forever() {
        let mut session = Session::new(test_config());
        session
            .ensure_backing_shell()
            .expect("backing shell must spawn");
        let backing = session.backing.as_mut().expect("backing shell present");

        // Bypass the production DEFAULT_COMMAND_TIMEOUT (5 minutes) with a
        // short one so the test itself doesn't hang for minutes: exercise
        // drain_until_sentinel directly against a command that never prints
        // the sentinel (it sleeps well past our short test timeout, and we
        // never write the sentinel echo at all).
        writeln!(backing.stdin, "sleep 30").unwrap();
        backing.stdin.flush().unwrap();

        let result =
            drain_until_sentinel(backing, "__RTK_NEVER_ARRIVES__", Duration::from_millis(300));
        assert!(result.is_err(), "expected timeout error, not a hang");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("timed out"),
            "error should mention timeout, got: {msg}"
        );
    }

    // --- Regression tests: sentinel leak + real filtering (this fix) -------

    #[test]
    fn test_exec_capturing_never_leaks_sentinel_into_captured_text() {
        // Regression test for the sentinel-leak bug: drain_until_sentinel
        // must fully strip the completion marker from *both* streams before
        // returning, even when one stream's sentinel line settles before the
        // other's. Two back-to-back exec_capturing calls (mirroring what
        // filter_git_status's extra porcelain round-trip does) reproduce the
        // exact shape that used to leak a stray sentinel line into the
        // second call's captured stdout.
        let mut session = Session::new(test_config());
        session
            .ensure_backing_shell()
            .expect("backing shell must spawn");

        let (code1, stdout1, stderr1) = session
            .exec_capturing("echo first")
            .expect("first command should complete");
        assert_eq!(code1, 0);
        assert!(
            !stdout1.contains("__RTK_DONE_") && !stderr1.contains("__RTK_DONE_"),
            "first command's captured output must not contain the sentinel: stdout={stdout1:?} stderr={stderr1:?}"
        );

        let (code2, stdout2, stderr2) = session
            .exec_capturing("echo second")
            .expect("second command should complete");
        assert_eq!(code2, 0);
        assert!(
            !stdout2.contains("__RTK_DONE_") && !stderr2.contains("__RTK_DONE_"),
            "second command's captured output must not contain a leftover sentinel from the first: stdout={stdout2:?} stderr={stderr2:?}"
        );
        assert!(
            stdout2.contains("second"),
            "second command's own output must still be captured correctly, got: {stdout2:?}"
        );
    }

    #[test]
    fn test_filterable_cargo_test_output_is_condensed_not_raw() {
        // Filterable segments must actually be filtered (not raw passthrough)
        // in session mode. Exercise apply_ecosystem_filter directly against
        // realistic raw `cargo test` output (compilation lines, "Running
        // unittests ..." lines, per-test "test ... ok" lines, a trailing
        // summary) to prove real condensation happens, rather than spawning
        // a real (slow, potentially-recursive) `cargo test` subprocess from
        // within a test.
        let mut session = Session::new(test_config());
        session
            .ensure_backing_shell()
            .expect("backing shell must spawn");

        let raw_stdout = "   Compiling rtk v0.43.0\n    Finished `test` profile\n     Running unittests src/lib.rs\n\nrunning 3 tests\ntest shell::dispatch::tests::test_empty_line_yields_no_segments ... ok\ntest shell::dispatch::tests::test_splits_on_semicolon ... ok\ntest shell::dispatch::tests::test_splits_on_double_ampersand ... ok\n\ntest result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 2276 filtered out; finished in 0.00s\n\n";
        let raw_combined = raw_stdout.to_string();

        let filtered = session
            .apply_ecosystem_filter("rtk cargo test", raw_stdout, "", 0)
            .expect("filter dispatch should not error")
            .expect("cargo test must have a real filter wired up, not fall back to raw");

        assert!(
            filtered.len() < raw_combined.len(),
            "filtered cargo test output must be shorter than raw: filtered={} raw={}",
            filtered.len(),
            raw_combined.len()
        );
        assert!(
            filtered.contains("test result: ok"),
            "filtered output should still surface the summary line, got: {filtered:?}"
        );
        assert!(
            !filtered.contains("Compiling") && !filtered.contains("Running unittests"),
            "filtered output must strip build noise, got: {filtered:?}"
        );
        assert!(
            !filtered.contains("__RTK_DONE_"),
            "filtered output must never contain the internal sentinel"
        );
    }

    #[test]
    fn test_forward_segment_output_never_contains_sentinel() {
        // Forward segments (unrecognized/piped commands) print raw captured
        // output verbatim, but must still never leak the internal sentinel
        // used to detect completion.
        let mut session = Session::new(test_config());
        session
            .ensure_backing_shell()
            .expect("backing shell must spawn");

        let (code, stdout, stderr) = session
            .exec_capturing("echo hello | grep hello")
            .expect("forward-style pipeline should run");
        assert_eq!(code, 0);
        assert_eq!(stdout.trim(), "hello");
        assert!(!stdout.contains("__RTK_DONE_"));
        assert!(!stderr.contains("__RTK_DONE_"));
    }

    #[test]
    fn test_unrecognized_command_falls_back_to_raw_via_apply_ecosystem_filter() {
        // Ecosystems with no pure filter wired up (e.g. "git log", not in
        // the currently-supported list) must fall back to None so the
        // caller shows raw captured text, per RTK's fallback pattern.
        let mut session = Session::new(test_config());
        session
            .ensure_backing_shell()
            .expect("backing shell must spawn");

        let result = session
            .apply_ecosystem_filter("rtk git log", "some raw log output", "", 0)
            .expect("dispatch itself should not error");
        assert!(
            result.is_none(),
            "git log has no wired-up pure filter yet and must fall back to raw"
        );
    }

    #[test]
    fn test_git_status_explicit_args_path_filters_without_extra_round_trip() {
        // `git status --short` (and other explicit-args shapes that
        // disqualify the compact path — see uses_compact_status_path)
        // filters the already-captured plain-text output directly; it must
        // not need (or attempt) an extra backing-shell round-trip. Passing
        // extra_args that disqualify the compact path exercises exactly
        // that branch of filter_git_status via apply_ecosystem_filter.
        let mut session = Session::new(test_config());
        session
            .ensure_backing_shell()
            .expect("backing shell must spawn");

        let raw_stdout = "On branch main\nChanges not staged for commit:\n  (use \"git add <file>...\" to update what will be committed)\n\tmodified:   foo.rs\n\nno changes added to commit (use \"git add\" and/or \"git commit -a\")\n";

        let filtered = session
            .apply_ecosystem_filter("rtk git status --porcelain", raw_stdout, "", 0)
            .expect("filter dispatch should not error")
            .expect("git status must have a real filter wired up, not fall back to raw");

        assert!(
            !filtered.contains("(use \"git add"),
            "hint lines must be stripped, got: {filtered:?}"
        );
        assert!(filtered.contains("modified:   foo.rs"));
        assert!(!filtered.contains("__RTK_DONE_"));
    }
}
