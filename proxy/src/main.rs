//! `rtk-proxy` — the daemon behind `rtk llm-proxy`.
//!
//! A transparent LLM proxy: point a client's base URL here and it relays
//! `/v1/messages` (Anthropic) and `/v1/chat/completions` (OpenAI) to the real
//! upstream, normalising tool results on the way out (ANSI, progress
//! redraws, duplicate lines) and streaming the response back. Reversible CCR
//! compression exists but activates only when a pipeline rule names it.
//!
//! `start` runs the server in the foreground (blocking) until Ctrl-C or
//! `rtk-proxy stop`; `stop`/`status`/`stats`/`config` are thin HTTP clients
//! that talk to an already-running instance — no PID files or signals, so
//! they work no matter how `start` was launched (a terminal, `&`, a login
//! service, or spawned by `rtk llm-proxy start`).

use clap::{Args, Parser, Subcommand};
use rtk_proxy::config::{Config, StartArgs};
use rtk_proxy::settings::keys as setting_keys;
use rtk_proxy::{AppState, memory, server};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "rtk-proxy",
    version,
    about = "Local LLM proxy: normalises tool results, then forwards them upstream"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the proxy server (blocks in the foreground; Ctrl-C or `rtk-proxy stop` to end)
    Start(StartArgs),
    /// Ask a running proxy to shut down gracefully
    Stop(ConnectArgs),
    /// Check whether a proxy is running and reachable
    Status(ConnectArgs),
    /// Print usage stats from a running proxy
    Stats(ConnectArgs),
    /// Get or set runtime settings on a running proxy
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Manage a per-user background service (`start` across logins)
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
}

#[derive(Args, Debug, Clone)]
struct ConnectArgs {
    /// Address of the running proxy to talk to.
    #[arg(long, env = "RTK_PROXY_BIND", default_value = "127.0.0.1:8080")]
    bind: String,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print current settings (or a single key, if given)
    Get {
        #[command(flatten)]
        connect: ConnectArgs,
        /// Setting key to print (omit for all settings)
        key: Option<String>,
    },
    /// Set one runtime setting: compression,
    /// api_key, openai_provider, anthropic_provider, anthropic_upstream,
    /// openai_upstream
    Set {
        #[command(flatten)]
        connect: ConnectArgs,
        key: String,
        value: String,
    },
}

#[derive(Subcommand)]
enum ServiceAction {
    /// Install and start a per-user background service that runs `rtk-proxy
    /// start` on every login (launchd / systemd --user / Task Scheduler --
    /// whichever this platform uses; never system-wide, never needs
    /// sudo/admin). Refuses a non-loopback bind: a background service must
    /// stay on this machine only.
    Install(Box<StartArgs>),
    /// Stop and remove the installed service
    Uninstall,
    /// Whether a service is installed, and whether it's currently reachable
    Status,
    /// Print recent service log output and follow it (Ctrl-C to stop)
    Logs,
}

fn base_url(bind: &str) -> String {
    format!("http://{bind}")
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Start(args) => run_start(args).await,
        Command::Stop(connect) => run_stop(connect).await,
        Command::Status(connect) => run_status(connect).await,
        Command::Stats(connect) => run_stats(connect).await,
        Command::Config { action } => run_config(action).await,
        Command::Service { action } => run_service(action).await,
    }
}

async fn run_start(args: StartArgs) {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let mut config = match Config::from_cli(args) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "config error");
            std::process::exit(1);
        }
    };
    // The transforms the daemon runs come from the active pipeline's
    // `proxy.request` rules. Without `extends = "rtk:proxy"` (or equivalent
    // rules) the daemon forwards unchanged. Floors resolve rule → env → default.
    let rules = rtk_proxy::engine_rules::load();
    config.ccr = config.ccr && rules.ccr;
    config.cache_align = config.cache_align && rules.align;
    // Category A is rule-driven like the others, but its rule lives in
    // `rtk:default` rather than `rtk:proxy`: it is a pure function of block
    // content so it rewrites no cached prefix, and it has no recovery path to
    // get wrong. A daemon you deliberately started should do that
    // much without a second opt-in.
    config.normalize = config.normalize && rules.normalize;
    config.engine_methods = rules.methods;
    rtk_proxy::pipeline::normalize::configure_min_size(rules.min_size);
    rtk_proxy::pipeline::ccr::configure_floors(rtk_proxy::pipeline::ccr::ConfiguredFloors {
        bulk: rules.bulk_floor,
        other: rules.other_floor,
        retire: if rules.retire {
            rules.retire_floor
        } else {
            Some(usize::MAX)
        },
    });
    if !config.ccr && !config.cache_align {
        // Name the path this platform actually reads (`dirs::config_dir()` is
        // ~/Library/Application Support on macOS, %APPDATA% on Windows), and
        // quote a file that parses: a bare `extends = ...` is rejected with
        // E001 `unknown section [extends]` because the key lives under
        // [pipeline]. Either mistake makes the on-ramp silently no-op.
        let path = dirs::config_dir()
            .map(|d| d.join("rtk").join("pipeline.toml").display().to_string())
            .unwrap_or_else(|| "<config dir>/rtk/pipeline.toml".to_string());
        let doing = if config.engine_methods {
            "running your proxy.request method rules"
        } else if config.normalize {
            "running category-A normalisation only (ANSI, progress redraws, adjacent \
             duplicate lines)"
        } else {
            "forwarding unchanged"
        };
        tracing::info!(
            "no cache-breakpoint or CCR rules — {doing}. To add cache-breakpoint alignment, \
             put `schema_version = 1` and `[pipeline]` / `extends = \"rtk:proxy\"` in {path} \
             and restart"
        );
    }
    let bind = config.bind.clone();
    // Every route here is unauthenticated and the proxy forwards the caller's
    // provider credentials, so a non-loopback bind exposes those to the
    // network. `service install` refuses outright; a foreground `start` is an
    // explicit operator action, so warn loudly and continue.
    if !config.binds_loopback() {
        tracing::warn!(
            bind = %bind,
            "binding a non-loopback address: this proxy has no authentication and forwards \
             provider credentials -- put it behind a network boundary or bind 127.0.0.1"
        );
    }
    // CCR cache expiry: once at boot (before the store opens, so no lock
    // contention) and then daily for long-lived daemons. Best-effort by
    // design -- a failed prune is a warning, never a dead daemon.
    let (icm_path, icm_ttl) = (config.icm_db.clone(), config.icm_ttl_secs);
    match memory::prune_ccr_cache(&icm_path, icm_ttl) {
        Ok(0) => {}
        Ok(n) => tracing::info!(pruned = n, ttl_secs = icm_ttl, "ccr cache expired entries"),
        Err(e) => tracing::warn!(error = %e, "ccr cache prune failed"),
    }
    if icm_ttl > 0 && !matches!(icm_path.trim(), "" | "off" | ":memory:") {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
            tick.tick().await;
            loop {
                tick.tick().await;
                let path = icm_path.clone();
                let swept =
                    tokio::task::spawn_blocking(move || memory::prune_ccr_cache(&path, icm_ttl))
                        .await;
                match swept {
                    Ok(Ok(n)) if n > 0 => tracing::info!(pruned = n, "ccr cache daily sweep"),
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => tracing::warn!(error = %e, "ccr cache daily sweep failed"),
                    Err(e) => tracing::warn!(error = %e, "ccr cache sweep task failed"),
                }
            }
        });
    }
    let state = AppState::new(config).with_pipeline_name(&rules.pipeline);
    let shutdown = state.shutdown.clone();
    let app = server::build_router(state);

    let listener = match tokio::net::TcpListener::bind(&bind).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, bind, "failed to bind");
            std::process::exit(1);
        }
    };
    tracing::info!(bind, "rtk-proxy listening");
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let reason = wait_for_shutdown(shutdown).await;
            tracing::info!("shutdown requested via {reason}");
        })
        .await;
    if let Err(e) = result {
        tracing::error!(error = %e, "server error");
        std::process::exit(1);
    }
}

async fn wait_for_shutdown(shutdown: std::sync::Arc<tokio::sync::Notify>) -> &'static str {
    tokio::select! {
        _ = shutdown.notified() => "/admin/shutdown",
        _ = ctrl_c() => "SIGINT",
        _ = sigterm() => "SIGTERM",
    }
}

async fn ctrl_c() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

#[cfg(unix)]
async fn sigterm() {
    use tokio::signal::unix::{SignalKind, signal};
    match signal(SignalKind::terminate()) {
        Ok(mut s) => {
            if s.recv().await.is_none() {
                std::future::pending::<()>().await;
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "SIGTERM handler unavailable");
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(not(unix))]
async fn sigterm() {
    std::future::pending::<()>().await;
}

async fn run_stop(connect: ConnectArgs) {
    let url = format!("{}/admin/shutdown", base_url(&connect.bind));
    match reqwest::Client::new()
        .post(&url)
        .header(rtk_proxy::server::CLI_HEADER, "1")
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => {
            println!("stopped (was listening on {})", connect.bind);
        }
        Ok(r) => {
            eprintln!(
                "rtk-proxy at {} refused shutdown: {}",
                connect.bind,
                r.status()
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("no proxy reachable at {}: {e}", connect.bind);
            std::process::exit(1);
        }
    }
}

async fn run_status(connect: ConnectArgs) {
    let url = format!("{}/health", base_url(&connect.bind));
    let reachable = match reqwest::get(&url).await {
        Ok(r) if r.status().is_success() => r.json::<serde_json::Value>().await.ok(),
        _ => None,
    };
    match reachable {
        Some(j) => println!(
            "running at {} (edition {}, v{})",
            connect.bind,
            j.get("edition").and_then(|v| v.as_str()).unwrap_or("?"),
            j.get("version").and_then(|v| v.as_str()).unwrap_or("?"),
        ),
        None => {
            println!("not running (no proxy reachable at {})", connect.bind);
            std::process::exit(1);
        }
    }
}

async fn run_stats(connect: ConnectArgs) {
    let url = format!("{}/stats/summary", base_url(&connect.bind));
    let resp = match reqwest::get(&url).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("no proxy reachable at {}: {e}", connect.bind);
            std::process::exit(1);
        }
    };
    let j: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(e) => {
            eprintln!("bad response from proxy: {e}");
            std::process::exit(1);
        }
    };
    if j.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
        println!("usage recording is disabled on this proxy (RTK_PROXY_DB=off)");
        return;
    }
    let total = j
        .get("total_requests")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let tokens_in = j.get("tokens_in").and_then(|v| v.as_i64()).unwrap_or(0);
    let tokens_out = j.get("tokens_out").and_then(|v| v.as_i64()).unwrap_or(0);
    let saved = j.get("tokens_saved").and_then(|v| v.as_i64()).unwrap_or(0);
    let cost = j.get("cost_usd").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let pct = if tokens_in + saved > 0 {
        saved as f64 / (tokens_in + saved) as f64 * 100.0
    } else {
        0.0
    };
    println!("rtk-proxy stats ({})", connect.bind);
    println!("  total requests : {total}");
    println!("  tokens in      : {tokens_in}");
    println!("  tokens out     : {tokens_out}");
    println!("  tokens saved   : {saved} ({pct:.1}%)");
    println!("  estimated cost : ${cost:.4}");

    // Best-effort: a compression breakdown is a bonus, never worth failing
    // `stats` over if the request itself errors.
    if let Ok(resp) =
        reqwest::get(format!("{}/stats/by-compression", base_url(&connect.bind))).await
    {
        if let Ok(serde_json::Value::Array(rows)) = resp.json::<serde_json::Value>().await {
            if rows.len() > 1
                || rows
                    .first()
                    .and_then(|r| r.get("compression_level"))
                    .and_then(|v| v.as_str())
                    != Some("unknown")
            {
                println!();
                println!("  by compression level:");
                for row in &rows {
                    let level = row
                        .get("compression_level")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    let n = row.get("requests").and_then(|v| v.as_i64()).unwrap_or(0);
                    let saved = row
                        .get("tokens_saved")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    let pct = row
                        .get("avg_savings_pct")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    println!("    {level:<12} {n:>6} req   {saved:>10} saved   {pct:>5.1}%");
                }
            }
        }
    }
}

async fn run_config(action: ConfigAction) {
    match action {
        ConfigAction::Get { connect, key } => run_config_get(connect, key).await,
        ConfigAction::Set {
            connect,
            key,
            value,
        } => run_config_set(connect, key, value).await,
    }
}

async fn run_config_get(connect: ConnectArgs, key: Option<String>) {
    let url = format!("{}/admin/settings", base_url(&connect.bind));
    let resp = match reqwest::get(&url).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("no proxy reachable at {}: {e}", connect.bind);
            std::process::exit(1);
        }
    };
    let j: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(e) => {
            eprintln!("bad response from proxy: {e}");
            std::process::exit(1);
        }
    };
    match key {
        Some(k) => match j.get(&k) {
            Some(v) => println!("{}", pretty_value(v)),
            None => {
                eprintln!("unknown setting: {k}");
                std::process::exit(1);
            }
        },
        None => println!(
            "{}",
            serde_json::to_string_pretty(&j).unwrap_or_else(|_| j.to_string())
        ),
    }
}

async fn run_config_set(connect: ConnectArgs, key: String, value: String) {
    const KNOWN_KEYS: &[&str] = &[
        setting_keys::CCR_ENABLED,
        setting_keys::ANTHROPIC_UPSTREAM,
        setting_keys::OPENAI_UPSTREAM,
    ];
    if !KNOWN_KEYS.contains(&key.as_str()) {
        eprintln!(
            "unknown setting: {key} (expected one of: {})",
            KNOWN_KEYS.join(", ")
        );
        std::process::exit(1);
    }

    let body = setting_json_body(&key, &value);
    let url = format!("{}/admin/settings", base_url(&connect.bind));
    let resp = match reqwest::Client::new()
        .put(&url)
        .header(rtk_proxy::server::CLI_HEADER, "1")
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("no proxy reachable at {}: {e}", connect.bind);
            std::process::exit(1);
        }
    };
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        eprintln!("rtk-proxy rejected the setting ({status}): {text}");
        std::process::exit(1);
    }
    println!("{key} = {value}");
}

fn pretty_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Build the `PUT /admin/settings` JSON body for one key=value pair, coercing
/// the CLI's plain string value to the type the server expects.
fn setting_json_body(key: &str, value: &str) -> serde_json::Value {
    let typed = if key == setting_keys::CCR_ENABLED {
        value
            .parse::<bool>()
            .map(serde_json::Value::from)
            .unwrap_or_else(|_| serde_json::Value::String(value.to_string()))
    } else {
        serde_json::Value::String(value.to_string())
    };
    serde_json::json!({ key: typed })
}

async fn run_service(action: ServiceAction) {
    match action {
        ServiceAction::Install(args) => run_service_install(*args).await,
        ServiceAction::Uninstall => run_service_uninstall(),
        ServiceAction::Status => run_service_status().await,
        ServiceAction::Logs => run_service_logs(),
    }
}

async fn run_service_install(args: StartArgs) {
    let config = match Config::from_cli(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e:#}");
            std::process::exit(1);
        }
    };
    match rtk_proxy::service::install(&config) {
        Ok(path) => {
            println!(
                "installed via {} ({})",
                rtk_proxy::service::MECHANISM,
                path.display()
            );
            println!("bind: {} (loopback only)", config.bind);
        }
        Err(e) => {
            eprintln!("install failed: {e:#}");
            std::process::exit(1);
        }
    }
}

fn run_service_uninstall() {
    match rtk_proxy::service::uninstall() {
        Ok(true) => println!("uninstalled"),
        Ok(false) => println!("nothing was installed"),
        Err(e) => {
            eprintln!("uninstall failed: {e:#}");
            std::process::exit(1);
        }
    }
}

async fn run_service_status() {
    let installed = match rtk_proxy::service::is_installed() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("status check failed: {e:#}");
            std::process::exit(1);
        }
    };
    if !installed {
        println!("not installed ({})", rtk_proxy::service::MECHANISM);
        return;
    }
    let bind = rtk_proxy::service::installed_bind()
        .ok()
        .flatten()
        .unwrap_or_else(|| "127.0.0.1:8080".to_string());
    println!(
        "installed via {} (bind {bind})",
        rtk_proxy::service::MECHANISM
    );
    run_status(ConnectArgs { bind }).await;
}

fn run_service_logs() {
    if let Err(e) = rtk_proxy::service::logs() {
        eprintln!("{e:#}");
        std::process::exit(1);
    }
}
