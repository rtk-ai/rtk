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
    /// Per-tool behavior rules, evaluated top-to-bottom (first match wins per field).
    /// See docs/pr_briefs/005-per-tool-config-design. Empty by default → no behavior change.
    #[serde(default)]
    pub tools: Vec<ToolRule>,
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
    #[cfg_attr(not(feature = "pty"), allow(dead_code))]
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
    #[cfg_attr(not(feature = "pty"), allow(dead_code))]
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
    #[cfg_attr(not(feature = "pty"), allow(dead_code))]
    pub fn tool_rule_for(&self, command: &str, args: &[String]) -> Option<&ToolRule> {
        self.tools.iter().find(|r| r.match_.matches(command, args))
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
}
