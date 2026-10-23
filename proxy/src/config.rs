//! Proxy configuration.
//!
//! Resolved with precedence **CLI flag > environment variable > config file >
//! built-in default**. Everything works with zero config; a `rtk-proxy.toml`
//! file is an optional convenience for anyone who prefers one versioned file
//! over a handful of env vars. The file is discovered via `RTK_PROXY_CONFIG`,
//! else `./rtk-proxy.toml`, else `/etc/rtk-proxy/rtk-proxy.toml`.
//!
//! The generic [`load_toml_file`] discovery/parse helper is deliberately
//! public: a downstream binary with a larger config *shape* can still locate
//! its file the same way.

use clap::Args;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

const DEFAULT_BIND: &str = "127.0.0.1:8080";
const DEFAULT_ANTHROPIC_UPSTREAM: &str = "https://api.anthropic.com";
const DEFAULT_OPENAI_UPSTREAM: &str = "https://api.openai.com";
/// Forward path appended to the resolved `openai_upstream` for
/// `/v1/chat/completions` requests when `openai_path` is not set.
const DEFAULT_OPENAI_PATH: &str = "/v1/chat/completions";
const DB_FILE_NAME: &str = "rtk-proxy.db";
const ICM_DB_FILE_NAME: &str = "rtk-proxy-memory.db";
const DEFAULT_UPSTREAM_RETRIES: u32 = 2;

/// Where the daemon keeps its data unless told otherwise: the same directory
/// the `rtk` CLI reads for `gain`. A bare filename would resolve against the
/// launch directory — a manual start would scatter databases into whatever
/// shell cwd it ran from, a launchd service (cwd `/`) could not write at all,
/// and `rtk gain`, which reads one fixed path, would see none of it.
fn default_data_dir() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|dir| dir.join("rtk").join("llm-proxy"))
}

fn default_path(file_name: &str) -> String {
    default_data_dir()
        .map(|dir| dir.join(file_name).to_string_lossy().into_owned())
        .unwrap_or_else(|| file_name.to_string())
}

/// The default usage-store path, for callers that need to recognise it.
pub fn default_db_path() -> String {
    default_path(DB_FILE_NAME)
}

/// The default ICM-store path, for callers that need to recognise it.
pub fn default_icm_db_path() -> String {
    default_path(ICM_DB_FILE_NAME)
}

/// Move a database found in the launch directory to where the CLI looks, so its history shows up in `rtk gain` instead of being
/// orphaned. Returns the path it came from.
///
/// Refuses to act unless the destination is absent — an existing canonical
/// file is real data, never overwritten. The write-ahead log travels with the
/// database: a daemon killed uncleanly leaves committed rows in `-wal`, and
/// moving the main file alone would drop them.
pub fn adopt_stray_db(canonical: &str) -> Option<std::path::PathBuf> {
    let dest = std::path::Path::new(canonical);
    adopt_from(&std::path::PathBuf::from(dest.file_name()?), dest)
}

fn adopt_from(stray: &std::path::Path, dest: &std::path::Path) -> Option<std::path::PathBuf> {
    if stray == dest || !stray.is_file() || dest.exists() {
        return None;
    }
    std::fs::create_dir_all(dest.parent()?).ok()?;
    move_file(stray, dest)?;
    for suffix in ["-wal", "-shm"] {
        let from = with_suffix(stray, suffix);
        if from.is_file() {
            move_file(&from, &with_suffix(dest, suffix));
        }
    }
    Some(stray.to_path_buf())
}

fn with_suffix(path: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    std::path::PathBuf::from(name)
}

fn move_file(from: &std::path::Path, to: &std::path::Path) -> Option<()> {
    if std::fs::rename(from, to).is_ok() {
        return Some(());
    }
    std::fs::copy(from, to).ok()?;
    let _ = std::fs::remove_file(from);
    Some(())
}
/// Total timeout for a BUFFERED upstream call. Ten minutes is far above any
/// real non-streaming completion and still finite: without a bound, a wedged
/// socket holds the agent's turn open with no exit but Ctrl-C.
/// Streamed responses are never subject to it (see `forward`); they are
/// bounded by the client's read timeout instead.
const DEFAULT_UPSTREAM_TIMEOUT_SECS: u64 = 600;
/// CCR cache entries older than this are pruned (boot + daily). A retrieve
/// a week after the conversation died serves nobody, and the cache stores
/// raw tool output verbatim -- expiry bounds how long that sits on disk.
const DEFAULT_ICM_TTL_SECS: u64 = 7 * 24 * 60 * 60;

/// Default config-file locations tried when `RTK_PROXY_CONFIG` is unset.
const CONFIG_PATHS: &[&str] = &["rtk-proxy.toml", "/etc/rtk-proxy/rtk-proxy.toml"];

/// The resolved runtime configuration (all layers merged).
#[derive(Debug, Clone)]
pub struct Config {
    /// Address the proxy HTTP server binds to.
    pub bind: String,
    /// Upstream base URL for the Anthropic Messages API (`/v1/messages`).
    pub anthropic_upstream: String,
    /// Upstream base URL for the OpenAI Chat Completions API.
    pub openai_upstream: String,
    /// Forward path for OpenAI Chat Completions requests, appended to
    /// `openai_upstream`. Override for providers whose endpoint doesn't live
    /// under `/v1` (e.g. Z.AI's `/chat/completions`). Boot-only: restarting
    /// re-applies it, same as the upstream base.
    pub openai_path: String,
    /// Boot default for CCR (Compress-Cache-Retrieve). On — it is the only
    /// compression this proxy performs. A persisted runtime value wins over
    /// this (see `settings::RuntimeSettings`).
    pub ccr: bool,
    /// Cache-breakpoint alignment (the `cache-align` method). Boot-only on
    /// purpose: flipping it mid-session would rewrite the cached prefix.
    pub cache_align: bool,
    /// Category-A normalisation (the `ansi` / `progress` / `dedup` methods).
    /// Boot-only for the same reason as `cache_align`: the stage is a pure
    /// function of block content, so toggling it mid-session would rewrite
    /// every cached block at once.
    pub normalize: bool,
    pub engine_methods: bool,
    /// Token-compression level for the ingest filter:
    /// `off` | `light` | `balanced` | `aggressive`. Off by default: the
    /// filter truncates, which is lossy, so it is opt-in.
    pub compression: String,
    /// Tuning for pixel-compress. EXPERIMENTAL and off unless its own
    /// `enabled` says otherwise.
    pub pixel_compress: PixelCompressConfig,
    /// Lend the agent's relayed credential to a local worker that arrives
    /// without one, so delegating to the same provider needs no second copy
    /// of a key the agent already sends. Loopback-only; see `relayed_auth`.
    pub relay_borrowing: bool,
    /// SQLite path for the usage store; empty/`off` disables recording.
    pub db: String,
    /// SQLite path for the ICM store backing CCR's cache. Independent of
    /// `db` — a separate file.
    pub icm_db: String,
    /// Seconds a CCR cache entry lives before pruning; 0 keeps entries forever.
    pub icm_ttl_secs: u64,
    /// Total request timeout (seconds) applied to **buffered** (non-streaming)
    /// upstream calls; 0 disables. Never applied to a streamed SSE response
    /// (which is long-lived and must not be cut).
    pub upstream_timeout_secs: u64,
    /// Retries on a transient upstream failure (429/5xx or a connect/timeout
    /// error), with exponential backoff, before the response starts streaming.
    /// 0 disables.
    pub upstream_retries: u32,
}

/// CLI flags + environment (the highest-precedence layer) for `rtk-proxy start`.
/// Every field is optional so an unset flag/env falls through to the file, then
/// the default.
#[derive(Debug, Args)]
pub struct StartArgs {
    /// Address the proxy HTTP server binds to. [default: 127.0.0.1:8080]
    #[arg(long, env = "RTK_PROXY_BIND")]
    bind: Option<String>,

    /// Upstream base URL for the Anthropic Messages API. [default: https://api.anthropic.com]
    #[arg(long, env = "RTK_PROXY_ANTHROPIC_UPSTREAM")]
    anthropic_upstream: Option<String>,

    /// Upstream base URL for the OpenAI Chat Completions API. [default: https://api.openai.com]
    #[arg(long, env = "RTK_PROXY_OPENAI_UPSTREAM")]
    openai_upstream: Option<String>,

    /// Forward path for OpenAI Chat Completions requests, appended to
    /// `openai_upstream`. [default: /v1/chat/completions]
    #[arg(long, env = "RTK_PROXY_OPENAI_PATH")]
    openai_path: Option<String>,

    /// Compress context with CCR before forwarding. [default: true]
    #[arg(long, env = "RTK_PROXY_CCR")]
    ccr: Option<bool>,

    /// Inject a cache breakpoint when the client set none. [default: true]
    #[arg(long, env = "RTK_PROXY_CACHE_ALIGN")]
    cache_align: Option<bool>,

    /// Strip bytes no reader sees: ANSI escapes, trailing whitespace, CR
    /// progress redraws, runs of identical adjacent lines. [default: true]
    #[arg(long, env = "RTK_PROXY_NORMALIZE")]
    normalize: Option<bool>,

    /// Ingest-filter level: off | light | balanced | aggressive. [default: off]
    #[arg(long, env = "RTK_PROXY_COMPRESSION")]
    compression: Option<String>,

    /// Lend the relayed credential to a local worker that sends none. [default: true]
    #[arg(long, env = "RTK_PROXY_RELAY_BORROWING")]
    relay_borrowing: Option<bool>,

    /// SQLite path for the usage store; empty/`off` disables recording. [default: rtk-proxy.db]
    #[arg(long, env = "RTK_PROXY_DB")]
    db: Option<String>,

    /// SQLite path for the ICM store backing CCR's cache. [default: rtk-proxy-memory.db]
    #[arg(long, env = "RTK_PROXY_ICM_DB")]
    icm_db: Option<String>,

    /// Seconds a CCR cache entry lives; 0 = forever. [default: 604800]
    #[arg(long, env = "RTK_PROXY_ICM_TTL_SECS")]
    icm_ttl_secs: Option<u64>,

    /// Total timeout (s) for buffered upstream calls; 0 disables. [default: 600]
    #[arg(long, env = "RTK_PROXY_UPSTREAM_TIMEOUT")]
    upstream_timeout_secs: Option<u64>,

    /// Retries on a transient upstream failure (429/5xx/connect). [default: 2]
    #[arg(long, env = "RTK_PROXY_UPSTREAM_RETRIES")]
    upstream_retries: Option<u32>,
}

/// The optional TOML config file. Keys mirror the env vars 1:1 (lowercased,
/// without the `RTK_PROXY_` prefix). Any omitted key falls through to the default.
/// `Serialize` backs `rtk-proxy service install`, which writes one out fully
/// resolved (see [`Config::to_file_config`]) so the installed service doesn't
/// depend on the shell environment it happened to be installed from.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct FileConfig {
    bind: Option<String>,
    anthropic_upstream: Option<String>,
    openai_upstream: Option<String>,
    openai_path: Option<String>,
    ccr: Option<bool>,
    cache_align: Option<bool>,
    normalize: Option<bool>,
    compression: Option<String>,
    #[serde(default)]
    pixel_compress: PixelCompressConfig,
    relay_borrowing: Option<bool>,
    db: Option<String>,
    icm_db: Option<String>,
    icm_ttl_secs: Option<u64>,
    upstream_timeout_secs: Option<u64>,
    upstream_retries: Option<u32>,
}

impl FileConfig {
    /// The `bind` a saved file specifies, if any — `rtk-proxy service status`
    /// reads this from the installed config to know where to reach the daemon
    /// without the caller having to repeat `--bind`.
    pub(crate) fn bind(&self) -> Option<&str> {
        self.bind.as_deref()
    }
}

/// Tuning for `pipeline::pixel_compress` (render eligible `tool_result` text as
/// a PNG instead of truncating it). Defaults are the conservative
/// configuration -- Anthropic only, code-shaped content only, a generous
/// size floor.
/// A `[pixel_compress]` table in the TOML file overrides individual fields;
/// anything omitted keeps its default.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PixelCompressConfig {
    /// Boot default for the runtime-togglable enable flag. Off.
    pub enabled: bool,
    /// Minimum estimated tokens a `tool_result` block must have before
    /// pixel-compress considers it (below this, a PNG's fixed overhead isn't
    /// worth it and truncation-free plain text is cheaper anyway).
    pub min_tokens: usize,
    /// Integer multiple of the 8x8 base glyph. Claude bills images by
    /// `ceil(w/28) * ceil(h/28)` tiles, which scales with the SQUARE of
    /// `font_scale` for a fixed amount of text -- at scale 2 (~16px glyphs)
    /// the image consistently costs MORE tokens than the text it replaced,
    /// regardless of content shape (`build_replacement` refuses any render
    /// that isn't smaller than the original and would just no-op every
    /// request at that scale). 1 (unscaled 8px) is the smallest size that
    /// still renders every glyph on a full pixel grid -- the only scale where
    /// this feature can pay for itself under Claude's own pricing model.
    pub font_scale: u32,
    /// Wrap width, in characters, before starting a new rendered line.
    pub max_line_chars: usize,
    /// Which upstream dialects (`"anthropic"` / `"openai"`) this is allowed
    /// to activate for. Empty by default reasoning would be "everywhere" --
    /// deliberately explicit-opt-in per provider instead, since the two
    /// providers do not behave the same at any given render density.
    pub providers: Vec<String>,
    /// Skip content that looks like JSON (its render costs more than the text).
    pub skip_json: bool,
    /// Skip content that looks like a log/timestamped stream (same reason).
    pub skip_logs: bool,
}

impl Default for PixelCompressConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_tokens: 1500,
            font_scale: 1,
            max_line_chars: 100,
            providers: vec!["anthropic".to_string()],
            skip_json: true,
            skip_logs: true,
        }
    }
}

impl Config {
    /// Resolve from already-parsed `start` args + env + config file + defaults.
    pub fn from_cli(cli: StartArgs) -> anyhow::Result<Self> {
        let file: FileConfig =
            load_toml_file("RTK_PROXY_CONFIG", CONFIG_PATHS)?.unwrap_or_default();
        Ok(Self::resolve(cli, file))
    }

    /// Merge the three layers (CLI wins over file wins over default). Pure — the
    /// testable core of [`Config::from_cli`].
    fn resolve(cli: StartArgs, file: FileConfig) -> Self {
        Config {
            bind: cli
                .bind
                .or(file.bind)
                .unwrap_or_else(|| DEFAULT_BIND.into()),
            anthropic_upstream: cli
                .anthropic_upstream
                .or(file.anthropic_upstream)
                .unwrap_or_else(|| DEFAULT_ANTHROPIC_UPSTREAM.into()),
            openai_upstream: cli
                .openai_upstream
                .or(file.openai_upstream)
                .unwrap_or_else(|| DEFAULT_OPENAI_UPSTREAM.into()),
            openai_path: {
                // A missing leading slash would silently join into an invalid
                // host; normalize here instead of failing every request at
                // runtime with an opaque DNS error.
                let mut p = cli
                    .openai_path
                    .or(file.openai_path)
                    .unwrap_or_else(|| DEFAULT_OPENAI_PATH.into());
                if !p.starts_with('/') {
                    p.insert(0, '/');
                }
                p
            },
            ccr: cli.ccr.or(file.ccr).unwrap_or(true),
            cache_align: cli.cache_align.or(file.cache_align).unwrap_or(true),
            normalize: cli.normalize.or(file.normalize).unwrap_or(true),
            engine_methods: false,
            compression: cli
                .compression
                .or(file.compression)
                .unwrap_or_else(|| "off".to_string()),
            pixel_compress: file.pixel_compress,
            relay_borrowing: cli.relay_borrowing.or(file.relay_borrowing).unwrap_or(true),
            db: cli.db.or(file.db).unwrap_or_else(default_db_path),
            icm_db: cli
                .icm_db
                .or(file.icm_db)
                .unwrap_or_else(default_icm_db_path),
            icm_ttl_secs: cli
                .icm_ttl_secs
                .or(file.icm_ttl_secs)
                .unwrap_or(DEFAULT_ICM_TTL_SECS),
            upstream_timeout_secs: cli
                .upstream_timeout_secs
                .or(file.upstream_timeout_secs)
                .unwrap_or(DEFAULT_UPSTREAM_TIMEOUT_SECS),
            upstream_retries: cli
                .upstream_retries
                .or(file.upstream_retries)
                .unwrap_or(DEFAULT_UPSTREAM_RETRIES),
        }
    }

    /// Every field fully resolved, as a `FileConfig` ready to serialize. Used by
    /// `rtk-proxy service install` to freeze the CLI+env+file config it was given
    /// at install time into one self-contained file the installed service reads
    /// via `RTK_PROXY_CONFIG` — so it behaves the same on every login, regardless
    /// of what's in the shell environment at boot.
    pub(crate) fn to_file_config(&self) -> FileConfig {
        FileConfig {
            bind: Some(self.bind.clone()),
            anthropic_upstream: Some(self.anthropic_upstream.clone()),
            openai_upstream: Some(self.openai_upstream.clone()),
            openai_path: Some(self.openai_path.clone()),
            ccr: Some(self.ccr),
            cache_align: Some(self.cache_align),
            normalize: Some(self.normalize),
            compression: Some(self.compression.clone()),
            pixel_compress: self.pixel_compress.clone(),
            relay_borrowing: Some(self.relay_borrowing),
            db: Some(self.db.clone()),
            icm_db: Some(self.icm_db.clone()),
            icm_ttl_secs: Some(self.icm_ttl_secs),
            upstream_timeout_secs: Some(self.upstream_timeout_secs),
            upstream_retries: Some(self.upstream_retries),
        }
    }

    /// Whether `bind` points at the loopback interface. A proxy that forwards
    /// provider credentials has no authentication of its own, so binding it
    /// anywhere else exposes those credentials to the network — `start` warns
    /// and `service install` refuses.
    pub fn binds_loopback(&self) -> bool {
        bind_is_loopback(&self.bind)
    }
}

/// Whether `addr` is a loopback bind. Pure, so the policy is testable without
/// constructing a whole `Config`.
pub fn bind_is_loopback(addr: &str) -> bool {
    let host = match addr.rsplit_once(':') {
        // `[::1]:8080` — strip the brackets IPv6 literals carry in an authority.
        Some((h, _)) => h.trim_start_matches('[').trim_end_matches(']'),
        None => addr,
    };
    match host.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.is_loopback(),
        // Not an IP literal: only the canonical loopback name qualifies.
        Err(_) => host.eq_ignore_ascii_case("localhost"),
    }
}

/// Load a TOML config file into `T`, to layer under CLI/env in the caller.
///
/// An explicit path in `env_var` wins; otherwise `default_paths` are tried in
/// order. Returns `Ok(None)` when no file is present (an absent file is fine),
/// `Err` on a parse error (a malformed file should fail loudly, not be
/// silently ignored).
pub fn load_toml_file<T: DeserializeOwned>(
    env_var: &str,
    default_paths: &[&str],
) -> anyhow::Result<Option<T>> {
    let candidates: Vec<String> = match std::env::var(env_var).ok().filter(|s| !s.is_empty()) {
        Some(p) => vec![p],
        None => default_paths.iter().map(|s| s.to_string()).collect(),
    };
    let explicit = std::env::var(env_var)
        .ok()
        .filter(|s| !s.is_empty())
        .is_some();
    for path in candidates {
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let parsed: T = toml::from_str(&text)
                    .map_err(|e| anyhow::anyhow!("invalid config {path}: {e}"))?;
                tracing::info!(path = %path, "loaded TOML config");
                return Ok(Some(parsed));
            }
            // An EXPLICITLY named file that cannot be read is an error: the
            // user pointed at a config on purpose, and starting on silent
            // defaults they believe they overrode is the worst outcome. An
            // absent file on the default search path stays fine.
            Err(e) if explicit => {
                return Err(anyhow::anyhow!(
                    "cannot read config {path} (from ${env_var}): {e}"
                ));
            }
            Err(_) => {}
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_cli() -> StartArgs {
        StartArgs {
            bind: None,
            anthropic_upstream: None,
            openai_upstream: None,
            openai_path: None,
            ccr: None,
            cache_align: None,
            normalize: None,
            compression: None,
            relay_borrowing: None,
            db: None,
            icm_db: None,
            icm_ttl_secs: None,
            upstream_timeout_secs: None,
            upstream_retries: None,
        }
    }

    #[test]
    fn defaults_when_all_unset() {
        let c = Config::resolve(empty_cli(), FileConfig::default());
        assert_eq!(c.bind, DEFAULT_BIND);
        assert_eq!(c.anthropic_upstream, DEFAULT_ANTHROPIC_UPSTREAM);
        assert_eq!(c.openai_path, DEFAULT_OPENAI_PATH);
        assert_eq!(c.db, default_db_path());
        assert_eq!(c.icm_db, default_icm_db_path());
        // Bounded by default: a wedged upstream must not hold a turn open
        // forever.
        assert_eq!(c.upstream_timeout_secs, DEFAULT_UPSTREAM_TIMEOUT_SECS);
        // CCR is the whole point of this proxy: on unless turned off.
        assert!(c.ccr);
    }

    #[test]
    fn ccr_can_be_turned_off_by_file_and_by_cli() {
        let file: FileConfig = toml::from_str("ccr = false").unwrap();
        assert!(!Config::resolve(empty_cli(), file).ccr);

        let mut cli = empty_cli();
        cli.ccr = Some(false);
        assert!(!Config::resolve(cli, FileConfig::default()).ccr);
    }

    #[test]
    fn cache_align_defaults_on_and_can_be_turned_off() {
        assert!(Config::resolve(empty_cli(), FileConfig::default()).cache_align);
        let file: FileConfig = toml::from_str("cache_align = false").unwrap();
        assert!(!Config::resolve(empty_cli(), file).cache_align);
    }

    #[test]
    fn file_overrides_default_cli_overrides_file() {
        let file: FileConfig = toml::from_str(
            r#"
            bind = "0.0.0.0:9000"
            openai_upstream = "https://api.groq.com/openai"
            openai_path = "chat/completions"
        "#,
        )
        .unwrap();

        let c = Config::resolve(empty_cli(), file);
        assert_eq!(c.bind, "0.0.0.0:9000");
        assert_eq!(c.openai_upstream, "https://api.groq.com/openai");
        // Missing leading slash is normalized, not relayed as-is.
        assert_eq!(c.openai_path, "/chat/completions");
        // Untouched key still defaults.
        assert_eq!(c.anthropic_upstream, DEFAULT_ANTHROPIC_UPSTREAM);

        let mut cli = empty_cli();
        cli.bind = Some("127.0.0.1:1".into());
        let file2: FileConfig = toml::from_str(r#"bind = "0.0.0.0:9000""#).unwrap();
        assert_eq!(Config::resolve(cli, file2).bind, "127.0.0.1:1");
    }

    #[test]
    fn to_file_config_round_trips_every_field() {
        let mut cli = empty_cli();
        cli.bind = Some("127.0.0.1:9999".into());
        cli.ccr = Some(false);
        let c = Config::resolve(cli, FileConfig::default());

        let text = toml::to_string(&c.to_file_config()).unwrap();
        let back: FileConfig = toml::from_str(&text).unwrap();
        let c2 = Config::resolve(empty_cli(), back);

        assert_eq!(c2.bind, "127.0.0.1:9999");
        assert!(!c2.ccr);
        assert_eq!(c2.upstream_retries, DEFAULT_UPSTREAM_RETRIES);
    }

    #[test]
    fn loopback_detection_covers_v4_v6_and_names() {
        for yes in [
            "127.0.0.1:8080",
            "localhost:8080",
            "[::1]:8080",
            "127.5.5.5:1",
        ] {
            assert!(bind_is_loopback(yes), "{yes} should be loopback");
        }
        for no in [
            "0.0.0.0:8080",
            "192.168.1.10:8080",
            "[::]:8080",
            "example.com:80",
        ] {
            assert!(!bind_is_loopback(no), "{no} should not be loopback");
        }
    }
}

#[cfg(test)]
mod default_paths {
    use super::*;

    /// The default database paths must not depend on the launch
    /// directory: `rtk gain` reads one fixed location, and a service runs
    /// with `/` as its cwd.
    #[test]
    fn default_db_paths_are_absolute() {
        let c = Config::resolve(
            StartArgs {
                bind: None,
                anthropic_upstream: None,
                openai_upstream: None,
                openai_path: None,
                ccr: None,
                cache_align: None,
                normalize: None,
                compression: None,
                relay_borrowing: None,
                db: None,
                icm_db: None,
                icm_ttl_secs: None,
                upstream_timeout_secs: None,
                upstream_retries: None,
            },
            FileConfig::default(),
        );
        assert!(
            std::path::Path::new(&c.db).is_absolute(),
            "usage store default must be absolute, got {}",
            c.db
        );
        assert!(
            std::path::Path::new(&c.icm_db).is_absolute(),
            "icm store default must be absolute, got {}",
            c.icm_db
        );
    }

    /// An EXPLICITLY named config file that cannot be read must fail
    /// loudly, not silently fall back to defaults the user thinks they
    /// overrode.
    #[test]
    fn an_unreadable_explicit_config_fails_loudly() {
        temp_env::with_var(
            "RTK_PROXY_TEST_UNREADABLE_CONFIG",
            Some("/definitely/not/a/real/config.toml"),
            || {
                let r: anyhow::Result<Option<FileConfig>> =
                    load_toml_file("RTK_PROXY_TEST_UNREADABLE_CONFIG", &[]);
                assert!(
                    r.is_err(),
                    "an explicit config path that cannot be read must error, got {r:?}"
                );
            },
        );
    }
}
