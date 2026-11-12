//! Windows Task Scheduler, user-level (no admin): an "at log on" task running
//! with limited rights, plus a repeating trigger that relaunches a proxy
//! that died (a no-op while it is alive). `schtasks` has no native way to pass
//! individual environment variables to a task, so the task runs a small
//! `cmd /C` wrapper that sets `RTK_PROXY_CONFIG` and redirects stdout/stderr
//! into the log file itself (there's no journald/launchd-style log capture
//! here to rely on).

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const MECHANISM: &str = "Task Scheduler (log-on trigger, relaunched when it exits)";

const TASK_NAME: &str = "rtk-proxy";

fn current_user() -> Option<String> {
    let name = std::env::var("USERNAME").ok().filter(|n| !n.is_empty())?;
    Some(match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => format!("{domain}\\{name}"),
        _ => name,
    })
}

pub fn install(bin: &Path, config_path: &Path, log_path: &Path) -> Result<PathBuf> {
    let xml =
        super::windows_task::definition(bin, config_path, log_path, current_user().as_deref());
    let xml_path = super::state_dir()?.join("rtk-proxy-task.xml");
    std::fs::write(&xml_path, super::windows_task::encode(&xml))
        .with_context(|| format!("writing {}", xml_path.display()))?;
    let status = Command::new("schtasks")
        .args(["/Create", "/F", "/TN", TASK_NAME, "/XML"])
        .arg(&xml_path)
        .status()
        .context("running schtasks /Create");
    let _ = std::fs::remove_file(&xml_path);
    let status = status?;
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
