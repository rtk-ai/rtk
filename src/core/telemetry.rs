//! Optional usage ping so we know which commands people run most.

use super::constants::RTK_DATA_DIR;
use crate::core::config;
use crate::core::tracking;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

static CACHED_SALT: OnceLock<String> = OnceLock::new();

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

    // Get stats from tracking DB (single connection for both basic + enriched)
    let tracker = tracking::Tracker::new().ok();
    let (commands_24h, top_commands, savings_pct, tokens_saved_24h, tokens_saved_total) =
        match &tracker {
            Some(t) => get_stats(t),
            None => (0, vec![], None, 0, 0),
        };
    let enriched = match &tracker {
        Some(t) => get_enriched_stats(t),
        None => EnrichedStats {
            passthrough_top: vec![],
            parse_failures_24h: 0,
            low_savings_commands: vec![],
            avg_savings_per_command: 0.0,
            hook_type: detect_hook_type(),
            custom_toml_filters: count_custom_toml_filters(),
            first_seen_days: 0,
            active_days_30d: 0,
            commands_total: 0,
            ecosystem_mix: serde_json::json!({}),
            tokens_saved_30d: 0,
            estimated_savings_usd_30d: 0.0,
            has_config_toml: detect_has_config(),
            exclude_commands_count: count_exclude_commands(),
            projects_count: 0,
            meta_usage: serde_json::json!({}),
        },
    };

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
        // Quality: identify gaps and weak filters
        "passthrough_top": enriched.passthrough_top,
        "parse_failures_24h": enriched.parse_failures_24h,
        "low_savings_commands": enriched.low_savings_commands,
        "avg_savings_per_command": enriched.avg_savings_per_command,
        // Adoption: which tools and configs
        "hook_type": enriched.hook_type,
        "custom_toml_filters": enriched.custom_toml_filters,
        // Retention: engagement signals
        "first_seen_days": enriched.first_seen_days,
        "active_days_30d": enriched.active_days_30d,
        "commands_total": enriched.commands_total,
        // Ecosystem: where to invest filters
        "ecosystem_mix": enriched.ecosystem_mix,
        // Economics: value delivered
        "tokens_saved_30d": enriched.tokens_saved_30d,
        "estimated_savings_usd_30d": enriched.estimated_savings_usd_30d,
        // Configuration: user maturity
        "has_config_toml": enriched.has_config_toml,
        "exclude_commands_count": enriched.exclude_commands_count,
        "projects_count": enriched.projects_count,
        // Meta-commands: feature adoption
        "meta_usage": enriched.meta_usage,
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
    CACHED_SALT
        .get_or_init(|| {
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
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(
                        &salt_path,
                        std::fs::Permissions::from_mode(0o600),
                    );
                }
            }
            salt
        })
        .clone()
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

fn get_stats(tracker: &tracking::Tracker) -> (i64, Vec<String>, Option<f64>, i64, i64) {
    let since_24h = chrono::Utc::now() - chrono::Duration::hours(24);

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

struct EnrichedStats {
    // Quality: identify gaps and weak filters
    passthrough_top: Vec<String>,
    parse_failures_24h: i64,
    low_savings_commands: Vec<String>,
    avg_savings_per_command: f64,
    // Adoption: which tools and configs
    hook_type: String,
    custom_toml_filters: usize,
    // Retention: engagement signals
    first_seen_days: i64,
    active_days_30d: i64,
    commands_total: i64,
    // Ecosystem: where to invest filters
    ecosystem_mix: serde_json::Value,
    // Economics: value delivered
    tokens_saved_30d: i64,
    estimated_savings_usd_30d: f64,
    // Configuration: user maturity
    has_config_toml: bool,
    exclude_commands_count: usize,
    projects_count: i64,
    // Meta-commands: feature adoption
    meta_usage: serde_json::Value,
}

fn get_enriched_stats(tracker: &tracking::Tracker) -> EnrichedStats {
    let since_24h = chrono::Utc::now() - chrono::Duration::hours(24);

    let passthrough_top = tracker
        .top_passthrough(5)
        .unwrap_or_default()
        .into_iter()
        .map(|(cmd, count)| format!("{}:{}", cmd, count))
        .collect();

    let parse_failures_24h = tracker.parse_failures_since(since_24h).unwrap_or(0);

    let low_savings_commands = tracker
        .low_savings_commands(5)
        .unwrap_or_default()
        .into_iter()
        .map(|(cmd, pct)| format!("{}:{:.0}%", cmd, pct))
        .collect();

    let avg_savings_per_command = tracker.avg_savings_per_command().unwrap_or(0.0);

    let first_seen_days = tracker.first_seen_days().unwrap_or(0);
    let active_days_30d = tracker.active_days_30d().unwrap_or(0);
    let commands_total = tracker.commands_total().unwrap_or(0);

    let ecosystem_mix = serde_json::Value::Object(
        tracker
            .ecosystem_mix()
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| (k, serde_json::json!(v)))
            .collect(),
    );

    let tokens_saved_30d = tracker.tokens_saved_30d().unwrap_or(0);
    // Estimate USD savings: tokens_saved are input tokens (CLI output compressed before
    // reaching the LLM). Use input pricing: Claude Sonnet $3/Mtok.
    let estimated_savings_usd_30d = tokens_saved_30d as f64 / 1_000_000.0 * 3.0;

    let projects_count = tracker.projects_count().unwrap_or(0);

    let meta_usage = build_meta_usage(&tracker);

    EnrichedStats {
        passthrough_top,
        parse_failures_24h,
        low_savings_commands,
        avg_savings_per_command,
        hook_type: detect_hook_type(),
        custom_toml_filters: count_custom_toml_filters(),
        first_seen_days,
        active_days_30d,
        commands_total,
        ecosystem_mix,
        tokens_saved_30d,
        estimated_savings_usd_30d,
        projects_count,
        has_config_toml: detect_has_config(),
        exclude_commands_count: count_exclude_commands(),
        meta_usage,
    }
}

/// Build meta-command usage counts (gain, discover, proxy, verify, learn, init).
fn build_meta_usage(tracker: &tracking::Tracker) -> serde_json::Value {
    let meta_cmds = ["gain", "discover", "proxy", "verify", "learn", "init"];
    let mut usage = serde_json::Map::new();
    for meta in &meta_cmds {
        let count = tracker.count_meta_command(meta).unwrap_or(0);
        if count > 0 {
            usage.insert(meta.to_string(), serde_json::json!(count));
        }
    }
    serde_json::Value::Object(usage)
}

/// Check if user has a config.toml file.
fn detect_has_config() -> bool {
    dirs::config_dir()
        .map(|d| d.join("rtk/config.toml").exists())
        .unwrap_or(false)
}

/// Count commands in exclude_commands config.
fn count_exclude_commands() -> usize {
    crate::core::config::Config::load()
        .map(|c| c.hooks.exclude_commands.len())
        .unwrap_or(0)
}

/// Detect which AI agent hook is installed.
fn detect_hook_type() -> String {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return "unknown".to_string(),
    };

    // Check in order of popularity
    let checks = [
        (home.join(".claude/hooks/rtk-rewrite.sh"), "claude"),
        (home.join(".claude/hooks/rtk-rewrite.json"), "claude"),
        (home.join(".gemini/hooks/rtk-hook.sh"), "gemini"),
        (home.join(".codex/AGENTS.md"), "codex"),
        (home.join(".cursor/hooks/rtk-rewrite.json"), "cursor"),
    ];

    for (path, name) in &checks {
        if path.exists() {
            return name.to_string();
        }
    }

    // Check project-level hooks
    if let Ok(cwd) = std::env::current_dir() {
        if cwd.join(".claude/hooks/rtk-rewrite.sh").exists() {
            return "claude".to_string();
        }
    }

    "none".to_string()
}

/// Count user-defined TOML filter files (project-local + global).
fn count_custom_toml_filters() -> usize {
    let mut count = 0;

    // Project-local: .rtk/filters/*.toml
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(entries) = std::fs::read_dir(cwd.join(".rtk/filters")) {
            count += entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "toml"))
                .count();
        }
    }

    // Global: ~/.config/rtk/filters/*.toml
    if let Some(config_dir) = dirs::config_dir() {
        if let Ok(entries) = std::fs::read_dir(config_dir.join("rtk/filters")) {
            count += entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "toml"))
                .count();
        }
    }

    count
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
        .join(RTK_DATA_DIR);
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
        let tracker = match tracking::Tracker::new() {
            Ok(t) => t,
            Err(_) => return, // No DB — skip
        };
        let (cmds, top, pct, saved_24h, saved_total) = get_stats(&tracker);
        assert!(cmds >= 0);
        assert!(top.len() <= 5);
        assert!(saved_24h >= 0);
        assert!(saved_total >= 0);
        if let Some(p) = pct {
            assert!((0.0..=100.0).contains(&p));
        }
    }

    #[test]
    fn test_enriched_stats_returns_valid_data() {
        let tracker = match tracking::Tracker::new() {
            Ok(t) => t,
            Err(_) => return,
        };
        let stats = get_enriched_stats(&tracker);
        assert!(stats.passthrough_top.len() <= 5);
        assert!(stats.parse_failures_24h >= 0);
        assert!(stats.low_savings_commands.len() <= 5);
        assert!((0.0..=100.0).contains(&stats.avg_savings_per_command));
        assert!(
            ["claude", "gemini", "codex", "cursor", "none", "unknown"]
                .iter()
                .any(|&h| stats.hook_type.starts_with(h)),
            "Unexpected hook type: {}",
            stats.hook_type
        );
    }

    #[test]
    fn test_detect_hook_type_returns_known() {
        let ht = detect_hook_type();
        assert!(
            ["claude", "gemini", "codex", "cursor", "none", "unknown"].contains(&ht.as_str()),
            "Unexpected hook type: {}",
            ht
        );
    }

    #[test]
    fn test_count_custom_toml_filters() {
        // Should not panic even if directories don't exist
        let count = count_custom_toml_filters();
        assert!(count < 10000); // sanity check
    }
}
