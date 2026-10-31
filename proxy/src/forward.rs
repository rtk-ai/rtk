//! Transparent forwarding of an intercepted request to the real upstream.
//!
//! The proxy is a passthrough: it relays the caller's request (headers + body,
//! including their own API key) to the configured upstream and streams the
//! response straight back — so any Anthropic/OpenAI client keeps working with
//! its base URL pointed here. There is no routing table, no key pool and no
//! credential of its own; a downstream binary layers that on top.
//!
//! On the way back the response is *tallied* (without altering it): the usage
//! outcome — output tokens, cache tokens, latency — is extracted so the stats
//! store records a real cost, not a zero. Buffered (non-stream) bodies are parsed
//! once; streamed SSE bodies are relayed chunk-by-chunk with the tally computed
//! on the fly, the final [`UsageOutcome`] delivered through a `oneshot` when the
//! stream ends. This `relay` + `SseAccumulator` is the forwarding leaf both
//! CCR's retrieve loop ([`crate::ccr_forward`]) and any downstream routing
//! machinery build on.

use crate::AppState;
use crate::extract;
use crate::model::{InterceptedRequest, Provider, ToolCall, UsageOutcome};
use axum::body::{Body, Bytes};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use std::time::Instant;
use tokio::sync::oneshot;

/// Max length (bytes) of a captured upstream error body before truncation.
const MAX_ERROR_BODY: usize = 4096;

/// Transparent passthrough for provider API paths this proxy does not
/// specifically intercept — `POST /v1/messages/count_tokens`, `GET /v1/models`,
/// and anything else a client SDK calls on the same base URL.
///
/// Claude Code points *only* its `ANTHROPIC_BASE_URL` here, so every one of its
/// calls arrives at this proxy, not just `/v1/messages`. Without this, those
/// requests 404 and the SDK gets a body it cannot parse where it expects JSON —
/// which is enough to break the client outright.
///
/// Relays verbatim: same method, same path+query, the auth/version headers the
/// upstream needs, and the body unchanged. No pipeline, no recording — these
/// are metadata calls, not completions.
pub async fn passthrough(
    http: &reqwest::Client,
    upstream_base: &str,
    req: axum::extract::Request,
) -> crate::error::AppResult<Response> {
    use crate::error::AppError;

    let (parts, body) = req.into_parts();
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or_else(|| parts.uri.path());
    let url = format!("{}{}", upstream_base.trim_end_matches('/'), path_and_query);

    // Same ceiling as the router, so a passthrough endpoint is never a tighter
    // bottleneck than `/v1/messages`.
    let body_bytes = axum::body::to_bytes(body, crate::server::BODY_LIMIT_BYTES)
        .await
        .map_err(|e| AppError::BadRequest(format!("read request body: {e}")))?;

    let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes())
        .map_err(|e| AppError::BadRequest(format!("invalid method: {e}")))?;

    let mut builder = http.request(method, &url);
    for (name, value) in parts.headers.iter() {
        if forwardable_to_upstream(name.as_str()) {
            builder = builder.header(name, value);
        }
    }
    if !body_bytes.is_empty() {
        builder = builder.body(body_bytes.to_vec());
    }

    let upstream = builder.send().await?;
    let status = upstream.status().as_u16();
    let headers = upstream.headers().clone();
    let bytes = upstream.bytes().await?;

    let mut response = Response::builder().status(status);
    for (name, value) in headers.iter() {
        if !is_hop_by_hop(name.as_str()) {
            response = response.header(name, value);
        }
    }
    response
        .body(Body::from(bytes))
        .map_err(|e| AppError::Internal(format!("build passthrough response: {e}")))
}

/// The request after the pipeline ran: the (possibly-rewritten) bytes to forward,
/// plus a few fields for the usage store.
pub struct Applied {
    pub body: Bytes,
    pub model: String,
    /// Estimated input tokens of the request **as forwarded** (post-pipeline),
    /// i.e. what the caller will actually be billed for on input. It is the
    /// whole body -- system, tool definitions and every message -- not just
    /// the latest user turn, so `tokens_saved / (tokens_in + tokens_saved)`
    /// is a truthful "share of the original context removed". Used as the
    /// cost fallback only when the provider reports no input count of its own.
    pub tokens_in: i64,
    pub tokens_saved: i64,
    /// The model the client asked for when a route transform swapped it, so
    /// the usage row can price the counterfactual (same tokens at the
    /// original model). `None` when no route fired.
    pub routed_from: Option<String>,
    /// Per-stage attribution of `tokens_saved` — see
    /// [`InterceptedRequest::saved_by_stage`]. Carries whatever stages ran,
    /// so a downstream binary's own transforms show up here without a change
    /// to this struct.
    pub saved_by_stage: Vec<(&'static str, i64)>,
    /// Whether the client asked for a streamed (SSE) response.
    pub stream: bool,
    /// Session the request declared (`x-rtk-session`), carried through so the
    /// CCR forward loop scopes its cache reads the same way the compress step
    /// scoped its writes.
    pub session: Option<String>,
    /// Project the request declared (`x-rtk-project`), for the usage row.
    pub project: Option<String>,
    /// Whether CCR compressed something this turn (or a prior turn's marker
    /// is still in the history) -- `server::handle` dispatches to
    /// `crate::ccr_forward::execute` instead of the plain `relay` when true.
    pub ccr_active: bool,
    /// How the (post-pipeline) request body breaks down — tool output vs.
    /// tool-call args vs. images vs. plain text — for the usage store's
    /// context-composition columns. Default (all zero) on the early-return
    /// paths (non-JSON body, pipeline error): no body to analyze.
    pub ctx: crate::context_analysis::ContextComposition,
}

/// Truncate `s` to at most `max` bytes on a char boundary, appending `…` when cut.
pub fn truncate_text(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// Parse the JSON body, run the compression pipeline over it, and report the
/// bytes to forward + stats fields. A pure passthrough (body unchanged, stats
/// zeroed) when the body isn't JSON; a no-op rewrite when the pipeline is empty
/// or a transform errors (compression must never break a request).
pub async fn apply(
    state: &AppState,
    provider: Provider,
    path: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> Applied {
    let json: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return Applied {
                body,
                model: String::new(),
                tokens_in: 0,
                tokens_saved: 0,
                routed_from: None,
                saved_by_stage: Vec::new(),
                session: None,
                project: None,
                stream: false,
                ccr_active: false,
                ctx: crate::context_analysis::ContextComposition::default(),
            };
        }
    };
    let mut req =
        InterceptedRequest::for_pipeline_with_headers(provider, path.to_string(), json, headers);
    let stream = req.stream;

    if !state.pipeline.stage_names().is_empty()
        && state.pipeline.run_request(&mut req).await.is_err()
    {
        // Transform failed — forward the original body, keep the stats we can
        // still trust. The composition is measured off the parsed body rather
        // than skipped: a failed stage must not also blank out the usage row.
        let ctx = crate::context_analysis::analyze(&req.body);
        return Applied {
            body,
            model: req.model,
            tokens_in: ctx.total_tokens,
            tokens_saved: 0,
            routed_from: None,
            saved_by_stage: Vec::new(),
            session: req.session.clone(),
            project: req.project.clone(),
            stream,
            ccr_active: false,
            ctx,
        };
    }
    // Anthropic/OpenAI only (CCR has no Google/Gemini dialect implementation).
    // Arm the resolving loop only when a retrieve could actually name
    // something: the tool is injected on EVERY request (cache stability --
    // see pipeline::ccr::ensure_retrieve_tool), but an id exists only inside
    // a marker, and a block compressed THIS turn leaves its marker in this
    // very body -- so one history scan covers both. Arming on tool presence
    // alone would force every request of a CCR session through the buffered
    // loop, which rewrites `stream:false`. A fresh conversation streams; the
    // first compression makes later turns buffered, which are exactly the
    // turns a retrieve can occur in. The
    // residual -- the model inventing a retrieve id it never saw, mid-stream
    // -- surfaces as an unresolved tool call rather than silently, and names
    // nothing.
    let ccr_active = matches!(req.provider, Provider::Anthropic | Provider::Openai)
        && crate::pipeline::ccr::retrieve_tool_present(req.provider, &req.body)
        && crate::pipeline::ccr::history_has_marker(&req.body)
        && !crate::pipeline::ccr::force_stream();
    // CCR reports its savings on `req.ccr` (its compress step runs before it
    // knows the final token tally), so fold that into the shared aggregate +
    // per-stage breakdown here rather than inside the transform.
    let ccr_saved = req.ccr.tokens_compressed;
    req.record_saving(crate::pipeline::ccr::STAGE_NAME, ccr_saved);
    // Composition of the body as it's actually being sent (post-pipeline) --
    // reflects what compression already removed, not the original size. Its
    // total doubles as the input-token estimate: counting only the latest
    // user turn (what `prompt_text` returns) would leave `tokens_in` at 0 for
    // a tool-result-only request and report a nonsensical 100% saving.
    let ctx = crate::context_analysis::analyze(&req.body);
    let out = serde_json::to_vec(&req.body)
        .map(Bytes::from)
        .unwrap_or(body);
    Applied {
        body: out,
        model: req.model,
        tokens_in: ctx.total_tokens,
        tokens_saved: req.tokens_saved,
        routed_from: req.routed_from,
        saved_by_stage: req.saved_by_stage,
        session: req.session,
        project: req.project,
        stream,
        ccr_active,
        ctx,
    }
}

/// Headers we must not copy through (hop-by-hop or length/host, which the client
/// library recomputes). Applied to both request and response headers.
/// Proxy-internal request headers: consumed here, never forwarded.
///
/// These select or annotate proxy behaviour (`x-rtk-session` scopes CCR's
/// cache, `x-rtk-project` attributes the usage row). Leaking them upstream
/// sends the provider a header it never asked for and discloses local session
/// and project names to a third party.
pub(crate) fn is_proxy_internal(name: &str) -> bool {
    name.len() > 6 && name[..6].eq_ignore_ascii_case("x-rtk-")
}

/// Whether a client request header may be relayed to the upstream. The single
/// predicate every request-side header copy uses, so a new internal header
/// cannot leak through one path while being stripped on another.
pub(crate) fn forwardable_to_upstream(name: &str) -> bool {
    !is_hop_by_hop(name) && !is_proxy_internal(name)
}

pub(crate) fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name,
        "host"
            | "content-length"
            | "connection"
            | "transfer-encoding"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "upgrade"
            | "accept-encoding"
    )
}

/// The result of forwarding: a client-facing response plus the way its usage
/// outcome is delivered — immediately for a buffered body, deferred (via a
/// `oneshot` that fires when the stream terminates) for a streamed one.
pub enum Forwarded {
    Buffered {
        response: Response,
        outcome: UsageOutcome,
    },
    Streaming {
        response: Response,
        outcome_rx: oneshot::Receiver<UsageOutcome>,
    },
}

/// Forward `body` + `headers` to `{base}{path}` and relay the upstream response
/// back **transparently** (every non-hop-by-hop header preserved), tallying the
/// usage outcome for the stats store. `stream` (the client's `stream: true`)
/// selects the SSE relay, which counts tokens as the bytes flow; otherwise the
/// body is buffered and parsed once.
pub async fn relay(
    state: &AppState,
    base: &str,
    path: &str,
    headers: HeaderMap,
    body: Bytes,
    provider: Provider,
    stream: bool,
) -> Forwarded {
    let url = format!("{}{}", base.trim_end_matches('/'), path);
    let retries = state.config.upstream_retries;
    let timeout = state.config.upstream_timeout_secs;
    let started = Instant::now();

    let mut attempt: u32 = 0;
    loop {
        let mut req = state.http.post(&url).body(body.clone());
        // A total request timeout is safe only for buffered calls; a streamed SSE
        // response is long-lived and must never be cut mid-stream.
        if !stream && timeout > 0 {
            req = req.timeout(std::time::Duration::from_secs(timeout));
        }
        for (k, v) in headers.iter() {
            if forwardable_to_upstream(k.as_str()) {
                req = req.header(k, v);
            }
        }
        // A local worker that sent no credential borrows the one the agent is
        // already relaying, rather than needing a second copy of it.
        if state.config.relay_borrowing {
            if let Some((name, value)) = state.relayed_auth.lend_if_missing(provider, &headers) {
                req = req.header(name, value);
            }
        }

        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                // Retry a transient status only before we've handed off the body
                // (headers are in; the body — buffered or streamed — is untouched).
                if is_transient(status.as_u16()) && attempt < retries {
                    attempt += 1;
                    tracing::warn!(
                        url,
                        status = status.as_u16(),
                        attempt,
                        "transient upstream status; retrying"
                    );
                    backoff(attempt).await;
                    continue;
                }
                return if stream && status.is_success() {
                    stream_relay(resp, provider, started)
                } else {
                    buffered_relay(resp, provider, started).await
                };
            }
            Err(e) => {
                if (e.is_timeout() || e.is_connect()) && attempt < retries {
                    attempt += 1;
                    tracing::warn!(error = %e, url, attempt, "transient upstream error; retrying");
                    backoff(attempt).await;
                    continue;
                }
                tracing::warn!(error = %e, url, "upstream forward failed");
                let latency_ms = started.elapsed().as_millis() as i64;
                let outcome = UsageOutcome {
                    status: 502,
                    // The real error -- upstream URL included -- belongs in
                    // the local usage row, where the operator debugging their
                    // own daemon reads it. The client body goes through
                    // AppError::Upstream, whose whole job is to not say it.
                    error_body: Some(format!("upstream error: {e}")),
                    latency_ms,
                    ttfb_ms: Some(latency_ms),
                    gen_ms: Some(0),
                    ..Default::default()
                };
                let response = crate::error::AppError::Upstream(e.to_string()).into_response();
                return Forwarded::Buffered { response, outcome };
            }
        }
    }
}

/// Upstream statuses worth retrying (rate-limit + transient server errors).
fn is_transient(code: u16) -> bool {
    matches!(code, 429 | 500 | 502 | 503 | 504)
}

/// Exponential backoff between retry attempts (200ms, 400, 800, … capped at 2s).
async fn backoff(attempt: u32) {
    let ms = 100u64.saturating_mul(1u64 << attempt.min(4));
    tokio::time::sleep(std::time::Duration::from_millis(ms.min(2000))).await;
}

/// Copy the upstream's non-hop-by-hop headers onto the client-facing response,
/// keeping the relay transparent (this proxy never curates provider headers
/// — rate-limit, request-id, cache signals all pass through unchanged).
fn copy_resp_headers(
    mut builder: axum::http::response::Builder,
    src: &reqwest::header::HeaderMap,
) -> axum::http::response::Builder {
    for (k, v) in src.iter() {
        if !is_hop_by_hop(k.as_str()) {
            builder = builder.header(k, v);
        }
    }
    builder
}

/// Buffer the whole upstream body (non-stream or error), parse its usage, and
/// return it with a fully-known [`UsageOutcome`].
async fn buffered_relay(
    resp: reqwest::Response,
    provider: Provider,
    started: Instant,
) -> Forwarded {
    let status = resp.status();
    let status_u16 = status.as_u16();
    let headers = resp.headers().clone();
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, "reading upstream body failed");
            let latency_ms = started.elapsed().as_millis() as i64;
            let outcome = UsageOutcome {
                status: 502,
                error_body: Some(format!("upstream body error: {e}")),
                latency_ms,
                ttfb_ms: Some(latency_ms),
                gen_ms: Some(0),
                ..Default::default()
            };
            return Forwarded::Buffered {
                response: crate::error::AppError::Upstream(e.to_string()).into_response(),
                outcome,
            };
        }
    };
    let latency_ms = started.elapsed().as_millis() as i64;

    let (usage, tool_calls) = match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(json) => (
            extract::usage_from_response(&provider, &json),
            extract::tool_calls_from_response(&provider, &json),
        ),
        Err(_) => (extract::Usage::default(), Vec::new()),
    };
    let error_body = (status_u16 >= 400)
        .then(|| truncate_error(&bytes))
        .flatten();

    let outcome = UsageOutcome {
        status: status_u16,
        tokens_input: usage.input,
        tokens_output: usage.output,
        cache_creation_tokens: usage.cache_creation,
        cache_read_tokens: usage.cache_read,
        ccr_retrieve_calls: 0,
        latency_ms,
        error_body,
        tool_calls,
        // Buffered: the whole body arrives at once — all wall-clock is ttfb.
        ttfb_ms: Some(latency_ms),
        gen_ms: Some(0),
    };

    let builder = copy_resp_headers(Response::builder().status(status), &headers);
    let response = builder
        .body(Body::from(bytes))
        .unwrap_or_else(|_| status.into_response());
    Forwarded::Buffered { response, outcome }
}

/// Relay an SSE stream chunk-by-chunk, tallying usage on the fly. The outcome is
/// delivered through `outcome_rx` once the upstream stream terminates.
/// Build the usage outcome for a streamed response. A mid-stream upstream
/// error is recorded as a 502 with the error captured, so a truncated response
/// is excluded from the cost aggregates (which sum `status < 400`) and counted
/// among failures rather than booked as a clean success.
#[allow(clippy::too_many_arguments)]
fn stream_outcome(
    status_u16: u16,
    usage: crate::extract::Usage,
    tool_calls: Vec<crate::model::ToolCall>,
    stream_error: Option<String>,
    latency_ms: i64,
    ttfb_ms: Option<i64>,
    gen_ms: Option<i64>,
) -> UsageOutcome {
    let (status, error_body) = match stream_error {
        Some(e) => (502, Some(format!("upstream stream interrupted: {e}"))),
        None => (status_u16, None),
    };
    UsageOutcome {
        status,
        tokens_input: usage.input,
        tokens_output: usage.output,
        cache_creation_tokens: usage.cache_creation,
        cache_read_tokens: usage.cache_read,
        ccr_retrieve_calls: 0,
        latency_ms,
        error_body,
        tool_calls,
        ttfb_ms,
        gen_ms,
    }
}

fn stream_relay(resp: reqwest::Response, provider: Provider, started: Instant) -> Forwarded {
    let status = resp.status();
    let status_u16 = status.as_u16();
    let headers = resp.headers().clone();
    let has_ct = headers.get("content-type").is_some();

    let (tx, outcome_rx) = oneshot::channel::<UsageOutcome>();

    let body_stream = async_stream::stream! {
        let mut acc = SseAccumulator::new(provider);
        let mut upstream = resp.bytes_stream();
        // First non-empty upstream byte = time-to-first-byte boundary; everything
        // after it is generation (streamed token emission).
        let mut first_byte_at: Option<Instant> = None;
        // An upstream that cuts mid-stream would otherwise be recorded with the
        // 200 the header line carried and no error body — a truncated response
        // booked as a clean success. Capture the break so the usage row tells
        // the truth.
        let mut stream_error: Option<String> = None;
        while let Some(chunk) = upstream.next().await {
            match &chunk {
                Ok(bytes) => {
                    if first_byte_at.is_none() && !bytes.is_empty() {
                        first_byte_at = Some(Instant::now());
                    }
                    acc.feed(bytes);
                }
                Err(e) => stream_error = Some(e.to_string()),
            }
            yield chunk;
        }
        let tool_calls = acc.tool_calls();
        let usage = acc.finish();
        let ttfb_ms = first_byte_at.map(|t| t.duration_since(started).as_millis() as i64);
        let gen_ms = first_byte_at.map(|t| t.elapsed().as_millis() as i64);
        let _ = tx.send(stream_outcome(
            status_u16,
            usage,
            tool_calls,
            stream_error,
            started.elapsed().as_millis() as i64,
            ttfb_ms,
            gen_ms,
        ));
    };

    let mut builder = copy_resp_headers(Response::builder().status(status), &headers);
    if !has_ct {
        builder = builder.header("content-type", "text/event-stream");
    }
    let response = builder
        .body(Body::from_stream(body_stream))
        .unwrap_or_else(|_| status.into_response());
    Forwarded::Streaming {
        response,
        outcome_rx,
    }
}

/// Trim an upstream error body to [`MAX_ERROR_BODY`] on a char boundary.
pub fn truncate_error(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() > MAX_ERROR_BODY {
        let mut end = MAX_ERROR_BODY;
        while !trimmed.is_char_boundary(end) {
            end -= 1;
        }
        Some(format!("{}… (truncated)", &trimmed[..end]))
    } else {
        Some(trimmed.to_string())
    }
}

/// Accumulates a provider's SSE stream to recover the usage totals (and, best
/// effort, the completion text + tool calls) once the stream ends — without
/// altering a single relayed byte. Anthropic splits input/cache into
/// `message_start` and output into `message_delta`; OpenAI emits a final `usage`
/// chunk; native Gemini streams `usageMetadata`. RAW bytes are buffered and only
/// complete (newline-terminated) lines decoded, so a multi-byte UTF-8 char split
/// across a chunk boundary is never corrupted.
pub struct SseAccumulator {
    provider: Provider,
    buf: Vec<u8>,
    usage: extract::Usage,
    text: String,
    /// Streamed tool calls by content-block index: (name, accumulated input JSON).
    tool_buf: std::collections::BTreeMap<i64, (String, String)>,
}

impl SseAccumulator {
    pub fn new(provider: Provider) -> Self {
        Self {
            provider,
            buf: Vec::new(),
            usage: extract::Usage::default(),
            text: String::new(),
            tool_buf: std::collections::BTreeMap::new(),
        }
    }

    /// The tool calls reconstructed from the stream (`input` parsed from the
    /// accumulated `input_json_delta` fragments).
    pub fn tool_calls(&self) -> Vec<ToolCall> {
        self.tool_buf
            .values()
            .map(|(name, input_json)| ToolCall {
                name: name.clone(),
                input: serde_json::from_str(input_json).unwrap_or(serde_json::Value::Null),
            })
            .collect()
    }

    /// Feed a raw chunk; parses any complete `data:` lines for usage fields.
    pub fn feed(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
        // Detach every complete line (up to the last newline) in ONE front-drain,
        // then iterate its lines by slicing — no per-line Vec/String allocation and
        // no repeated front-drain memmove. The trailing partial line stays in buf.
        let Some(last_nl) = self.buf.iter().rposition(|&b| b == b'\n') else {
            return;
        };
        let complete: Vec<u8> = self.buf.drain(..=last_nl).collect();
        for line in complete.split(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(line);
            self.feed_line(line.trim());
        }
    }

    /// Merge a freshly parsed usage object, keeping the latest non-zero values.
    fn merge(&mut self, usage: &serde_json::Value) {
        let u = extract::usage_from_obj(&self.provider, usage);
        if u.input > 0 {
            self.usage.input = u.input;
        }
        if u.output > 0 {
            self.usage.output = u.output;
        }
        if u.cache_creation > 0 {
            self.usage.cache_creation = u.cache_creation;
        }
        if u.cache_read > 0 {
            self.usage.cache_read = u.cache_read;
        }
    }

    fn feed_line(&mut self, line: &str) {
        let data = match line.strip_prefix("data:") {
            Some(d) => d.trim(),
            None => return,
        };
        if data == "[DONE]" || data.is_empty() {
            return;
        }
        let json: serde_json::Value = match serde_json::from_str(data) {
            Ok(j) => j,
            Err(_) => return,
        };
        // Anthropic `message_start` nests usage under `message`.
        if let Some(usage) = json.get("message").and_then(|m| m.get("usage")) {
            self.merge(usage);
        }
        if let Some(usage) = json.get("usage").filter(|u| !u.is_null()) {
            self.merge(usage);
        }
        // Native Gemini streams usage under `usageMetadata`.
        if let Some(usage) = json.get("usageMetadata").filter(|u| !u.is_null()) {
            self.merge(usage);
        }
        self.feed_text(&json);
        self.feed_tool_use(&json);
    }

    /// Reconstruct streamed tool calls across the three dialects.
    fn feed_tool_use(&mut self, json: &serde_json::Value) {
        // Native Gemini streams a whole functionCall (name + args) per chunk.
        if matches!(self.provider, Provider::Google) {
            if let Some(parts) = json
                .pointer("/candidates/0/content/parts")
                .and_then(|p| p.as_array())
            {
                for p in parts {
                    if let Some(fc) = p.get("functionCall") {
                        if let Some(name) = fc.get("name").and_then(|n| n.as_str()) {
                            let args = fc
                                .get("args")
                                .map(|a| a.to_string())
                                .unwrap_or_else(|| "null".into());
                            let idx = self.tool_buf.len() as i64;
                            self.tool_buf.insert(idx, (name.to_string(), args));
                        }
                    }
                }
            }
            return;
        }
        // OpenAI streams tool calls as choices[].delta.tool_calls[] with a stable
        // `index`, a `function.name` (first chunk) and `function.arguments` frags.
        if matches!(self.provider, Provider::Openai) {
            if let Some(choices) = json.get("choices").and_then(|c| c.as_array()) {
                for ch in choices {
                    if let Some(tcs) = ch.pointer("/delta/tool_calls").and_then(|t| t.as_array()) {
                        for tc in tcs {
                            let idx = tc.get("index").and_then(|i| i.as_i64()).unwrap_or(0);
                            let entry = self
                                .tool_buf
                                .entry(idx)
                                .or_insert_with(|| (String::new(), String::new()));
                            if let Some(name) =
                                tc.pointer("/function/name").and_then(|n| n.as_str())
                            {
                                if !name.is_empty() {
                                    entry.0 = name.to_string();
                                }
                            }
                            if let Some(frag) =
                                tc.pointer("/function/arguments").and_then(|a| a.as_str())
                            {
                                entry.1.push_str(frag);
                            }
                        }
                    }
                }
            }
            return;
        }
        if !matches!(self.provider, Provider::Anthropic) {
            return;
        }
        let index = json.get("index").and_then(|i| i.as_i64()).unwrap_or(0);
        match json.get("type").and_then(|t| t.as_str()) {
            Some("content_block_start") => {
                let block = json.get("content_block");
                if block.and_then(|b| b.get("type")).and_then(|t| t.as_str()) == Some("tool_use") {
                    let name = block
                        .and_then(|b| b.get("name"))
                        .and_then(|n| n.as_str())
                        .unwrap_or_default()
                        .to_string();
                    self.tool_buf.insert(index, (name, String::new()));
                }
            }
            Some("content_block_delta")
                if json.pointer("/delta/type").and_then(|t| t.as_str())
                    == Some("input_json_delta") =>
            {
                if let Some(frag) = json.pointer("/delta/partial_json").and_then(|p| p.as_str()) {
                    if let Some((_, buf)) = self.tool_buf.get_mut(&index) {
                        buf.push_str(frag);
                    }
                }
            }
            _ => {}
        }
    }

    /// Append any assistant text carried by this SSE event.
    fn feed_text(&mut self, json: &serde_json::Value) {
        match self.provider {
            Provider::Anthropic => {
                if json.get("type").and_then(|t| t.as_str()) == Some("content_block_delta") {
                    if let Some(t) = json
                        .get("delta")
                        .and_then(|d| d.get("text"))
                        .and_then(|t| t.as_str())
                    {
                        self.text.push_str(t);
                    }
                }
            }
            Provider::Google => {
                if let Some(parts) = json
                    .pointer("/candidates/0/content/parts")
                    .and_then(|p| p.as_array())
                {
                    for p in parts {
                        if let Some(t) = p.get("text").and_then(|t| t.as_str()) {
                            self.text.push_str(t);
                        }
                    }
                }
            }
            Provider::Openai => {
                if let Some(choices) = json.get("choices").and_then(|c| c.as_array()) {
                    for ch in choices {
                        if let Some(t) = ch
                            .get("delta")
                            .and_then(|d| d.get("content"))
                            .and_then(|t| t.as_str())
                        {
                            self.text.push_str(t);
                        }
                    }
                }
            }
        }
    }

    pub fn finish(self) -> extract::Usage {
        self.usage
    }
}

#[cfg(test)]
mod stream_outcome {
    use super::*;

    /// An upstream that cuts mid-stream must not be booked as a clean
    /// success. The outcome builder records a truncated stream as 502 with the
    /// error captured, so the cost aggregates (status < 400) exclude it and it
    /// counts among failures.
    #[test]
    fn interrupted_stream_is_not_recorded_as_success() {
        let usage = crate::extract::Usage {
            input: 100,
            output: 20,
            cache_creation: 0,
            cache_read: 0,
        };
        let clean = stream_outcome(200, usage, vec![], None, 10, Some(1), Some(9));
        assert_eq!(clean.status, 200);
        assert!(clean.error_body.is_none());

        let cut = stream_outcome(
            200,
            usage,
            vec![],
            Some("connection reset".into()),
            10,
            Some(1),
            Some(9),
        );
        assert!(cut.status >= 400, "a truncated stream must not record 2xx");
        assert!(
            cut.error_body
                .as_deref()
                .unwrap_or("")
                .contains("interrupted"),
            "the break must be captured: {:?}",
            cut.error_body
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    /// A config with recording and CCR's cache both off-disk. `ccr` selects
    /// whether the one compressing stage is armed.
    fn cfg(ccr: bool) -> Config {
        Config {
            cache_align: true,
            normalize: true,
            engine_methods: false,
            bind: "127.0.0.1:0".into(),
            anthropic_upstream: "https://api.anthropic.com".into(),
            openai_upstream: "https://api.openai.com".into(),
            ccr,
            compression: "off".into(),
            pixel_compress: Default::default(),
            relay_borrowing: true,
            db: ":memory:".into(),
            icm_db: ":memory:".into(),
            icm_ttl_secs: 0,
            upstream_timeout_secs: 0,
            upstream_retries: 0,
        }
    }

    /// A big tool_result payload — over CCR's compression threshold.
    fn big_anthropic_body() -> Bytes {
        let big = (0..4000)
            .map(|i| format!("line {i} of command output\n"))
            .collect::<String>();
        let body = serde_json::json!({
            "model": "claude-haiku-4-5",
            "max_tokens": 100,
            "messages": [
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "t1", "content": big }
                ]}
            ]
        });
        Bytes::from(serde_json::to_vec(&body).unwrap())
    }

    #[tokio::test]
    async fn ccr_off_is_passthrough() {
        let st = AppState::new(cfg(false));
        let body = big_anthropic_body();
        let applied = apply(
            &st,
            Provider::Anthropic,
            "/v1/messages",
            &HeaderMap::new(),
            body.clone(),
        )
        .await;
        assert_eq!(applied.body, body, "ccr off must not touch the body");
        assert_eq!(applied.tokens_saved, 0);
        assert!(applied.saved_by_stage.is_empty());
        assert!(!applied.ccr_active);
    }

    #[tokio::test]
    async fn ccr_on_compresses_and_attributes_the_saving() {
        // `icm_db: ":memory:"` gives CCR a real (ephemeral) cache, so the
        // compress step runs end to end rather than no-opping.
        let st = AppState::new(cfg(true));
        let body = big_anthropic_body();
        let applied = apply(
            &st,
            Provider::Anthropic,
            "/v1/messages",
            &HeaderMap::new(),
            body.clone(),
        )
        .await;

        assert!(
            applied.body.len() < body.len(),
            "ccr must shrink an oversized tool_result: {} -> {}",
            body.len(),
            applied.body.len()
        );
        assert!(applied.ccr_active, "the ccr forward loop must be armed");
        // The aggregate and the per-stage breakdown agree, and the saving is
        // attributed to ccr by name -- not just added to the total.
        assert!(applied.tokens_saved > 0);
        assert_eq!(
            applied.saved_by_stage,
            vec![(crate::pipeline::ccr::STAGE_NAME, applied.tokens_saved)]
        );
    }

    /// A tool-result-only request carries no user *text*, so counting only
    /// the user turn would make `tokens_in` 0 and every saving read as
    /// "100%". The estimate covers the whole forwarded body, so the ratio
    /// stays truthful.
    #[tokio::test]
    async fn tokens_in_counts_the_whole_body_not_just_the_user_turn() {
        let st = AppState::new(cfg(false));
        let applied = apply(
            &st,
            Provider::Anthropic,
            "/v1/messages",
            &HeaderMap::new(),
            big_anthropic_body(),
        )
        .await;
        assert!(
            applied.tokens_in > 1000,
            "a 4000-line tool_result must not estimate as ~0 input tokens, got {}",
            applied.tokens_in
        );
        assert_eq!(applied.tokens_in, applied.ctx.total_tokens);
    }

    #[tokio::test]
    async fn apply_detects_streaming_request() {
        let st = AppState::new(cfg(false));
        let body = Bytes::from(
            r#"{"model":"claude-haiku-4-5","max_tokens":5,"stream":true,"messages":[{"role":"user","content":"hi"}]}"#,
        );
        let applied = apply(
            &st,
            Provider::Anthropic,
            "/v1/messages",
            &HeaderMap::new(),
            body,
        )
        .await;
        assert!(
            applied.stream,
            "stream:true must be detected for the SSE relay"
        );
    }

    #[test]
    fn proxy_internal_headers_never_go_upstream() {
        for internal in ["x-rtk-session", "x-rtk-project", "X-RTK-Session"] {
            assert!(is_proxy_internal(internal), "{internal}");
            assert!(!forwardable_to_upstream(internal), "{internal}");
        }
        for allowed in [
            "authorization",
            "x-api-key",
            "anthropic-version",
            "content-type",
        ] {
            assert!(!is_proxy_internal(allowed), "{allowed}");
            assert!(forwardable_to_upstream(allowed), "{allowed}");
        }
        // A bare "x-rtk-" prefix with no name is not a real header; the length
        // guard keeps the predicate from matching it.
        assert!(!is_proxy_internal("x-rtk-"));
        assert!(!forwardable_to_upstream("content-length"));
        assert!(!forwardable_to_upstream("host"));
    }

    #[test]
    fn sse_tally_anthropic_split_across_start_and_delta() {
        let mut acc = SseAccumulator::new(Provider::Anthropic);
        acc.feed(b"data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":120,\"output_tokens\":1,\"cache_creation_input_tokens\":30,\"cache_read_input_tokens\":900}}}\n\n");
        acc.feed(b"data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n");
        acc.feed(b"data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":88}}\n\n");
        acc.feed(b"data: [DONE]\n\n");
        let u = acc.finish();
        assert_eq!(u.input, 120);
        assert_eq!(u.output, 88, "latest non-zero output wins");
        assert_eq!(u.cache_creation, 30);
        assert_eq!(u.cache_read, 900);
    }

    #[test]
    fn sse_tally_openai_final_usage_chunk() {
        let mut acc = SseAccumulator::new(Provider::Openai);
        acc.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}],\"usage\":null}\n\n");
        acc.feed(
            b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":42,\"completion_tokens\":17}}\n\n",
        );
        acc.feed(b"data: [DONE]\n\n");
        let u = acc.finish();
        assert_eq!(u.input, 42);
        assert_eq!(u.output, 17);
    }

    #[test]
    fn sse_tally_survives_a_chunk_split_mid_line() {
        // A `data:` line split across two feeds must still parse (raw-byte buffering).
        let mut acc = SseAccumulator::new(Provider::Anthropic);
        acc.feed(b"data: {\"type\":\"message_delta\",\"usage\":{\"out");
        acc.feed(b"put_tokens\":55}}\n\n");
        assert_eq!(acc.finish().output, 55);
    }
}
