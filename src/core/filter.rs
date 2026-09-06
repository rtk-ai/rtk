//! Strips comments and boilerplate from source code to save tokens.

use regex::Regex;
use std::str::FromStr;
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterLevel {
    None,
    Minimal,
    Aggressive,
}

impl FromStr for FilterLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "none" => Ok(FilterLevel::None),
            "minimal" => Ok(FilterLevel::Minimal),
            "aggressive" => Ok(FilterLevel::Aggressive),
            _ => Err(format!("Unknown filter level: {}", s)),
        }
    }
}

impl std::fmt::Display for FilterLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FilterLevel::None => write!(f, "none"),
            FilterLevel::Minimal => write!(f, "minimal"),
            FilterLevel::Aggressive => write!(f, "aggressive"),
        }
    }
}

pub trait FilterStrategy {
    fn filter(&self, content: &str, lang: &Language) -> String;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Go,
    C,
    Cpp,
    Java,
    Ruby,
    Shell,
    /// Data formats (JSON, YAML, TOML, XML, CSV) — no comment stripping
    Data,
    Unknown,
}

impl Language {
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "rs" => Language::Rust,
            "py" | "pyw" => Language::Python,
            "js" | "mjs" | "cjs" => Language::JavaScript,
            "ts" | "tsx" => Language::TypeScript,
            "go" => Language::Go,
            "c" | "h" => Language::C,
            "cpp" | "cc" | "cxx" | "hpp" | "hh" => Language::Cpp,
            "java" => Language::Java,
            "rb" => Language::Ruby,
            "sh" | "bash" | "zsh" => Language::Shell,
            "json" | "jsonc" | "json5" | "yaml" | "yml" | "toml" | "xml" | "csv" | "tsv"
            | "graphql" | "gql" | "sql" | "md" | "markdown" | "txt" | "env" | "lock" => {
                Language::Data
            }
            _ => Language::Unknown,
        }
    }

    pub fn comment_patterns(&self) -> CommentPatterns {
        match self {
            Language::Rust => CommentPatterns {
                line: Some("//"),
                block_start: Some("/*"),
                block_end: Some("*/"),
                doc_line: Some("///"),
                doc_block_start: Some("/**"),
            },
            Language::Python => CommentPatterns {
                line: Some("#"),
                block_start: Some("\"\"\""),
                block_end: Some("\"\"\""),
                doc_line: None,
                doc_block_start: Some("\"\"\""),
            },
            Language::JavaScript
            | Language::TypeScript
            | Language::Go
            | Language::C
            | Language::Cpp
            | Language::Java => CommentPatterns {
                line: Some("//"),
                block_start: Some("/*"),
                block_end: Some("*/"),
                doc_line: None,
                doc_block_start: Some("/**"),
            },
            Language::Ruby => CommentPatterns {
                line: Some("#"),
                block_start: Some("=begin"),
                block_end: Some("=end"),
                doc_line: None,
                doc_block_start: None,
            },
            Language::Shell => CommentPatterns {
                line: Some("#"),
                block_start: None,
                block_end: None,
                doc_line: None,
                doc_block_start: None,
            },
            Language::Data => CommentPatterns {
                line: None,
                block_start: None,
                block_end: None,
                doc_line: None,
                doc_block_start: None,
            },
            Language::Unknown => CommentPatterns {
                line: Some("//"),
                block_start: Some("/*"),
                block_end: Some("*/"),
                doc_line: None,
                doc_block_start: None,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct CommentPatterns {
    pub line: Option<&'static str>,
    pub block_start: Option<&'static str>,
    pub block_end: Option<&'static str>,
    pub doc_line: Option<&'static str>,
    pub doc_block_start: Option<&'static str>,
}

pub struct NoFilter;

impl FilterStrategy for NoFilter {
    fn filter(&self, content: &str, _lang: &Language) -> String {
        content.to_string()
    }
}

pub struct MinimalFilter;

static MULTIPLE_BLANK_LINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());

impl FilterStrategy for MinimalFilter {
    fn filter(&self, content: &str, lang: &Language) -> String {
        let patterns = lang.comment_patterns();
        let mut result = String::with_capacity(content.len());
        let mut in_block_comment = false;
        let mut in_docstring = false;

        for line in content.lines() {
            let trimmed = line.trim();

            // Handle block comments
            if let (Some(start), Some(end)) = (patterns.block_start, patterns.block_end) {
                if !in_docstring
                    && trimmed.contains(start)
                    && !trimmed.starts_with(patterns.doc_block_start.unwrap_or("###"))
                {
                    in_block_comment = true;
                }
                if in_block_comment {
                    if trimmed.contains(end) {
                        in_block_comment = false;
                    }
                    continue;
                }
            }

            // Handle Python docstrings (keep them in minimal mode)
            if *lang == Language::Python && trimmed.starts_with("\"\"\"") {
                in_docstring = !in_docstring;
                result.push_str(line);
                result.push('\n');
                continue;
            }

            if in_docstring {
                result.push_str(line);
                result.push('\n');
                continue;
            }

            // Skip single-line comments (but keep doc comments)
            if let Some(line_comment) = patterns.line {
                if trimmed.starts_with(line_comment) {
                    // Keep doc comments
                    if let Some(doc) = patterns.doc_line {
                        if trimmed.starts_with(doc) {
                            result.push_str(line);
                            result.push('\n');
                        }
                    }
                    continue;
                }
            }

            // Skip empty lines at this point, we'll normalize later
            if trimmed.is_empty() {
                result.push('\n');
                continue;
            }

            result.push_str(line);
            result.push('\n');
        }

        // Normalize multiple blank lines to max 2
        let result = MULTIPLE_BLANK_LINES.replace_all(&result, "\n\n");
        result.trim().to_string()
    }
}

pub struct AggressiveFilter;

static IMPORT_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(use |import |from |require\(|#include)").unwrap());
static FUNC_SIGNATURE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(pub\s+)?(async\s+)?(fn|def|function|func|class|struct|enum|trait|interface|type)\s+\w+",
    )
    .unwrap()
});

impl FilterStrategy for AggressiveFilter {
    fn filter(&self, content: &str, lang: &Language) -> String {
        // Data formats (JSON, YAML, etc.) must never be code-filtered
        if *lang == Language::Data {
            return MinimalFilter.filter(content, lang);
        }

        let minimal = MinimalFilter.filter(content, lang);
        let mut result = String::with_capacity(minimal.len() / 2);
        let mut brace_depth = 0;
        let mut in_impl_body = false;

        for line in minimal.lines() {
            let trimmed = line.trim();

            // Always keep imports
            if IMPORT_PATTERN.is_match(trimmed) {
                result.push_str(line);
                result.push('\n');
                continue;
            }

            // Always keep function/struct/class signatures
            if FUNC_SIGNATURE.is_match(trimmed) {
                result.push_str(line);
                result.push('\n');
                in_impl_body = true;
                brace_depth = 0;
                continue;
            }

            // Track brace depth for implementation bodies
            let open_braces = trimmed.matches('{').count();
            let close_braces = trimmed.matches('}').count();

            if in_impl_body {
                brace_depth += open_braces as i32;
                brace_depth -= close_braces as i32;

                // Only keep the opening and closing braces
                if brace_depth <= 1 && (trimmed == "{" || trimmed == "}" || trimmed.ends_with('{'))
                {
                    result.push_str(line);
                    result.push('\n');
                }

                if brace_depth <= 0 {
                    in_impl_body = false;
                    if !trimmed.is_empty() && trimmed != "}" {
                        result.push_str("    // ... implementation\n");
                    }
                }
                continue;
            }

            // Keep type definitions, constants, etc.
            if trimmed.starts_with("const ")
                || trimmed.starts_with("static ")
                || trimmed.starts_with("let ")
                || trimmed.starts_with("pub const ")
                || trimmed.starts_with("pub static ")
            {
                result.push_str(line);
                result.push('\n');
            }
        }

        result.trim().to_string()
    }
}

pub fn get_filter(level: FilterLevel) -> Box<dyn FilterStrategy> {
    match level {
        FilterLevel::None => Box::new(NoFilter),
        FilterLevel::Minimal => Box::new(MinimalFilter),
        FilterLevel::Aggressive => Box::new(AggressiveFilter),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum TruncatedLine<'a> {
    Source { number: usize, text: &'a str },
    Omitted { count: usize },
}

fn is_structurally_important(line: &str) -> bool {
    let trimmed = line.trim();
    FUNC_SIGNATURE.is_match(trimmed)
        || IMPORT_PATTERN.is_match(trimmed)
        || trimmed.starts_with("pub ")
        || trimmed.starts_with("export ")
        || trimmed == "}"
        || trimmed == "{"
}

fn select_truncated_lines<'a>(lines: &'a [&'a str], max_lines: usize) -> Vec<TruncatedLine<'a>> {
    if max_lines == 0 || lines.is_empty() {
        return Vec::new();
    }

    let prefix_len = (max_lines / 2)
        .min(max_lines.saturating_sub(1))
        .min(lines.len());
    let mut selected: Vec<usize> = (0..prefix_len).collect();
    let mut gap_count = 0;

    for (index, line) in lines.iter().enumerate().skip(prefix_len) {
        if !is_structurally_important(line) {
            continue;
        }

        let adds_gap = selected
            .last()
            .map_or(index > 0, |previous| index > previous + 1);
        let needs_tail_marker = index + 1 < lines.len();
        let rendered_len =
            selected.len() + 1 + gap_count + usize::from(adds_gap) + usize::from(needs_tail_marker);

        if rendered_len <= max_lines {
            selected.push(index);
            gap_count += usize::from(adds_gap);
        }
    }

    if selected.is_empty() {
        return vec![TruncatedLine::Omitted { count: lines.len() }];
    }

    let mut result = Vec::with_capacity(max_lines);
    let mut next_source_index = 0;
    for index in selected {
        if index > next_source_index {
            result.push(TruncatedLine::Omitted {
                count: index - next_source_index,
            });
        }
        result.push(TruncatedLine::Source {
            number: index + 1,
            text: lines[index],
        });
        next_source_index = index + 1;
    }
    if next_source_index < lines.len() {
        result.push(TruncatedLine::Omitted {
            count: lines.len() - next_source_index,
        });
    }
    result
}

fn omission_marker(count: usize) -> String {
    let noun = if count == 1 { "line" } else { "lines" };
    format!("[{count} {noun} omitted]")
}

fn render_truncated(lines: &[TruncatedLine<'_>]) -> String {
    lines
        .iter()
        .map(|line| match line {
            TruncatedLine::Source { text, .. } => (*text).to_string(),
            TruncatedLine::Omitted { count } => omission_marker(*count),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn smart_truncate(content: &str, max_lines: usize, _lang: &Language) -> String {
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() <= max_lines {
        return content.to_string();
    }
    render_truncated(&select_truncated_lines(&lines, max_lines))
}

pub(crate) fn smart_truncate_numbered(content: &str, max_lines: usize, _lang: &Language) -> String {
    let source_lines: Vec<&str> = content.lines().collect();
    let width = source_lines.len().to_string().len();
    let lines = if source_lines.len() <= max_lines {
        source_lines
            .iter()
            .enumerate()
            .map(|(index, text)| TruncatedLine::Source {
                number: index + 1,
                text,
            })
            .collect()
    } else {
        select_truncated_lines(&source_lines, max_lines)
    };

    let mut output = String::new();
    for line in lines {
        match line {
            TruncatedLine::Source { number, text } => {
                output.push_str(&format!("{number:>width$} │ {text}\n"));
            }
            TruncatedLine::Omitted { count } => {
                output.push_str(&format!("{:width$} │ {}\n", "", omission_marker(count)));
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_level_parsing() {
        assert_eq!(FilterLevel::from_str("none").unwrap(), FilterLevel::None);
        assert_eq!(
            FilterLevel::from_str("minimal").unwrap(),
            FilterLevel::Minimal
        );
        assert_eq!(
            FilterLevel::from_str("aggressive").unwrap(),
            FilterLevel::Aggressive
        );
    }

    #[test]
    fn test_language_detection() {
        assert_eq!(Language::from_extension("rs"), Language::Rust);
        assert_eq!(Language::from_extension("py"), Language::Python);
        assert_eq!(Language::from_extension("js"), Language::JavaScript);
    }

    #[test]
    fn test_language_detection_data_formats() {
        assert_eq!(Language::from_extension("json"), Language::Data);
        assert_eq!(Language::from_extension("yaml"), Language::Data);
        assert_eq!(Language::from_extension("yml"), Language::Data);
        assert_eq!(Language::from_extension("toml"), Language::Data);
        assert_eq!(Language::from_extension("xml"), Language::Data);
        assert_eq!(Language::from_extension("csv"), Language::Data);
        assert_eq!(Language::from_extension("md"), Language::Data);
        assert_eq!(Language::from_extension("lock"), Language::Data);
    }

    #[test]
    fn test_json_no_comment_stripping() {
        // Reproduces #464: package.json with "packages/*" was corrupted
        // because /* was treated as block comment start
        let json = r#"{
  "workspaces": {
    "packages": [
      "packages/*"
    ]
  },
  "scripts": {
    "build": "bun run --workspaces build"
  },
  "lint-staged": {
    "**/package.json": [
      "sort-package-json"
    ]
  }
}"#;
        let filter = MinimalFilter;
        let result = filter.filter(json, &Language::Data);
        // All fields must be preserved — no comment stripping on JSON
        assert!(
            result.contains("packages/*"),
            "packages/* should not be treated as block comment start"
        );
        assert!(
            result.contains("scripts"),
            "scripts section must not be stripped"
        );
        assert!(
            result.contains("lint-staged"),
            "lint-staged section must not be stripped"
        );
        assert!(
            result.contains("**/package.json"),
            "**/package.json should not be treated as block comment end"
        );
    }

    #[test]
    fn test_json_aggressive_filter_preserves_structure() {
        let json = r#"{
  "name": "my-app",
  "dependencies": {
    "react": "^18.0.0"
  },
  "scripts": {
    "dev": "next dev /* not a comment */"
  }
}"#;
        let filter = AggressiveFilter;
        let result = filter.filter(json, &Language::Data);
        assert!(
            result.contains("/* not a comment */"),
            "Aggressive filter must not strip comment-like patterns in JSON"
        );
    }

    #[test]
    fn test_minimal_filter_removes_comments() {
        let code = r#"
// This is a comment
fn main() {
    println!("Hello");
}
"#;
        let filter = MinimalFilter;
        let result = filter.filter(code, &Language::Rust);
        assert!(!result.contains("// This is a comment"));
        assert!(result.contains("fn main()"));
    }

    // --- truncation accuracy ---

    #[test]
    fn test_smart_truncate_overflow_count_exact() {
        // 200 plain-text lines (no function signatures/imports) with max_lines=20.
        // Smart selection keeps up to max_lines/2=10 non-important lines then stops.
        // Omission markers must account for every source line not shown.
        let total_lines = 200usize;
        let max_lines = 20usize;
        let content: String = (0..total_lines)
            .map(|i| format!("plain text line number {}", i))
            .collect::<Vec<_>>()
            .join("\n");

        let output = smart_truncate(&content, max_lines, &Language::Rust);

        let reported_omitted: usize = output
            .lines()
            .filter(|line| line.contains("omitted"))
            .map(|line| {
                line.trim()
                    .strip_prefix('[')
                    .and_then(|value| value.split_whitespace().next())
                    .and_then(|count| count.parse::<usize>().ok())
                    .unwrap_or_else(|| panic!("Could not parse omission count from: {line}"))
            })
            .sum();

        let kept_count = output
            .lines()
            .filter(|line| !line.contains("omitted"))
            .count();

        assert_eq!(
            kept_count + reported_omitted,
            total_lines,
            "kept ({}) + reported_omitted ({}) must equal total ({})",
            kept_count,
            reported_omitted,
            total_lines
        );
    }

    #[test]
    fn test_smart_truncate_no_annotations() {
        // 10 plain-text lines, max_lines=3: smart logic keeps first max_lines/2=1 line.
        // (None of the lines match FUNC_SIGNATURE or IMPORT_PATTERN patterns.)
        let input = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n";
        let output = smart_truncate(input, 3, &Language::Unknown);
        // Must NOT contain old-style "// ... N lines omitted" annotations
        assert!(
            !output.contains("// ..."),
            "smart_truncate must not insert synthetic comment annotations"
        );
        // Must contain a non-code omission marker (1 kept + 9 omitted = 10 total)
        assert!(output.contains("[9 lines omitted]"));
        // Only the first line is kept (plain-text, no important signatures)
        assert!(output.starts_with("line1\n"));
    }

    #[test]
    fn test_smart_truncate_no_truncation_when_under_limit() {
        let input = "a\nb\nc\n";
        let output = smart_truncate(input, 10, &Language::Unknown);
        assert_eq!(output, input);
        assert!(!output.contains("more lines"));
    }

    #[test]
    fn test_smart_truncate_exact_limit() {
        let input = "a\nb\nc";
        let output = smart_truncate(input, 3, &Language::Unknown);
        assert_eq!(output, input);
    }

    #[test]
    fn test_smart_truncate_marks_each_discontinuity() {
        let input = r#"fn can_admin(user: &User) -> bool {
    if !user.is_admin {
        return false;
    }
    if user.is_banned {
        return false;
    }
    if user.mfa_ok {
        return true;
    }
    false
}
fn wipe_database() {
    drop_all();
}"#;

        let output = smart_truncate(input, 10, &Language::Rust);

        assert!(output.contains("    if user.is_banned {\n[1 line omitted]\n    }"));
        assert!(
            !output.contains("    if user.is_banned {\n    }"),
            "a removed guard body must never look like an empty source block"
        );
    }

    #[test]
    fn test_smart_truncate_zero_lines_is_empty() {
        assert_eq!(smart_truncate("first\nsecond\n", 0, &Language::Unknown), "");
    }

    #[test]
    fn test_smart_truncate_never_exceeds_line_budget() {
        let input = "plain one\nplain two\nplain three\nfn final_item() {}";
        for max_lines in 0..=3 {
            let output = smart_truncate(input, max_lines, &Language::Rust);
            assert!(
                output.lines().count() <= max_lines,
                "max_lines={max_lines} produced {output:?}"
            );
        }
    }
}
