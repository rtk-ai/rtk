//! Layer 1+2 of shape-aware compression: normalise presentation artifacts,
//! extract content-intrinsic features, classify into a [`Shape`]. No tool
//! names, no format names, no regex patterns — structural statistics only,
//! in fixed-point integer arithmetic so a classification can never drift.

/// Separators whose stripped line numbers are trusted as FILE coordinates.
/// `:` and `. ` are not here on purpose (grep hits, prose lists).
const TRUSTED_SEPS: [char; 3] = ['\t', '→', '|'];

/// All numbering separators recognised for measurement stripping.
const ALL_SEPS: [char; 4] = ['\t', '→', '|', ':'];

/// One normalised line: measurement content plus its source coordinates.
#[derive(Debug, Clone)]
pub struct NLine {
    /// Content with ANSI, CR and any line-number prefix removed.
    pub content: String,
    /// Stripped line number, when a numbering scheme was detected.
    pub number: Option<u64>,
}

/// Result of normalising a block for measurement.
#[derive(Debug)]
pub struct Normalized {
    pub lines: Vec<NLine>,
    /// Numbering was detected AND its separator marks file coordinates.
    pub file_coords: bool,
    pub had_numbering: bool,
    pub had_ansi: bool,
    pub had_crlf: bool,
}

impl Normalized {
    pub fn normaliser_tags(&self) -> String {
        let mut v = Vec::new();
        if self.had_numbering {
            v.push("linenum");
        }
        if self.had_ansi {
            v.push("ansi");
        }
        if self.had_crlf {
            v.push("crlf");
        }
        v.join("|")
    }
}

fn strip_ansi(s: &str) -> (String, bool) {
    if !s.contains('\u{1b}') {
        return (s.to_string(), false);
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                for f in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&f) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                while let Some(f) = chars.next() {
                    if f == '\u{7}' {
                        break;
                    }
                    if f == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {
                chars.next();
            }
        }
    }
    (out, true)
}

fn parse_numbered(line: &str) -> Option<(u64, char, usize)> {
    let mut idx = 0;
    let bytes = line.as_bytes();
    while idx < bytes.len() && bytes[idx] == b' ' {
        idx += 1;
    }
    let start = idx;
    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
        idx += 1;
    }
    if idx == start || idx - start > 8 {
        return None;
    }
    let num: u64 = line[start..idx].parse().ok()?;
    let rest = &line[idx..];
    let sep = rest.chars().next()?;
    if !ALL_SEPS.contains(&sep) {
        return None;
    }
    Some((num, sep, idx + sep.len_utf8()))
}

/// Normalise a block for measurement. The emitted replacement is always
/// built from the original bytes — this output is never sent upstream.
pub fn normalize(text: &str) -> Normalized {
    let had_crlf = text.contains('\r');
    let raw: Vec<&str> = text.split('\n').collect();
    let mut cleaned: Vec<(String, bool)> = Vec::with_capacity(raw.len());
    let mut had_ansi = false;
    for l in &raw {
        let l = l.strip_suffix('\r').unwrap_or(l);
        let (c, a) = strip_ansi(l);
        had_ansi |= a;
        cleaned.push((c, a));
    }

    let mut parsed: Vec<Option<(u64, char, usize)>> = Vec::with_capacity(cleaned.len());
    let mut nonblank = 0usize;
    for (c, _) in &cleaned {
        if c.trim().is_empty() {
            parsed.push(None);
        } else {
            nonblank += 1;
            parsed.push(parse_numbered(c));
        }
    }
    let numbered: Vec<(u64, char)> = parsed.iter().flatten().map(|(n, s, _)| (*n, *s)).collect();
    let mut numbering_sep: Option<char> = None;
    if nonblank >= 5 && numbered.len() * 10 >= nonblank * 8 {
        let sep = numbered[0].1;
        let same_sep = numbered.iter().all(|(_, s)| *s == sep);
        let increasing = numbered.windows(2).all(|w| w[1].0 > w[0].0);
        if same_sep && increasing {
            numbering_sep = Some(sep);
        }
    }

    let file_coords = numbering_sep.is_some_and(|s| TRUSTED_SEPS.contains(&s));
    let lines = cleaned
        .into_iter()
        .zip(parsed)
        .map(|((c, _), p)| match (numbering_sep, p) {
            (Some(_), Some((num, _, off))) => NLine {
                content: c[off..].to_string(),
                number: Some(num),
            },
            _ => NLine {
                content: c,
                number: None,
            },
        })
        .collect();

    Normalized {
        lines,
        file_coords,
        had_numbering: numbering_sep.is_some(),
        had_ansi,
        had_crlf,
    }
}

/// Content-intrinsic features, all in thousandths (fixed-point).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Features {
    pub indent_sd: u32,
    pub prefix_rep: u32,
    pub unique_ratio: u32,
    pub bracket_balance: u32,
    pub bracket_density: u32,
    pub line_len_sd: u32,
    pub blank_ratio: u32,
    pub nonblank: u32,
    pub chars: u32,
}

fn isqrt(v: u64) -> u64 {
    if v < 2 {
        return v;
    }
    let mut x = v;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + v / x) / 2;
    }
    x
}

fn sd_milli(values: &[u32]) -> u32 {
    let n = values.len() as u64;
    if n < 2 {
        return 0;
    }
    let sum: u64 = values.iter().map(|v| *v as u64).sum();
    let sumsq: u64 = values.iter().map(|v| (*v as u64) * (*v as u64)).sum();
    let var_e6 = (sumsq * n)
        .saturating_sub(sum * sum)
        .saturating_mul(1_000_000)
        / (n * n);
    isqrt(var_e6) as u32
}

pub fn extract(norm: &Normalized, original_chars: usize) -> Features {
    let total = norm.lines.len().max(1);
    let nb: Vec<&NLine> = norm
        .lines
        .iter()
        .filter(|l| !l.content.trim().is_empty())
        .collect();
    let n = nb.len();
    if n == 0 {
        return Features {
            blank_ratio: 1000,
            chars: original_chars.min(u32::MAX as usize) as u32,
            ..Default::default()
        };
    }

    let indents: Vec<u32> = nb
        .iter()
        .map(|l| (l.content.len() - l.content.trim_start().len()).min(400) as u32)
        .collect();
    let lens: Vec<u32> = nb
        .iter()
        .map(|l| l.content.chars().count().min(4000) as u32)
        .collect();

    let mut prefixes = std::collections::HashSet::new();
    let mut uniques = std::collections::HashSet::new();
    let (mut open, mut close) = (0u64, 0u64);
    for l in &nb {
        let t = l.content.trim();
        prefixes.insert(t.chars().take(12).collect::<String>());
        uniques.insert(t);
        for c in t.chars() {
            match c {
                '(' | '{' | '[' => open += 1,
                ')' | '}' | ']' => close += 1,
                _ => {}
            }
        }
    }
    let bal = match open + close {
        0 => 1000,
        tot => 1000 - (1000 * open.abs_diff(close) / tot) as u32,
    };

    Features {
        indent_sd: sd_milli(&indents),
        prefix_rep: (1000 - 1000 * prefixes.len() / n) as u32,
        unique_ratio: (1000 * uniques.len() / n) as u32,
        bracket_balance: bal,
        bracket_density: ((open + close) * 1000 / n as u64).min(u32::MAX as u64) as u32,
        line_len_sd: sd_milli(&lens),
        blank_ratio: (1000 * (total - n) / total) as u32,
        nonblank: n as u32,
        chars: original_chars.min(u32::MAX as usize) as u32,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Nested,
    Repetitive,
    FlatUnique,
    Structured,
    Monolithic,
    Unknown,
}

impl Shape {
    pub fn as_str(&self) -> &'static str {
        match self {
            Shape::Nested => "nested",
            Shape::Repetitive => "repetitive",
            Shape::FlatUnique => "flat-unique",
            Shape::Structured => "structured",
            Shape::Monolithic => "monolithic",
            Shape::Unknown => "unknown",
        }
    }
}

/// Cut points, chosen on error
/// asymmetry — misrouting a log to `Nested` costs ratio, misrouting code
/// away from it costs the paging loop, so `Nested` is checked first.
const NESTED_INDENT_SD_MILLI: u32 = 1500;
const REPETITIVE_PREFIX_MILLI: u32 = 500;
const FLAT_UNIQUE_MILLI: u32 = 950;
const MONOLITHIC_MAX_LINES: u32 = 3;
const MONOLITHIC_MIN_CHARS: u32 = 600;
const JSON_PARSE_MAX_BYTES: usize = 5 * 1024 * 1024;

pub fn classify(text: &str, f: &Features) -> (Shape, &'static str) {
    if f.nonblank <= MONOLITHIC_MAX_LINES && f.chars > MONOLITHIC_MIN_CHARS {
        let t = text.trim();
        if text.len() <= JSON_PARSE_MAX_BYTES
            && (t.starts_with('{') || t.starts_with('['))
            && (t.ends_with('}') || t.ends_with(']'))
            && serde_json::from_str::<serde_json::Value>(t).is_ok()
        {
            return (Shape::Structured, "parses-as-json");
        }
        return (Shape::Monolithic, "lines<=3&chars>600");
    }
    if f.indent_sd >= NESTED_INDENT_SD_MILLI {
        return (Shape::Nested, "indent_sd>=1.5");
    }
    if f.prefix_rep >= REPETITIVE_PREFIX_MILLI {
        return (Shape::Repetitive, "prefix_rep>=0.5");
    }
    if f.unique_ratio >= FLAT_UNIQUE_MILLI {
        return (Shape::FlatUnique, "unique>=0.95");
    }
    (Shape::Unknown, "no-rule")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code_block() -> String {
        let mut s = String::new();
        for i in 0..40 {
            s.push_str(&format!("fn item_{i}(x: u32) -> u32 {{\n"));
            s.push_str("    let y = x + 1;\n");
            s.push_str("    if y > 2 {\n        return y * 2;\n    }\n");
            s.push_str("    y\n}\n\n");
        }
        s
    }

    fn log_block() -> String {
        (0..80)
            .map(|i| format!("2026-09-11T12:00:{:02}Z INFO worker: heartbeat ok", i % 60))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn listing_block() -> String {
        // grep -l style: distinct paths, no shared 12-char prefix
        (0..60)
            .map(|i| format!("{i:02}_pkg/handler_{i}.rs:matched"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn code_classifies_nested() {
        let t = code_block();
        let n = normalize(&t);
        let f = extract(&n, t.chars().count());
        assert!(f.indent_sd >= 1500, "indent_sd={}", f.indent_sd);
        assert_eq!(classify(&t, &f).0, Shape::Nested);
    }

    #[test]
    fn log_classifies_repetitive() {
        let t = log_block();
        let n = normalize(&t);
        let f = extract(&n, t.chars().count());
        assert_eq!(classify(&t, &f).0, Shape::Repetitive);
    }

    #[test]
    fn listing_classifies_flat_unique() {
        let t = listing_block();
        let n = normalize(&t);
        let f = extract(&n, t.chars().count());
        assert_eq!(classify(&t, &f).0, Shape::FlatUnique);
    }

    #[test]
    fn minified_json_classifies_structured() {
        let inner: Vec<String> = (0..200).map(|i| format!("{{\"k{i}\":{i}}}")).collect();
        let t = format!("[{}]", inner.join(","));
        let n = normalize(&t);
        let f = extract(&n, t.chars().count());
        assert_eq!(classify(&t, &f).0, Shape::Structured);
    }

    #[test]
    fn base64_blob_classifies_monolithic() {
        let t = "QUJD".repeat(500);
        let n = normalize(&t);
        let f = extract(&n, t.chars().count());
        assert_eq!(classify(&t, &f).0, Shape::Monolithic);
    }

    #[test]
    fn numbered_code_is_normalised_back_to_nested() {
        let plain = code_block();
        let numbered: String = plain
            .lines()
            .enumerate()
            .map(|(i, l)| format!("{:>6}→{l}", i + 1))
            .collect::<Vec<_>>()
            .join("\n");
        let n = normalize(&numbered);
        assert!(n.had_numbering);
        assert!(n.file_coords);
        assert_eq!(n.lines[0].number, Some(1));
        let f = extract(&n, numbered.chars().count());
        assert_eq!(classify(&numbered, &f).0, Shape::Nested);
    }

    #[test]
    fn grep_colon_numbers_are_stripped_but_not_file_coords() {
        let t: String = (0..30)
            .map(|i| format!("{}:match line body {}", i * 7 + 3, i))
            .collect::<Vec<_>>()
            .join("\n");
        let n = normalize(&t);
        assert!(n.had_numbering);
        assert!(!n.file_coords);
    }

    #[test]
    fn prose_numbered_list_is_not_numbering() {
        let t: String = (1..=20)
            .map(|i| format!("{i}. item number {i} in the list"))
            .collect::<Vec<_>>()
            .join("\n");
        let n = normalize(&t);
        assert!(!n.had_numbering);
    }

    #[test]
    fn ansi_is_stripped_for_measurement() {
        let t = (0..30)
            .map(|i| format!("\u{1b}[32mok\u{1b}[0m step {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let n = normalize(&t);
        assert!(n.had_ansi);
        assert!(n.lines[0].content.starts_with("ok step"));
    }

    #[test]
    fn empty_and_blank_are_degenerate_not_panicking() {
        for t in ["", "   \n\n  \n", "\n"] {
            let n = normalize(t);
            let f = extract(&n, t.chars().count());
            assert_eq!(f.nonblank, 0);
            let _ = classify(t, &f);
        }
    }

    #[test]
    fn features_are_deterministic() {
        let t = code_block();
        let a = extract(&normalize(&t), t.chars().count());
        let b = extract(&normalize(&t), t.chars().count());
        assert_eq!(a, b);
    }

    #[test]
    fn crlf_is_normalised() {
        let t = "a\r\nb\r\nc";
        let n = normalize(t);
        assert!(n.had_crlf);
        assert_eq!(n.lines[0].content, "a");
    }
}
