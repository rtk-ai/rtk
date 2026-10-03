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
            Language::Data | Language::Unknown => CommentPatterns {
                line: None,
                block_start: None,
                block_end: None,
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

/// Advances triple-quoted string state across one line, returning the delimiter
/// still open at end of line. The two quote kinds are tracked separately so a
/// `'''` inside a `"""` string is text rather than a close. Outside a string, a
/// one-line string literal is skipped whole and a `#` ends the scan, so a `"""`
/// written inside `'"""'` or after a trailing comment opens nothing. A backslash
/// escapes the next byte in every kind of string, raw ones included.
fn advance_triple_quote(line: &str, open: Option<&'static str>) -> Option<&'static str> {
    let bytes = line.as_bytes();
    let mut state = open;
    let mut i = 0;

    while i < bytes.len() {
        let rest = &bytes[i..];
        if let Some(current) = state {
            if rest[0] == b'\\' {
                i += 2;
            } else if rest.starts_with(current.as_bytes()) {
                state = None;
                i += 3;
            } else {
                i += 1;
            }
            continue;
        }

        match rest[0] {
            b'#' => break,
            b'"' if rest.starts_with(b"\"\"\"") => {
                state = Some("\"\"\"");
                i += 3;
            }
            b'\'' if rest.starts_with(b"'''") => {
                state = Some("'''");
                i += 3;
            }
            quote @ (b'"' | b'\'') => {
                let interpolated = has_interpolation_prefix(&bytes[..i]);
                let mut depth = 0usize;
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i += 1,
                        b'{' if interpolated => {
                            if depth == 0 && bytes.get(i + 1) == Some(&b'{') {
                                i += 1;
                            } else {
                                depth += 1;
                            }
                        }
                        b'}' if interpolated && depth > 0 => depth -= 1,
                        b if b == quote && depth == 0 => break,
                        _ => {}
                    }
                    i += 1;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    state
}

/// True when the identifier ending at `before` is an f-string or t-string prefix.
/// A replacement field in those may reuse the outer quote (PEP 701), so the
/// quote only ends the string outside `{...}`.
fn has_interpolation_prefix(before: &[u8]) -> bool {
    let start = before
        .iter()
        .rposition(|b| !(b.is_ascii_alphanumeric() || *b == b'_'))
        .map_or(0, |p| p + 1);
    matches!(
        before[start..].to_ascii_lowercase().as_slice(),
        b"f" | b"fr" | b"rf" | b"t" | b"tr" | b"rt"
    )
}

/// Python has no block comments. `"""` opens a *string*, which may be a
/// docstring or an ordinary value, so it cannot be matched with the
/// line-oriented block-comment rules the other languages use: a line such as
/// `QUERY = """` both contains and "closes" the delimiter, and a single-line
/// docstring toggles the state once and never back.
///
/// Minimal keeps docstrings, so the only thing to remove here is `#` comments,
/// and the only state needed is whether we are inside a triple-quoted string.
fn filter_python_minimal(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let mut open_string: Option<&'static str> = None;

    for line in content.lines() {
        let trimmed = line.trim();

        // Inside a string every line is literal text, including one that starts
        // with `#`.
        if open_string.is_some() {
            result.push_str(line);
            result.push('\n');
            open_string = advance_triple_quote(line, open_string);
            continue;
        }

        // A comment's contents are not code, so any delimiter in it is not real.
        if trimmed.starts_with('#') {
            continue;
        }

        if trimmed.is_empty() {
            result.push('\n');
            continue;
        }

        result.push_str(line);
        result.push('\n');
        open_string = advance_triple_quote(line, None);
    }

    let result = MULTIPLE_BLANK_LINES.replace_all(&result, "\n\n");
    result.trim().to_string()
}

impl FilterStrategy for MinimalFilter {
    fn filter(&self, content: &str, lang: &Language) -> String {
        if *lang == Language::Python {
            return filter_python_minimal(content);
        }

        let patterns = lang.comment_patterns();
        let mut result = String::with_capacity(content.len());
        let mut in_block_comment = false;
        let mut in_docstring = false;

        for line in content.lines() {
            let trimmed = line.trim();

            // Handle block comments
            if let (Some(start), Some(end)) = (patterns.block_start, patterns.block_end) {
                // starts_with, not contains: `/*` inside a string literal or
                // glob (e.g. "src/*.rs") must not open a comment block (#2385)
                if !in_docstring
                    && trimmed.starts_with(start)
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
            if let Some(line_comment) = patterns.line
                && trimmed.starts_with(line_comment)
            {
                // Keep doc comments
                if let Some(doc) = patterns.doc_line
                    && trimmed.starts_with(doc)
                {
                    result.push_str(line);
                    result.push('\n');
                }
                continue;
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

pub fn smart_truncate(content: &str, max_lines: usize, _lang: &Language) -> String {
    // A zero budget shows nothing, matching `--tail-lines 0`/`--head-lines 0`.
    // Returning early also keeps `max_lines - 1` below from underflowing.
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

        // Prioritize structurally important lines so the visible window stays useful.
        // The old approach interleaved "// ... N lines omitted" markers which AI agents
        // treated as code, causing parsing confusion and extra retry loops.
        let is_important = FUNC_SIGNATURE.is_match(trimmed)
            || IMPORT_PATTERN.is_match(trimmed)
            || trimmed.starts_with("pub ")
            || trimmed.starts_with("export ")
            || trimmed == "}"
            || trimmed == "{";

        if is_important || kept_lines < max_lines / 2 {
            result.push((*line).to_string());
            kept_lines += 1;
        }
        // Non-important lines beyond max_lines/2 are silently skipped —
        // no inline markers that could be mistaken for file content.

        if kept_lines + 1 >= max_lines {
            break;
        }
    }

    // Single end-of-output marker: not code syntax, unambiguous to AI agents.
    // Invariant: kept_lines + N == lines.len() (N = lines not shown)
    result.push(format!("[{} more lines]", lines.len() - kept_lines));

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
    fn test_unknown_language_no_comment_stripping() {
        // Reproduces #4249: extensionless files like .gitignore with "/*.log"
        // were corrupted because /* was treated as C block comment start
        let gitignore = "node_modules\n/*.log\n!/src\ndist/\n";
        let filter = MinimalFilter;
        let result = filter.filter(gitignore, &Language::Unknown);
        assert_eq!(result, "node_modules\n/*.log\n!/src\ndist/");
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

    // --- Python triple-quoted strings in minimal mode ---

    #[test]
    fn test_minimal_python_keeps_multiline_string_assignment() {
        let code = r#"QUERY = """
SELECT id
FROM users
"""
import os
"#;
        let result = MinimalFilter.filter(code, &Language::Python);
        assert!(
            result.contains(r#"QUERY = """#),
            "the line opening the string was dropped, leaving its body as loose text:\n{}",
            result
        );
        assert!(
            result.contains("SELECT id"),
            "string body lost:\n{}",
            result
        );
        assert!(
            result.contains("import os"),
            "code after string lost:\n{}",
            result
        );
    }

    #[test]
    fn test_minimal_python_strips_comments_after_oneline_docstring() {
        let code = r#""""Module doc."""
import os
# strip me
def go():
    return 1
"#;
        let result = MinimalFilter.filter(code, &Language::Python);
        assert!(
            !result.contains("# strip me"),
            "a one-line docstring left the filter stuck in docstring mode, so no \
             later comment was stripped:\n{}",
            result
        );
        assert!(
            result.contains(r#""""Module doc.""""#),
            "docstring lost:\n{}",
            result
        );
        assert!(result.contains("def go():"), "code lost:\n{}", result);
    }

    #[test]
    fn test_minimal_python_keeps_hash_inside_docstring() {
        let code = r#""""
# not a comment, it is prose
"""
x = 1
"#;
        let result = MinimalFilter.filter(code, &Language::Python);
        assert!(
            result.contains("# not a comment, it is prose"),
            "text inside a docstring was stripped as a comment:\n{}",
            result
        );
        assert!(result.contains("x = 1"));
    }

    #[test]
    fn test_minimal_python_single_quoted_docstring_does_not_close_double() {
        let code = r#""""Doc mentioning ''' inline."""
# strip me
x = 1
"#;
        let result = MinimalFilter.filter(code, &Language::Python);
        assert!(
            !result.contains("# strip me"),
            "a ''' inside a \"\"\" string confused the tracker:\n{}",
            result
        );
        assert!(result.contains("x = 1"));
    }

    #[test]
    fn test_minimal_python_still_strips_plain_comments() {
        let code = r#"# leading comment
import os
x = 1  # trailing comment kept, matching prior behavior
"#;
        let result = MinimalFilter.filter(code, &Language::Python);
        assert!(!result.contains("# leading comment"));
        assert!(result.contains("import os"));
        assert!(result.contains("x = 1"));
    }

    #[test]
    fn test_minimal_python_triple_quote_in_one_line_string_opens_nothing() {
        let code = r#"marker = '"""'
# strip me
def f():
    """
    # prose inside the docstring
    """
    return 1
"#;
        let result = MinimalFilter.filter(code, &Language::Python);
        assert!(result.contains(r#"marker = '"""'"#));
        assert!(
            !result.contains("# strip me"),
            "a \"\"\" inside a one-line string opened a string:\n{}",
            result
        );
        assert!(
            result.contains("# prose inside the docstring"),
            "string state inverted, so docstring text was stripped:\n{}",
            result
        );
    }

    #[test]
    fn test_minimal_python_triple_quote_in_trailing_comment_opens_nothing() {
        let code = r#"x = 1  # see """
# strip me
SCRIPT = """
# shell comment inside the string
"""
"#;
        let result = MinimalFilter.filter(code, &Language::Python);
        assert!(
            !result.contains("# strip me"),
            "a \"\"\" inside a trailing comment opened a string:\n{}",
            result
        );
        assert!(
            result.contains("# shell comment inside the string"),
            "string state inverted, so string text was stripped:\n{}",
            result
        );
    }

    #[test]
    fn test_minimal_python_fstring_field_reusing_quote_opens_nothing() {
        let code = r#"v = f"{"""x"""}"
# strip me
w = f"{'"'}" + """
# inside the string
"""
if"{" == v:
    pass
# strip me too
"#;
        let result = MinimalFilter.filter(code, &Language::Python);
        assert!(
            !result.contains("# strip me"),
            "a quote inside an f-string field ended the string:\n{}",
            result
        );
        assert!(
            result.contains("# inside the string"),
            "string state inverted, so string text was stripped:\n{}",
            result
        );
    }

    #[test]
    fn test_minimal_python_escaped_quote_does_not_close_string() {
        let code = r#"s = r"""a\"""
# still inside the string
"""
# strip me
"#;
        let result = MinimalFilter.filter(code, &Language::Python);
        assert!(
            result.contains("# still inside the string"),
            "an escaped quote closed the string early:\n{}",
            result
        );
        assert!(!result.contains("# strip me"));
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

    // --- block comment detection (#2385) ---

    #[test]
    fn test_minimal_keeps_code_with_inline_block_marker() {
        let code = "let glob = \"src/*.rs\";\nfn bar() {}\nfn baz() {}";
        let filter = MinimalFilter;
        let result = filter.filter(code, &Language::Rust);
        assert!(
            result.contains("let glob = \"src/*.rs\";"),
            "line with /* in string literal must be kept, got:\n{}",
            result
        );
        assert!(
            result.contains("fn bar()") && result.contains("fn baz()"),
            "block-comment state must not leak past a non-comment line, got:\n{}",
            result
        );
    }

    #[test]
    fn test_minimal_same_line_block_comment_no_state_leak() {
        let code = "/* inline comment */\nfn foo() {}";
        let filter = MinimalFilter;
        let result = filter.filter(code, &Language::Rust);
        assert!(
            !result.contains("inline comment"),
            "comment-only line is dropped"
        );
        assert!(
            result.contains("fn foo()"),
            "code after same-line block comment must be kept, got:\n{}",
            result
        );
    }

    #[test]
    fn test_minimal_still_drops_multiline_block_comment() {
        let code = "/* start\nstill comment\n*/\nfn after() {}";
        let filter = MinimalFilter;
        let result = filter.filter(code, &Language::Rust);
        assert!(!result.contains("still comment"));
        assert!(result.contains("fn after()"));
    }

    #[test]
    fn test_minimal_keeps_python_inline_triple_quote_assignment() {
        let code = "x = \"\"\"inline\"\"\"\ndef f():\n    pass";
        let filter = MinimalFilter;
        let result = filter.filter(code, &Language::Python);
        assert!(
            result.contains("x = "),
            "assignment with inline triple-quote must be kept, got:\n{}",
            result
        );
        assert!(result.contains("def f():"));
    }

    #[test]
    fn test_minimal_keeps_url_with_trailing_line_comment() {
        let code = "const API: &str = \"http://example.com/v1\";  // endpoint\nfn foo() {}";
        let filter = MinimalFilter;
        let result = filter.filter(code, &Language::Rust);
        assert!(
            result.contains("http://example.com/v1"),
            "URL line must be kept, got:\n{}",
            result
        );
    }

    // --- truncation accuracy ---

    #[test]
    fn test_smart_truncate_overflow_count_exact() {
        // 200 plain-text lines (no function signatures/imports) with max_lines=20.
        // Smart selection keeps up to max_lines/2=10 non-important lines then stops.
        // The overflow message "[N more lines]" must satisfy:
        //   kept_count + N == total_lines
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

        // Parse "[N more lines]"
        let reported_more: usize = overflow_line
            .trim()
            .strip_prefix('[')
            .and_then(|s| s.split_whitespace().next())
            .and_then(|n| n.parse().ok())
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
        // Must contain clean end-of-output marker (1 kept + 9 omitted = 10 total)
        assert!(output.contains("[9 more lines]"));
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
}
