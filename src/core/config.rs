//! Reads user settings from config.toml.

use super::constants::{CONFIG_TOML, DEFAULT_HISTORY_DAYS, RTK_DATA_DIR};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub tracking: TrackingConfig,
    #[serde(default)]
    pub display: DisplayConfig,
    #[serde(default)]
    pub filters: FilterConfig,
    #[serde(default)]
    pub tee: crate::core::tee::TeeConfig,
    #[serde(default)]
    pub telemetry: TelemetryConfig,
    #[serde(default)]
    pub hooks: HooksConfig,
    #[serde(default)]
    pub limits: LimitsConfig,
    #[serde(default)]
    pub redaction: RedactionConfig,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RedactionConfig {
    /// Master switch. PII redaction is ON by default; `--no-redact` overrides per invocation.
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub email: bool,
    #[serde(default = "default_true")]
    pub phone: bool,
    /// Indian PAN card (ABCDE1234F).
    #[serde(default = "default_true")]
    pub pan: bool,
    /// Aadhaar: 12-digit candidates are Verhoeff-validated before redaction.
    #[serde(default = "default_true")]
    pub aadhaar: bool,
    /// Card numbers: 13-19 digit candidates are Luhn-validated before redaction.
    #[serde(default = "default_true")]
    pub card: bool,
    /// AWS keys, JWTs, bearer tokens, PEM private keys, api_key=/password= assignments.
    #[serde(default = "default_true")]
    pub secrets: bool,
    /// IPv4 addresses (loopback and 0.0.0.0 are kept visible).
    #[serde(default = "default_true")]
    pub ip: bool,
    /// Extra user-defined patterns: [[redaction.custom]] name = "...", pattern = "..."
    #[serde(default)]
    pub custom: Vec<CustomPattern>,
    /// Lines matching any of these regexes are never redacted.
    #[serde(default)]
    pub allowlist: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomPattern {
    pub name: String,
    pub pattern: String,
}

impl Default for RedactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            email: true,
            phone: true,
            pan: true,
            aadhaar: true,
            card: true,
            secrets: true,
            ip: true,
            custom: Vec::new(),
            allowlist: Vec::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct HooksConfig {
    /// Commands to exclude from auto-rewrite (e.g. ["curl", "playwright"]).
    /// Survives `rtk init -g` re-runs since config.toml is user-owned.
    #[serde(default)]
    pub exclude_commands: Vec<String>,

    /// Wrapper prefixes that should be transparently stripped before routing
    /// to a filter, then re-prepended on the rewrite. For example, with
    /// `transparent_prefixes = ["docker exec mycontainer"]`, the command
    /// `docker exec mycontainer git status` rewrites to
    /// `docker exec mycontainer rtk git status` instead of passing through
    /// unrewritten.
    ///
    /// Useful for any per-project env wrapper that sits in front of every
    /// command — e.g. `docker exec mycontainer`, `direnv exec .`, `poetry run`,
    /// or `bundle exec`.
    ///
    /// Matching is literal, not pattern-based. Configure the exact concrete
    /// prefix you actually use, such as `docker exec mycontainer`.
    ///
    /// Extends the built-in `SHELL_PREFIX_BUILTINS` list (`noglob`, `command`,
    /// `builtin`, `exec`, `nocorrect`) with user- or organization-specific
    /// wrappers. Matching is strict: a configured prefix `"foo bar"` matches
    /// a command that starts with `"foo bar "` (or strictly equals `"foo bar"`),
    /// not anything else.
    #[serde(default)]
    pub transparent_prefixes: Vec<String>,
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
            history_days: DEFAULT_HISTORY_DAYS as u32,
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

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct TelemetryConfig {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent_given: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent_date: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LimitsConfig {
    /// Max total grep results to show (default: 200)
    pub grep_max_results: usize,
    /// Max matches per file in grep output (default: 25)
    pub grep_max_per_file: usize,
    /// Max staged/modified files shown in git status (default: 15)
    pub status_max_files: usize,
    /// Max untracked files shown in git status (default: 10)
    pub status_max_untracked: usize,
    /// Max chars for parser passthrough fallback (default: 2000)
    pub passthrough_max_chars: usize,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            grep_max_results: 200,
            grep_max_per_file: 25,
            status_max_files: 15,
            status_max_untracked: 10,
            passthrough_max_chars: 2000,
        }
    }
}

/// Get limits config. Falls back to defaults if config can't be loaded.
pub fn limits() -> LimitsConfig {
    Config::load().map(|c| c.limits).unwrap_or_default()
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
    Ok(config_dir.join(RTK_DATA_DIR).join(CONFIG_TOML))
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
    }

    #[test]
    fn test_hooks_config_default_empty() {
        let config = Config::default();
        assert!(config.hooks.exclude_commands.is_empty());
        assert!(config.hooks.transparent_prefixes.is_empty());
    }

    #[test]
    fn test_hooks_config_transparent_prefixes_deserialize() {
        let toml = r#"
[hooks]
transparent_prefixes = ["direnv exec .", "nix develop --command"]
"#;
        let config: Config = toml::from_str(toml).expect("valid toml");
        assert_eq!(
            config.hooks.transparent_prefixes,
            vec!["direnv exec .", "nix develop --command"]
        );
    }

    #[test]
    fn test_hooks_config_transparent_prefixes_missing_is_empty() {
        // Older configs that predate this field must still parse.
        let toml = r#"
[hooks]
exclude_commands = ["curl"]
"#;
        let config: Config = toml::from_str(toml).expect("valid toml");
        assert_eq!(config.hooks.exclude_commands, vec!["curl"]);
        assert!(config.hooks.transparent_prefixes.is_empty());
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
    }

    #[test]
    fn test_old_toml_without_consent_fields() {
        let toml = r#"
[telemetry]
enabled = true
"#;
        let config: Config = toml::from_str(toml).expect("valid toml");
        assert!(config.telemetry.enabled);
        assert!(config.telemetry.consent_given.is_none());
        assert!(config.telemetry.consent_date.is_none());
    }

    #[test]
    fn test_telemetry_default_disabled() {
        let config = Config::default();
        assert!(!config.telemetry.enabled);
        assert!(config.telemetry.consent_given.is_none());
    }

    #[test]
    fn test_redaction_config_default_enabled() {
        let config = Config::default();
        assert!(config.redaction.enabled);
        assert!(config.redaction.email);
        assert!(config.redaction.phone);
        assert!(config.redaction.pan);
        assert!(config.redaction.aadhaar);
        assert!(config.redaction.card);
        assert!(config.redaction.secrets);
        assert!(config.redaction.custom.is_empty());
        assert!(config.redaction.allowlist.is_empty());
    }

    #[test]
    fn test_redaction_config_deserialize_disabled() {
        let toml = r#"
[redaction]
enabled = false
"#;
        let config: Config = toml::from_str(toml).expect("valid toml");
        assert!(!config.redaction.enabled);
        // Category toggles keep their defaults when the section only sets `enabled`.
        assert!(config.redaction.email);
    }

    #[test]
    fn test_redaction_config_per_category_toggle() {
        let toml = r#"
[redaction]
email = false
card = false
"#;
        let config: Config = toml::from_str(toml).expect("valid toml");
        assert!(config.redaction.enabled);
        assert!(!config.redaction.email);
        assert!(!config.redaction.card);
        assert!(config.redaction.phone);
    }

    #[test]
    fn test_redaction_config_custom_patterns_deserialize() {
        let toml = r#"
[redaction]
allowlist = ["EXAMPLE-DO-NOT-REDACT"]

[[redaction.custom]]
name = "employee_id"
pattern = "EMP-\\d{6}"
"#;
        let config: Config = toml::from_str(toml).expect("valid toml");
        assert_eq!(config.redaction.custom.len(), 1);
        assert_eq!(config.redaction.custom[0].name, "employee_id");
        assert_eq!(config.redaction.custom[0].pattern, "EMP-\\d{6}");
        assert_eq!(config.redaction.allowlist, vec!["EXAMPLE-DO-NOT-REDACT"]);
    }

    #[test]
    fn test_config_without_redaction_section_is_valid() {
        // Older configs that predate [redaction] must still parse, with redaction ON.
        let toml = r#"
[tracking]
enabled = true
history_days = 90
"#;
        let config: Config = toml::from_str(toml).expect("valid toml");
        assert!(config.redaction.enabled);
    }

    #[test]
    fn test_telemetry_consent_roundtrip() {
        let toml = r#"
[telemetry]
enabled = true
consent_given = true
consent_date = "2026-04-10T12:00:00Z"
"#;
        let config: Config = toml::from_str(toml).expect("valid toml");
        assert_eq!(config.telemetry.consent_given, Some(true));
        assert_eq!(
            config.telemetry.consent_date.as_deref(),
            Some("2026-04-10T12:00:00Z")
        );
    }
}
