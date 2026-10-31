//! `proxy.request.model` — send some requests to a different upstream.
//!
//! An upstream names a provider, a model, and optionally where to reach it, so
//! one rule covers both "same provider, cheaper model" and "a different
//! Anthropic-shaped endpoint". Translation between provider dialects is out of
//! scope and always will be: the request shapes, tool-call encodings, streaming
//! event sequences and thinking signatures do not round-trip.
//!
//! # Why this refuses to route a request that carries a warm cache
//!
//! Prompt caches are model-scoped and a model switch invalidates all three
//! tiers — tools, system and messages — with no escape hatch. An agent resends
//! the whole conversation each turn, so a cache read costs 0.1x input against
//! 1.0x cold: routing one turn of a warm conversation only breaks even if the
//! other model is ten times cheaper. No Anthropic pair is that far apart
//! (Haiku against Opus is 0.2x, so the routed turn costs about double).
//!
//! The guard therefore asks one question, and it is the same question on every
//! agent and in either dialect: **is there a cache here to lose?** Below the
//! provider's minimum cacheable prefix nothing is cached at all, so routing is
//! free. Above it, a route is refused.
//!
//! `x-claude-code-request-class` refines that where it exists — Claude Code
//! labels session titles, classifiers and summaries `auxiliary` and subagent
//! turns `subagent`, and labels the cached conversation `main`. A `main` label
//! is refused whatever the size says. But the header is Claude Code's alone, so
//! it is a refinement and never the gate; an agent that sends nothing still
//! gets the size rule.

use crate::error::AppResult;
use crate::model::InterceptedRequest;
use crate::pipeline::Transform;
use std::sync::atomic::{AtomicU64, Ordering};

/// Request classes whose context is the cached conversation, for the one agent
/// that labels them. Routing one rebuilds the cache.
const CARRIES_THE_CACHED_CONVERSATION: &[&str] = &["main", "workflow"];

/// Below this, a request is too small to have established a cache, so routing
/// it forfeits nothing.
///
/// Anthropic's minimum cacheable prefix is 1024 tokens on most models and 512
/// on the newest; OpenAI's automatic caching starts at 1024. Taking the
/// smallest means the guard never assumes a cache the provider would not have
/// created — it is the conservative end of a documented range, not a guess.
const MIN_CACHEABLE_PREFIX_TOKENS: usize = 512;

/// A resolved destination: what to send, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub model: String,
    pub base_url: Option<String>,
    pub rule: String,
}

/// How much cacheable prefix this request carries.
///
/// Measured off the serialized body rather than a dialect's message shape, so
/// it reads the same for Anthropic `messages` + `system` + `tools` and for
/// OpenAI `messages`. An over-estimate is the safe direction: it refuses a
/// route rather than forfeiting a cache.
fn prefix_size(body: &serde_json::Value) -> usize {
    crate::tokenizer::count(&body.to_string())
}

pub struct ModelRouteTransform {
    routes: Vec<(String, Destination)>,
    routed: AtomicU64,
    declined: AtomicU64,
}

impl ModelRouteTransform {
    /// `routes` maps a request class to its destination.
    pub fn new(routes: Vec<(String, Destination)>) -> Self {
        Self {
            routes,
            routed: AtomicU64::new(0),
            declined: AtomicU64::new(0),
        }
    }

    pub fn routed_count(&self) -> u64 {
        self.routed.load(Ordering::Relaxed)
    }

    /// Requests a rule matched but the cache guard refused.
    pub fn declined_count(&self) -> u64 {
        self.declined.load(Ordering::Relaxed)
    }

    /// The destination for a request of `class` carrying `prefix_tokens`, or
    /// `None` to leave it alone.
    ///
    /// Both gates are refusals, and either one is enough: a labelled `main`
    /// request is never routed however small, and an unlabelled request big
    /// enough to have a cache is never routed either.
    pub fn destination_for(
        &self,
        class: Option<&str>,
        prefix_tokens: usize,
    ) -> Option<&Destination> {
        let class = class?;
        let dest = self
            .routes
            .iter()
            .find(|(c, _)| c == class)
            .map(|(_, d)| d)?;
        if CARRIES_THE_CACHED_CONVERSATION.contains(&class) {
            return None;
        }
        if prefix_tokens >= MIN_CACHEABLE_PREFIX_TOKENS {
            return None;
        }
        Some(dest)
    }
}

#[async_trait::async_trait]
impl Transform for ModelRouteTransform {
    fn name(&self) -> &'static str {
        "model_route"
    }

    async fn on_request(&self, req: &mut InterceptedRequest) -> AppResult<()> {
        let class = req.request_class.clone();
        let Some(class) = class else { return Ok(()) };
        if self.routes.iter().all(|(c, _)| *c != class) {
            return Ok(());
        }
        let prefix_tokens = prefix_size(&req.body);
        let Some(dest) = self.destination_for(Some(&class), prefix_tokens) else {
            // A rule matched, and the guard refused it. Worth counting: a user
            // who routes `main` should be able to see that nothing happened.
            self.declined.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(
                class = %class,
                prefix_tokens,
                "model route declined: there is a cache here to lose"
            );
            return Ok(());
        };

        let from = std::mem::replace(&mut req.model, dest.model.clone());
        if let Some(obj) = req.body.as_object_mut() {
            obj.insert(
                "model".to_string(),
                serde_json::Value::String(dest.model.clone()),
            );
        }
        req.routed_from = Some(from);
        self.routed.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(class = %class, to = %dest.model, rule = %dest.rule, "model routed");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Provider;

    fn dest(model: &str) -> Destination {
        Destination {
            model: model.to_string(),
            base_url: None,
            rule: "cheap-aux".to_string(),
        }
    }

    fn stage() -> ModelRouteTransform {
        ModelRouteTransform::new(vec![
            ("auxiliary".to_string(), dest("claude-haiku-4-5")),
            ("subagent".to_string(), dest("claude-haiku-4-5")),
            // A user may well write this. The guard, not the config, refuses it.
            ("main".to_string(), dest("claude-haiku-4-5")),
        ])
    }

    fn req(class: Option<&str>) -> InterceptedRequest {
        let mut r = InterceptedRequest::for_pipeline(
            Provider::Anthropic,
            "/v1/messages".into(),
            serde_json::json!({"model": "claude-opus-5", "messages": []}),
        );
        r.model = "claude-opus-5".into();
        r.request_class = class.map(str::to_string);
        r
    }

    #[tokio::test]
    async fn a_side_request_is_routed_and_records_where_it_came_from() {
        let s = stage();
        let mut r = req(Some("auxiliary"));
        s.on_request(&mut r).await.expect("stage runs");
        assert_eq!(r.model, "claude-haiku-4-5");
        assert_eq!(r.body["model"], "claude-haiku-4-5");
        assert_eq!(r.routed_from.as_deref(), Some("claude-opus-5"));
        assert_eq!(s.routed_count(), 1);
    }

    #[tokio::test]
    async fn a_main_request_is_never_routed_however_the_rule_is_written() {
        let s = stage();
        let mut r = req(Some("main"));
        s.on_request(&mut r).await.expect("stage runs");
        assert_eq!(r.model, "claude-opus-5", "main must keep its model");
        assert_eq!(r.body["model"], "claude-opus-5");
        assert!(r.routed_from.is_none());
        assert_eq!(s.routed_count(), 0);
        assert_eq!(s.declined_count(), 1, "a refusal should be visible");
    }

    #[tokio::test]
    async fn a_class_with_no_rule_is_left_alone() {
        let s = stage();
        let mut r = req(Some("compaction"));
        s.on_request(&mut r).await.expect("stage runs");
        assert_eq!(r.model, "claude-opus-5");
        assert_eq!(s.routed_count(), 0);
        assert_eq!(
            s.declined_count(),
            0,
            "no rule matched, so nothing declined"
        );
    }

    #[tokio::test]
    async fn without_the_hint_header_nothing_is_routed() {
        // The header is opt-in (CLAUDE_CODE_GATEWAY_HINT_HEADERS=1) and absent
        // on every other agent, so an unlabelled request must pass through.
        let s = stage();
        let mut r = req(None);
        s.on_request(&mut r).await.expect("stage runs");
        assert_eq!(r.model, "claude-opus-5");
        assert_eq!(s.routed_count(), 0);
    }

    #[test]
    fn the_guard_covers_every_class_that_carries_the_conversation() {
        let s = stage();
        for class in CARRIES_THE_CACHED_CONVERSATION {
            assert!(
                s.destination_for(Some(class), 0).is_none(),
                "{class} must never be routed"
            );
        }
    }

    /// The OpenAI dialect puts `model` in the same place, so a same-dialect
    /// route is the same rewrite. Cross-dialect is deliberately not attempted.
    #[tokio::test]
    async fn an_openai_dialect_request_routes_the_same_way() {
        let s = ModelRouteTransform::new(vec![(
            "auxiliary".to_string(),
            Destination {
                model: "gpt-5.6-luna".to_string(),
                base_url: None,
                rule: "cheap-aux".to_string(),
            },
        )]);
        let mut r = InterceptedRequest::for_pipeline(
            Provider::Openai,
            "/v1/chat/completions".into(),
            serde_json::json!({"model": "gpt-5.6-terra", "messages": []}),
        );
        r.model = "gpt-5.6-terra".into();
        r.request_class = Some("auxiliary".into());
        s.on_request(&mut r).await.expect("stage runs");
        assert_eq!(r.body["model"], "gpt-5.6-luna");
        assert_eq!(r.routed_from.as_deref(), Some("gpt-5.6-terra"));
    }

    /// The size guard is the one that works without a vendor header, so it has
    /// to hold on its own.
    #[tokio::test]
    async fn a_request_big_enough_to_be_cached_is_never_routed() {
        let s = stage();
        let mut r = req(Some("auxiliary"));
        r.body = serde_json::json!({
            "model": "claude-opus-5",
            "messages": [{"role": "user", "content": "x".repeat(40_000)}]
        });
        assert!(
            prefix_size(&r.body) >= MIN_CACHEABLE_PREFIX_TOKENS,
            "fixture must exceed the threshold to test the guard"
        );
        s.on_request(&mut r).await.expect("stage runs");
        assert_eq!(
            r.model, "claude-opus-5",
            "a cacheable prefix must not be routed"
        );
        assert_eq!(s.declined_count(), 1);
    }

    #[test]
    fn the_size_guard_holds_with_no_class_label_at_all() {
        // An agent that sends no hint header still gets the size rule, because
        // the gate is "is there a cache to lose", not "did Claude Code say so".
        let s = stage();
        assert!(s.destination_for(Some("auxiliary"), 0).is_some());
        assert!(
            s.destination_for(Some("auxiliary"), MIN_CACHEABLE_PREFIX_TOKENS)
                .is_none()
        );
    }
}
