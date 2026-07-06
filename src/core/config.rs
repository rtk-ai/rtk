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
    pub retriever: crate::core::retriever::RetrieverConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tee: Option<LegacyTeeConfig>,
    #[serde(skip)]
    pub migrated_from_legacy_tee: bool,
    #[serde(default)]
    pub telemetry: TelemetryConfig,
    #[serde(default)]
    pub hooks: HooksConfig,
    #[serde(default)]
    pub limits: LimitsConfig,
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(default)]
struct LegacyTeeConfig {
    enabled: Option<bool>,
    mode: Option<String>,
    max_files: Option<usize>,
    max_file_size: Option<usize>,
    directory: Option<PathBuf>,
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

/// Get `(exclude_commands, transparent_prefixes)` for hook-rewrite decisions.
/// Falls back to empty (no exclusions/prefixes) if config can't be loaded.
/// Shared by every place that decides whether/how to rewrite a command
/// (`hooks::hook_cmd`, `hooks::rewrite_cmd`, `discover`, `rtk rewrite`'s CLI
/// entry point in `main.rs`) so they can't drift from each other.
///
/// Reads the process-wide cached config (see `cached_config`), not a fresh
/// `Config::load()`: this is on the PreToolUse hook's hot path, and
/// `tracking::get_db_path` (called via `Tracker::new()` for `hook_decisions`
/// logging, right after this in the same hook invocation) also reads config —
/// without caching, that's two full disk-read-plus-TOML-parse round trips per
/// single Bash tool call instead of one.
pub fn hook_rewrite_params() -> (Vec<String>, Vec<String>) {
    let c = cached_config();
    (
        c.hooks.exclude_commands.clone(),
        c.hooks.transparent_prefixes.clone(),
    )
}

/// Process-wide cached `Config::load()` result, populated on first use.
///
/// Safe for read-only callers on hot paths that may load config multiple times
/// within a single `rtk` invocation (a `rtk` process is short-lived and exits
/// after one subcommand, so there's no cross-invocation staleness to worry
/// about) — but NOT used by any path that mutates and saves config within the
/// same process run (e.g. `hooks::init::save_telemetry_consent`'s load-mutate-save),
/// since those must always observe a fresh read. Only reach for this from a
/// caller that never itself writes config.toml.
pub(crate) fn cached_config() -> &'static Config {
    static CACHE: std::sync::OnceLock<Config> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Config::load().unwrap_or_default())
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = get_config_path()?;

        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            Self::from_toml(&content)
        } else {
            Ok(Config::default())
        }
    }

    fn from_toml(content: &str) -> Result<Self> {
        let value: toml::Value = toml::from_str(content)?;
        let has_retriever = value.get("retriever").is_some();
        let mut config = Config::deserialize(value)?;
        config.migrate_legacy_tee(has_retriever);
        Ok(config)
    }

    fn migrate_legacy_tee(&mut self, has_retriever: bool) {
        let Some(tee) = self.tee.take() else {
            return;
        };
        if has_retriever {
            return;
        }
        self.migrated_from_legacy_tee = true;
        use crate::core::retriever::RecoveryMode;
        let r = &mut self.retriever;
        if tee.enabled == Some(false) || tee.mode.as_deref() == Some("never") {
            r.mode = RecoveryMode::Disabled;
        } else {
            r.mode = RecoveryMode::Tee;
        }
        if let Some(v) = tee.max_files {
            r.tee_max_files = v;
        }
        if let Some(v) = tee.max_file_size {
            r.tee_max_file_size = v;
        }
        if let Some(d) = tee.directory {
            r.tee_directory = Some(d);
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

fn apply_recall_mode(content: &str, mode: crate::core::retriever::RecoveryMode) -> Result<String> {
    use crate::core::retriever::RecoveryMode;
    let mut doc: toml_edit::DocumentMut = content
        .parse()
        .map_err(|e| anyhow::anyhow!("config.toml is not valid TOML: {e}"))?;

    let legacy = doc.remove("tee");
    let retriever_is_table = doc
        .get("retriever")
        .is_some_and(|item| item.as_table_like().is_some());
    if !retriever_is_table {
        doc["retriever"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let mode_str = match mode {
        RecoveryMode::Sqlite => "sqlite",
        RecoveryMode::Tee => "tee",
        RecoveryMode::Disabled => "disabled",
    };
    doc["retriever"]["mode"] = toml_edit::value(mode_str);

    if let Some(legacy) = legacy.as_ref().and_then(|i| i.as_table_like()) {
        for (old_key, new_key) in [
            ("max_files", "tee_max_files"),
            ("max_file_size", "tee_max_file_size"),
            ("directory", "tee_directory"),
        ] {
            if let Some(v) = legacy.get(old_key).and_then(|i| i.as_value()) {
                if doc["retriever"].get(new_key).is_none() {
                    doc["retriever"][new_key] = toml_edit::Item::Value(v.clone());
                }
            }
        }
    }
    Ok(doc.to_string())
}

pub fn set_recall_mode(mode: crate::core::retriever::RecoveryMode) -> Result<PathBuf> {
    let path = get_config_path()?;
    let content = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        String::new()
    };
    let updated = apply_recall_mode(&content, mode)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, updated)?;
    Ok(path)
}

pub fn show_recall_mode() -> Result<()> {
    use crate::core::retriever::RecoveryMode;
    let config = Config::load().unwrap_or_default();
    let mode = match config.retriever.mode {
        RecoveryMode::Sqlite => "sqlite",
        RecoveryMode::Tee => "tee",
        RecoveryMode::Disabled => "disabled",
    };
    println!("recall mode: {mode}");
    if config.migrated_from_legacy_tee {
        println!("source: legacy [tee] section (auto-migrated at load)");
    }
    if std::env::var("RTK_RECALL").ok().as_deref() == Some("0")
        || std::env::var("RTK_TEE").ok().as_deref() == Some("0")
    {
        println!("note: RTK_RECALL=0/RTK_TEE=0 is set — recovery disabled for this environment");
    }
    println!("change with: rtk config recall <sqlite|tee|disabled>");
    Ok(())
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

    #[test]
    fn test_legacy_tee_disabled_maps_to_disabled_mode() {
        use crate::core::retriever::RecoveryMode;
        let toml = r#"
[tee]
enabled = false
"#;
        let config = Config::from_toml(toml).expect("valid toml");
        assert_eq!(config.retriever.mode, RecoveryMode::Disabled);
    }

    #[test]
    fn test_legacy_tee_never_mode_maps_to_disabled_mode() {
        use crate::core::retriever::RecoveryMode;
        let toml = r#"
[tee]
mode = "never"
"#;
        let config = Config::from_toml(toml).expect("valid toml");
        assert_eq!(config.retriever.mode, RecoveryMode::Disabled);
    }

    #[test]
    fn test_legacy_tee_section_maps_to_tee_mode_with_fields() {
        use crate::core::retriever::RecoveryMode;
        let toml = r#"
[tee]
enabled = true
mode = "failures"
max_files = 7
max_file_size = 4096
directory = "/custom/tee"
"#;
        let config = Config::from_toml(toml).expect("valid toml");
        assert_eq!(config.retriever.mode, RecoveryMode::Tee);
        assert_eq!(config.retriever.tee_max_files, 7);
        assert_eq!(config.retriever.tee_max_file_size, 4096);
        assert_eq!(
            config.retriever.tee_directory,
            Some(PathBuf::from("/custom/tee"))
        );
    }

    #[test]
    fn test_retriever_section_wins_over_legacy_tee() {
        use crate::core::retriever::RecoveryMode;
        let toml = r#"
[retriever]
mode = "sqlite"

[tee]
enabled = false
"#;
        let config = Config::from_toml(toml).expect("valid toml");
        assert_eq!(config.retriever.mode, RecoveryMode::Sqlite);
    }

    #[test]
    fn test_migrated_flag_set_only_on_legacy_migration() {
        let migrated = Config::from_toml("[tee]\nenabled = true\n").expect("valid");
        assert!(migrated.migrated_from_legacy_tee);
        let explicit = Config::from_toml("[retriever]\nmode = \"tee\"\n\n[tee]\nenabled = true\n")
            .expect("valid");
        assert!(!explicit.migrated_from_legacy_tee);
        let fresh = Config::from_toml("").expect("valid");
        assert!(!fresh.migrated_from_legacy_tee);
    }

    #[test]
    fn test_no_tee_section_defaults_to_sqlite() {
        use crate::core::retriever::RecoveryMode;
        let config = Config::from_toml("").expect("valid toml");
        assert_eq!(config.retriever.mode, RecoveryMode::Sqlite);
    }

    #[test]
    fn test_apply_recall_mode_preserves_other_content() {
        use crate::core::retriever::RecoveryMode;
        let input = "# my personal notes\n[hooks]\nexclude_commands = [\"curl\"]\n\n[tee]\nenabled = true\nmode = \"failures\"\n";
        let out = apply_recall_mode(input, RecoveryMode::Sqlite).expect("valid");
        assert!(out.contains("# my personal notes"));
        assert!(out.contains("exclude_commands = [\"curl\"]"));
        assert!(!out.contains("[tee]"), "legacy section must be removed");
        assert!(out.contains("[retriever]"));
        assert!(out.contains("mode = \"sqlite\""));
        let reparsed = Config::from_toml(&out).expect("output must stay valid");
        assert_eq!(reparsed.retriever.mode, RecoveryMode::Sqlite);
    }

    #[test]
    fn test_apply_recall_mode_updates_existing_retriever() {
        use crate::core::retriever::RecoveryMode;
        let input = "[retriever]\nmode = \"sqlite\"\nmax_entries = 50\n";
        let out = apply_recall_mode(input, RecoveryMode::Tee).expect("valid");
        assert!(out.contains("mode = \"tee\""));
        assert!(out.contains("max_entries = 50"), "sibling keys preserved");
    }

    #[test]
    fn test_apply_recall_mode_replaces_scalar_retriever_key() {
        use crate::core::retriever::RecoveryMode;
        let out = apply_recall_mode("retriever = \"sqlite\"\n", RecoveryMode::Tee)
            .expect("must not panic on a scalar retriever key");
        let reparsed = Config::from_toml(&out).expect("valid output");
        assert_eq!(reparsed.retriever.mode, RecoveryMode::Tee);
    }

    #[test]
    fn test_apply_recall_mode_from_empty_file() {
        use crate::core::retriever::RecoveryMode;
        let out = apply_recall_mode("", RecoveryMode::Disabled).expect("valid");
        let reparsed = Config::from_toml(&out).expect("valid output");
        assert_eq!(reparsed.retriever.mode, RecoveryMode::Disabled);
    }

    #[test]
    fn test_apply_recall_mode_carries_legacy_tee_fields() {
        use crate::core::retriever::RecoveryMode;
        let input = "[tee]\nmax_files = 7\ndirectory = \"/custom/tee\"\n";
        let out = apply_recall_mode(input, RecoveryMode::Tee).expect("valid");
        let reparsed = Config::from_toml(&out).expect("valid output");
        assert_eq!(reparsed.retriever.mode, RecoveryMode::Tee);
        assert_eq!(reparsed.retriever.tee_max_files, 7);
        assert_eq!(
            reparsed.retriever.tee_directory,
            Some(PathBuf::from("/custom/tee"))
        );
    }
}
