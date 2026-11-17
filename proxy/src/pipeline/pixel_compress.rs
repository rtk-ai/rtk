//! Pixel-compress: EXPERIMENTAL, off by default. Renders an eligible
//! `tool_result` block as a PNG image instead of ingest_filter's lossy
//! head/tail text truncation.
//!
//! See `pipeline/mod.rs`'s module doc for why this defaults to
//! Anthropic-only, code-shaped content only, skip JSON/logs: those are the
//! only conditions under which rendering-as-image can win, and even then
//! only at a "readable" render density -- denser renderings are misread.
//!
//! Rendering is a native, pure-Rust bitmap rasterizer (`font8x8`'s public-
//! domain 8x8 glyphs, via the already-present `image` crate for PNG
//! encoding) rather than a headless-browser screenshot: spawning a full
//! browser process synchronously per request is not viable on a live proxy's
//! request path. The density (`font_scale`) defaults to the safe end.
//!
//! Unlike `ccr`, this is NOT reversible: once
//! rendered, the original text is gone from the request. It trades one
//! failure mode (ingest_filter's truncation losing the omitted middle) for a
//! different one (a vision encoder misreading the image) -- track whether it
//! actually helps via `rtk gain`'s LLM Proxy panel before trusting it.

use crate::config::PixelCompressConfig;
use crate::error::AppResult;
use crate::model::InterceptedRequest;
use crate::pipeline::Transform;
use crate::pipeline::image_opt::claude_visual_tokens;

use base64::Engine;
use image::{ImageFormat, RgbImage};
use std::sync::{Arc, RwLock};

/// Base glyph size from `font8x8` (8x8 pixels), before `font_scale`.
const GLYPH_PX: u32 = 8;
/// Border around the rendered text block, in pixels (pre-scale).
const PADDING: u32 = 12;
/// Cream background.
const BG: [u8; 3] = [0xf4, 0xf1, 0xea];
/// Dark navy text.
const FG: [u8; 3] = [0x1c, 0x23, 0x31];
/// Anthropic hard-*rejects* (never downscales) an oversized image inside a
/// `tool_result` block — unlike a normal top-level image, which just gets
/// downscaled. 1568px is the long-edge limit every model honors (the
/// "Standard" resolution tier); newer models allow more, but this transform
/// has no way to know the target model, so it stays inside the limit all of
/// them accept. A taller render (a multi-thousand-line file) comes back a
/// 400 "image could not be processed".
const MAX_LONG_EDGE: u32 = 1568;
/// Prepended to the rendered block so the model (and anyone reading the
/// transcript) knows the modality changed — same transparency principle as
/// ingest_filter's truncation marker, and deliberately plain/factual rather
/// than self-referential jargon ("EXPERIMENTAL pixel-compress"): hedging,
/// tool-name-dropping phrasing reads as unusual enough to trip
/// Claude Code's own prompt-injection suspicion on tool results, making it
/// refuse to trust the (correct) content without asking the user to
/// re-verify manually. Note this only softens the wording — swapping an
/// unexpected image in for text a text-only tool (Read) was expected to
/// return is itself the bigger anomaly, and no caption fully avoids that.
const CAPTION: &str = "[rtk-proxy: file content shown below as an image to save context space]";

pub struct PixelCompressTransform {
    enabled: Arc<RwLock<bool>>,
    cfg: Arc<PixelCompressConfig>,
}

impl PixelCompressTransform {
    pub fn new(enabled: Arc<RwLock<bool>>, cfg: Arc<PixelCompressConfig>) -> Self {
        Self { enabled, cfg }
    }

    #[cfg(test)]
    fn fixed(cfg: PixelCompressConfig) -> Self {
        Self {
            enabled: Arc::new(RwLock::new(cfg.enabled)),
            cfg: Arc::new(cfg),
        }
    }

    fn is_on(&self) -> bool {
        self.enabled.read().map(|g| *g).unwrap_or(false)
    }
}

/// Word-wrap `text` to at most `max_chars` characters per rendered line.
/// Character-count wrap (not word-aware) — good enough for code/prose/log
/// lines, and simple enough to keep the renderer a pure function of its
/// input. Pure helper, independently testable.
fn wrap_lines(text: &str, max_chars: usize) -> Vec<String> {
    let max_chars = max_chars.max(1);
    let mut out = Vec::new();
    for raw_line in text.lines() {
        if raw_line.is_empty() {
            out.push(String::new());
            continue;
        }
        let chars: Vec<char> = raw_line.chars().collect();
        for chunk in chars.chunks(max_chars) {
            out.push(chunk.iter().collect());
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Draw one ASCII glyph at `(x0, y0)` (top-left corner, pre-scale grid
/// origin), `scale`x upsampled. Characters outside `font8x8`'s 0..=127 basic
/// range are left blank rather than guessed at.
fn draw_glyph(img: &mut RgbImage, ch: char, x0: u32, y0: u32, scale: u32) {
    let code = ch as u32;
    if code > 127 {
        return;
    }
    let rows = font8x8::legacy::BASIC_LEGACY[code as usize];
    for (ry, row_bits) in rows.iter().enumerate() {
        for rx in 0..GLYPH_PX {
            // bit0 = leftmost column (font8x8's documented convention).
            if (row_bits >> rx) & 1 != 1 {
                continue;
            }
            for sy in 0..scale {
                for sx in 0..scale {
                    let px = x0 + rx * scale + sx;
                    let py = y0 + ry as u32 * scale + sy;
                    if px < img.width() && py < img.height() {
                        img.put_pixel(px, py, image::Rgb(FG));
                    }
                }
            }
        }
    }
}

/// Render `text` as a PNG: cream background, navy bitmap glyphs, wrapped at
/// `cfg.max_line_chars`, cell size `8 * cfg.font_scale` pixels. Pure function
/// of `text` and `cfg` — independently testable without any request
/// plumbing.
pub fn render_to_png(text: &str, cfg: &PixelCompressConfig) -> Vec<u8> {
    let scale = cfg.font_scale.max(1);
    let cell = GLYPH_PX * scale;
    let lines = wrap_lines(text, cfg.max_line_chars);
    let cols = lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(1)
        .max(1) as u32;
    let width = cols * cell + PADDING * 2;
    let height = lines.len() as u32 * cell + PADDING * 2;

    let mut img = RgbImage::from_pixel(width, height, image::Rgb(BG));
    for (row, line) in lines.iter().enumerate() {
        for (col, ch) in line.chars().enumerate() {
            draw_glyph(
                &mut img,
                ch,
                PADDING + col as u32 * cell,
                PADDING + row as u32 * cell,
                scale,
            );
        }
    }

    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Png)
        .expect("encoding a freshly-built RgbImage to PNG cannot fail");
    buf.into_inner()
}

/// Heuristic: does `text` look like JSON? Skipped by default (`skip_json`) —
/// its render costs more than the text on every provider.
fn looks_like_json(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with('{') || t.starts_with('[')
}

/// Heuristic: does `text` look like a timestamped/log-level log stream?
/// Skipped by default (`skip_logs`) — same reason. Crude on purpose (a
/// majority-of-lines vote), since a false negative here just means falling
/// back to the existing truncation path, not a correctness bug.
///
/// A leading digit run immediately followed by a tab or `→` is Claude Code's
/// own Read-tool line-number prefix (`"123\tcontent"`), not a log timestamp
/// — file reads are a very common large `tool_result`, so without this
/// exclusion `skip_logs` (on by default) would silently disable
/// pixel-compress for that entire case.
fn looks_like_log(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().take(20).collect();
    if lines.len() < 3 {
        return false;
    }
    let hits = lines
        .iter()
        .filter(|l| {
            let l = l.trim_start();
            if l.is_empty() {
                return false;
            }
            if l.starts_with("[INFO")
                || l.starts_with("[WARN")
                || l.starts_with("[ERROR")
                || l.starts_with("[DEBUG")
                || l.starts_with("INFO ")
                || l.starts_with("WARN ")
                || l.starts_with("ERROR ")
                || l.starts_with("DEBUG ")
            {
                return true;
            }
            if !l.as_bytes()[0].is_ascii_digit() {
                return false;
            }
            let after_digits = l.trim_start_matches(|c: char| c.is_ascii_digit());
            !(after_digits.starts_with('\t') || after_digits.starts_with('\u{2192}'))
        })
        .count();
    hits * 2 >= lines.len()
}

/// Whether `text` is eligible for pixel-compress under `cfg`: big enough,
/// and not a content shape known to lose (JSON/logs).
fn eligible(text: &str, cfg: &PixelCompressConfig) -> bool {
    if crate::tokenizer::count(text) < cfg.min_tokens {
        return false;
    }
    if cfg.skip_json && looks_like_json(text) {
        return false;
    }
    if cfg.skip_logs && looks_like_log(text) {
        return false;
    }
    true
}

/// Build the replacement `content` array for a compressed `tool_result`: a
/// short caption (audit trail — same transparency principle as
/// ingest_filter's truncation marker) followed by the rendered image block.
///
/// `None` when rendering would NOT actually save tokens (e.g. many short,
/// highly-repetitive lines produce a tall image whose visual-token cost
/// exceeds the original text) — same "never emit a replacement bigger than
/// the original" guarantee `ingest_filter`/CCR both hold, just enforced here
/// against the real encoded PNG rather than an estimate.
fn build_replacement(text: &str, cfg: &PixelCompressConfig) -> Option<(serde_json::Value, i64)> {
    let png = render_to_png(text, cfg);
    let (w, h) = png_dimensions(&png);
    if w > MAX_LONG_EDGE || h > MAX_LONG_EDGE {
        return None;
    }
    let before = crate::tokenizer::count(text) as i64;
    let after = claude_visual_tokens(w, h);
    if after >= before {
        return None;
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
    let replacement = serde_json::json!([
        { "type": "text", "text": CAPTION },
        { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": b64 } }
    ]);
    Some((replacement, before - after))
}

/// Read a PNG's `(width, height)` back out of its own bytes we just encoded
/// — avoids threading the pre-encode dimensions through a second return path.
fn png_dimensions(png: &[u8]) -> (u32, u32) {
    image::load_from_memory(png)
        .map(|i| (i.width(), i.height()))
        .unwrap_or((0, 0))
}

/// Compress a single `tool_result` block's `content` in place if eligible.
/// Returns tokens saved (0 if skipped/ineligible). Only a bare string
/// content, or a single-text-block array, is considered — a `tool_result`
/// whose content already mixes multiple blocks (e.g. an existing image) is
/// left untouched rather than risk mangling it.
fn compress_tool_result_block(block: &mut serde_json::Value, cfg: &PixelCompressConfig) -> i64 {
    if block.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
        return 0;
    }
    let text: Option<String> = match block.get("content") {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Array(inner)) if inner.len() == 1 => inner[0]
            .get("text")
            .and_then(|t| t.as_str())
            .map(|s| s.to_string()),
        _ => None,
    };
    let Some(text) = text else { return 0 };
    if !eligible(&text, cfg) {
        return 0;
    }
    let Some((replacement, saved)) = build_replacement(&text, cfg) else {
        return 0;
    };
    if let Some(obj) = block.as_object_mut() {
        obj.insert("content".to_string(), replacement);
    }
    saved
}

fn compress_content_array(blocks: &mut [serde_json::Value], cfg: &PixelCompressConfig) -> i64 {
    blocks
        .iter_mut()
        .map(|b| compress_tool_result_block(b, cfg))
        .sum()
}

#[async_trait::async_trait]
impl Transform for PixelCompressTransform {
    fn name(&self) -> &'static str {
        "pixel_compress"
    }

    async fn on_request(&self, req: &mut InterceptedRequest) -> AppResult<()> {
        if !self.is_on() {
            return Ok(());
        }
        if !self
            .cfg
            .providers
            .iter()
            .any(|p| p.eq_ignore_ascii_case(req.provider.as_str()))
        {
            return Ok(());
        }
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
            if let Some(serde_json::Value::Array(blocks)) =
                msg.as_object_mut().and_then(|o| o.get_mut("content"))
            {
                saved += compress_content_array(blocks, &self.cfg);
            }
        }
        if saved > 0 {
            req.record_saving(self.name(), saved);
            tracing::info!(tokens_saved = saved, "pixel-compressed a tool_result");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Provider;

    fn cfg() -> PixelCompressConfig {
        PixelCompressConfig {
            enabled: true,
            min_tokens: 5,
            font_scale: 1,
            max_line_chars: 40,
            providers: vec!["anthropic".to_string()],
            skip_json: true,
            skip_logs: true,
        }
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

    #[test]
    fn renders_a_valid_decodable_png() {
        let png = render_to_png("fn main() {\n    println!(\"hi\");\n}", &cfg());
        let img = image::load_from_memory(&png).expect("valid PNG");
        assert!(img.width() > 0 && img.height() > 0);
    }

    #[test]
    fn wraps_long_lines_at_max_chars() {
        let lines = wrap_lines("abcdefghij", 4);
        assert_eq!(lines, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn json_like_content_is_skipped() {
        assert!(looks_like_json(r#"{"a": 1, "b": [1,2,3]}"#));
        assert!(looks_like_json("  [1, 2, 3]"));
        assert!(!looks_like_json("fn main() {}"));
    }

    #[test]
    fn log_like_content_is_skipped() {
        let log = "2026-01-01T00:00:00Z INFO starting\n2026-01-01T00:00:01Z WARN slow\n2026-01-01T00:00:02Z INFO ready\n";
        assert!(looks_like_log(log));
        assert!(!looks_like_log("fn main() {\n    println!(\"hi\");\n}\n"));
    }

    #[test]
    fn claude_code_line_numbered_read_output_is_not_mistaken_for_a_log() {
        // Claude Code's Read tool prefixes every
        // line with its line number + a tab before the actual content.
        let read_output = "1\tdef process_item_0(value, options=None):\n\
             2\t    return value\n\
             3\t\n\
             4\tdef process_item_1(value, options=None):\n\
             5\t    return value\n";
        assert!(!looks_like_log(read_output));
    }

    #[tokio::test]
    async fn compresses_an_eligible_tool_result_and_records_savings() {
        // Kept under MAX_LONG_EDGE (200 lines would render ~1624px tall,
        // over the 1568px cap tool_result images get hard-rejected past).
        let big_code = "fn compress() {\n".repeat(100);
        let mut req = make_req(serde_json::json!({
            "messages": [
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "a", "content": big_code }
                ] }
            ]
        }));
        PixelCompressTransform::fixed(cfg())
            .on_request(&mut req)
            .await
            .unwrap();

        let content = &req.body["messages"][0]["content"][0]["content"];
        assert!(content.is_array());
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image");
        assert!(req.tokens_saved > 0);
    }

    #[tokio::test]
    async fn content_too_tall_for_the_long_edge_cap_is_left_untouched() {
        // A multi-thousand-line file renders taller than MAX_LONG_EDGE and
        // the Anthropic API hard-rejects the tool_result image with a 400
        // instead of accepting it -- this must
        // stay uncompressed (and thus readable as plain text) rather than
        // ship an image the API will refuse.
        let huge_code = "fn compress() {\n".repeat(400);
        let mut req = make_req(serde_json::json!({
            "messages": [
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "a", "content": huge_code.clone() }
                ] }
            ]
        }));
        PixelCompressTransform::fixed(cfg())
            .on_request(&mut req)
            .await
            .unwrap();

        let content = &req.body["messages"][0]["content"][0]["content"];
        assert_eq!(content.as_str(), Some(huge_code.as_str()));
        assert_eq!(req.tokens_saved, 0);
    }

    #[tokio::test]
    async fn json_tool_result_is_left_untouched() {
        let original = r#"{"key": "value", "nested": {"a": 1}}"#.repeat(50);
        let mut req = make_req(serde_json::json!({
            "messages": [
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "a", "content": original.clone() }
                ] }
            ]
        }));
        PixelCompressTransform::fixed(cfg())
            .on_request(&mut req)
            .await
            .unwrap();

        assert_eq!(
            req.body["messages"][0]["content"][0]["content"]
                .as_str()
                .unwrap(),
            original
        );
        assert_eq!(req.tokens_saved, 0);
    }

    #[tokio::test]
    async fn disabled_is_a_noop() {
        let big_code = "fn compress() {\n".repeat(200);
        let original = big_code.clone();
        let mut req = make_req(serde_json::json!({
            "messages": [
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "a", "content": big_code }
                ] }
            ]
        }));
        let mut off = cfg();
        off.enabled = false;
        PixelCompressTransform::fixed(off)
            .on_request(&mut req)
            .await
            .unwrap();

        assert_eq!(
            req.body["messages"][0]["content"][0]["content"]
                .as_str()
                .unwrap(),
            original
        );
        assert_eq!(req.tokens_saved, 0);
    }

    #[tokio::test]
    async fn wrong_provider_is_a_noop() {
        let big_code = "fn compress() {\n".repeat(200);
        let mut req = make_req(serde_json::json!({
            "messages": [
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "a", "content": big_code.clone() }
                ] }
            ]
        }));
        req.provider = Provider::Openai;
        PixelCompressTransform::fixed(cfg())
            .on_request(&mut req)
            .await
            .unwrap();

        assert_eq!(
            req.body["messages"][0]["content"][0]["content"]
                .as_str()
                .unwrap(),
            big_code
        );
        assert_eq!(req.tokens_saved, 0);
    }

    #[tokio::test]
    async fn below_min_tokens_is_a_noop() {
        let mut req = make_req(serde_json::json!({
            "messages": [
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "a", "content": "tiny" }
                ] }
            ]
        }));
        let mut strict = cfg();
        strict.min_tokens = 10_000;
        PixelCompressTransform::fixed(strict)
            .on_request(&mut req)
            .await
            .unwrap();

        assert_eq!(
            req.body["messages"][0]["content"][0]["content"]
                .as_str()
                .unwrap(),
            "tiny"
        );
    }
}
