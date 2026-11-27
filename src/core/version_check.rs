//! Periodic, non-blocking check for newer RTK releases (Issue #725).
//!
//! Checks GitHub Releases once every 3 hours (or 24 hours on API rate limit/error)
//! in a detached thread, caching the latest tag to `$RTK_DATA_DIR/.version_check.json`.
//! Formats and displays an upgrade notification only on informational/meta commands
//! and `--version`/`--help`. Never displays on operational or hook commands.

use crate::core::{http, user_dirs, user_env, utils};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const NORMAL_CHECK_INTERVAL_SECS: u64 = 3 * 3600; // 3 hours
pub const ERROR_BACKOFF_INTERVAL_SECS: u64 = 24 * 3600; // 24 hours
const CACHE_FILE_NAME: &str = ".version_check.json";
const RELEASES_API_URL: &str = "https://api.github.com/repos/rtk-ai/rtk/releases/latest";
const HTTP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VersionCache {
    pub latest_version: String,
    pub last_checked_epoch: u64,
    pub rate_limited_until: u64,
}

impl VersionCache {
    pub fn is_due(&self, now: u64) -> bool {
        if now < self.rate_limited_until {
            return false;
        }
        now.saturating_sub(self.last_checked_epoch) >= NORMAL_CHECK_INTERVAL_SECS
    }
}

pub fn parse_version(s: &str) -> (u32, u32, u32) {
    let s = s.trim().trim_start_matches('v');
    // Strip prerelease or build metadata if present (e.g. "1.2.3-rc1" -> "1.2.3")
    let core = s
        .split('-')
        .next()
        .unwrap_or(s)
        .split('+')
        .next()
        .unwrap_or(s);
    let mut parts = core.split('.').filter_map(|p| p.parse::<u32>().ok());
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

pub fn is_newer(remote: &str, current: &str) -> bool {
    parse_version(remote) > parse_version(current)
}

pub fn install_method_from_path(path: &str) -> &'static str {
    if path.contains("/Cellar/rtk/")
        || path.contains("/homebrew/")
        || path.contains("/opt/homebrew/")
    {
        "homebrew"
    } else if path.contains("/.cargo/bin/") || path.contains(r"\.cargo\bin\") {
        "cargo"
    } else if path.contains("/.local/bin/") || path.contains(r"\.local\bin\") {
        "script"
    } else if path.contains("/nix/store/") {
        "nix"
    } else if path.contains("WinGet") || path.contains("winget") {
        "winget"
    } else {
        "other"
    }
}

pub fn detect_install_method() -> &'static str {
    #[cfg(test)]
    if let Some(override_method) = user_env::var("RTK_TEST_INSTALL_METHOD") {
        return match override_method.as_str() {
            "homebrew" => "homebrew",
            "cargo" => "cargo",
            "script" => "script",
            "nix" => "nix",
            "winget" => "winget",
            _ => "other",
        };
    }

    if let Some(override_method) = user_env::var("RTK_INSTALL_METHOD") {
        return match override_method.as_str() {
            "homebrew" => "homebrew",
            "cargo" => "cargo",
            "script" => "script",
            "nix" => "nix",
            "winget" => "winget",
            _ => "other",
        };
    }

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return "other",
    };
    let real_path = std::fs::canonicalize(&exe)
        .unwrap_or(exe)
        .to_string_lossy()
        .to_string();
    install_method_from_path(&real_path)
}

pub fn upgrade_command_for_method(method: &str) -> Option<&'static str> {
    match method {
        "homebrew" => Some("brew upgrade rtk"),
        "cargo" => Some("cargo install --git https://github.com/rtk-ai/rtk rtk"),
        "script" => {
            Some("curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/master/install.sh | sh")
        }
        "nix" => Some("nix profile upgrade rtk"),
        "winget" => Some("winget upgrade rtk-ai.rtk"),
        _ => None,
    }
}

pub fn cache_path() -> Option<PathBuf> {
    user_dirs::data().map(|d| d.join(CACHE_FILE_NAME))
}

pub fn load_cache() -> Option<VersionCache> {
    let path = cache_path()?;
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn save_cache(cache: &VersionCache) -> Option<()> {
    let path = cache_path()?;
    if let Some(parent) = path.parent() {
        let _ = utils::create_private_dir(parent);
    }
    let data = serde_json::to_string(cache).ok()?;
    let tmp_path = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp_path, data).ok()?;
    let _ = std::fs::rename(&tmp_path, &path);
    Some(())
}

fn current_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Returns a formatted 1-line update notification message if a newer version is cached.
pub fn notification_message() -> Option<String> {
    let cache = load_cache()?;
    if is_newer(&cache.latest_version, env!("CARGO_PKG_VERSION")) {
        let tag = if cache.latest_version.starts_with('v') {
            cache.latest_version
        } else {
            format!("v{}", cache.latest_version)
        };
        let method = detect_install_method();
        let message = match upgrade_command_for_method(method) {
            Some(cmd) => format!("[rtk] {} available. Run: {}", tag, cmd),
            None => format!(
                "[rtk] {} available. See https://github.com/rtk-ai/rtk/releases",
                tag
            ),
        };
        Some(message)
    } else {
        None
    }
}

/// Checks GitHub for updates in a non-blocking background thread if interval has elapsed.
pub fn maybe_refresh_background() {
    if user_env::var("RTK_NO_UPDATE_CHECK").as_deref() == Some("1") || user_env::var("CI").is_some()
    {
        return;
    }

    let now = current_epoch_secs();
    let cache = load_cache();
    if let Some(ref c) = cache
        && !c.is_due(now)
    {
        return;
    }

    // Touch/claim the timestamp immediately to prevent race stampedes from concurrent commands
    let existing_version = cache
        .as_ref()
        .map(|c| c.latest_version.clone())
        .unwrap_or_default();
    let claim_cache = VersionCache {
        latest_version: existing_version,
        last_checked_epoch: now,
        rate_limited_until: 0,
    };
    save_cache(&claim_cache);

    let _ = std::thread::Builder::new()
        .name("rtk-version-check".into())
        .spawn(move || {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(fetch_and_update));
        });
}

fn fetch_and_update() {
    let user_agent = format!("rtk/{}", env!("CARGO_PKG_VERSION"));
    // nosemgrep: ureq-outside-telemetry -- GitHub Releases version check for Issue #725
    let req = ureq::get(RELEASES_API_URL)
        .header("User-Agent", &user_agent)
        .header("Accept", "application/vnd.github.v3+json");

    let now = current_epoch_secs();
    match http::global_timeout(req, HTTP_TIMEOUT).call() {
        Ok(mut res) => {
            if let Ok(text) = http::read_text_lossy(&mut res, 4096)
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
                && let Some(tag) = json.get("tag_name").and_then(|v| v.as_str())
            {
                let updated = VersionCache {
                    latest_version: tag.to_string(),
                    last_checked_epoch: now,
                    rate_limited_until: 0,
                };
                save_cache(&updated);
            }
        }
        Err(e) => {
            // If rate limited (HTTP 403 / 429) or network failure, back off for 24 hours
            let existing_version = load_cache().map(|c| c.latest_version).unwrap_or_default();
            let is_rate_limit = match &e {
                ureq::Error::StatusCode(code) => *code == 403 || *code == 429,
                _ => false,
            };
            let backoff_secs = if is_rate_limit {
                ERROR_BACKOFF_INTERVAL_SECS
            } else {
                NORMAL_CHECK_INTERVAL_SECS
            };
            let backed_off = VersionCache {
                latest_version: existing_version,
                last_checked_epoch: now,
                rate_limited_until: now + backoff_secs,
            };
            save_cache(&backed_off);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version() {
        assert_eq!(parse_version("v0.52.0"), (0, 52, 0));
        assert_eq!(parse_version("0.52.0"), (0, 52, 0));
        assert_eq!(parse_version("v1.2.3-rc1"), (1, 2, 3));
        assert_eq!(parse_version(""), (0, 0, 0));
        assert_eq!(parse_version("invalid"), (0, 0, 0));
    }

    #[test]
    fn test_is_newer() {
        assert!(is_newer("v0.52.0", "0.51.0"));
        assert!(is_newer("1.0.0", "0.99.9"));
        assert!(is_newer("0.51.1", "0.51.0"));
        assert!(!is_newer("0.51.0", "0.51.0"));
        assert!(!is_newer("0.50.9", "0.51.0"));
    }

    #[test]
    fn test_install_method_from_path() {
        assert_eq!(
            install_method_from_path("/opt/homebrew/Cellar/rtk/0.51.0/bin/rtk"),
            "homebrew"
        );
        assert_eq!(
            install_method_from_path("/usr/local/homebrew/bin/rtk"),
            "homebrew"
        );
        assert_eq!(
            install_method_from_path("/home/user/.cargo/bin/rtk"),
            "cargo"
        );
        assert_eq!(
            install_method_from_path(r"C:\Users\user\.cargo\bin\rtk.exe"),
            "cargo"
        );
        assert_eq!(
            install_method_from_path("/home/user/.local/bin/rtk"),
            "script"
        );
        assert_eq!(
            install_method_from_path("/nix/store/abc-rtk/bin/rtk"),
            "nix"
        );
        assert_eq!(
            install_method_from_path(r"C:\Program Files\WinGet\rtk.exe"),
            "winget"
        );
        assert_eq!(install_method_from_path("/usr/bin/rtk"), "other");
    }

    #[test]
    fn test_upgrade_command_for_method() {
        assert_eq!(
            upgrade_command_for_method("homebrew"),
            Some("brew upgrade rtk")
        );
        assert_eq!(
            upgrade_command_for_method("cargo"),
            Some("cargo install --git https://github.com/rtk-ai/rtk rtk")
        );
        assert_eq!(
            upgrade_command_for_method("script"),
            Some("curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/master/install.sh | sh")
        );
        assert_eq!(
            upgrade_command_for_method("nix"),
            Some("nix profile upgrade rtk")
        );
        assert_eq!(
            upgrade_command_for_method("winget"),
            Some("winget upgrade rtk-ai.rtk")
        );
        assert_eq!(upgrade_command_for_method("other"), None);
    }

    #[test]
    fn test_cache_is_due_intervals() {
        let now = 100_000;
        let fresh_cache = VersionCache {
            latest_version: "v0.51.0".into(),
            last_checked_epoch: now - 3600, // 1 hour ago
            rate_limited_until: 0,
        };
        assert!(!fresh_cache.is_due(now));

        let due_cache = VersionCache {
            latest_version: "v0.51.0".into(),
            last_checked_epoch: now - (3 * 3600 + 1), // > 3 hours ago
            rate_limited_until: 0,
        };
        assert!(due_cache.is_due(now));

        let rate_limited_cache = VersionCache {
            latest_version: "v0.51.0".into(),
            last_checked_epoch: now - (4 * 3600),
            rate_limited_until: now + 3600, // Still locked for 1 hour
        };
        assert!(!rate_limited_cache.is_due(now));
    }

    #[test]
    fn test_cache_save_and_load_roundtrip() {
        let cache = VersionCache {
            latest_version: "v99.0.0".into(),
            last_checked_epoch: 1234567,
            rate_limited_until: 0,
        };
        assert!(save_cache(&cache).is_some());
        let loaded = load_cache().expect("cache should load");
        assert_eq!(loaded, cache);

        // Test notification_message formatting (falls back to GitHub releases link when in test harness)
        let msg = notification_message().expect("should produce notification for v99.0.0");
        assert!(
            msg.contains("[rtk] v99.0.0 available. See https://github.com/rtk-ai/rtk/releases")
        );
    }
}
