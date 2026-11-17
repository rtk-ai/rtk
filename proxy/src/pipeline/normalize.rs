//! Category A: irreversible-but-inert normalisation of tool results.
//!
//! Unlike [`super::ccr`] this stage emits no marker, injects no tool and keeps
//! no cache: every transform is a pure function of the block's own bytes, so
//! the request is byte-stable across turns and nothing can ever be asked
//! back. That removes the recovery cost CCR carries, which is the reason this
//! stage exists separately.
//!
//! ANSI and CR handling is cheap and correct, though non-TTY output rarely
//! carries either. The two larger transforms are both off by default, because
//! neither is inert: see [`strip_read_line_prefixes`] and
//! [`collapse_duplicate_lines`].

//! Division of labour with the rtk hook — do not duplicate its work.
//!
//! The hook rewrites bash output at EXECUTION time, before it ever enters the
//! conversation, so it pays no cache invalidation at all. Build-log
//! line-class filtering (`[INFO]`/`Downloading`/`Resolved` lines, category B)
//! belongs in the hook, at its better interception point, not here.
//!
//! This stage therefore restricts itself to what the hook structurally cannot
//! reach: `Read` output, which the hook never intercepts, and cross-block
//! relationships, which it cannot see because it processes one command at a
//! time with no memory of earlier output.
//!
//! Anything the hook has already truncated is left alone — see
//! [`hook_truncated`]. Each filter is safe on its own; stacking two lossy
//! passes over the same bytes compounds the loss without either stage knowing,
//! and the line-collapsing transforms would additionally collapse the hook's
//! own marker lines, which repeat across blocks by design.

use crate::error::AppResult;
use crate::model::{InterceptedRequest, Provider};
use crate::pipeline::Transform;
use std::sync::atomic::{AtomicU64, Ordering};

/// Stage name used in logs, metrics and the per-stage savings breakdown.
pub const STAGE: &str = "normalize";

/// Strip ANSI CSI escape sequences. Inert by construction — terminal styling
/// carries no information the model can act on.
///
/// `RTK_PROXY_NORM_ANSI`, default ON.
fn strip_ansi_enabled() -> bool {
    flag("RTK_PROXY_NORM_ANSI", true)
}

/// Collapse carriage-return progress redraws to the final rendered line, and
/// drop trailing whitespace. Both inert: `\r`-overwritten text was never
/// visible and trailing blanks carry nothing.
///
/// `RTK_PROXY_NORM_WHITESPACE`, default ON.
fn whitespace_enabled() -> bool {
    flag("RTK_PROXY_NORM_WHITESPACE", true)
}

/// Collapse a run of IDENTICAL ADJACENT lines to one line plus a count.
/// Adjacent-only is the safe form: it cannot reorder a block or separate a
/// line from its context, and the count preserves the multiplicity the model
/// might reason about. Enabled because it is provably safe.
///
/// `RTK_PROXY_NORM_ADJACENT_DUPS`, default ON.
fn adjacent_dups_enabled() -> bool {
    flag("RTK_PROXY_NORM_ADJACENT_DUPS", true)
}

/// Strip Claude Code's `N\t` line-number prefixes from `Read` results.
///
/// OFF BY DEFAULT, unlike the other category-A transforms, because the
/// prefixes are consumed: agents pass `Read` an `offset` and reference
/// "line N" using the numbers they saw in earlier Read output.
///
/// `RTK_PROXY_NORM_READ_LINE_PREFIX=1` enables.
fn read_prefix_enabled() -> bool {
    flag("RTK_PROXY_NORM_READ_LINE_PREFIX", false)
}

/// Collapse NON-ADJACENT duplicate lines within a block, keeping the first
/// occurrence and appending a count to it.
///
/// OFF BY DEFAULT. Scattered duplicates cannot be removed without changing
/// the block's structure, and ordered output (directory
/// listings, diffs, per-module build sections) can depend on that structure.
///
/// `RTK_PROXY_NORM_DUP_LINES=1` enables.
fn scattered_dups_enabled() -> bool {
    flag("RTK_PROXY_NORM_DUP_LINES", false)
}

static GUARD_MIN_BYTES: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();

pub fn configure_min_size(guard_bytes: Option<usize>) {
    let _ = GUARD_MIN_BYTES.set(guard_bytes);
}

fn below_floor(text: &str) -> bool {
    let env = std::env::var("RTK_PROXY_NORM_MIN_CHARS")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok());
    match (env, GUARD_MIN_BYTES.get().copied().flatten()) {
        (Some(chars), _) => text.chars().count() < chars,
        (None, Some(bytes)) => text.len() < bytes,
        (None, None) => text.chars().count() < 400,
    }
}

fn flag(key: &str, default: bool) -> bool {
    match std::env::var(key) {
        Ok(v) => !matches!(v.trim(), "0" | "false" | "off"),
        Err(_) => default,
    }
}

/// Whether a lossy stage upstream of us (the rtk hook) already truncated this
/// block. Identified by the shared tag every truncating filter must emit; CCR
/// uses the same signal to refuse caching such a body as "the original".
fn hook_truncated(text: &str) -> bool {
    text.contains(crate::pipeline::TRUNCATION_MARKER_TAG)
}

/// Remove ANSI CSI/OSC escape sequences.
fn strip_ansi(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'[' => {
                    // CSI: ESC [ params... final-byte in @-~
                    let mut j = i + 2;
                    while j < bytes.len() && !(0x40..=0x7e).contains(&bytes[j]) {
                        j += 1;
                    }
                    i = (j + 1).min(bytes.len());
                    continue;
                }
                b']' => {
                    // OSC: ESC ] ... BEL or ESC \
                    let mut j = i + 2;
                    while j < bytes.len() && bytes[j] != 0x07 {
                        if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                            j += 1;
                            break;
                        }
                        j += 1;
                    }
                    i = (j + 1).min(bytes.len());
                    continue;
                }
                _ => {}
            }
        }
        // Push one whole UTF-8 char, never a partial sequence.
        let ch_len = utf8_len(bytes[i]);
        let end = (i + ch_len).min(bytes.len());
        out.push_str(&text[i..end]);
        i = end;
    }
    out
}

fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else if b >> 3 == 0b11110 {
        4
    } else {
        1
    }
}

/// Keep only the final rendered segment of each `\r`-overwritten line, and
/// drop trailing whitespace from every line.
fn normalise_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        // A progress redraw is a line containing \r: only what follows the
        // last \r was ever displayed.
        let line = line.strip_suffix('\r').unwrap_or(line);
        let rendered = line.rsplit('\r').next().unwrap_or(line);
        out.push_str(rendered.trim_end());
    }
    out
}

/// Collapse runs of identical adjacent lines. `a a a b` -> `a` + count, `b`.
fn collapse_adjacent(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let mut j = i + 1;
        while j < lines.len() && lines[j] == lines[i] {
            j += 1;
        }
        let run = j - i;
        // Only collapse runs of substantive lines: blank-line runs are
        // structure, not repetition.
        if run >= 3
            && lines[i].trim().len() >= 8
            && !lines[i].contains(crate::pipeline::TRUNCATION_MARKER_TAG)
        {
            out.push(format!("{} [x{run}]", lines[i]));
        } else {
            for line in &lines[i..j] {
                out.push(line.to_string());
            }
        }
        i = j;
    }
    out.join("\n")
}

/// Collapse non-adjacent duplicate lines, keeping the first occurrence.
fn collapse_duplicate_lines(text: &str) -> String {
    use std::collections::HashMap;
    let lines: Vec<&str> = text.split('\n').collect();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for l in &lines {
        if l.trim().len() >= 8 {
            *counts.entry(*l).or_insert(0) += 1;
        }
    }
    let mut emitted: HashMap<&str, bool> = HashMap::new();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for l in &lines {
        let n = counts.get(*l).copied().unwrap_or(1);
        if n < 2 || l.trim().len() < 8 || l.contains(crate::pipeline::TRUNCATION_MARKER_TAG) {
            out.push((*l).to_string());
            continue;
        }
        if emitted.insert(*l, true).is_none() {
            out.push(format!("{l} [x{n}]"));
        }
    }
    out.join("\n")
}

/// Strip `N\t` line-number prefixes, as Claude Code's `Read` emits them
/// (`1\tcontent`). Applied only when the block actually looks numbered —
/// a clear majority of non-blank lines carrying ascending prefixes — so a
/// file whose own content begins with digits and a tab is left alone.
fn strip_read_line_prefixes(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut numbered = 0usize;
    let mut candidates = 0usize;
    for l in &lines {
        if l.trim().is_empty() {
            continue;
        }
        candidates += 1;
        if split_line_prefix(l).is_some() {
            numbered += 1;
        }
    }
    if candidates == 0 || numbered * 4 < candidates * 3 {
        return text.to_string();
    }
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    for l in &lines {
        out.push(split_line_prefix(l).map(|(_, rest)| rest).unwrap_or(l));
    }
    out.join("\n")
}

/// `"  42\tfoo"` -> `Some((42, "foo"))`.
fn split_line_prefix(line: &str) -> Option<(u64, &str)> {
    let (head, rest) = line.split_once('\t')?;
    // `trim_start` already guarantees everything before the digits is
    // whitespace, so verifying the digits is the whole check.
    let digits = head.trim_start();
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((digits.parse().ok()?, rest))
}

/// Normalise one block. `is_read` selects the Read-specific transform.
/// Pure function of `(text, is_read, flags)` — the property that keeps the
/// forwarded bytes identical from turn to turn.
pub fn normalise_block(text: &str, is_read: bool) -> String {
    let mut s = text.to_string();
    // A Read result is the model's view of bytes on disk: any change to it —
    // trailing whitespace, a stray ESC byte, an adjacent duplicate line —
    // makes a later Edit's old_string miss, and the failed edit costs more
    // than the compression saved. Same exemption CCR applies. The one
    // read-specific transform (line-prefix stripping) stays available as an
    // explicit opt-in.
    if is_read {
        if read_prefix_enabled() {
            s = strip_read_line_prefixes(&s);
        }
        return s;
    }
    // ANSI and whitespace are inert on terminal output — they remove bytes
    // that were never displayed — so they stay safe over hook-filtered output.
    if strip_ansi_enabled() && s.as_bytes().contains(&0x1b) {
        s = strip_ansi(&s);
    }
    if whitespace_enabled() {
        s = normalise_whitespace(&s);
    }
    // Everything below REMOVES lines. Never stack that on a block the hook
    // has already cut: the two passes compound, and collapsing would eat the
    // hook's own marker lines, which repeat across blocks by design.
    if hook_truncated(&s) {
        return s;
    }
    if scattered_dups_enabled() {
        s = collapse_duplicate_lines(&s);
    } else if adjacent_dups_enabled() {
        s = collapse_adjacent(&s);
    }
    s
}

/// The stage. Holds only counters; all configuration is boot-time env, so a
/// decision never changes mid-session and the prefix stays stable.
#[derive(Default)]
pub struct NormalizeTransform {
    blocks: AtomicU64,
    chars_removed: AtomicU64,
}

impl NormalizeTransform {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn blocks(&self) -> u64 {
        self.blocks.load(Ordering::Relaxed)
    }
}

/// Which tool produced each `tool_use_id` in this request, so a `tool_result`
/// can be routed to the Read-specific transform.
fn tool_by_id(body: &serde_json::Value) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    if let Some(msgs) = body.get("messages").and_then(|m| m.as_array()) {
        for m in msgs {
            if let Some(blocks) = m.get("content").and_then(|c| c.as_array()) {
                for b in blocks {
                    if b.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                        if let (Some(id), Some(name)) = (
                            b.get("id").and_then(|v| v.as_str()),
                            b.get("name").and_then(|v| v.as_str()),
                        ) {
                            map.insert(id.to_string(), name.to_string());
                        }
                    }
                }
            }
        }
    }
    map
}

/// Rewrite a `tool_result`'s content in place, string or block form.
/// Returns characters removed.
fn normalise_tool_result(block: &mut serde_json::Value, is_read: bool) -> i64 {
    let mut removed = 0i64;
    match block.get_mut("content") {
        Some(serde_json::Value::String(s)) => {
            if below_floor(s) {
                return 0;
            }
            let new = normalise_block(s, is_read);
            if new.len() < s.len() {
                removed = (s.len() - new.len()) as i64;
                *s = new;
            }
        }
        Some(serde_json::Value::Array(parts)) => {
            for p in parts.iter_mut() {
                if p.get("type").and_then(|t| t.as_str()) != Some("text") {
                    continue;
                }
                if let Some(serde_json::Value::String(s)) = p.get_mut("text") {
                    if below_floor(s) {
                        continue;
                    }
                    let new = normalise_block(s, is_read);
                    if new.len() < s.len() {
                        removed += (s.len() - new.len()) as i64;
                        *s = new;
                    }
                }
            }
        }
        _ => {}
    }
    removed
}

#[async_trait::async_trait]
impl Transform for NormalizeTransform {
    fn name(&self) -> &'static str {
        STAGE
    }

    async fn on_request(&self, req: &mut InterceptedRequest) -> AppResult<()> {
        // Anthropic dialect only: the OpenAI path stores tool output in a
        // different shape.
        if !matches!(req.provider, Provider::Anthropic) {
            return Ok(());
        }
        let tools = tool_by_id(&req.body);
        let mut chars_removed = 0i64;
        let mut blocks = 0u64;
        let mut before = 0usize;
        let mut after = 0usize;
        if let Some(msgs) = req.body.get_mut("messages").and_then(|m| m.as_array_mut()) {
            for m in msgs.iter_mut() {
                let Some(content) = m.get_mut("content").and_then(|c| c.as_array_mut()) else {
                    continue;
                };
                for b in content.iter_mut() {
                    if b.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                        continue;
                    }
                    let is_read = b
                        .get("tool_use_id")
                        .and_then(|v| v.as_str())
                        .and_then(|id| tools.get(id))
                        .map(|t| t == "Read" || t == "NotebookRead")
                        .unwrap_or(false);
                    let pre = block_chars(b);
                    let removed = normalise_tool_result(b, is_read);
                    if removed > 0 {
                        blocks += 1;
                        chars_removed += removed;
                        before += pre;
                        after += block_chars(b);
                    }
                }
            }
        }
        if chars_removed > 0 {
            self.blocks.fetch_add(blocks, Ordering::Relaxed);
            self.chars_removed
                .fetch_add(chars_removed as u64, Ordering::Relaxed);
            // Report the saving in TOKENS, which is what the bill is in.
            // Counted on the concatenated before/after text rather than
            // scaled from characters, because the transforms remove
            // short high-frequency substrings whose token cost is not
            // proportional to their length.
            let saved = (before.saturating_sub(after)) as i64;
            req.record_saving(STAGE, saved);
            tracing::info!(
                blocks,
                chars_removed,
                tokens_saved = saved,
                tokens_before = before,
                "normalize: category-A normalisation applied"
            );
        }
        Ok(())
    }
}

fn block_chars(block: &serde_json::Value) -> usize {
    match block.get("content") {
        Some(serde_json::Value::String(s)) => crate::tokenizer::count(s),
        Some(serde_json::Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .map(crate::tokenizer::count)
            .sum(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_line_endings_keep_their_text() {
        assert_eq!(normalise_whitespace("abc\r\ndef\r\n"), "abc\ndef\n");
        assert_eq!(normalise_whitespace("10%\r55%\r100%\r\ndone"), "100%\ndone");
        assert_eq!(normalise_whitespace("10%\r55%\r100%"), "100%");
    }

    #[test]
    fn read_blocks_come_back_byte_identical() {
        let read = "line with trailing spaces   \nsame line\nsame line\n\x1b[31mliteral esc\x1b[0m\nmd hard break  \n";
        assert_eq!(
            normalise_block(read, true),
            read,
            "a Read result must match disk byte-for-byte or a later Edit misses"
        );
        assert_ne!(
            normalise_block(read, false),
            read,
            "the same content as terminal output is still normalised"
        );
    }

    #[test]
    fn ansi_is_stripped_without_breaking_utf8() {
        let s = "\x1b[31mERROR\x1b[0m: café built\x1b]0;title\x07 done";
        assert_eq!(strip_ansi(s), "ERROR: café built done");
    }

    #[test]
    fn progress_redraws_keep_only_the_final_render() {
        let s = "Progress 10%\rProgress 50%\rProgress 100%\nnext line   ";
        assert_eq!(normalise_whitespace(s), "Progress 100%\nnext line");
    }

    #[test]
    fn adjacent_runs_collapse_but_blank_lines_and_pairs_do_not() {
        let s = "keep this line\nsame long line\nsame long line\nsame long line\nend";
        assert_eq!(
            collapse_adjacent(s),
            "keep this line\nsame long line [x3]\nend"
        );
        // A run of two is left alone, and blank runs are structure.
        let s2 = "aaaaaaaaaa\naaaaaaaaaa\n\n\n\nz";
        assert_eq!(collapse_adjacent(s2), s2);
    }

    #[test]
    fn scattered_duplicates_keep_first_occurrence_with_a_count() {
        let s = "alpha line here\nbeta line here\nalpha line here\ngamma\nalpha line here";
        assert_eq!(
            collapse_duplicate_lines(s),
            "alpha line here [x3]\nbeta line here\ngamma"
        );
    }

    #[test]
    fn read_prefixes_are_stripped_only_from_numbered_blocks() {
        let numbered = "1\tdef f():\n2\t    return 1\n3\t\n4\tprint(f())";
        assert_eq!(
            strip_read_line_prefixes(numbered),
            "def f():\n    return 1\n\nprint(f())"
        );
        let tsv = "name\tvalue\nalpha\t1\nbeta\t2";
        assert_eq!(strip_read_line_prefixes(tsv), tsv);
    }

    #[test]
    fn split_line_prefix_rejects_non_numeric_heads() {
        assert_eq!(split_line_prefix("12\tbody"), Some((12, "body")));
        assert_eq!(split_line_prefix("  7\tbody"), Some((7, "body")));
        assert_eq!(split_line_prefix("12a\tbody"), None);
        assert_eq!(split_line_prefix("no tab here"), None);
    }

    #[test]
    fn hook_filtered_blocks_are_not_cut_again() {
        let tag = crate::pipeline::TRUNCATION_MARKER_TAG;
        let body = format!(
            "same long line here\nsame long line here\nsame long line here\n[… 900 {tag} …]\ntail"
        );
        let out = normalise_block(&body, false);
        assert!(!out.contains("[x3]"), "no collapsing: {out}");
        assert!(out.contains(tag), "the hook's marker survives");
        assert_eq!(out.matches("same long line here").count(), 3);
    }

    #[test]
    fn the_hooks_marker_line_is_never_collapsed() {
        let tag = crate::pipeline::TRUNCATION_MARKER_TAG;
        // Same marker text twice, as it legitimately appears across a
        // session. Even reached directly, the collapsers must not touch it.
        let m = format!("[… 40 {tag} …]");
        let s = format!("{m}\nalpha line here\n{m}\nbeta line here\n{m}");
        assert_eq!(collapse_duplicate_lines(&s).matches(&m).count(), 3);
        let run = format!("{m}\n{m}\n{m}\nz");
        assert_eq!(collapse_adjacent(&run), run);
    }

    #[test]
    fn normalise_block_is_idempotent() {
        // Byte-stability across turns requires f(f(x)) == f(x): the stage
        // runs on every request and history is re-sent each turn.
        let s = "\x1b[31mERROR\x1b[0m   \nsame long line\nsame long line\nsame long line\ntail";
        let once = normalise_block(s, false);
        assert_eq!(normalise_block(&once, false), once);
    }

    #[tokio::test]
    async fn stage_rewrites_tool_results_and_records_a_saving() {
        let big: String = "identical padded log line\n".repeat(60);
        let mut req = crate::model::InterceptedRequest {
            provider: Provider::Anthropic,
            path: "/v1/messages".into(),
            model: "claude-sonnet-5".into(),
            stream: false,
            client: Some("claude-code".into()),
            project: None,
            session: Some("sess1".into()),
            api_key_hash: None,
            tokens_saved: 0,
            saved_by_stage: Vec::new(),
            routed_from: None,
            request_class: None,
            user_id: None,
            ccr: Default::default(),
            ccr_retrieve_calls: 0,
            pipeline_ms: 0,
            body: serde_json::json!({
                "messages": [
                    {"role":"assistant","content":[
                        {"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"make"}}]},
                    {"role":"user","content":[
                        {"type":"tool_result","tool_use_id":"tu1","content": big}]}
                ]
            }),
            budget_reserved: 0.0,
            team_budget_reserved: 0.0,
            team_budget_reserved_at: None,
        };
        NormalizeTransform::new()
            .on_request(&mut req)
            .await
            .expect("stage runs");
        let out = req.body["messages"][1]["content"][0]["content"]
            .as_str()
            .expect("content stays a string");
        assert!(out.contains("[x60]"), "adjacent run collapsed: {out}");
        assert!(req.saved_by(STAGE) > 0, "saving recorded");
    }

    /// A client's own `cache_control` must survive compression untouched.
    ///
    /// This failure is common in request-rewriting gateways, and it fails
    /// silently with a 200: the marker is
    /// dropped, nothing errors, and the whole conversation re-bills as uncached
    /// on every turn while still paying the 1.25x write rate. The transform
    /// edits a tool_result's `content` in place for exactly this reason; this
    /// pins that so a future rewrite cannot quietly undo it.
    #[tokio::test]
    async fn a_clients_cache_control_survives_compression() {
        let big: String = "identical padded log line\n".repeat(60);
        let mut req = crate::model::InterceptedRequest {
            provider: Provider::Anthropic,
            path: "/v1/messages".into(),
            model: "claude-sonnet-5".into(),
            stream: false,
            client: Some("claude-code".into()),
            project: None,
            session: Some("sess1".into()),
            api_key_hash: None,
            tokens_saved: 0,
            saved_by_stage: Vec::new(),
            routed_from: None,
            request_class: None,
            user_id: None,
            ccr: Default::default(),
            ccr_retrieve_calls: 0,
            pipeline_ms: 0,
            body: serde_json::json!({
                "system": [
                    {"type":"text","text":"stable prefix",
                     "cache_control":{"type":"ephemeral"}}
                ],
                "messages": [
                    {"role":"assistant","content":[
                        {"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"make"}}]},
                    {"role":"user","content":[
                        {"type":"tool_result","tool_use_id":"tu1","content": big,
                         "cache_control":{"type":"ephemeral"}}]}
                ]
            }),
            budget_reserved: 0.0,
            team_budget_reserved: 0.0,
            team_budget_reserved_at: None,
        };
        NormalizeTransform::new()
            .on_request(&mut req)
            .await
            .expect("stage runs");

        // The content was compressed...
        let out = req.body["messages"][1]["content"][0]["content"]
            .as_str()
            .expect("tool_result content stays a string");
        assert!(out.len() < big.len(), "the stage should have compressed");

        // ...and every breakpoint the client set is still exactly where it was.
        assert_eq!(
            req.body["messages"][1]["content"][0]["cache_control"],
            serde_json::json!({"type":"ephemeral"}),
            "a tool_result's cache_control was dropped by compression"
        );
        assert_eq!(
            req.body["system"][0]["cache_control"],
            serde_json::json!({"type":"ephemeral"}),
            "the system prefix's cache_control was dropped"
        );
    }
}
