//! Windows Task Scheduler, user-level (no admin): an "at log on" task running
//! with limited rights (`/RL LIMITED`). `schtasks` has no native way to pass
//! individual environment variables to a task, so the task runs a small
//! `cmd /C` wrapper that sets `RTK_PROXY_CONFIG` and redirects stdout/stderr
//! into the log file itself (there's no journald/launchd-style log capture
//! here to rely on).

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const MECHANISM: &str = "Task Scheduler (log-on trigger)";

const TASK_NAME: &str = "rtk-proxy";

pub fn install(bin: &Path, config_path: &Path, log_path: &Path) -> Result<PathBuf> {
    let cmd = format!(
        "cmd /C \"set RTK_PROXY_CONFIG={config} && \"{bin}\" start >> \"{log}\" 2>&1\"",
        config = config_path.display(),
        bin = bin.display(),
        log = log_path.display(),
    );
    let status = Command::new("schtasks")
        .args([
            "/Create", "/F", "/TN", TASK_NAME, "/SC", "ONLOGON", "/RL", "LIMITED", "/TR", &cmd,
        ])
        .status()
        .context("running schtasks /Create")?;
    if !status.success() {
        bail!("schtasks /Create failed ({status})");
    }
    // schtasks has no on-disk definition file the caller can inspect directly;
    // report the task name in its place for the CLI message.
    Ok(PathBuf::from(TASK_NAME))
}

pub fn uninstall() -> Result<bool> {
    if !is_installed()? {
        return Ok(false);
    }
    let status = Command::new("schtasks")
        .args(["/Delete", "/TN", TASK_NAME, "/F"])
        .status()
        .context("running schtasks /Delete")?;
    if !status.success() {
        bail!("schtasks /Delete failed ({status})");
    }
    Ok(true)
}

pub fn is_installed() -> Result<bool> {
    let out = Command::new("schtasks")
        .args(["/Query", "/TN", TASK_NAME])
        .output()
        .context("running schtasks /Query")?;
    Ok(out.status.success())
}

pub fn logs() -> Result<()> {
    let log_path = super::log_file_path()?;
    if !log_path.exists() {
        bail!(
            "no log file yet at {} — has the service been started?",
            log_path.display()
        );
    }
    poll_tail(&log_path)
}

/// Minimal `tail -f` fallback: Windows has no built-in equivalent. Prints
/// the current content, then polls for appended bytes until interrupted.
fn poll_tail(path: &Path) -> Result<()> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file =
        std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).context("reading log file")?;
    print!("{buf}");

    let mut pos = file
        .stream_position()
        .context("reading log file position")?;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let len = file.metadata().context("reading log file metadata")?.len();
        if len > pos {
            file.seek(SeekFrom::Start(pos))
                .context("seeking log file")?;
            let mut chunk = String::new();
            file.read_to_string(&mut chunk)
                .context("reading appended log lines")?;
            print!("{chunk}");
            pos = file
                .stream_position()
                .context("reading log file position")?;
        }
    }
}
