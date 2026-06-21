//! Inspects JSON structure without showing values, saving tokens on large payloads.

use crate::core::guard::never_worse;
use crate::core::tracking;
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::borrow::Cow;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

/// Reject non-JSON files with a clear error before doing any I/O.
fn validate_json_extension(file: &Path) -> Result<()> {
    if let Some(ext) = file.extension().and_then(|e| e.to_str()) {
        let format_name = match ext {
            "toml" => Some("TOML"),
            "yaml" | "yml" => Some("YAML"),
            "xml" => Some("XML"),
            "csv" => Some("CSV"),
            "ini" => Some("INI"),
            "env" => Some("env"),
            "txt" => Some("plain text"),
            _ => None,
        };
        if let Some(fmt) = format_name {
            let mut msg = format!(
                "{} is not a JSON file (detected {}). Use `rtk read` for non-JSON files.",
                file.display(),
                fmt
            );
            if ext == "toml" && file.file_name().is_some_and(|n| n == "Cargo.toml") {
                msg.push_str(" Tip: use `rtk deps` for Cargo.toml.");
            }
            bail!("{}", msg);
        }
    }
    Ok(())
}

/// Show JSON (compact with values by default, or keys-only with --keys-only)
pub fn run(file: &Path, max_depth: usize, schema_only: bool, verbose: u8) -> Result<()> {
    validate_json_extension(file)?;
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Analyzing JSON: {}", file.display());
    }

    let content = fs::read_to_string(file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let output = if schema_only {
        filter_json_string(&content, max_depth)?
    } else {
        filter_json_compact(&content, max_depth)?
    };
    let shown = never_worse(&content, &output);
    println!("{}", shown);
    timer.track(
        &format!("cat {}", file.display()),
        "rtk json",
        &content,
        shown,
    );
    Ok(())
}

/// Show JSON from stdin
pub fn run_stdin(max_depth: usize, schema_only: bool, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Analyzing JSON from stdin");
    }

    let mut content = String::new();
    io::stdin()
        .lock()
        .read_to_string(&mut content)
        .context("Failed to read from stdin")?;

    let output = if schema_only {
        filter_json_string(&content, max_depth)?
    } else {
        filter_json_compact(&content, max_depth)?
    };
    let shown = never_worse(&content, &output);
    println!("{}", shown);
    timer.track("cat - (stdin)", "rtk json -", &content, shown);
    Ok(())
}

/// Parse a JSON string and return compact representation with values preserved.
/// Long strings are truncated, arrays are summarized.
pub fn filter_json_compact(json_str: &str, max_depth: usize) -> Result<String> {
    let value = parse_json_lenient(json_str)?;
    Ok(compact_json(&value, 0, max_depth))
}

/// Parse JSON, tolerating raw (unescaped) control characters inside strings.
///
/// serde_json correctly rejects U+0000–U+001F appearing literally inside a
/// string (RFC 8259 §7 requires them escaped). Some real-world producers emit
/// them anyway — e.g. an API echoing a user-supplied newline verbatim into a
/// field. Strict parsing then fails and `rtk json` prints *nothing*, losing the
/// whole payload and forcing the user to re-fetch with a raw passthrough. To
/// degrade gracefully we retry once with those control characters escaped to
/// their equivalent `\uXXXX` form. Valid input takes the fast path untouched,
/// and genuinely malformed input still surfaces the original strict error.
fn parse_json_lenient(json_str: &str) -> Result<Value> {
    match serde_json::from_str::<Value>(json_str) {
        Ok(value) => Ok(value),
        Err(strict_err) => {
            // Only worth retrying if escaping actually changed something.
            if let Cow::Owned(sanitized) = escape_raw_control_chars(json_str) {
                if let Ok(value) = serde_json::from_str::<Value>(&sanitized) {
                    return Ok(value);
                }
            }
            Err(strict_err).context("Failed to parse JSON")
        }
    }
}

/// Escape raw control characters (U+0000–U+001F) that appear *inside* JSON
/// string literals, leaving everything else — including the insignificant
/// whitespace between tokens — byte-for-byte identical. Returns
/// `Cow::Borrowed` when there is nothing to escape so the common valid-JSON
/// path never allocates.
fn escape_raw_control_chars(input: &str) -> Cow<'_, str> {
    // Fast path: no control bytes at all means nothing to escape.
    if !input.bytes().any(|b| b < 0x20) {
        return Cow::Borrowed(input);
    }

    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut prev_backslash = false;
    let mut changed = false;

    for ch in input.chars() {
        if in_string {
            if prev_backslash {
                // This char is part of an escape sequence (e.g. \n, \"); emit verbatim.
                out.push(ch);
                prev_backslash = false;
            } else if ch == '\\' {
                out.push(ch);
                prev_backslash = true;
            } else if ch == '"' {
                out.push(ch);
                in_string = false;
            } else if (ch as u32) < 0x20 {
                // Raw control char inside a string: rewrite to its \uXXXX escape.
                out.push_str(&format!("\\u{:04x}", ch as u32));
                changed = true;
            } else {
                out.push(ch);
            }
        } else {
            if ch == '"' {
                in_string = true;
            }
            // Control chars outside strings are either valid JSON whitespace or
            // a structural error we cannot fix here — pass them through unchanged.
            out.push(ch);
        }
    }

    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(input)
    }
}

fn compact_json(value: &Value, depth: usize, max_depth: usize) -> String {
    let indent = "  ".repeat(depth);

    if depth > max_depth {
        return format!("{}...", indent);
    }

    match value {
        Value::Null => format!("{}null", indent),
        Value::Bool(b) => format!("{}{}", indent, b),
        Value::Number(n) => format!("{}{}", indent, n),
        Value::String(s) => {
            if s.len() > 80 {
                let end = s.floor_char_boundary(77);
                format!("{}\"{}...\"", indent, &s[..end])
            } else {
                format!("{}\"{}\"", indent, s)
            }
        }
        Value::Array(arr) => {
            if arr.is_empty() {
                format!("{}[]", indent)
            } else if arr.len() > 5 {
                let first = compact_json(&arr[0], depth + 1, max_depth);
                format!("{}[{}, ... +{} more]", indent, first.trim(), arr.len() - 1)
            } else {
                let items: Vec<String> = arr
                    .iter()
                    .map(|v| compact_json(v, depth + 1, max_depth))
                    .collect();
                let all_simple = arr.iter().all(|v| {
                    matches!(
                        v,
                        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
                    )
                });
                if all_simple {
                    let inline: Vec<&str> = items.iter().map(|s| s.trim()).collect();
                    format!("{}[{}]", indent, inline.join(", "))
                } else {
                    let mut lines = vec![format!("{}[", indent)];
                    for item in &items {
                        lines.push(format!("{},", item));
                    }
                    lines.push(format!("{}]", indent));
                    lines.join("\n")
                }
            }
        }
        Value::Object(map) => {
            if map.is_empty() {
                format!("{}{{}}", indent)
            } else {
                let mut lines = vec![format!("{}{{", indent)];
                let mut keys: Vec<_> = map.keys().collect();
                keys.sort();

                for (i, key) in keys.iter().enumerate() {
                    let val = &map[*key];
                    let is_simple = matches!(
                        val,
                        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
                    );

                    if is_simple {
                        let val_str = compact_json(val, 0, max_depth);
                        lines.push(format!("{}  {}: {}", indent, key, val_str.trim()));
                    } else {
                        lines.push(format!("{}  {}:", indent, key));
                        lines.push(compact_json(val, depth + 1, max_depth));
                    }

                    if i >= 20 {
                        lines.push(format!("{}  ... +{} more keys", indent, keys.len() - i - 1));
                        break;
                    }
                }
                lines.push(format!("{}}}", indent));
                lines.join("\n")
            }
        }
    }
}

/// Parse a JSON string and return its schema representation (types only, no values).
/// Useful for piping JSON from other commands (e.g., `gh api`, `curl`).
pub fn filter_json_string(json_str: &str, max_depth: usize) -> Result<String> {
    let value = parse_json_lenient(json_str)?;
    Ok(extract_schema(&value, 0, max_depth))
}

fn extract_schema(value: &Value, depth: usize, max_depth: usize) -> String {
    let indent = "  ".repeat(depth);

    if depth > max_depth {
        return format!("{}...", indent);
    }

    match value {
        Value::Null => format!("{}null", indent),
        Value::Bool(_) => format!("{}bool", indent),
        Value::Number(n) => {
            if n.is_i64() {
                format!("{}int", indent)
            } else {
                format!("{}float", indent)
            }
        }
        Value::String(s) => {
            if s.len() > 50 {
                format!("{}string[{}]", indent, s.len())
            } else if s.is_empty() {
                format!("{}string", indent)
            } else {
                // Check if it looks like a URL, date, etc.
                if s.starts_with("http") {
                    format!("{}url", indent)
                } else if s.contains('-') && s.len() == 10 {
                    format!("{}date?", indent)
                } else {
                    format!("{}string", indent)
                }
            }
        }
        Value::Array(arr) => {
            if arr.is_empty() {
                format!("{}[]", indent)
            } else {
                let first_schema = extract_schema(&arr[0], depth + 1, max_depth);
                let trimmed = first_schema.trim();
                if arr.len() == 1 {
                    format!("{}[\n{}\n{}]", indent, first_schema, indent)
                } else {
                    format!("{}[{}] ({})", indent, trimmed, arr.len())
                }
            }
        }
        Value::Object(map) => {
            if map.is_empty() {
                format!("{}{{}}", indent)
            } else {
                let mut lines = vec![format!("{}{{", indent)];
                let mut keys: Vec<_> = map.keys().collect();
                keys.sort();

                for (i, key) in keys.iter().enumerate() {
                    let val = &map[*key];
                    let val_schema = extract_schema(val, depth + 1, max_depth);
                    let val_trimmed = val_schema.trim();

                    // Inline simple types
                    let is_simple = matches!(
                        val,
                        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
                    );

                    if is_simple {
                        if i < keys.len() - 1 {
                            lines.push(format!("{}  {}: {},", indent, key, val_trimmed));
                        } else {
                            lines.push(format!("{}  {}: {}", indent, key, val_trimmed));
                        }
                    } else {
                        lines.push(format!("{}  {}:", indent, key));
                        lines.push(val_schema);
                    }

                    // Limit keys shown
                    if i >= 15 {
                        lines.push(format!("{}  ... +{} more keys", indent, keys.len() - i - 1));
                        break;
                    }
                }
                lines.push(format!("{}}}", indent));
                lines.join("\n")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- #347: validate_json_extension ---

    #[test]
    fn test_toml_file_rejected() {
        let err = validate_json_extension(Path::new("config.toml")).unwrap_err();
        assert!(err.to_string().contains("not a JSON file"));
        assert!(err.to_string().contains("TOML"));
    }

    #[test]
    fn test_cargo_toml_suggests_deps() {
        let err = validate_json_extension(Path::new("Cargo.toml")).unwrap_err();
        assert!(err.to_string().contains("rtk deps"));
    }

    #[test]
    fn test_yaml_file_rejected() {
        let err = validate_json_extension(Path::new("config.yaml")).unwrap_err();
        assert!(err.to_string().contains("YAML"));
    }

    #[test]
    fn test_json_file_accepted() {
        assert!(validate_json_extension(Path::new("data.json")).is_ok());
    }

    #[test]
    fn test_unknown_extension_accepted() {
        assert!(validate_json_extension(Path::new("data.xyz")).is_ok());
    }

    #[test]
    fn test_no_extension_accepted() {
        assert!(validate_json_extension(Path::new("Makefile")).is_ok());
    }

    #[test]
    fn test_extract_schema_simple() {
        let json: Value = serde_json::from_str(r#"{"name": "test", "count": 42}"#).unwrap();
        let schema = extract_schema(&json, 0, 5);
        assert!(schema.contains("name"));
        assert!(schema.contains("string"));
        assert!(schema.contains("int"));
    }

    #[test]
    fn test_extract_schema_array() {
        let json: Value = serde_json::from_str(r#"{"items": [1, 2, 3]}"#).unwrap();
        let schema = extract_schema(&json, 0, 5);
        assert!(schema.contains("items"));
        assert!(schema.contains("(3)"));
    }

    fn assert_value_truncated(payload: &str) {
        let json = format!(r#"{{"key": "{}"}}"#, payload);
        let output = filter_json_compact(&json, 5)
            .expect("filter_json_compact must not error on valid JSON");

        assert!(output.contains("key"));
        assert!(
            output.contains("..."),
            "long string should be truncated, got: {output}"
        );

        let value = output
            .split('"')
            .nth(1)
            .expect("output should contain a quoted string value");
        assert!(
            value.len() <= 80,
            "truncated value is {} bytes: {value}",
            value.len()
        );
    }

    #[test]
    fn test_compact_truncates_pure_multibyte_string() {
        assert_value_truncated(&"日本語テスト".repeat(85));
    }

    #[test]
    fn test_compact_truncates_mixed_ascii_multibyte_string() {
        assert_value_truncated(&("a".repeat(76) + &"日本語".repeat(5)));
    }

    // --- graceful recovery from raw control characters inside strings ---

    #[test]
    fn test_compact_recovers_raw_control_char() {
        // Real newline + tab inside a string value — strict serde_json rejects
        // these, but rtk should still render the payload instead of printing
        // nothing.
        let json = "{\"body\":\"line1\nline2\ttab\"}";
        let out = filter_json_compact(json, 5)
            .expect("control chars inside strings must not abort the render");
        assert!(out.contains("body"), "got: {out}");
    }

    #[test]
    fn test_schema_recovers_raw_control_char() {
        let json = "{\"msg\":\"a\nb\"}";
        let out = filter_json_string(json, 5)
            .expect("control chars inside strings must not abort the schema");
        assert!(out.contains("msg"), "got: {out}");
    }

    #[test]
    fn test_raw_control_char_in_key_recovered() {
        // Control chars are illegal in keys too; the same string-aware pass fixes them.
        let json = "{\"a\nb\":1}";
        let out = filter_json_compact(json, 5).expect("control char in key must recover");
        assert!(out.contains("a") && out.contains("1"), "got: {out}");
    }

    #[test]
    fn test_valid_json_unaffected_by_lenient_parse() {
        let json = r#"{"name":"test","n":42,"ok":true}"#;
        let strict: Value = serde_json::from_str(json).unwrap();
        assert_eq!(parse_json_lenient(json).unwrap(), strict);
    }

    #[test]
    fn test_malformed_json_still_errors() {
        // A structural error (not a control char) must still fail loudly.
        let err = filter_json_compact("{not valid", 5).unwrap_err();
        assert!(err.to_string().contains("Failed to parse JSON"));
    }

    #[test]
    fn test_escape_fast_path_borrows_clean_input() {
        // Pretty-printed JSON has newlines *between* tokens (valid whitespace)
        // but none inside strings — and serde parses it fine, so escaping is
        // never even invoked. Here we assert the escaper itself leaves any
        // control-free input borrowed.
        assert!(matches!(
            escape_raw_control_chars(r#"{"a":1}"#),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn test_escape_leaves_whitespace_between_tokens() {
        // Newlines outside strings are valid JSON whitespace and must survive
        // unchanged; only in-string control chars get rewritten.
        let pretty = "{\n  \"a\": 1\n}";
        assert!(matches!(escape_raw_control_chars(pretty), Cow::Borrowed(_)));
    }

    #[test]
    fn test_escape_preserves_existing_backslash_escapes() {
        // An already-escaped \n must not be double-processed.
        let json = r#"{"a":"x\ny"}"#;
        assert!(matches!(escape_raw_control_chars(json), Cow::Borrowed(_)));
        // And it still parses to the real newline value.
        let v = parse_json_lenient(json).unwrap();
        assert_eq!(v["a"], "x\ny");
    }

    #[test]
    fn test_escape_rewrites_only_in_string_control() {
        let json = "{\"a\":\"b\tc\"}";
        match escape_raw_control_chars(json) {
            Cow::Owned(s) => {
                assert!(s.contains("\\u0009"), "tab should be escaped: {s}");
                assert!(!s.contains('\t'), "no raw tab should remain: {s:?}");
            }
            Cow::Borrowed(_) => panic!("expected rewrite for in-string control char"),
        }
    }
}
