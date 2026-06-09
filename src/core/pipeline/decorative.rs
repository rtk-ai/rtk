//! Decorative layer: lossless chrome removal (ANSI, blank runs, box-drawing).

use crate::core::stream::StreamFilter;
use crate::core::utils::strip_ansi;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DecorativeLevel {
    None,
    Light,
    #[default]
    Reasonable,
    High,
}

impl DecorativeLevel {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "off" => Some(Self::None),
            "light" | "low" => Some(Self::Light),
            "reasonable" | "normal" | "default" | "medium" | "med" => Some(Self::Reasonable),
            "high" | "aggressive" => Some(Self::High),
            _ => None,
        }
    }
}

pub fn apply(input: &str, level: DecorativeLevel) -> String {
    if level == DecorativeLevel::None {
        return input.to_string();
    }
    if level == DecorativeLevel::Light {
        return strip_ansi(input);
    }

    let mut prev_blank = false;
    let mut out: Vec<String> = input
        .lines()
        .filter_map(|line| line_step(line, level, &mut prev_blank))
        .collect();

    while out.first().is_some_and(String::is_empty) {
        out.remove(0);
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out.join("\n")
}

// None = line dropped (redundant blank, or decoration at High).
fn line_step(line: &str, level: DecorativeLevel, prev_blank: &mut bool) -> Option<String> {
    let stripped = strip_ansi(line);
    if level == DecorativeLevel::Light {
        return Some(stripped);
    }
    let trimmed = stripped.trim_end();
    if trimmed.is_empty() {
        if *prev_blank {
            return None;
        }
        *prev_blank = true;
        return Some(String::new());
    }
    if level == DecorativeLevel::High && is_decoration_line(trimmed) {
        return None;
    }
    *prev_blank = false;
    Some(trimmed.to_string())
}

fn is_decoration_line(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|c| is_decoration_char(c) || c.is_whitespace())
}

// Box Drawing + Block Elements; ASCII rules (---, ===) are left alone since they
// often carry meaning.
fn is_decoration_char(c: char) -> bool {
    matches!(c, '\u{2500}'..='\u{259F}') || matches!(c, '•' | '·')
}

struct Decorating<'a> {
    inner: Box<dyn StreamFilter + 'a>,
    level: DecorativeLevel,
    prev_blank: bool,
}

impl StreamFilter for Decorating<'_> {
    fn feed_line(&mut self, line: &str) -> Option<String> {
        let clean = line_step(line, self.level, &mut self.prev_blank)?;
        self.inner.feed_line(&clean)
    }

    fn flush(&mut self) -> String {
        self.inner.flush()
    }

    fn on_exit(&mut self, exit_code: i32, raw: &str) -> Option<String> {
        self.inner.on_exit(exit_code, raw)
    }
}

pub(super) fn wrap_stream<'a>(
    inner: Box<dyn StreamFilter + 'a>,
    level: DecorativeLevel,
) -> Box<dyn StreamFilter + 'a> {
    if level == DecorativeLevel::None {
        return inner;
    }
    Box::new(Decorating {
        inner,
        level,
        prev_blank: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_strips_ansi_only() {
        let out = apply("\x1b[32mok\x1b[0m\n\n\ntrailing   ", DecorativeLevel::Light);
        assert_eq!(out, "ok\n\n\ntrailing   ");
    }

    #[test]
    fn reasonable_collapses_blanks_and_trims() {
        let out = apply("a\n\n\n\nb   \n", DecorativeLevel::Reasonable);
        assert_eq!(out, "a\n\nb");
    }

    #[test]
    fn reasonable_strips_ansi() {
        assert_eq!(
            apply("\x1b[1mbold\x1b[0m", DecorativeLevel::Reasonable),
            "bold"
        );
    }

    #[test]
    fn high_drops_box_drawing_lines() {
        let out = apply("header\n──────────\nbody\n│ kept │", DecorativeLevel::High);
        assert_eq!(out, "header\nbody\n│ kept │");
    }

    #[test]
    fn high_preserves_ascii_rules() {
        let out = apply("title\n-----\n===\nbody", DecorativeLevel::High);
        assert_eq!(out, "title\n-----\n===\nbody");
    }

    #[test]
    fn none_is_identity() {
        let raw = "\x1b[32mok\x1b[0m\n\n\n──────\nbye   ";
        assert_eq!(apply(raw, DecorativeLevel::None), raw);
        assert_eq!(DecorativeLevel::parse("none"), Some(DecorativeLevel::None));
        assert_eq!(DecorativeLevel::parse("off"), Some(DecorativeLevel::None));
    }

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(apply("", DecorativeLevel::High), "");
    }

    #[test]
    fn parse_accepts_known_values() {
        assert_eq!(
            DecorativeLevel::parse("light"),
            Some(DecorativeLevel::Light)
        );
        assert_eq!(
            DecorativeLevel::parse("REASONABLE"),
            Some(DecorativeLevel::Reasonable)
        );
        assert_eq!(
            DecorativeLevel::parse(" High "),
            Some(DecorativeLevel::High)
        );
        assert_eq!(DecorativeLevel::parse("bogus"), None);
    }
}
