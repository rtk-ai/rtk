//! Shared domain model for intercepted traffic.
//!
//! This is the single source of truth used by the proxy handlers, the
//! transformation pipeline, the storage layer and the statistics layer.

use serde::{Deserialize, Serialize};

/// Which upstream API dialect an intercepted request speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// Anthropic Messages API (`/v1/messages`) — Claude Code talks this.
    Anthropic,
    /// OpenAI Chat Completions API (`/v1/chat/completions`) — OpenCode and others.
    Openai,
    /// Google Gemini API (`generateContent`) — reached only via cross-provider
    /// translation (there is no native Gemini inbound handler for the offload
    /// direction; a Gemini client is served on a mapped path).
    Google,
}

impl Provider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::Openai => "openai",
            Provider::Google => "google",
        }
    }
}

/// A request intercepted from a client (Claude Code, OpenCode, …), parsed into
/// a uniform shape the pipeline can reason about. The original JSON body is
/// preserved in [`InterceptedRequest::body`] so transforms can mutate it and we
/// can forward a faithful copy upstream.
#[derive(Debug, Clone)]
pub struct InterceptedRequest {
    pub provider: Provider,
    /// Upstream path to forward to, e.g. `/v1/messages`.
    pub path: String,
    /// Model requested by the client.
    pub model: String,
    /// Whether the client asked for a streaming (SSE) response.
    pub stream: bool,
    /// Detected client identity (`claude-code`, `opencode`, …), best-effort.
    pub client: Option<String>,
    /// Logical project, surfaced via header `x-rtk-project` when present.
    pub project: Option<String>,
    /// Logical session, surfaced via header `x-rtk-session` when present.
    pub session: Option<String>,
    /// SHA-256 (hex, truncated) of the upstream API key, for per-key stats.
    pub api_key_hash: Option<String>,
    /// Tokens removed by compression transforms this request (accumulated as the
    /// pipeline runs). Surfaced in stats/charts.
    pub tokens_saved: i64,
    /// Per-stage attribution of `tokens_saved`. A transform that removes
    /// tokens appends `(Transform::name(), removed)` here as well as adding
    /// to `tokens_saved`, so the usage store can break the aggregate down by
    /// which lever did the work. This is the extension point: a downstream
    /// stage attributes its own savings without any change here.
    pub saved_by_stage: Vec<(&'static str, i64)>,
    /// Original model when complexity-routing downgraded it (else `None`).
    pub routed_from: Option<String>,
    /// What kind of request this is, when the client says so.
    ///
    /// Claude Code sends `x-claude-code-request-class` (`main`, `subagent`,
    /// `workflow`, `compaction`, `auxiliary`) behind
    /// `CLAUDE_CODE_GATEWAY_HINT_HEADERS=1`, documented as a fact "a gateway or
    /// router can use". It is the signal model routing needs: a `main` request
    /// carries the cached conversation, the others do not.
    pub request_class: Option<String>,
    /// Logical user: header `x-rtk-user` when present, else the API key hash.
    /// Scopes memory, stats and request attribution per user.
    pub user_id: Option<String>,
    /// EXPERIMENTAL CCR metrics for this request (all zero unless CCR compressed
    /// at least one block). Carried here so the recording step can persist them.
    pub ccr: crate::model::CcrMetrics,
    /// EXPERIMENTAL CCR: number of `rtk_retrieve` calls the proxy resolved
    /// internally while forwarding this request (0 unless the model retrieved).
    pub ccr_retrieve_calls: i64,
    /// The parsed JSON request body (mutable by the pipeline).
    pub body: serde_json::Value,
    /// USD reserved against the caller's budget at request start (0.0 when
    /// budgeting is off). record() reconciles the actual cost against it.
    pub budget_reserved: f64,
    /// USD reserved against the caller's TEAM pool(s) at request start (0.0 when
    /// no team budget applies). record() reconciles the actual cost against it.
    pub team_budget_reserved: f64,
    /// Timestamp of the team-pool reservation. record() MUST reconcile against
    /// this instant, not a fresh `now()`, so the +estimate and its −(estimate −
    /// actual) land in the SAME window bucket even when a long/streaming request
    /// straddles a Day/Week/Month UTC boundary. `None` when no team reservation
    /// was taken.
    pub team_budget_reserved_at: Option<chrono::DateTime<chrono::Utc>>,
    /// LATENCY SPAN — ms spent in the request-phase transform pipeline, set right
    /// after `pipeline.run_request` so the recording step can persist it. 0 until
    /// measured. See [`UsageRecord::pipeline_ms`].
    pub pipeline_ms: i64,
}

impl InterceptedRequest {
    /// Record `removed` tokens saved by stage `name`, keeping the aggregate
    /// and the per-stage breakdown in step. A no-op for a zero/negative
    /// count, so a stage can call it unconditionally.
    pub fn record_saving(&mut self, name: &'static str, removed: i64) {
        if removed <= 0 {
            return;
        }
        self.tokens_saved += removed;
        match self.saved_by_stage.iter_mut().find(|(n, _)| *n == name) {
            Some((_, acc)) => *acc += removed,
            None => self.saved_by_stage.push((name, removed)),
        }
    }

    /// Tokens attributed to one stage, 0 if it never ran or saved nothing.
    pub fn saved_by(&self, name: &str) -> i64 {
        self.saved_by_stage
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v)
            .unwrap_or(0)
    }

    /// Build a request for the forward's pipeline: `provider`/`path`/`body`
    /// are given, `model`/`stream` are derived from the body, and every other
    /// field (stats, budget, CCR) defaults to zero/None. The daemon uses this
    /// to run its transforms before forwarding; a downstream binary may build
    /// the fuller request itself.
    /// [`Self::for_pipeline`] plus the caller-supplied identity headers.
    ///
    /// `x-rtk-session` scopes CCR's cache (`ccr/{session}/{id}`) and
    /// `x-rtk-project` attributes the usage row. Both are optional: a client
    /// that sends neither still works, it just shares the `default` scope.
    pub fn for_pipeline_with_headers(
        provider: Provider,
        path: String,
        body: serde_json::Value,
        headers: &axum::http::HeaderMap,
    ) -> Self {
        let header = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.trim())
                .filter(|v| !v.is_empty())
                .map(|v| v.to_string())
        };
        Self {
            session: header("x-rtk-session"),
            project: header("x-rtk-project"),
            request_class: header("x-claude-code-request-class"),
            client: crate::extract::detect_client(header("user-agent").as_deref()),
            ..Self::for_pipeline(provider, path, body)
        }
    }

    pub fn for_pipeline(provider: Provider, path: String, body: serde_json::Value) -> Self {
        let model = crate::extract::model(&body);
        let stream = crate::extract::stream(&body);
        Self {
            provider,
            path,
            model,
            stream,
            body,
            client: None,
            project: None,
            session: None,
            api_key_hash: None,
            tokens_saved: 0,
            saved_by_stage: Vec::new(),
            routed_from: None,
            request_class: None,
            user_id: None,
            ccr: CcrMetrics::default(),
            ccr_retrieve_calls: 0,
            budget_reserved: 0.0,
            team_budget_reserved: 0.0,
            team_budget_reserved_at: None,
            pipeline_ms: 0,
        }
    }
}

/// A tool invocation the assistant emitted (Anthropic `tool_use` / OpenAI
/// `tool_calls`), captured for the logs view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub input: serde_json::Value,
}

/// The measurable outcome of a forwarded request, used for stats + memory.
#[derive(Debug, Clone, Default)]
pub struct UsageOutcome {
    pub status: u16,
    pub tokens_input: i64,
    pub tokens_output: i64,
    /// Tokens written to the provider prompt cache (Anthropic).
    pub cache_creation_tokens: i64,
    /// Tokens served from the provider prompt cache.
    pub cache_read_tokens: i64,
    /// `rtk_retrieve` calls the proxy resolved for this request -- how often
    /// the model had to go back for content CCR removed.
    pub ccr_retrieve_calls: i64,
    pub latency_ms: i64,
    /// Upstream error body (truncated), captured when `status >= 400`.
    pub error_body: Option<String>,
    /// Tool calls the assistant emitted in the response.
    pub tool_calls: Vec<ToolCall>,
    /// LATENCY SPAN — time-to-first-byte from the upstream: the send→first
    /// response byte gap (the model "thinking" / queue). `None` when not
    /// measured. For a buffered (non-stream) response this equals `latency_ms`
    /// (the whole body arrives at once, so there is no first/last split).
    pub ttfb_ms: Option<i64>,
    /// LATENCY SPAN — generation time: first→last upstream byte (streaming token
    /// emission). `None` when not measured; `Some(0)` for a buffered response.
    pub gen_ms: Option<i64>,
}

/// A persisted usage record. Mirrors the `usage_records` table.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct UsageRecord {
    pub id: String,
    /// RFC3339 timestamp.
    pub ts: String,
    pub provider: String,
    pub path: String,
    pub model: String,
    pub stream: bool,
    pub status: i64,
    pub tokens_input: i64,
    pub tokens_output: i64,
    /// Our pre-send estimate of the input tokens (tiktoken `o200k`), recorded so
    /// a consumer can compare *predicted* vs the provider's *actual*
    /// `tokens_input` and surface estimation accuracy. 0 when not estimated.
    pub est_tokens_input: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub latency_ms: i64,
    pub client: Option<String>,
    pub project: Option<String>,
    pub session: Option<String>,
    pub api_key_hash: Option<String>,
    /// Upstream error body (truncated), present when `status >= 400`.
    #[serde(default)]
    pub error_body: Option<String>,
    /// Which backend served the request: `anthropic` (default) or `gpu`.
    #[serde(default)]
    pub backend: Option<String>,
    /// Logical user (header `x-rtk-user` or API key hash), for per-user analytics.
    #[serde(default)]
    pub user_id: Option<String>,
    /// Whether the request carried (non-empty) tool definitions.
    #[serde(default)]
    pub has_tools: bool,
    /// Token count of the request's `tools` block (0 when none).
    #[serde(default)]
    pub tools_tokens: i64,
    /// JSON of the agentic tool activity: assistant tool calls (name+input) and
    /// the tool results (command outputs) from the request. Truncated; present
    /// only when body logging is enabled.
    #[serde(default)]
    pub tool_activity: Option<String>,
    /// Tokens removed by compression transforms on this request.
    #[serde(default)]
    pub tokens_saved: i64,
    /// Original model when complexity-routing downgraded it.
    #[serde(default)]
    pub routed_from: Option<String>,
    /// Estimated cost saved by the downgrade, in micro-USD.
    #[serde(default)]
    pub cost_saved_micro_usd: i64,
    /// tool_result blocks in this request whose output already carries an rtk
    /// truncation marker (`… +N more`), i.e. command output the rtk CLI filtered
    /// before the client sent it back.
    #[serde(default)]
    pub rtk_rewrites: i64,
    /// Total tool_result blocks in this request (the denominator for rtk
    /// command-output coverage).
    #[serde(default)]
    pub tool_results_total: i64,
    /// Estimated total context tokens (system + tools + messages), byte/4 scale.
    #[serde(default)]
    pub ctx_total_tokens: i64,
    /// Estimated tokens inside `tool_result` blocks (ingest-filter candidate).
    #[serde(default)]
    pub ctx_tool_result_tokens: i64,
    /// Estimated tokens inside `tool_use` input string values (ingest-filter
    /// candidate: file bodies echoed by agentic clients).
    #[serde(default)]
    pub ctx_tool_use_input_tokens: i64,
    /// Estimated tokens that exactly repeat an earlier block (dedup candidate).
    #[serde(default)]
    pub ctx_duplicate_tokens: i64,
    /// Estimated tokens for image/document blocks (flat per-block allowance).
    #[serde(default)]
    pub ctx_image_tokens: i64,
    /// SHADOW per-tier cost bound: estimated micro-USD at the model actually
    /// used (cache-aware). Observe-only; never affects the model chosen.
    #[serde(default)]
    pub shadow_cost_actual_micro: i64,
    /// SHADOW per-tier cost bound: estimated micro-USD if this request had run
    /// on Sonnet (cache-break applied on a tier change).
    #[serde(default)]
    pub shadow_cost_sonnet_micro: i64,
    /// SHADOW per-tier cost bound: estimated micro-USD if this request had run
    /// on Haiku (cache-break applied on a tier change).
    #[serde(default)]
    pub shadow_cost_haiku_micro: i64,
    /// SHADOW selective policy: tier a smart (recalibrated) router would have
    /// downgraded this request to, or `None` to keep the actual model.
    #[serde(default)]
    pub shadow_routed_to: Option<String>,
    /// SHADOW selective policy: estimated micro-USD saved if that downgrade had
    /// been applied (cache-break included). Zero when no downgrade.
    #[serde(default)]
    pub shadow_saved_micro: i64,
    /// SHADOW local routing (EXPERIMENTAL, observe-only): whether this request's
    /// context is small enough for a local GPU model (`ctx <= threshold`).
    #[serde(default)]
    pub shadow_local_eligible: bool,
    /// SHADOW local routing: estimated micro-USD that would move to a ~free local
    /// model if this request ran locally. Zero when not eligible.
    #[serde(default)]
    pub shadow_local_saved_micro: i64,
    /// SHADOW multi-provider routing (EXPERIMENTAL, observe-only): cheapest
    /// cross-provider cost in micro-USD, with a full cache-break.
    #[serde(default)]
    pub shadow_multiprovider_best_micro: i64,
    /// SHADOW multi-provider routing: provider label of the cheapest option
    /// (e.g. `"openai"`), or `None`.
    #[serde(default)]
    pub shadow_multiprovider_provider: Option<String>,
    /// CCR (EXPERIMENTAL): original tokens moved into the reversible cache this
    /// request (what was removed from context before the preview replaced it).
    #[serde(default)]
    pub ccr_tokens_compressed: i64,
    /// CCR (EXPERIMENTAL): tokens of preview+marker left in context in their place.
    #[serde(default)]
    pub ccr_tokens_in_context: i64,
    /// CCR (EXPERIMENTAL): number of tool_result blocks compressed this request.
    #[serde(default)]
    pub ccr_blocks_compressed: i64,
    /// CCR (EXPERIMENTAL): number of `rtk_retrieve` calls the proxy resolved
    /// internally for this request (how often the model needed the original).
    #[serde(default)]
    pub ccr_retrieve_calls: i64,
    /// LATENCY SPAN — time (ms) spent in the request-phase transform pipeline
    /// (cache_align→ccr, plus any downstream stages) before the upstream send. The "how much does rtk add" number. 0 = unknown.
    #[serde(default)]
    pub pipeline_ms: i64,
    /// LATENCY SPAN — upstream time-to-first-byte (send→first response byte), the
    /// model "thinking" / queue. 0 = unknown. See [`UsageOutcome::ttfb_ms`].
    #[serde(default)]
    pub ttfb_ms: i64,
    /// LATENCY SPAN — upstream generation time (first→last byte, streaming). For a
    /// buffered response this is 0 (no split). 0 = unknown/none.
    #[serde(default)]
    pub gen_ms: i64,
}

/// CCR (Compress-Cache-Retrieve) per-request metrics, recorded on the usage row.
/// A plain data type (only these counters are part of the data model, not
/// the compression logic).
/// All zero when CCR is off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CcrMetrics {
    /// Total tokens of ORIGINAL content moved into the cache (what was removed
    /// from context, before the preview was substituted).
    pub tokens_compressed: i64,
    /// Total tokens of the preview+marker text left in the context in their place.
    pub tokens_in_context: i64,
    /// Number of `tool_result` blocks compressed this request.
    pub blocks_compressed: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn for_pipeline_with_headers_detects_the_client() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("user-agent", "claude-cli/2.0 (external)".parse().unwrap());
        let req = InterceptedRequest::for_pipeline_with_headers(
            Provider::Anthropic,
            "/v1/messages".to_string(),
            serde_json::json!({}),
            &headers,
        );
        assert_eq!(req.client.as_deref(), Some("claude-code"));

        let req = InterceptedRequest::for_pipeline_with_headers(
            Provider::Anthropic,
            "/v1/messages".to_string(),
            serde_json::json!({}),
            &axum::http::HeaderMap::new(),
        );
        assert_eq!(req.client, None);
    }
}
