//! systemd `--user` unit at `~/.config/systemd/user/rtk-proxy.service`. Logs
//! go to the user journal (`journalctl --user`), not a separate file — a
//! `--user` unit's stdout/stderr is already captured there, so `log_path`
//! (used by macOS/Windows) is unused here.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const MECHANISM: &str = "systemd (user unit)";

const UNIT_NAME: &str = "rtk-proxy.service";

fn unit_path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("could not resolve a user config directory")?
        .join("systemd/user");
    Ok(dir.join(UNIT_NAME))
}

pub fn install(bin: &Path, config_path: &Path, _log_path: &Path) -> Result<PathBuf> {
    let path = unit_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }

    let unit = format!(
        "[Unit]\n\
         Description=rtk-proxy -- local LLM proxy\n\
         \n\
         [Service]\n\
         ExecStart={bin} start\n\
         Environment=RTK_PROXY_CONFIG={config}\n\
         Restart=on-failure\n\
         RestartSec=2\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        bin = bin.display(),
        config = config_path.display(),
    );
    std::fs::write(&path, unit).with_context(|| format!("writing {}", path.display()))?;

    run_systemctl(&["daemon-reload"])?;
    run_systemctl(&["enable", "--now", UNIT_NAME])?;
    Ok(path)
}

pub fn uninstall() -> Result<bool> {
    let path = unit_path()?;
    if !path.exists() {
        return Ok(false);
    }
    let _ = Command::new("systemctl")
        .args(["--user", "disable", "--now", UNIT_NAME])
        .status();
    std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    let _ = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    Ok(true)
}

pub fn is_installed() -> Result<bool> {
    Ok(unit_path()?.exists())
}

pub fn logs() -> Result<()> {
    let status = Command::new("journalctl")
        .args(["--user", "-u", UNIT_NAME, "-n", "50", "-f"])
        .status()
        .context("running journalctl")?;
    if !status.success() {
        bail!("journalctl exited with {status}");
    }
    Ok(())
}

fn run_systemctl(args: &[&str]) -> Result<()> {
    let mut full = vec!["--user"];
    full.extend_from_slice(args);
    let status = Command::new("systemctl")
        .args(&full)
        .status()
        .with_context(|| format!("running systemctl {}", full.join(" ")))?;
    if !status.success() {
        bail!("systemctl {} failed ({status})", full.join(" "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_path_is_under_systemd_user() {
        let path = unit_path().unwrap();
        assert!(path.to_string_lossy().contains("systemd/user"));
        assert!(path.to_string_lossy().ends_with("rtk-proxy.service"));
    }
}
