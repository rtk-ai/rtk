//! Compact filter for `jq` — truncates large JSON results with structural awareness.
//!
//! Compression strategies:
//! - Small results (≤MAX_LINES lines and ≤MAX_INLINE_CHARS bytes): pass through verbatim
//! - Large arrays: first_N + "… (X more elements omitted) …" + last_N
//! - Large raw text (`-r`/`--raw-output`/`-j`): line-based head/tail
//! - Large objects, JSON streams, or compact single-line payloads: head/tail with tee recovery
//!
//! Expected savings: 40–70% on large JSON outputs.

use crate::core::args_utils;
use crate::core::runner::{self, RunOptions};
use crate::core::shell::display_args;
use crate::core::truncate::CAP_LIST;
use crate::core::utils::{ChildArgExt, resolved_command};
use anyhow::Result;

/// Maximum lines before truncation kicks in.
const MAX_LINES: usize = CAP_LIST; // 20

/// Maximum byte length for compact (`jq -c`) or wide outputs within `MAX_LINES`.
const MAX_INLINE_CHARS: usize = 4_000;
const INLINE_HEAD_CHARS: usize = 1_500;
const INLINE_TAIL_CHARS: usize = 500;

/// How many items to show at the head and tail of a truncated array.
const ARRAY_HEAD: usize = 5;
const ARRAY_TAIL: usize = 3;

/// Maximum lines for raw-output mode head/tail.
const RAW_HEAD: usize = 15;
const RAW_TAIL: usize = 5;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let args = &args_utils::restore_double_dash(args);
    let mut cmd = resolved_command("jq");
    cmd.child_args(args);

    if verbose > 0 {
        eprintln!("Running: jq {}", display_args(args));
    }

    let is_raw = has_raw_output_flag(args);

    // jq commonly reads from stdin (piped JSON) or from a file argument.
    // Inherit stdin so piped usage works (`curl ... | rtk jq '.paths'`), and
    // leave failed runs (syntax errors, `-e` false/null exits) unfiltered.
    let opts = RunOptions::stdout_only()
        .inherit_stdin()
        .early_exit_on_failure();

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

/// Detect `-r` / `--raw-output` / `--raw-output0` / `-j` / `--join-output`
/// before `--` or the jq filter expression, skipping option arguments.
fn has_raw_output_flag(args: &[String]) -> bool {
    let mut skip = 0usize;
    for arg in args {
        if skip > 0 {
            skip -= 1;
            continue;
        }
        let s = arg.as_str();
        if s == "--" || s == "--args" || s == "--jsonargs" {
            break;
        }
        if matches!(s, "--arg" | "--argjson" | "--slurpfile" | "--rawfile") {
            skip = 2;
            continue;
        }
        if matches!(s, "-f" | "--from-file" | "-L" | "--indent") {
            skip = 1;
            continue;
        }
        if matches!(s, "--raw-output" | "--raw-output0" | "--join-output") {
            return true;
        }
        if let Some(short) = s.strip_prefix('-').filter(|rest| !rest.starts_with('-')) {
            if short.is_empty() {
                break;
            }
            // `-f<path>` or `-L<dir>` can carry an attached argument; examine only
            // flag letters up to `f` or `L`.
            let flag_prefix = match short.find(['f', 'L']) {
                Some(idx) => {
                    if idx + 1 == short.len() {
                        skip = 1;
                    }
                    &short[..idx]
                }
                None => short,
            };
            if flag_prefix.chars().any(|c| c == 'r' || c == 'j') {
                return true;
            }
        }
    }
    false
}

/// Filter raw-output mode: line-based head/tail truncation (plus inline cap for wide lines).
fn filter_jq_raw(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    let total = lines.len();

    if total <= RAW_HEAD + RAW_TAIL {
        if raw.len() > MAX_INLINE_CHARS {
            return compress_inline_bytes(raw, "jq-raw");
        }
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

    if let Some(hint) = crate::core::tee::force_tee_tail_hint(raw, "jq-raw", RAW_HEAD + 1) {
        out.push('\n');
        out.push_str(&hint);
    }

    out
}

/// Filter JSON-mode output with structural awareness.
///
/// Detects single top-level arrays and applies head/tail element truncation.
/// For other structures (objects, JSON streams, compact single-line JSON),
/// applies line-based or byte-window truncation.
fn filter_jq_json(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let lines: Vec<&str> = trimmed.lines().collect();
    if lines.len() <= MAX_LINES {
        if raw.len() > MAX_INLINE_CHARS {
            return compress_inline_bytes(raw, "jq");
        }
        return raw.to_string();
    }

    // Try to detect and compress a single top-level JSON array.
    if trimmed.starts_with('[')
        && let Some(compressed) = compress_json_array(trimmed)
    {
        return maybe_add_whole_hint(compressed, raw);
    }

    // Fallback: line-based head/tail for large objects or multi-value streams.
    compress_lines(&lines, raw)
}

/// Compress a single top-level JSON array by keeping first_N and last_N elements.
fn compress_json_array(json: &str) -> Option<String> {
    let elements = split_top_level_elements(json)?;
    let count = elements.len();

    if count <= ARRAY_HEAD + ARRAY_TAIL {
        return None;
    }

    let mut out = String::with_capacity(json.len() / 2);
    out.push_str("[\n");

    for elem in elements.iter().take(ARRAY_HEAD) {
        out.push_str("  ");
        out.push_str(elem.trim());
        out.push_str(",\n");
    }

    let hidden = count - ARRAY_HEAD - ARRAY_TAIL;
    out.push_str(&format!("  … ({hidden} more elements omitted) …\n"));

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

/// Split a single top-level JSON array into its elements.
///
/// Returns `None` if `json` is not one balanced JSON array (for instance, a
/// newline-delimited stream of multiple arrays `[1, 2]\n[3, 4]`, where the
/// outer `[` closes before the end of the buffer).
fn split_top_level_elements(json: &str) -> Option<Vec<&str>> {
    let trimmed = json.trim();
    if !trimmed.starts_with('[') || !trimmed.ends_with(']') {
        return None;
    }

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
                if depth < 0 {
                    return None;
                }
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

    if depth != 0 || in_string {
        return None;
    }

    if found_content {
        elements.push(&inner[elem_start..]);
    }

    if elements.is_empty() {
        None
    } else {
        Some(elements)
    }
}

/// Line-based head/tail compression for large multi-line outputs.
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

    if let Some(hint) = crate::core::tee::force_tee_tail_hint(raw, "jq", head + 1) {
        out.push('\n');
        out.push_str(&hint);
    }

    out
}

/// Byte-window head/tail compression for compact single-line outputs (`jq -c`).
fn compress_inline_bytes(raw: &str, slug: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() <= MAX_INLINE_CHARS {
        return raw.to_string();
    }
    let head_end = trimmed.floor_char_boundary(INLINE_HEAD_CHARS);
    let tail_start = trimmed.ceil_char_boundary(trimmed.len() - INLINE_TAIL_CHARS);
    if head_end >= tail_start {
        return raw.to_string();
    }
    let omitted = tail_start - head_end;
    let mut out = format!(
        "{} … ({omitted} bytes omitted) … {}",
        &trimmed[..head_end],
        &trimmed[tail_start..]
    );
    if let Some(hint) = crate::core::tee::force_tee_hint(raw, slug) {
        out.push('\n');
        out.push_str(&hint);
    }
    out
}

/// Append a whole-output tee recovery hint for element-truncated arrays.
fn maybe_add_whole_hint(mut output: String, raw: &str) -> String {
    if let Some(hint) = crate::core::tee::force_tee_hint(raw, "jq") {
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
        let elements: Vec<String> = (1..=30).map(|i| format!("  {i}")).collect();
        let raw = format!("[\n{}\n]", elements.join(",\n"));

        let result = filter_jq_json(&raw);
        assert!(result.contains("… (22 more elements omitted) …"));
        assert!(result.contains("  5,\n"));
        assert!(result.contains("1"));
        assert!(result.contains("28"));
        assert!(result.contains("30"));
        assert!(
            result.len() < raw.len(),
            "filtered ({}) should be shorter than raw ({})",
            result.len(),
            raw.len()
        );
    }

    #[test]
    fn test_large_object_array_truncation() {
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
    fn test_multi_array_stream_is_not_merged_into_single_array() {
        let arr1: Vec<String> = (1..=15).map(|i| format!("  {i}")).collect();
        let arr2: Vec<String> = (16..=30).map(|i| format!("  {i}")).collect();
        let raw = format!("[\n{}\n]\n[\n{}\n]", arr1.join(",\n"), arr2.join(",\n"));

        assert!(split_top_level_elements(&raw).is_none());
        let result = filter_jq_json(&raw);
        assert!(result.contains("lines omitted) …"));
        assert!(!result.contains("more elements omitted"));
    }

    #[test]
    fn test_compact_single_line_json_truncation() {
        let items: Vec<String> = (1..=300)
            .map(|i| format!("\"/api/v1/resources/{i}/details\""))
            .collect();
        let raw = format!("[{}]\n", items.join(","));
        assert!(raw.len() > MAX_INLINE_CHARS);

        let result = filter_jq_json(&raw);
        assert!(result.contains("bytes omitted) …"));
        assert!(result.len() < raw.len() / 2);
    }

    #[test]
    fn test_raw_flag_detection() {
        let vec_str = |items: &[&str]| items.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        assert!(has_raw_output_flag(&vec_str(&["-r", ".foo", "a.json"])));
        assert!(has_raw_output_flag(&vec_str(&["-cr", ".foo", "a.json"])));
        assert!(has_raw_output_flag(&vec_str(&["-rn", ".foo"])));
        assert!(has_raw_output_flag(&vec_str(&["--raw-output", ".foo"])));
        assert!(has_raw_output_flag(&vec_str(&["--join-output", ".foo"])));
        assert!(!has_raw_output_flag(&vec_str(&[
            "--arg", "x", "-r", ".foo", "a.json"
        ])));
        assert!(!has_raw_output_flag(&vec_str(&[".foo", "--", "-r.json"])));
    }

    #[test]
    fn test_large_lines_fallback() {
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
        let elems = split_top_level_elements(json).expect("valid array");
        assert_eq!(elems.len(), 3);
    }

    #[test]
    fn test_split_elements_objects() {
        let json = r#"[{"a":1}, {"b":2}]"#;
        let elems = split_top_level_elements(json).expect("valid array");
        assert_eq!(elems.len(), 2);
    }

    #[test]
    fn test_split_elements_nested_arrays() {
        let json = "[[1,2], [3,4], [5]]";
        let elems = split_top_level_elements(json).expect("valid array");
        assert_eq!(elems.len(), 3);
    }

    #[test]
    fn test_split_elements_strings_with_commas() {
        let json = r#"["hello, world", "foo, bar"]"#;
        let elems = split_top_level_elements(json).expect("valid array");
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
