//! The proxy's HTTP router: the two provider-compatible intercept endpoints,
//! plus JSON stats/settings/metrics/shutdown and a health check.
//!
//! There is no dashboard. An unmatched path is not a 404 either: a client
//! points its whole base URL here, so anything under `/v1/*` that this proxy
//! does not specifically intercept is relayed to the upstream verbatim (see
//! [`crate::forward::passthrough`]). `stop`/`status`/`stats`/`config` on the
//! CLI are thin HTTP clients over these routes, which is why they work
//! regardless of how the daemon was launched (foreground, `&`, `rtk llm-proxy
//! start`, or a login service).
//!
//! Every route is unauthenticated: the daemon is a single-operator local
//! process and the default bind is loopback-only. `start` warns and
//! `service install` refuses when the bind is not loopback (see
//! [`crate::config::Config::binds_loopback`]).

use crate::AppState;
use crate::model::Provider;
use axum::body::Bytes;
use axum::extract::DefaultBodyLimit;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::trace::TraceLayer;

/// Ceiling on a request body, replacing axum's 2 MB default.
///
/// Real coding-agent requests routinely exceed 2 MB — a long conversation, a
/// pasted file, a big tool result — and axum rejects those with a bare 413
/// before any handler runs, so the proxy would break exactly the traffic it
/// exists to compress. 64 MB leaves the provider's own limit as the real
/// ceiling rather than this proxy.
pub const BODY_LIMIT_BYTES: usize = 64 * 1024 * 1024;

/// Build the proxy router.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/messages", post(anthropic_messages))
        .route("/v1/chat/completions", post(openai_chat_completions))
        .route("/health", get(health))
        // Only the reads `rtk-proxy stats` performs: a route with no caller is
        // surface, and one returning prompt or completion text could leak a
        // conversation. `rtk gain` does not come through here at all -- it
        // opens the usage DB read-only.
        .route("/stats/summary", get(stats_summary))
        .route("/stats/by-compression", get(stats_by_compression))
        .route("/admin/settings", get(get_settings).put(put_settings))
        .route("/admin/shutdown", post(shutdown))
        // Anything else under the provider's API surface (count_tokens,
        // /v1/models, …) is relayed untouched rather than 404'd — a client
        // aims its whole base URL here, not just the completion endpoints.
        .fallback(passthrough)
        .layer(DefaultBodyLimit::max(BODY_LIMIT_BYTES))
        // A handler that panics would otherwise drop the connection with no
        // response at all, which a client reports as a network failure rather
        // than the daemon's fault. 500 is the honest answer, and the process
        // keeps serving every other request.
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Empty-store fallback for the stats endpoints (recording disabled).
fn no_store() -> Response {
    Json(serde_json::json!({ "enabled": false })).into_response()
}

async fn stats_summary(State(s): State<AppState>) -> Response {
    match &s.store {
        Some(st) => Json(st.summary().await).into_response(),
        None => no_store(),
    }
}
async fn stats_by_compression(State(s): State<AppState>) -> Response {
    match &s.store {
        Some(st) => Json(st.by_compression_level().await).into_response(),
        None => no_store(),
    }
}

/// Current settings as JSON (secrets masked): the runtime-editable values plus
/// the live compression level and the read-only boot values.
fn settings_json(s: &AppState) -> serde_json::Value {
    let mut v = s
        .runtime
        .read()
        .map(|r| r.to_json())
        .unwrap_or_else(|_| serde_json::json!({}));
    v["ccr"] = serde_json::json!(s.ccr_enabled.read().map(|g| *g).unwrap_or(false));
    // CCR silently no-ops without an ICM store, so surface that separately
    // rather than lying about `ccr` being on.
    v["ccr_memory_available"] = serde_json::json!(s.memory.is_some());
    v["recording"] = serde_json::json!(s.store.is_some());
    v["bind"] = serde_json::json!(s.config.bind); // read-only (restart to change)
    v["db"] = serde_json::json!(s.config.db); // read-only
    v["openai_path"] = serde_json::json!(s.config.openai_path); // read-only (restart to change)
    v
}

/// `GET /admin/settings` — the current runtime settings (api_key masked).
async fn get_settings(State(s): State<AppState>) -> Response {
    Json(settings_json(&s)).into_response()
}

/// `PUT /admin/settings` — update runtime settings (applied live and
/// persisted). Only the keys present in the body are changed; `bind`/`db` are
/// read-only (restart to change them).
async fn put_settings(
    State(s): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if !from_cli(&headers) {
        return browser_refused();
    }
    let Some(obj) = body.as_object() else {
        return (StatusCode::BAD_REQUEST, "expected a JSON object\n").into_response();
    };
    use crate::settings::keys;
    let mut persist: Vec<(&str, String)> = Vec::new();

    // CCR also needs the ICM store, which is boot-only: turning this on with
    // no memory store is accepted and persisted, but stays a no-op until the
    // proxy is restarted with an ICM DB.
    if let Some(v) = obj.get("ccr").and_then(|v| v.as_bool()) {
        // Same rule as the persisted value at boot: a setting may veto CCR,
        // never enable it past what config and the pipeline rules decided.
        // Accepting the raw bool here would let one PUT out-rank the gate
        // until the next restart re-applied it.
        let effective = v && s.ccr_boot_gate;
        if let Ok(mut g) = s.ccr_enabled.write() {
            *g = effective;
        }
        persist.push((keys::CCR_ENABLED, v.to_string()));
        if v && !effective {
            tracing::warn!(
                "ccr=true accepted and persisted, but the boot gate (config/pipeline) vetoes it — staying off"
            );
        }
    }

    {
        let opt = |v: &serde_json::Value| {
            v.as_str()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        // Upstreams are deliberately NOT settable here. Repointing them is
        // how a request steals the caller's provider key: `forward` passes
        // `authorization` and `x-api-key` to whatever upstream is configured,
        // so one accepted write sends the next turn's key to another host, and
        // it persists. They stay settable where local access is already
        // required -- `--anthropic-upstream`, `RTK_PROXY_*_UPSTREAM`,
        // `rtk-proxy.toml` -- and take effect on restart.
        let _ = &opt;
    }

    if let Some(store) = &s.store {
        for (k, val) in &persist {
            store.put_setting(k, val).await;
        }
    }
    Json(settings_json(&s)).into_response()
}

/// `POST /admin/shutdown` — signals the `Notify` axum's graceful
/// shutdown is waiting on in `main`, then returns immediately; the actual
/// shutdown happens once in-flight requests drain. Unauthenticated, same trust
/// model as `/admin/settings` (default bind is localhost-only).
/// Header the CLI sets on every state-changing request.
///
/// Not a secret and not authentication: its job is to make such a request
/// *non-simple* in CORS terms, so a browser must preflight it. With no CORS
/// layer the preflight fails and the request is never sent — which closes the
/// one hole dropping CORS leaves open, because a `POST` carrying no custom
/// header and a simple content type needs no preflight and would otherwise
/// reach the daemon from any page the user happens to visit.
pub const CLI_HEADER: &str = "x-rtk-proxy";

fn from_cli(headers: &axum::http::HeaderMap) -> bool {
    headers.contains_key(CLI_HEADER)
}

fn browser_refused() -> Response {
    (
        StatusCode::FORBIDDEN,
        format!("this endpoint requires the {CLI_HEADER} header\n"),
    )
        .into_response()
}

async fn shutdown(State(s): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if !from_cli(&headers) {
        return browser_refused();
    }
    s.shutdown.notify_one();
    Json(serde_json::json!({ "status": "shutting down" })).into_response()
}

/// `POST /v1/messages` — Anthropic Messages API (Claude Code).
async fn anthropic_messages(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let base = upstream_for(&s, Provider::Anthropic);
    handle(s, Provider::Anthropic, base, "/v1/messages", headers, body).await
}

/// `POST /v1/chat/completions` — OpenAI Chat Completions API.
async fn openai_chat_completions(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let base = upstream_for(&s, Provider::Openai);
    // Providers whose OpenAI-compatible endpoint doesn't live under `/v1`
    // (e.g. Z.AI's `/chat/completions`) set an explicit forward path; the
    // default matches the historical fixed forward path. Boot-only, same
    // restart-to-apply contract as the upstream base.
    let path = s.config.openai_path.clone();
    handle(s, Provider::Openai, base, &path, headers, body).await
}

/// The upstream base URL for a dialect. One configured upstream per dialect,
/// swappable at runtime (`config set anthropic_upstream …`) — that covers
/// pointing the proxy at an API-compatible gateway without needing a routing
/// table. The request path is forwarded unchanged.
fn upstream_for(s: &AppState, dialect: Provider) -> String {
    let rt = s.runtime.read().expect("runtime settings lock");
    match dialect {
        Provider::Anthropic => rt.anthropic_upstream.clone(),
        _ => rt.openai_upstream.clone(),
    }
}

/// Shared flow: run the compression pipeline, forward, and record usage (the
/// record is spawned so it never adds latency to the response).
async fn handle(
    s: AppState,
    provider: Provider,
    base: String,
    path: &str,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let client =
        crate::extract::detect_client(headers.get("user-agent").and_then(|v| v.to_str().ok()));
    // The agent's credential, kept so a local worker routing to this same
    // provider can borrow it instead of being handed a second copy.
    s.relayed_auth.remember(provider, &headers);
    let applied = crate::forward::apply(&s, provider, path, &headers, body).await;
    let (model, tokens_in, tokens_saved, stream) = (
        applied.model.clone(),
        applied.tokens_in,
        applied.tokens_saved,
        applied.stream,
    );
    let ctx = applied.ctx;
    // A caller that displaced a model declares it with `x-rtk-routed-from`, so
    // the counterfactual prices the work against the model that would have
    // done it. A hook-layer route (delegating a read to a worker) never passes
    // through a proxy transform, so without this its saving is invisible: the
    // worker request would be priced at the worker model it actually used.
    let routed_from = applied
        .routed_from
        .clone()
        .or_else(|| declared_routed_from(&headers));
    let saved_by_stage = applied.saved_by_stage.clone();
    let project = applied.project.clone();
    let session = applied.session.clone();

    // CCR needs its own forward loop: the model can call the injected
    // `rtk_retrieve` tool, which the proxy resolves from the cache itself and
    // re-calls upstream, rather than surfacing to the client. Anything else
    // (CCR off, no ICM store, nothing compressed in this request) takes the
    // plain single-shot relay.
    let ccr_ready = applied.ccr_active
        && s.ccr_enabled.read().map(|g| *g).unwrap_or(false)
        && s.memory.is_some();
    let fwd = if ccr_ready {
        let ccr_body: serde_json::Value =
            serde_json::from_slice(&applied.body).unwrap_or(serde_json::Value::Null);
        let req = crate::model::InterceptedRequest::for_pipeline_with_headers(
            provider,
            path.to_string(),
            ccr_body,
            &headers,
        );
        let memory = s.memory.as_ref().expect("checked above");
        match crate::ccr_forward::execute(&s, &base, &headers, &req, memory).await {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(error = %e, "ccr forward failed; falling back to plain relay");
                crate::forward::relay(&s, &base, path, headers, applied.body, provider, stream)
                    .await
            }
        }
    } else {
        crate::forward::relay(&s, &base, path, headers, applied.body, provider, stream).await
    };

    match fwd {
        // Buffered body → the usage outcome is known now.
        crate::forward::Forwarded::Buffered { response, outcome } => {
            record_usage(
                &s,
                provider,
                path,
                &model,
                tokens_in,
                tokens_saved,
                client,
                project,
                session,
                routed_from,
                ctx,
                &saved_by_stage,
                &outcome,
            );
            response
        }
        // Streamed body → the outcome only lands once the SSE stream ends; record
        // it off the response path when the deferred outcome arrives.
        crate::forward::Forwarded::Streaming {
            response,
            outcome_rx,
        } => {
            if s.store.is_some() {
                let s2 = s.clone();
                let path = path.to_string();
                let saved_by_stage = saved_by_stage.clone();
                let (project, session) = (project.clone(), session.clone());
                let routed_from = routed_from.clone();
                tokio::spawn(async move {
                    if let Ok(outcome) = outcome_rx.await {
                        record_usage(
                            &s2,
                            provider,
                            &path,
                            &model,
                            tokens_in,
                            tokens_saved,
                            client,
                            project,
                            session,
                            routed_from,
                            ctx,
                            &saved_by_stage,
                            &outcome,
                        );
                    }
                });
            }
            response
        }
    }
}

/// Persist one usage row (best-effort, off the response path) from a forward
/// outcome — real output tokens + USD cost, not a zero.
#[allow(clippy::too_many_arguments)]
fn record_usage(
    s: &AppState,
    provider: Provider,
    path: &str,
    model: &str,
    tokens_in: i64,
    tokens_saved: i64,
    client: Option<String>,
    project: Option<String>,
    session: Option<String>,
    routed_from: Option<String>,
    ctx: crate::context_analysis::ContextComposition,
    saved_by_stage: &[(&'static str, i64)],
    outcome: &crate::model::UsageOutcome,
) {
    let Some(store) = s.store.clone() else { return };
    // Billed tokens come from the provider's `usage` block or not at all. A
    // body without one -- every 4xx/5xx -- billed nothing, and estimating
    // from the request size would book a cost that was never charged.
    let billed_in = outcome.tokens_input;
    // Cache tokens are billed tokens: reads at the model's read multiplier,
    // writes at 1.25x input. Pricing them at zero would understate a
    // cache-heavy turn several-fold.
    let billed_any = billed_in > 0
        || outcome.tokens_output > 0
        || outcome.cache_read_tokens > 0
        || outcome.cache_creation_tokens > 0;
    let cost_usd = if billed_any {
        crate::pricing::cost_usd_cached(
            model,
            billed_in,
            outcome.cache_creation_tokens,
            outcome.cache_read_tokens,
            outcome.tokens_output,
        )
        .unwrap_or(0.0)
    } else {
        0.0
    };
    // The same request priced as if the pipeline had not acted: tokens the
    // transforms removed are added back on the input side, and the price is
    // the model the client asked for (`routed_from`) when a route fired.
    // Cache tokens are carried over unchanged — the raw path's real cache
    // behaviour is unobservable, so this side of the comparison is modeled.
    let cost_without = if billed_any {
        crate::pricing::cost_usd_cached(
            routed_from.as_deref().unwrap_or(model),
            billed_in + tokens_saved.max(0),
            outcome.cache_creation_tokens,
            outcome.cache_read_tokens,
            outcome.tokens_output,
        )
        .unwrap_or(0.0)
    } else {
        0.0
    };
    // What compressed this request, for `/stats/by-compression`. CCR is the
    // only lever this build ships, so the column is "ccr" or "off".
    let compression_level = Some(
        if s.ccr_enabled.read().map(|g| *g).unwrap_or(false) && s.memory.is_some() {
            "ccr"
        } else {
            "off"
        }
        .to_string(),
    );
    let row = crate::store::UsageRow {
        ts: chrono::Utc::now().to_rfc3339(),
        provider: provider.as_str().to_string(),
        model: model.to_string(),
        tokens_in,
        tokens_out: outcome.tokens_output,
        tokens_saved,
        cost_usd,
        status: outcome.status as i64,
        client,
        project,
        session,
        pipeline: s.pipeline_name.clone(),
        routed_from,
        cost_without,
        latency_ms: outcome.latency_ms,
        cache_read_tokens: outcome.cache_read_tokens,
        cache_creation_tokens: outcome.cache_creation_tokens,
        ccr_retrieve_calls: outcome.ccr_retrieve_calls,
        path: path.to_string(),
        compression_level,
        ctx_total: ctx.total_tokens,
        ctx_tool_result: ctx.tool_result_tokens,
        ctx_tool_use: ctx.tool_use_input_tokens,
        ctx_duplicate: ctx.duplicate_tokens,
        ctx_image: ctx.image_tokens,
        // One column per known stage. A stage this build doesn't ship stays
        // 0, so one schema serves any set of transforms and `rtk gain` can
        // render the breakdown without knowing which stages actually ran.
        saved_ingest_filter: saved_for("ingest_filter", saved_by_stage),
        saved_pixel_compress: saved_for("pixel_compress", saved_by_stage),
        saved_ccr: saved_for(crate::pipeline::ccr::STAGE_NAME, saved_by_stage),
        saved_normalize: saved_for(crate::pipeline::normalize::STAGE, saved_by_stage),
    };
    tokio::spawn(async move { store.record(row).await });
}

/// Tokens attributed to one stage in a per-stage breakdown, 0 when that stage
/// didn't run.
fn saved_for(stage: &str, saved_by_stage: &[(&'static str, i64)]) -> i64 {
    saved_by_stage
        .iter()
        .find(|(n, _)| *n == stage)
        .map(|(_, v)| *v)
        .unwrap_or(0)
}

/// Relay the rest of the Anthropic Messages dialect; refuse everything else.
///
/// The destination is never inferred. Configuring `anthropic_upstream` is
/// configuring the Messages dialect, so its own namespace (`/v1/messages/…`,
/// e.g. `count_tokens`) relays to exactly that upstream — zero guessing. Any
/// other path answers 404 naming the problem: relaying everything to the
/// Anthropic base would send an OpenAI client's `Authorization` header — its
/// API key — to the wrong provider's host (see
/// `an_openai_clients_key_never_reaches_the_anthropic_upstream`). A 404 on an
/// unknown path is strictly less bad than a credential at the wrong host.
async fn passthrough(State(s): State<AppState>, req: axum::extract::Request) -> Response {
    let path = req.uri().path();
    let in_messages_dialect = path == "/v1/messages" || path.starts_with("/v1/messages/");
    if !in_messages_dialect {
        let method = req.method().clone();
        tracing::warn!(%method, path, "no route: destination unknown, refusing to relay");
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": {
                    "type": "routing_unknown",
                    "message": format!(
                        "no route for {method} {path}: the routing destination is unknown. \
                         This proxy relays only its configured dialect endpoints \
                         (anthropic: /v1/messages/*, openai: /v1/chat/completions); \
                         it never picks an upstream for other paths, because sending \
                         a client's credentials to a guessed host is worse than a 404. \
                         Run this client un-proxied, or point it directly at its provider."
                    ),
                }
            })),
        )
            .into_response();
    }
    let upstream = upstream_for(&s, Provider::Anthropic);
    match crate::forward::passthrough(&s.http, &upstream, req).await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::warn!(error = %e, "passthrough failed");
            e.into_response()
        }
    }
}

/// The model a caller says it displaced (`x-rtk-routed-from`). Consumed here
/// and never forwarded — it is an `x-rtk-` header, so `forwardable_to_upstream`
/// already strips it.
fn declared_routed_from(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-rtk-routed-from")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim())
        .filter(|v| !v.is_empty() && v.len() <= 128)
        .map(|v| v.to_string())
}
async fn health() -> Response {
    Json(serde_json::json!({
        "status": "ok",
        // The service name is the identity the rtk CLI verifies before it
        // trusts the port: 8080 is the most common dev-server port, and a
        // bare 200 from anything listening there must not count as "the
        // proxy is running".
        "service": "rtk-proxy",
        // Distinguishes this local single-operator daemon from any larger
        // deployment built on the same crate; `status` prints it.
        "edition": "local",
        "version": env!("CARGO_PKG_VERSION"),
    }))
    .into_response()
}
