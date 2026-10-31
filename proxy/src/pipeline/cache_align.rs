//! Cache-alignment transform (lossless).
//!
//! Anthropic prompt caching reads the cached prefix at 0.1x, but only when the
//! request carries `cache_control` breakpoints — many clients don't send them,
//! so a perfectly stable prefix (tools/system) is re-billed in full every turn.
//! This transform marks the end of the stable prefix with an ephemeral
//! `cache_control` breakpoint when the client set none, so the provider caches
//! it. It NEVER overrides a client that already manages caching (e.g. Claude
//! Code), and changes no content — purely a caching hint.

use crate::error::AppResult;
use crate::model::{InterceptedRequest, Provider};
use crate::pipeline::Transform;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

/// Runtime on/off flag, shared with the admin Settings.
pub type SharedFlag = Arc<RwLock<bool>>;

pub struct CacheAlignTransform {
    enabled: SharedFlag,
    aligned: Arc<AtomicU64>,
}

impl CacheAlignTransform {
    pub fn new(enabled: SharedFlag) -> Self {
        Self {
            enabled,
            aligned: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn aligned_count(&self) -> u64 {
        self.aligned.load(Ordering::Relaxed)
    }

    fn is_enabled(&self) -> bool {
        self.enabled.read().map(|e| *e).unwrap_or(false)
    }
}

/// Whether the body already carries any `cache_control` breakpoint.
fn has_cache_control(body: &serde_json::Value) -> bool {
    if let Some(tools) = body.get("tools").and_then(|t| t.as_array()) {
        if tools.iter().any(|t| t.get("cache_control").is_some()) {
            return true;
        }
    }
    if let Some(sys) = body.get("system").and_then(|s| s.as_array()) {
        if sys.iter().any(|b| b.get("cache_control").is_some()) {
            return true;
        }
    }
    if let Some(msgs) = body.get("messages").and_then(|m| m.as_array()) {
        for m in msgs {
            if let Some(blocks) = m.get("content").and_then(|c| c.as_array()) {
                if blocks.iter().any(|b| b.get("cache_control").is_some()) {
                    return true;
                }
            }
        }
    }
    false
}

fn ephemeral() -> serde_json::Value {
    serde_json::json!({ "type": "ephemeral" })
}

/// Mark the end of the stable prefix. Prefers the last tool definition (the
/// dominant, fully-static block); falls back to the last system block.
/// Returns true if a breakpoint was added.
fn mark_prefix(body: &mut serde_json::Value) -> bool {
    let obj = match body.as_object_mut() {
        Some(o) => o,
        None => return false,
    };
    if let Some(tools) = obj.get_mut("tools").and_then(|t| t.as_array_mut()) {
        if let Some(last) = tools.last_mut() {
            if let Some(t) = last.as_object_mut() {
                t.insert("cache_control".to_string(), ephemeral());
                return true;
            }
        }
    }
    // 2) Last system block (only when system is an array of blocks).
    if let Some(sys) = obj.get_mut("system").and_then(|s| s.as_array_mut()) {
        if let Some(last) = sys.last_mut() {
            if let Some(b) = last.as_object_mut() {
                b.insert("cache_control".to_string(), ephemeral());
                return true;
            }
        }
    }
    false
}

#[async_trait::async_trait]
impl Transform for CacheAlignTransform {
    fn name(&self) -> &'static str {
        "cache_align"
    }

    async fn on_request(&self, req: &mut InterceptedRequest) -> AppResult<()> {
        if !self.is_enabled() || !matches!(req.provider, Provider::Anthropic) {
            return Ok(());
        }
        // Respect clients that already manage caching.
        if has_cache_control(&req.body) {
            return Ok(());
        }
        if mark_prefix(&mut req.body) {
            self.aligned.fetch_add(1, Ordering::Relaxed);
            tracing::debug!("cache-aligned: added cache_control on the stable prefix");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag(on: bool) -> SharedFlag {
        Arc::new(RwLock::new(on))
    }

    fn req(body: serde_json::Value) -> InterceptedRequest {
        InterceptedRequest {
            provider: Provider::Anthropic,
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

    #[tokio::test]
    async fn marks_last_tool_when_no_cache_control() {
        let t = CacheAlignTransform::new(flag(true));
        let mut r = req(serde_json::json!({
            "tools": [ { "name": "a" }, { "name": "b" } ],
            "messages": [ { "role": "user", "content": "hi" } ]
        }));
        t.on_request(&mut r).await.unwrap();
        assert!(r.body["tools"][1]["cache_control"].is_object());
        assert!(r.body["tools"][0].get("cache_control").is_none());
        assert_eq!(t.aligned_count(), 1);
    }

    #[tokio::test]
    async fn respects_existing_cache_control() {
        let t = CacheAlignTransform::new(flag(true));
        let mut r = req(serde_json::json!({
            "tools": [ { "name": "a", "cache_control": { "type": "ephemeral" } } ],
            "messages": [ { "role": "user", "content": "hi" } ]
        }));
        t.on_request(&mut r).await.unwrap();
        assert_eq!(t.aligned_count(), 0);
    }

    #[tokio::test]
    async fn disabled_is_noop() {
        let t = CacheAlignTransform::new(flag(false));
        let mut r = req(serde_json::json!({ "tools": [ { "name": "a" } ] }));
        t.on_request(&mut r).await.unwrap();
        assert!(r.body["tools"][0].get("cache_control").is_none());
    }

    #[tokio::test]
    async fn falls_back_to_system_blocks() {
        let t = CacheAlignTransform::new(flag(true));
        let mut r = req(serde_json::json!({
            "system": [ { "type": "text", "text": "be terse" } ],
            "messages": [ { "role": "user", "content": "hi" } ]
        }));
        t.on_request(&mut r).await.unwrap();
        assert!(r.body["system"][0]["cache_control"].is_object());
    }
}
