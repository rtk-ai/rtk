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
    #[serde(skip)]
    pub legacy_tee_fields_merged: bool,
    #[serde(default)]
    pub telemetry: TelemetryConfig,
    #[serde(default)]
    pub hooks: HooksConfig,
    #[serde(default)]
    pub limits: LimitsConfig,
    /// Per-tool behavior rules, evaluated top-to-bottom (first match wins per field).
    /// See docs/pr_briefs/005-per-tool-config-design. Empty by default → no behavior change.
    #[serde(default)]
    pub tools: Vec<ToolRule>,
    #[serde(default)]
    pub awareness: AwarenessConfig,
}

/// How rtk captures a child's stdout/stderr.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CaptureMode {
    /// Ordinary pipe (the historical default). Child sees a non-tty.
    #[default]
    Pipe,
    /// Pseudo-terminal: child behaves as in a real terminal (one-shot, clean exit).
    /// Fixes hangs where a detached descendant holds a captured pipe open
    /// (see docs/pr_briefs/001-pipe-eof-grandchild-hang).
    Pty,
}

/// A per-tool rule: when `match` applies, adjust capture/sanitization for that command.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolRule {
    #[serde(rename = "match")]
    pub match_: ToolMatch,
    /// Pipe (default) or Pty.
    #[serde(default)]
    pub capture: CaptureMode,
    /// Strip ANSI escapes at the capture boundary. Defaults to true when capture = pty
    /// (a pty makes children emit color/cursor/spinner sequences), false otherwise.
    #[serde(default)]
    pub strip_ansi: Option<bool>,
    /// Environment variables to set on the child before spawning. The preferred fix for
    /// builders that hang on a pipe but honor a non-interactive signal — e.g.
    /// `env = { CI = "1" }` makes `ng build`/vite run one-shot and exit, no PTY needed.
    /// Applied for any capture mode.
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
}

impl ToolRule {
    /// Effective strip_ansi: explicit value, else default (true iff capturing via pty).
    // Consumed by the pty capture path; without that feature it is still part of the
    // public config surface (rules parse regardless of which capture backends are built).
    #[allow(dead_code)]
    pub fn strip_ansi_effective(&self) -> bool {
        self.strip_ansi.unwrap_or(self.capture == CaptureMode::Pty)
    }
}

/// Predicate matched against the resolved command invocation.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolMatch {
    /// Required: the command basename, e.g. "ng" or "npm".
    pub command: String,
    /// Optional: first non-flag argument, e.g. "build" (for `ng build`) or "run"
    /// (for `npm run …`).
    #[serde(default)]
    pub subcommand: Option<String>,
    /// Optional: every listed token must appear somewhere in the args. Use this to
    /// target a specific npm script — `command="npm", subcommand="run",
    /// args_contains=["build"]` matches `npm run build` but not `npm run test`.
    #[serde(default)]
    pub args_contains: Vec<String>,
}

impl ToolMatch {
    /// True if this predicate matches the given command + argument list.
    #[allow(dead_code)]
    pub fn matches(&self, command: &str, args: &[String]) -> bool {
        if command != self.command {
            return false;
        }
        if let Some(sub) = &self.subcommand {
            let first_positional = args
                .iter()
                .find(|a| !a.starts_with('-'))
                .map(|a| a.as_str());
            if first_positional != Some(sub.as_str()) {
                return false;
            }
        }
        self.args_contains
            .iter()
            .all(|needle| args.iter().any(|a| a == needle))
    }
}

impl Config {
    /// First `[[tools]]` rule whose `match` applies to this invocation, if any.
    #[allow(dead_code)]
    pub fn tool_rule_for(&self, command: &str, args: &[String]) -> Option<&ToolRule> {
        self.tools.iter().find(|r| r.match_.matches(command, args))
    }
}

/// How much the agent is told about RTK by the instructions file `rtk init` writes.
///
/// - `default`: output contract only. The agent never learns rtk exists.
/// - `high`: adds what RTK is and its meta commands (`rtk gain`, `rtk proxy`, `RTK_DISABLED=1`).
/// - `full`: adds "prefix every command with `rtk`". Required for agents without a command
///   hook; those agents always receive `full` regardless of this setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AwarenessLevel {
    #[default]
    Default,
    High,
    Full,
}

impl AwarenessLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            AwarenessLevel::Default => "default",
            AwarenessLevel::High => "high",
            AwarenessLevel::Full => "full",
        }
    }
}

impl std::fmt::Display for AwarenessLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct AwarenessConfig {
    #[serde(default)]
    pub level: AwarenessLevel,
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

struct LegacyMapping {
    mode: Option<crate::core::retriever::RecoveryMode>,
    tee_on_success: Option<bool>,
    tee_max_files: Option<usize>,
    tee_max_file_size: Option<usize>,
    tee_directory: Option<PathBuf>,
}

/// The single source of truth for mapping a legacy `[tee]` section onto
/// `[retriever]`, shared by `Config::load()` and `rtk config recall` so the
/// two paths can never disagree on the same input.
fn map_legacy_tee(
    tee: &LegacyTeeConfig,
    has_retriever: bool,
    explicit_retriever_keys: &[String],
) -> LegacyMapping {
    use crate::core::retriever::RecoveryMode;
    let explicit = |key: &str| explicit_retriever_keys.iter().any(|k| k == key);
    let mode = if has_retriever {
        None
    } else if tee.enabled == Some(false) || tee.mode.as_deref() == Some("never") {
        Some(RecoveryMode::Disabled)
    } else {
        Some(RecoveryMode::Tee)
    };
    LegacyMapping {
        mode,
        tee_on_success: (tee.mode.as_deref() == Some("always") && !explicit("tee_on_success"))
            .then_some(true),
        tee_max_files: tee.max_files.filter(|_| !explicit("tee_max_files")),
        tee_max_file_size: tee.max_file_size.filter(|_| !explicit("tee_max_file_size")),
        tee_directory: tee.directory.clone().filter(|_| !explicit("tee_directory")),
    }
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
        let explicit: Vec<String> = value
            .get("retriever")
            .and_then(|r| r.as_table())
            .map(|t| t.keys().cloned().collect())
            .unwrap_or_default();
        let has_retriever = value.get("retriever").is_some();
        let mut config = Config::deserialize(value)?;
        config.migrate_legacy_tee(has_retriever, &explicit);
        Ok(config)
    }

    fn migrate_legacy_tee(&mut self, has_retriever: bool, explicit_retriever_keys: &[String]) {
        let Some(tee) = self.tee.take() else {
            return;
        };
        use crate::core::retriever::RecoveryMode;
        let mapping = map_legacy_tee(&tee, has_retriever, explicit_retriever_keys);
        let r = &mut self.retriever;
        if let Some(mode) = mapping.mode {
            r.mode = mode;
            if mode == RecoveryMode::Tee {
                self.migrated_from_legacy_tee = true;
            }
        }
        if let Some(v) = mapping.tee_on_success {
            r.tee_on_success = v;
        }
        let merged = mapping.tee_max_files.is_some()
            || mapping.tee_max_file_size.is_some()
            || mapping.tee_directory.is_some();
        if let Some(v) = mapping.tee_max_files {
            r.tee_max_files = v;
        }
        if let Some(v) = mapping.tee_max_file_size {
            r.tee_max_file_size = v;
        }
        if let Some(d) = mapping.tee_directory {
            r.tee_directory = Some(d);
        }
        if has_retriever && merged {
            self.legacy_tee_fields_merged = true;
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
        let tee = LegacyTeeConfig {
            enabled: legacy
                .get("enabled")
                .and_then(|i| i.as_value())
                .and_then(|v| v.as_bool()),
            mode: legacy
                .get("mode")
                .and_then(|i| i.as_value())
                .and_then(|v| v.as_str())
                .map(str::to_string),
            max_files: legacy
                .get("max_files")
                .and_then(|i| i.as_value())
                .and_then(|v| v.as_integer())
                .map(|n| n as usize),
            max_file_size: legacy
                .get("max_file_size")
                .and_then(|i| i.as_value())
                .and_then(|v| v.as_integer())
                .map(|n| n as usize),
            directory: legacy
                .get("directory")
                .and_then(|i| i.as_value())
                .and_then(|v| v.as_str())
                .map(PathBuf::from),
        };
        let explicit_keys: Vec<String> = doc["retriever"]
            .as_table_like()
            .map(|t| t.iter().map(|(k, _)| k.to_string()).collect())
            .unwrap_or_default();
        let mapping = map_legacy_tee(&tee, true, &explicit_keys);
        if let Some(v) = mapping.tee_max_files {
            doc["retriever"]["tee_max_files"] = toml_edit::value(v as i64);
        }
        if let Some(v) = mapping.tee_max_file_size {
            doc["retriever"]["tee_max_file_size"] = toml_edit::value(v as i64);
        }
        if let Some(d) = mapping.tee_directory {
            doc["retriever"]["tee_directory"] = toml_edit::value(d.to_string_lossy().as_ref());
        }
        if mapping.tee_on_success == Some(true) && mode == RecoveryMode::Tee {
            doc["retriever"]["tee_on_success"] = toml_edit::value(true);
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
    fn test_awareness_level_deserialize_each_variant() {
        for (raw, expected) in [
            ("default", AwarenessLevel::Default),
            ("high", AwarenessLevel::High),
            ("full", AwarenessLevel::Full),
        ] {
            let toml = format!("[awareness]\nlevel = \"{raw}\"\n");
            let config: Config = toml::from_str(&toml).expect("valid toml");
            assert_eq!(config.awareness.level, expected, "level = {raw}");
        }
    }

    #[test]
    fn test_awareness_level_defaults_when_missing() {
        let config: Config = toml::from_str("").expect("valid toml");
        assert_eq!(config.awareness.level, AwarenessLevel::Default);

        let config: Config = toml::from_str("[awareness]\n").expect("valid toml");
        assert_eq!(config.awareness.level, AwarenessLevel::Default);
    }

    #[test]
    fn test_awareness_level_rejects_unknown_value() {
        let result: Result<Config, _> = toml::from_str("[awareness]\nlevel = \"max\"\n");
        assert!(
            result.is_err(),
            "unknown awareness level must be a parse error"
        );
    }

    #[test]
    fn test_awareness_level_round_trips_through_default_config() {
        let serialized = toml::to_string_pretty(&Config::default()).expect("serializable");
        assert!(
            serialized.contains("[awareness]") && serialized.contains("level = \"default\""),
            "rtk config must show the awareness level, got:\n{serialized}"
        );
        let parsed: Config = toml::from_str(&serialized).expect("round trip");
        assert_eq!(parsed.awareness.level, AwarenessLevel::Default);
    }

    #[test]
    fn test_awareness_level_display_matches_toml_value() {
        assert_eq!(AwarenessLevel::Default.to_string(), "default");
        assert_eq!(AwarenessLevel::High.to_string(), "high");
        assert_eq!(AwarenessLevel::Full.to_string(), "full");
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
    fn test_tools_empty_by_default() {
        let config = Config::default();
        assert!(config.tools.is_empty());
        assert!(config.tool_rule_for("ng", &["build".into()]).is_none());
    }

    #[test]
    fn test_tools_rule_parses_and_matches() {
        let toml = r#"
[[tools]]
match = { command = "ng", subcommand = "build" }
capture = "pty"
strip_ansi = true
"#;
        let config: Config = toml::from_str(toml).expect("valid toml");
        let rule = config
            .tool_rule_for("ng", &["build".into(), "--prod".into()])
            .expect("rule matches ng build");
        assert_eq!(rule.capture, CaptureMode::Pty);
        assert!(rule.strip_ansi_effective());
        // subcommand mismatch → no match
        assert!(config.tool_rule_for("ng", &["serve".into()]).is_none());
        // command mismatch → no match
        assert!(config.tool_rule_for("vite", &["build".into()]).is_none());
    }

    #[test]
    fn test_tools_match_without_subcommand_matches_any_args() {
        let toml = r#"
[[tools]]
match = { command = "vite" }
capture = "pty"
"#;
        let config: Config = toml::from_str(toml).expect("valid toml");
        assert!(config.tool_rule_for("vite", &[]).is_some());
        assert!(config.tool_rule_for("vite", &["build".into()]).is_some());
    }

    #[test]
    fn test_tools_strip_ansi_defaults_to_pty() {
        // capture = pty, strip_ansi unset → effective true
        let pty: ToolRule = toml::from_str(
            r#"
match = { command = "ng" }
capture = "pty"
"#,
        )
        .unwrap();
        assert!(pty.strip_ansi_effective());
        // capture = pipe (default), strip_ansi unset → effective false
        let pipe: ToolRule = toml::from_str(r#"match = { command = "git" }"#).unwrap();
        assert_eq!(pipe.capture, CaptureMode::Pipe);
        assert!(!pipe.strip_ansi_effective());
        // explicit override wins
        let forced: ToolRule = toml::from_str(
            r#"
match = { command = "ng" }
capture = "pty"
strip_ansi = false
"#,
        )
        .unwrap();
        assert!(!forced.strip_ansi_effective());
    }

    #[test]
    fn test_args_contains_targets_specific_npm_script() {
        let toml = r#"
[[tools]]
match = { command = "npm", subcommand = "run", args_contains = ["build"] }
capture = "pty"
"#;
        let config: Config = toml::from_str(toml).expect("valid toml");
        // npm run build → match
        assert!(config
            .tool_rule_for("npm", &["run".into(), "build".into()])
            .is_some());
        // npm run test → no match (different script)
        assert!(config
            .tool_rule_for("npm", &["run".into(), "test".into()])
            .is_none());
        // npm install → no match (subcommand differs)
        assert!(config.tool_rule_for("npm", &["install".into()]).is_none());
    }

    #[test]
    fn test_first_matching_rule_wins() {
        let toml = r#"
[[tools]]
match = { command = "ng", subcommand = "build" }
capture = "pty"

[[tools]]
match = { command = "ng" }
capture = "pipe"
"#;
        let config: Config = toml::from_str(toml).expect("valid toml");
        // "ng build" matches the first (pty) rule, not the broader second.
        assert_eq!(
            config
                .tool_rule_for("ng", &["build".into()])
                .unwrap()
                .capture,
            CaptureMode::Pty
        );
        // "ng serve" falls through to the second (pipe) rule.
        assert_eq!(
            config
                .tool_rule_for("ng", &["serve".into()])
                .unwrap()
                .capture,
            CaptureMode::Pipe
        );
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
    fn test_coexisting_tee_fields_merge_as_fallback() {
        use crate::core::retriever::RecoveryMode;
        let toml = "[retriever]\nretention_days = 90\nmode = \"tee\"\n\n[tee]\nmax_files = 100\ndirectory = \"/mnt/big-disk/tee\"\n";
        let config = Config::from_toml(toml).expect("valid");
        assert_eq!(config.retriever.mode, RecoveryMode::Tee);
        assert_eq!(config.retriever.retention_days, 90);
        assert_eq!(config.retriever.tee_max_files, 100);
        assert_eq!(
            config.retriever.tee_directory,
            Some(PathBuf::from("/mnt/big-disk/tee"))
        );
        assert!(!config.migrated_from_legacy_tee);
    }

    #[test]
    fn test_explicit_retriever_field_beats_legacy_tee_field() {
        let toml = "[retriever]\ntee_max_files = 5\n\n[tee]\nmax_files = 100\n";
        let config = Config::from_toml(toml).expect("valid");
        assert_eq!(config.retriever.tee_max_files, 5);
    }

    #[test]
    fn test_legacy_always_is_preserved_not_downgraded() {
        use crate::core::retriever::RecoveryMode;
        let config = Config::from_toml("[tee]\nmode = \"always\"\n").expect("valid");
        assert_eq!(config.retriever.mode, RecoveryMode::Tee);
        assert!(
            config.retriever.tee_on_success,
            "legacy always must keep archiving successful runs"
        );
        let failures = Config::from_toml("[tee]\nmode = \"failures\"\n").expect("valid");
        assert!(!failures.retriever.tee_on_success);
        let rewritten = apply_recall_mode("[tee]\nmode = \"always\"\n", RecoveryMode::Tee)
            .expect("valid rewrite");
        assert!(
            Config::from_toml(&rewritten)
                .unwrap()
                .retriever
                .tee_on_success,
            "rtk config recall must carry the always intent over too"
        );
    }

    #[test]
    fn test_notice_flags_track_migration_outcome() {
        let disabled = Config::from_toml("[tee]\nenabled = false\n").expect("valid");
        assert!(
            !disabled.migrated_from_legacy_tee,
            "a user who disabled tee must not get the file-mode-kept notice"
        );
        let kept = Config::from_toml("[tee]\nenabled = true\n").expect("valid");
        assert!(kept.migrated_from_legacy_tee);
        let merged =
            Config::from_toml("[retriever]\nretention_days = 90\n\n[tee]\nmax_files = 100\n")
                .expect("valid");
        assert!(
            merged.legacy_tee_fields_merged,
            "coexisting legacy values still in effect must be disclosed"
        );
        assert!(!merged.migrated_from_legacy_tee);
        let clean = Config::from_toml("[retriever]\nmode = \"sqlite\"\n").expect("valid");
        assert!(!clean.legacy_tee_fields_merged);
    }

    #[test]
    fn test_migration_paths_agree_on_every_legacy_shape() {
        let modes = [
            "",
            "mode = \"failures\"\n",
            "mode = \"always\"\n",
            "mode = \"never\"\n",
        ];
        let enableds = ["", "enabled = true\n", "enabled = false\n"];
        let retrievers = [
            "",
            "[retriever]\nretention_days = 90\n\n",
            "[retriever]\ntee_max_files = 5\n\n",
        ];
        for m in modes {
            for e in enableds {
                for r in retrievers {
                    let orig = format!("{r}[tee]\n{m}{e}max_files = 100\ndirectory = \"/big\"\n");
                    let loaded = Config::from_toml(&orig).expect("load").retriever;
                    let rewritten = apply_recall_mode(&orig, loaded.mode).expect("apply");
                    let reloaded = Config::from_toml(&rewritten).expect("reload").retriever;
                    assert_eq!(
                        (
                            loaded.mode,
                            loaded.tee_max_files,
                            loaded.tee_max_file_size,
                            loaded.tee_directory.clone()
                        ),
                        (
                            reloaded.mode,
                            reloaded.tee_max_files,
                            reloaded.tee_max_file_size,
                            reloaded.tee_directory.clone()
                        ),
                        "Config::load and rtk config recall must agree on: {orig}"
                    );
                }
            }
        }
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
