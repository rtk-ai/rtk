//! Fallback for platforms without a service-management implementation (e.g.
//! the BSDs) — errors clearly instead of silently doing nothing.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

pub const MECHANISM: &str = "unsupported";

pub fn install(_bin: &Path, _config_path: &Path, _log_path: &Path) -> Result<PathBuf> {
    bail!("rtk-proxy service management isn't implemented for this platform yet");
}

pub fn uninstall() -> Result<bool> {
    bail!("rtk-proxy service management isn't implemented for this platform yet");
}

pub fn is_installed() -> Result<bool> {
    Ok(false)
}

pub fn logs() -> Result<()> {
    bail!("rtk-proxy service management isn't implemented for this platform yet");
}
