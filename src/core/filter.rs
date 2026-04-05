//! Strips comments and boilerplate from source code to save tokens.

use lazy_static::lazy_static;
use regex::Regex;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterLevel {
    None,
    Minimal,
    Smart,
    Aggressive,
}

impl FromStr for FilterLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "none" => Ok(FilterLevel::None),
            "minimal" => Ok(FilterLevel::Minimal),
            "smart" => Ok(FilterLevel::Smart),
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
            FilterLevel::Smart => write!(f, "smart"),
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

lazy_static! {
    static ref MULTIPLE_BLANK_LINES: Regex = Regex::new(r"\n{3,}").unwrap();
    static ref TRAILING_WHITESPACE: Regex = Regex::new(r"[ \t]+$").unwrap();
}

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

pub struct SmartFilter;

lazy_static! {
    static ref IMPORT_PATTERN: Regex =
        Regex::new(r"^(use |import |from |require\(|#include)").unwrap();
    static ref FUNC_SIGNATURE: Regex = Regex::new(
        r"^(pub\s+)?(async\s+)?(fn|def|function|func|class|struct|enum|trait|interface|type)\s+\w+"
    )
    .unwrap();
    static ref TEST_PATTERN: Regex = Regex::new(
        r"(#\[test\]|#\[cfg\(test\)\]|fn test_|def test_|it\(|test\(|describe\()"
    )
    .unwrap();
    static ref DOC_COMMENT_PATTERN: Regex =
        Regex::new(r"^(///|//!|/\*\*|#\s|##\s|\*\s|@\w+)").unwrap();
    static ref MULTI_BLANK_SMART: Regex = Regex::new(r"\n{3,}").unwrap();
}

impl FilterStrategy for SmartFilter {
    fn filter(&self, content: &str, lang: &Language) -> String {
        // Data formats: delegate to MinimalFilter (no smart collapsing)
        if *lang == Language::Data {
            return MinimalFilter.filter(content, lang);
        }

        // Start with minimal filtering (strip comments, normalize blanks)
        let minimal = MinimalFilter.filter(content, lang);
        let lines: Vec<&str> = minimal.lines().collect();
        let mut result: Vec<String> = Vec::with_capacity(lines.len());

        let mut i = 0;
        while i < lines.len() {
            let trimmed = lines[i].trim();

            // --- Collapse import blocks (>10 consecutive → keep first 5 + last 2) ---
            if IMPORT_PATTERN.is_match(trimmed) {
                let import_start = i;
                while i < lines.len() {
                    let t = lines[i].trim();
                    if t.is_empty() || IMPORT_PATTERN.is_match(t) {
                        i += 1;
                    } else {
                        break;
                    }
                }
                let import_end = i;
                let import_lines: Vec<&str> = lines[import_start..import_end]
                    .iter()
                    .filter(|l| !l.trim().is_empty())
                    .copied()
                    .collect();

                if import_lines.len() > 10 {
                    for line in import_lines.iter().take(5) {
                        result.push(line.to_string());
                    }
                    result.push(format!(
                        "// ... +{} more imports",
                        import_lines.len() - 7
                    ));
                    for line in import_lines.iter().skip(import_lines.len() - 2) {
                        result.push(line.to_string());
                    }
                } else {
                    for line in &import_lines {
                        result.push(line.to_string());
                    }
                }
                continue;
            }

            // --- Collapse test blocks ---
            if TEST_PATTERN.is_match(trimmed) {
                let test_start = i;
                let mut test_count = 0;
                let mut test_blocks: Vec<(usize, usize)> = Vec::new();

                // Scan ahead to collect consecutive test functions
                while i < lines.len() {
                    let t = lines[i].trim();

                    // Is this a test marker or test function start?
                    if TEST_PATTERN.is_match(t) {
                        test_count += 1;
                        let block_start = i;
                        // Find the end of this test block (track braces/indentation)
                        let mut brace_depth = 0i32;
                        let mut found_brace = false;
                        i += 1;
                        while i < lines.len() {
                            let bt = lines[i].trim();
                            brace_depth += bt.matches('{').count() as i32;
                            brace_depth -= bt.matches('}').count() as i32;
                            if bt.contains('{') {
                                found_brace = true;
                            }
                            i += 1;
                            if found_brace && brace_depth <= 0 {
                                break;
                            }
                            // Python/JS: no braces, use indentation
                            if !found_brace
                                && i < lines.len()
                                && !lines[i].trim().is_empty()
                                && !lines[i].starts_with(' ')
                                && !lines[i].starts_with('\t')
                            {
                                break;
                            }
                        }
                        test_blocks.push((block_start, i));
                    } else if t.is_empty() {
                        i += 1;
                    } else {
                        break;
                    }
                }

                if test_count > 2 {
                    // Keep first 2 test blocks, summarize the rest
                    for &(start, end) in test_blocks.iter().take(2) {
                        for line in &lines[start..end] {
                            result.push(line.to_string());
                        }
                    }
                    result.push(format!(
                        "// ... +{} more tests",
                        test_count - 2
                    ));
                } else {
                    // ≤2 tests: keep everything
                    for line in &lines[test_start..i] {
                        result.push(line.to_string());
                    }
                }
                continue;
            }

            // --- Collapse doc-comment blocks (>15 consecutive lines) ---
            if DOC_COMMENT_PATTERN.is_match(trimmed) {
                let doc_start = i;
                while i < lines.len() {
                    let t = lines[i].trim();
                    if t.is_empty()
                        || DOC_COMMENT_PATTERN.is_match(t)
                        || t == "*/"
                        || t == "*"
                    {
                        i += 1;
                    } else {
                        break;
                    }
                }
                let doc_lines: Vec<&str> = lines[doc_start..i]
                    .iter()
                    .filter(|l| !l.trim().is_empty())
                    .copied()
                    .collect();

                if doc_lines.len() > 15 {
                    for line in doc_lines.iter().take(3) {
                        result.push(line.to_string());
                    }
                    result.push(format!(
                        "// ... +{} more doc lines",
                        doc_lines.len() - 3
                    ));
                } else {
                    for line in &doc_lines {
                        result.push(line.to_string());
                    }
                }
                continue;
            }

            // --- Truncate long string literals (>200 chars) ---
            if trimmed.len() > 200 {
                // Check if this looks like a string/array literal line
                let has_long_string = trimmed.contains('"') && trimmed.len() > 200;
                if has_long_string {
                    let truncated: String = trimmed.chars().take(120).collect();
                    let indent = lines[i].len() - lines[i].trim_start().len();
                    let prefix = &lines[i][..indent];
                    result.push(format!(
                        "{}{}... (line truncated, {} chars total)",
                        prefix,
                        truncated,
                        trimmed.len()
                    ));
                    i += 1;
                    continue;
                }
            }

            result.push(lines[i].to_string());
            i += 1;
        }

        // Normalize consecutive blank lines (3+ → 2)
        let joined = result.join("\n").trim().to_string();
        MULTI_BLANK_SMART.replace_all(&joined, "\n\n").to_string()
    }
}

pub struct AggressiveFilter;

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
        FilterLevel::Smart => Box::new(SmartFilter),
        FilterLevel::Aggressive => Box::new(AggressiveFilter),
    }
}

pub fn smart_truncate(content: &str, max_lines: usize, _lang: &Language) -> String {
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() <= max_lines {
        return content.to_string();
    }

    let mut result = Vec::with_capacity(max_lines);
    let mut kept_lines = 0;
    let mut skipped_section = false;

    for line in &lines {
        let trimmed = line.trim();

        // Always keep signatures and important structural elements
        let is_important = FUNC_SIGNATURE.is_match(trimmed)
            || IMPORT_PATTERN.is_match(trimmed)
            || trimmed.starts_with("pub ")
            || trimmed.starts_with("export ")
            || trimmed == "}"
            || trimmed == "{";

        if is_important || kept_lines < max_lines / 2 {
            if skipped_section {
                result.push(format!(
                    "    // ... {} lines omitted",
                    lines.len() - kept_lines
                ));
                skipped_section = false;
            }
            result.push((*line).to_string());
            kept_lines += 1;
        } else {
            skipped_section = true;
        }

        if kept_lines >= max_lines - 1 {
            break;
        }
    }

    if skipped_section || kept_lines < lines.len() {
        result.push(format!(
            "// ... {} more lines (total: {})",
            lines.len() - kept_lines,
            lines.len()
        ));
    }

    result.join("\n")
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
            FilterLevel::from_str("smart").unwrap(),
            FilterLevel::Smart
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

    // --- SmartFilter tests ---

    #[test]
    fn test_smart_filter_collapses_imports() {
        let mut code = String::new();
        for i in 0..15 {
            code.push_str(&format!("use crate::module{};\n", i));
        }
        code.push_str("\nfn main() {}\n");

        let filter = SmartFilter;
        let result = filter.filter(&code, &Language::Rust);
        // Should have collapsed 15 imports to 5 + summary + 2
        assert!(
            result.contains("more imports"),
            "Expected import collapse summary, got:\n{}",
            result
        );
        // First 5 should be present
        assert!(result.contains("use crate::module0;"));
        assert!(result.contains("use crate::module4;"));
        // Last 2 should be present
        assert!(result.contains("use crate::module13;"));
        assert!(result.contains("use crate::module14;"));
        // Middle ones should NOT be present
        assert!(!result.contains("use crate::module6;"));
    }

    #[test]
    fn test_smart_filter_keeps_small_import_blocks() {
        let code = "use std::fmt;\nuse std::io;\nuse std::fs;\n\nfn main() {}\n";
        let filter = SmartFilter;
        let result = filter.filter(code, &Language::Rust);
        // Only 3 imports — should keep all
        assert!(!result.contains("more imports"));
        assert!(result.contains("use std::fmt;"));
        assert!(result.contains("use std::io;"));
        assert!(result.contains("use std::fs;"));
    }

    #[test]
    fn test_smart_filter_collapses_tests() {
        let code = r#"
fn main() {}

#[test]
fn test_one() {
    assert!(true);
}

#[test]
fn test_two() {
    assert!(true);
}

#[test]
fn test_three() {
    assert!(true);
}

#[test]
fn test_four() {
    assert!(true);
}
"#;
        let filter = SmartFilter;
        let result = filter.filter(code, &Language::Rust);
        // Should keep first 2 tests, collapse the rest
        assert!(
            result.contains("more tests"),
            "Expected test collapse summary, got:\n{}",
            result
        );
        assert!(result.contains("fn test_one()"));
        assert!(result.contains("fn test_two()"));
        // test_three and test_four should be collapsed
        assert!(!result.contains("fn test_three()"));
    }

    #[test]
    fn test_smart_filter_truncates_long_strings() {
        let long = format!(
            "    let s = \"{}\";",
            "x".repeat(250)
        );
        let code = format!("fn main() {{\n{}\n}}\n", long);
        let filter = SmartFilter;
        let result = filter.filter(&code, &Language::Rust);
        assert!(
            result.contains("line truncated"),
            "Expected long string truncation, got:\n{}",
            result
        );
    }

    #[test]
    fn test_smart_filter_data_delegates_to_minimal() {
        let json = r#"{"key": "value"}"#;
        let smart = SmartFilter.filter(json, &Language::Data);
        let minimal = MinimalFilter.filter(json, &Language::Data);
        assert_eq!(smart, minimal);
    }

    #[test]
    fn test_smart_filter_collapses_long_doc_comments() {
        let mut code = String::from("fn main() {}\n\n");
        // 20 doc-comment lines
        for i in 0..20 {
            code.push_str(&format!("/// Doc line {}\n", i));
        }
        code.push_str("fn documented() {}\n");

        let filter = SmartFilter;
        let result = filter.filter(&code, &Language::Rust);
        assert!(
            result.contains("more doc lines"),
            "Expected doc-comment collapse, got:\n{}",
            result
        );
        // First 3 should be kept
        assert!(result.contains("/// Doc line 0"));
        assert!(result.contains("/// Doc line 2"));
        // Later ones should be collapsed
        assert!(!result.contains("/// Doc line 10"));
    }

    #[test]
    fn test_smart_filter_keeps_short_doc_comments() {
        let code = "/// Short doc\n/// Second line\nfn foo() {}\n";
        let filter = SmartFilter;
        let result = filter.filter(code, &Language::Rust);
        assert!(!result.contains("more doc lines"));
        assert!(result.contains("/// Short doc"));
        assert!(result.contains("/// Second line"));
    }

    #[test]
    fn test_smart_filter_normalizes_blank_lines() {
        // After stripping comments, we might end up with 3+ blank lines
        let code = "fn a() {}\n\n\n\n\n\nfn b() {}\n";
        let filter = SmartFilter;
        let result = filter.filter(code, &Language::Rust);
        // Should not have 3+ consecutive newlines
        assert!(
            !result.contains("\n\n\n"),
            "Should collapse 3+ blanks, got:\n{:?}",
            result
        );
        assert!(result.contains("fn a()"));
        assert!(result.contains("fn b()"));
    }

    // --- truncation accuracy ---

    #[test]
    fn test_smart_truncate_overflow_count_exact() {
        // 200 plain-text lines with max_lines=20.
        // smart_truncate keeps the first max_lines/2=10 lines, then skips the rest.
        // The overflow message "// ... N more lines (total: T)" must satisfy:
        //   kept_count + N == T
        let total_lines = 200usize;
        let max_lines = 20usize;
        let content: String = (0..total_lines)
            .map(|i| format!("plain text line number {}", i))
            .collect::<Vec<_>>()
            .join("\n");

        let output = smart_truncate(&content, max_lines, &Language::Rust);

        // Extract the overflow message
        let overflow_line = output
            .lines()
            .find(|l| l.contains("more lines"))
            .unwrap_or_else(|| panic!("No overflow message found in:\n{}", output));

        // Parse "// ... N more lines (total: T)"
        let reported_more: usize = overflow_line
            .split_whitespace()
            .find(|w| w.parse::<usize>().is_ok())
            .and_then(|w| w.parse().ok())
            .unwrap_or_else(|| panic!("Could not parse overflow count from: {}", overflow_line));

        let kept_count = output
            .lines()
            .filter(|l| !l.contains("more lines") && !l.contains("omitted"))
            .count();

        assert_eq!(
            kept_count + reported_more,
            total_lines,
            "kept ({}) + reported_more ({}) must equal total ({})",
            kept_count,
            reported_more,
            total_lines
        );
    }
}
