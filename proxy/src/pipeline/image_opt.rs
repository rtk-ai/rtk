//! Image optimization transform.
//!
//! Resizes oversized images carried in a request so they cost fewer provider
//! vision tokens. Anthropic image blocks look like:
//!
//! ```json
//! { "type": "image",
//!   "source": { "type": "base64", "media_type": "image/png", "data": "<base64>" } }
//! ```
//!
//! These appear inside a message's `content` array, and also nested inside a
//! `tool_result` block's own `content` array. We only touch `source.type ==
//! "base64"` blocks whose `media_type` is `image/png` or `image/jpeg`, and we
//! re-encode to the SAME format. The aggressiveness is a runtime
//! [`CompressionLevel`] set from the admin Settings, **Off** by default (no-op).
//!
//! Token savings are estimated with Claude's visual-token model — an image of
//! `w * h` pixels costs roughly `ceil(w/28) * ceil(h/28)` tokens — and the
//! before/after difference is accumulated into [`InterceptedRequest::tokens_saved`].

use crate::error::AppResult;
use crate::model::InterceptedRequest;
use crate::pipeline::Transform;
use crate::pipeline::level::{CompressionLevel, SharedCompression};

use base64::Engine;
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat};

/// Decode client-supplied image bytes with hard resource limits, so a small
/// base64 blob declaring huge dimensions can't trigger a multi-GB allocation
/// (decompression-bomb DoS). Returns None on any decode error or limit breach.
pub fn decode_image_limited(raw: &[u8]) -> Option<image::DynamicImage> {
    use std::io::Cursor;
    let mut reader = image::ImageReader::new(Cursor::new(raw))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(12_000);
    limits.max_image_height = Some(12_000);
    limits.max_alloc = Some(256 * 1024 * 1024); // 256 MB decode ceiling
    reader.limits(limits);
    reader.decode().ok()
}

/// JPEG re-encode quality (0..=100). 82 is visually near-lossless for screenshots
/// while keeping the payload small.
const JPEG_QUALITY: u8 = 82;

/// Anthropic visual-token tile size, in pixels. Claude prices an image at roughly
/// `ceil(w / 28) * ceil(h / 28)` tokens.
const TILE_PX: u32 = 28;

pub struct ImageOptimizeTransform {
    level: SharedCompression,
}

impl ImageOptimizeTransform {
    pub fn new(level: SharedCompression) -> Self {
        Self { level }
    }

    /// Build with a fixed level (used by tests).
    #[cfg(test)]
    pub fn fixed(level: CompressionLevel) -> Self {
        use std::sync::{Arc, RwLock};
        Self {
            level: Arc::new(RwLock::new(level)),
        }
    }

    fn level(&self) -> CompressionLevel {
        self.level
            .read()
            .map(|l| *l)
            .unwrap_or(CompressionLevel::Off)
    }
}

/// Longest-edge cap, in pixels, for a given level. `None` means disabled.
pub fn cap_for(level: CompressionLevel) -> Option<u32> {
    match level {
        CompressionLevel::Off => None,
        CompressionLevel::Light => Some(2000),
        CompressionLevel::Balanced => Some(1568),
        CompressionLevel::Aggressive => Some(1024),
    }
}

/// Estimate the number of Claude visual tokens an image of `w x h` pixels costs:
/// `ceil(w / 28) * ceil(h / 28)`.
pub fn claude_visual_tokens(w: u32, h: u32) -> i64 {
    let tiles_w = w.div_ceil(TILE_PX) as i64;
    let tiles_h = h.div_ceil(TILE_PX) as i64;
    tiles_w * tiles_h
}

/// Supported re-encode formats, derived from the block's `media_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImgFormat {
    Png,
    Jpeg,
}

impl ImgFormat {
    fn from_media_type(media_type: &str) -> Option<Self> {
        match media_type {
            "image/png" => Some(ImgFormat::Png),
            "image/jpeg" | "image/jpg" => Some(ImgFormat::Jpeg),
            _ => None,
        }
    }
}

/// Resize `img` down so its longest edge equals `cap`, preserving aspect ratio.
/// Returns `None` if the image already fits within `cap` (no resize needed).
///
/// Pure helper, independent of any request plumbing, so it can be unit-tested.
pub fn resize_to_cap(img: &DynamicImage, cap: u32) -> Option<DynamicImage> {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 || cap == 0 {
        return None;
    }
    if w.max(h) <= cap {
        return None;
    }
    // `resize` fits the image within the cap x cap box while preserving the
    // aspect ratio, so the longest edge becomes exactly `cap`.
    Some(img.resize(cap, cap, FilterType::Lanczos3))
}

/// Re-encode `img` to `format` and return the raw bytes. JPEG flattens any alpha
/// (no alpha channel in the format) by encoding the RGB8 view.
fn encode(img: &DynamicImage, format: ImgFormat) -> Result<Vec<u8>, image::ImageError> {
    let mut buf = std::io::Cursor::new(Vec::new());
    match format {
        ImgFormat::Png => {
            img.write_to(&mut buf, ImageFormat::Png)?;
        }
        ImgFormat::Jpeg => {
            let rgb = img.to_rgb8();
            let mut enc =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY);
            enc.encode_image(&rgb)?;
        }
    }
    Ok(buf.into_inner())
}

/// Optimize a single Anthropic-style image block in place. Returns the number of
/// estimated visual tokens saved (0 if the block was skipped or already small).
/// Never errors: any decode/encode failure leaves the block untouched.
fn optimize_image_block(block: &mut serde_json::Value, cap: u32) -> i64 {
    let Some(obj) = block.as_object_mut() else {
        return 0;
    };
    if obj.get("type").and_then(|t| t.as_str()) != Some("image") {
        return 0;
    }
    let Some(source) = obj.get_mut("source").and_then(|s| s.as_object_mut()) else {
        return 0;
    };
    if source.get("type").and_then(|t| t.as_str()) != Some("base64") {
        return 0;
    }
    let Some(format) = source
        .get("media_type")
        .and_then(|m| m.as_str())
        .and_then(ImgFormat::from_media_type)
    else {
        return 0;
    };
    let Some(data_b64) = source.get("data").and_then(|d| d.as_str()) else {
        return 0;
    };

    let engine = base64::engine::general_purpose::STANDARD;
    let Ok(raw) = engine.decode(data_b64) else {
        return 0;
    };
    let Some(img) = decode_image_limited(&raw) else {
        return 0;
    };

    let before = claude_visual_tokens(img.width(), img.height());
    let Some(resized) = resize_to_cap(&img, cap) else {
        return 0;
    };
    let after = claude_visual_tokens(resized.width(), resized.height());

    let Ok(out_bytes) = encode(&resized, format) else {
        return 0;
    };
    let out_b64 = engine.encode(&out_bytes);

    // Replace the data in place; media_type stays as-is (same format).
    source.insert("data".to_string(), serde_json::Value::String(out_b64));

    (before - after).max(0)
}

/// Walk a `content` array and optimize every image block it holds, recursing one
/// level into `tool_result` blocks' own `content` arrays. Returns tokens saved.
fn optimize_content_array(blocks: &mut [serde_json::Value], cap: u32) -> i64 {
    let mut saved = 0;
    for block in blocks.iter_mut() {
        // Direct image block.
        saved += optimize_image_block(block, cap);

        // Nested: a tool_result block whose own `content` is an array of blocks.
        if block.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
            if let Some(serde_json::Value::Array(inner)) = block.get_mut("content") {
                saved += optimize_content_array(inner, cap);
            }
        }
    }
    saved
}

/// Optimize every image inside a single message's `content` (array form only;
/// string content cannot carry an image block). Returns tokens saved.
fn optimize_message(message: &mut serde_json::Value, cap: u32) -> i64 {
    let Some(obj) = message.as_object_mut() else {
        return 0;
    };
    match obj.get_mut("content") {
        Some(serde_json::Value::Array(blocks)) => optimize_content_array(blocks, cap),
        _ => 0,
    }
}

#[async_trait::async_trait]
impl Transform for ImageOptimizeTransform {
    fn name(&self) -> &'static str {
        "image_optimize"
    }

    async fn on_request(&self, req: &mut InterceptedRequest) -> AppResult<()> {
        // No-op unless a cap is currently selected.
        let cap = match cap_for(self.level()) {
            Some(c) => c,
            None => return Ok(()),
        };
        let Some(messages) = req
            .body
            .as_object_mut()
            .and_then(|o| o.get_mut("messages"))
            .and_then(|m| m.as_array_mut())
        else {
            return Ok(());
        };

        let mut saved = 0;
        for msg in messages.iter_mut() {
            saved += optimize_message(msg, cap);
        }
        if saved > 0 {
            req.tokens_saved += saved;
            tracing::info!(tokens_saved = saved, "optimized images in request");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Provider;

    /// Build a square PNG image of `side` px, base64-encoded, as the Anthropic
    /// image block source data.
    fn png_b64(side: u32) -> String {
        let img = DynamicImage::ImageRgba8(image::RgbaImage::new(side, side));
        let bytes = encode(&img, ImgFormat::Png).unwrap();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn image_block(media_type: &str, data: String) -> serde_json::Value {
        serde_json::json!({
            "type": "image",
            "source": { "type": "base64", "media_type": media_type, "data": data }
        })
    }

    fn make_req(body: serde_json::Value) -> InterceptedRequest {
        InterceptedRequest {
            provider: Provider::Anthropic,
            path: "/v1/messages".into(),
            model: "m".into(),
            stream: false,
            client: None,
            project: None,
            session: None,
            api_key_hash: None,
            user_id: None,
            ccr: Default::default(),
            ccr_retrieve_calls: 0,
            pipeline_ms: 0,
            budget_reserved: 0.0,
            team_budget_reserved: 0.0,
            team_budget_reserved_at: None,
            body,
            tokens_saved: 0,
            saved_by_stage: Vec::new(),
            routed_from: None,
            request_class: None,
        }
    }

    /// Decode the base64 image data at a JSON pointer and return its dimensions.
    fn dims_at(req: &InterceptedRequest, pointer: &str) -> (u32, u32) {
        let data = req.body.pointer(pointer).and_then(|v| v.as_str()).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap();
        let img = image::load_from_memory(&bytes).unwrap();
        (img.width(), img.height())
    }

    #[test]
    fn cap_mapping() {
        assert_eq!(cap_for(CompressionLevel::Off), None);
        assert_eq!(cap_for(CompressionLevel::Light), Some(2000));
        assert_eq!(cap_for(CompressionLevel::Balanced), Some(1568));
        assert_eq!(cap_for(CompressionLevel::Aggressive), Some(1024));
    }

    #[test]
    fn visual_tokens_values() {
        // 2800 / 28 = 100 exactly -> 100 x 100 = 10000.
        assert_eq!(claude_visual_tokens(2800, 2800), 10_000);
        // 1568 / 28 = 56 exactly -> 56 x 56 = 3136.
        assert_eq!(claude_visual_tokens(1568, 1568), 3_136);
        // Non-multiples round up: ceil(29/28)=2, ceil(1/28)=1 -> 2.
        assert_eq!(claude_visual_tokens(29, 1), 2);
    }

    #[test]
    fn resize_below_cap_is_noop() {
        let img = DynamicImage::ImageRgba8(image::RgbaImage::new(800, 600));
        assert!(resize_to_cap(&img, 1568).is_none());
    }

    #[test]
    fn resize_preserves_aspect_and_caps_long_edge() {
        let img = DynamicImage::ImageRgba8(image::RgbaImage::new(2800, 1400));
        let out = resize_to_cap(&img, 1568).unwrap();
        // Longest edge becomes the cap; aspect ratio (2:1) preserved.
        assert_eq!(out.width(), 1568);
        assert_eq!(out.height(), 784);
    }

    #[tokio::test]
    async fn balanced_resizes_png_and_records_savings() {
        let mut req = make_req(serde_json::json!({
            "messages": [
                { "role": "user", "content": [ image_block("image/png", png_b64(2800)) ] }
            ]
        }));
        ImageOptimizeTransform::fixed(CompressionLevel::Balanced)
            .on_request(&mut req)
            .await
            .unwrap();

        let (w, h) = dims_at(&req, "/messages/0/content/0/source/data");
        assert_eq!(w.max(h), 1568);
        // 10000 (before) - 3136 (after) = 6864 tokens saved.
        assert_eq!(req.tokens_saved, 6_864);
        // media_type preserved.
        assert_eq!(
            req.body["messages"][0]["content"][0]["source"]["media_type"],
            "image/png"
        );
    }

    #[tokio::test]
    async fn optimizes_image_nested_in_tool_result() {
        let mut req = make_req(serde_json::json!({
            "messages": [
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "a", "content": [
                        image_block("image/png", png_b64(2800))
                    ] }
                ] }
            ]
        }));
        ImageOptimizeTransform::fixed(CompressionLevel::Aggressive)
            .on_request(&mut req)
            .await
            .unwrap();

        let (w, h) = dims_at(&req, "/messages/0/content/0/content/0/source/data");
        assert_eq!(w.max(h), 1024);
        assert!(req.tokens_saved > 0);
    }

    #[tokio::test]
    async fn off_level_is_a_noop() {
        let original = png_b64(2800);
        let mut req = make_req(serde_json::json!({
            "messages": [
                { "role": "user", "content": [ image_block("image/png", original.clone()) ] }
            ]
        }));
        ImageOptimizeTransform::fixed(CompressionLevel::Off)
            .on_request(&mut req)
            .await
            .unwrap();

        // Data untouched and no savings recorded.
        assert_eq!(
            req.body["messages"][0]["content"][0]["source"]["data"]
                .as_str()
                .unwrap(),
            original
        );
        assert_eq!(req.tokens_saved, 0);
    }
}
