use crate::block::Provenance;
use regex::Regex;
use std::sync::LazyLock;

pub const TRUNCATION_MARKER_TAG: &str = "lines omitted by rtk-proxy";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp<'a> {
    pub method: &'a str,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub hint: Option<&'a str>,
}

impl Stamp<'_> {
    pub fn render(&self) -> String {
        match self.hint {
            Some(hint) => format!(
                "[rtk:{} {}→{} tokens · {}]",
                self.method, self.tokens_before, self.tokens_after, hint
            ),
            None => format!(
                "[rtk:{} {}→{} tokens]",
                self.method, self.tokens_before, self.tokens_after
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedStamp {
    pub method: String,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub hint: Option<String>,
}

static STAMP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[rtk:([a-z0-9][a-z0-9-]*) (\d+)→(\d+) tokens(?: · ([^\]]+))?\]")
        .expect("stamp grammar regex is a literal")
});

pub fn parse(text: &str) -> Vec<ParsedStamp> {
    STAMP_RE
        .captures_iter(text)
        .filter_map(|c| {
            Some(ParsedStamp {
                method: c.get(1)?.as_str().to_string(),
                tokens_before: c.get(2)?.as_str().parse().ok()?,
                tokens_after: c.get(3)?.as_str().parse().ok()?,
                hint: c.get(4).map(|m| m.as_str().to_string()),
            })
        })
        .collect()
}

pub fn provenance_of(text: &str) -> Provenance {
    Provenance {
        applied: parse(text).into_iter().map(|s| s.method).collect(),
    }
}

/// Text the hook site leaves behind, used by the proxy to skip a block that has
/// already been compressed once (single-site resolution). `cap`'s current
/// marker is `[N lines elided]`; the longer phrasing is kept because it is
/// already sitting in conversation histories written by earlier versions.
const HOOK_MARKER_SIGNATURES: &[&str] = &[
    "lines elided]",
    "full output in recall file]",
    "[full output: ",
];

pub fn carries_hook_stamp(text: &str) -> bool {
    if HOOK_MARKER_SIGNATURES.iter().any(|sig| text.contains(sig)) {
        return true;
    }
    !parse(text).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_parse_round_trip() {
        let stamp = Stamp {
            method: "base64-elide",
            tokens_before: 900,
            tokens_after: 40,
            hint: Some("rtk recall 9f8e7d"),
        };
        let text = format!("output head\n{}\n", stamp.render());
        let parsed = parse(&text);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].method, "base64-elide");
        assert_eq!(parsed[0].tokens_before, 900);
        assert_eq!(parsed[0].tokens_after, 40);
        assert_eq!(parsed[0].hint.as_deref(), Some("rtk recall 9f8e7d"));
    }

    #[test]
    fn hintless_stamp_round_trips() {
        let stamp = Stamp {
            method: "dedup",
            tokens_before: 100,
            tokens_after: 60,
            hint: None,
        };
        let parsed = parse(&stamp.render());
        assert_eq!(parsed[0].hint, None);
    }

    #[test]
    fn provenance_lists_methods_in_order() {
        let text = "[rtk:cap 10→5 tokens]\nmiddle\n[rtk:ccr 200→20 tokens · rtk_retrieve(\"x\")]";
        let prov = provenance_of(text);
        assert_eq!(prov.applied, vec!["cap".to_string(), "ccr".to_string()]);
        assert!(prov.contains("ccr"));
    }

    #[test]
    fn plain_brackets_are_not_stamps() {
        assert!(parse("[1200 lines omitted] [rtk: nope] [x→y tokens]").is_empty());
    }

    #[test]
    fn truncation_tag_value_is_frozen() {
        assert_eq!(TRUNCATION_MARKER_TAG, "lines omitted by rtk-proxy");
    }
}
