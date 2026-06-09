//! Dedup layer: collapse consecutive repeated lines into `[×N] line`.

use crate::core::stream::StreamFilter;
use lazy_static::lazy_static;
use regex::Regex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DedupLevel {
    None,
    #[default]
    Exact,
    Normalized,
}

impl DedupLevel {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "off" => Some(Self::None),
            "exact" => Some(Self::Exact),
            "normalized" | "normalised" | "fuzzy" => Some(Self::Normalized),
            _ => None,
        }
    }
}

lazy_static! {
    static ref HEX_RE: Regex = Regex::new(r"0x[0-9a-fA-F]+").unwrap();
    static ref NUM_RE: Regex = Regex::new(r"\d+").unwrap();
}

// Normalized key masks volatile tokens (hex addresses, numbers) so near-identical
// lines — timestamps, counters, ids — collapse together.
fn key(line: &str, level: DedupLevel) -> String {
    match level {
        DedupLevel::None | DedupLevel::Exact => line.to_string(),
        DedupLevel::Normalized => {
            let masked = HEX_RE.replace_all(line, "0x#");
            NUM_RE.replace_all(&masked, "#").into_owned()
        }
    }
}

struct Pending {
    display: String,
    key: String,
    count: usize,
}

fn render(p: Pending) -> String {
    if p.count > 1 {
        format!("[×{}] {}", p.count, p.display)
    } else {
        p.display
    }
}

struct Collapser {
    level: DedupLevel,
    pending: Option<Pending>,
}

impl Collapser {
    fn new(level: DedupLevel) -> Self {
        Self {
            level,
            pending: None,
        }
    }

    fn feed(&mut self, line: &str) -> Option<String> {
        let k = key(line, self.level);
        if let Some(p) = &mut self.pending {
            if p.key == k {
                p.count += 1;
                return None;
            }
        }
        let emit = self.pending.take().map(render);
        self.pending = Some(Pending {
            display: line.to_string(),
            key: k,
            count: 1,
        });
        emit
    }

    fn flush(&mut self) -> Option<String> {
        self.pending.take().map(render)
    }
}

pub fn apply(input: &str, level: DedupLevel) -> String {
    if level == DedupLevel::None {
        return input.to_string();
    }
    let mut collapser = Collapser::new(level);
    let mut out: Vec<String> = Vec::new();
    for line in input.lines() {
        if let Some(emit) = collapser.feed(line) {
            out.push(emit);
        }
    }
    if let Some(tail) = collapser.flush() {
        out.push(tail);
    }
    out.join("\n")
}

struct Dedup<'a> {
    inner: Box<dyn StreamFilter + 'a>,
    collapser: Collapser,
}

impl StreamFilter for Dedup<'_> {
    fn feed_line(&mut self, line: &str) -> Option<String> {
        let emit = self.collapser.feed(line)?;
        self.inner.feed_line(&emit)
    }

    fn flush(&mut self) -> String {
        let mut out = String::new();
        if let Some(tail) = self.collapser.flush() {
            if let Some(o) = self.inner.feed_line(&tail) {
                out.push_str(&o);
            }
        }
        out.push_str(&self.inner.flush());
        out
    }

    fn on_exit(&mut self, exit_code: i32, raw: &str) -> Option<String> {
        self.inner.on_exit(exit_code, raw)
    }
}

pub(super) fn wrap_stream<'a>(
    inner: Box<dyn StreamFilter + 'a>,
    level: DedupLevel,
) -> Box<dyn StreamFilter + 'a> {
    if level == DedupLevel::None {
        return inner;
    }
    Box::new(Dedup {
        inner,
        collapser: Collapser::new(level),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_collapses_consecutive_repeats() {
        assert_eq!(apply("a\na\na\nb", DedupLevel::Exact), "[×3] a\nb");
    }

    #[test]
    fn exact_keeps_distinct_lines() {
        assert_eq!(apply("a\nb\nc", DedupLevel::Exact), "a\nb\nc");
    }

    #[test]
    fn exact_does_not_collapse_nonconsecutive() {
        assert_eq!(apply("a\nb\na", DedupLevel::Exact), "a\nb\na");
    }

    #[test]
    fn normalized_collapses_volatile_tokens() {
        let input = "[12:00:01] retry 1\n[12:00:02] retry 2\n[12:00:03] retry 3";
        assert_eq!(
            apply(input, DedupLevel::Normalized),
            "[×3] [12:00:01] retry 1"
        );
    }

    #[test]
    fn none_is_identity() {
        assert_eq!(apply("a\na\na\nb", DedupLevel::None), "a\na\na\nb");
        assert_eq!(DedupLevel::parse("none"), Some(DedupLevel::None));
        assert_eq!(DedupLevel::parse("off"), Some(DedupLevel::None));
    }

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(apply("", DedupLevel::Exact), "");
    }

    #[test]
    fn parse_accepts_known_values() {
        assert_eq!(DedupLevel::parse("exact"), Some(DedupLevel::Exact));
        assert_eq!(
            DedupLevel::parse("normalized"),
            Some(DedupLevel::Normalized)
        );
        assert_eq!(DedupLevel::parse("bogus"), None);
    }
}
