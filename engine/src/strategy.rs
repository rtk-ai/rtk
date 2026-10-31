//! Layer 3 of shape-aware compression: a lossless RLE pre-pass, then a
//! per-[`Shape`] strategy chosen from a registry. Selection plus
//! deterministic generation (run counts, elided ranges, JSON skeletons) —
//! never a model, so every output is a pure function of the input bytes.

use crate::shape::{self as content, Features, Shape};

pub const PREVIEW_HEAD_LINES: usize = 20;
pub const PREVIEW_TAIL_LINES: usize = 8;
pub const PREVIEW_HEAD_CHARS: usize = 400;
pub const PREVIEW_TAIL_CHARS: usize = 200;
use std::sync::{Arc, OnceLock};

/// Cap on lines the Structural index keeps (`RTK_PROXY_CCR_STRUCT_MAX_LINES`).
pub fn struct_max_lines() -> usize {
    static V: OnceLock<usize> = OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("RTK_PROXY_CCR_STRUCT_MAX_LINES")
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .filter(|n| *n >= 8)
            .unwrap_or(48)
    })
}

const RLE_MIN_RUN: usize = 3;
const ELIDE_MIN_GAP: usize = 3;
const JSON_SKELETON_MAX_CHARS: usize = 700;

/// What a compression decision looked like, for the decision log.
#[derive(Debug, Clone, Default)]
pub struct DecisionMeta {
    pub shape: &'static str,
    pub strategy: &'static str,
    pub decided_by: &'static str,
    pub features: Features,
    pub normalisers: String,
    pub elided_ranges: String,
}

pub struct Rendered {
    pub body: String,
    pub meta: DecisionMeta,
}

/// One line as strategies see it: original bytes plus measurement info.
#[derive(Clone)]
pub struct SLine {
    pub raw: String,
    /// Trimmed normalised content — the RLE comparison key, so numbered
    /// identical lines still collapse.
    pub key: String,
    pub indent: u32,
    pub blank: bool,
    pub coord: Option<u64>,
    pub annotation: bool,
}

pub struct StrategyCtx {
    pub file_coords: bool,
}

pub trait Strategy: Send + Sync {
    fn name(&self) -> &'static str;
    fn applies_to(&self, shape: Shape) -> bool;
    /// Body plus a compact elided-ranges note; `None` when not applicable.
    fn apply(&self, seq: &[SLine], ctx: &StrategyCtx) -> Option<(String, String)>;
}

fn range_note(ctx: &StrategyCtx, first: &SLine, last: &SLine, n: usize) -> String {
    match (ctx.file_coords, first.coord, last.coord) {
        (true, Some(a), Some(b)) => format!("[… {n} lines elided (lines {a}–{b}) …]"),
        _ => format!("[… {n} lines elided …]"),
    }
}

fn range_key(ctx: &StrategyCtx, first: &SLine, last: &SLine, n: usize) -> String {
    match (ctx.file_coords, first.coord, last.coord) {
        (true, Some(a), Some(b)) => format!("{a}-{b}"),
        _ => format!("×{n}"),
    }
}

struct HeadTail;
impl Strategy for HeadTail {
    fn name(&self) -> &'static str {
        "headtail"
    }
    fn applies_to(&self, shape: Shape) -> bool {
        matches!(
            shape,
            Shape::FlatUnique | Shape::Unknown | Shape::Repetitive
        )
    }
    fn apply(&self, seq: &[SLine], ctx: &StrategyCtx) -> Option<(String, String)> {
        let head = PREVIEW_HEAD_LINES;
        let tail = PREVIEW_TAIL_LINES;
        if seq.len() <= head + tail {
            return None;
        }
        let n = seq.len() - head - tail;
        let mut out: Vec<&str> = seq[..head].iter().map(|l| l.raw.as_str()).collect();
        let note = range_note(ctx, &seq[head], &seq[seq.len() - tail - 1], n);
        out.push(&note);
        out.extend(seq[seq.len() - tail..].iter().map(|l| l.raw.as_str()));
        Some((
            out.join("\n"),
            range_key(ctx, &seq[head], &seq[seq.len() - tail - 1], n),
        ))
    }
}

struct Structural;
impl Structural {
    fn kept_mask(seq: &[SLine]) -> Vec<bool> {
        let min_indent = seq
            .iter()
            .filter(|l| !l.blank && !l.annotation)
            .map(|l| l.indent)
            .min()
            .unwrap_or(0);
        let mut kept = vec![false; seq.len()];
        for i in 0..seq.len() {
            let l = &seq[i];
            if l.annotation {
                kept[i] = true;
                continue;
            }
            if l.blank {
                continue;
            }
            if l.indent == min_indent {
                kept[i] = true;
                continue;
            }
            if let Some(next) = seq[i + 1..].iter().find(|x| !x.blank && !x.annotation) {
                if next.indent > l.indent {
                    kept[i] = true;
                }
            }
        }
        let cap = struct_max_lines();
        // The cap governs structural lines; RLE annotations ride along free.
        // `levels` never holds annotations, so counting them here would index
        // it out of bounds. Counted on the same set `levels` is built from,
        // the index is in-bounds by construction.
        let mut count = seq
            .iter()
            .zip(&kept)
            .filter(|(l, k)| **k && !l.annotation)
            .count();
        if count > cap {
            let mut levels: Vec<u32> = seq
                .iter()
                .zip(&kept)
                .filter(|(l, k)| **k && !l.annotation)
                .map(|(l, _)| l.indent)
                .collect();
            levels.sort_unstable();
            let cutoff = levels[cap.saturating_sub(1)];
            for (i, l) in seq.iter().enumerate() {
                if kept[i] && !l.annotation && l.indent > cutoff && count > cap {
                    kept[i] = false;
                    count -= 1;
                }
            }
        }
        kept
    }
}
impl Strategy for Structural {
    fn name(&self) -> &'static str {
        "structural"
    }
    fn applies_to(&self, shape: Shape) -> bool {
        shape == Shape::Nested
    }
    fn apply(&self, seq: &[SLine], ctx: &StrategyCtx) -> Option<(String, String)> {
        if seq.len() < ELIDE_MIN_GAP * 2 {
            return None;
        }
        let kept = Self::kept_mask(seq);
        let mut out: Vec<String> = Vec::new();
        let mut ranges: Vec<String> = Vec::new();
        let mut i = 0;
        while i < seq.len() {
            if kept[i] {
                out.push(seq[i].raw.clone());
                i += 1;
                continue;
            }
            let start = i;
            while i < seq.len() && !kept[i] {
                i += 1;
            }
            let gap = i - start;
            if gap < ELIDE_MIN_GAP {
                for l in &seq[start..i] {
                    out.push(l.raw.clone());
                }
            } else {
                out.push(range_note(ctx, &seq[start], &seq[i - 1], gap));
                ranges.push(range_key(ctx, &seq[start], &seq[i - 1], gap));
            }
        }
        if ranges.is_empty() {
            return None;
        }
        Some((out.join("\n"), ranges.join(",")))
    }
}

struct JsonSkeleton;
impl JsonSkeleton {
    fn skel(v: &serde_json::Value, depth: usize, out: &mut String) {
        use serde_json::Value::*;
        match v {
            Null => out.push_str("null"),
            Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Number(n) => out.push_str(&n.to_string()),
            String(s) => {
                let short: std::string::String = s.chars().take(24).collect();
                if s.chars().count() > 24 {
                    out.push_str(&format!("\"{short}…\"({} chars)", s.chars().count()));
                } else {
                    out.push_str(&format!("\"{short}\""));
                }
            }
            Array(a) => {
                if a.is_empty() {
                    out.push_str("[]");
                } else if depth == 0 {
                    out.push_str(&format!("[{}× ", a.len()));
                    Self::skel(&a[0], depth + 1, out);
                    out.push(']');
                } else {
                    out.push_str(&format!("[{} items]", a.len()));
                }
            }
            Object(o) => {
                if depth >= 2 {
                    out.push_str(&format!("{{{} keys}}", o.len()));
                    return;
                }
                out.push('{');
                for (i, (k, val)) in o.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    if out.len() > JSON_SKELETON_MAX_CHARS {
                        out.push_str(&format!("… +{} more keys", o.len() - i));
                        break;
                    }
                    out.push_str(&format!("{k}: "));
                    Self::skel(val, depth + 1, out);
                }
                out.push('}');
            }
        }
    }
}
impl Strategy for JsonSkeleton {
    fn name(&self) -> &'static str {
        "json-skeleton"
    }
    fn applies_to(&self, shape: Shape) -> bool {
        shape == Shape::Structured
    }
    fn apply(&self, seq: &[SLine], _ctx: &StrategyCtx) -> Option<(String, String)> {
        let text: String = seq
            .iter()
            .map(|l| l.raw.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let v: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
        let kind = match &v {
            serde_json::Value::Array(a) => format!("array, {} items", a.len()),
            serde_json::Value::Object(o) => format!("object, {} keys", o.len()),
            _ => "value".to_string(),
        };
        let mut body = format!("[json {kind}] ");
        Self::skel(&v, 0, &mut body);
        if body.len() > JSON_SKELETON_MAX_CHARS + 200 {
            let cut = body
                .char_indices()
                .take_while(|(i, _)| *i <= JSON_SKELETON_MAX_CHARS + 200)
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(body.len());
            body.truncate(cut);
            body.push('…');
        }
        Some((body, "json".to_string()))
    }
}

struct CharWindow;
impl Strategy for CharWindow {
    fn name(&self) -> &'static str {
        "char-window"
    }
    fn applies_to(&self, shape: Shape) -> bool {
        shape == Shape::Monolithic
    }
    fn apply(&self, seq: &[SLine], _ctx: &StrategyCtx) -> Option<(String, String)> {
        let text: String = seq
            .iter()
            .map(|l| l.raw.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let total = text.chars().count();
        if total <= PREVIEW_HEAD_CHARS + PREVIEW_TAIL_CHARS {
            return None;
        }
        let head: String = text.chars().take(PREVIEW_HEAD_CHARS).collect();
        let tail_rev: Vec<char> = text.chars().rev().take(PREVIEW_TAIL_CHARS).collect();
        let tail: String = tail_rev.into_iter().rev().collect();
        let elided = total - PREVIEW_HEAD_CHARS - PREVIEW_TAIL_CHARS;
        Some((
            format!("{head}[… {elided} chars elided …]{tail}"),
            format!("chars×{elided}"),
        ))
    }
}

fn registry() -> &'static Vec<Arc<dyn Strategy>> {
    static R: OnceLock<Vec<Arc<dyn Strategy>>> = OnceLock::new();
    R.get_or_init(|| {
        vec![
            Arc::new(Structural),
            Arc::new(JsonSkeleton),
            Arc::new(CharWindow),
            Arc::new(HeadTail),
        ]
    })
}

fn to_slines(norm: &content::Normalized, original: &str) -> Vec<SLine> {
    let raw: Vec<&str> = original.split('\n').collect();
    norm.lines
        .iter()
        .zip(raw)
        .map(|(n, r)| SLine {
            raw: r.strip_suffix('\r').unwrap_or(r).to_string(),
            key: n.content.trim().to_string(),
            indent: (n.content.len() - n.content.trim_start().len()).min(400) as u32,
            blank: n.content.trim().is_empty(),
            coord: n.number,
            annotation: false,
        })
        .collect()
}

/// Lossless pre-pass: collapse runs of ≥[`RLE_MIN_RUN`] adjacent lines with
/// identical normalised content into the first line plus a count annotation.
/// Adjacent-only, so ordering survives and the collapse is reconstructible.
fn rle_collapse(seq: Vec<SLine>, ctx: &StrategyCtx) -> (Vec<SLine>, usize) {
    let mut out: Vec<SLine> = Vec::with_capacity(seq.len());
    let mut collapsed = 0usize;
    let mut i = 0;
    while i < seq.len() {
        let mut j = i + 1;
        while j < seq.len() && !seq[i].blank && seq[j].key == seq[i].key {
            j += 1;
        }
        let run = j - i;
        if run >= RLE_MIN_RUN {
            let note = match (ctx.file_coords, seq[i].coord, seq[j - 1].coord) {
                (true, Some(a), Some(b)) => {
                    format!("[… ×{run} identical lines (lines {a}–{b}) …]")
                }
                _ => format!("[… ×{run} identical lines …]"),
            };
            out.push(seq[i].clone());
            out.push(SLine {
                raw: note.clone(),
                key: note,
                indent: 0,
                blank: false,
                coord: None,
                annotation: true,
            });
            collapsed += run - 1;
        } else {
            for l in &seq[i..j] {
                out.push(l.clone());
            }
        }
        i = j;
    }
    (out, collapsed)
}

/// Shape-aware replacement body for `text`. Returns `None` when the block is
/// best left verbatim. `floor` lets the lossless RLE pass end the pipeline
/// early: if dedup alone brings the block under the floor, no lossy step runs.
pub fn rle_only(text: &str) -> Option<String> {
    let norm = content::normalize(text);
    let ctx = StrategyCtx {
        file_coords: norm.file_coords,
    };
    let seq = to_slines(&norm, text);
    let (seq, collapsed) = rle_collapse(seq, &ctx);
    if collapsed == 0 {
        return None;
    }
    Some(
        seq.iter()
            .map(|l| l.raw.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

pub fn render(text: &str, floor: usize, count: fn(&str) -> usize) -> Option<Rendered> {
    let norm = content::normalize(text);
    let features = content::extract(&norm, text.chars().count());
    let (shape, decided_by) = content::classify(text, &features);
    let ctx = StrategyCtx {
        file_coords: norm.file_coords,
    };
    let normalisers = norm.normaliser_tags();

    let seq = to_slines(&norm, text);
    let (seq, collapsed) = rle_collapse(seq, &ctx);

    let deduped_body: String = seq
        .iter()
        .map(|l| l.raw.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let dedup_meta = |ranges: String| DecisionMeta {
        shape: shape.as_str(),
        strategy: "dedup",
        decided_by,
        features,
        normalisers: normalisers.clone(),
        elided_ranges: ranges,
    };

    // Lossless RLE alone brought it under the floor: stop, no lossy step.
    if collapsed > 0 && count(&deduped_body) < floor {
        return Some(Rendered {
            body: deduped_body,
            meta: dedup_meta(format!("rle×{collapsed}")),
        });
    }

    let lossy = registry()
        .iter()
        .find(|s| s.applies_to(shape))
        .and_then(|s| s.apply(&seq, &ctx).map(|r| (s.name(), r)))
        .or_else(|| HeadTail.apply(&seq, &ctx).map(|r| ("headtail", r)));

    match lossy {
        Some((name, (body, elided))) => Some(Rendered {
            body,
            meta: DecisionMeta {
                shape: shape.as_str(),
                strategy: name,
                decided_by,
                features,
                normalisers,
                elided_ranges: elided,
            },
        }),
        // No lossy strategy shrank it further, but lossless RLE did: keep that.
        None if collapsed > 0 => Some(Rendered {
            body: deduped_body,
            meta: dedup_meta(format!("rle×{collapsed}")),
        }),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes4(t: &str) -> usize {
        t.len().div_ceil(4)
    }

    fn render_ok(text: &str) -> Rendered {
        render(text, 1, bytes4).expect("render")
    }

    fn code() -> String {
        let mut s = String::new();
        for i in 0..40 {
            s.push_str(&format!("fn item_{i}(x: u32) -> u32 {{\n"));
            s.push_str("    let y = x + 1;\n");
            s.push_str("    if y > 2 {\n        return y * 2;\n    }\n");
            s.push_str("    y\n}\n");
        }
        s
    }

    #[test]
    fn structural_keeps_declarations_and_emits_ranges() {
        let r = render_ok(&code());
        assert_eq!(r.meta.strategy, "structural");
        assert!(r.body.contains("fn item_0"));
        assert!(r.body.contains("fn item_39"));
        assert!(r.body.contains("lines elided"));
        assert!(!r.meta.elided_ranges.is_empty());
        assert!(!r.body.contains("return y * 2"));
    }

    #[test]
    fn structural_on_numbered_code_emits_file_coordinates() {
        let numbered: String = code()
            .lines()
            .enumerate()
            .map(|(i, l)| format!("{:>6}→{l}", i + 1))
            .collect::<Vec<_>>()
            .join("\n");
        let r = render_ok(&numbered);
        assert_eq!(r.meta.strategy, "structural");
        assert!(
            r.body.contains("(lines "),
            "coordinates missing: {}",
            &r.body[..300.min(r.body.len())]
        );
    }

    #[test]
    fn rle_collapses_adjacent_identical_lines_only() {
        let t = format!(
            "start\n{}\nmiddle\n{}\nend",
            "same line\n".repeat(30).trim_end(),
            "same line\n".repeat(30).trim_end()
        );
        let r = render_ok(&t);
        assert_eq!(r.meta.strategy, "dedup");
        assert_eq!(r.body.matches("×30 identical lines").count(), 2);
        assert!(r.body.contains("start"));
        assert!(r.body.contains("middle"));
        assert!(r.body.contains("end"));
    }

    #[test]
    fn interleaved_lines_are_not_collapsed() {
        let t = "a\nb\n".repeat(40);
        let r = render(&t, 1, bytes4);
        if let Some(r) = r {
            assert_ne!(r.meta.strategy, "dedup");
        }
    }

    #[test]
    fn json_skeleton_reports_keys_and_lengths() {
        let inner: Vec<String> = (0..300)
            .map(|i| format!("{{\"id\":{i},\"name\":\"row{i}\"}}"))
            .collect();
        let t = format!("[{}]", inner.join(","));
        let r = render_ok(&t);
        assert_eq!(r.meta.strategy, "json-skeleton");
        assert!(r.body.contains("300"));
        assert!(r.body.len() < 1200);
    }

    #[test]
    fn monolithic_blob_gets_char_window_with_count() {
        let t = "QUJD".repeat(600);
        let r = render_ok(&t);
        assert_eq!(r.meta.strategy, "char-window");
        assert!(r.body.contains("chars elided"));
    }

    #[test]
    fn flat_listing_gets_headtail() {
        let t: String = (0..80)
            .map(|i| format!("src/module_{i}/file_{i}.rs"))
            .collect::<Vec<_>>()
            .join("\n");
        let r = render_ok(&t);
        assert_eq!(r.meta.strategy, "headtail");
        assert!(r.body.contains("src/module_0/"));
        assert!(r.body.contains("src/module_79/"));
        assert!(r.body.contains("lines elided"));
    }

    #[test]
    fn render_is_deterministic() {
        let t = code();
        let a = render_ok(&t);
        let b = render_ok(&t);
        assert_eq!(a.body, b.body);
        assert_eq!(a.meta.elided_ranges, b.meta.elided_ranges);
    }

    #[test]
    fn annotations_never_collide_with_the_retrieve_marker() {
        for t in [
            code(),
            "same\n".repeat(50),
            (0..80)
                .map(|i| format!("f {i}"))
                .collect::<Vec<_>>()
                .join("\n"),
        ] {
            if let Some(r) = render(&t, 1, bytes4) {
                assert!(!r.body.contains(" tokens — call rtk_retrieve(\""));
            }
        }
    }

    #[test]
    fn dedup_stops_before_lossy_when_under_floor() {
        let t = format!("head\n{}\ntail", "same line\n".repeat(200).trim_end());
        let r = render(&t, 1_000_000, bytes4).expect("render");
        assert_eq!(r.meta.strategy, "dedup");
        assert!(r.body.contains("×200 identical lines"));
    }

    #[test]
    fn struct_cap_limits_kept_lines() {
        // Functions with real multi-line bodies, so Structural has ≥3-line
        // gaps to elide (single-line bodies correctly fall through to headtail).
        let mut s = String::new();
        for i in 0..120 {
            s.push_str(&format!("fn f_{i}(x: u32) -> u32 {{\n"));
            s.push_str("    let a = x + 1;\n    let b = a * 2;\n    let c = b - 3;\n");
            s.push_str("    c\n}\n");
        }
        let r = render_ok(&s);
        assert_eq!(r.meta.strategy, "structural");
        assert!(r.body.lines().count() < s.lines().count());
    }
}

#[cfg(test)]
mod rle_levels {
    use super::*;

    fn bytes4(s: &str) -> usize {
        s.len() / 4
    }

    /// If `count` tallied every kept line, RLE annotations included, while
    /// `levels` collects only non-annotations, ordinary code with a few
    /// identical indented runs would index `levels` out of bounds.
    #[test]
    fn struct_selection_survives_rle_annotations_near_the_cap() {
        let mut lines: Vec<String> = (0..44).map(|i| format!("def handler_{i}(req):")).collect();
        for group in 0..5 {
            for _ in 0..3 {
                lines.push(format!("    retry_{group}()"));
            }
        }
        let text = lines.join("\n");
        let rendered = render(&text, 1, bytes4);
        assert!(rendered.is_some(), "ordinary output must render, not panic");
    }
}
