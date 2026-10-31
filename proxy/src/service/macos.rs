//! launchd user agent: a plist under `~/Library/LaunchAgents/`, loaded with
//! `launchctl load -w`. Deliberately uses the older `load`/`unload` verbs
//! (still supported) rather than the modern `bootstrap`/`bootout`, which need
//! a resolved `gui/<uid>` domain — one fewer moving part for a single-file
//! install/uninstall.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const MECHANISM: &str = "launchd (user agent)";

const LABEL: &str = "app.rtk-ai.rtk-proxy";

fn plist_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not resolve the home directory")?;
    Ok(home
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

pub fn install(bin: &Path, config_path: &Path, log_path: &Path) -> Result<PathBuf> {
    let path = plist_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }

    // Idempotent re-install: unload any previous registration first (fine if
    // there wasn't one — launchctl just errors, which we ignore).
    let _ = Command::new("launchctl").arg("unload").arg(&path).output();

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{bin}</string>
        <string>start</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>RTK_PROXY_CONFIG</key>
        <string>{config}</string>
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>WorkingDirectory</key>
    <string>{workdir}</string>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#,
        bin = bin.display(),
        config = config_path.display(),
        log = log_path.display(),
        // Without this, launchd runs the daemon from `/`, where a relative
        // database path cannot be created and `rtk gain` never looks. Anchor
        // it to the config's own directory so any relative default lands
        // beside the config the service was installed with.
        workdir = config_path
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "/".to_string()),
    );
    std::fs::write(&path, plist).with_context(|| format!("writing {}", path.display()))?;

    let status = Command::new("launchctl")
        .arg("load")
        .arg("-w")
        .arg(&path)
        .status()
        .context("running launchctl load")?;
    if !status.success() {
        bail!("launchctl load failed ({status})");
    }
    Ok(path)
}

pub fn uninstall() -> Result<bool> {
    let path = plist_path()?;
    if !path.exists() {
        return Ok(false);
    }
    let _ = Command::new("launchctl").arg("unload").arg(&path).status();
    std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    Ok(true)
}

pub fn is_installed() -> Result<bool> {
    Ok(plist_path()?.exists())
}

pub fn logs() -> Result<()> {
    let log_path = super::log_file_path()?;
    if !log_path.exists() {
        bail!(
            "no log file yet at {} — has the service been started?",
            log_path.display()
        );
    }
    let status = Command::new("tail")
        .arg("-n")
        .arg("50")
        .arg("-f")
        .arg(&log_path)
        .status()
        .context("running tail -f")?;
    if !status.success() {
        bail!("tail exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_path_is_under_launch_agents() {
        let path = plist_path().unwrap();
        assert!(path.to_string_lossy().contains("Library/LaunchAgents"));
        assert!(path.to_string_lossy().ends_with(".plist"));
    }
}
