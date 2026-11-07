//! Text primitives with no domain knowledge, shared by the `rtk` CLI and
//! `rtk-engine`. Apache-2.0 so the CLI stays installable without the engine.

use regex::Regex;
use std::sync::LazyLock;

pub fn truncate(s: &str, max_len: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_len {
        s.to_string()
    } else if max_len < 3 {
        "...".to_string()
    } else {
        format!("{}...", s.chars().take(max_len - 3).collect::<String>())
    }
}

pub fn strip_ansi(text: &str) -> String {
    static ANSI_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").unwrap());

    ANSI_RE.replace_all(text, "").to_string()
}

static IMPORT_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(use |import |from |require\(|#include)").unwrap());
static FUNC_SIGNATURE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(pub\s+)?(async\s+)?(fn|def|function|func|class|struct|enum|trait|interface|type)\s+\w+",
    )
    .unwrap()
});

pub fn is_import_line(trimmed: &str) -> bool {
    IMPORT_PATTERN.is_match(trimmed)
}

pub fn is_func_signature(trimmed: &str) -> bool {
    FUNC_SIGNATURE.is_match(trimmed)
}

pub fn smart_truncate(content: &str, max_lines: usize) -> String {
    if max_lines == 0 {
        return String::new();
    }
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() <= max_lines {
        return content.to_string();
    }

    let mut result = Vec::with_capacity(max_lines + 1);
    let mut kept_lines = 0;

    for line in &lines {
        let trimmed = line.trim();

        let is_important = is_func_signature(trimmed)
            || is_import_line(trimmed)
            || trimmed.starts_with("pub ")
            || trimmed.starts_with("export ")
            || trimmed == "}"
            || trimmed == "{";

        if is_important || kept_lines < max_lines / 2 {
            result.push((*line).to_string());
            kept_lines += 1;
        }

        if kept_lines >= max_lines - 1 {
            break;
        }
    }

    result.push(format!("[{} more lines]", lines.len() - kept_lines));

    result.join("\n")
}

pub fn estimate_tokens(text: &str) -> usize {
    estimate_tokens_from_len(text.len())
}

pub fn estimate_tokens_from_len(len: usize) -> usize {
    (len as f64 / 4.0).ceil() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_and_exact_strings_pass_through() {
        assert_eq!(truncate("hi", 10), "hi");
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn truncate_cuts_on_characters_not_bytes() {
        assert_eq!(truncate("hello world", 8), "hello...");
        assert_eq!(truncate("привет мир", 7), "прив...");
    }

    #[test]
    fn strip_ansi_removes_color_codes() {
        assert_eq!(strip_ansi("\x1b[31mError\x1b[0m"), "Error");
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn smart_truncate_marker_count_is_exact() {
        let content = (0..100)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = smart_truncate(&content, 10);
        let last = out.lines().last().unwrap();
        let kept = out.lines().count() - 1;
        assert_eq!(last, format!("[{} more lines]", 100 - kept));
    }

    #[test]
    fn smart_truncate_short_content_is_untouched() {
        let content = "a\nb\nc";
        assert_eq!(smart_truncate(content, 10), content);
    }

    #[test]
    fn smart_truncate_zero_budget_shows_nothing() {
        assert_eq!(smart_truncate("a\nb\nc", 0), "");
    }

    #[test]
    fn estimates_round_up() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(estimate_tokens("hello world"), 3);
    }
}
