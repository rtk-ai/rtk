//! Transformation pipeline.
//!
//! An ordered chain of [`Transform`] stages every intercepted request passes
//! through before it is forwarded upstream, and again over the outcome once
//! the upstream responds. Transforms are cheap, composable and independently
//! testable.
//!
//! The trait and the chain are the extension point: this crate ships the
//! stages below, and a downstream binary adds its own by implementing
//! [`Transform`] — it never has to fork the chain itself.
//!
//! Stages here:
//!
//! - [`ccr`] — Compress-Cache-Retrieve, the reversible context compressor.
//!   The only stage in the default chain that actually removes tokens.
//! - [`cache_align`] — injects a prompt-cache breakpoint at the end of the
//!   stable prefix when the client set none. Always safe, always on.
//! - [`redact`] — best-effort secret scrub. Provided as a library module,
//!   deliberately **not** wired into the default chain (it rewrites request
//!   content, which a caller must opt into knowingly).
//! - [`ingest_filter`] — trims oversized `tool_result` / `tool_use` blocks.
//!   Lossy (it truncates and leaves a marker), so it is `Off` unless a
//!   `compression` level is set.
//! - [`pixel_compress`] — EXPERIMENTAL: renders an eligible `tool_result`
//!   as a PNG instead of truncating it. Off by default; see below for why
//!   its defaults are Anthropic-only and code-shaped-only.
//! - [`image_opt`] — downscales oversized inbound images. Its
//!   `claude_visual_tokens` helper is what [`pixel_compress`] prices a
//!   render with; the transform itself is library-only, not in the chain.
//! - [`level`] — the shared `CompressionLevel` enum stages are parametrized
//!   with.
//!
//! ## Design constraint: optical/pixel-mode context compression
//!
//! [`pixel_compress`] implements this, and these constraints are why it
//! ships off, Anthropic-only, and code-shaped-only.
//!
//! Rendering context as an image (PNG) instead of text — "pixel mode" — does
//! NOT generalise across providers or content types:
//!   - Claude: prose and code can win; JSON and logs lose.
//!   - GPT: only code can win; dense renders are read unreliably. Cost
//!     models differ too (Claude bills by area; GPT by tiles).
//!   - JSON/logs lose on BOTH providers: a text tokenizer already compresses
//!     repetition/structure better than an image can.
//!
//! An optical-compression stage must NOT apply one rule per content type
//! across providers — render density AND the activation policy must be
//! calibrated per destination provider, with code alone as the safest
//! default if only one blanket rule is possible.

pub mod cache_align;
pub mod ccr;
pub mod engine_methods;
pub mod image_opt;
pub mod ingest_filter;
pub mod ingest_openai;
pub mod level;
pub mod model_route;
pub mod normalize;
pub mod normalize_openai;
pub mod pixel_compress;
pub mod redact;

use crate::error::AppResult;
use crate::model::{InterceptedRequest, UsageOutcome};
use std::sync::Arc;

/// Marker text a **lossy** stage must embed where it dropped content.
///
/// This crate ships no such stage — [`ccr`] is reversible — but a downstream
/// binary's truncating filter must use this tag, because [`ccr`] refuses to
/// cache any block containing it: caching an already-truncated body as "the
/// original" would make `rtk_retrieve` hand the model a body with its middle
/// gone while claiming it is complete.
pub use rtk_engine::marker::TRUNCATION_MARKER_TAG;

/// A single transformation stage.
#[async_trait::async_trait]
pub trait Transform: Send + Sync {
    /// Stable name, used in logs and metrics.
    fn name(&self) -> &'static str;

    /// Mutate the request before it is forwarded upstream.
    async fn on_request(&self, _req: &mut InterceptedRequest) -> AppResult<()> {
        Ok(())
    }

    /// Observe the outcome after the upstream responded (for side effects such
    /// as recording memory). Must not fail the request — errors are logged.
    async fn on_response(
        &self,
        _req: &InterceptedRequest,
        _outcome: &UsageOutcome,
    ) -> AppResult<()> {
        Ok(())
    }
}

/// An ordered chain of transforms.
#[derive(Clone, Default)]
pub struct Pipeline {
    stages: Vec<Arc<dyn Transform>>,
}

impl Pipeline {
    pub fn new() -> Self {
        Self { stages: Vec::new() }
    }

    /// Append a stage; returns `self` for builder-style composition.
    pub fn with(mut self, stage: Arc<dyn Transform>) -> Self {
        self.stages.push(stage);
        self
    }

    /// Add `stage` only when `cond` holds. The closure defers construction so
    /// a disabled stage is never built, and `stage_names()` then reports the
    /// chain that actually runs -- which is what the boot log publishes.
    pub fn with_if(self, cond: bool, stage: impl FnOnce() -> Arc<dyn Transform>) -> Self {
        if cond { self.with(stage()) } else { self }
    }

    pub fn stage_names(&self) -> Vec<&'static str> {
        self.stages.iter().map(|s| s.name()).collect()
    }

    /// Run the request phase across all stages, in order.
    pub async fn run_request(&self, req: &mut InterceptedRequest) -> AppResult<()> {
        for stage in &self.stages {
            stage.on_request(req).await?;
        }
        Ok(())
    }

    /// Run the response phase across all stages. Errors are logged, not fatal.
    pub async fn run_response(&self, req: &InterceptedRequest, outcome: &UsageOutcome) {
        for stage in &self.stages {
            if let Err(e) = stage.on_response(req, outcome).await {
                tracing::warn!(stage = stage.name(), error = %e, "response transform failed");
            }
        }
    }
}
