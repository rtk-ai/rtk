use crate::config;
use crate::tracking;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

const TELEMETRY_URL: Option<&str> = option_env!("RTK_TELEMETRY_URL");
const TELEMETRY_TOKEN: Option<&str> = option_env!("RTK_TELEMETRY_TOKEN");
const PING_INTERVAL_SECS: u64 = 23 * 3600; // 23 hours

/// Send a telemetry ping if enabled and not already sent today.
/// Fire-and-forget: errors are silently ignored.
pub fn maybe_ping() {
    // No URL compiled in → telemetry disabled
    if TELEMETRY_URL.is_none() {
        return;
    }

    // Check opt-out: env var
    if std::env::var("RTK_TELEMETRY_DISABLED").unwrap_or_default() == "1" {
        return;
    }

    // Check opt-out: config.toml
    if let Some(false) = config::telemetry_enabled() {
        return;
    }

    // Check last ping time
    let marker = telemetry_marker_path();
    if let Ok(metadata) = std::fs::metadata(&marker) {
        if let Ok(modified) = metadata.modified() {
            if let Ok(elapsed) = modified.elapsed() {
                if elapsed.as_secs() < PING_INTERVAL_SECS {
                    return;
                }
            }
        }
    }

    // Touch marker file immediately (before sending) to avoid double-ping
    touch_marker(&marker);

    // Spawn thread so we never block the CLI
    std::thread::spawn(|| {
        let _ = send_ping();
    });
}

fn send_ping() -> Result<(), Box<dyn std::error::Error>> {
    let url = TELEMETRY_URL.ok_or("no telemetry URL")?;
    let device_hash = generate_device_hash();
    let version = env!("CARGO_PKG_VERSION").to_string();
    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();
    let install_method = detect_install_method();

    // Get stats from tracking DB
    let (commands_24h, top_commands, savings_pct) = get_stats();

    let payload = serde_json::json!({
        "device_hash": device_hash,
        "version": version,
        "os": os,
        "arch": arch,
        "install_method": install_method,
        "commands_24h": commands_24h,
        "top_commands": top_commands,
        "savings_pct": savings_pct,
    });

    let mut req = ureq::post(url).set("Content-Type", "application/json");

    if let Some(token) = TELEMETRY_TOKEN {
        req = req.set("X-RTK-Token", token);
    }

    // 2 second timeout — if server is down, we move on
    req.timeout(std::time::Duration::from_secs(2))
        .send_string(&payload.to_string())?;

    Ok(())
}

fn generate_device_hash() -> String {
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let username = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();

    let mut hasher = Sha256::new();
    hasher.update(hostname.as_bytes());
    hasher.update(b":");
    hasher.update(username.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn get_stats() -> (i64, Vec<String>, Option<f64>) {
    let tracker = match tracking::Tracker::new() {
        Ok(t) => t,
        Err(_) => return (0, vec![], None),
    };

    // Get 24h command count and top commands from tracking DB
    let commands_24h = tracker
        .count_commands_since(chrono::Utc::now() - chrono::Duration::hours(24))
        .unwrap_or(0);

    let top_commands = tracker.top_commands(5).unwrap_or_default();

    let savings_pct = tracker.overall_savings_pct().ok();

    (commands_24h, top_commands, savings_pct)
}

fn detect_install_method() -> &'static str {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return "unknown",
    };
    let real_path = std::fs::canonicalize(&exe)
        .unwrap_or(exe)
        .to_string_lossy()
        .to_string();
    install_method_from_path(&real_path)
}

fn install_method_from_path(path: &str) -> &'static str {
    if path.contains("/Cellar/rtk/") || path.contains("/homebrew/") {
        "homebrew"
    } else if path.contains("/.cargo/bin/") || path.contains("\\.cargo\\bin\\") {
        "cargo"
    } else if path.contains("/.local/bin/") || path.contains("\\.local\\bin\\") {
        "script"
    } else if path.contains("/nix/store/") {
        "nix"
    } else {
        "other"
    }
}

fn telemetry_marker_path() -> PathBuf {
    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("rtk");
    let _ = std::fs::create_dir_all(&data_dir);
    data_dir.join(".telemetry_last_ping")
}

fn touch_marker(path: &PathBuf) {
    let _ = std::fs::write(path, b"");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_hash_is_stable() {
        let h1 = generate_device_hash();
        let h2 = generate_device_hash();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex
    }

    #[test]
    fn test_marker_path_exists() {
        let path = telemetry_marker_path();
        assert!(path.to_string_lossy().contains("rtk"));
    }

    #[test]
    fn test_install_method_unix_paths() {
        assert_eq!(
            install_method_from_path("/opt/homebrew/Cellar/rtk/0.28.0/bin/rtk"),
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
            install_method_from_path("/home/user/.local/bin/rtk"),
            "script"
        );
        assert_eq!(
            install_method_from_path("/nix/store/abc123-rtk/bin/rtk"),
            "nix"
        );
        assert_eq!(install_method_from_path("/usr/bin/rtk"), "other");
    }

    #[test]
    fn test_install_method_windows_paths() {
        assert_eq!(
            install_method_from_path("C:\\Users\\user\\.cargo\\bin\\rtk.exe"),
            "cargo"
        );
        assert_eq!(
            install_method_from_path("C:\\Users\\user\\.local\\bin\\rtk.exe"),
            "script"
        );
        assert_eq!(
            install_method_from_path("C:\\Program Files\\rtk\\rtk.exe"),
            "other"
        );
    }

    #[test]
    fn test_detect_install_method_returns_known_value() {
        let method = detect_install_method();
        assert!(
            ["homebrew", "cargo", "script", "nix", "other", "unknown"].contains(&method),
            "Unexpected install method: {}",
            method
        );
    }
}
