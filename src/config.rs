use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub tracking: TrackingConfig,
    #[serde(default)]
    pub display: DisplayConfig,
    #[serde(default)]
    pub filters: FilterConfig,
    #[serde(default)]
    pub tee: crate::tee::TeeConfig,
    #[serde(default)]
    pub telemetry: TelemetryConfig,
    #[serde(default)]
    pub hooks: HooksConfig,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HooksConfig {
    /// Commands to exclude from auto-rewrite (e.g. ["curl", "playwright"]).
    /// Survives `rtk init -g` re-runs since config.toml is user-owned.
    #[serde(default)]
    pub exclude_commands: Vec<String>,

    /// When true, rewritten commands bypass Claude Code's permission prompt.
    /// When false, commands are still rewritten but the user is prompted before execution.
    /// Override: RTK_HOOK_AUTO_APPROVE=0 (or =1)
    #[serde(default = "default_true")]
    pub auto_approve: bool,

    /// Claude Code data directory for hook installation.
    /// Override: CLAUDE_CONFIG_DIR env var or --claude-dir CLI flag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_dir: Option<PathBuf>,
}

fn default_true() -> bool {
    true
}

impl Default for HooksConfig {
    fn default() -> Self {
        Self {
            exclude_commands: vec![],
            auto_approve: true,
            claude_dir: None,
        }
    }
}

/// Resolve the Claude Code data directory.
/// Priority: cli_override > CLAUDE_CONFIG_DIR env var > config.toml > ~/.claude
pub fn resolve_claude_dir(cli_override: Option<&Path>) -> anyhow::Result<PathBuf> {
    if let Some(dir) = cli_override {
        return Ok(dir.to_path_buf());
    }
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    if let Ok(config) = Config::load() {
        if let Some(dir) = config.hooks.claude_dir {
            return Ok(dir);
        }
    }
    dirs::home_dir()
        .map(|h| h.join(".claude"))
        .context("Cannot determine home directory. Is $HOME set?")
}

/// Parse a boolean-ish env var value. Accepts "1", "true", "yes" as truthy (case-insensitive).
/// Everything else (including empty string) is falsy.
fn parse_bool_env(val: &str) -> bool {
    matches!(val.to_lowercase().as_str(), "1" | "true" | "yes")
}

/// Resolve hooks.auto_approve with env var override, using an already-loaded config.
/// Priority: RTK_HOOK_AUTO_APPROVE env var > config value.
pub fn resolve_auto_approve_with(hooks: &HooksConfig) -> bool {
    if let Ok(val) = std::env::var("RTK_HOOK_AUTO_APPROVE") {
        return parse_bool_env(&val);
    }
    hooks.auto_approve
}

#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
pub struct TelemetryConfig {
    pub enabled: bool,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Resolve telemetry.enabled with env var override.
/// Priority: RTK_TELEMETRY_DISABLED env var > config.toml > default (true).
/// Note: env var uses inverted polarity — "1"/"true"/"yes" means DISABLED.
pub fn resolve_telemetry_enabled() -> bool {
    if let Ok(val) = std::env::var("RTK_TELEMETRY_DISABLED") {
        return !parse_bool_env(&val);
    }
    Config::load().map(|c| c.telemetry.enabled).unwrap_or(true)
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = get_config_path()?;

        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let config: Config = toml::from_str(&content)?;
            Ok(config)
        } else {
            Ok(Config::default())
        }
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

    pub fn create_default() -> Result<PathBuf> {
        let config = Config::default();
        config.save()?;
        get_config_path()
    }
}

fn get_config_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    Ok(config_dir.join("rtk").join("config.toml"))
}

pub fn show_config() -> Result<()> {
    let path = get_config_path()?;
    println!("Config: {}", path.display());
    println!();

    if path.exists() {
        let config = Config::load()?;
        println!("{}", toml::to_string_pretty(&config)?);
    } else {
        println!("(default config, file not created)");
        println!();
        let config = Config::default();
        println!("{}", toml::to_string_pretty(&config)?);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hooks_config_deserialize() {
        let toml = r#"
[hooks]
exclude_commands = ["curl", "gh"]
"#;
        let config: Config = toml::from_str(toml).expect("valid toml");
        assert_eq!(config.hooks.exclude_commands, vec!["curl", "gh"]);
        // auto_approve defaults to true when omitted
        assert!(config.hooks.auto_approve);
    }

    #[test]
    fn test_hooks_config_auto_approve_false() {
        let toml = r#"
[hooks]
auto_approve = false
exclude_commands = []
"#;
        let config: Config = toml::from_str(toml).expect("valid toml");
        assert!(!config.hooks.auto_approve);
    }

    #[test]
    fn test_hooks_config_default_empty() {
        let config = Config::default();
        assert!(config.hooks.exclude_commands.is_empty());
        assert!(config.hooks.auto_approve);
    }

    #[test]
    fn test_config_without_hooks_section_is_valid() {
        let toml = r#"
[tracking]
enabled = true
history_days = 90
"#;
        let config: Config = toml::from_str(toml).expect("valid toml");
        assert!(config.hooks.exclude_commands.is_empty());
        assert!(config.hooks.auto_approve);
    }

    #[test]
    fn test_hooks_config_claude_dir() {
        let toml = r#"
[hooks]
claude_dir = "/custom/claude"
"#;
        let config: Config = toml::from_str(toml).expect("valid toml");
        assert_eq!(
            config.hooks.claude_dir,
            Some(PathBuf::from("/custom/claude"))
        );
    }

    #[test]
    fn test_parse_bool_env_truthy() {
        assert!(parse_bool_env("1"));
        assert!(parse_bool_env("true"));
        assert!(parse_bool_env("TRUE"));
        assert!(parse_bool_env("True"));
        assert!(parse_bool_env("yes"));
        assert!(parse_bool_env("YES"));
    }

    #[test]
    fn test_parse_bool_env_falsy() {
        assert!(!parse_bool_env("0"));
        assert!(!parse_bool_env("false"));
        assert!(!parse_bool_env("no"));
        assert!(!parse_bool_env(""));
        assert!(!parse_bool_env("anything_else"));
    }

    #[test]
    fn test_resolve_auto_approve_with_config_default() {
        let hooks = HooksConfig::default();
        // Without env var set, uses config value (true by default)
        assert!(resolve_auto_approve_with(&hooks));
    }

    #[test]
    fn test_resolve_auto_approve_with_config_false() {
        let hooks = HooksConfig {
            auto_approve: false,
            ..Default::default()
        };
        assert!(!resolve_auto_approve_with(&hooks));
    }

    #[test]
    fn test_resolve_claude_dir_cli_override() {
        let override_path = Path::new("/custom/override");
        let result = resolve_claude_dir(Some(override_path)).unwrap();
        assert_eq!(result, PathBuf::from("/custom/override"));
    }

    #[test]
    fn test_resolve_claude_dir_cli_wins_over_env() {
        // CLI override takes priority even when CLAUDE_CONFIG_DIR is set
        let override_path = Path::new("/explicit/override");
        let result = resolve_claude_dir(Some(override_path)).unwrap();
        assert_eq!(result, PathBuf::from("/explicit/override"));
    }
}
