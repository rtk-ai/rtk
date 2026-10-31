//! rtk-proxy — the local LLM proxy behind `rtk llm-proxy`.
//!
//! A single-process daemon that sits between an AI coding tool (Claude Code,
//! Codex, OpenCode, …) and the real provider API, compresses the context on
//! the way out, and forwards the traffic transparently. It is deliberately
//! small: intercept, compress, forward, record. No dashboard, no multi-tenant
//! concern, no hosted deployment story.
//!
//! The one compression lever it ships is **CCR** ([`pipeline::ccr`]) —
//! Compress-Cache-Retrieve, which is reversible: oversized tool output is
//! replaced by a preview plus a marker, the original goes to a local cache,
//! and the model gets it back on demand through an injected `rtk_retrieve`
//! tool the proxy resolves itself.
//!
//! The library surface here (the [`pipeline::Transform`] chain, [`forward`],
//! [`model`], [`store`], [`tokenizer`], [`pricing`], [`metrics`], [`error`])
//! is the extension point downstream binaries build on: they add stages and
//! routes on top rather than forking the daemon.

pub mod ccr_forward;
pub mod config;
pub mod context_analysis;
pub mod engine_rules;
pub mod error;
pub mod extract;
pub mod forward;
pub mod memory;
pub mod metrics;
pub mod model;
pub mod models;
pub mod pipeline;
pub mod pricing;
pub mod relayed_auth;
pub mod server;
pub mod service;
pub mod settings;
pub mod store;
pub mod tokenizer;

use std::sync::{Arc, RwLock};
use tokio::sync::Notify;

/// Shared state injected into every handler.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<config::Config>,
    /// HTTP client to the upstream providers (rustls, streaming).
    pub http: reqwest::Client,
    /// The request-transform pipeline. Empty = passthrough.
    pub pipeline: pipeline::Pipeline,
    /// Usage store for stats. `None` when recording is disabled (`RTK_PROXY_DB`
    /// empty/off) or the DB failed to open.
    pub store: Option<store::Store>,
    /// Runtime-editable settings, seeded from config and overlaid with the
    /// persisted `app_settings` (so a `config set` survives a restart).
    pub runtime: Arc<RwLock<settings::RuntimeSettings>>,
    /// Live ingest-filter level, togglable at runtime. `Off` = the stage is
    /// a pure no-op, which is the default: the filter truncates, and that is
    /// lossy, so it is opt-in.
    pub compression_level: Arc<RwLock<pipeline::level::CompressionLevel>>,
    /// EXPERIMENTAL pixel-compress on/off, and its TOML-only tuning.
    pub pixel_compress_enabled: Arc<RwLock<bool>>,
    pub pixel_compress_cfg: Arc<config::PixelCompressConfig>,
    /// The agent's own credential, as last relayed. Lent to a local worker
    /// routing to the same provider, so it needs no second copy of a key the
    /// agent is already sending. In memory only — see [`relayed_auth`].
    pub relayed_auth: Arc<relayed_auth::RelayedAuth>,
    /// Permanent memory (ICM): CCR's cache backend. `None` when
    /// `RTK_PROXY_ICM_DB` is empty/off or the store failed to open — in which
    /// case CCR silently stays a no-op regardless of `ccr_enabled` (see
    /// [`pipeline::ccr::CcrTransform`]).
    pub memory: Option<memory::IcmMemory>,
    /// CCR (Compress-Cache-Retrieve) on/off, runtime-togglable via
    /// `rtk llm-proxy config set ccr false`.
    pub ccr_enabled: pipeline::ccr::SharedFlag,
    /// What boot decided CCR may be at most (config AND pipeline rules).
    /// Runtime settings may veto below this, never enable above it — the
    /// same invariant `effective_ccr` applies to the persisted value.
    pub ccr_boot_gate: bool,
    /// Signaled by `POST /admin/shutdown` (`rtk-proxy stop`) to trigger axum's
    /// graceful shutdown from `main`. No PID files/signals: `stop`/`status`/
    /// `stats` are plain HTTP calls, so they work regardless of how `start`
    /// was launched (foreground terminal, `&`, or spawned by `rtk llm-proxy`).
    pub shutdown: Arc<Notify>,
    /// Name of the resolved pipeline whose `proxy.request` rules drive this
    /// daemon (`engine_rules::load` at boot). Stamped on every usage row so
    /// `rtk gain` can attribute savings per pipeline. `None` when boot did
    /// not resolve one (tests, embedded use).
    pub pipeline_name: Option<String>,
}

/// Seconds to establish TCP+TLS to the provider. A connection that has not
/// been established will never produce tokens, so this is short.
const CONNECT_TIMEOUT_SECS: u64 = 10;

/// Seconds of silence tolerated WITHIN a response before it is abandoned.
/// Applies to streams too, where a total timeout must not: providers send
/// keep-alives, so a gap this long means the far end is gone. Without it a
/// half-open socket holds the agent's turn open indefinitely.
const READ_TIMEOUT_SECS: u64 = 120;

/// The upstream client. `reqwest::Client::new()` sets no timeouts of any
/// kind, which turns one unresponsive provider socket into a turn that
/// never ends.
fn http_client() -> reqwest::Client {
    http_client_with(
        std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS),
        std::time::Duration::from_secs(READ_TIMEOUT_SECS),
    )
}

fn http_client_with(connect: std::time::Duration, read: std::time::Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(connect)
        .read_timeout(read)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

impl AppState {
    pub fn new(config: config::Config) -> Self {
        // A database left as a bare filename in the shell's cwd is invisible
        // to `rtk gain`, which reads one fixed path. Adopt such a file once,
        // only when still on the default path (never over an explicit
        // setting).
        for (path, default) in [
            (&config.db, config::default_db_path()),
            (&config.icm_db, config::default_icm_db_path()),
        ] {
            if *path == default {
                if let Some(from) = config::adopt_stray_db(path) {
                    tracing::info!(from = %from.display(), to = %path, "adopted database from launch directory");
                }
            }
        }
        // A relative path a `service install` froze still resolves against the
        // service's cwd (`/` under launchd), unwritable and invisible to
        // `rtk gain`. Nothing here may override an explicit setting, so it
        // says so and names both exits.
        for (key, path) in [
            ("db", config.db.as_str()),
            ("icm_db", config.icm_db.as_str()),
        ] {
            let p = path.trim();
            if !matches!(p, "" | "off" | ":memory:") && std::path::Path::new(p).is_relative() {
                tracing::warn!(
                    key,
                    path = p,
                    "database path is relative: it resolves against this process's working \
                     directory, while `rtk gain` reads one fixed path. Set an absolute path, or \
                     re-run `rtk llm-proxy service install` to refresh a frozen service config."
                );
            }
        }
        let store = match config.db.trim() {
            "" | "off" => None,
            path => match store::Store::open(path) {
                Ok(s) => {
                    tracing::info!(db = path, "usage store open");
                    Some(s)
                }
                Err(e) => {
                    tracing::warn!(error = %e, db = path, "usage store disabled (open failed)");
                    None
                }
            },
        };

        let mut runtime = settings::RuntimeSettings::from_config(&config);
        if let Some(st) = &store {
            runtime.apply_persisted(&st.all_settings_sync());
        }

        // Ingest filter: level from the file, overridden by whatever was last
        // persisted at runtime (DB wins, same precedence as CCR). The stage is
        // always present so the level can be toggled live; `Off` is a no-op.
        let mut level = pipeline::level::CompressionLevel::parse(&config.compression)
            .unwrap_or(pipeline::level::CompressionLevel::Off);
        if let Some(st) = &store {
            if let Some(persisted) = st
                .all_settings_sync()
                .get(settings::keys::COMPRESSION)
                .and_then(|v| pipeline::level::CompressionLevel::parse(v))
            {
                level = persisted;
            }
        }
        let compression_level = Arc::new(RwLock::new(level));

        // EXPERIMENTAL pixel-compress: boot default from `[pixel_compress]
        // enabled`, overridden by the persisted runtime value. The rest of the
        // tuning is TOML-only, captured once here.
        let mut pixel_compress_on = config.pixel_compress.enabled;
        if let Some(st) = &store {
            if let Some(v) = st
                .all_settings_sync()
                .get(settings::keys::PIXEL_COMPRESS)
                .map(|v| settings::parse_bool(v))
            {
                pixel_compress_on = v;
            }
        }
        let pixel_compress_enabled = Arc::new(RwLock::new(pixel_compress_on));
        let pixel_compress_cfg = Arc::new(config.pixel_compress.clone());

        let memory = match config.icm_db.trim() {
            "" | "off" => None,
            path => match memory::IcmMemory::open(path) {
                Ok(m) => {
                    tracing::info!(icm_db = path, "icm memory store open");
                    Some(m)
                }
                Err(e) => {
                    tracing::warn!(error = %e, icm_db = path, "icm memory store disabled (open failed)");
                    None
                }
            },
        };

        // CCR: `config.ccr` arrives already gated by the resolved pipeline's
        // rules (main.rs ANDs them at boot). The persisted runtime key may
        // only DISABLE within that authorization — never enable CCR a
        // pipeline no longer names, or a `config set ccr true` from months
        // ago would outlive the rule that justified it and silently rewrite
        // history against the off-by-default rule.
        let persisted_ccr = store
            .as_ref()
            .and_then(|st| {
                st.all_settings_sync()
                    .get(settings::keys::CCR_ENABLED)
                    .cloned()
            })
            .map(|v| settings::parse_bool(&v));
        let ccr_boot_gate = config.ccr;
        let ccr_on = effective_ccr(ccr_boot_gate, persisted_ccr);
        let ccr_enabled: pipeline::ccr::SharedFlag = Arc::new(RwLock::new(ccr_on));

        if ccr_on && memory.is_none() {
            tracing::warn!(
                "CCR is enabled but the ICM memory store is unavailable — CCR will no-op"
            );
        }

        let pipeline = pipeline::Pipeline::new()
            // Ingest filter first: it trims tool_result /
            // tool_use blocks before any later stage pays to look at them.
            // Lossy (it truncates), so `Off` unless a level is set.
            .with(Arc::new(
                pipeline::ingest_filter::IngestFilterTransform::new(
                    compression_level.clone(),
                    Arc::new(RwLock::new(false)),
                ),
            ))
            // Cache alignment is safe when on (only injects an ephemeral cache
            // breakpoint when the client set none — never overrides one).
            // Boot-only toggle: flipping it mid-session would rewrite the
            // cached prefix.
            .with(Arc::new(pipeline::cache_align::CacheAlignTransform::new(
                Arc::new(RwLock::new(config.cache_align)),
            )))
            // Category A normalisation runs BEFORE ccr: it is lossless-by-
            // intent and marker-free, so whatever it removes is removed
            // without any recovery path, and ccr then sees smaller blocks.
            // Its line-removing transforms refuse to touch a block the rtk
            // hook already truncated (see `normalize::hook_truncated`).
            //
            // Gated like every other stage: a daemon with no `proxy.request`
            // rules forwards unchanged, so the boot message stays true.
            .with_if(config.normalize, || {
                Arc::new(pipeline::normalize::NormalizeTransform::new())
            })
            .with_if(config.engine_methods, || {
                Arc::new(pipeline::engine_methods::EngineMethodsTransform::new(
                    engine_rules::load_resolved().0,
                ))
            })
            // Pixel-compress sits after normalize (so it renders already-clean
            // text) and before ccr.
            .with(Arc::new(
                pipeline::pixel_compress::PixelCompressTransform::new(
                    pixel_compress_enabled.clone(),
                    pixel_compress_cfg.clone(),
                ),
            ))
            .with(Arc::new(
                pipeline::ccr::CcrTransform::new(ccr_enabled.clone(), memory.clone())
                    .with_store(store.clone()),
            ));
        tracing::info!(stages = ?pipeline.stage_names(), ccr = ccr_on, "pipeline");

        Self {
            config: Arc::new(config),
            http: http_client(),
            pipeline,
            store,
            runtime: Arc::new(RwLock::new(runtime)),
            compression_level,
            pixel_compress_enabled,
            pixel_compress_cfg,
            relayed_auth: Arc::new(relayed_auth::RelayedAuth::default()),
            memory,
            ccr_enabled,
            ccr_boot_gate,
            shutdown: Arc::new(Notify::new()),
            pipeline_name: None,
        }
    }

    pub fn with_pipeline_name(mut self, name: &str) -> Self {
        self.pipeline_name = (!name.is_empty()).then(|| name.to_string());
        self
    }
}

/// The runtime CCR flag a boot resolves to: the rules-gated config value,
/// which a persisted setting may veto but never override upward.
fn effective_ccr(gated: bool, persisted: Option<bool>) -> bool {
    gated && persisted.unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::effective_ccr;

    #[test]
    fn persisted_ccr_can_veto_but_never_enable() {
        assert!(effective_ccr(true, None));
        assert!(effective_ccr(true, Some(true)));
        assert!(!effective_ccr(true, Some(false)));
        assert!(
            !effective_ccr(false, Some(true)),
            "a stale `config set ccr true` must not outlive the pipeline rule"
        );
        assert!(!effective_ccr(false, None));
    }
}

#[cfg(test)]
mod http_client_tests {
    /// The upstream client must abandon a socket that goes
    /// silent. A listener that accepts and never writes would hold the
    /// request forever; with a read timeout it errors out.
    #[tokio::test]
    async fn a_silent_upstream_socket_times_out() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for conn in listener.incoming() {
                held.push(conn);
            }
        });
        let client = super::http_client_with(
            std::time::Duration::from_millis(500),
            std::time::Duration::from_millis(200),
        );
        let started = std::time::Instant::now();
        let result = client
            .post(format!("http://{addr}/v1/messages"))
            .body("{}")
            .send()
            .await;
        assert!(result.is_err(), "a never-answering socket must not hang");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "the timeout must bound the wait"
        );
    }
}
