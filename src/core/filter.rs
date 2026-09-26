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
            // Python is filtered by `filter_python_minimal` and never reaches the
            // block-comment walk, where `"""` would be misread as a comment.
            Language::Python => CommentPatterns {
                line: Some("#"),
                block_start: None,
                block_end: None,
                doc_line: None,
                doc_block_start: None,
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
/// An encoding declaration (PEP 263), with the pattern Python's tokenizer uses:
/// ASCII only in the encoding name.
static PYTHON_CODING_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[ \t\x0c]*#.*?coding[:=][ \t]*[-A-Za-z0-9_.]+").unwrap());

/// One open token of a Python line scan: a string literal, or a replacement
/// field inside an f-string or t-string. Scan state is a stack of these, carried
/// from line to line, because a string or a field can hold the other.
enum PyToken {
    Str {
        quote: u8,
        triple: bool,
        interpolated: bool,
    },
    /// `depth` counts the brackets opened inside the field.
    Field { depth: usize },
    /// The format spec of a field, after its `:`: literal text in which `{`
    /// opens a nested field and `}` closes the field the spec belongs to.
    Spec,
}

/// True when the identifier ending at `before` is an f-string or t-string
/// prefix (`f`, `Rt`, ...). Any other identifier, such as the `if` in `if"x"`,
/// is not one.
fn is_interpolated_prefix(before: &[u8]) -> bool {
    let start = before
        .iter()
        .rposition(|b| !(b.is_ascii_alphanumeric() || *b == b'_'))
        .map_or(0, |p| p + 1);
    let prefix = &before[start..];
    ["f", "fr", "rf", "t", "tr", "rt"]
        .iter()
        .any(|p| prefix.eq_ignore_ascii_case(p.as_bytes()))
}

/// Advances the scan state across one physical line.
///
/// Outside a string a `#` ends the line and a quote opens a string, so a `"""`
/// inside `'"""'` or after a trailing comment opens nothing. Inside a string a
/// backslash keeps the next byte from closing it; in an f-string it never
/// escapes a brace (`\{{` is a backslash then an escaped brace). The braces of
/// a `\N{...}` escape are scanned as a field, which changes nothing: a
/// character name holds no quote, colon or `#`. A one-line string ends with its
/// line unless a backslash continues it or one of its replacement fields is
/// still open.
///
/// Returns whether the line ends with a backslash outside string text and
/// comments (in code or a replacement field), which joins it to the next line.
fn scan_python_line(line: &str, stack: &mut Vec<PyToken>) -> bool {
    let bytes = line.as_bytes();
    let mut continued = false;
    let mut joins_next = false;
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];
        match stack.last_mut() {
            Some(&mut PyToken::Str {
                quote,
                triple,
                interpolated,
            }) => match b {
                b'\\' => match bytes.get(i + 1) {
                    None => {
                        continued = true;
                        i += 1;
                    }
                    Some(b'{' | b'}') if interpolated => i += 1,
                    Some(_) => i += 2,
                },
                b'{' | b'}' if interpolated => {
                    if bytes.get(i + 1) == Some(&b) {
                        i += 2;
                    } else {
                        if b == b'{' {
                            stack.push(PyToken::Field { depth: 0 });
                        }
                        i += 1;
                    }
                }
                _ if b == quote && (!triple || bytes[i..].starts_with(&[quote; 3])) => {
                    stack.pop();
                    i += if triple { 3 } else { 1 };
                }
                _ => i += 1,
            },
            Some(PyToken::Spec) => {
                match b {
                    b'{' => stack.push(PyToken::Field { depth: 0 }),
                    b'}' => {
                        stack.pop();
                    }
                    _ => {}
                }
                i += 1;
            }
            _ => {
                match b {
                    b'#' => break,
                    b'"' | b'\'' => {
                        let triple = bytes[i..].starts_with(&[b; 3]);
                        stack.push(PyToken::Str {
                            quote: b,
                            triple,
                            interpolated: is_interpolated_prefix(&bytes[..i]),
                        });
                        i += if triple { 3 } else { 1 };
                        continue;
                    }
                    b'(' | b'[' | b'{' => {
                        if let Some(PyToken::Field { depth }) = stack.last_mut() {
                            *depth += 1;
                        }
                    }
                    b')' | b']' | b'}' => match stack.last_mut() {
                        Some(PyToken::Field { depth: 0 }) if b == b'}' => {
                            stack.pop();
                        }
                        Some(PyToken::Field { depth }) => *depth = depth.saturating_sub(1),
                        _ => {}
                    },
                    b':' => {
                        if let Some(top @ PyToken::Field { depth: 0 }) = stack.last_mut() {
                            *top = PyToken::Spec;
                        }
                    }
                    b'\\' if i + 1 == bytes.len() => joins_next = true,
                    _ => {}
                }
                i += 1;
            }
        }
    }

    if !continued && let Some(PyToken::Str { triple: false, .. }) = stack.last() {
        stack.pop();
    }
    joins_next
}

/// Python has no block comments. `"""` opens a *string*, which may be a
/// docstring or an ordinary value, so it cannot be matched with the
/// line-oriented block-comment rules the other languages use: a line such as
/// `QUERY = """` both contains and "closes" the delimiter, and a single-line
/// docstring toggles the state once and never back.
///
/// Minimal keeps docstrings, so the only thing to remove here is `#` comment
/// lines, and the only state needed is which strings and fields are open. A
/// file Python cannot parse, such as one being edited, is followed only as far
/// as the scan can track it; nothing is promised past the point where it breaks.
fn filter_python_minimal(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let mut open: Vec<PyToken> = Vec::new();
    // The last kept line ends with a backslash that joins it to the next line.
    let mut joined = false;
    // A blank line was kept since the last kept line of code; starting true
    // keeps blank lines from opening the output.
    let mut blank_kept = true;
    // The last kept line of code joined onto a blank line.
    let mut ends_in_joined_blank = false;
    // Python may read an encoding declaration on line 2 (set by line 1).
    let mut coding_on_line_two = false;

    // Python ends a line at `\n`, `\r\n` or a lone `\r`; `lines()` only knows
    // the first two.
    for (number, line) in content
        .lines()
        .flat_map(|line| line.split('\r'))
        .enumerate()
    {
        let trimmed = line.trim();

        // A `#!` at the very start of the file names the interpreter, and an
        // encoding declaration sets how Python decodes the file. Python reads
        // one on line 1, or on line 2 when line 1 is blank or a comment that
        // declares none. Such lines carry meaning, so none is dropped as a
        // comment.
        let declares_coding = number < 2 && PYTHON_CODING_LINE.is_match(line);
        let meaningful = match number {
            0 => line.starts_with("#!") || declares_coding,
            1 => coding_on_line_two && declares_coding,
            _ => false,
        };
        if number == 0 {
            let head = line.trim_start_matches([' ', '\t', '\x0c']);
            coding_on_line_two = (head.is_empty() || head.starts_with('#')) && !declares_coding;
        }

        // Inside a string or a replacement field every line is kept as it is,
        // blank ones and ones that start with `#` included. Outside, a
        // comment's contents are not code, so any delimiter in it is not real,
        // except that a comment right after a backslash continuation ends that
        // logical line: dropping it would join the continuation to the next
        // statement. Runs of blank lines outside strings are cut to one.
        if open.is_empty() {
            if trimmed.starts_with('#') && !joined && !meaningful {
                continue;
            }
            if trimmed.is_empty() {
                ends_in_joined_blank |= joined;
                joined = false;
                if !blank_kept {
                    blank_kept = true;
                    result.push('\n');
                }
                continue;
            }
        }

        // A declaration kept from line 2 that comes first and starts with `#!`
        // would become a shebang the file does not have: a blank line in front
        // keeps it on line 2, where Python still reads it.
        if number == 1 && meaningful && result.is_empty() && line.starts_with("#!") {
            result.push('\n');
        }
        result.push_str(line);
        result.push('\n');
        blank_kept = false;
        ends_in_joined_blank = false;
        joined = scan_python_line(line, &mut open);
    }

    result.truncate(result.trim_end().len());
    // A continuation needs a line after it, and a file that ends with one
    // keeps the blank line that follows it.
    if ends_in_joined_blank {
        result.push_str("\n\n");
    }
    result
}

impl FilterStrategy for MinimalFilter {
    fn filter(&self, content: &str, lang: &Language) -> String {
        if *lang == Language::Python {
            return filter_python_minimal(content);
        }

        let patterns = lang.comment_patterns();
        let mut result = String::with_capacity(content.len());
        let mut in_block_comment = false;

        for line in content.lines() {
            let trimmed = line.trim();

            // Handle block comments
            if let (Some(start), Some(end)) = (patterns.block_start, patterns.block_end) {
                // starts_with, not contains: `/*` inside a string literal or
                // glob (e.g. "src/*.rs") must not open a comment block (#2385)
                if trimmed.starts_with(start)
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
    fn test_minimal_python_keeps_blank_lines_inside_strings() {
        let code = "s = \"\"\"a\n\n\n\nb\"\"\"\n\n\n\nx = 1\n";
        let result = MinimalFilter.filter(code, &Language::Python);
        assert_eq!(result, "s = \"\"\"a\n\n\n\nb\"\"\"\n\nx = 1");
    }

    #[test]
    fn test_minimal_python_keeps_the_line_after_a_final_continuation() {
        for code in ["x = 1 \\\n\n", "x = 1 \\\n   \n", "x = 1 \\\r\x0c\r# c\r"] {
            let result = MinimalFilter.filter(code, &Language::Python);
            assert_eq!(result, "x = 1 \\\n\n", "input {code:?}");
        }
        let result = MinimalFilter.filter("x = 1 \\\n# c\n", &Language::Python);
        assert_eq!(result, "x = 1 \\\n# c");
        let result = MinimalFilter.filter("x = 1 \\\n\ny = 2\n", &Language::Python);
        assert_eq!(result, "x = 1 \\\n\ny = 2");
    }

    #[test]
    fn test_minimal_python_keeps_shebang_and_coding_lines() {
        let code = "#!/usr/bin/env python3\n# -*- coding: latin-1 -*-\n# drop\nx = 1\n";
        let result = MinimalFilter.filter(code, &Language::Python);
        assert_eq!(
            result,
            "#!/usr/bin/env python3\n# -*- coding: latin-1 -*-\nx = 1"
        );
        let code = "# vim: set fileencoding=utf-8 :\n# drop\n# coding: ascii\nx = 1\n";
        let result = MinimalFilter.filter(code, &Language::Python);
        assert_eq!(result, "# vim: set fileencoding=utf-8 :\nx = 1");
        let code = "x = 1\n#!/not/a/shebang\n";
        assert_eq!(MinimalFilter.filter(code, &Language::Python), "x = 1");
        // Lines Python does not read as a shebang or an encoding declaration.
        for code in [
            "   #!/usr/bin/env python3\nx = 1\n",
            "# coding: \u{e9}t\u{e9}\nx = 1\n",
            "x = 1\n# coding: ascii\n",
            "# -*- coding: latin-1 -*-\n# coding: ascii\nx = 1\n",
        ] {
            let result = MinimalFilter.filter(code, &Language::Python);
            assert!(
                !result.contains("#!/usr")
                    && !result.contains("coding: ascii")
                    && !result.contains("coding: \u{e9}"),
                "{code:?} gave {result:?}"
            );
        }
        let code = "\n# coding: latin-1\nx = 1\n";
        assert_eq!(
            MinimalFilter.filter(code, &Language::Python),
            "# coding: latin-1\nx = 1"
        );
        for (code, expected) in [
            (
                "\x0c# c\n# coding: latin-1\nx = 1\n",
                "# coding: latin-1\nx = 1",
            ),
            (
                "  # c\n# coding: latin-1\nx = 1\n",
                "# coding: latin-1\nx = 1",
            ),
            // A `#!` that is not at the start of the file must not end up there.
            (
                "  #!/usr/bin/python -*- coding: latin-1 -*-\nx = 1\n",
                "  #!/usr/bin/python -*- coding: latin-1 -*-\nx = 1",
            ),
            (
                "# Copyright\n#!/usr/bin/python -*- coding: latin-1 -*-\nx = 1\n",
                "\n#!/usr/bin/python -*- coding: latin-1 -*-\nx = 1",
            ),
            (
                "\n#!/usr/bin/python -*- coding: latin-1 -*-\nx = 1\n",
                "\n#!/usr/bin/python -*- coding: latin-1 -*-\nx = 1",
            ),
            (
                "# c\r#!/x coding: latin-1\rx = 1\r",
                "\n#!/x coding: latin-1\nx = 1",
            ),
            // The first kept line keeps its indentation.
            ("# c\n    x = 1\n", "    x = 1"),
            // Behind a real shebang, a line-2 declaration stays on line 2.
            (
                "#!/usr/bin/python\n#!x coding: latin-1\nx = 1\n",
                "#!/usr/bin/python\n#!x coding: latin-1\nx = 1",
            ),
            ("\n\n\nx = 1\n", "x = 1"),
        ] {
            assert_eq!(
                MinimalFilter.filter(code, &Language::Python),
                expected,
                "{code:?}"
            );
        }
    }

    #[test]
    fn test_minimal_python_byte_order_mark_makes_line_one_code() {
        // `trim()` does not strip U+FEFF, so the mark makes line 1 a code line:
        // it is kept as it is, and so is a blank line after it.
        for (code, expected) in [
            ("\u{feff}", "\u{feff}"),
            ("\u{feff}x = 1\n", "\u{feff}x = 1"),
            ("\u{feff}\nx = 1\n", "\u{feff}\nx = 1"),
            ("\u{feff}# c\nx = 1\n", "\u{feff}# c\nx = 1"),
        ] {
            assert_eq!(
                MinimalFilter.filter(code, &Language::Python),
                expected,
                "{code:?}"
            );
        }
    }

    /// Every line of a case that contains `keep` must come out whole, and no
    /// `# drop` comment may come out at all.
    #[test]
    fn test_minimal_python_string_scan_keeps_string_lines_and_drops_comments() {
        let cases = [
            (
                "backslash before an escaped brace in an f-string",
                "OPEN = rf\"\\{{\"; HELP = \"\"\"\n# keep usage\n\"\"\"\n# drop\n",
            ),
            (
                "backslash before an escaped brace, non-raw (an invalid escape, a SyntaxWarning since 3.12)",
                "x = f\"a\\{{\" + \"\"\"\n# keep\n\"\"\"\n# drop\n",
            ),
            (
                "escaped brace at depth zero",
                "x = f\"{{\" + \"\"\"\n# keep\n\"\"\"\n# drop\n",
            ),
            (
                "escaped quote in a one-line string",
                "x = 'a\\'b' + \"\"\"\n# keep\n\"\"\"\n# drop\n",
            ),
            (
                "keyword before a quote is not a prefix",
                "if\"{\" == v: s = \"\"\"\n# keep\n\"\"\"\n# drop\n",
            ),
            (
                "triple quote inside a field of a triple-quoted f-string",
                "x = f\"\"\"{'\"\"\"'}\n# keep\n\"\"\"\n# drop\n",
            ),
            (
                "colon inside brackets of a field is not a format spec",
                "x = f\"{ {1: 'x'}['\"'] }\" + '''\n# keep\n'''\n# drop\n",
            ),
            (
                "format spec of a triple-quoted f-string spanning lines",
                "x = f'{f\"\"\"{y:\nz}\"\"\"}'\n# drop\ndef f():\n    \"\"\"\n    # keep\n    \"\"\"\n",
            ),
            (
                "format spec left open across a backslash-continued line",
                "msg = f\"\"\"{f'{d:%Y-%m-%d \\\n%A}'}\n# keep text\n\"\"\"\n# drop\n",
            ),
            (
                "triple-quoted string nested in a field spanning lines",
                "x = f\"{'''\ntext\n'''}\" + \"\"\"\n# keep\n\"\"\"\n# drop\n",
            ),
            (
                "format spec text across lines starting with a quote and a hash",
                "x = f\"\"\"{y:\n'#>10}\"\"\" + \"\"\"\n# keep\n\"\"\"\n# drop\n",
            ),
            (
                "open brace inside a same-quote string nested in a field",
                "d = f\"{\"{\"}\" + \"\"\"\n# keep\n\"\"\"\n# drop\n",
            ),
            (
                "field spanning lines in a single-quoted f-string",
                "a = f'{\n    n\n}' + \"\"\"\n# keep\n\"\"\"\n# drop\n",
            ),
            (
                "hash inside a field starts a comment, even before a quote",
                "x = f'''{\n    y  # }'''\n    + 1}\n# keep\n'''\n# drop\n",
            ),
            (
                "CRLF line endings with a continued one-line string",
                "x = \"abc \\\r\n# keep continued\"\r\n# drop\r\n",
            ),
            (
                "comment line after a backslash continuation",
                "x = 1 \\\n# keep, it ends the joined line\ny = 2\n# drop\n",
            ),
            (
                "blank line ends a backslash continuation",
                "x = 1 \\\n\n# drop\ny = 2\n",
            ),
            (
                "comment ending in a backslash does not continue",
                "x = 1  # note \\\n# drop\ny = 2\n",
            ),
            (
                "lone CR line endings",
                "x = 1\r# drop\rs = \"\"\"\r# keep\r\"\"\"\r# drop\r",
            ),
            (
                "field nested in a format spec",
                "x = f\"\"\"{x:{'}\"\"\"'}}\n# keep\n\"\"\"\n# drop\n",
            ),
            (
                "comment line inside a field spanning lines is kept, like every line of an open field",
                "x = f\"{\n    # keep comment in field\n    x\n}\"\n# drop\n",
            ),
            (
                "continued one-line string followed by a triple quote",
                "x = \"abc \\\ninside\"; y = \"\"\"\n# keep\n\"\"\"\n# drop\n",
            ),
            (
                "field spanning lines",
                "a = f\"{\n    n\n}\" + \"\"\"\n# keep\n\"\"\"\n# drop\n",
            ),
            (
                "open brace inside a string nested in a field",
                "a = f\"{'{'}\" + \"\"\"\n# keep\n\"\"\"\n# drop\n",
            ),
            (
                "close brace inside a same-quote nested string",
                "c = f\"{\"}\"}\" + \"\"\"\n# keep\n\"\"\"\n# drop\n",
            ),
            (
                "brackets inside a field",
                "x = f\"{ {'#': 1}[\"#\"] }\" + \"\"\"\n# keep\n\"\"\"\n# drop\n",
            ),
            (
                "unterminated one-line string (invalid Python) ends with its line",
                "x = 'abc\n# drop\ndef f():\n    \"\"\"\n    # keep prose\n    \"\"\"\n",
            ),
            (
                "hash and quote in a format spec",
                "a = f\"{n:#x}\" + f\"{n:'>10}\" + \"\"\"\n# keep\n\"\"\"\n# drop\n",
            ),
            (
                "backslash-newline continues a one-line string",
                "x = \"abc \\\n# keep continued\"\n# drop\n",
            ),
        ];

        for (name, code) in cases {
            let result = MinimalFilter.filter(code, &Language::Python);
            for line in code.lines().flat_map(|line| line.split('\r')) {
                if line.contains("keep") {
                    assert!(
                        result.lines().any(|kept| kept == line),
                        "{name}: a line that must be kept was stripped: {line:?}\n{result}"
                    );
                }
            }
            assert!(
                !result.contains("# drop"),
                "{name}: comment outside every string was kept:\n{result}"
            );
        }
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
