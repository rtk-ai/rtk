//! Install/uninstall/status/logs for a per-user background
//! service that runs `rtk-proxy start` across logins — launchd (macOS),
//! systemd `--user` (Linux), Task Scheduler (Windows). User-level only: never
//! touches a system-wide daemon/service, never needs sudo/admin.
//!
//! Every install is pinned to a loopback bind ([`ensure_loopback_bind`]) —
//! this proxy relays LLM API traffic (and whatever upstream API keys pass
//! through it), so a background service must never be reachable
//! off this machine, no matter what bind a caller passes at install time.
//!
//! The resolved config at install time is frozen into a TOML file under a
//! per-user state dir (`dirs::config_dir()/rtk-proxy/`) and the installed
//! service points `RTK_PROXY_CONFIG` at it — so the service behaves the same
//! on every login regardless of what's in the shell environment at boot.

use crate::config::Config;
use anyhow::{Context, Result, bail};
use std::path::PathBuf;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
use unsupported as platform;
#[cfg(target_os = "windows")]
use windows as platform;

/// Name of the mechanism used on this platform, for CLI messages.
pub const MECHANISM: &str = platform::MECHANISM;

/// Refuse anything that isn't loopback. This proxy relays LLM API traffic
/// (including whatever upstream API keys pass through it), so a background
/// service must never be reachable off this machine.
pub fn ensure_loopback_bind(bind: &str) -> Result<()> {
    let host = bind.rsplit_once(':').map(|(h, _)| h).unwrap_or(bind);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let is_loopback = host == "localhost"
        || host
            .parse::<std::net::Ipv4Addr>()
            .map(|a| a.is_loopback())
            .unwrap_or(false)
        || host
            .parse::<std::net::Ipv6Addr>()
            .map(|a| a.is_loopback())
            .unwrap_or(false);
    if !is_loopback {
        bail!(
            "refusing to install rtk-proxy as a background service bound to `{bind}` — \
             only a loopback bind (127.0.0.1 / ::1 / localhost) is allowed for a service; \
             a background daemon must never be reachable off this machine"
        );
    }
    Ok(())
}

/// Per-user state dir for the generated config + log file, created on demand.
fn state_dir() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("could not resolve a user config directory")?
        .join("rtk-proxy");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

/// Where the frozen, fully-resolved config for the installed service lives.
pub fn config_file_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("rtk-proxy.toml"))
}

/// Where the installed service's stdout/stderr are captured.
pub fn log_file_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("rtk-proxy.log"))
}

/// Freeze `config` into the per-user config file, then hand off to the
/// platform-specific installer. Returns the path of the installed service
/// definition (plist / systemd unit / scheduled task marker), for the CLI to
/// print.
pub fn install(config: &Config) -> Result<PathBuf> {
    ensure_loopback_bind(&config.bind)?;

    let config_path = config_file_path()?;
    let toml_text = toml::to_string_pretty(&config.to_file_config())
        .context("serializing the resolved config")?;
    std::fs::write(&config_path, toml_text)
        .with_context(|| format!("writing {}", config_path.display()))?;

    let log_path = log_file_path()?;
    let bin = std::env::current_exe().context("resolving rtk-proxy's own binary path")?;
    platform::install(&bin, &config_path, &log_path)
}

/// Unregister the service (best-effort stop first) and remove its definition
/// file. Returns `false` when nothing was installed to begin with.
pub fn uninstall() -> Result<bool> {
    platform::uninstall()
}

/// Whether a service definition is currently registered on this platform.
/// This is about registration, not liveness — pair with an HTTP health check
/// (see `main.rs`'s `run_status`) to know if it's actually running.
pub fn is_installed() -> Result<bool> {
    platform::is_installed()
}

/// The `bind` recorded in the installed service's config file, if a service
/// is installed — lets `status`/`logs` find the daemon without repeating
/// `--bind`.
pub fn installed_bind() -> Result<Option<String>> {
    let path = config_file_path()?;
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let file: crate::config::FileConfig =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(file.bind().map(str::to_string))
}

/// Print recent log output and keep following it until interrupted
/// (Ctrl-C). Blocks.
pub fn logs() -> Result<()> {
    platform::logs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_loopback_binds() {
        assert!(ensure_loopback_bind("127.0.0.1:8080").is_ok());
        assert!(ensure_loopback_bind("localhost:8080").is_ok());
        assert!(ensure_loopback_bind("[::1]:8080").is_ok());
        assert!(ensure_loopback_bind("127.5.5.5:1").is_ok());
    }

    #[test]
    fn rejects_non_loopback_binds() {
        assert!(ensure_loopback_bind("0.0.0.0:8080").is_err());
        assert!(ensure_loopback_bind("192.168.1.10:8080").is_err());
        assert!(ensure_loopback_bind("example.com:8080").is_err());
        assert!(ensure_loopback_bind("[::]:8080").is_err());
    }
}
