//! Compact filter for `jq` — truncates large JSON results with structural awareness.
//!
//! Compression strategies:
//! - Small results (<MAX_LINES lines): pass through verbatim
//! - Large arrays: first_N + "… (X more) …" + last_N
//! - Large raw text (`-r`/`--raw-output`): line-based head/tail
//! - Large objects/mixed: head/tail with line count hint
//!
//! Expected savings: 40–70% on large JSON outputs.

use crate::core::runner::{self, RunOptions};
use crate::core::shell::display_args;
use crate::core::truncate::CAP_LIST;
use crate::core::utils::{ChildArgExt, resolved_command};
use anyhow::Result;

/// Maximum lines before truncation kicks in.
const MAX_LINES: usize = CAP_LIST; // 20

/// How many items to show at the head and tail of a truncated array.
const ARRAY_HEAD: usize = 5;
const ARRAY_TAIL: usize = 3;

/// Maximum lines for raw-output mode head/tail.
const RAW_HEAD: usize = 15;
const RAW_TAIL: usize = 5;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("jq");
    cmd.child_args(args);

    if verbose > 0 {
        eprintln!("Running: jq {}", display_args(args));
    }

    let is_raw = args
        .iter()
        .any(|a| a == "-r" || a == "--raw-output" || a == "-er" || a == "-re");

    // jq commonly reads from stdin (piped JSON) or from a file argument.
    // Always inherit stdin so piped usage works: `curl ... | rtk jq '.paths'`
    let opts = RunOptions::stdout_only().inherit_stdin();

    runner::run_filtered(
        cmd,
        "jq",
        &display_args(args),
        |stdout| {
            if is_raw {
                filter_jq_raw(stdout)
            } else {
                filter_jq_json(stdout)
            }
        },
        opts,
    )
}

/// Filter raw-output mode: simple line-based head/tail truncation.
fn filter_jq_raw(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    let total = lines.len();

    if total <= RAW_HEAD + RAW_TAIL {
        return raw.to_string();
    }

    let mut out = String::with_capacity(raw.len() / 2);
    for line in &lines[..RAW_HEAD] {
        out.push_str(line);
        out.push('\n');
    }

    let hidden = total - RAW_HEAD - RAW_TAIL;
    out.push_str(&format!("… ({hidden} lines omitted) …\n"));

    for (i, line) in lines[total - RAW_TAIL..].iter().enumerate() {
        out.push_str(line);
        if i < RAW_TAIL - 1 {
            out.push('\n');
        }
    }

    // Append a tee hint so the agent can recover the full output.
    if let Some(hint) = crate::core::tee::force_tee_tail_hint(raw, "jq-raw", RAW_HEAD + 1) {
        out.push('\n');
        out.push_str(&hint);
    }

    out
}

/// Filter JSON-mode output with structural awareness.
///
/// Detects top-level arrays and applies head/tail element truncation.
/// For other structures (objects, mixed), applies line-based truncation.
fn filter_jq_json(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let lines: Vec<&str> = trimmed.lines().collect();
    if lines.len() <= MAX_LINES {
        return raw.to_string();
    }

    // Try to detect and compress a top-level JSON array.
    if trimmed.starts_with('[')
        && let Some(compressed) = compress_json_array(trimmed)
    {
        return maybe_add_hint(compressed, raw);
    }

    // Fallback: line-based head/tail for large objects or mixed output.
    compress_lines(&lines, raw)
}

/// Compress a top-level JSON array by keeping first_N and last_N elements.
///
/// jq pretty-prints arrays as one element per line (simple values) or as
/// multi-line blocks (objects). We count top-level elements by tracking
/// bracket depth.
fn compress_json_array(json: &str) -> Option<String> {
    // Parse to count elements. We use a lightweight approach: split by
    // top-level commas (depth == 1) to identify element boundaries.
    let elements = split_top_level_elements(json)?;
    let count = elements.len();

    if count <= ARRAY_HEAD + ARRAY_TAIL {
        return None; // small enough — let never_worse handle it
    }

    let mut out = String::with_capacity(json.len() / 2);
    out.push_str("[\n");

    // Head elements
    for (i, elem) in elements.iter().take(ARRAY_HEAD).enumerate() {
        out.push_str("  ");
        out.push_str(elem.trim());
        if i < ARRAY_HEAD - 1 {
            out.push(',');
        }
        out.push('\n');
    }

    // Omission marker
    let hidden = count - ARRAY_HEAD - ARRAY_TAIL;
    out.push_str(&format!("  … ({hidden} more elements omitted) …\n"));

    // Tail elements
    let tail_start = count - ARRAY_TAIL;
    for (i, elem) in elements[tail_start..].iter().enumerate() {
        out.push_str("  ");
        out.push_str(elem.trim());
        if i < ARRAY_TAIL - 1 {
            out.push(',');
        }
        out.push('\n');
    }

    out.push(']');
    Some(out)
}

/// Split a JSON array string into its top-level elements.
/// Returns `None` if the input doesn't look like a valid JSON array.
fn split_top_level_elements(json: &str) -> Option<Vec<&str>> {
    let trimmed = json.trim();
    if !trimmed.starts_with('[') || !trimmed.ends_with(']') {
        return None;
    }

    // Strip outer brackets
    let inner = &trimmed[1..trimmed.len() - 1];
    let bytes = inner.as_bytes();
    let mut elements = Vec::new();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape_next = false;
    let mut elem_start = 0;
    let mut found_content = false;

    for (i, &b) in bytes.iter().enumerate() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if b == b'\\' && in_string {
            escape_next = true;
            continue;
        }
        if b == b'"' {
            in_string = !in_string;
            found_content = true;
            continue;
        }
        if in_string {
            continue;
        }
        match b {
            b'{' | b'[' => {
                depth += 1;
                found_content = true;
            }
            b'}' | b']' => {
                depth -= 1;
                found_content = true;
            }
            b',' if depth == 0 => {
                if found_content {
                    elements.push(&inner[elem_start..i]);
                }
                elem_start = i + 1;
                found_content = false;
            }
            b if !b.is_ascii_whitespace() => {
                found_content = true;
            }
            _ => {}
        }
    }

    // Last element
    if found_content {
        elements.push(&inner[elem_start..]);
    }

    if elements.is_empty() {
        None
    } else {
        Some(elements)
    }
}

/// Line-based head/tail compression for large outputs.
fn compress_lines(lines: &[&str], raw: &str) -> String {
    let total = lines.len();
    let head = MAX_LINES / 2; // 10
    let tail = MAX_LINES - head; // 10

    let mut out = String::with_capacity(raw.len() / 2);
    for line in &lines[..head] {
        out.push_str(line);
        out.push('\n');
    }

    let hidden = total - head - tail;
    out.push_str(&format!("… ({hidden} lines omitted) …\n"));

    for (i, line) in lines[total - tail..].iter().enumerate() {
        out.push_str(line);
        if i < tail - 1 {
            out.push('\n');
        }
    }

    maybe_add_hint(out, raw)
}

/// Append a tee recovery hint if available.
fn maybe_add_hint(mut output: String, raw: &str) -> String {
    if let Some(hint) = crate::core::tee::force_tee_tail_hint(raw, "jq", 1) {
        output.push('\n');
        output.push_str(&hint);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_small_json_passthrough() {
        let raw = r#"{"name": "test", "version": "1.0"}"#;
        let result = filter_jq_json(raw);
        assert_eq!(result, raw);
    }

    #[test]
    fn test_empty_output() {
        assert_eq!(filter_jq_json(""), "");
    }

    #[test]
    fn test_small_array_passthrough() {
        let raw = "[\n  1,\n  2,\n  3\n]";
        let result = filter_jq_json(raw);
        assert_eq!(result, raw);
    }

    #[test]
    fn test_large_array_truncation() {
        // Build a 30-element array
        let elements: Vec<String> = (1..=30).map(|i| format!("  {i}")).collect();
        let raw = format!("[\n{}\n]", elements.join(",\n"));

        let result = filter_jq_json(&raw);
        assert!(result.contains("… (22 more elements omitted) …"));
        // Head: 1..5, Tail: 28..30
        assert!(result.contains("1"));
        assert!(result.contains("5"));
        assert!(result.contains("28"));
        assert!(result.contains("30"));
        // Verify savings
        assert!(
            result.len() < raw.len(),
            "filtered ({}) should be shorter than raw ({})",
            result.len(),
            raw.len()
        );
    }

    #[test]
    fn test_large_object_array_truncation() {
        // Build a 25-element array of objects
        let elements: Vec<String> = (1..=25)
            .map(|i| {
                format!(
                    "  {{\n    \"id\": {i},\n    \"name\": \"item_{i}\",\n    \"value\": {}\n  }}",
                    i * 100
                )
            })
            .collect();
        let raw = format!("[\n{}\n]", elements.join(",\n"));

        let result = filter_jq_json(&raw);
        assert!(result.contains("… (17 more elements omitted) …"));
        assert!(result.contains("\"id\": 1"));
        assert!(result.contains("\"id\": 25"));
    }

    #[test]
    fn test_large_lines_fallback() {
        // Build a large non-array output (e.g., deeply nested object)
        let lines: Vec<String> = (0..50)
            .map(|i| format!("  \"key_{i}\": \"value_{i}\""))
            .collect();
        let raw = format!("{{\n{}\n}}", lines.join(",\n"));

        let result = filter_jq_json(&raw);
        assert!(result.contains("… ("));
        assert!(result.contains("lines omitted) …"));
    }

    #[test]
    fn test_raw_mode_small_passthrough() {
        let raw = "line1\nline2\nline3\n";
        let result = filter_jq_raw(raw);
        assert_eq!(result, raw);
    }

    #[test]
    fn test_raw_mode_truncation() {
        let lines: Vec<String> = (1..=50).map(|i| format!("path/to/key_{i}")).collect();
        let raw = lines.join("\n");

        let result = filter_jq_raw(&raw);
        assert!(result.contains("… (30 lines omitted) …"));
        assert!(result.contains("path/to/key_1"));
        assert!(result.contains("path/to/key_15"));
        assert!(result.contains("path/to/key_50"));
    }

    #[test]
    fn test_split_elements_simple() {
        let json = "[1, 2, 3]";
        let elems = split_top_level_elements(json).unwrap();
        assert_eq!(elems.len(), 3);
    }

    #[test]
    fn test_split_elements_objects() {
        let json = r#"[{"a":1}, {"b":2}]"#;
        let elems = split_top_level_elements(json).unwrap();
        assert_eq!(elems.len(), 2);
    }

    #[test]
    fn test_split_elements_nested_arrays() {
        let json = "[[1,2], [3,4], [5]]";
        let elems = split_top_level_elements(json).unwrap();
        assert_eq!(elems.len(), 3);
    }

    #[test]
    fn test_split_elements_strings_with_commas() {
        let json = r#"["hello, world", "foo, bar"]"#;
        let elems = split_top_level_elements(json).unwrap();
        assert_eq!(elems.len(), 2);
    }

    #[test]
    fn test_split_elements_empty_array() {
        let json = "[]";
        let elems = split_top_level_elements(json);
        assert!(elems.is_none());
    }

    #[test]
    fn test_not_json_array() {
        let json = r#"{"key": "value"}"#;
        let elems = split_top_level_elements(json);
        assert!(elems.is_none());
    }

    #[test]
    fn test_savings_at_least_twenty_percent() {
        // Build a realistic 100-element array (like `jq '.paths | keys[]'` on an OpenAPI spec)
        let elements: Vec<String> = (1..=100)
            .map(|i| format!("  \"/api/v1/endpoint_{i}/action\""))
            .collect();
        let raw = format!("[\n{}\n]", elements.join(",\n"));

        let result = filter_jq_json(&raw);
        let savings = 1.0 - (result.len() as f64 / raw.len() as f64);
        assert!(
            savings >= 0.20,
            "savings {:.0}% < 20% (raw={}, filtered={})",
            savings * 100.0,
            raw.len(),
            result.len()
        );
    }

    #[test]
    fn test_raw_savings_at_least_twenty_percent() {
        let lines: Vec<String> = (1..=100)
            .map(|i| format!("/api/v1/endpoint_{i}/action"))
            .collect();
        let raw = lines.join("\n");

        let result = filter_jq_raw(&raw);
        let savings = 1.0 - (result.len() as f64 / raw.len() as f64);
        assert!(
            savings >= 0.20,
            "raw-mode savings {:.0}% < 20% (raw={}, filtered={})",
            savings * 100.0,
            raw.len(),
            result.len()
        );
    }
}
