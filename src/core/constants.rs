use std::path::PathBuf;

pub const RTK_DATA_DIR: &str = "rtk";
pub const HISTORY_DB: &str = "history.db";
pub const CONFIG_TOML: &str = "config.toml";
pub const FILTERS_TOML: &str = "filters.toml";
pub const TRUSTED_FILTERS_JSON: &str = "trusted_filters.json";
pub const DEFAULT_HISTORY_DAYS: i64 = 90;

/// Everything rtk keeps on disk: `~/.local/share/rtk` and its platform
/// equivalents. The tracking database, tee spool, telemetry salt, trust store
/// and hook markers all hang off this.
///
/// `None` when the platform gives no answer, which `dirs` reports for a
/// container UID with no passwd entry and no `HOME`. Callers that accept a
/// path from the environment or the config file check that first — see
/// `tracking::resolve_db_path`.
///
/// In a test build it is a scratch directory, so a `cargo test` run leaves the
/// developer's own untouched.
pub fn data_dir() -> Option<PathBuf> {
    #[cfg(test)]
    {
        Some(crate::core::test_isolation::scratch_dir().join(RTK_DATA_DIR))
    }
    #[cfg(not(test))]
    {
        dirs::data_local_dir().map(|d| d.join(RTK_DATA_DIR))
    }
}

/// [`data_dir`], falling back to rtk's own directory under `root` where the
/// platform gives no answer. For callers that carry on without one rather
/// than failing.
pub fn data_dir_under(root: &str) -> PathBuf {
    data_dir().unwrap_or_else(|| PathBuf::from(root).join(RTK_DATA_DIR))
}

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
