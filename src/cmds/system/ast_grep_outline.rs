//! Filters `ast-grep outline` plain output: caps multi-file scans, truncates
//! container name lists, and keeps single-file outlines intact. The run/json
//! dispatch lives in `ast_grep_cmd`.

use std::borrow::Cow;
use std::sync::LazyLock;

/// `outline` item/member lines: indent, then a line number or kind word.
/// Bare file-header paths never match.
static OUTLINE_ITEM_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\s*(?:\d+|[A-Za-z_][A-Za-z0-9_]*):\s").unwrap());

/// Container member lists (`kind: name, …`). Signature lines start with a line
/// number, so they never match and stay intact.
static OUTLINE_NAMES_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^(\s*[A-Za-z_][A-Za-z0-9_]*: )(.*)$").unwrap());

const OUTLINE_MAX_FILES: usize = 30;
const OUTLINE_MAX_LINES_PER_FILE: usize = 12;
const OUTLINE_MAX_NAMES: usize = 8;

/// One `outline` file block: header path, member lines, and whether a blank
/// line preceded the header in the raw output.
struct OutlineBlock<'a> {
    header: &'a str,
    items: Vec<&'a str>,
    blank_before: bool,
}

/// Applies the outline filter with the default caps.
pub fn filter(raw: &str) -> String {
    filter_ast_grep_outline(
        raw,
        OUTLINE_MAX_FILES,
        OUTLINE_MAX_LINES_PER_FILE,
        OUTLINE_MAX_NAMES,
    )
}

/// A non-item, space-free line with a path marker (`/` or `.`) is an
/// `outline` file header. Content lines that merely mention a path (imports,
/// comments) contain spaces and stay items, so unrelated text falls through
/// and the raw passthrough stays intact.
fn is_outline_header(line: &str) -> bool {
    !line.trim().is_empty()
        && !OUTLINE_ITEM_RE.is_match(line)
        && !line.contains(' ')
        && (line.contains('/') || line.contains('.'))
}

/// Truncates `kind: name, …` lists to `max_names` names, collapsing the tail.
fn truncate_name_list(line: &str, max_names: usize) -> Cow<'_, str> {
    let Some(caps) = OUTLINE_NAMES_RE.captures(line) else {
        return Cow::Borrowed(line);
    };
    let (Some(prefix), Some(rest)) = (
        caps.get(1).map(|m| m.as_str()),
        caps.get(2).map(|m| m.as_str()),
    ) else {
        return Cow::Borrowed(line);
    };
    if !rest.contains(", ") {
        return Cow::Borrowed(line);
    }
    let names: Vec<&str> = rest.split(", ").collect();
    if names.len() <= max_names {
        return Cow::Borrowed(line);
    }
    Cow::Owned(format!(
        "{}{}, … +{} more",
        prefix,
        names[..max_names].join(", "),
        names.len() - max_names
    ))
}

/// Keeps at most `max_files` blocks in a multi-file scan and `max_lines_per_file`
/// lines per block; overflow collapses to a count hint. A single-file outline
/// keeps every line, only truncating name lists, so structure never disappears.
fn filter_ast_grep_outline(
    raw: &str,
    max_files: usize,
    max_lines_per_file: usize,
    max_names: usize,
) -> String {
    let mut blocks: Vec<OutlineBlock> = Vec::new();
    let mut prologue: Vec<&str> = Vec::new();
    let mut prev_blank = false;
    for line in raw.lines() {
        if line.trim().is_empty() {
            prev_blank = true;
            continue;
        }
        if is_outline_header(line) {
            blocks.push(OutlineBlock {
                header: line,
                items: Vec::new(),
                blank_before: prev_blank,
            });
            prev_blank = false;
        } else if let Some(last) = blocks.last_mut() {
            last.items.push(line);
            prev_blank = false;
        } else {
            // Lines before the first header are content too; keep them.
            prologue.push(line);
            prev_blank = false;
        }
    }
    if blocks.is_empty() {
        return raw.to_string();
    }

    let line_cap = if blocks.len() > 1 {
        max_lines_per_file
    } else {
        usize::MAX
    };
    let mut out = String::new();
    for line in &prologue {
        out.push_str(line);
        out.push('\n');
    }
    let mut shown = 0;
    let mut skipped = 0;
    for block in &blocks {
        if shown >= max_files {
            skipped += 1;
            continue;
        }
        if block.blank_before && !out.is_empty() {
            out.push('\n');
        }
        out.push_str(block.header);
        out.push('\n');
        for item in block.items.iter().take(line_cap) {
            out.push_str(&truncate_name_list(item, max_names));
            out.push('\n');
        }
        if block.items.len() > line_cap {
            out.push_str(&format!(
                "  … {} more items in {}\n",
                block.items.len() - line_cap,
                block.header
            ));
        }
        shown += 1;
    }
    if skipped > 0 {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!(
            "… {} more file(s) not shown (narrow the path or use --view names)\n",
            skipped
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    fn savings_pct(input: &str, output: &str, count: impl Fn(&str) -> usize) -> f64 {
        let input_tokens = count(input);
        let output_tokens = count(output);
        100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0)
    }

    #[test]
    fn test_outline_caps_files_and_lines() {
        let input = "\
src/a.rs
function: run
struct: Foo, Bar

src/b.rs
function: main

src/c.rs
function: helper
";
        let out = filter_ast_grep_outline(input, 2, 5, 8);
        assert!(out.contains("src/a.rs\nfunction: run"));
        assert!(out.contains("src/b.rs\nfunction: main"));
        assert!(!out.contains("src/c.rs"));
        assert!(out.contains("1 more file(s) not shown"));
    }

    #[test]
    fn test_outline_overflow_items_collapse_to_count() {
        let input = "\
src/a.rs
 1: fn one
 2: fn two
 3: fn three
 4: fn four

src/b.rs
 1: fn main
";
        let out = filter_ast_grep_outline(input, 10, 3, 8);
        assert!(out.contains(" 1: fn one"));
        assert!(out.contains(" 3: fn three"));
        assert!(!out.contains(" 4: fn four"));
        assert!(out.contains("1 more items in src/a.rs"));
        assert!(out.contains("src/b.rs\n 1: fn main"));
    }

    #[test]
    fn test_outline_single_file_keeps_every_item() {
        let input = "\
src/a.rs
 1: fn one
 2: fn two
 3: fn three
 4: fn four
 5: fn five
";
        let out = filter_ast_grep_outline(input, 10, 2, 8);
        assert_eq!(out.lines().count(), input.lines().count());
        assert!(out.contains(" 5: fn five"));
        assert!(!out.contains("more items"));
    }

    #[test]
    fn test_outline_truncates_member_name_lists() {
        let input = "\
src/main.rs
   1: enum Color
         enumMember: Red, Green, Blue, Cyan, Magenta, Yellow, Black, White, Transparent
";
        let out = filter_ast_grep_outline(input, 10, 10, 6);
        assert!(out.contains("Red, Green, Blue, Cyan, Magenta, Yellow, … +3 more"));
        assert!(!out.contains("White"));
    }

    #[test]
    fn test_outline_signatures_not_name_truncated() {
        let input = "\
src/main.rs
  10: pub fn run(file: &Path, verbose: u8, name: &str, extra: bool)
";
        let out = filter_ast_grep_outline(input, 10, 10, 2);
        assert!(out.contains("pub fn run(file: &Path, verbose: u8, name: &str, extra: bool)"));
    }

    #[test]
    fn test_outline_unparseable_returns_raw() {
        let input = "garbage line\nnot an outline\n";
        assert_eq!(filter_ast_grep_outline(input, 10, 10, 8), input);
    }

    #[test]
    fn test_outline_keeps_raw_block_separators() {
        let input = "\
src/a.rs
module: foo, bar
src/b.rs
module: baz, qux
";
        let out = filter_ast_grep_outline(input, 10, 10, 8);
        assert!(out.contains("module: foo, bar\nsrc/b.rs"));
        assert!(!out.contains("module: foo, bar\n\nsrc/b.rs"));
    }

    #[test]
    fn test_outline_empty_input() {
        assert_eq!(filter_ast_grep_outline("", 10, 10, 8), "");
    }

    #[test]
    fn test_outline_slash_lines_stay_items_not_headers() {
        let mut input = String::from("src/app.js\n");
        input.push_str("import { bar } from './bar.js';\n");
        for i in 0..15 {
            input.push_str(&format!("{:>4}: fn item{}\n", i + 1, i + 1));
        }
        let out = filter_ast_grep_outline(&input, 30, 12, 8);
        assert!(
            out.contains(" 15: fn item15"),
            "import lines must not turn a single-file outline into a capped multi-file scan"
        );
        assert!(out.contains("import { bar } from './bar.js';"));
        assert!(!out.contains("more items"));
    }

    #[test]
    fn test_outline_keeps_leading_lines() {
        let input = "scan starting\nnotice: warmup\nsrc/a.rs\n 1: fn main\n";
        let out = filter_ast_grep_outline(input, 10, 10, 8);
        assert!(out.starts_with("scan starting\nnotice: warmup\nsrc/a.rs"));
    }

    #[test]
    fn test_outline_real_fixture_savings() {
        let input = include_str!("../../../tests/fixtures/ast_grep_outline_dir_raw.txt");
        let output = filter(input);

        let savings = savings_pct(input, &output, count_tokens);
        assert!(
            savings >= 60.0,
            "ast-grep outline filter: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_outline_file_fixture_keeps_all_lines_and_saves() {
        let input = include_str!("../../../tests/fixtures/ast_grep_outline_file_raw.txt");
        let output = filter(input);

        assert_eq!(
            output.lines().count(),
            input.lines().count(),
            "single-file outline must keep every line"
        );

        let savings = savings_pct(input, &output, crate::core::tracking::estimate_tokens);
        assert!(
            savings >= 40.0,
            "ast-grep outline file filter: expected >=40% savings, got {:.1}%",
            savings
        );
    }
}
