//! Optional usage ping so we know which commands people run most.

use crate::core::config;
use crate::core::tracking;
use sha2::{Digest, Sha256};
use std::io::Write;
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
    let (commands_24h, top_commands, savings_pct, tokens_saved_24h, tokens_saved_total) =
        get_stats();

    let payload = serde_json::json!({
        "device_hash": device_hash,
        "version": version,
        "os": os,
        "arch": arch,
        "install_method": install_method,
        "commands_24h": commands_24h,
        "top_commands": top_commands,
        "savings_pct": savings_pct,
        "tokens_saved_24h": tokens_saved_24h,
        "tokens_saved_total": tokens_saved_total,
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
    let salt = get_or_create_salt();
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let username = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();

    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update(b":");
    hasher.update(hostname.as_bytes());
    hasher.update(b":");
    hasher.update(username.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn get_or_create_salt() -> String {
    let salt_path = salt_file_path();

    if let Ok(contents) = std::fs::read_to_string(&salt_path) {
        let trimmed = contents.trim().to_string();
        if trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
            return trimmed;
        }
    }

    let salt = random_salt();
    if let Some(parent) = salt_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::File::create(&salt_path) {
        let _ = f.write_all(salt.as_bytes());
    }
    salt
}

fn random_salt() -> String {
    let mut buf = [0u8; 32];
    if getrandom::fill(&mut buf).is_err() {
        let fallback = format!("{:?}:{}", std::time::SystemTime::now(), std::process::id());
        let mut hasher = Sha256::new();
        hasher.update(fallback.as_bytes());
        return format!("{:x}", hasher.finalize());
    }
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

fn salt_file_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("rtk")
        .join(".device_salt")
}

fn get_stats() -> (i64, Vec<String>, Option<f64>, i64, i64) {
    let tracker = match tracking::Tracker::new() {
        Ok(t) => t,
        Err(_) => return (0, vec![], None, 0, 0),
    };

    let since_24h = chrono::Utc::now() - chrono::Duration::hours(24);

    // Get 24h command count and top commands from tracking DB
    let commands_24h = tracker.count_commands_since(since_24h).unwrap_or(0);

    let top_commands = tracker.top_commands(5).unwrap_or_default();

    let savings_pct = tracker.overall_savings_pct().ok();

    let tokens_saved_24h = tracker.tokens_saved_24h(since_24h).unwrap_or(0);

    let tokens_saved_total = tracker.total_tokens_saved().unwrap_or(0);

    (
        commands_24h,
        top_commands,
        savings_pct,
        tokens_saved_24h,
        tokens_saved_total,
    )
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
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn test_device_hash_is_valid_hex() {
        let hash = generate_device_hash();
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_salt_is_persisted() {
        let s1 = get_or_create_salt();
        let s2 = get_or_create_salt();
        assert_eq!(s1, s2);
        assert_eq!(s1.len(), 64);
        assert!(s1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_random_salt_uniqueness() {
        let s1 = random_salt();
        let s2 = random_salt();
        assert_ne!(s1, s2);
        assert_eq!(s1.len(), 64);
        assert_eq!(s2.len(), 64);
    }

    #[test]
    fn test_salt_file_path_is_in_rtk_dir() {
        let path = salt_file_path();
        assert!(path.to_string_lossy().contains("rtk"));
        assert!(path.to_string_lossy().contains(".device_salt"));
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

    #[test]
    fn test_get_stats_returns_tuple() {
        let (cmds, top, pct, saved_24h, saved_total) = get_stats();
        assert!(cmds >= 0);
        assert!(top.len() <= 5);
        assert!(saved_24h >= 0);
        assert!(saved_total >= 0);
        if let Some(p) = pct {
            assert!((0.0..=100.0).contains(&p));
        }
    }
}
