//! PTY-backed command capture (opt-in via `[[tools]] capture = "pty"`).
//!
//! Some interactive builders — `ng build`, `vite`, `webpack serve` — only exit
//! cleanly when they believe they are attached to a terminal. Captured over an
//! ordinary pipe they spawn a persistent helper (e.g. `esbuild --service`) that
//! inherits the pipe and holds it open, so rtk's reader never sees EOF and hangs
//! (see docs/pr_briefs/001-pipe-eof-grandchild-hang).
//!
//! Running the child under a pseudo-terminal makes it behave as in a real
//! terminal — one-shot, clean exit. The trade-off is the child then emits ANSI
//! color/cursor/spinner sequences, so we strip ANSI and normalize carriage
//! returns at the capture boundary to keep downstream filters fed clean text.

use crate::core::stream::StreamResult;
use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::Read;
use std::process::Command;

/// Run `cmd` attached to a PTY, capturing combined output. `strip_ansi` removes
/// terminal control sequences from the captured text (recommended for PTY).
///
/// Returns a [`StreamResult`] with `raw`/`raw_stdout` set to the (optionally
/// sanitized) output and `raw_stderr` empty — a PTY merges both streams onto one
/// terminal, so there is no separate stderr to report.
pub fn run_pty_capture(cmd: &Command, strip_ansi: bool) -> Result<StreamResult> {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("Failed to open pty")?;

    // Translate std::process::Command → portable_pty::CommandBuilder.
    let mut builder = CommandBuilder::new(cmd.get_program());
    for arg in cmd.get_args() {
        builder.arg(arg);
    }
    if let Some(dir) = cmd.get_current_dir() {
        builder.cwd(dir);
    }
    for (key, val) in cmd.get_envs() {
        match val {
            Some(v) => builder.env(key, v),
            None => builder.env_remove(key),
        }
    }

    let mut child = pair
        .slave
        .spawn_command(builder)
        .context("Failed to spawn under pty")?;
    // Critical: drop the slave so the master receives EOF once the child tree exits.
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .context("Failed to clone pty reader")?;
    // We send no input; drop the writer handle so nothing keeps the master alive.
    drop(pair.master);

    // Read on a background thread so a misbehaving child can't wedge the wait().
    let reader_thread = std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
        }
        out
    });

    let status = child.wait().context("Failed to wait for pty child")?;
    let bytes = reader_thread.join().unwrap_or_default();

    let exit_code = status.exit_code() as i32;
    let raw_text = String::from_utf8_lossy(&bytes).into_owned();
    let cleaned = if strip_ansi {
        crate::core::utils::strip_ansi(&raw_text)
    } else {
        raw_text
    };

    Ok(StreamResult {
        exit_code,
        raw: cleaned.clone(),
        raw_stdout: cleaned.clone(),
        raw_stderr: String::new(),
        filtered: cleaned,
    })
}
