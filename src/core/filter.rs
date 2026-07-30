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

    /// Bodies are scoped by indentation rather than braces or block keywords.
    /// The brace-depth walk in `AggressiveFilter` is blind to these languages.
    pub fn is_indentation_based(&self) -> bool {
        matches!(self, Language::Python)
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
static PY_DEF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(async\s+)?def\s+\w+").unwrap());
static PY_CLASS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^class\s+\w+").unwrap());

/// Statement keywords that can never open a binding, so a `=` after one is a
/// default argument or a comparison rather than an assignment we want to keep.
const PY_STATEMENT_KEYWORDS: [&str; 22] = [
    "if", "elif", "else", "while", "for", "return", "assert", "yield", "raise", "del", "import",
    "from", "lambda", "not", "and", "or", "in", "is", "await", "global", "nonlocal", "with",
];

/// The elision marker uses the target language's own comment syntax. Emitting a
/// C-style `//` into Python or Ruby breaks the Transparency rule: filtered
/// output must read as a shorter version of the real file, not a new format.
fn elision_marker(lang: &Language, indent: usize) -> String {
    let comment = lang.comment_patterns().line.unwrap_or("//");
    format!(
        "{:indent$}{} ... implementation\n",
        "",
        comment,
        indent = indent
    )
}

/// Net bracket nesting introduced by a line, ignoring brackets inside string
/// literals and trailing comments. A positive result means the statement
/// continues onto the following lines.
fn bracket_delta(line: &str) -> i32 {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;

    for ch in line.chars() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '#' => break,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    depth
}

/// Whether the text left of the assignment operator names a binding target.
fn is_binding_target(left: &str) -> bool {
    let head = left.split(':').next().unwrap_or("").trim();
    let head = head
        .trim_end_matches(['+', '-', '*', '/', '%', '&', '|', '^', '>', '<', '@'])
        .trim();
    let ident = match head.find(['[', '(', '.']) {
        Some(pos) => head[..pos].trim(),
        None => head,
    };

    let mut chars = ident.chars();
    let first_is_name_start = chars.next().is_some_and(|c| c.is_alphabetic() || c == '_');

    first_is_name_start
        && chars.all(|c| c.is_alphanumeric() || c == '_')
        && !PY_STATEMENT_KEYWORDS.contains(&ident)
}

/// True for bindings such as `_inherit = "sale.order"`, `total: int = 0` or
/// `count += 1`. Comparisons and keyword statements are rejected.
fn is_assignment(trimmed: &str) -> bool {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut prev = ' ';
    let mut chars = trimmed.char_indices().peekable();

    while let Some((idx, ch)) = chars.next() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == q {
                quote = None;
            }
            prev = ch;
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '#' => return false,
            '=' if depth == 0 => {
                let next = chars.peek().map(|&(_, c)| c).unwrap_or(' ');
                let is_comparison = next == '=' || matches!(prev, '=' | '!' | '<' | '>');
                return !is_comparison && is_binding_target(trimmed[..idx].trim_end());
            }
            _ => {}
        }
        prev = ch;
    }
    false
}

/// The triple-quote delimiter a line leaves open, if any. The two quote kinds
/// are tracked separately: a `'''` inside a `"""` block is text, not a close.
fn unterminated_triple_quote(line: &str) -> Option<&'static str> {
    let bytes = line.as_bytes();
    let mut open: Option<&'static str> = None;
    let mut i = 0;

    while i + 3 <= bytes.len() {
        let delim = match &bytes[i..i + 3] {
            b"\"\"\"" => Some("\"\"\""),
            b"'''" => Some("'''"),
            _ => None,
        };

        match (open, delim) {
            (Some(current), Some(found)) if current == found => {
                open = None;
                i += 3;
            }
            (None, Some(found)) => {
                open = Some(found);
                i += 3;
            }
            _ => i += 1,
        }
    }
    open
}

/// Skeletonizes an indentation-scoped language: keeps imports, `class`/`def`
/// signatures, decorators and every module- or class-level binding, and elides
/// only the statements nested inside a function body.
///
/// Class-level bindings are the point of this path. For ORM-style frameworks
/// the field declarations *are* the model, so dropping them (as the brace-depth
/// walk did) leaves output that describes nothing.
fn filter_indentation_based(minimal: &str, lang: &Language) -> String {
    let mut result = String::with_capacity(minimal.len() / 2);
    let mut def_indent: Option<usize> = None;
    let mut body_elided = false;
    let mut open_brackets = 0i32;
    // Delimiter of the multi-line string being consumed, and whether its
    // contents belong to a line we kept.
    let mut open_string: Option<(&'static str, bool)> = None;

    for line in minimal.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Inside a multi-line string every line is opaque text. Examining it as
        // code would let a docstring sentence be mistaken for a declaration.
        if let Some((delim, keep_contents)) = open_string {
            if keep_contents {
                result.push_str(line);
                result.push('\n');
            }
            if line.contains(delim) {
                open_string = None;
            }
            continue;
        }

        let keep;

        if open_brackets > 0 {
            // A kept statement left brackets open; dropping the remainder would
            // emit syntactically invalid output.
            keep = true;
            open_brackets += bracket_delta(trimmed);
        } else {
            let indent = line.len() - line.trim_start().len();

            if let Some(body_indent) = def_indent {
                if indent > body_indent {
                    if !body_elided {
                        result.push_str(&elision_marker(lang, body_indent + 4));
                        body_elided = true;
                    }
                    open_string = unterminated_triple_quote(line).map(|d| (d, false));
                    continue;
                }
                def_indent = None;
            }

            let is_def = PY_DEF.is_match(trimmed);
            keep = is_def
                || IMPORT_PATTERN.is_match(trimmed)
                || PY_CLASS.is_match(trimmed)
                || trimmed.starts_with('@')
                || is_assignment(trimmed);

            if keep {
                open_brackets = bracket_delta(trimmed).max(0);
                if is_def {
                    def_indent = Some(indent);
                    body_elided = false;
                }
            }
        }

        if keep {
            result.push_str(line);
            result.push('\n');
        }
        open_string = unterminated_triple_quote(line).map(|d| (d, keep));
    }

    result.trim().to_string()
}

impl FilterStrategy for AggressiveFilter {
    fn filter(&self, content: &str, lang: &Language) -> String {
        // Data formats (JSON, YAML, etc.) must never be code-filtered
        if *lang == Language::Data {
            return MinimalFilter.filter(content, lang);
        }

        let minimal = MinimalFilter.filter(content, lang);

        if lang.is_indentation_based() {
            return filter_indentation_based(&minimal, lang);
        }

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
                        result.push_str(&elision_marker(lang, 4));
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

        if kept_lines >= max_lines - 1 {
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

    // --- aggressive filtering of indentation-scoped languages ---

    const ORM_MODEL: &str = r#"from odoo import api, fields, models


class SaleOrder(models.Model):
    """Sales order."""

    _inherit = "sale.order"
    __slots__ = ()

    picking_policy = fields.Selection(
        [("direct", "ASAP"), ("one", "When ready")],
        string="Shipping Policy",
    )

    @api.depends("order_line.move_ids")
    def _compute_reservation(self):
        for order in self:
            order.count = len(order.order_line)

    def action_confirm(self):
        if self.state == "draft":
            return super().action_confirm()
        return False
"#;

    #[test]
    fn test_python_aggressive_keeps_class_level_fields() {
        let result = AggressiveFilter.filter(ORM_MODEL, &Language::Python);

        assert!(
            result.contains(r#"_inherit = "sale.order""#),
            "class-level binding dropped:\n{}",
            result
        );
        assert!(
            result.contains("picking_policy = fields.Selection("),
            "field declaration dropped:\n{}",
            result
        );
        assert!(
            result.contains("__slots__ = ()"),
            "dunder attribute dropped:\n{}",
            result
        );
    }

    #[test]
    fn test_python_aggressive_keeps_decorators() {
        let result = AggressiveFilter.filter(ORM_MODEL, &Language::Python);
        assert!(
            result.contains(r#"@api.depends("order_line.move_ids")"#),
            "decorator dropped:\n{}",
            result
        );
    }

    #[test]
    fn test_python_aggressive_keeps_multiline_field_intact() {
        let result = AggressiveFilter.filter(ORM_MODEL, &Language::Python);
        // A field whose continuation lines are dropped is invalid syntax.
        assert!(
            result.contains(r#"string="Shipping Policy","#),
            "continuation line dropped:\n{}",
            result
        );
        assert!(
            result.contains("    )"),
            "closing paren dropped:\n{}",
            result
        );
    }

    #[test]
    fn test_python_aggressive_still_elides_function_bodies() {
        let result = AggressiveFilter.filter(ORM_MODEL, &Language::Python);

        assert!(result.contains("def _compute_reservation(self):"));
        assert!(
            !result.contains("order.count = len(order.order_line)"),
            "function body kept, savings lost:\n{}",
            result
        );
        assert!(
            !result.contains(r#"if self.state == "draft":"#),
            "function body kept, savings lost:\n{}",
            result
        );
        assert!(
            !result.contains("Sales order."),
            "docstring kept in aggressive skeleton:\n{}",
            result
        );
    }

    #[test]
    fn test_python_aggressive_uses_python_comment_marker() {
        let result = AggressiveFilter.filter(ORM_MODEL, &Language::Python);
        assert!(
            result.contains("# ... implementation"),
            "expected a Python comment marker:\n{}",
            result
        );
        assert!(
            !result.contains("// ... implementation"),
            "C-style marker emitted into Python:\n{}",
            result
        );
    }

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_python_aggressive_savings_on_body_heavy_module() {
        // Bodies are what aggressive mode exists to remove, so a module whose
        // bulk is implementation must clear the project's 60% floor.
        let module = r#"import logging

LOGGER = logging.getLogger(__name__)


class ReportBuilder:
    """Builds aggregated reports."""

    DEFAULT_LIMIT = 100

    def collect(self, rows, limit=None):
        limit = limit or self.DEFAULT_LIMIT
        seen = set()
        result = []
        for row in rows:
            key = (row.get("id"), row.get("kind"))
            if key in seen:
                continue
            seen.add(key)
            result.append(row)
            if len(result) >= limit:
                LOGGER.debug("hit limit %s", limit)
                break
        return result

    def summarize(self, rows):
        totals = {}
        for row in rows:
            kind = row.get("kind", "unknown")
            totals.setdefault(kind, 0)
            totals[kind] += row.get("amount", 0)
        ordered = sorted(totals.items(), key=lambda item: item[1], reverse=True)
        return [{"kind": k, "total": v} for k, v in ordered]
"#;

        let result = AggressiveFilter.filter(module, &Language::Python);
        let savings = 100.0 - (count_tokens(&result) as f64 / count_tokens(module) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "expected >=60% savings, got {:.1}%:\n{}",
            savings,
            result
        );
    }

    #[test]
    fn test_python_aggressive_still_compresses_field_heavy_model() {
        // A model that is mostly field declarations cannot reach the 60% floor,
        // because those declarations are the content the agent came for. Assert
        // only that the skeleton stays materially smaller than the source.
        let result = AggressiveFilter.filter(ORM_MODEL, &Language::Python);
        assert!(
            count_tokens(&result) < count_tokens(ORM_MODEL),
            "aggressive output must be smaller than the source:\n{}",
            result
        );
    }

    #[test]
    fn test_brace_language_aggressive_keeps_c_style_marker() {
        let code = r#"fn main() {
    let x = compute();
    println!("{}", x);
}
"#;
        let result = AggressiveFilter.filter(code, &Language::Rust);
        assert!(result.contains("fn main()"));
        assert!(
            result.contains("// ... implementation"),
            "Rust must keep the C-style marker:\n{}",
            result
        );
    }

    #[test]
    fn test_is_assignment_accepts_bindings() {
        assert!(is_assignment(r#"_inherit = "sale.order""#));
        assert!(is_assignment("total: int = 0"));
        assert!(is_assignment("count += 1"));
        assert!(is_assignment("MAPPING = {'a': 1}"));
        assert!(is_assignment("obj.attr = 3"));
    }

    #[test]
    fn test_is_assignment_rejects_non_bindings() {
        assert!(!is_assignment(r#"if self.state == "draft":"#));
        assert!(!is_assignment("assert a != b"));
        assert!(!is_assignment("return value"));
        assert!(!is_assignment("check(threshold=3)"));
        assert!(!is_assignment("while count <= limit:"));
        assert!(!is_assignment(r#"raise ValueError("x = 1")"#));
    }

    #[test]
    fn test_python_aggressive_docstring_does_not_swallow_following_code() {
        // A ''' inside a """ block is text. Closing on either delimiter made the
        // filter lose track and eat the class that followed.
        let module = r#""""Module docstring.

Example: value = compute('''x''')
"""

class Config:
    name = "x"
"#;
        let result = AggressiveFilter.filter(module, &Language::Python);

        assert!(
            result.contains("class Config:"),
            "class after docstring was swallowed:\n{}",
            result
        );
        assert!(
            result.contains(r#"name = "x""#),
            "class body after docstring was swallowed:\n{}",
            result
        );
        assert!(
            !result.contains("Example:"),
            "docstring prose leaked into the skeleton:\n{}",
            result
        );
    }

    #[test]
    fn test_unterminated_triple_quote() {
        assert_eq!(unterminated_triple_quote(r#"x = """"#), Some("\"\"\""));
        assert_eq!(unterminated_triple_quote("x = '''"), Some("'''"));
        assert_eq!(unterminated_triple_quote(r#"x = """one line""""#), None);
        // The inner delimiter is text, so the outer string is still open.
        assert_eq!(
            unterminated_triple_quote(r#""""outer '''inner'''"#),
            Some("\"\"\"")
        );
        assert_eq!(unterminated_triple_quote("plain = 1"), None);
    }

    #[test]
    fn test_bracket_delta_ignores_strings_and_comments() {
        assert_eq!(bracket_delta("x = f("), 1);
        assert_eq!(bracket_delta("x = f()"), 0);
        assert_eq!(bracket_delta(r#"x = "((("  # )))"#), 0);
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
