pub const RTK_DATA_DIR: &str = "rtk";
pub const HISTORY_DB: &str = "history.db";
pub const CONFIG_TOML: &str = "config.toml";
pub const FILTERS_TOML: &str = "filters.toml";
pub const TRUSTED_FILTERS_JSON: &str = "trusted_filters.json";
pub const DEFAULT_HISTORY_DAYS: i64 = 90;
/// Default char ceiling for tracked token estimates — a conservative default
/// for coding-agent shells that truncate captured output (~30,000 chars is a
/// common ceiling), so `rtk gain` doesn't count savings for output no model
/// ever actually saw.
pub const DEFAULT_ESTIMATE_CAP_CHARS: usize = 30_000;

/// RTK-only subcommands that should never fall back to raw execution.
/// When adding a new RTK-only subcommand to `Commands`, add its clap name here.
pub const RTK_META_COMMANDS: &[&str] = &[
    "gain",
    "discover",
    "learn",
    "init",
    "config",
    "proxy",
    "run",
    "hook",
    "hook-audit",
    "pipe",
    "cc-economics",
    "verify",
    "trust",
    "untrust",
    "session",
    "rewrite",
    "telemetry",
    "smart",
    "deps",
    "json",
];
