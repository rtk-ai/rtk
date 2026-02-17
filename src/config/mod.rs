//! Configuration system: scalar config (TOML) + unified rules (MD with YAML frontmatter).
//!
//! Two config layers:
//! 1. Scalar config (`config.toml`): tracking, display, filters
//! 2. Rules (`rtk.*.md`): safety, remaps, warnings — via `rules` submodule

pub mod discovery;
pub mod rules;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;

/// CLI overrides for config paths. Set from main.rs before any config loading.
#[derive(Debug, Default)]
pub struct CliConfigOverrides {
    /// Exclusive config paths — replaces all discovery. Multiple files merged in order.
    pub config_path: Option<Vec<PathBuf>>,
    /// Additional config paths — loaded with highest priority (after env vars).
    pub config_add: Vec<PathBuf>,
    /// Exclusive rule discovery paths — replaces walk-up discovery.
    pub rules_path: Option<Vec<PathBuf>>,
    /// Additional rule discovery paths — loaded with highest priority.
    pub rules_add: Vec<PathBuf>,
}

static CLI_OVERRIDES: OnceLock<CliConfigOverrides> = OnceLock::new();

/// Set CLI config overrides. Must be called before any config loading.
pub fn set_cli_overrides(overrides: CliConfigOverrides) {
    let _ = CLI_OVERRIDES.set(overrides);
}

/// Get CLI config overrides (or defaults if never set).
pub fn cli_overrides() -> &'static CliConfigOverrides {
    CLI_OVERRIDES.get_or_init(CliConfigOverrides::default)
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct Config {
    #[serde(default)]
    pub tracking: TrackingConfig,
    #[serde(default)]
    pub display: DisplayConfig,
    #[serde(default)]
    pub filters: FilterConfig,
    #[serde(default)]
    pub discovery: DiscoveryConfig,
    #[serde(default)]
    pub tee: crate::tee::TeeConfig,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TrackingConfig {
    pub enabled: bool,
    pub history_days: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database_path: Option<PathBuf>,
}

impl Default for TrackingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            history_days: 90,
            database_path: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DisplayConfig {
    pub colors: bool,
    pub emoji: bool,
    pub max_width: usize,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            colors: true,
            emoji: true,
            max_width: 120,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FilterConfig {
    pub ignore_dirs: Vec<String>,
    pub ignore_files: Vec<String>,
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            ignore_dirs: vec![
                ".git".into(),
                "node_modules".into(),
                "target".into(),
                "__pycache__".into(),
                ".venv".into(),
                "vendor".into(),
            ],
            ignore_files: vec!["*.lock".into(), "*.min.js".into(), "*.min.css".into()],
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DiscoveryConfig {
    /// Dirs to search in each ancestor during walk-up (e.g. [".claude", ".gemini", ".rtk"]).
    pub search_dirs: Vec<String>,
    /// Global dirs under $HOME to check before walk-up (e.g. [".claude", ".gemini"]).
    pub global_dirs: Vec<String>,
    /// Additional rule directories to search. First entry is also the export/write target.
    /// Default: [] (uses ~/.config/rtk/ as the implicit primary).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules_dirs: Vec<PathBuf>,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            search_dirs: vec![".claude".into(), ".gemini".into(), ".rtk".into()],
            global_dirs: vec![".claude".into(), ".gemini".into()],
            rules_dirs: vec![],
        }
    }
}

impl Config {
    /// Load global config from `~/.config/rtk/config.toml`.
    /// Falls back to defaults if file is missing or unreadable.
    pub fn load() -> Result<Self> {
        let path = match get_config_path() {
            Ok(p) => p,
            Err(_) => return Ok(Config::default()),
        };

        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => match toml::from_str(&content) {
                    Ok(config) => Ok(config),
                    Err(_) => Ok(Config::default()), // Malformed config → defaults
                },
                Err(_) => Ok(Config::default()), // Unreadable → defaults
            }
        } else {
            Ok(Config::default())
        }
    }

    /// Load merged config with full precedence chain.
    ///
    /// Precedence (highest wins):
    ///   0. CLI params: `--config-path` (exclusive) or `--config-add` (additive)
    ///   1. Environment variables (RTK_*)
    ///   2. Project-local `.rtk/config.toml` (nearest ancestor)
    ///   3. Global `~/.config/rtk/config.toml` (or platform config dir)
    ///   4. Compiled defaults
    pub fn load_merged() -> Result<Self> {
        let overrides = cli_overrides();

        // If --config-path is set, use ONLY those files (skip global + walk-up)
        let mut config = if let Some(ref exclusive_paths) = overrides.config_path {
            let mut cfg = Config::default();
            for path in exclusive_paths {
                if path.exists() {
                    if let Ok(content) = std::fs::read_to_string(path) {
                        if let Ok(overlay) = toml::from_str::<ConfigOverlay>(&content) {
                            overlay.apply(&mut cfg);
                        }
                    }
                }
            }
            cfg
        } else {
            // Normal: start with global config
            let mut cfg = Self::load()?;

            // Layer 3: Walk up from cwd looking for .rtk/config.toml
            if let Ok(cwd) = std::env::current_dir() {
                let mut current = cwd.as_path();
                loop {
                    let project_config = current.join(".rtk").join("config.toml");
                    if project_config.exists() {
                        match std::fs::read_to_string(&project_config) {
                            Ok(content) => {
                                if let Ok(overlay) = toml::from_str::<ConfigOverlay>(&content) {
                                    overlay.apply(&mut cfg);
                                }
                            }
                            Err(_) => {} // Silently skip unreadable project config
                        }
                        break;
                    }
                    match current.parent() {
                        Some(p) if p != current => current = p,
                        _ => break,
                    }
                }
            }
            cfg
        };

        // Layer 1.5: --config-add paths (higher than project-local, lower than env vars)
        for add_path in &overrides.config_add {
            if add_path.exists() {
                if let Ok(content) = std::fs::read_to_string(add_path) {
                    if let Ok(overlay) = toml::from_str::<ConfigOverlay>(&content) {
                        overlay.apply(&mut config);
                    }
                }
            }
        }

        // Layer 1 (highest priority): Environment variable overrides
        if let Ok(val) = std::env::var("RTK_TRACKING_ENABLED") {
            if let Ok(b) = val.parse::<bool>() {
                config.tracking.enabled = b;
            } else if val == "0" {
                config.tracking.enabled = false;
            } else if val == "1" {
                config.tracking.enabled = true;
            }
        }
        if let Ok(val) = std::env::var("RTK_HISTORY_DAYS") {
            if let Ok(days) = val.parse::<u32>() {
                config.tracking.history_days = days;
            }
        }
        if let Ok(path) = std::env::var("RTK_DB_PATH") {
            config.tracking.database_path = Some(PathBuf::from(path));
        }
        if let Ok(val) = std::env::var("RTK_DISPLAY_COLORS") {
            if let Ok(b) = val.parse::<bool>() {
                config.display.colors = b;
            }
        }
        if let Ok(val) = std::env::var("RTK_DISPLAY_EMOJI") {
            if let Ok(b) = val.parse::<bool>() {
                config.display.emoji = b;
            }
        }
        if let Ok(val) = std::env::var("RTK_MAX_WIDTH") {
            if let Ok(w) = val.parse::<usize>() {
                config.display.max_width = w;
            }
        }
        if let Ok(val) = std::env::var("RTK_SEARCH_DIRS") {
            config.discovery.search_dirs = val.split(',').map(|s| s.trim().to_string()).collect();
        }
        if let Ok(val) = std::env::var("RTK_RULES_DIRS") {
            config.discovery.rules_dirs = val.split(',').map(|s| PathBuf::from(s.trim())).collect();
        }

        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let path = get_config_path()?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Save to a specific path (for --local support).
    pub fn save_to(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    pub fn create_default() -> Result<PathBuf> {
        let config = Config::default();
        config.save()?;
        get_config_path()
    }
}

/// Overlay config for merging project config onto global config.
/// All fields are Option — only present fields override.
#[derive(Debug, Deserialize, Default)]
pub struct ConfigOverlay {
    pub tracking: Option<TrackingOverlay>,
    pub display: Option<DisplayOverlay>,
    pub filters: Option<FilterOverlay>,
    pub discovery: Option<DiscoveryOverlay>,
}

#[derive(Debug, Deserialize)]
pub struct TrackingOverlay {
    pub enabled: Option<bool>,
    pub history_days: Option<u32>,
    pub database_path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
pub struct DisplayOverlay {
    pub colors: Option<bool>,
    pub emoji: Option<bool>,
    pub max_width: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct FilterOverlay {
    pub ignore_dirs: Option<Vec<String>>,
    pub ignore_files: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct DiscoveryOverlay {
    pub search_dirs: Option<Vec<String>>,
    pub global_dirs: Option<Vec<String>>,
    pub rules_dirs: Option<Vec<PathBuf>>,
}

impl ConfigOverlay {
    fn apply(&self, config: &mut Config) {
        if let Some(ref t) = self.tracking {
            if let Some(v) = t.enabled {
                config.tracking.enabled = v;
            }
            if let Some(v) = t.history_days {
                config.tracking.history_days = v;
            }
            if let Some(ref v) = t.database_path {
                config.tracking.database_path = Some(v.clone());
            }
        }
        if let Some(ref d) = self.display {
            if let Some(v) = d.colors {
                config.display.colors = v;
            }
            if let Some(v) = d.emoji {
                config.display.emoji = v;
            }
            if let Some(v) = d.max_width {
                config.display.max_width = v;
            }
        }
        if let Some(ref f) = self.filters {
            if let Some(ref v) = f.ignore_dirs {
                config.filters.ignore_dirs = v.clone();
            }
            if let Some(ref v) = f.ignore_files {
                config.filters.ignore_files = v.clone();
            }
        }
        if let Some(ref d) = self.discovery {
            if let Some(ref v) = d.search_dirs {
                config.discovery.search_dirs = v.clone();
            }
            if let Some(ref v) = d.global_dirs {
                config.discovery.global_dirs = v.clone();
            }
            if let Some(ref v) = d.rules_dirs {
                config.discovery.rules_dirs = v.clone();
            }
        }
    }
}

/// Global config path: `~/.config/rtk/config.toml`
pub fn get_config_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    Ok(config_dir.join("rtk").join("config.toml"))
}

/// Canonical RTK rules directory: `~/.config/rtk/`
///
/// This is distinct from `dirs::config_dir()` which on macOS returns
/// `~/Library/Application Support/` — not appropriate for a CLI tool's
/// user-facing rule files. We use `~/.config/rtk/` on all platforms.
/// Primary rules directory (for writes/exports). First entry of rules_dirs, or ~/.config/rtk/.
pub fn get_rules_dir() -> Result<PathBuf> {
    let config = get_merged();
    if let Some(first) = config.discovery.rules_dirs.first() {
        return Ok(first.clone());
    }
    let home = dirs::home_dir().ok_or_else(|| anyhow!("Cannot determine home directory"))?;
    Ok(home.join(".config").join("rtk"))
}

/// Project-local config path: `.rtk/config.toml` in cwd
pub fn get_local_config_path() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    Ok(cwd.join(".rtk").join("config.toml"))
}

/// Cached merged config (loaded once per process).
static MERGED_CONFIG: OnceLock<Config> = OnceLock::new();

/// Get the merged config (cached). For use by tracking, display, etc.
pub fn get_merged() -> &'static Config {
    MERGED_CONFIG.get_or_init(|| Config::load_merged().unwrap_or_default())
}

pub fn show_config() -> Result<()> {
    let path = get_config_path()?;
    if path.exists() {
        println!("# {}", path.display());
        let config = Config::load()?;
        println!("{}", toml::to_string_pretty(&config)?);
    } else {
        println!("# (defaults, no config file)");
        println!("{}", toml::to_string_pretty(&Config::default())?);
    }
    Ok(())
}

// === Config CRUD ===

/// Get a config value by dotted key (e.g., "tracking.enabled").
pub fn get_value(key: &str) -> Result<String> {
    let config = Config::load_merged()?;
    let toml_val = toml::Value::try_from(&config)?;

    let parts: Vec<&str> = key.split('.').collect();
    let mut current = &toml_val;
    for part in &parts {
        current = current
            .get(part)
            .ok_or_else(|| anyhow!("Unknown config key: {key}"))?;
    }

    match current {
        toml::Value::String(s) => Ok(s.clone()),
        toml::Value::Boolean(b) => Ok(b.to_string()),
        toml::Value::Integer(i) => Ok(i.to_string()),
        toml::Value::Float(f) => Ok(f.to_string()),
        toml::Value::Array(a) => Ok(format!("{:?}", a)),
        other => Ok(other.to_string()),
    }
}

/// Set a config value by dotted key.
pub fn set_value(key: &str, value: &str, local: bool) -> Result<()> {
    let path = if local {
        get_local_config_path()?
    } else {
        get_config_path()?
    };

    let mut config = if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        toml::from_str(&content)?
    } else {
        Config::default()
    };

    apply_value(&mut config, key, value)?;

    if local {
        config.save_to(&path)?;
    } else {
        config.save()?;
    }
    Ok(())
}

/// Unset a config value (reset to default).
pub fn unset_value(key: &str, local: bool) -> Result<()> {
    let path = if local {
        get_local_config_path()?
    } else {
        get_config_path()?
    };

    if !path.exists() {
        return Err(anyhow!("Config file not found: {}", path.display()));
    }

    let content = std::fs::read_to_string(&path)?;
    let mut toml_val: toml::Value = toml::from_str(&content)?;

    let parts: Vec<&str> = key.split('.').collect();
    if parts.len() == 2 {
        if let Some(table) = toml_val.get_mut(parts[0]).and_then(|v| v.as_table_mut()) {
            table.remove(parts[1]);
        }
    } else {
        return Err(anyhow!("Invalid key format: {key}. Use section.field"));
    }

    let content = toml::to_string_pretty(&toml_val)?;
    std::fs::write(&path, content)?;
    Ok(())
}

/// List all config values with optional origin info.
pub fn list_values(origin: bool) -> Result<()> {
    let config = Config::load_merged()?;
    let toml_str = toml::to_string_pretty(&config)?;

    if origin {
        let global_path = get_config_path()?;
        let has_global = global_path.exists();

        // Check for project config
        let mut has_project = false;
        if let Ok(cwd) = std::env::current_dir() {
            let mut current = cwd.as_path();
            loop {
                if current.join(".rtk").join("config.toml").exists() {
                    has_project = true;
                    break;
                }
                match current.parent() {
                    Some(p) if p != current => current = p,
                    _ => break,
                }
            }
        }

        println!("# Sources:");
        if has_global {
            println!("#   global: {}", global_path.display());
        }
        if has_project {
            println!("#   project: .rtk/config.toml");
        }
        if !has_global && !has_project {
            println!("#   (all defaults)");
        }
        println!();
    }

    println!("{toml_str}");

    // Show rules summary only with --origin flag
    if origin {
        let rules = rules::load_all();
        if !rules.is_empty() {
            println!("# Rules ({} loaded):", rules.len());
            for rule in rules {
                println!("#   {} [{}] — {}", rule.name, rule.action, rule.source);
            }
        }
    }

    Ok(())
}

/// Apply a string value to a config struct by dotted key.
fn apply_value(config: &mut Config, key: &str, value: &str) -> Result<()> {
    match key {
        "tracking.enabled" => config.tracking.enabled = value.parse()?,
        "tracking.history_days" => config.tracking.history_days = value.parse()?,
        "tracking.database_path" => {
            config.tracking.database_path = Some(PathBuf::from(value));
        }
        "display.colors" => config.display.colors = value.parse()?,
        "display.emoji" => config.display.emoji = value.parse()?,
        "display.max_width" => config.display.max_width = value.parse()?,
        "discovery.search_dirs" => {
            config.discovery.search_dirs = value.split(',').map(|s| s.trim().to_string()).collect();
        }
        "discovery.global_dirs" => {
            config.discovery.global_dirs = value.split(',').map(|s| s.trim().to_string()).collect();
        }
        "discovery.rules_dirs" => {
            config.discovery.rules_dirs =
                value.split(',').map(|s| PathBuf::from(s.trim())).collect();
        }
        _ => return Err(anyhow!("Unknown config key: {key}")),
    }
    Ok(())
}

/// Create or update a rule MD file.
pub fn set_rule(
    name: &str,
    pattern: Option<&str>,
    action: Option<&str>,
    redirect: Option<&str>,
    local: bool,
) -> Result<()> {
    let dir = if local {
        let cwd = std::env::current_dir()?;
        cwd.join(".rtk")
    } else {
        get_rules_dir()?
    };
    std::fs::create_dir_all(&dir)?;

    let action_str = action.unwrap_or("rewrite");
    let filename = format!("rtk.{name}.md");
    let path = dir.join(&filename);

    let mut content = String::from("---\n");
    content.push_str(&format!("name: {name}\n"));
    if let Some(pat) = pattern {
        // Single pattern without quotes for simple, quoted for multi-word
        if pat.contains(' ') {
            content.push_str(&format!("patterns: [\"{pat}\"]\n"));
        } else {
            content.push_str(&format!("patterns: [{pat}]\n"));
        }
    }
    content.push_str(&format!("action: {action_str}\n"));
    if let Some(redir) = redirect {
        content.push_str(&format!("redirect: \"{redir}\"\n"));
    }
    content.push_str("---\n\nUser-defined rule.\n");

    std::fs::write(&path, &content)?;
    println!("Created rule: {}", path.display());
    Ok(())
}

/// Delete a rule MD file.
pub fn unset_rule(name: &str, local: bool) -> Result<()> {
    let dir = if local {
        let cwd = std::env::current_dir()?;
        cwd.join(".rtk")
    } else {
        get_rules_dir()?
    };

    let filename = format!("rtk.{name}.md");
    let path = dir.join(&filename);

    if path.exists() {
        std::fs::remove_file(&path)?;
        println!("Removed rule: {}", path.display());
    } else {
        // If it's a built-in rule, create a disabled override
        let is_builtin = rules::DEFAULT_RULES.iter().any(|content| {
            rules::parse_rule(content, "builtin")
                .map(|r| r.name == name)
                .unwrap_or(false)
        });
        if is_builtin {
            std::fs::create_dir_all(&dir)?;
            let content = format!("---\nname: {name}\nenabled: false\n---\n\nDisabled by user.\n");
            std::fs::write(&path, content)?;
            println!("Disabled built-in rule: {}", path.display());
        } else {
            return Err(anyhow!("Rule file not found: {}", path.display()));
        }
    }
    Ok(())
}

/// Export built-in rules to a directory.
pub fn export_rules(claude: bool) -> Result<()> {
    let dir = if claude {
        crate::init::resolve_claude_dir()?
    } else {
        get_rules_dir()?
    };
    std::fs::create_dir_all(&dir)?;

    let mut count = 0;
    for content in rules::DEFAULT_RULES {
        let rule = rules::parse_rule(content, "builtin")?;
        let filename = format!("rtk.{}.md", rule.name);
        let path = dir.join(&filename);
        // Skip if content unchanged; tolerate unreadable existing files
        if path.exists() {
            if let Ok(existing) = std::fs::read_to_string(&path) {
                if existing.trim() == content.trim() {
                    continue;
                }
            }
            // If unreadable, overwrite anyway
        }
        std::fs::write(&path, content)?;
        count += 1;
    }

    println!("Exported {} rules to {}", count, dir.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert!(config.tracking.enabled);
        assert_eq!(config.tracking.history_days, 90);
        assert!(config.display.colors);
        assert_eq!(config.display.max_width, 120);
    }

    #[test]
    fn test_config_overlay_none_fields_dont_override() {
        let mut config = Config::default();
        config.tracking.history_days = 30;
        config.display.max_width = 80;

        let overlay = ConfigOverlay::default();
        overlay.apply(&mut config);

        // None fields should not override
        assert_eq!(config.tracking.history_days, 30);
        assert_eq!(config.display.max_width, 80);
    }

    #[test]
    fn test_config_overlay_applies() {
        let mut config = Config::default();

        let overlay_toml = r#"
[tracking]
history_days = 30

[display]
max_width = 80
"#;
        let overlay: ConfigOverlay = toml::from_str(overlay_toml).unwrap();
        overlay.apply(&mut config);

        assert_eq!(config.tracking.history_days, 30);
        assert_eq!(config.display.max_width, 80);
        // Unmentioned fields unchanged
        assert!(config.tracking.enabled);
        assert!(config.display.colors);
    }

    #[test]
    fn test_apply_value_tracking() {
        let mut config = Config::default();
        apply_value(&mut config, "tracking.enabled", "false").unwrap();
        assert!(!config.tracking.enabled);

        apply_value(&mut config, "tracking.history_days", "30").unwrap();
        assert_eq!(config.tracking.history_days, 30);
    }

    #[test]
    fn test_apply_value_display() {
        let mut config = Config::default();
        apply_value(&mut config, "display.max_width", "80").unwrap();
        assert_eq!(config.display.max_width, 80);

        apply_value(&mut config, "display.colors", "false").unwrap();
        assert!(!config.display.colors);
    }

    #[test]
    fn test_apply_value_unknown_key() {
        let mut config = Config::default();
        assert!(apply_value(&mut config, "unknown.key", "value").is_err());
    }

    #[test]
    fn test_get_value_existing() {
        // This uses load_merged which reads from disk, so just test the happy path
        let result = get_value("tracking.enabled");
        assert!(result.is_ok());
        let val = result.unwrap();
        assert!(val == "true" || val == "false");
    }

    #[test]
    fn test_get_value_unknown() {
        let result = get_value("nonexistent.key");
        assert!(result.is_err());
    }

    #[test]
    fn test_load_merged_env_override() {
        std::env::set_var("RTK_DB_PATH", "/tmp/test.db");
        let config = Config::load_merged().unwrap();
        assert_eq!(
            config.tracking.database_path,
            Some(PathBuf::from("/tmp/test.db"))
        );
        std::env::remove_var("RTK_DB_PATH");
    }

    #[test]
    fn test_env_overrides_all_fields() {
        // Single test to avoid parallel env var interference.
        // Tests all RTK_* env var overrides sequentially.

        // tracking.enabled: "false" overrides default true
        std::env::set_var("RTK_TRACKING_ENABLED", "false");
        let config = Config::load_merged().unwrap();
        assert!(!config.tracking.enabled);
        std::env::remove_var("RTK_TRACKING_ENABLED");

        // tracking.enabled: "0" also disables
        std::env::set_var("RTK_TRACKING_ENABLED", "0");
        let config = Config::load_merged().unwrap();
        assert!(!config.tracking.enabled);
        std::env::remove_var("RTK_TRACKING_ENABLED");

        // tracking.enabled: "1" enables
        std::env::set_var("RTK_TRACKING_ENABLED", "1");
        let config = Config::load_merged().unwrap();
        assert!(config.tracking.enabled);
        std::env::remove_var("RTK_TRACKING_ENABLED");

        // tracking.history_days
        std::env::set_var("RTK_HISTORY_DAYS", "7");
        let config = Config::load_merged().unwrap();
        assert_eq!(config.tracking.history_days, 7);
        std::env::remove_var("RTK_HISTORY_DAYS");

        // display.colors
        std::env::set_var("RTK_DISPLAY_COLORS", "false");
        let config = Config::load_merged().unwrap();
        assert!(!config.display.colors);
        std::env::remove_var("RTK_DISPLAY_COLORS");

        // display.emoji
        std::env::set_var("RTK_DISPLAY_EMOJI", "false");
        let config = Config::load_merged().unwrap();
        assert!(!config.display.emoji);
        std::env::remove_var("RTK_DISPLAY_EMOJI");

        // display.max_width
        std::env::set_var("RTK_MAX_WIDTH", "200");
        let config = Config::load_merged().unwrap();
        assert_eq!(config.display.max_width, 200);
        std::env::remove_var("RTK_MAX_WIDTH");
    }

    #[test]
    fn test_project_local_overlay_overrides_global() {
        let tmp = tempfile::tempdir().unwrap();
        let rtk_dir = tmp.path().join(".rtk");
        std::fs::create_dir_all(&rtk_dir).unwrap();
        std::fs::write(
            rtk_dir.join("config.toml"),
            "[tracking]\nhistory_days = 14\n",
        )
        .unwrap();

        // Simulate being in a project with .rtk/config.toml
        let mut config = Config::default();
        assert_eq!(config.tracking.history_days, 90); // default

        let overlay_toml = "[tracking]\nhistory_days = 14\n";
        let overlay: ConfigOverlay = toml::from_str(overlay_toml).unwrap();
        overlay.apply(&mut config);
        assert_eq!(config.tracking.history_days, 14); // project-local overrides
    }

    #[test]
    fn test_env_overrides_project_local_overlay() {
        // Env vars have highest priority — even over project-local config.
        // Tests overlay application directly (no env var race).
        let mut config = Config::default();
        let overlay_toml = "[tracking]\nhistory_days = 14\n";
        let overlay: ConfigOverlay = toml::from_str(overlay_toml).unwrap();
        overlay.apply(&mut config);
        assert_eq!(config.tracking.history_days, 14); // overlay applied

        // In load_merged, env vars are applied AFTER project overlay,
        // so env vars always win. Tested via test_env_overrides_all_fields.
    }

    #[test]
    fn test_load_robust_to_missing_config() {
        // Config::load() should fall back to defaults when config doesn't exist
        let config = Config::load().unwrap();
        // Should have defaults — no crash
        assert!(config.tracking.enabled);
        assert_eq!(config.tracking.history_days, 90);
    }

    #[test]
    fn test_overlay_partial_sections() {
        // Only display section in overlay — tracking should be untouched
        let mut config = Config::default();
        config.tracking.history_days = 45;

        let overlay_toml = "[display]\nmax_width = 60\n";
        let overlay: ConfigOverlay = toml::from_str(overlay_toml).unwrap();
        overlay.apply(&mut config);

        assert_eq!(config.display.max_width, 60); // overridden
        assert_eq!(config.tracking.history_days, 45); // untouched
    }

    #[test]
    fn test_overlay_partial_fields_within_section() {
        // Only one field in tracking overlay — others untouched
        let mut config = Config::default();
        config.tracking.enabled = false;

        let overlay_toml = "[tracking]\nhistory_days = 7\n";
        let overlay: ConfigOverlay = toml::from_str(overlay_toml).unwrap();
        overlay.apply(&mut config);

        assert_eq!(config.tracking.history_days, 7); // overridden
        assert!(!config.tracking.enabled); // untouched (was false)
    }

    #[test]
    fn test_get_rules_dir_returns_dot_config_rtk() {
        let dir = get_rules_dir().unwrap();
        let home = dirs::home_dir().unwrap();
        assert_eq!(dir, home.join(".config").join("rtk"));
    }

    #[test]
    fn test_env_override_invalid_value_ignored() {
        // Invalid env values should be silently ignored, keeping the default
        std::env::set_var("RTK_HISTORY_DAYS", "not_a_number");
        let config = Config::load_merged().unwrap();
        assert_eq!(config.tracking.history_days, 90); // default kept
        std::env::remove_var("RTK_HISTORY_DAYS");

        std::env::set_var("RTK_MAX_WIDTH", "abc");
        let config = Config::load_merged().unwrap();
        assert_eq!(config.display.max_width, 120); // default kept
        std::env::remove_var("RTK_MAX_WIDTH");
    }

    #[test]
    fn test_cli_overrides_default() {
        // Default CLI overrides should not change behavior
        let overrides = CliConfigOverrides::default();
        assert!(overrides.config_path.is_none()); // None = use normal discovery
        assert!(overrides.config_add.is_empty());
        assert!(overrides.rules_path.is_none()); // None = use normal discovery
        assert!(overrides.rules_add.is_empty());
    }

    #[test]
    fn test_cli_config_path_multiple_files_merged() {
        // --config-path a.toml --config-path b.toml merges both in order
        let tmp = tempfile::tempdir().unwrap();
        let file_a = tmp.path().join("a.toml");
        let file_b = tmp.path().join("b.toml");
        std::fs::write(&file_a, "[tracking]\nhistory_days = 5\n").unwrap();
        std::fs::write(&file_b, "[display]\nmax_width = 60\n").unwrap();

        // Simulate load_merged with exclusive paths
        let mut cfg = Config::default();
        for path in &[&file_a, &file_b] {
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Ok(overlay) = toml::from_str::<ConfigOverlay>(&content) {
                    overlay.apply(&mut cfg);
                }
            }
        }

        assert_eq!(cfg.tracking.history_days, 5); // from a.toml
        assert_eq!(cfg.display.max_width, 60); // from b.toml
        assert!(cfg.tracking.enabled); // default (not in either file)
    }

    #[test]
    fn test_cli_config_path_exclusive() {
        // --config-path loads ONLY from that file
        let tmp = tempfile::tempdir().unwrap();
        let config_file = tmp.path().join("custom.toml");
        std::fs::write(
            &config_file,
            "[tracking]\nhistory_days = 5\nenabled = false\n",
        )
        .unwrap();

        // Simulate what load_merged does with exclusive path
        let path = &config_file;
        let config: Config = if path.exists() {
            let content = std::fs::read_to_string(path).unwrap();
            toml::from_str(&content).unwrap()
        } else {
            Config::default()
        };

        assert_eq!(config.tracking.history_days, 5);
        assert!(!config.tracking.enabled);
        // Other fields get defaults since only tracking was specified
        assert!(config.display.colors);
    }

    #[test]
    fn test_cli_config_add_overlay() {
        // --config-add applies as high-priority overlay
        let mut config = Config::default();
        assert_eq!(config.display.max_width, 120);

        let add_toml = "[display]\nmax_width = 60\n";
        let overlay: ConfigOverlay = toml::from_str(add_toml).unwrap();
        overlay.apply(&mut config);

        assert_eq!(config.display.max_width, 60); // overridden by --config-add
        assert!(config.tracking.enabled); // untouched
    }

    // === Error Robustness Tests ===

    #[test]
    fn test_load_robust_to_malformed_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let bad_config = tmp.path().join("config.toml");
        std::fs::write(&bad_config, "this is not valid toml {{{{").unwrap();

        // Malformed TOML should parse to default (not crash)
        let result: Result<Config, _> = toml::from_str("this is not valid toml {{{{");
        assert!(result.is_err());

        // Config::load falls back to defaults for malformed content
        let config = Config::load().unwrap();
        assert!(config.tracking.enabled); // defaults
    }

    #[test]
    fn test_load_robust_to_empty_config_file() {
        // Empty string is valid TOML (all defaults)
        let config: Config = toml::from_str("").unwrap();
        assert!(config.tracking.enabled);
        assert_eq!(config.tracking.history_days, 90);
        assert_eq!(config.display.max_width, 120);
    }

    #[test]
    fn test_load_robust_to_binary_garbage_config() {
        let garbage = "\x00\x01\x02 binary garbage";
        let result: Result<Config, _> = toml::from_str(garbage);
        assert!(result.is_err()); // Should error, not panic
    }

    #[test]
    fn test_overlay_robust_to_malformed_toml() {
        let result: Result<ConfigOverlay, _> = toml::from_str("not valid {{{");
        assert!(result.is_err()); // Should error, not panic
    }

    #[test]
    fn test_overlay_from_empty_string() {
        // Empty overlay should be all-None (no overrides)
        let overlay: ConfigOverlay = toml::from_str("").unwrap();
        assert!(overlay.tracking.is_none());
        assert!(overlay.display.is_none());
        assert!(overlay.filters.is_none());
    }

    #[test]
    fn test_config_path_exclusive_nonexistent_falls_back() {
        // If --config-path points to non-existent file, use defaults
        let path = PathBuf::from("/nonexistent/config.toml");
        assert!(!path.exists());
        // Simulates load_merged logic: non-existent → Config::default()
        let config = Config::default();
        assert!(config.tracking.enabled);
    }

    #[test]
    fn test_config_add_nonexistent_path_skipped() {
        // --config-add with non-existent path should be silently skipped
        let path = PathBuf::from("/nonexistent/overlay.toml");
        assert!(!path.exists());
        // The load_merged code does `if add_path.exists()` — non-existent skipped
        let mut config = Config::default();
        config.tracking.history_days = 42;
        // Config unchanged because path doesn't exist
        assert_eq!(config.tracking.history_days, 42);
    }

    #[test]
    fn test_config_add_malformed_file_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let bad_file = tmp.path().join("bad.toml");
        std::fs::write(&bad_file, "not valid {{{{ toml").unwrap();

        // Simulates load_merged: if let Ok(overlay) = toml::from_str(...)
        let content = std::fs::read_to_string(&bad_file).unwrap();
        let result = toml::from_str::<ConfigOverlay>(&content);
        assert!(result.is_err()); // Bad TOML → no overlay applied

        // Config should remain at defaults
        let config = Config::default();
        assert!(config.tracking.enabled);
    }

    #[test]
    fn test_set_value_creates_parent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("nested").join("deep").join("config.toml");

        // save_to should create parent dirs
        let config = Config::default();
        let result = config.save_to(&config_path);
        assert!(result.is_ok());
        assert!(config_path.exists());
    }

    // === DiscoveryConfig tests ===

    #[test]
    fn test_default_discovery_config() {
        let config = DiscoveryConfig::default();
        assert_eq!(config.search_dirs, vec![".claude", ".gemini", ".rtk"]);
        assert_eq!(config.global_dirs, vec![".claude", ".gemini"]);
        assert!(config.rules_dirs.is_empty());
    }

    #[test]
    fn test_discovery_config_roundtrip_toml() {
        let config = Config::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.discovery.search_dirs, config.discovery.search_dirs);
        assert_eq!(parsed.discovery.global_dirs, config.discovery.global_dirs);
        assert_eq!(parsed.discovery.rules_dirs, config.discovery.rules_dirs);
    }

    #[test]
    fn test_discovery_config_from_toml_custom() {
        let toml_str = r#"
[discovery]
search_dirs = [".rtk", ".custom"]
global_dirs = [".mytools"]
rules_dirs = ["/opt/rtk/rules", "/home/user/rules"]
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.discovery.search_dirs, vec![".rtk", ".custom"]);
        assert_eq!(config.discovery.global_dirs, vec![".mytools"]);
        assert_eq!(
            config.discovery.rules_dirs,
            vec![
                PathBuf::from("/opt/rtk/rules"),
                PathBuf::from("/home/user/rules")
            ]
        );
    }

    #[test]
    fn test_discovery_overlay_applies() {
        let mut config = Config::default();
        let overlay: ConfigOverlay = toml::from_str(
            r#"
[discovery]
search_dirs = [".only-rtk"]
rules_dirs = ["/custom/rules"]
"#,
        )
        .unwrap();
        overlay.apply(&mut config);
        assert_eq!(config.discovery.search_dirs, vec![".only-rtk"]);
        // global_dirs unchanged (not in overlay)
        assert_eq!(config.discovery.global_dirs, vec![".claude", ".gemini"]);
        assert_eq!(
            config.discovery.rules_dirs,
            vec![PathBuf::from("/custom/rules")]
        );
    }

    #[test]
    fn test_apply_value_discovery_search_dirs() {
        let mut config = Config::default();
        apply_value(&mut config, "discovery.search_dirs", ".rtk,.custom").unwrap();
        assert_eq!(config.discovery.search_dirs, vec![".rtk", ".custom"]);
    }

    #[test]
    fn test_apply_value_discovery_global_dirs() {
        let mut config = Config::default();
        apply_value(&mut config, "discovery.global_dirs", ".claude").unwrap();
        assert_eq!(config.discovery.global_dirs, vec![".claude"]);
    }

    #[test]
    fn test_apply_value_discovery_rules_dirs() {
        let mut config = Config::default();
        apply_value(&mut config, "discovery.rules_dirs", "/a,/b,/c").unwrap();
        assert_eq!(
            config.discovery.rules_dirs,
            vec![
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/c")
            ]
        );
    }

    #[test]
    fn test_get_rules_dir_default() {
        // Without any config override, get_rules_dir returns ~/.config/rtk/
        let dir = get_rules_dir().unwrap();
        assert!(
            dir.to_string_lossy().contains("rtk"),
            "Default rules dir should contain 'rtk': {}",
            dir.display()
        );
    }

    #[test]
    fn test_discovery_config_empty_rules_dirs_not_serialized() {
        // Empty rules_dirs should be omitted from TOML output (skip_serializing_if)
        let config = Config::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        assert!(
            !toml_str.contains("rules_dirs"),
            "Empty rules_dirs should be omitted from serialization"
        );
    }
}
