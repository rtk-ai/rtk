//! CCR forward: resolves the retrieve-round loop against the real upstream,
//! dialect-dispatched (Anthropic / OpenAI). CCR only runs on the default
//! (non-routed) upstream, which this proxy's passthrough path never pools
//! credentials for.
//!
//! Used only when [`crate::pipeline::ccr::compress_request`] compressed
//! something for this request (or the history already carries a marker
//! from a prior turn); the OFF/nothing-compressed path uses
//! [`crate::forward::relay`] unchanged.

use crate::AppState;
use crate::error::{AppError, AppResult};
use crate::forward::{Forwarded, forwardable_to_upstream};
use crate::memory::IcmMemory;
use crate::model::{InterceptedRequest, Provider, UsageOutcome};
use crate::pipeline::ccr;
use axum::body::Body;
use axum::http::HeaderMap;
use axum::response::Response;
use bytes::Bytes;
use std::future::Future;
use std::time::{Duration, Instant};

/// How long a streaming client may wait on buffered CCR rounds before the
/// proxy commits to an SSE response and keeps it alive with `ping` events.
/// Well under the read timeouts of fronting proxies (Cloudflare's is a fixed
/// 120 s), so a long generation never looks like a dead origin.
const KEEPALIVE_SECS: u64 = 15;

/// The Anthropic Messages API's own keepalive event; clients ignore it.
const ANTHROPIC_PING: &[u8] = b"event: ping\ndata: {\"type\": \"ping\"}\n\n";

/// Forward `req` through the CCR retrieve-resolution loop. The client's
/// `stream` choice only affects how the FINAL answer is emitted; internally
/// every round talks non-stream to upstream (each round must be parsed to
/// look for a retrieve call).
pub async fn execute(
    state: &AppState,
    base: &str,
    headers: &HeaderMap,
    req: &InterceptedRequest,
    memory: &IcmMemory,
) -> AppResult<Forwarded> {
    match req.provider {
        Provider::Anthropic if req.stream => {
            let (state, base, headers, req, memory) = (
                state.clone(),
                base.to_string(),
                headers.clone(),
                req.clone(),
                memory.clone(),
            );
            let rounds = async move { run_anthropic(&state, &base, &headers, &req, &memory).await };
            keep_alive(rounds, Duration::from_secs(KEEPALIVE_SECS)).await
        }
        Provider::Anthropic => run_anthropic(state, base, headers, req, memory).await,
        Provider::Openai => run_openai(state, base, headers, req, memory).await,
        Provider::Google => {
            unreachable!("ccr is only dispatched for anthropic/openai (see server::handle)")
        }
    }
}

/// One non-stream POST to `url` with `headers` + `body`, returning
/// `(status, bytes)`. `None` on a transport-level failure (connect/timeout).
async fn post_once(
    state: &AppState,
    url: &str,
    headers: &HeaderMap,
    body: &serde_json::Value,
) -> Option<(u16, Vec<u8>)> {
    let body_bytes = serde_json::to_vec(body).ok()?;
    let mut builder = state.buffered_post(url).body(body_bytes);
    for (k, v) in headers.iter() {
        if forwardable_to_upstream(k.as_str()) {
            builder = builder.header(k, v);
        }
    }
    let resp = match builder.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, url, "ccr: upstream request failed");
            return None;
        }
    };
    let status = resp.status().as_u16();
    match resp.bytes().await {
        Ok(b) => Some((status, b.to_vec())),
        Err(e) => {
            tracing::warn!(error = %e, "ccr: reading upstream body failed");
            None
        }
    }
}

/// Run the buffered CCR `rounds` for a streaming Anthropic client without
/// leaving its connection silent. A result that lands within `every` is
/// returned as is, so fast turns (and the plain-relay fallback on a failed
/// first round) are unchanged. Past that, the client gets the SSE headers at
/// once and a `ping` event every `every` until the rounds end, then the
/// synthesized events -- or an `error` event, as the 200 is already sent.
async fn keep_alive<F>(rounds: F, every: Duration) -> AppResult<Forwarded>
where
    F: Future<Output = AppResult<Forwarded>> + Send + 'static,
{
    let mut rounds = Box::pin(rounds);
    if let Ok(done) = tokio::time::timeout(every, &mut rounds).await {
        return done;
    }
    let started = Instant::now();
    let (tx, outcome_rx) = tokio::sync::oneshot::channel::<UsageOutcome>();
    let body = async_stream::stream! {
        let mut tick = tokio::time::interval(every);
        let done = loop {
            tokio::select! {
                done = &mut rounds => break done,
                _ = tick.tick() => yield Ok::<_, std::io::Error>(Bytes::from_static(ANTHROPIC_PING)),
            }
        };
        let (status, bytes, outcome) = match done {
            Ok(Forwarded::Buffered { response, outcome }) => {
                let status = response.status().as_u16();
                let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap_or_default();
                (status, bytes, outcome)
            }
            Ok(Forwarded::Streaming { response, outcome_rx }) => {
                let status = response.status().as_u16();
                let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap_or_default();
                (status, bytes, outcome_rx.await.unwrap_or_default())
            }
            Err(e) => {
                tracing::warn!(error = %e, "ccr forward failed after the SSE response began");
                let msg = format!("rtk-proxy: {e}");
                let outcome = UsageOutcome {
                    status: 502,
                    latency_ms: started.elapsed().as_millis() as i64,
                    error_body: Some(msg.clone()),
                    ..Default::default()
                };
                (502, Bytes::from(msg), outcome)
            }
        };
        if status < 400 {
            yield Ok(bytes);
        } else {
            yield Ok(Bytes::from(anthropic_sse_error(&bytes)));
        }
        let _ = tx.send(outcome);
    };
    let response = Response::builder()
        .status(200)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(body))
        .expect("valid sse response");
    Ok(Forwarded::Streaming {
        response,
        outcome_rx,
    })
}

/// An Anthropic SSE `error` event carrying `bytes`: the upstream's own error
/// object when it is one, otherwise `bytes` as an `api_error` message.
fn anthropic_sse_error(bytes: &[u8]) -> String {
    let data = match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(j) if j.get("type").and_then(|t| t.as_str()) == Some("error") => j,
        _ => serde_json::json!({
            "type": "error",
            "error": { "type": "api_error", "message": String::from_utf8_lossy(bytes) }
        }),
    };
    format!("event: error\ndata: {data}\n\n")
}

async fn run_anthropic(
    state: &AppState,
    base: &str,
    headers: &HeaderMap,
    req: &InterceptedRequest,
    memory: &IcmMemory,
) -> AppResult<Forwarded> {
    use crate::pipeline::ccr::anthropic as dialect;

    let url = format!("{}{}", base.trim_end_matches('/'), req.path);
    let session = ccr::session_of(req);
    let mut body = req.body.clone();
    if let Some(obj) = body.as_object_mut() {
        obj.insert("stream".to_string(), serde_json::Value::Bool(false));
    }

    let started = Instant::now();
    let mut retrieve_calls: i64 = 0;
    let mut last_status = 0u16;
    let mut last_bytes: Vec<u8> = Vec::new();
    let mut prior_usage = crate::extract::Usage::default();

    for round in 0..ccr::MAX_ROUNDS {
        if !last_bytes.is_empty() {
            if let Ok(prev) = serde_json::from_slice::<serde_json::Value>(&last_bytes) {
                let ru = crate::extract::usage_from_response(&req.provider, &prev);
                prior_usage.input += ru.input;
                prior_usage.output += ru.output;
                prior_usage.cache_creation += ru.cache_creation;
                prior_usage.cache_read += ru.cache_read;
            }
        }
        let Some((status, bytes)) = post_once(state, &url, headers, &body).await else {
            if round == 0 {
                return Err(AppError::Upstream(
                    "ccr: upstream unreachable on the first round".to_string(),
                ));
            }
            break;
        };
        last_status = status;
        last_bytes = bytes;
        if last_status >= 400 {
            break;
        }
        let json: serde_json::Value = match serde_json::from_slice(&last_bytes) {
            Ok(j) => j,
            Err(_) => break,
        };
        let calls = dialect::retrieve_calls_in_response(&json);
        if calls.is_empty() {
            break;
        }
        if dialect::has_foreign_tool_use(&json) {
            if let Some(clean) = dialect::sanitize_client_response(&json) {
                if let Ok(b) = serde_json::to_vec(&clean) {
                    last_bytes = b;
                }
                tracing::info!(
                    "ccr: mixed tool_use response — retrieve stripped, returned to client"
                );
            }
            break;
        }
        // A retrieve-carrying response that also carries a thinking block is
        // resolved internally, not handed back to the client: Claude Code
        // sends `thinking: {"type":"adaptive"}`, so a thinking block
        // accompanies retrieve calls routinely. The follow-up echoes the
        // response content verbatim, thinking blocks included.
        //
        // A `400 ... each thinking block must contain thinking` comes only
        // from a MALFORMED thinking block (empty / unsigned), which is what a
        // buffered->SSE synthesiser produces when it drops
        // `thinking_delta`/`signature_delta` -- see the "thinking" arm in
        // `synthesize_anthropic_sse`.
        let mut resolved = Vec::with_capacity(calls.len());
        for call in calls {
            let content = resolve_one(state, memory, &session, &call).await;
            retrieve_calls += 1;
            resolved.push((call, content));
        }
        let response_content = json
            .get("content")
            .cloned()
            .unwrap_or(serde_json::Value::Array(Vec::new()));
        let (assistant, user) = dialect::build_followup(&response_content, &resolved);
        if let Some(messages) = body.get_mut("messages").and_then(|m| m.as_array_mut()) {
            messages.push(assistant);
            messages.push(user);
        } else {
            break;
        }
    }

    if last_status < 400 {
        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&last_bytes) {
            if let Some(clean) = dialect::sanitize_client_response(&json) {
                if let Ok(b) = serde_json::to_vec(&clean) {
                    last_bytes = b;
                }
                tracing::warn!(
                    "ccr: retrieve rounds exhausted — stripped unresolved retrieve from the client response"
                );
            }
        }
    }

    let latency_ms = started.elapsed().as_millis() as i64;
    Ok(finalize(
        req,
        last_status,
        &last_bytes,
        latency_ms,
        prior_usage,
        retrieve_calls,
        synthesize_anthropic_sse,
    ))
}

fn openai_non_streaming(body: &serde_json::Value) -> serde_json::Value {
    let mut body = body.clone();
    if let Some(obj) = body.as_object_mut() {
        obj.insert("stream".to_string(), serde_json::Value::Bool(false));
        obj.remove("stream_options");
    }
    body
}

async fn run_openai(
    state: &AppState,
    base: &str,
    headers: &HeaderMap,
    req: &InterceptedRequest,
    memory: &IcmMemory,
) -> AppResult<Forwarded> {
    use crate::pipeline::ccr::openai as dialect;

    let url = format!("{}{}", base.trim_end_matches('/'), req.path);
    let session = ccr::session_of(req);
    let mut body = openai_non_streaming(&req.body);

    let started = Instant::now();
    let mut retrieve_calls: i64 = 0;
    let mut last_status = 0u16;
    let mut last_bytes: Vec<u8> = Vec::new();
    let mut prior_usage = crate::extract::Usage::default();

    for round in 0..ccr::MAX_ROUNDS {
        if !last_bytes.is_empty() {
            if let Ok(prev) = serde_json::from_slice::<serde_json::Value>(&last_bytes) {
                let ru = crate::extract::usage_from_response(&req.provider, &prev);
                prior_usage.input += ru.input;
                prior_usage.output += ru.output;
                prior_usage.cache_creation += ru.cache_creation;
                prior_usage.cache_read += ru.cache_read;
            }
        }
        let Some((status, bytes)) = post_once(state, &url, headers, &body).await else {
            if round == 0 {
                return Err(AppError::Upstream(
                    "ccr: upstream unreachable on the first round".to_string(),
                ));
            }
            break;
        };
        last_status = status;
        last_bytes = bytes;
        if last_status >= 400 {
            break;
        }
        let json: serde_json::Value = match serde_json::from_slice(&last_bytes) {
            Ok(j) => j,
            Err(_) => break,
        };
        let calls = dialect::retrieve_calls_in_response(&json);
        if calls.is_empty() {
            break;
        }
        if dialect::has_foreign_tool_use(&json) {
            if let Some(clean) = dialect::sanitize_client_response(&json) {
                if let Ok(b) = serde_json::to_vec(&clean) {
                    last_bytes = b;
                }
                tracing::info!(
                    "ccr: mixed tool_use response — retrieve stripped, returned to client"
                );
            }
            break;
        }
        let mut resolved = Vec::with_capacity(calls.len());
        for call in calls {
            let content = resolve_one(state, memory, &session, &call).await;
            retrieve_calls += 1;
            resolved.push((call, content));
        }
        let followup = dialect::build_followup(&json, &resolved);
        if let Some(messages) = body.get_mut("messages").and_then(|m| m.as_array_mut()) {
            messages.extend(followup);
        } else {
            break;
        }
    }

    if last_status < 400 {
        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&last_bytes) {
            if let Some(clean) = dialect::sanitize_client_response(&json) {
                if let Ok(b) = serde_json::to_vec(&clean) {
                    last_bytes = b;
                }
                tracing::warn!(
                    "ccr: retrieve rounds exhausted — stripped unresolved retrieve from the client response"
                );
            }
        }
    }

    let latency_ms = started.elapsed().as_millis() as i64;
    Ok(finalize(
        req,
        last_status,
        &last_bytes,
        latency_ms,
        prior_usage,
        retrieve_calls,
        synthesize_openai_sse,
    ))
}

/// Resolve one retrieve call from the session-scoped cache, applying its
/// optional query narrowing. Never fails the round — a lookup error or miss
/// becomes an explanatory string the model reads like any tool result.
async fn resolve_one(
    state: &AppState,
    memory: &IcmMemory,
    session: &str,
    call: &ccr::RetrieveCall,
) -> String {
    let topic = ccr::cache_topic(session, &call.id);
    let content = match memory.get_exact(topic).await {
        Ok(Some(original)) => ccr::apply_query(&original, call.query.as_deref()),
        Ok(None) => format!(
            "[rtk-proxy: no cached content for id \"{}\" in this session]",
            call.id
        ),
        Err(e) => {
            tracing::warn!(error = %e, "ccr: cache lookup failed");
            format!("[rtk-proxy: retrieve failed for id \"{}\"]", call.id)
        }
    };
    let found = !content.starts_with("[rtk-proxy:");
    tracing::info!(
        id = %call.id,
        found,
        "ccr: served a retrieve round-trip (model asked for compressed content)"
    );
    // Tally it so a block the model keeps re-asking for stops being
    // compressed (see `ccr::pin_after_retrieves`). Only a HIT counts: a miss
    // means there is nothing cached under that id, so there is no block to
    // exempt and a hallucinated id must not consume registry capacity.
    if found {
        ccr::note_retrieve(session, &call.id);
        if let Some(store) = &state.store {
            let pinned = ccr::pins().is_pinned_at(session, &call.id, ccr::pin_after_retrieves());
            store.bump_retrieve(session, &call.id, pinned).await;
        }
    }
    content
}

/// Turn the final buffered CCR result into a [`Forwarded`], computing the
/// usage outcome and emitting either a buffered JSON body (non-stream
/// client) or a synthetic SSE stream (`synth`, dialect-specific).
fn finalize(
    req: &InterceptedRequest,
    status: u16,
    bytes: &[u8],
    latency_ms: i64,
    prior: crate::extract::Usage,
    retrieve_calls: i64,
    synth: fn(&[u8]) -> String,
) -> Forwarded {
    let provider = req.provider;
    let (usage, tool_calls) = match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(json) => (
            crate::extract::usage_from_response(&provider, &json),
            crate::extract::tool_calls_from_response(&provider, &json),
        ),
        Err(_) => (crate::extract::Usage::default(), Vec::new()),
    };
    let error_body = (status >= 400).then(|| truncate_error(bytes)).flatten();
    let outcome = UsageOutcome {
        status,
        tokens_input: usage.input + prior.input,
        tokens_output: usage.output + prior.output,
        cache_creation_tokens: usage.cache_creation + prior.cache_creation,
        cache_read_tokens: usage.cache_read + prior.cache_read,
        ccr_retrieve_calls: retrieve_calls,
        latency_ms,
        error_body,
        tool_calls,
        ttfb_ms: Some(latency_ms),
        gen_ms: Some(0),
    };

    if req.stream && status < 400 {
        let sse = synth(bytes);
        let response = Response::builder()
            .status(status)
            .header("content-type", "text/event-stream")
            .body(Body::from(sse))
            .expect("valid sse response");
        Forwarded::Buffered { response, outcome }
    } else {
        let response = Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(Body::from(bytes.to_vec()))
            .expect("valid response");
        Forwarded::Buffered { response, outcome }
    }
}

/// Truncate an upstream error body for storage, matching `forward.rs`'s cap.
fn truncate_error(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    if text.is_empty() {
        return None;
    }
    Some(crate::forward::truncate_text(&text, 4096))
}

/// Build a minimal-but-valid Anthropic SSE event sequence from a complete
/// (non-stream) message JSON, so a streaming client can consume the
/// CCR-resolved answer.
fn synthesize_anthropic_sse(bytes: &[u8]) -> String {
    let json: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(j) => j,
        Err(_) => return String::from_utf8_lossy(bytes).into_owned(),
    };
    let mut out = String::new();
    let ev = |out: &mut String, event: &str, data: &serde_json::Value| {
        out.push_str(&format!("event: {event}\ndata: {data}\n\n"));
    };

    let mut start_msg = json.clone();
    if let Some(o) = start_msg.as_object_mut() {
        o.insert("content".to_string(), serde_json::Value::Array(Vec::new()));
    }
    ev(
        &mut out,
        "message_start",
        &serde_json::json!({ "type": "message_start", "message": start_msg }),
    );

    let empty = Vec::new();
    let blocks = json
        .get("content")
        .and_then(|c| c.as_array())
        .unwrap_or(&empty);
    for (i, block) in blocks.iter().enumerate() {
        let btype = block.get("type").and_then(|t| t.as_str()).unwrap_or("text");
        match btype {
            "text" => {
                let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
                ev(
                    &mut out,
                    "content_block_start",
                    &serde_json::json!({
                        "type": "content_block_start", "index": i,
                        "content_block": { "type": "text", "text": "" }
                    }),
                );
                ev(
                    &mut out,
                    "content_block_delta",
                    &serde_json::json!({
                        "type": "content_block_delta", "index": i,
                        "delta": { "type": "text_delta", "text": text }
                    }),
                );
                ev(
                    &mut out,
                    "content_block_stop",
                    &serde_json::json!({
                        "type": "content_block_stop", "index": i
                    }),
                );
            }
            "tool_use" => {
                let input = block.get("input").cloned().unwrap_or(serde_json::json!({}));
                ev(
                    &mut out,
                    "content_block_start",
                    &serde_json::json!({
                        "type": "content_block_start", "index": i,
                        "content_block": {
                            "type": "tool_use",
                            "id": block.get("id").cloned().unwrap_or(serde_json::Value::Null),
                            "name": block.get("name").cloned().unwrap_or(serde_json::Value::Null),
                            "input": {}
                        }
                    }),
                );
                ev(
                    &mut out,
                    "content_block_delta",
                    &serde_json::json!({
                        "type": "content_block_delta", "index": i,
                        "delta": { "type": "input_json_delta", "partial_json": input.to_string() }
                    }),
                );
                ev(
                    &mut out,
                    "content_block_stop",
                    &serde_json::json!({
                        "type": "content_block_stop", "index": i
                    }),
                );
            }
            "redacted_thinking" => {
                ev(
                    &mut out,
                    "content_block_start",
                    &serde_json::json!({
                        "type": "content_block_start", "index": i,
                        "content_block": block
                    }),
                );
                ev(
                    &mut out,
                    "content_block_stop",
                    &serde_json::json!({
                        "type": "content_block_stop", "index": i
                    }),
                );
            }
            "thinking" => {
                // A thinking block STREAMS as an empty start, then its text and
                // signature as deltas. Clients (Claude Code included) accumulate
                // the text from `thinking_delta` and the signature from
                // `signature_delta`; emitting the block whole in
                // `content_block_start` with no deltas leaves the client holding
                // an EMPTY, unsigned thinking block, which it then replays as
                // history -- and the NEXT upstream call dies with
                // `400 messages.N.content.0.thinking: each thinking block must
                // contain thinking`. Reproduce the real event sequence instead.
                //
                // The retrieve tool is injected unconditionally, which routes
                // EVERY response through this synthesiser, and Claude Code sends
                // `thinking: {"type":"adaptive"}`, so thinking blocks are
                // routine, not an edge case.
                let thinking = block.get("thinking").and_then(|t| t.as_str()).unwrap_or("");
                let signature = block
                    .get("signature")
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                ev(
                    &mut out,
                    "content_block_start",
                    &serde_json::json!({
                        "type": "content_block_start", "index": i,
                        "content_block": { "type": btype, "thinking": "" }
                    }),
                );
                ev(
                    &mut out,
                    "content_block_delta",
                    &serde_json::json!({
                        "type": "content_block_delta", "index": i,
                        "delta": { "type": "thinking_delta", "thinking": thinking }
                    }),
                );
                ev(
                    &mut out,
                    "content_block_delta",
                    &serde_json::json!({
                        "type": "content_block_delta", "index": i,
                        "delta": { "type": "signature_delta", "signature": signature }
                    }),
                );
                ev(
                    &mut out,
                    "content_block_stop",
                    &serde_json::json!({
                        "type": "content_block_stop", "index": i
                    }),
                );
            }
            _ => {
                ev(
                    &mut out,
                    "content_block_start",
                    &serde_json::json!({
                        "type": "content_block_start", "index": i, "content_block": block
                    }),
                );
                ev(
                    &mut out,
                    "content_block_stop",
                    &serde_json::json!({
                        "type": "content_block_stop", "index": i
                    }),
                );
            }
        }
    }

    let stop_reason = json
        .get("stop_reason")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let usage = json.get("usage").cloned().unwrap_or(serde_json::json!({}));
    ev(
        &mut out,
        "message_delta",
        &serde_json::json!({
            "type": "message_delta",
            "delta": { "stop_reason": stop_reason },
            "usage": usage
        }),
    );
    ev(
        &mut out,
        "message_stop",
        &serde_json::json!({ "type": "message_stop" }),
    );
    out
}

/// Build a minimal-but-valid OpenAI `chat.completion.chunk` SSE sequence
/// from a complete (non-stream) response JSON.
fn synthesize_openai_sse(bytes: &[u8]) -> String {
    let json: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(j) => j,
        Err(_) => return String::from_utf8_lossy(bytes).into_owned(),
    };
    let id = json.get("id").cloned().unwrap_or(serde_json::Value::Null);
    let model = json
        .get("model")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let message = json
        .pointer("/choices/0/message")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    let finish_reason = json
        .pointer("/choices/0/finish_reason")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    let mut delta = serde_json::json!({ "role": "assistant" });
    if let Some(content) = message.get("content") {
        if !content.is_null() {
            delta["content"] = content.clone();
        }
    }
    if let Some(tool_calls) = message.get("tool_calls") {
        delta["tool_calls"] = tool_calls.clone();
    }

    let chunk = serde_json::json!({
        "id": id, "object": "chat.completion.chunk", "model": model,
        "choices": [ { "index": 0, "delta": delta, "finish_reason": serde_json::Value::Null } ]
    });
    let final_chunk = serde_json::json!({
        "id": id, "object": "chat.completion.chunk", "model": model,
        "choices": [ { "index": 0, "delta": {}, "finish_reason": finish_reason } ]
    });

    format!("data: {chunk}\n\ndata: {final_chunk}\n\ndata: [DONE]\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A finished CCR turn, as `finalize` hands it back to a streaming client.
    fn buffered(status: u16, body: &str) -> AppResult<Forwarded> {
        let response = Response::builder()
            .status(status)
            .body(Body::from(body.to_string()))
            .unwrap();
        let outcome = UsageOutcome {
            status,
            ..Default::default()
        };
        Ok(Forwarded::Buffered { response, outcome })
    }

    async fn drain(f: Forwarded) -> (u16, String, UsageOutcome) {
        let Forwarded::Streaming {
            response,
            outcome_rx,
        } = f
        else {
            panic!("a slow CCR turn must be streamed to the client");
        };
        let status = response.status().as_u16();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let outcome = outcome_rx.await.unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap(), outcome)
    }

    /// While buffered CCR rounds run past the keepalive interval, the client
    /// already has its SSE headers and gets `ping` events, so a fronting
    /// proxy's read timeout (Cloudflare: 120 s) never sees a silent origin.
    #[tokio::test]
    async fn a_slow_ccr_turn_pings_the_streaming_client() {
        let rounds = async {
            tokio::time::sleep(Duration::from_millis(250)).await;
            buffered(200, "event: message_stop\ndata: {}\n\n")
        };
        let got = keep_alive(rounds, Duration::from_millis(50)).await.unwrap();
        let (status, body, outcome) = drain(got).await;
        assert_eq!(status, 200);
        assert!(body.starts_with("event: ping\n"), "{body}");
        assert!(
            body.ends_with("event: message_stop\ndata: {}\n\n"),
            "{body}"
        );
        assert_eq!(outcome.status, 200);
    }

    /// A turn that ends inside the interval is returned untouched: no ping,
    /// and the upstream status still reaches the client.
    #[tokio::test]
    async fn a_fast_ccr_turn_is_returned_as_is() {
        let rounds = async { buffered(429, r#"{"type":"error"}"#) };
        let got = keep_alive(rounds, Duration::from_secs(5)).await.unwrap();
        let Forwarded::Buffered { response, .. } = got else {
            panic!("a fast CCR turn must stay buffered");
        };
        assert_eq!(response.status().as_u16(), 429);
    }

    /// Once the 200 is sent, an upstream error can only travel as an SSE
    /// `error` event; the usage row still records the real status.
    #[tokio::test]
    async fn an_upstream_error_after_pings_becomes_an_sse_error_event() {
        let err = r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#;
        let rounds = async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            buffered(529, err)
        };
        let got = keep_alive(rounds, Duration::from_millis(50)).await.unwrap();
        let (status, body, outcome) = drain(got).await;
        assert_eq!(status, 200);
        let last = body.rsplit("\n\n").nth(1).unwrap_or_default();
        assert!(last.starts_with("event: error\ndata: "), "{body}");
        assert!(last.contains(r#""overloaded_error""#), "{body}");
        assert_eq!(outcome.status, 529);

        let rounds = async {
            tokio::time::sleep(Duration::from_millis(150)).await;
            Err(AppError::Upstream("down".into()))
        };
        let got = keep_alive(rounds, Duration::from_millis(50)).await.unwrap();
        let (_, body, outcome) = drain(got).await;
        assert!(body.contains(r#""type":"api_error""#), "{body}");
        assert_eq!(outcome.status, 502);
    }

    /// Every CCR round is a buffered call: a long generation sends nothing
    /// until it is done, so the per-read silence limit must not cut it.
    #[tokio::test]
    async fn a_ccr_round_outlives_the_read_timeout() {
        let mut st = AppState::new(crate::config::Config {
            cache_align: true,
            normalize: true,
            engine_methods: false,
            bind: "127.0.0.1:0".into(),
            anthropic_upstream: "https://api.anthropic.com".into(),
            openai_upstream: "https://api.openai.com".into(),
            ccr: true,
            compression: "off".into(),
            pixel_compress: Default::default(),
            relay_borrowing: true,
            db: ":memory:".into(),
            icm_db: ":memory:".into(),
            icm_ttl_secs: 0,
            upstream_timeout_secs: 30,
            upstream_retries: 0,
        });
        st.http = crate::http_client_with(
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(200),
        );
        let base = crate::http_client_tests::slow_upstream(
            std::time::Duration::from_millis(600),
            r#"{"type":"message","content":[]}"#,
        );
        let got = post_once(
            &st,
            &format!("{base}/v1/messages"),
            &HeaderMap::new(),
            &serde_json::json!({}),
        )
        .await;
        assert_eq!(got.map(|(s, _)| s), Some(200));
    }

    #[test]
    fn openai_ccr_body_drops_stream_options_with_streaming() {
        let body = serde_json::json!({
            "model": "m",
            "stream": true,
            "stream_options": {"include_usage": true},
        });
        let out = openai_non_streaming(&body);
        assert_eq!(out["stream"], serde_json::json!(false));
        assert!(out.get("stream_options").is_none(), "{out}");
        assert_eq!(out["model"], serde_json::json!("m"));
    }

    /// A thinking block must survive the buffered->SSE round trip with its text
    /// AND signature carried as deltas. Emitting it whole in
    /// `content_block_start` leaves the client with an empty, unsigned block
    /// that it replays as history, and the next upstream call fails with
    /// `400 ... each thinking block must contain thinking`. Every response
    /// flows through this synthesiser.
    #[test]
    fn synthesised_sse_preserves_thinking_text_and_signature() {
        let body = serde_json::json!({
            "type": "message",
            "content": [
                {"type": "thinking", "thinking": "step one", "signature": "sig-abc"},
                {"type": "text", "text": "answer"}
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1, "output_tokens": 2}
        });
        let sse = synthesize_anthropic_sse(body.to_string().as_bytes());

        assert!(
            sse.contains(r#""type":"thinking_delta""#) && sse.contains("step one"),
            "thinking text must arrive as a thinking_delta:\n{sse}"
        );
        assert!(
            sse.contains(r#""type":"signature_delta""#) && sse.contains("sig-abc"),
            "signature must arrive as a signature_delta:\n{sse}"
        );
        // The start event must be EMPTY -- a client that also accumulates the
        // start payload would otherwise double the thinking text.
        assert!(
            sse.contains(r#""content_block":{"type":"thinking","thinking":""}"#),
            "thinking must start empty and be filled by deltas:\n{sse}"
        );
        assert!(
            sse.contains("answer"),
            "sibling text block must survive too"
        );
    }

    #[test]
    fn synthesised_sse_sends_redacted_thinking_whole_with_its_data() {
        let body = serde_json::json!({
            "type": "message",
            "content": [{"type": "redacted_thinking", "data": "EqQBCgIYAhIM"}],
        });
        let sse = synthesize_anthropic_sse(body.to_string().as_bytes());
        assert!(sse.contains(r#""type":"redacted_thinking""#), "{sse}");
        assert!(sse.contains(r#""data":"EqQBCgIYAhIM""#), "{sse}");
        assert!(!sse.contains("thinking_delta"), "{sse}");
        assert!(sse.contains("content_block_stop"), "{sse}");
    }
}
