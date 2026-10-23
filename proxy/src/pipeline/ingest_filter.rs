//! Tool-result / tool-use ingest filter: deterministic, cache-safe compression
//! of bloated `tool_result` content AND `tool_use` input values across the WHOLE
//! message history.
//!
//! # EXPERIMENTAL
//!
//! Every lever in this module is **EXPERIMENTAL and NOT validated for
//! production**. Truncation is *lossy* — it drops the middle of a block, which
//! can degrade LLM output quality on the affected turn. Dedup is *near-lossless*
//! (it only replaces an EXACT byte-identical repeat with an identifying
//! back-reference) but it still **alters the context** the model sees, so it has
//! not been signed off for production either. All levers default OFF; turn them
//! on only when you can A/B the effect on quality and cache-token metrics.
//!
//! Claude Code resends the full (unfiltered) conversation every turn. This
//! transform rewrites `tool_result` blocks as a **pure function of their own
//! content** (plus, for dedup, the set of *earlier* blocks in the same request).
//! Because the rewrite is deterministic and position-independent for a given
//! content, the prefix the provider sees stays byte-identical across turns, so
//! the prompt cache keeps hitting and the saving compounds on every future
//! `cache_read`. A non-deterministic or position-dependent transform would bust
//! the cache — so nothing here may depend on time, randomness, or absolute
//! position. The transform is also idempotent: re-filtering filtered text is a
//! no-op.
//!
//! Two stages, applied per `tool_result` text payload, in history order:
//!   1. **Size filter** — if the text exceeds a token threshold, keep the first
//!      `head` and last `tail` lines and replace the middle with a marker.
//!   2. **Dedup (backward-only)** — if the (post-truncation) text is identical
//!      to one seen *earlier* in this request, replace it with a short marker.
//!      Only earlier blocks are ever referenced, so the decision is order-stable.
//!
//! `tool_use` blocks get the **size filter only** (no dedup): agentic clients
//! echo whole file bodies into `tool_use.input` — Write sends `content`, Edit
//! sends `old_string`/`new_string` — and these large text values are resent in
//! the history every turn, exactly like tool_result output. Each top-level
//! STRING VALUE of the input object that exceeds the threshold gets the same
//! deterministic head+tail line truncation. The JSON structure, the tool name,
//! `file_path`, and small fields are left intact; truncation only ever shortens
//! a long string value. Dedup is deliberately NOT applied to tool_use values, so
//! an argument is never replaced by a "duplicate" marker (which would be
//! semantically wrong for a call's input). The truncate is a pure function of
//! the value, so single-line values (Bash `command`, `file_path`) are no-ops.
//! Because tool_use blocks are HISTORICAL — the call already executed and its
//! result is elsewhere in the history — shortening the echoed argument body does
//! not change any future behaviour.
//!
//! Only Anthropic and OpenAI (Chat Completions) requests are touched; system,
//! tools, and plain text are left untouched. Off by default.

use crate::error::AppResult;
use crate::model::{InterceptedRequest, Provider};
use crate::pipeline::Transform;
use crate::pipeline::level::{CompressionLevel, SharedCompression};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Marker substring used for size-truncated middles. Its presence is also the
/// idempotency guard (already-truncated text is never re-truncated).
pub const TRUNC_MARKER_TAG: &str = "lines omitted by rtk-proxy";

/// Stable substring present in EVERY dedup back-reference marker. It is the
/// idempotency guard (an already-dedup'd block is never dedup'd again) and lets
/// callers recognize a marker without parsing it. Shared with `the dedup marker helpers`
/// so the identifying marker is used everywhere dedup happens.
pub(crate) const DEDUP_MARKER_TAG: &str = "rtk-proxy: identical to the earlier";

/// Minimum size (in tokens) a block must reach to be eligible for *dedup-only*
/// mode. Below this floor a repeat is left untouched — tiny echoes ("OK") are
/// not worth a back-reference and the marker could be longer than the content.
const DEDUP_ONLY_MIN_TOKENS: usize = 200;

/// Build an **identifying, universal** dedup back-reference marker for a block.
///
/// The marker is unambiguous even when several DIFFERENT blocks are dedup'd in
/// the same request: it is derived from the block's OWN content — a discriminant
/// prefix (first ~2 lines / ~120 chars) plus the block's line count — so the LLM
/// can tell which earlier original each reference points at. It works for any
/// repeated text (tool_result, tool_use input value, or a plain text/code block).
///
/// When the block is a `tool_result`, an optional `origin` (e.g. `Read(/src/x)`,
/// the originating call) is prefixed for readability — but the content-prefix is
/// the real identifier, so the marker is meaningful for non-tool text too.
///
/// Pure function of `text` (+ the optional `origin`), so it is byte-stable across
/// turns: cache-safe.
pub(crate) fn dedup_marker(text: &str, origin: Option<&str>) -> String {
    let line_count = text.split('\n').count();

    // Discriminant prefix: first ~2 lines, capped at ~120 chars, with control
    // whitespace made visible so the snippet stays on one line and readable.
    let mut prefix: String = text.split('\n').take(2).collect::<Vec<_>>().join("\n");
    // Cap on a char boundary (≈120 chars).
    if prefix.chars().count() > 120 {
        prefix = prefix.chars().take(120).collect::<String>();
        prefix.push('…');
    }
    let prefix = prefix.replace('\n', "\\n").replace('\t', "\\t");

    match origin {
        Some(o) => {
            format!("[{DEDUP_MARKER_TAG} {line_count}-line {o} block starting with: \"{prefix}\"]")
        }
        None => format!("[{DEDUP_MARKER_TAG} {line_count}-line block starting with: \"{prefix}\"]"),
    }
}

/// Per-level thresholds for the size filter.
///
/// `max_tokens` is the threshold above which a `tool_result` is truncated;
/// `head`/`tail` are the number of leading/trailing lines kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
    pub max_tokens: usize,
    pub head: usize,
    pub tail: usize,
}

/// Map a [`CompressionLevel`] to its ingest-filter thresholds; `None` = no-op.
pub fn thresholds(level: CompressionLevel) -> Option<Thresholds> {
    match level {
        CompressionLevel::Off => None,
        CompressionLevel::Light => Some(Thresholds {
            max_tokens: 4000,
            head: 120,
            tail: 40,
        }),
        CompressionLevel::Balanced => Some(Thresholds {
            max_tokens: 1500,
            head: 60,
            tail: 20,
        }),
        // Keep 150 head + 50 tail so large file bodies in tool_result /
        // tool_use are not gutted in the middle (which can corrupt edits).
        CompressionLevel::Aggressive => Some(Thresholds {
            max_tokens: 1000,
            head: 150,
            tail: 50,
        }),
    }
}

/// EXPERIMENTAL suffix appended to every user-facing level description, so the
/// operator always sees that these levers are not production-validated.
const EXPERIMENTAL_SUFFIX: &str =
    " (EXPERIMENTAL — not validated for production; may degrade LLM output quality)";

/// One-line human description of a level, for the Settings UI.
pub fn description(level: CompressionLevel) -> &'static str {
    match level {
        CompressionLevel::Off => "No tool-result filtering (default).",
        CompressionLevel::Light => {
            "Truncate tool_results over ~4000 tokens (keep 120 head / 40 tail lines; never dedups). (EXPERIMENTAL — not validated for production; may degrade LLM output quality)"
        }
        CompressionLevel::Balanced => {
            "Truncate tool_results over ~1500 tokens (keep 60 head / 20 tail lines; never dedups). (EXPERIMENTAL — not validated for production; may degrade LLM output quality)"
        }
        CompressionLevel::Aggressive => {
            "Truncate tool_results over ~1000 tokens (keep 150 head / 50 tail lines; never dedups). (EXPERIMENTAL — not validated for production; may degrade LLM output quality)"
        }
    }
}

/// Description of the **dedup-only** mode (near-lossless lever), for the UI.
pub fn dedup_only_description() -> String {
    format!(
        "Remove EXACT byte-identical repeats of an earlier large block (> {DEDUP_ONLY_MIN_TOKENS} tokens) \
         and NEVER truncate. Replaces each repeat with an identifying back-reference to its original. \
         Backward-only and deterministic (cache-safe). The safest lever, but still alters context.{EXPERIMENTAL_SUFFIX}"
    )
}

/// Line-oriented head+tail truncation. Pure function of `text` and `th`.
///
/// Returns `text` unchanged when it is within the token budget, already
/// truncated by us (idempotency), or too few lines to be worth truncating.
fn truncate(text: &str, th: Thresholds) -> String {
    // Idempotency guard: never re-truncate our own output.
    if text.contains(TRUNC_MARKER_TAG) {
        return text.to_string();
    }
    // Cheap length gate before tokenizing (≈ < 1 token/byte, so this never
    // skips anything that could exceed the threshold).
    if text.len() <= th.max_tokens {
        return text.to_string();
    }
    if crate::tokenizer::count(text) <= th.max_tokens {
        return text.to_string();
    }
    let lines: Vec<&str> = text.split('\n').collect();
    // Nothing to gain if the kept head+tail would cover the whole thing.
    if lines.len() <= th.head + th.tail + 1 {
        return text.to_string();
    }
    let omitted = lines.len() - th.head - th.tail;
    let mut out = String::with_capacity(th.max_tokens * 4);
    for l in &lines[..th.head] {
        out.push_str(l);
        out.push('\n');
    }
    out.push('\n');
    out.push_str(&format!("[… {omitted} {TRUNC_MARKER_TAG} …]"));
    out.push('\n');
    out.push('\n');
    for (i, l) in lines[lines.len() - th.tail..].iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(l);
    }
    out
}

/// Which mode the filter runs in for a single request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Level-based: size truncation + exact dedup of `tool_result` blocks.
    LevelFilter(Thresholds),
    /// Dedup-only: NEVER truncate; remove exact byte-identical repeats of an
    /// earlier large block (tool_result, tool_use input value, OR plain text).
    DedupOnly,
}

/// Shared state while walking the request's text payloads in order.
pub(crate) struct Filter {
    mode: Mode,
    /// Maps each dedup *identifier* (the marker we would emit for a block) to the
    /// full-content hash of the FIRST earlier block that produced it. Backward-
    /// only ⇒ the decision for any block depends solely on blocks before it ⇒
    /// order-stable across turns (cache-safe).
    ///
    /// The full-content hash is the **anti-ambiguity guard**: if a later block
    /// yields an identifier already seen but with a DIFFERENT full content (same
    /// prefix + line count, different body), we refuse to dedup it — a missed
    /// dedup is safe, an ambiguous back-reference is not.
    seen: HashMap<String, u64>,
    saved: i64,
}

impl Filter {
    fn hash(s: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        s.hash(&mut h);
        h.finish()
    }

    /// Core backward-only dedup decision, shared by every block kind.
    ///
    /// Given a block's `text` and an optional originating-call `origin` (only for
    /// `tool_result`), returns `Some(marker)` when this block is an exact repeat
    /// of an earlier one that is safe to replace, else `None` (keep verbatim).
    ///
    /// Anti-ambiguity: the first block for a given identifier is recorded with
    /// its full-content hash. A later block whose identifier collides but whose
    /// full content differs is NOT dedup'd (we'd otherwise emit a marker that
    /// could point at two different originals).
    fn dedup_decision(&mut self, text: &str, origin: Option<&str>) -> Option<String> {
        let marker = dedup_marker(text, origin);
        let content_hash = Self::hash(text);
        match self.seen.get(&marker) {
            Some(&first_hash) if first_hash == content_hash => {
                // Exact repeat of the earlier block with this identifier → dedup.
                Some(marker)
            }
            Some(_) => {
                // Identifier collision with DIFFERENT content → keep verbatim.
                None
            }
            None => {
                self.seen.insert(marker, content_hash);
                None
            }
        }
    }

    /// Process one `tool_result` text payload; returns `Some(new)` if it changed.
    /// `origin` is the originating call (e.g. `Read(/src/main.rs)`), used only to
    /// make the dedup marker more readable — the content-prefix is the real id.
    fn process(&mut self, text: &str, origin: Option<&str>) -> Option<String> {
        // Already a dedup marker → nothing to do (idempotency).
        if text.contains(DEDUP_MARKER_TAG) {
            return None;
        }
        match self.mode {
            Mode::DedupOnly => {
                // Dedup-only: NEVER truncate. Only dedup blocks above the floor.
                let before = crate::tokenizer::count(text);
                if before < DEDUP_ONLY_MIN_TOKENS {
                    return None;
                }
                let marker = self.dedup_decision(text, origin)?;
                let after = crate::tokenizer::count(&marker);
                self.saved += (before as i64 - after as i64).max(0);
                Some(marker)
            }
            Mode::LevelFilter(th) => {
                // Truncate ONLY — a level mode NEVER dedups. A dedup marker
                // replaces a whole block, and coding agents copy tool_result
                // content verbatim into edits (Edit `old_string`, Write
                // `content`), so a marker there corrupts the edit ("File not
                // found" / no match). Truncation keeps head + tail verbatim and
                // clearly marks the omitted middle, so the agent only acts on
                // content it can actually see. Dedup stays opt-in (DedupOnly).
                let truncated = truncate(text, th);
                if truncated == text {
                    return None;
                }
                let before = crate::tokenizer::count(text);
                let after = crate::tokenizer::count(&truncated);
                self.saved += (before as i64 - after as i64).max(0);
                Some(truncated)
            }
        }
    }

    /// Process one `tool_use` input string value.
    ///
    /// * In `LevelFilter` mode: size filter ONLY (no dedup). A call argument must
    ///   NEVER become a "duplicate of an earlier block" marker — coding agents
    ///   copy tool_use input values (Edit `old_string`/`new_string`, Write
    ///   `content`) verbatim into the file, so a back-reference marker there breaks
    ///   the edit ("File not found" / no match). Pure function of the value,
    ///   byte-stable across turns, idempotent.
    /// * In `DedupOnly` mode (opt-in, off by default): a large input value that
    ///   EXACTLY repeats an earlier block is dedup'd. This mode is not the default
    ///   precisely because of the risk above.
    fn process_tool_use_value(&mut self, text: &str) -> Option<String> {
        match self.mode {
            Mode::DedupOnly => {
                if text.contains(DEDUP_MARKER_TAG) {
                    return None;
                }
                let before = crate::tokenizer::count(text);
                if before < DEDUP_ONLY_MIN_TOKENS {
                    return None;
                }
                let marker = self.dedup_decision(text, None)?;
                let after = crate::tokenizer::count(&marker);
                self.saved += (before as i64 - after as i64).max(0);
                Some(marker)
            }
            Mode::LevelFilter(th) => {
                let truncated = truncate(text, th);
                if truncated == text {
                    return None;
                }
                let before = crate::tokenizer::count(text);
                let after = crate::tokenizer::count(&truncated);
                self.saved += (before as i64 - after as i64).max(0);
                Some(truncated)
            }
        }
    }

    /// Process one plain-text block (`{ type: "text", text: "..." }`).
    ///
    /// Only active in `DedupOnly` mode: removes exact repeats of an earlier large
    /// text/code block. In `LevelFilter` mode plain text is left untouched (its
    /// historical scope is only tool_result / tool_use), so this is a no-op there.
    fn process_text_block(&mut self, text: &str) -> Option<String> {
        if self.mode != Mode::DedupOnly {
            return None;
        }
        if text.contains(DEDUP_MARKER_TAG) {
            return None;
        }
        let before = crate::tokenizer::count(text);
        if before < DEDUP_ONLY_MIN_TOKENS {
            return None;
        }
        let marker = self.dedup_decision(text, None)?;
        let after = crate::tokenizer::count(&marker);
        self.saved += (before as i64 - after as i64).max(0);
        Some(marker)
    }
}

/// Build a compact `Tool(arg)` origin label from a `tool_result`'s originating
/// call, if it can be resolved from the `tool_use_id` → call map. Returns e.g.
/// `Read(/src/main.rs)` or just `Bash` when no salient argument is available.
pub(crate) fn origin_label(call: Option<&(String, Option<String>)>) -> Option<String> {
    let (name, arg) = call?;
    match arg {
        Some(a) if !a.is_empty() => {
            // A heredoc-sized `command` would make the dedup marker bigger
            // than the saving it reports; the label is cosmetic, so cap it.
            let capped = if a.len() > 80 {
                let mut end = 80;
                while !a.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{}…", &a[..end])
            } else {
                a.clone()
            };
            Some(format!("{name}({capped})"))
        }
        _ => Some(name.clone()),
    }
}

/// Map of `tool_use_id` → (tool name, key argument), collected from `tool_use`
/// blocks so a `tool_result` can name its originating call in the dedup marker.
pub(crate) type CallMap = HashMap<String, (String, Option<String>)>;

/// Rewrite the `content` of a single tool-result-shaped object in place — an
/// Anthropic `tool_result` block or an OpenAI `role: "tool"` message (both
/// carry tool output in `content`, string or text-parts). `origin` is a
/// readable `Tool(arg)` label for the originating call, if known.
pub(crate) fn filter_tool_result_block(
    block: &mut serde_json::Value,
    f: &mut Filter,
    origin: Option<&str>,
) {
    match block.get_mut("content") {
        // String content form: { type: tool_result, content: "..." }
        Some(serde_json::Value::String(s)) => {
            if let Some(r) = f.process(s, origin) {
                *s = r;
            }
        }
        // Array-of-text-blocks form: content: [ { type: text, text: "..." }, ... ]
        Some(serde_json::Value::Array(inner)) => {
            for tb in inner.iter_mut() {
                if let Some(serde_json::Value::String(t)) = tb.get_mut("text") {
                    if let Some(r) = f.process(t, origin) {
                        *t = r;
                    }
                }
            }
        }
        _ => {}
    }
}

/// Truncate the bloated string VALUES inside a single `tool_use` block's
/// `input` object, in place. Only top-level string fields are considered, and
/// only the size filter runs (no dedup). The tool `name`, `id`, the input keys,
/// and any non-string / small values are left untouched — only a long string
/// value is shortened, and the JSON shape is preserved.
fn filter_tool_use_block(block: &mut serde_json::Value, f: &mut Filter) {
    let Some(serde_json::Value::Object(input)) = block.get_mut("input") else {
        return;
    };
    // Iterate values in the map; serde_json preserves key order, but since each
    // value is processed as a pure function of its own content (+ earlier blocks
    // for dedup-only), iteration order does not affect the per-value result.
    for v in input.values_mut() {
        if let serde_json::Value::String(s) = v {
            if let Some(r) = f.process_tool_use_value(s) {
                *s = r;
            }
        }
    }
}

/// Extract a salient argument from a `tool_use` input object for the origin
/// label (e.g. `file_path`, `path`, `command`, or `pattern`). Best-effort and
/// purely cosmetic — only used to make the dedup marker more readable.
pub(crate) fn salient_arg(input: &serde_json::Value) -> Option<String> {
    let obj = input.as_object()?;
    for key in [
        "file_path",
        "path",
        "notebook_path",
        "command",
        "pattern",
        "url",
        "query",
    ] {
        if let Some(serde_json::Value::String(s)) = obj.get(key) {
            if !s.is_empty() {
                return Some(s.clone());
            }
        }
    }
    None
}

/// Collect `tool_use_id` → (tool name, key argument) from every `tool_use`
/// block, so a later `tool_result` can name its originating call in the dedup
/// marker. `want_labels` is false in modes that never print the label
/// (`LevelFilter` reads `origin` only in `DedupOnly`), skipping the
/// salient-argument extraction entirely.
fn collect_calls(messages: &[serde_json::Value], calls: &mut CallMap, want_labels: bool) {
    for msg in messages {
        let Some(blocks) = msg.get("content").and_then(|c| c.as_array()) else {
            continue;
        };
        for block in blocks {
            if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                continue;
            }
            let (Some(id), Some(name)) = (
                block.get("id").and_then(|v| v.as_str()),
                block.get("name").and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            let arg = if want_labels {
                block.get("input").and_then(salient_arg)
            } else {
                None
            };
            calls.insert(id.to_string(), (name.to_string(), arg));
        }
    }
}

/// Walk one message, filtering any `tool_result` or `tool_use` blocks it has.
fn filter_message(message: &mut serde_json::Value, f: &mut Filter, calls: &CallMap) {
    let Some(blocks) = message
        .as_object_mut()
        .and_then(|o| o.get_mut("content"))
        .and_then(|c| c.as_array_mut())
    else {
        return;
    };
    for block in blocks.iter_mut() {
        match block.get("type").and_then(|t| t.as_str()) {
            Some("tool_result") => {
                // Resolve the originating call for a readable marker.
                let origin = block
                    .get("tool_use_id")
                    .and_then(|v| v.as_str())
                    .and_then(|id| calls.get(id))
                    .and_then(|c| origin_label(Some(c)));
                filter_tool_result_block(block, f, origin.as_deref());
            }
            Some("tool_use") => filter_tool_use_block(block, f),
            // Plain text/code blocks: only dedup-only mode touches these.
            Some("text") => {
                if let Some(serde_json::Value::String(t)) = block.get_mut("text") {
                    if let Some(r) = f.process_text_block(t) {
                        *t = r;
                    }
                }
            }
            _ => {}
        }
    }
}

/// Runtime-mutable boolean flag (e.g. the `dedup_only` toggle).
pub type SharedFlag = Arc<RwLock<bool>>;

/// Context guard: when a request's estimated context nears the provider's max
/// window, force aggressive compression so it doesn't fail with "prompt is too
/// long". Config via env; `limit = 0` disables. On by default — it acts ONLY on
/// requests already near the limit (which would otherwise be rejected outright),
/// so quality/cache impact is confined to the rare rescue case.
#[derive(Clone, Copy, Debug)]
pub struct ContextGuard {
    /// Provider context window, tokens. `0` disables the guard.
    pub limit: u64,
    /// Fire when the estimate exceeds `limit * ratio` (0 < ratio ≤ 1).
    pub ratio: f64,
}

impl ContextGuard {
    /// Boot config from env. **Off by default** (`limit = 0`): near-limit rescue
    /// works by forcing compression, but compression modifies tool_result /
    /// tool_use content that coding agents copy verbatim into file operations —
    /// so escalating it can corrupt an edit. Opt in with
    /// `RTK_PROXY_CONTEXT_GUARD_LIMIT` (e.g. 1000000) only if that risk is
    /// acceptable for the workload; `RTK_PROXY_CONTEXT_GUARD_RATIO` defaults 0.95.
    pub fn from_env() -> Self {
        let limit = std::env::var("RTK_PROXY_CONTEXT_GUARD_LIMIT")
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let ratio = std::env::var("RTK_PROXY_CONTEXT_GUARD_RATIO")
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .filter(|r| *r > 0.0 && *r <= 1.0)
            .unwrap_or(0.95);
        Self { limit, ratio }
    }
    fn triggers(&self, est_tokens: u64) -> bool {
        self.limit > 0 && (est_tokens as f64) > (self.limit as f64) * self.ratio
    }
}

/// Two-stage context estimate for the [`ContextGuard`].
///
/// Stage 1 is a cheap O(n) byte walk. bytes/4 UNDER-counts dense code/JSON
/// several-fold, so it must never be the firing signal — only a conservative gate: at ≥ ~1.2
/// bytes/token best case, a body whose bytes can't reach the threshold even at
/// that density is definitively under it, and we return the cheap estimate.
/// Stage 2 (rare: only near-limit whales) sums the REAL tokenizer count over
/// every string value — accurate, and its cost is irrelevant next to the
/// upstream latency of a request that size.
fn estimate_context_tokens(body: &serde_json::Value, guard: &ContextGuard) -> u64 {
    fn text_bytes(v: &serde_json::Value) -> usize {
        match v {
            serde_json::Value::String(s) => s.len(),
            serde_json::Value::Array(a) => a.iter().map(text_bytes).sum(),
            serde_json::Value::Object(o) => o.values().map(text_bytes).sum(),
            _ => 0,
        }
    }
    fn token_count(v: &serde_json::Value) -> u64 {
        match v {
            serde_json::Value::String(s) => crate::tokenizer::count(s) as u64,
            serde_json::Value::Array(a) => a.iter().map(token_count).sum(),
            serde_json::Value::Object(o) => o.values().map(token_count).sum(),
            _ => 0,
        }
    }
    let bytes = text_bytes(body) as f64;
    let threshold = (guard.limit as f64) * guard.ratio;
    // A token is at least one byte, so `bytes` is a hard UPPER bound on the
    // token count. If even that bound can't reach the threshold, the body is
    // definitively under it — skip the (costly) tokenizer and return the cheap
    // estimate. (Using a >1 bytes/token assumption here would be unsafe: dense
    // single-byte-token content can approach 1 byte/token and slip past.)
    if bytes < threshold {
        return (bytes / 4.0) as u64;
    }
    token_count(body)
}

pub struct IngestFilterTransform {
    level: SharedCompression,
    /// When ON, the transform runs in **dedup-only** mode: it NEVER truncates,
    /// and only removes exact byte-identical repeats of an earlier large block.
    /// Takes precedence over the level-based size filter. Default OFF.
    dedup_only: SharedFlag,
    /// Near-limit rescue: force aggressive compression when the request is close
    /// to the provider context window.
    guard: ContextGuard,
}

impl IngestFilterTransform {
    pub fn new(level: SharedCompression, dedup_only: SharedFlag) -> Self {
        Self {
            level,
            dedup_only,
            guard: ContextGuard::from_env(),
        }
    }

    #[cfg(test)]
    pub fn fixed(level: CompressionLevel) -> Self {
        Self {
            level: Arc::new(RwLock::new(level)),
            dedup_only: Arc::new(RwLock::new(false)),
            guard: ContextGuard::from_env(),
        }
    }

    #[cfg(test)]
    pub fn fixed_with_guard(level: CompressionLevel, guard: ContextGuard) -> Self {
        Self {
            level: Arc::new(RwLock::new(level)),
            dedup_only: Arc::new(RwLock::new(false)),
            guard,
        }
    }

    /// Build a dedup-only filter (size filter disabled regardless of `level`).
    #[cfg(test)]
    pub fn dedup_only() -> Self {
        Self {
            level: Arc::new(RwLock::new(CompressionLevel::Off)),
            dedup_only: Arc::new(RwLock::new(true)),
            guard: ContextGuard::from_env(),
        }
    }

    fn level(&self) -> CompressionLevel {
        self.level
            .read()
            .map(|l| *l)
            .unwrap_or(CompressionLevel::Off)
    }

    fn is_dedup_only(&self) -> bool {
        self.dedup_only.read().map(|d| *d).unwrap_or(false)
    }

    /// Resolve the active mode for a request, or `None` if the transform is a
    /// no-op (dedup-only OFF and level Off).
    fn mode(&self) -> Option<Mode> {
        if self.is_dedup_only() {
            // Dedup-only wins over the level-based size filter.
            return Some(Mode::DedupOnly);
        }
        thresholds(self.level()).map(Mode::LevelFilter)
    }
}

/// Per-dialect navigation pair, kept atomic: mixing a collect with the
/// other dialect's filter would mislabel every dedup marker, and a bare
/// tuple of fn-pointers would happily mix them. `collect_calls` takes
/// `want_labels` (see its doc).
struct DialectNav {
    collect_calls: fn(&[serde_json::Value], &mut CallMap, bool),
    filter_message: fn(&mut serde_json::Value, &mut Filter, &CallMap),
}

const ANTHROPIC_NAV: DialectNav = DialectNav {
    collect_calls,
    filter_message,
};

const OPENAI_NAV: DialectNav = DialectNav {
    collect_calls: crate::pipeline::ingest_openai::collect_calls,
    filter_message: crate::pipeline::ingest_openai::filter_message,
};

#[async_trait::async_trait]
impl Transform for IngestFilterTransform {
    fn name(&self) -> &'static str {
        "ingest_filter"
    }

    async fn on_request(&self, req: &mut InterceptedRequest) -> AppResult<()> {
        // Dialect selection: the per-message walk shape differs (Anthropic
        // content blocks vs OpenAI `role: "tool"` messages), the Filter
        // machinery is shared verbatim. See ingest_openai's module docs.
        let nav = match req.provider {
            Provider::Anthropic => ANTHROPIC_NAV,
            Provider::Openai => OPENAI_NAV,
            _ => return Ok(()),
        };
        // Context guard: if the request is near the provider's context window,
        // force aggressive compression so it fits — even if compression is off or
        // light by default. Prevents outright "prompt is too long" rejections.
        let est = if self.guard.limit > 0 {
            estimate_context_tokens(&req.body, &self.guard)
        } else {
            0
        };
        let guard_fired = self.guard.triggers(est);
        let mode = if guard_fired {
            Mode::LevelFilter(
                thresholds(CompressionLevel::Aggressive).expect("aggressive has thresholds"),
            )
        } else {
            let Some(mode) = self.mode() else {
                return Ok(());
            };
            mode
        };
        let Some(messages) = req
            .body
            .as_object_mut()
            .and_then(|o| o.get_mut("messages"))
            .and_then(|m| m.as_array_mut())
        else {
            return Ok(());
        };
        // Pre-pass: map tool_use_id → originating call, so a tool_result's dedup
        // marker can name its call (read-only over the same history).
        let mut calls = CallMap::new();
        // Origin labels are only ever printed by DedupOnly; LevelFilter (the
        // common mode) would parse `input`/`function.arguments` bodies for a
        // label it discards.
        (nav.collect_calls)(messages, &mut calls, matches!(mode, Mode::DedupOnly));

        let mut f = Filter {
            mode,
            seen: HashMap::new(),
            saved: 0,
        };
        // Whole history, in order. Earlier-first ordering is what makes the
        // backward-only dedup order-stable (hence cache-safe).
        for msg in messages.iter_mut() {
            (nav.filter_message)(msg, &mut f, &calls);
        }
        if guard_fired {
            tracing::warn!(
                est_tokens = est,
                limit = self.guard.limit,
                tokens_saved = f.saved,
                "context guard: escalated to aggressive compression near the provider context limit"
            );
        } else if f.saved > 0 {
            tracing::info!(
                tokens_saved = f.saved,
                dedup_only = self.is_dedup_only(),
                "ingest_filter trimmed tool_result / tool_use blocks"
            );
        }
        req.record_saving(self.name(), f.saved);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Provider;

    const BAL: Thresholds = Thresholds {
        max_tokens: 1500,
        head: 60,
        tail: 20,
    };

    fn lines(n: usize) -> String {
        (0..n)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A tool_result-bearing user message (string-content form).
    fn tr_msg(id: &str, content: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "role": "user",
            "content": [ { "type": "tool_result", "tool_use_id": id, "content": content } ]
        })
    }

    /// A tool_use-bearing assistant message with the given input object.
    fn tu_msg(id: &str, name: &str, input: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "role": "assistant",
            "content": [ { "type": "tool_use", "id": id, "name": name, "input": input } ]
        })
    }

    fn req(provider: Provider, body: serde_json::Value) -> InterceptedRequest {
        InterceptedRequest {
            provider,
            path: "/v1/messages".into(),
            model: "m".into(),
            stream: false,
            client: None,
            project: None,
            session: None,
            api_key_hash: None,
            tokens_saved: 0,
            saved_by_stage: Vec::new(),
            routed_from: None,
            request_class: None,
            user_id: None,
            ccr: Default::default(),
            ccr_retrieve_calls: 0,
            pipeline_ms: 0,
            budget_reserved: 0.0,
            team_budget_reserved: 0.0,
            team_budget_reserved_at: None,
            body,
        }
    }

    // ---- cache-safety ----------------------------------------------------

    /// Dedup must keep the SHARED PREFIX byte-identical as the conversation grows,
    /// or it would break the prompt cache (the dominant saving) every turn. Here a
    /// big block is repeated; later copies collapse to a back-reference to the
    /// first. Appending more turns must not perturb any earlier message.
    #[tokio::test]
    async fn dedup_prefix_is_byte_stable_across_growing_turns() {
        let filter = IngestFilterTransform::dedup_only();
        let big = lines(2000); // well over the dedup size floor
        let base = vec![
            serde_json::json!({ "role": "user", "content": "go" }),
            tu_msg("a", "read", serde_json::json!({})),
            tr_msg("a", serde_json::json!(big)),
            tu_msg("b", "read", serde_json::json!({})),
            tr_msg("b", serde_json::json!(big)), // exact repeat → dedup target
        ];
        // Next turn = same history + one more identical repeat.
        let mut grown = base.clone();
        grown.push(tu_msg("c", "read", serde_json::json!({})));
        grown.push(tr_msg("c", serde_json::json!(big)));

        let mut r_n = req(Provider::Anthropic, serde_json::json!({ "messages": base }));
        let mut r_n1 = req(
            Provider::Anthropic,
            serde_json::json!({ "messages": grown }),
        );
        filter.on_request(&mut r_n).await.unwrap();
        filter.on_request(&mut r_n1).await.unwrap();

        let a = r_n.body["messages"].as_array().unwrap();
        let b = r_n1.body["messages"].as_array().unwrap();
        // Something must actually have been dedup'd (test isn't vacuous)…
        assert!(r_n.tokens_saved > 0, "the repeated block was dedup'd");
        // …and every message of the shorter turn is byte-identical in the longer.
        for (i, msg) in a.iter().enumerate() {
            assert_eq!(
                *msg, b[i],
                "message {i} diverged as the turn grew → cache break"
            );
        }
    }

    /// The lossy level-based path (truncation) must ALSO keep the shared prefix
    /// byte-stable across turns — truncation rewrites each block as a pure function
    /// of its own content, so growing the conversation cannot perturb earlier
    /// (already-truncated) blocks and the prompt cache keeps hitting.
    #[tokio::test]
    async fn truncation_prefix_is_byte_stable_across_growing_turns() {
        let filter = IngestFilterTransform::fixed(CompressionLevel::Balanced);
        // Distinct big blocks (so this exercises truncation, not dedup).
        let a1 = lines(2000);
        let a2 = (0..2000)
            .map(|i| format!("other-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let base = vec![
            serde_json::json!({ "role": "user", "content": "go" }),
            tu_msg("a", "read", serde_json::json!({})),
            tr_msg("a", serde_json::json!(a1)),
            tu_msg("b", "read", serde_json::json!({})),
            tr_msg("b", serde_json::json!(a2)),
        ];
        let mut grown = base.clone();
        grown.push(tu_msg("c", "read", serde_json::json!({})));
        grown.push(tr_msg("c", serde_json::json!(lines(1800))));

        let mut r_n = req(Provider::Anthropic, serde_json::json!({ "messages": base }));
        let mut r_n1 = req(
            Provider::Anthropic,
            serde_json::json!({ "messages": grown }),
        );
        filter.on_request(&mut r_n).await.unwrap();
        filter.on_request(&mut r_n1).await.unwrap();

        let a = r_n.body["messages"].as_array().unwrap();
        let b = r_n1.body["messages"].as_array().unwrap();
        assert!(r_n.tokens_saved > 0, "the big blocks were truncated");
        for (i, msg) in a.iter().enumerate() {
            assert_eq!(
                *msg, b[i],
                "truncated message {i} diverged as the turn grew → cache break"
            );
        }
    }

    // ---- truncation ------------------------------------------------------

    #[test]
    fn truncate_keeps_head_tail_and_marker() {
        let text = lines(2000); // far over 1500 tokens, 2000 lines
        let out = truncate(&text, BAL);
        let out_lines: Vec<&str> = out.split('\n').collect();
        assert_eq!(out_lines[0], "line-0", "head preserved");
        assert_eq!(out_lines[1], "line-1");
        assert!(out.contains(TRUNC_MARKER_TAG), "marker present: {out:?}");
        assert!(out.contains("line-1999"), "tail preserved");
        // The marker reports the right count of omitted lines.
        assert!(
            out.contains("[… 1920 lines omitted by rtk-proxy …]"),
            "{out}"
        );
        assert!(
            crate::tokenizer::count(&out) < crate::tokenizer::count(&text),
            "truncation must shrink"
        );
    }

    #[test]
    fn truncate_under_threshold_is_untouched() {
        let text = "small\noutput\nhere";
        assert_eq!(truncate(text, BAL), text);
    }

    #[test]
    fn truncate_is_idempotent() {
        let text = lines(2000);
        let once = truncate(&text, BAL);
        let twice = truncate(&once, BAL);
        assert_eq!(once, twice, "re-truncating filtered text is a no-op");
    }

    #[test]
    fn truncate_few_lines_huge_is_untouched_by_line_count() {
        // Over the token budget but only 3 lines: head+tail covers it, no gain.
        let text = format!("{}\nmid\n{}", "x".repeat(8000), "y".repeat(8000));
        assert_eq!(truncate(&text, BAL), text);
    }

    // ---- dedup -----------------------------------------------------------

    /// A level mode NEVER dedups tool_results into a marker — even exact repeats
    /// are left verbatim (a marker would corrupt an agent that copies the content
    /// into an edit). Dedup only happens in the opt-in DedupOnly mode.
    #[tokio::test]
    async fn level_mode_never_dedups_tool_results() {
        let body = serde_json::json!({
            "messages": [
                tr_msg("a", serde_json::Value::String("same exact output".into())),
                { "role": "assistant", "content": "ok" },
                tr_msg("b", serde_json::Value::String("same exact output".into()))
            ]
        });
        let t = IngestFilterTransform::fixed(CompressionLevel::Balanced);
        let mut r = req(Provider::Anthropic, body);
        t.on_request(&mut r).await.unwrap();
        // Both kept verbatim — no marker anywhere.
        assert_eq!(
            r.body["messages"][0]["content"][0]["content"],
            "same exact output"
        );
        let second = r.body["messages"][2]["content"][0]["content"]
            .as_str()
            .unwrap();
        assert!(
            !second.contains(DEDUP_MARKER_TAG),
            "no dedup marker in level mode: {second}"
        );
        assert_eq!(second, "same exact output", "second kept verbatim");
    }

    #[tokio::test]
    async fn unique_blocks_left_alone() {
        let body = serde_json::json!({
            "messages": [
                tr_msg("a", serde_json::Value::String("first output".into())),
                tr_msg("b", serde_json::Value::String("second output".into()))
            ]
        });
        let t = IngestFilterTransform::fixed(CompressionLevel::Balanced);
        let mut r = req(Provider::Anthropic, body);
        t.on_request(&mut r).await.unwrap();
        assert_eq!(r.tokens_saved, 0);
        assert_eq!(
            r.body["messages"][0]["content"][0]["content"],
            "first output"
        );
        assert_eq!(
            r.body["messages"][1]["content"][0]["content"],
            "second output"
        );
    }

    // ---- scope -----------------------------------------------------------

    #[tokio::test]
    async fn off_is_noop() {
        let big = lines(2000);
        let body = serde_json::json!({
            "messages": [ tr_msg("a", serde_json::Value::String(big.clone())) ]
        });
        let t = IngestFilterTransform::fixed(CompressionLevel::Off);
        let mut r = req(Provider::Anthropic, body);
        t.on_request(&mut r).await.unwrap();
        assert_eq!(r.body["messages"][0]["content"][0]["content"], big);
        assert_eq!(r.tokens_saved, 0);
    }

    #[tokio::test]
    async fn unsupported_dialect_is_untouched() {
        let big = lines(2000);
        let body = serde_json::json!({
            "messages": [ tr_msg("a", serde_json::Value::String(big.clone())) ]
        });
        let t = IngestFilterTransform::fixed(CompressionLevel::Balanced);
        let mut r = req(Provider::Google, body);
        t.on_request(&mut r).await.unwrap();
        assert_eq!(r.body["messages"][0]["content"][0]["content"], big);
        assert_eq!(r.tokens_saved, 0);
    }

    #[tokio::test]
    async fn leaves_plain_text_untouched_but_filters_tool_use() {
        // A huge plain text block must NOT be filtered (only tool_result and
        // tool_use are in scope); the tool_use big input value IS truncated.
        let big = lines(2000);
        let body = serde_json::json!({
            "messages": [
                { "role": "user", "content": [ { "type": "text", "text": big.clone() } ] },
                { "role": "assistant", "content": [
                    { "type": "tool_use", "id": "t", "name": "x", "input": { "q": big.clone() } }
                ] }
            ]
        });
        let t = IngestFilterTransform::fixed(CompressionLevel::Aggressive);
        let mut r = req(Provider::Anthropic, body);
        t.on_request(&mut r).await.unwrap();
        // Plain text block left verbatim.
        assert_eq!(r.body["messages"][0]["content"][0]["text"], big);
        // tool_use big value truncated; saving recorded.
        assert!(
            r.body["messages"][1]["content"][0]["input"]["q"]
                .as_str()
                .unwrap()
                .contains(TRUNC_MARKER_TAG)
        );
        assert!(r.tokens_saved > 0, "tool_use input truncation saved tokens");
    }

    #[tokio::test]
    async fn array_text_content_form_is_truncated() {
        let big = lines(2000);
        let body = serde_json::json!({
            "messages": [ tr_msg("a", serde_json::json!([
                { "type": "text", "text": big }
            ])) ]
        });
        let t = IngestFilterTransform::fixed(CompressionLevel::Balanced);
        let mut r = req(Provider::Anthropic, body);
        t.on_request(&mut r).await.unwrap();
        let got = r.body["messages"][0]["content"][0]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert!(got.contains(TRUNC_MARKER_TAG), "array-text form truncated");
        assert!(r.tokens_saved > 0);
    }

    // ---- tool_use input truncation --------------------------------------

    /// A Write-style tool_use with a huge `content` value gets that value
    /// truncated, while the tool name, file_path, and other small fields stay
    /// byte-identical.
    #[tokio::test]
    async fn tool_use_large_value_truncated_small_fields_untouched() {
        let big = lines(2000); // far over threshold, many lines
        let body = serde_json::json!({
            "messages": [
                tu_msg("u1", "Write", serde_json::json!({
                    "file_path": "/repo/src/main.rs",
                    "content": big.clone(),
                }))
            ]
        });
        let t = IngestFilterTransform::fixed(CompressionLevel::Balanced);
        let mut r = req(Provider::Anthropic, body);
        t.on_request(&mut r).await.unwrap();

        let content = r.body["messages"][0]["content"][0]["input"]["content"]
            .as_str()
            .unwrap();
        assert!(
            content.contains(TRUNC_MARKER_TAG),
            "big content value truncated"
        );
        assert!(content.contains("line-0"), "head kept");
        assert!(content.contains("line-1999"), "tail kept");
        // Small fields and structure untouched.
        assert_eq!(
            r.body["messages"][0]["content"][0]["input"]["file_path"], "/repo/src/main.rs",
            "file_path untouched"
        );
        assert_eq!(
            r.body["messages"][0]["content"][0]["name"], "Write",
            "name untouched"
        );
        assert_eq!(
            r.body["messages"][0]["content"][0]["id"], "u1",
            "id untouched"
        );
        assert!(r.tokens_saved > 0);
    }

    /// Edit-style tool_use: both `old_string` and `new_string` get truncated
    /// when large; small fields are not.
    #[tokio::test]
    async fn tool_use_edit_old_and_new_string_truncated() {
        let big_old = lines(1800);
        let big_new = lines(2200);
        let body = serde_json::json!({
            "messages": [
                tu_msg("u1", "Edit", serde_json::json!({
                    "file_path": "/a/b.rs",
                    "old_string": big_old,
                    "new_string": big_new,
                }))
            ]
        });
        let t = IngestFilterTransform::fixed(CompressionLevel::Balanced);
        let mut r = req(Provider::Anthropic, body);
        t.on_request(&mut r).await.unwrap();
        let inp = &r.body["messages"][0]["content"][0]["input"];
        assert!(
            inp["old_string"]
                .as_str()
                .unwrap()
                .contains(TRUNC_MARKER_TAG)
        );
        assert!(
            inp["new_string"]
                .as_str()
                .unwrap()
                .contains(TRUNC_MARKER_TAG)
        );
        assert_eq!(inp["file_path"], "/a/b.rs", "file_path untouched");
    }

    /// A small tool_use input (e.g. a Bash command) is left completely alone,
    /// including single-line values that exceed the byte budget but have no
    /// lines to drop.
    #[tokio::test]
    async fn tool_use_small_and_single_line_values_untouched() {
        let one_huge_line = "x".repeat(8000); // > budget, but a single line
        let body = serde_json::json!({
            "messages": [
                tu_msg("u1", "Bash", serde_json::json!({
                    "command": "ls -la /tmp",
                    "description": "list temp",
                })),
                tu_msg("u2", "Bash", serde_json::json!({ "command": one_huge_line.clone() })),
            ]
        });
        let t = IngestFilterTransform::fixed(CompressionLevel::Balanced);
        let before = serde_json::json!({
            "messages": [
                tu_msg("u1", "Bash", serde_json::json!({
                    "command": "ls -la /tmp",
                    "description": "list temp",
                })),
                tu_msg("u2", "Bash", serde_json::json!({ "command": one_huge_line })),
            ]
        });
        let mut r = req(Provider::Anthropic, body);
        t.on_request(&mut r).await.unwrap();
        assert_eq!(r.body["messages"], before["messages"], "no value changed");
        assert_eq!(r.tokens_saved, 0);
    }

    /// Re-running the filter over an already-truncated tool_use value is a no-op.
    #[tokio::test]
    async fn tool_use_truncation_is_idempotent() {
        let big = lines(2000);
        let body = serde_json::json!({
            "messages": [ tu_msg("u1", "Write", serde_json::json!({ "content": big })) ]
        });
        let t = IngestFilterTransform::fixed(CompressionLevel::Balanced);
        let mut r = req(Provider::Anthropic, body);
        t.on_request(&mut r).await.unwrap();
        let after_first = r.body.clone();
        let saved_first = r.tokens_saved;
        t.on_request(&mut r).await.unwrap();
        assert_eq!(r.body, after_first, "second pass must not change the body");
        assert_eq!(
            r.tokens_saved, saved_first,
            "second pass saves nothing more"
        );
    }

    // ---- THE cache-safety guarantee -------------------------------------

    /// Filtering a history, then filtering an EXTENDED history (same prefix +
    /// new blocks appended), must yield a byte-identical filtered prefix. This
    /// is exactly what keeps the provider prompt-cache prefix stable across
    /// turns. Covers ALL paths: a large tool_result that gets truncated, a
    /// duplicate tool_result that gets dedup'd, AND a tool_use block whose big
    /// input value gets truncated — the decision for every prefix block must not
    /// change when later turns are appended.
    #[tokio::test]
    async fn determinism_extended_history_keeps_prefix_identical() {
        let big = lines(2000); // truncated tool_result
        let dup = "exact duplicate output".to_string(); // small; dedup target
        let tu_big = lines(1900); // truncated tool_use input value

        // Turn N: a big tool_result, a tool_use with a big input value, then a
        // first copy of `dup`, then a second copy.
        let prefix = vec![
            tr_msg("a", serde_json::Value::String(big.clone())),
            tu_msg(
                "u1",
                "Write",
                serde_json::json!({
                    "file_path": "/x/y.rs",
                    "content": tu_big.clone(),
                }),
            ),
            {
                let mut m = tr_msg("b", serde_json::Value::String(dup.clone()));
                m["role"] = "user".into();
                m
            },
            tr_msg("c", serde_json::Value::String(dup.clone())),
        ];

        let mut short = req(
            Provider::Anthropic,
            serde_json::json!({ "messages": prefix.clone() }),
        );

        // Turn N+1: SAME prefix + new blocks appended. `big2` is a DISTINCT large
        // block (so it truncates, not dedups); a NEW tool_use with another big
        // value; `dup` repeats the earlier one (so it dedups against the prefix).
        let big2 = lines(2100);
        let tu_big2 = lines(1700);
        let mut extended_msgs = prefix.clone();
        extended_msgs.push(tr_msg("d", serde_json::Value::String(big2.clone())));
        extended_msgs.push(tu_msg(
            "u2",
            "Write",
            serde_json::json!({
                "file_path": "/x/z.rs",
                "content": tu_big2.clone(),
            }),
        ));
        extended_msgs.push(tr_msg("e", serde_json::Value::String(dup.clone())));
        let mut long = req(
            Provider::Anthropic,
            serde_json::json!({ "messages": extended_msgs }),
        );

        let t = IngestFilterTransform::fixed(CompressionLevel::Balanced);
        t.on_request(&mut short).await.unwrap();
        t.on_request(&mut long).await.unwrap();

        // The first 4 messages (the shared prefix, incl. the tool_use) must be
        // byte-identical across turns.
        for i in 0..4 {
            assert_eq!(
                short.body["messages"][i], long.body["messages"][i],
                "prefix message {i} diverged between turns — cache would break"
            );
        }
        // The prefix tool_use's big value was actually truncated (sanity).
        assert!(
            short.body["messages"][1]["content"][0]["input"]["content"]
                .as_str()
                .unwrap()
                .contains(TRUNC_MARKER_TAG)
        );
        // The newly appended distinct big block is truncated (filter works on the tail).
        let appended = long.body["messages"][4]["content"][0]["content"]
            .as_str()
            .unwrap();
        assert!(appended.contains(TRUNC_MARKER_TAG));
        // The appended new tool_use's big value is truncated too.
        assert!(
            long.body["messages"][5]["content"][0]["input"]["content"]
                .as_str()
                .unwrap()
                .contains(TRUNC_MARKER_TAG)
        );
        // The appended small dup is left VERBATIM — a level mode never dedups
        // (a marker would corrupt an agent that copies the content into an edit).
        assert_eq!(
            long.body["messages"][6]["content"][0]["content"], "exact duplicate output",
            "small repeats stay verbatim in a level mode (no dedup marker)"
        );
    }

    /// Idempotency at the transform level: running the filter twice over the
    /// same (already-filtered) body changes nothing the second time.
    #[tokio::test]
    async fn transform_is_idempotent() {
        let big = lines(2000);
        let body = serde_json::json!({
            "messages": [
                tr_msg("a", serde_json::Value::String(big.clone())),
                tr_msg("b", serde_json::Value::String(big.clone()))
            ]
        });
        let t = IngestFilterTransform::fixed(CompressionLevel::Balanced);
        let mut r = req(Provider::Anthropic, body);
        t.on_request(&mut r).await.unwrap();
        let after_first = r.body.clone();
        let saved_first = r.tokens_saved;
        t.on_request(&mut r).await.unwrap();
        assert_eq!(r.body, after_first, "second pass must not change the body");
        assert_eq!(
            r.tokens_saved, saved_first,
            "second pass must save nothing more"
        );
    }

    // ---- identifying dedup marker ---------------------------------------

    /// The marker is built from the block's own content (prefix + line count) so
    /// it identifies WHICH original it refers to, and is universal (no tool
    /// context needed). With an `origin` it additionally names the call.
    #[test]
    fn dedup_marker_is_identifying() {
        let block = "fn main() {\n    let config = Config::load();\n    run(config);\n}";
        let m = dedup_marker(block, None);
        assert!(m.contains(DEDUP_MARKER_TAG), "{m}");
        assert!(m.contains("4-line"), "carries the line count: {m}");
        assert!(m.contains("fn main() {"), "carries the content prefix: {m}");
        // Newlines in the prefix are escaped so the marker stays one line.
        assert!(!m.contains('\n'), "marker is single-line: {m:?}");

        // tool_result variant prefixes the originating call for readability.
        let m2 = dedup_marker(block, Some("Read(/src/main.rs)"));
        assert!(m2.contains("Read(/src/main.rs)"), "names the call: {m2}");
        assert!(
            m2.contains("fn main() {"),
            "still carries the content prefix"
        );
    }

    /// Two DIFFERENT blocks produce DIFFERENT identifying markers (the prefix or
    /// the line count discriminates), so a back-reference is never ambiguous in
    /// the normal case.
    #[test]
    fn dedup_marker_distinguishes_different_blocks() {
        let a = lines(300);
        let b = lines(340); // different line count
        let c = format!("DIFFERENT START\n{}", lines(299)); // different prefix, same len-ish
        assert_ne!(dedup_marker(&a, None), dedup_marker(&b, None));
        assert_ne!(dedup_marker(&a, None), dedup_marker(&c, None));
    }

    // ---- dedup-only mode -------------------------------------------------

    /// A reusable big block (well over the 200-token floor).
    fn big_block(tag: &str) -> String {
        // ~600 lines, each tagged so distinct tags yield distinct content.
        (0..600)
            .map(|i| format!("{tag}-line-{i}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Dedup-only NEVER truncates: a single huge unique tool_result is left
    /// byte-identical (no truncation marker), and nothing is saved.
    #[tokio::test]
    async fn dedup_only_never_truncates() {
        let big = big_block("solo");
        let body = serde_json::json!({
            "messages": [ tr_msg("a", serde_json::Value::String(big.clone())) ]
        });
        let t = IngestFilterTransform::dedup_only();
        let mut r = req(Provider::Anthropic, body);
        t.on_request(&mut r).await.unwrap();
        assert_eq!(
            r.body["messages"][0]["content"][0]["content"], big,
            "unique block kept verbatim — never truncated"
        );
        assert!(
            !r.body["messages"][0]["content"][0]["content"]
                .as_str()
                .unwrap()
                .contains(TRUNC_MARKER_TAG)
        );
        assert_eq!(r.tokens_saved, 0);
    }

    /// Dedup-only removes an EXACT repeat of an earlier large tool_result,
    /// replacing the later copy with the identifying marker (and naming the call).
    #[tokio::test]
    async fn dedup_only_dedups_exact_repeat_with_identifying_marker() {
        let big = big_block("repeat");
        let body = serde_json::json!({
            "messages": [
                tu_msg("call1", "Read", serde_json::json!({ "file_path": "/src/main.rs" })),
                tr_msg("call1", serde_json::Value::String(big.clone())),
                tu_msg("call2", "Read", serde_json::json!({ "file_path": "/src/main.rs" })),
                tr_msg("call2", serde_json::Value::String(big.clone())),
            ]
        });
        let t = IngestFilterTransform::dedup_only();
        let mut r = req(Provider::Anthropic, body);
        t.on_request(&mut r).await.unwrap();
        // First copy kept verbatim.
        assert_eq!(r.body["messages"][1]["content"][0]["content"], big);
        // Second copy → identifying marker that names the call + content prefix.
        let second = r.body["messages"][3]["content"][0]["content"]
            .as_str()
            .unwrap();
        assert!(second.contains(DEDUP_MARKER_TAG), "{second}");
        assert!(
            second.contains("Read(/src/main.rs)"),
            "names origin: {second}"
        );
        assert!(
            second.contains("repeat-line-0"),
            "carries content prefix: {second}"
        );
        assert!(r.tokens_saved > 0);
    }

    /// Dedup-only dedups a repeated PLAIN TEXT/code block too (not just tools).
    #[tokio::test]
    async fn dedup_only_dedups_plain_text_block() {
        let big = big_block("prose");
        let text_msg = |t: &str| {
            serde_json::json!({
                "role": "user",
                "content": [ { "type": "text", "text": t } ]
            })
        };
        let body = serde_json::json!({
            "messages": [ text_msg(&big), text_msg(&big) ]
        });
        let t = IngestFilterTransform::dedup_only();
        let mut r = req(Provider::Anthropic, body);
        t.on_request(&mut r).await.unwrap();
        assert_eq!(
            r.body["messages"][0]["content"][0]["text"], big,
            "first kept"
        );
        let second = r.body["messages"][1]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert!(
            second.contains(DEDUP_MARKER_TAG),
            "plain text dedup'd: {second}"
        );
        assert!(r.tokens_saved > 0);
    }

    /// Dedup-only dedups a repeated tool_use input VALUE (the echoed file body).
    #[tokio::test]
    async fn dedup_only_dedups_tool_use_value() {
        let big = big_block("body");
        let body = serde_json::json!({
            "messages": [
                tr_msg("a", serde_json::Value::String(big.clone())),
                tu_msg("u1", "Write", serde_json::json!({
                    "file_path": "/a.rs",
                    "content": big.clone(),
                })),
            ]
        });
        let t = IngestFilterTransform::dedup_only();
        let mut r = req(Provider::Anthropic, body);
        t.on_request(&mut r).await.unwrap();
        // tool_result kept (first occurrence), tool_use value dedup'd (repeat).
        assert_eq!(r.body["messages"][0]["content"][0]["content"], big);
        let echoed = r.body["messages"][1]["content"][0]["input"]["content"]
            .as_str()
            .unwrap();
        assert!(
            echoed.contains(DEDUP_MARKER_TAG),
            "echoed body dedup'd: {echoed}"
        );
        // file_path untouched.
        assert_eq!(
            r.body["messages"][1]["content"][0]["input"]["file_path"],
            "/a.rs"
        );
    }

    /// The guard estimator must not be fooled by dense text: bytes/4
    /// under-counts code/JSON several-fold, so near the threshold the exact
    /// tokenizer path takes over and reports the real count.
    #[test]
    fn guard_estimator_uses_tokenizer_near_the_limit() {
        // Dense, symbol-heavy text: real tokens per byte far above 1/4.
        let dense = "x=1;y+=2;fn(a,b)->{q}|&z^ ".repeat(400);
        let body =
            serde_json::json!({ "messages": [ { "role": "user", "content": dense.clone() } ] });
        let real = crate::tokenizer::count(&dense) as u64;
        let bytes4 = (dense.len() / 4) as u64;
        assert!(
            real > bytes4,
            "premise: dense text under-counted by bytes/4 ({real} vs {bytes4})"
        );
        // Threshold low enough that the byte gate cannot rule the body out →
        // the exact path runs and returns the true count.
        let g = ContextGuard {
            limit: 100,
            ratio: 1.0,
        };
        let est = estimate_context_tokens(&body, &g);
        // The walk also counts the tiny "role" value — allow a few tokens' slack.
        assert!(
            est >= real && est <= real + 10,
            "near the limit the estimate is the real tokenizer count ({est} vs {real})"
        );
        // Far below the threshold the cheap path answers (fast, approximate).
        let g_far = ContextGuard {
            limit: 100_000_000,
            ratio: 0.95,
        };
        let est_cheap = estimate_context_tokens(&body, &g_far);
        assert!(
            est_cheap >= bytes4 && est_cheap <= bytes4 + 10,
            "far from the limit the byte estimate answers ({est_cheap} vs {bytes4})"
        );
    }

    /// The context guard rescues a near-limit request: even with compression
    /// OFF, a request estimated near the provider window is aggressively
    /// compressed so it fits instead of failing "prompt is too long".
    #[tokio::test]
    async fn context_guard_forces_compression_when_off_but_near_limit() {
        let big = big_block("x");
        let body = serde_json::json!({
            "messages": [ tr_msg("a", serde_json::Value::String(big.clone())) ]
        });
        // Level Off (normally a no-op) + a tiny guard limit → guard fires.
        let t = IngestFilterTransform::fixed_with_guard(
            CompressionLevel::Off,
            ContextGuard {
                limit: 1,
                ratio: 1.0,
            },
        );
        let mut r = req(Provider::Anthropic, body);
        t.on_request(&mut r).await.unwrap();
        assert!(
            r.tokens_saved > 0,
            "guard forced compression despite level Off"
        );
        let out = r.body["messages"][0]["content"][0]["content"]
            .as_str()
            .unwrap();
        assert!(
            out.len() < big.len(),
            "big tool_result truncated by the guard"
        );
    }

    /// With the guard disabled (limit 0) and level Off, the transform is a no-op.
    #[tokio::test]
    async fn context_guard_disabled_is_noop_when_off() {
        let big = big_block("x");
        let body = serde_json::json!({
            "messages": [ tr_msg("a", serde_json::Value::String(big.clone())) ]
        });
        let t = IngestFilterTransform::fixed_with_guard(
            CompressionLevel::Off,
            ContextGuard {
                limit: 0,
                ratio: 0.95,
            },
        );
        let mut r = req(Provider::Anthropic, body);
        t.on_request(&mut r).await.unwrap();
        assert_eq!(r.tokens_saved, 0, "guard off + level off = no-op");
    }

    /// A tool_use input value is NEVER turned into a dedup marker in level mode:
    /// coding agents copy those values verbatim into file operations, so a
    /// back-reference marker there would corrupt the edit. Truncation is allowed;
    /// dedup markers are not.
    #[tokio::test]
    async fn balanced_never_dedups_tool_use_value_into_a_marker() {
        let big = big_block("body");
        let body = serde_json::json!({
            "messages": [
                tr_msg("a", serde_json::Value::String(big.clone())),
                tu_msg("u1", "Write", serde_json::json!({
                    "file_path": "/a.rs",
                    "content": big.clone(),
                })),
            ]
        });
        let t = IngestFilterTransform::fixed(CompressionLevel::Balanced);
        let mut r = req(Provider::Anthropic, body);
        t.on_request(&mut r).await.unwrap();
        let echoed = r.body["messages"][1]["content"][0]["input"]["content"]
            .as_str()
            .unwrap();
        assert!(
            !echoed.contains(DEDUP_MARKER_TAG),
            "tool_use value must not become a marker: {echoed}"
        );
        assert_eq!(
            r.body["messages"][1]["content"][0]["input"]["file_path"],
            "/a.rs"
        );
    }

    /// Tiny repeats below the 200-token floor are NOT touched in dedup-only mode.
    #[tokio::test]
    async fn dedup_only_ignores_small_repeats() {
        let body = serde_json::json!({
            "messages": [
                tr_msg("a", serde_json::Value::String("OK".into())),
                tr_msg("b", serde_json::Value::String("OK".into())),
            ]
        });
        let t = IngestFilterTransform::dedup_only();
        let mut r = req(Provider::Anthropic, body);
        t.on_request(&mut r).await.unwrap();
        assert_eq!(r.body["messages"][0]["content"][0]["content"], "OK");
        assert_eq!(
            r.body["messages"][1]["content"][0]["content"], "OK",
            "tiny repeat kept"
        );
        assert_eq!(r.tokens_saved, 0);
    }

    // ---- anti-ambiguity fallback ----------------------------------------

    /// Two DIFFERENT blocks that share the same identifier (same first 2 lines AND
    /// same line count, differing only later) must NOT both be dedup'd: the second
    /// is kept verbatim, because the marker would otherwise be ambiguous.
    #[tokio::test]
    async fn ambiguity_fallback_keeps_colliding_block() {
        // Same first two lines + same line count, but the BODY differs.
        let head = "HEADER LINE ONE\nHEADER LINE TWO";
        let block_a = format!(
            "{head}\n{}",
            (0..400)
                .map(|i| format!("A{i}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let block_b = format!(
            "{head}\n{}",
            (0..400)
                .map(|i| format!("B{i}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        // Sanity: same identifier, different content.
        assert_eq!(dedup_marker(&block_a, None), dedup_marker(&block_b, None));
        assert_ne!(block_a, block_b);

        let body = serde_json::json!({
            "messages": [
                tr_msg("a", serde_json::Value::String(block_a.clone())),
                tr_msg("b", serde_json::Value::String(block_b.clone())),
            ]
        });
        let t = IngestFilterTransform::dedup_only();
        let mut r = req(Provider::Anthropic, body);
        t.on_request(&mut r).await.unwrap();
        assert_eq!(
            r.body["messages"][0]["content"][0]["content"], block_a,
            "first kept"
        );
        assert_eq!(
            r.body["messages"][1]["content"][0]["content"], block_b,
            "colliding-but-different block kept verbatim (no ambiguous dedup)"
        );
        assert_eq!(r.tokens_saved, 0, "nothing dedup'd on collision");
    }

    /// But a TRUE exact repeat after a collision is still recognized: the third
    /// block (identical to the FIRST) is dedup'd; the colliding second is kept.
    #[tokio::test]
    async fn ambiguity_fallback_still_dedups_true_repeat() {
        let head = "HEADER LINE ONE\nHEADER LINE TWO";
        let block_a = format!(
            "{head}\n{}",
            (0..400)
                .map(|i| format!("A{i}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let block_b = format!(
            "{head}\n{}",
            (0..400)
                .map(|i| format!("B{i}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let body = serde_json::json!({
            "messages": [
                tr_msg("a", serde_json::Value::String(block_a.clone())),
                tr_msg("b", serde_json::Value::String(block_b.clone())),
                tr_msg("c", serde_json::Value::String(block_a.clone())),
            ]
        });
        let t = IngestFilterTransform::dedup_only();
        let mut r = req(Provider::Anthropic, body);
        t.on_request(&mut r).await.unwrap();
        assert_eq!(
            r.body["messages"][0]["content"][0]["content"], block_a,
            "first kept"
        );
        assert_eq!(
            r.body["messages"][1]["content"][0]["content"], block_b,
            "collision kept"
        );
        let third = r.body["messages"][2]["content"][0]["content"]
            .as_str()
            .unwrap();
        assert!(
            third.contains(DEDUP_MARKER_TAG),
            "true repeat of first dedup'd: {third}"
        );
    }

    // ---- dedup-only determinism (cache-safety) ---------------------------

    /// Dedup-only: filtering a history then an EXTENDED history (same prefix +
    /// appended turns) yields a byte-identical filtered prefix — the cache-safety
    /// guarantee for the dedup-only lever.
    #[tokio::test]
    async fn dedup_only_determinism_extended_history() {
        let big = big_block("ctx");
        let prefix = vec![tr_msg("a", serde_json::Value::String(big.clone())), {
            let mut m = tr_msg("b", serde_json::Value::String(big.clone()));
            m["role"] = "user".into();
            m
        }];
        let mut short = req(
            Provider::Anthropic,
            serde_json::json!({ "messages": prefix.clone() }),
        );

        let mut extended = prefix.clone();
        extended.push(tr_msg("c", serde_json::Value::String(big_block("other"))));
        extended.push(tr_msg("d", serde_json::Value::String(big.clone()))); // another repeat
        let mut long = req(
            Provider::Anthropic,
            serde_json::json!({ "messages": extended }),
        );

        let t = IngestFilterTransform::dedup_only();
        t.on_request(&mut short).await.unwrap();
        t.on_request(&mut long).await.unwrap();

        for i in 0..prefix.len() {
            assert_eq!(
                short.body["messages"][i], long.body["messages"][i],
                "dedup-only prefix message {i} diverged — cache would break"
            );
        }
        // The prefix's second copy was dedup'd; the appended repeat is dedup'd too.
        assert!(
            short.body["messages"][1]["content"][0]["content"]
                .as_str()
                .unwrap()
                .contains(DEDUP_MARKER_TAG)
        );
        assert!(
            long.body["messages"][3]["content"][0]["content"]
                .as_str()
                .unwrap()
                .contains(DEDUP_MARKER_TAG)
        );
    }

    /// Dedup-only is idempotent: a second pass over already-dedup'd output is a
    /// no-op (the marker tag is the idempotency guard).
    #[tokio::test]
    async fn dedup_only_is_idempotent() {
        let big = big_block("idem");
        let body = serde_json::json!({
            "messages": [
                tr_msg("a", serde_json::Value::String(big.clone())),
                tr_msg("b", serde_json::Value::String(big.clone())),
            ]
        });
        let t = IngestFilterTransform::dedup_only();
        let mut r = req(Provider::Anthropic, body);
        t.on_request(&mut r).await.unwrap();
        let after_first = r.body.clone();
        let saved_first = r.tokens_saved;
        t.on_request(&mut r).await.unwrap();
        assert_eq!(r.body, after_first, "second pass must not change the body");
        assert_eq!(
            r.tokens_saved, saved_first,
            "second pass saves nothing more"
        );
    }
}
