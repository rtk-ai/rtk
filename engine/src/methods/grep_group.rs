//! `grep-group` layer: groups `path:line:content` match lines by file.
//!
//! Same colon-heuristic parsing as `pipe_cmd::grep_wrapper`, cap-aware.
//!
//! Faithfulness guard: if any non-empty line does not parse as
//! `path:line:content` (e.g. `-C` context lines using `-` separators), the
//! whole input passes through unchanged — grouping must never drop lines.
//! Lossless by design: every match is emitted (no per-file cap) — the
//! posthook path converts, it never truncates.

use crate::block::Block;
use crate::method::{Method, MethodCtx, MethodInfo, Outcome};
use std::collections::HashMap;

pub struct GrepGroup;

impl Method for GrepGroup {
    fn name(&self) -> &'static str {
        "grep-group"
    }

    fn info(&self) -> MethodInfo {
        MethodInfo {
            consumes: &[crate::block::ContentFormat::Matches],
            // Trimming match lines drops their indentation -- which IS the
            // semantics in Python or YAML -- and blank matches vanish.
            lossy: true,
            recovery: crate::method::Recovery::Recall,
            ..MethodInfo::LOSSLESS_TRANSFORM
        }
    }

    fn apply(&self, block: &Block, _ctx: &MethodCtx) -> Outcome {
        let input = block.text;
        let mut by_file: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
        let mut order: Vec<&str> = Vec::new();
        let mut total = 0;

        for line in input.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.splitn(3, ':').collect();
            let parsed = parts.len() == 3 && parts[1].parse::<usize>().is_ok();
            if !parsed {
                // Context lines or unexpected format: stay faithful, pass through.
                return Outcome::Continue(input.to_string());
            }
            total += 1;
            let entry = by_file.entry(parts[0]).or_default();
            if entry.is_empty() {
                order.push(parts[0]);
            }
            entry.push((parts[1], parts[2]));
        }

        if total == 0 {
            return Outcome::Continue(input.to_string());
        }

        let mut out = format!("{} matches in {}F:\n\n", total, by_file.len());
        for file in order {
            let matches = &by_file[file];
            out.push_str(&format!("[file] {} ({}):\n", file, matches.len()));
            for (line_num, content) in matches {
                out.push_str(&format!("  {:>4}: {}\n", line_num, content.trim()));
            }
            out.push('\n');
        }

        Outcome::Continue(out.trim_end().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::ContentFormat;

    fn ctx() -> MethodCtx<'static> {
        MethodCtx { source: "grep" }
    }

    fn blk(text: &str) -> Block<'_> {
        crate::testutil::block(text, ContentFormat::Matches)
    }

    fn apply(input: &str) -> String {
        match GrepGroup.apply(&blk(input), &ctx()) {
            Outcome::Continue(out) | Outcome::ShortCircuit(out) => out,
            Outcome::Skip => input.to_string(),
        }
    }

    fn fixture_content(raw: &str) -> String {
        let event: serde_json::Value = serde_json::from_str(raw).expect("fixture parses");
        event["tool_response"]["content"]
            .as_str()
            .expect("fixture has content")
            .to_string()
    }

    #[test]
    fn test_groups_real_grep_content_by_file() {
        let input = fixture_content(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/posthook/grep_content_small.json"
        )));
        let out = apply(&input);
        assert!(out.starts_with("43 matches in 1F:"), "header: {out}");
        assert_eq!(
            out.matches("haystack.rs").count(),
            1,
            "filename must appear once, not per line"
        );
    }

    #[test]
    fn test_large_truncated_fixture_savings_at_least_20_percent() {
        let input = fixture_content(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/posthook/grep_content_large_truncated.json"
        )));
        let out = apply(&input);
        let savings = 100.0 - (out.len() as f64 / input.len() as f64 * 100.0);
        assert!(
            savings >= 20.0,
            "grep-group: expected ≥20% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_all_matches_emitted_no_cap() {
        let input = (1..=40)
            .map(|i| format!("src/big.rs:{i}:let x = {i};"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = apply(&input);
        assert!(out.starts_with("40 matches in 1F:"));
        assert!(
            out.contains("  40: let x = 40;"),
            "last match present: {out}"
        );
        assert!(
            !out.contains('+'),
            "no overflow marker, grouping is lossless"
        );
    }

    #[test]
    fn test_context_lines_pass_through_unchanged() {
        // `grep -C` context lines use `-` separators; grouping must not drop them.
        let input = "src/a.rs:10:match line\nsrc/a.rs-11-context line";
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_non_grep_content_passes_through() {
        let input = "just some text\nwithout match format";
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(apply(""), "");
    }
}
