//! CCR — Compress-Cache-Retrieve: a **reversible** context-compression lever.
//!
//! The proxy's one compression lever, and the reason it is worth running at
//! all. Unlike a truncating filter, which drops content permanently, CCR
//! keeps what it removes **retrievable**:
//!
//!   1. **Compress** — on the request, each oversized tool-output text block
//!      is replaced by a short head+tail preview plus a marker naming a
//!      content-hash `id`. Pure function of the block's own content, so the
//!      prefix the provider sees stays byte-stable across turns.
//!   2. **Cache** — the original full content is stored in ICM under a
//!      session-scoped topic (`ccr/{session}/{id}`). A distinct `ccr/`
//!      prefix keeps it out of keyword recall.
//!   3. **Retrieve** — an `rtk_retrieve` tool is injected into the request.
//!      When the model calls it, the proxy resolves it itself from the
//!      cache (see `crate::ccr_forward`) and never surfaces the tool to the
//!      client.
//!
//! Two dialects, model-agnostic: Anthropic ([`anthropic`], `tool_use`/
//! `tool_result` content blocks) and OpenAI ([`openai`], `tool_calls`/
//! `role: "tool"` messages) each implement the same shape independently —
//! the wire formats differ too much to share more than the pure
//! `compress_text`/cache logic below. Google/Gemini is not covered (no
//! native inbound handler exists for it in this proxy at all, see
//! `model::Provider`'s doc comment).
//!
//! There is no tenant scoping (`api_key_hash`) here: this proxy is a single
//! local process for one operator, not a multi-tenant service. Only
//! `{session}/` scoping remains — several concurrent Claude Code sessions
//! against the same local proxy is a real scenario.

pub mod anthropic;
pub mod content;
pub mod openai;
pub mod profile;
pub mod strategy;

use crate::memory::IcmMemory;
use crate::model::{CcrMetrics, InterceptedRequest, Provider};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Runtime-mutable on/off flag for CCR, shared into `AppState`.
pub type SharedFlag = Arc<RwLock<bool>>;

/// This stage's [`super::Transform::name`], also the key its savings are
/// attributed under in `InterceptedRequest::saved_by_stage` and the
/// `saved_ccr` usage column.
pub const STAGE_NAME: &str = "ccr";

/// Tool name the proxy injects and intercepts. Never seen by the client.
pub const RETRIEVE_TOOL_NAME: &str = "rtk_retrieve";

/// Stable substring present in every CCR compression marker. Doubles as the
/// idempotency guard: a block already carrying it is never re-compressed.
pub const CCR_MARKER_TAG: &str = "rtk-proxy: compressed";

/// Token threshold above which a tool-output block is compressed. Below the
/// floor a block is left verbatim (the marker + preview would not save
/// enough to bother).
///
/// Boot-time overridable via `RTK_PROXY_CCR_MIN_TOKENS`, same
/// read-once-and-freeze discipline as [`retire_min_tokens`]: the floor decides
/// how every historical block renders, so changing it mid-session re-renders
/// the history and invalidates the upstream prefix.
///
/// A floor this high leaves typical agent tool results alone. Below ~300 the
/// head+tail preview costs as much as the block.
pub const COMPRESS_MIN_TOKENS: usize = 1500;

/// Floors resolved from the pipeline's `proxy.request` rules at boot
/// (`ccr-bulk` / `ccr` / `retire-writes` `tokens_gt`). Set once before the
/// first request; the `RTK_PROXY_CCR_*` environment variables still win.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConfiguredFloors {
    pub bulk: Option<usize>,
    pub other: Option<usize>,
    pub retire: Option<usize>,
}

static CONFIGURED_FLOORS: std::sync::OnceLock<ConfiguredFloors> = std::sync::OnceLock::new();

pub fn configure_floors(floors: ConfiguredFloors) {
    let _ = CONFIGURED_FLOORS.set(floors);
}

fn configured_floors() -> ConfiguredFloors {
    CONFIGURED_FLOORS.get().copied().unwrap_or_default()
}

/// Floor for BULK tool output — `Bash`, `WebFetch`, task summaries — whose
/// recovery is one cheap re-run. `RTK_PROXY_CCR_BULK_MIN_TOKENS`, else the
/// `ccr-bulk` rule's `tokens_gt`, default 300.
pub fn bulk_min_tokens() -> usize {
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("RTK_PROXY_CCR_BULK_MIN_TOKENS")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|n| *n >= 100)
            .or_else(|| configured_floors().bulk.filter(|n| *n >= 100))
            .unwrap_or(300)
    })
}

/// Floor for `Read` output. `usize::MAX` by default — never compressed.
///
/// A `Read` is the agent asking for exact file bytes, and head+tail elision
/// removes the middle without giving it back. The agent then pages through
/// the file to reconstruct the elided middle, each recovery read adds a turn,
/// and session cost grows with turns faster than per-turn compression saves.
///
/// Size does not predict this; recovery cost does.
///
/// `RTK_PROXY_CCR_READ_MIN_TOKENS` re-enables it.
pub fn read_min_tokens() -> usize {
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("RTK_PROXY_CCR_READ_MIN_TOKENS")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|n| *n >= 100)
            .unwrap_or(usize::MAX)
    })
}

/// The floor for a tool_result, chosen by the tool that produced it. Unknown
/// tools get the conservative [`compress_min_tokens`] rather than the bulk
/// floor: an unrecognised tool's recovery cost is unknown.
pub fn floor_for_tool(tool: Option<&str>) -> usize {
    match tool.map(profile::tool_capability) {
        Some(profile::Capability::ReadFile) => read_min_tokens(),
        Some(profile::Capability::Bulk) => bulk_min_tokens(),
        _ => compress_min_tokens(),
    }
}

/// Effective compression floor: `RTK_PROXY_CCR_MIN_TOKENS` when set and >= 100
/// (below that the marker+preview overhead swamps the saving), else
/// [`COMPRESS_MIN_TOKENS`].
pub fn compress_min_tokens() -> usize {
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("RTK_PROXY_CCR_MIN_TOKENS")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|n| *n >= 100)
            .or_else(|| configured_floors().other.filter(|n| *n >= 100))
            .unwrap_or(COMPRESS_MIN_TOKENS)
    })
}

/// Token floor for RETIRING a file body written through a tool argument
/// (`Write.content`, `Edit.new_string`, …). Separate from
/// [`COMPRESS_MIN_TOKENS`] because the trade is different: the body is on
/// disk, so recovery is the agent's own `Read` rather than a preview, and the
/// replacement is a pointer rather than head+tail.
///
/// Boot-time only, via `RTK_PROXY_RETIRE_MIN_TOKENS`. It must not change
/// mid-session: the floor decides how every historical block renders, so
/// moving it re-renders the history and invalidates the upstream prefix
/// exactly once — the same failure [`ensure_retrieve_tool`] exists to avoid.
pub fn retire_min_tokens() -> usize {
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("RTK_PROXY_RETIRE_MIN_TOKENS")
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .or(configured_floors().retire)
            .unwrap_or(400)
    })
}

/// Trailing messages left verbatim, newest first. `0` (the default) compresses
/// every message, which is what makes every decision a pure function of block
/// content and therefore byte-stable across turns.
///
/// Above `0` a block renders verbatim while it is inside the window and
/// compressed once it leaves, so it costs ONE bounded tail invalidation as it
/// ages out — the only deliberate exception to the byte-stability invariant in
/// this module. It buys the model seeing its own newest tool output intact.
/// That is a token cost for an unproven quality gain, so it is opt-in and off
/// by default: set `RTK_PROXY_KEEP_RECENT_TURNS` to enable it.
pub fn keep_recent_turns() -> usize {
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("RTK_PROXY_KEEP_RECENT_TURNS")
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    })
}

/// Retrieves of the SAME block before it is pinned — exempted from
/// compression for the remainder of the session.
/// `RTK_PROXY_CCR_PIN_AFTER_RETRIEVES`, default 2. `0` disables pinning.
///
/// Why pinning is needed at all: a retrieve round is resolved INSIDE one
/// client request — [`crate::ccr_forward`] appends the resolved content to its
/// own local body, never to the client's conversation — so the expansion is
/// ephemeral. Next turn the client resends its own history with the marker
/// intact, and a model that still needs the block asks again. The cost is not
/// a bounded one-off; it RECURS every turn, one extra full upstream round-trip
/// each time. [`MAX_ROUNDS`] bounds the loop within a request and does nothing
/// about this.
///
/// Pinning pays for that with ONE prefix invalidation: the block stops
/// rendering as a marker and renders verbatim, so the cached tokens after its
/// position are re-written once. That is a deliberate, bounded exception to
/// this module's byte-stability invariant — the same trade
/// [`keep_recent_turns`] documents.
///
/// The default is 2, not 1, precisely because the invalidation is not free: a
/// block is pinned only once it has DEMONSTRATED recurrence, so a one-off
/// retrieve stays compressed and costs nothing extra.
pub fn pin_after_retrieves() -> usize {
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("RTK_PROXY_CCR_PIN_AFTER_RETRIEVES")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(2)
    })
}

/// Hard cap on tracked sessions. Beyond it a NEW session is not tracked at all
/// (pinning silently stays off for it) rather than growing without bound — the
/// proxy is long-lived and every distinct `x-rtk-session` would otherwise leak
/// an entry forever.
const MAX_PINNED_SESSIONS: usize = 64;

/// Hard cap on tracked block ids per session, same fail-open rule. Worst case
/// is 64 x 512 short hex keys, a few MB.
const MAX_PINNED_IDS_PER_SESSION: usize = 512;

/// Per-session tally of how many times each block id has been retrieved,
/// used to decide which blocks are exempt from compression.
///
/// Fail-open by construction: every path that cannot answer — a poisoned lock,
/// a capacity cap hit, pinning disabled — reports "not pinned", which is the
/// pre-pinning behaviour. A bug in here can cost tokens; it cannot make the
/// proxy serve wrong content or panic.
#[derive(Debug, Default)]
pub struct PinRegistry {
    inner: RwLock<HashMap<String, HashMap<String, u32>>>,
}

impl PinRegistry {
    /// Record one retrieve of `id` in `session`; returns the new count, or
    /// `None` if it could not be recorded (poisoned lock or capacity cap).
    pub fn note_retrieve(&self, session: &str, id: &str) -> Option<u32> {
        // `write()` errors only on poisoning, i.e. a panic while held. There is
        // no await inside this guard, so it cannot be held across a suspend.
        let mut map = self.inner.write().ok()?;
        let per_session = match map.get_mut(session) {
            Some(s) => s,
            None => {
                if map.len() >= MAX_PINNED_SESSIONS {
                    tracing::warn!(
                        session,
                        cap = MAX_PINNED_SESSIONS,
                        "ccr: pin registry session cap reached — pinning disabled for this session"
                    );
                    return None;
                }
                map.entry(session.to_string()).or_default()
            }
        };
        if !per_session.contains_key(id) && per_session.len() >= MAX_PINNED_IDS_PER_SESSION {
            tracing::warn!(
                session,
                cap = MAX_PINNED_IDS_PER_SESSION,
                "ccr: pin registry id cap reached — no further pins in this session"
            );
            return None;
        }
        let n = per_session.entry(id.to_string()).or_insert(0);
        *n = n.saturating_add(1);
        Some(*n)
    }

    /// Whether `id` has been retrieved at least `threshold` times in
    /// `session`. `threshold == 0` means pinning is disabled and is always
    /// `false` — NOT `count >= 0`, which would pin everything.
    pub fn is_pinned_at(&self, session: &str, id: &str, threshold: usize) -> bool {
        if threshold == 0 {
            return false;
        }
        let Ok(map) = self.inner.read() else {
            return false;
        };
        map.get(session)
            .and_then(|s| s.get(id))
            .is_some_and(|n| *n as usize >= threshold)
    }

    /// Drop a session's tally. Only used by tests today; kept because a
    /// long-lived proxy has no other way to reclaim a finished session.
    pub fn forget_session(&self, session: &str) {
        if let Ok(mut map) = self.inner.write() {
            map.remove(session);
        }
    }
}

/// Process-wide registry. One local process per operator (see the module
/// header), so a single global keyed by session is the right scope.
pub fn pins() -> &'static PinRegistry {
    static R: std::sync::OnceLock<PinRegistry> = std::sync::OnceLock::new();
    R.get_or_init(PinRegistry::default)
}

/// Record a retrieve and log if it just crossed the pin threshold.
/// Called from the retrieve resolver; safe to call for a miss.
pub fn note_retrieve(session: &str, id: &str) {
    let threshold = pin_after_retrieves();
    if threshold == 0 {
        return;
    }
    if let Some(n) = pins().note_retrieve(session, id) {
        if n as usize == threshold {
            tracing::info!(
                id,
                retrieves = n,
                "ccr: pinning block — retrieved repeatedly, will render verbatim from now on \
                 (one prefix invalidation, removes a per-turn round-trip)"
            );
        }
    }
}

/// Whether this block must be left verbatim because the model keeps asking
/// for it. Consulted by every path that would emit a marker.
fn is_pinned(session: &str, id: &str) -> bool {
    pins().is_pinned_at(session, id, pin_after_retrieves())
}

/// Compression strategy: `shape` (default, shape-aware) or `headtail`
/// (legacy `preview()` only). Boot-time, read-once — it decides how every
/// historical block renders, so changing it mid-session would re-render the
/// prefix. `RTK_PROXY_CCR_STRATEGY`.
pub fn strategy_mode() -> &'static str {
    static V: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    V.get_or_init(|| {
        std::env::var("RTK_PROXY_CCR_STRATEGY")
            .ok()
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| s == "headtail")
            .unwrap_or_else(|| "shape".to_string())
    })
}

/// DIAGNOSTIC: when `RTK_PROXY_CCR_INJECT=0`, the `rtk_retrieve` tool is not
/// injected into `tools[]`. Because `forward`'s `ccr_active` keys off the
/// tool's presence, this also disables response buffering — the request takes
/// the plain streaming relay. Compression still runs, so markers may reference
/// a tool that is absent; use only to isolate whether tool injection (not
/// buffering, not elision) is what changes model behaviour. Default: inject.
pub fn inject_enabled() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("RTK_PROXY_CCR_INJECT").ok().as_deref() != Some("0"))
}

/// DIAGNOSTIC: when `RTK_PROXY_FORCE_STREAM=1`, `forward` leaves `ccr_active`
/// false even though the retrieve tool IS injected, so the response streams
/// through unbuffered and `rtk_retrieve` calls are NOT resolved (they leak to
/// the client). Use only to isolate whether buffering — separate from tool
/// injection — is what changes model behaviour. Default: off.
pub fn force_stream() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("RTK_PROXY_FORCE_STREAM").ok().as_deref() == Some("1"))
}

/// Leading / trailing lines kept in the preview (head + tail).
pub(super) const PREVIEW_HEAD_LINES: usize = 20;
pub(super) const PREVIEW_TAIL_LINES: usize = 8;

/// Character head+tail kept when a block has too few newlines for the
/// line-based window to shrink it (minified JSON, base64, one long line).
pub(super) const PREVIEW_HEAD_CHARS: usize = 400;
pub(super) const PREVIEW_TAIL_CHARS: usize = 200;

/// Upper bound on internal CCR retrieve rounds per request, so a
/// misbehaving model cannot drive an unbounded re-call loop.
pub const MAX_ROUNDS: usize = 6;

/// A pending cache write produced by compression: the session-scoped topic
/// and the original content to store. Persisted to ICM by [`store_entries`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntry {
    pub topic: String,
    pub content: String,
}

/// One compression (or skip) decision, for the audit log. Written once per
/// unique `(session, block_id)` — the decision is a pure function of content,
/// so a block recurring across 60 requests is one row, not 60.
#[derive(Debug, Clone, Default)]
pub struct DecisionRecord {
    pub session: String,
    pub block_id: String,
    pub tool: Option<String>,
    pub field: Option<String>,
    pub shape: String,
    pub decided_by: String,
    pub strategy: String,
    pub normalisers: String,
    pub elided_ranges: String,
    pub tokens_before: i64,
    pub tokens_after: i64,
    pub f_indent_sd: i64,
    pub f_prefix_rep: i64,
    pub f_unique_ratio: i64,
}

impl DecisionRecord {
    pub(super) fn from_compressed(
        session: &str,
        tool: Option<&str>,
        field: Option<&str>,
        c: &Compressed,
    ) -> Self {
        let m = c.meta.clone().unwrap_or_default();
        DecisionRecord {
            session: session.to_string(),
            block_id: content_id(&c.entry.content),
            tool: tool.map(str::to_string),
            field: field.map(str::to_string),
            shape: m.shape.to_string(),
            decided_by: m.decided_by.to_string(),
            strategy: if m.strategy.is_empty() {
                "headtail".to_string()
            } else {
                m.strategy.to_string()
            },
            normalisers: m.normalisers,
            elided_ranges: m.elided_ranges,
            tokens_before: c.original_tokens as i64,
            tokens_after: c.replacement_tokens as i64,
            f_indent_sd: m.features.indent_sd as i64,
            f_prefix_rep: m.features.prefix_rep as i64,
            f_unique_ratio: m.features.unique_ratio as i64,
        }
    }
}

/// Build the session-scoped ICM topic for a cached block `id`. The `ccr/`
/// prefix keeps it out of the real-memory namespace.
pub fn cache_topic(session: &str, id: &str) -> String {
    format!("ccr/{session}/{id}")
}

/// Resolve the cache namespace for a request: the `x-rtk-session` header when
/// a client sends one, else a fingerprint of the conversation itself.
///
/// Agents do not send the header, and a single shared fallback would put every
/// conversation on the machine into one namespace -- content cached in
/// conversation A would be retrievable from conversation B by id. The first user
/// message is identical on every request of a conversation for its whole life,
/// so hashing it (plus the model) yields a stable per-conversation label with
/// zero client cooperation -- any agent, any dialect. Two conversations opened
/// with byte-identical first messages share a namespace; same owner, same
/// machine, accepted.
pub fn session_of(req: &InterceptedRequest) -> String {
    if let Some(session) = &req.session {
        return session.clone();
    }
    let first_user = req
        .body
        .get("messages")
        .and_then(|m| m.as_array())
        .and_then(|msgs| {
            msgs.iter()
                .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
        })
        .and_then(|m| m.get("content"));
    match first_user {
        Some(content) => {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(req.model.as_bytes());
            hasher.update(b"\0");
            hasher.update(content.to_string().as_bytes());
            let digest = hasher.finalize();
            let mut out = String::with_capacity(16);
            for byte in &digest[..8] {
                out.push_str(&format!("{byte:02x}"));
            }
            out
        }
        None => "default".to_string(),
    }
}

/// Content hash -> stable `id` for a block. Pure function of the content, so
/// the same block always maps to the same id (cache-safe + dedup).
pub fn content_id(content: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(content.as_bytes());
    // 16 hex chars (8 bytes) is plenty for a cache key; avoids a `hex` crate
    // dependency for one call site.
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// The compression marker substituted for a removed block middle. Pure
/// function of `(id, original_tokens)` so it is byte-stable across turns.
fn marker(id: &str, original_tokens: usize) -> String {
    format!(
        "\n[{CCR_MARKER_TAG} {original_tokens} tokens — call {RETRIEVE_TOOL_NAME}(\"{id}\") to get the full content]"
    )
}

/// Distinctive structural fragment of a real compression marker. The token
/// count is variable, but this exact frame is unique to a marker we
/// emitted — unlike the bare human-readable [`CCR_MARKER_TAG`], which
/// benign tool output can contain and thereby trip idempotency / retrieve
/// detection.
pub(super) fn marker_signature() -> String {
    format!(" tokens — call {RETRIEVE_TOOL_NAME}(\"")
}

/// Whether `text` already carries a CCR marker (idempotency guard).
fn already_compressed(text: &str) -> bool {
    text.contains(&marker_signature())
}

/// Produce the head+tail preview for `text`. Multi-line blocks keep the
/// first [`PREVIEW_HEAD_LINES`] and last [`PREVIEW_TAIL_LINES`] lines.
/// Blocks with too few newlines fall back to a CHARACTER head+tail window,
/// so a huge single-line block (minified JSON, base64) still compresses.
/// Small blocks are returned unchanged. Pure function of `text`.
fn preview(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.len() > PREVIEW_HEAD_LINES + PREVIEW_TAIL_LINES {
        let head = lines[..PREVIEW_HEAD_LINES].join("\n");
        let tail = lines[lines.len() - PREVIEW_TAIL_LINES..].join("\n");
        return format!("{head}\n…\n{tail}");
    }
    if text.chars().count() <= PREVIEW_HEAD_CHARS + PREVIEW_TAIL_CHARS {
        return text.to_string();
    }
    let head: String = text.chars().take(PREVIEW_HEAD_CHARS).collect();
    let tail_rev: Vec<char> = text.chars().rev().take(PREVIEW_TAIL_CHARS).collect();
    let tail: String = tail_rev.into_iter().rev().collect();
    format!("{head}…{tail}")
}

/// Outcome of compressing a single text payload.
pub(super) struct Compressed {
    pub text: String,
    pub entry: CacheEntry,
    pub original_tokens: usize,
    pub replacement_tokens: usize,
    /// How the shape pipeline rendered this block, for the decision log.
    /// `None` when the legacy head+tail path produced it.
    pub meta: Option<strategy::DecisionMeta>,
}

/// Compress one text payload if it is large enough and not already
/// compressed. Pure: returns `None` (no-op) when the block stays verbatim.
/// Provider-agnostic — shared by both dialect modules.
pub(super) fn compress_text(text: &str, session: &str) -> Option<Compressed> {
    compress_text_floor(text, session, compress_min_tokens())
}

/// [`compress_text`] with the floor supplied by the caller, so a tool_result
/// can be judged by the recovery cost of the tool that produced it — see
/// [`floor_for_tool`].
pub(super) fn compress_text_floor(text: &str, session: &str, floor: usize) -> Option<Compressed> {
    if already_compressed(text) {
        return None;
    }
    // Never compress a value a lossy stage already TRUNCATED: caching a
    // truncated text as "the original" would make rtk_retrieve return a body
    // with its middle gone while claiming it is complete. No stage in this
    // crate truncates; a downstream one might (see the tag's docs). The same
    // rule covers hook-site processing: a block the posthook already capped
    // or stamped is not "the original" either (single-site resolution).
    if text.contains(crate::pipeline::TRUNCATION_MARKER_TAG)
        || rtk_engine::marker::carries_hook_stamp(text)
    {
        return None;
    }
    let original_tokens = crate::tokenizer::count(text);
    if original_tokens < floor {
        return None;
    }
    let id = content_id(text);
    // The model has asked for this block back repeatedly; compressing it again
    // just buys another retrieve round-trip next turn. See
    // [`pin_after_retrieves`].
    if is_pinned(session, &id) {
        return None;
    }
    let (body, meta) = if strategy_mode() == "headtail" {
        (preview(text), None)
    } else {
        match strategy::render(text, floor, crate::tokenizer::count) {
            Some(r) => (r.body, Some(r.meta)),
            None => (preview(text), None),
        }
    };
    let replacement = format!("{body}{}", marker(&id, original_tokens));
    let replacement_tokens = crate::tokenizer::count(&replacement);
    if replacement_tokens >= original_tokens {
        return None;
    }
    Some(Compressed {
        entry: CacheEntry {
            topic: cache_topic(session, &id),
            content: text.to_string(),
        },
        text: replacement,
        original_tokens,
        replacement_tokens,
        meta,
    })
}

/// Replace a file body echoed back through a tool argument with a pointer to
/// the file it was written to.
///
/// Different trade from [`compress_text`], which keeps a head+tail preview
/// because the content exists nowhere else. Here it does: the write already
/// succeeded, so the bytes are on disk and recovery is the agent's own `Read`
/// — byte-exact, current, and no extra round trip through `rtk_retrieve`.
/// The ICM copy is still written and the id still emitted, because a file can
/// be deleted, moved or reverted; the pointer is the preferred path, not the
/// only one.
///
/// The standard [`marker`] is appended verbatim so idempotency
/// ([`already_compressed`]), history detection ([`history_has_marker`]) and
/// retrieve resolution all keep working with no special case.
pub(super) fn retire_to_pointer(text: &str, session: &str, path: &str) -> Option<Compressed> {
    if already_compressed(text) {
        return None;
    }
    if text.contains(crate::pipeline::TRUNCATION_MARKER_TAG)
        || rtk_engine::marker::carries_hook_stamp(text)
    {
        return None;
    }
    let original_tokens = crate::tokenizer::count(text);
    if original_tokens < retire_min_tokens() {
        return None;
    }
    let id = content_id(text);
    // Same rule as [`compress_text_floor`]. A retrieved pointer means the
    // agent could not recover the body from the file, so re-pointing it is
    // strictly worse than sending it.
    if is_pinned(session, &id) {
        return None;
    }
    let lines = text.split('\n').count();
    let replacement = format!(
        "[{CCR_MARKER_TAG} — {lines} lines written to {path}; \
         read that file for its current contents, and read it before editing \
         it so any old_string you build matches byte-for-byte]{}",
        marker(&id, original_tokens)
    );
    let replacement_tokens = crate::tokenizer::count(&replacement);
    if replacement_tokens >= original_tokens {
        return None;
    }
    Some(Compressed {
        entry: CacheEntry {
            topic: cache_topic(session, &id),
            content: text.to_string(),
        },
        text: replacement,
        original_tokens,
        replacement_tokens,
        meta: Some(strategy::DecisionMeta {
            strategy: "pointer",
            ..Default::default()
        }),
    })
}

/// Whether the request's message history already carries a CCR marker from
/// a PRIOR turn's compression. When true, the retrieve tool must be present
/// and the resolving loop must run even if THIS turn compressed nothing —
/// otherwise an in-context "call rtk_retrieve(id)" instruction points at a
/// tool absent from `tools[]`. Dialect-agnostic: a plain string scan over
/// the whole body.
pub fn history_has_marker(body: &serde_json::Value) -> bool {
    let sig = marker_signature();
    fn scan(v: &serde_json::Value, sig: &str) -> bool {
        match v {
            serde_json::Value::String(s) => s.contains(sig),
            serde_json::Value::Array(a) => a.iter().any(|x| scan(x, sig)),
            serde_json::Value::Object(o) => o.values().any(|x| scan(x, sig)),
            _ => false,
        }
    }
    body.get("messages")
        .and_then(|m| m.as_array())
        .is_some_and(|messages| messages.iter().any(|m| scan(m, &sig)))
}

/// Compress every eligible block in `req`'s body for its dialect, inject the
/// retrieve tool, and return the cache entries + metrics. `None` for an
/// unsupported provider or when nothing was compressed (tool NOT injected,
/// so off-traffic stays byte-identical).
pub fn compress_request(
    req: &mut InterceptedRequest,
) -> Option<(Vec<CacheEntry>, Vec<DecisionRecord>, CcrMetrics)> {
    match req.provider {
        Provider::Anthropic => anthropic::compress_request(req),
        Provider::Openai => openai::compress_request(req),
        Provider::Google => None,
    }
}

/// Inject the retrieve tool into `body` for `provider`'s dialect,
/// UNCONDITIONALLY -- whether or not this turn compressed anything and whether
/// or not the history already carries a marker.
///
/// This looks wasteful (~100 tokens on requests that need no retrieval) and is
/// the opposite: `tools[]` sits at the very front of the cached prefix, so
/// injecting the tool only once something HAS been compressed *changes the
/// prefix mid-session* and invalidates the whole prompt cache at that moment.
/// Keeping the tool present from request #1 makes
/// `tools[]` byte-stable for the session; its ~100 tokens are cache-read.
///
/// INVARIANT: tool present <=> the CCR resolving loop armed. `forward` keys
/// `ccr_active` off [`retrieve_tool_present`], so a model that calls
/// `rtk_retrieve` is always answered by the proxy and never leaks an
/// unresolved `tool_use` to the client.
pub fn ensure_retrieve_tool(provider: Provider, body: &mut serde_json::Value) {
    match provider {
        Provider::Anthropic => anthropic::inject_retrieve_tool(body),
        Provider::Openai => openai::inject_retrieve_tool(body),
        Provider::Google => {}
    }
}

/// Whether `body` already carries the injected retrieve tool, in `provider`'s
/// dialect (Anthropic: `tools[].name`; OpenAI: `tools[].function.name`).
/// This is the precise signal for "the CCR forward loop must be armed".
pub fn retrieve_tool_present(provider: Provider, body: &serde_json::Value) -> bool {
    let Some(arr) = body.get("tools").and_then(|t| t.as_array()) else {
        return false;
    };
    arr.iter().any(|t| {
        let name = match provider {
            Provider::Openai => t.pointer("/function/name"),
            _ => t.get("name"),
        };
        name.and_then(|n| n.as_str()) == Some(RETRIEVE_TOOL_NAME)
    })
}

/// Persist a batch of cache entries to ICM (best-effort; a failed write only
/// means that block can't be retrieved later — the preview still went out).
pub async fn store_entries(memory: &IcmMemory, entries: Vec<CacheEntry>) {
    for entry in entries {
        if let Err(e) = memory.put_exact(entry.topic, entry.content).await {
            tracing::warn!(error = %e, "ccr: failed to cache compressed block");
        }
    }
}

/// Narrow `content` to the part relevant to `query`. Best-effort,
/// dependency-free: keep lines containing any query term (case
/// insensitive), with a little surrounding context. No query (or no match)
/// returns the full content. Dialect-agnostic.
pub fn apply_query(content: &str, query: Option<&str>) -> String {
    let Some(q) = query.map(|s| s.trim()).filter(|s| !s.is_empty()) else {
        return content.to_string();
    };
    let terms: Vec<String> = q.split_whitespace().map(|t| t.to_lowercase()).collect();
    let lines: Vec<&str> = content.split('\n').collect();
    let mut keep = vec![false; lines.len()];
    let mut any = false;
    for (i, line) in lines.iter().enumerate() {
        let lower = line.to_lowercase();
        if terms.iter().any(|t| lower.contains(t)) {
            any = true;
            let lo = i.saturating_sub(1);
            let hi = (i + 1).min(lines.len().saturating_sub(1));
            for k in keep.iter_mut().take(hi + 1).skip(lo) {
                *k = true;
            }
        }
    }
    if !any {
        return content.to_string();
    }
    lines
        .iter()
        .zip(keep.iter())
        .filter(|(_, k)| **k)
        .map(|(l, _)| *l)
        .collect::<Vec<_>>()
        .join("\n")
}

/// A single retrieve tool call the proxy must resolve itself, dialect-
/// agnostic once extracted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrieveCall {
    /// The tool-call id (Anthropic `tool_use.id` / OpenAI `tool_calls[].id`)
    /// needed to build the matching result message.
    pub call_id: String,
    /// The `id` argument: the content hash naming the cached block.
    pub id: String,
    /// Optional `query` to narrow the returned content.
    pub query: Option<String>,
}

/// The pipeline stage: compresses eligible blocks and writes the cache
/// entries to ICM, in-request. The retrieve-round LOOP is not a
/// `Transform` (it must re-call upstream) — see `crate::ccr_forward`,
/// dispatched from `server::handle` when this stage compressed something
/// (or the history already carries a marker).
pub struct CcrTransform {
    enabled: SharedFlag,
    memory: Option<IcmMemory>,
    store: Option<crate::store::Store>,
}

impl CcrTransform {
    pub fn new(enabled: SharedFlag, memory: Option<IcmMemory>) -> Self {
        Self {
            enabled,
            memory,
            store: None,
        }
    }

    /// Attach the usage store so classifier decisions are logged. Optional:
    /// without it, compression still runs, just unaudited.
    pub fn with_store(mut self, store: Option<crate::store::Store>) -> Self {
        self.store = store;
        self
    }

    fn is_on(&self) -> bool {
        self.enabled.read().map(|g| *g).unwrap_or(false)
    }
}

#[async_trait::async_trait]
impl super::Transform for CcrTransform {
    fn name(&self) -> &'static str {
        STAGE_NAME
    }

    async fn on_request(&self, req: &mut InterceptedRequest) -> crate::error::AppResult<()> {
        if !self.is_on() {
            return Ok(());
        }
        let Some(memory) = &self.memory else {
            return Ok(());
        };
        let session = session_of(req);
        if let Some((entries, decisions, metrics)) = compress_request(req) {
            tracing::info!(
                session = %session,
                tokens_compressed = metrics.tokens_compressed,
                tokens_in_context = metrics.tokens_in_context,
                blocks_compressed = metrics.blocks_compressed,
                "ccr: compressed request, cached original content"
            );
            req.ccr = metrics;
            if let Some(store) = &self.store {
                let agent = req.client.clone().unwrap_or_default();
                let ts = chrono::Utc::now().to_rfc3339();
                let rows: Vec<crate::store::DecisionRow> = decisions
                    .into_iter()
                    .map(|d| crate::store::DecisionRow {
                        session: d.session,
                        block_id: d.block_id,
                        tool: d.tool,
                        field: d.field,
                        normalisers: d.normalisers,
                        shape: d.shape,
                        decided_by: d.decided_by,
                        strategy: d.strategy,
                        elided_ranges: d.elided_ranges,
                        tokens_before: d.tokens_before,
                        tokens_after: d.tokens_after,
                        f_indent_sd: d.f_indent_sd,
                        f_prefix_rep: d.f_prefix_rep,
                        f_unique_ratio: d.f_unique_ratio,
                    })
                    .collect();
                store.record_decisions(&agent, &ts, rows).await;
            }
            store_entries(memory, entries).await;
        } else {
            // Nothing compressed THIS turn -- inject anyway. Keeps `tools[]`
            // byte-stable so the first compression of the session cannot
            // invalidate the prompt cache (see `ensure_retrieve_tool`), and
            // keeps a prior turn's in-context "call rtk_retrieve(id)"
            // instruction pointing at a tool that is actually present.
            ensure_retrieve_tool(req.provider, &mut req.body);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_id_is_stable_and_short() {
        let a = content_id("hello world");
        let b = content_id("hello world");
        let c = content_id("different");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn small_block_is_not_compressed() {
        assert!(compress_text("tiny output", "s1").is_none());
    }

    #[test]
    fn large_block_is_compressed_with_marker_and_cached() {
        let big: String = (0..4000).map(|i| format!("line {i} of output\n")).collect();
        let c = compress_text(&big, "s1").expect("should compress");
        assert!(c.text.contains(CCR_MARKER_TAG));
        assert!(c.text.contains(RETRIEVE_TOOL_NAME));
        assert!(c.text.len() < big.len());
        assert_eq!(c.entry.content, big);
        assert!(c.entry.topic.starts_with("ccr/s1/"));
        assert!(c.original_tokens > c.replacement_tokens);
    }

    #[test]
    fn compression_is_idempotent() {
        let big: String = (0..4000).map(|i| format!("line {i} of output\n")).collect();
        let c = compress_text(&big, "s1").expect("first pass compresses");
        assert!(
            compress_text(&c.text, "s1").is_none(),
            "already compressed -> no-op"
        );
    }

    #[test]
    fn truncated_text_is_not_cached_as_original() {
        let mut big: String = (0..4000).map(|i| format!("line {i}\n")).collect();
        big.push_str(&format!(
            "\n[… 900 {} …]\n",
            crate::pipeline::TRUNCATION_MARKER_TAG
        ));
        assert!(
            compress_text(&big, "s1").is_none(),
            "truncated text left alone"
        );
    }

    #[test]
    fn hook_processed_text_is_not_compressed_again() {
        let mut capped: String = (0..4000).map(|i| format!("line {i}\n")).collect();
        capped.push_str("[1200 lines elided — full output in recall file]\n");
        assert!(
            compress_text(&capped, "s1").is_none(),
            "hook-capped block skipped"
        );

        let mut hinted: String = (0..4000).map(|i| format!("line {i}\n")).collect();
        hinted.push_str("[full output: ~/.local/share/rtk/tee/posthook/1_2-3_bash_x.log]\n");
        assert!(
            compress_text(&hinted, "s1").is_none(),
            "recall-hinted block skipped"
        );

        let mut stamped: String = (0..4000).map(|i| format!("line {i}\n")).collect();
        stamped.push_str("[rtk:cap 9000→400 tokens · rtk recall 1a2b3c]\n");
        assert!(
            compress_text(&stamped, "s1").is_none(),
            "engine-stamped block skipped"
        );
    }

    #[test]
    fn huge_single_line_blob_is_compressed() {
        let blob: String = (0..4000).map(|i| format!("k{i}:v{i},")).collect();
        assert_eq!(blob.matches('\n').count(), 0);
        assert!(crate::tokenizer::count(&blob) >= COMPRESS_MIN_TOKENS);
        let c = compress_text(&blob, "s1").expect("single-line blob must compress");
        assert!(c.text.len() < blob.len());
        assert_eq!(c.entry.content, blob);
    }

    #[test]
    fn preview_windows_low_newline_blob_but_keeps_small_verbatim() {
        assert_eq!(preview("small one-liner"), "small one-liner");
        let blob = "a".to_string() + &"bcdefghij".repeat(500);
        let p = preview(&blob);
        assert!(p.chars().count() < blob.chars().count());
        assert!(p.contains('…'));
        assert!(p.starts_with('a'));
    }

    #[test]
    fn benign_tag_mention_is_not_a_marker() {
        let benign = "note: rtk-proxy: compressed the cache to 12 entries";
        assert!(!already_compressed(benign));
        assert!(already_compressed(&format!(
            "preview…{}",
            marker("id1", 4200)
        )));
    }

    #[test]
    fn history_has_marker_detects_prior_compression() {
        let clean = serde_json::json!({ "messages": [ { "role": "user", "content": "hello" } ] });
        assert!(!history_has_marker(&clean));
        let withmarker = serde_json::json!({ "messages": [
            { "role": "user", "content": [ { "type": "tool_result", "tool_use_id": "t1",
                "content": format!("preview…{}", marker("abc123", 5000)) } ] }
        ]});
        assert!(history_has_marker(&withmarker));
    }

    #[test]
    fn apply_query_filters_to_matching_lines() {
        let mut lines: Vec<String> = (0..50).map(|i| format!("noise line {i}")).collect();
        lines[10] = "the error happened here".to_string();
        let content = lines.join("\n");
        let out = apply_query(&content, Some("error"));
        assert!(out.contains("the error happened here"));
        assert!(out.lines().count() < content.lines().count());
        assert_eq!(apply_query(&content, None), content);
        assert_eq!(apply_query(&content, Some("zzzznomatch")), content);
    }

    // The registry is a process global keyed by session, so every test below
    // uses its own session name. That keeps them isolated without a lock and
    // without touching process environment: the threshold is passed
    // explicitly to `is_pinned_at` rather than read from
    // `pin_after_retrieves()`.

    #[test]
    fn pin_registry_counts_per_session_and_id() {
        let r = PinRegistry::default();
        assert_eq!(r.note_retrieve("s1", "aaa"), Some(1));
        assert_eq!(r.note_retrieve("s1", "aaa"), Some(2));
        assert_eq!(r.note_retrieve("s1", "bbb"), Some(1));
        assert_eq!(r.note_retrieve("s2", "aaa"), Some(1));
    }

    #[test]
    fn pin_requires_reaching_the_threshold() {
        let r = PinRegistry::default();
        r.note_retrieve("s", "id1");
        assert!(!r.is_pinned_at("s", "id1", 2), "one retrieve must not pin");
        r.note_retrieve("s", "id1");
        assert!(r.is_pinned_at("s", "id1", 2), "second retrieve must pin");
    }

    #[test]
    fn pin_threshold_one_pins_immediately() {
        let r = PinRegistry::default();
        r.note_retrieve("s", "id1");
        assert!(r.is_pinned_at("s", "id1", 1));
    }

    #[test]
    fn pin_threshold_zero_disables_pinning() {
        // Guard against `count >= 0` pinning everything.
        let r = PinRegistry::default();
        for _ in 0..10 {
            r.note_retrieve("s", "id1");
        }
        assert!(!r.is_pinned_at("s", "id1", 0));
    }

    #[test]
    fn pin_is_scoped_to_its_session() {
        let r = PinRegistry::default();
        r.note_retrieve("alice", "shared-id");
        r.note_retrieve("alice", "shared-id");
        assert!(r.is_pinned_at("alice", "shared-id", 2));
        assert!(!r.is_pinned_at("bob", "shared-id", 2));
    }

    #[test]
    fn unknown_session_or_id_is_not_pinned() {
        let r = PinRegistry::default();
        assert!(!r.is_pinned_at("nobody", "nothing", 1));
    }

    #[test]
    fn pin_registry_caps_ids_per_session_without_panicking() {
        let r = PinRegistry::default();
        for i in 0..MAX_PINNED_IDS_PER_SESSION {
            assert_eq!(r.note_retrieve("s", &format!("id{i}")), Some(1));
        }
        // One past the cap: refused, fail-open, no panic.
        assert_eq!(r.note_retrieve("s", "overflow"), None);
        assert!(!r.is_pinned_at("s", "overflow", 1));
        // An ALREADY-tracked id still counts up once the cap is reached,
        // otherwise a busy session could never pin anything.
        assert_eq!(r.note_retrieve("s", "id0"), Some(2));
    }

    #[test]
    fn pin_registry_caps_sessions_without_panicking() {
        let r = PinRegistry::default();
        for i in 0..MAX_PINNED_SESSIONS {
            assert_eq!(r.note_retrieve(&format!("s{i}"), "id"), Some(1));
        }
        assert_eq!(r.note_retrieve("one-too-many", "id"), None);
    }

    #[test]
    fn forget_session_clears_its_pins() {
        let r = PinRegistry::default();
        r.note_retrieve("s", "id1");
        assert!(r.is_pinned_at("s", "id1", 1));
        r.forget_session("s");
        assert!(!r.is_pinned_at("s", "id1", 1));
    }

    #[test]
    fn pinned_block_is_left_verbatim_by_compress() {
        let session = "pin-compress-test";
        let text = "line\n".repeat(4000); // comfortably over any floor
        let id = content_id(&text);

        let before = compress_text_floor(&text, session, 300)
            .expect("an oversized block must compress before it is pinned");
        assert!(before.text.contains(CCR_MARKER_TAG));

        for _ in 0..pin_after_retrieves().max(1) {
            pins().note_retrieve(session, &id);
        }

        if pin_after_retrieves() == 0 {
            // Pinning disabled in this environment: behaviour must not change.
            assert!(compress_text_floor(&text, session, 300).is_some());
        } else {
            assert!(
                compress_text_floor(&text, session, 300).is_none(),
                "a pinned block must render verbatim, not as a marker"
            );
            assert!(
                compress_text_floor(&text, "some-other-session", 300).is_some(),
                "pinning must not leak across sessions"
            );
        }
        pins().forget_session(session);
    }

    #[test]
    fn pinned_written_file_body_is_not_retired_to_a_pointer() {
        let session = "pin-retire-test";
        let text = "x = 1\n".repeat(4000);
        let id = content_id(&text);
        assert!(
            retire_to_pointer(&text, session, "/app/f.py").is_some(),
            "an oversized written body must retire before it is pinned"
        );
        for _ in 0..pin_after_retrieves().max(1) {
            pins().note_retrieve(session, &id);
        }
        if pin_after_retrieves() != 0 {
            assert!(
                retire_to_pointer(&text, session, "/app/f.py").is_none(),
                "a pinned body must be sent verbatim rather than re-pointed"
            );
        }
        pins().forget_session(session);
    }
}
