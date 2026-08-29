//! JSON → TOON (Token-Oriented Object Notation) encoder.
//!
//! Implements the TOON v3.3 spec for lossless, token-efficient encoding of JSON
//! data. TOON saves 30–60% of tokens compared to JSON by:
//! - Declaring array schemas once (length + field names) instead of repeating keys
//! - Using indentation instead of braces/brackets
//! - Minimal quoting (only when values contain structural characters)
//!
//! See <https://github.com/toon-format/spec> for the full specification.

use anyhow::{Context, Result};
use serde_json::Value;

/// Parse a JSON string and encode it as TOON.
pub fn json_str_to_toon(json_str: &str) -> Result<String> {
    let value: Value = serde_json::from_str(json_str).context("Failed to parse JSON")?;
    Ok(json_to_toon(&value))
}

/// Encode a `serde_json::Value` as TOON text.
pub fn json_to_toon(value: &Value) -> String {
    let mut buf = String::new();
    encode_root(value, &mut buf);
    // Trim trailing newline for consistency with other rtk output
    while buf.ends_with('\n') {
        buf.pop();
    }
    buf
}

// ---------------------------------------------------------------------------
// Root encoding — determines the top-level form (object, array, or primitive)
// ---------------------------------------------------------------------------

fn encode_root(value: &Value, buf: &mut String) {
    match value {
        Value::Object(map) => {
            if map.is_empty() {
                buf.push_str("{}");
                return;
            }
            encode_object_fields(map, 0, buf);
        }
        Value::Array(arr) => {
            encode_root_array(arr, buf);
        }
        _ => {
            // Single primitive at root
            buf.push_str(&format_primitive(value));
        }
    }
}

// ---------------------------------------------------------------------------
// Object encoding — key: value with indentation
// ---------------------------------------------------------------------------

fn encode_object_fields(map: &serde_json::Map<String, Value>, depth: usize, buf: &mut String) {
    let indent = "  ".repeat(depth);

    for (key, value) in map.iter() {
        let key_str = format_key(key);
        match value {
            Value::Object(inner) => {
                if inner.is_empty() {
                    buf.push_str(&format!("{}{}: {{}}\n", indent, key_str));
                } else {
                    buf.push_str(&format!("{}{}:\n", indent, key_str));
                    encode_object_fields(inner, depth + 1, buf);
                }
            }
            Value::Array(arr) => {
                encode_keyed_array(&key_str, arr, depth, buf);
            }
            _ => {
                buf.push_str(&format!(
                    "{}{}: {}\n",
                    indent,
                    key_str,
                    format_primitive(value)
                ));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Array encoding — tabular, inline primitive, or expanded list items
// ---------------------------------------------------------------------------

/// Encode a root-level array (no key prefix).
fn encode_root_array(arr: &[Value], buf: &mut String) {
    if arr.is_empty() {
        buf.push_str("[]");
        return;
    }

    // Check for tabular (uniform array of objects with primitive-only values)
    if let Some(fields) = uniform_object_fields(arr) {
        // Tabular: [N]{f1,f2,...}:
        let field_headers: Vec<String> = fields.iter().map(|f| format_key(f)).collect();
        buf.push_str(&format!(
            "[{}]{{{}}}:\n",
            arr.len(),
            field_headers.join(",")
        ));
        for item in arr {
            if let Value::Object(map) = item {
                buf.push_str("  ");
                let cells: Vec<String> = fields
                    .iter()
                    .map(|f| format_cell(map.get(f).unwrap_or(&Value::Null)))
                    .collect();
                buf.push_str(&cells.join(","));
                buf.push('\n');
            }
        }
        return;
    }

    // Check for all-primitive array → inline
    if arr.iter().all(is_primitive) {
        let cells: Vec<String> = arr.iter().map(format_cell).collect();
        buf.push_str(&format!("[{}]: {}\n", arr.len(), cells.join(",")));
        return;
    }

    // Expanded list items
    buf.push_str(&format!("[{}]:\n", arr.len()));
    for item in arr {
        encode_list_item(item, 1, buf);
    }
}

/// Encode an array with a key prefix at the given depth.
fn encode_keyed_array(key: &str, arr: &[Value], depth: usize, buf: &mut String) {
    let indent = "  ".repeat(depth);

    if arr.is_empty() {
        buf.push_str(&format!("{}{}[0]:\n", indent, key));
        return;
    }

    // Check for tabular (uniform array of objects with primitive-only values)
    if let Some(fields) = uniform_object_fields(arr) {
        let field_headers: Vec<String> = fields.iter().map(|f| format_key(f)).collect();
        buf.push_str(&format!(
            "{}{}[{}]{{{}}}:\n",
            indent,
            key,
            arr.len(),
            field_headers.join(",")
        ));
        let row_indent = "  ".repeat(depth + 1);
        for item in arr {
            if let Value::Object(map) = item {
                buf.push_str(&row_indent);
                let cells: Vec<String> = fields
                    .iter()
                    .map(|f| format_cell(map.get(f).unwrap_or(&Value::Null)))
                    .collect();
                buf.push_str(&cells.join(","));
                buf.push('\n');
            }
        }
        return;
    }

    // Check for all-primitive array → inline
    if arr.iter().all(is_primitive) {
        let cells: Vec<String> = arr.iter().map(format_cell).collect();
        buf.push_str(&format!(
            "{}{}[{}]: {}\n",
            indent,
            key,
            arr.len(),
            cells.join(",")
        ));
        return;
    }

    // Expanded list items
    buf.push_str(&format!("{}{}[{}]:\n", indent, key, arr.len()));
    for item in arr {
        encode_list_item(item, depth + 1, buf);
    }
}

/// Encode a single list item (prefixed with "- ").
fn encode_list_item(value: &Value, depth: usize, buf: &mut String) {
    let indent = "  ".repeat(depth);

    match value {
        Value::Object(map) => {
            if map.is_empty() {
                buf.push_str(&format!("{}-\n", indent));
            } else {
                // First field on the "- " line, rest indented under it
                let mut iter = map.iter();
                if let Some((first_key, first_val)) = iter.next() {
                    let key_str = format_key(first_key);
                    match first_val {
                        Value::Object(inner) => {
                            if inner.is_empty() {
                                buf.push_str(&format!("{}- {}: {{}}\n", indent, key_str));
                            } else {
                                buf.push_str(&format!("{}- {}:\n", indent, key_str));
                                encode_object_fields(inner, depth + 2, buf);
                            }
                        }
                        Value::Array(arr) => {
                            // The "- " takes the key; array content indented
                            buf.push_str(&format!("{}- ", indent));
                            // Temporarily build the array line without leading indent
                            let mut arr_buf = String::new();
                            encode_keyed_array(&key_str, arr, 0, &mut arr_buf);
                            buf.push_str(arr_buf.trim_start());
                            // Re-indent nested content if needed
                        }
                        _ => {
                            buf.push_str(&format!(
                                "{}- {}: {}\n",
                                indent,
                                key_str,
                                format_primitive(first_val)
                            ));
                        }
                    }

                    // Remaining fields at depth + 1 (aligned under the key after "- ")
                    for (key, val) in iter {
                        let key_str = format_key(key);
                        let field_indent = "  ".repeat(depth + 1);
                        match val {
                            Value::Object(inner) => {
                                if inner.is_empty() {
                                    buf.push_str(&format!("{}{}: {{}}\n", field_indent, key_str));
                                } else {
                                    buf.push_str(&format!("{}{}:\n", field_indent, key_str));
                                    encode_object_fields(inner, depth + 2, buf);
                                }
                            }
                            Value::Array(arr) => {
                                encode_keyed_array(&key_str, arr, depth + 1, buf);
                            }
                            _ => {
                                buf.push_str(&format!(
                                    "{}{}: {}\n",
                                    field_indent,
                                    key_str,
                                    format_primitive(val)
                                ));
                            }
                        }
                    }
                }
            }
        }
        Value::Array(arr) => {
            // Nested array as list item
            if arr.is_empty() {
                buf.push_str(&format!("{}- [0]:\n", indent));
            } else if arr.iter().all(is_primitive) {
                let cells: Vec<String> = arr.iter().map(format_cell).collect();
                buf.push_str(&format!(
                    "{}- [{}]: {}\n",
                    indent,
                    arr.len(),
                    cells.join(",")
                ));
            } else {
                buf.push_str(&format!("{}- [{}]:\n", indent, arr.len()));
                for inner in arr {
                    encode_list_item(inner, depth + 1, buf);
                }
            }
        }
        _ => {
            buf.push_str(&format!("{}- {}\n", indent, format_primitive(value)));
        }
    }
}

// ---------------------------------------------------------------------------
// Tabular detection — checks if an array qualifies for compact tabular form
// ---------------------------------------------------------------------------

/// Returns the ordered field list if all elements are objects with the same keys
/// and all values are primitives (eligible for tabular encoding).
fn uniform_object_fields(arr: &[Value]) -> Option<Vec<String>> {
    if arr.is_empty() {
        return None;
    }

    let first = arr[0].as_object()?;
    if first.is_empty() {
        return None;
    }

    // All values in the first object must be primitives
    if !first.values().all(is_primitive) {
        return None;
    }

    let fields: Vec<String> = first.keys().cloned().collect();

    // Every subsequent element must be an object with the exact same keys
    for item in &arr[1..] {
        let obj = item.as_object()?;
        if obj.len() != fields.len() {
            return None;
        }
        for field in &fields {
            let val = obj.get(field)?;
            if !is_primitive(val) {
                return None;
            }
        }
    }

    Some(fields)
}

// ---------------------------------------------------------------------------
// Primitive formatting
// ---------------------------------------------------------------------------

fn is_primitive(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
}

/// Format a primitive value for standalone display (key: value context).
fn format_primitive(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => format_number(n),
        Value::String(s) => format_string_value(s),
        _ => String::new(), // Objects/arrays handled elsewhere
    }
}

/// Format a primitive value for use in tabular/inline array cells.
/// Applies the same rules as format_primitive but quotes are determined
/// by whether the value contains the active delimiter (comma).
fn format_cell(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => format_number(n),
        Value::String(s) => format_cell_string(s),
        _ => String::new(),
    }
}

/// Format a number in canonical TOON form:
/// - No trailing zeros in fractional part
/// - Integer-valued floats rendered as integers (1.0 → 1)
/// - -0 → 0
fn format_number(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }
    if let Some(u) = n.as_u64() {
        return u.to_string();
    }
    if let Some(f) = n.as_f64() {
        // Handle -0
        if f == 0.0 {
            return "0".to_string();
        }
        // If it's an integer value, emit without decimal
        if f.fract() == 0.0 && f.abs() < 1e15 {
            return format!("{}", f as i64);
        }
        // Use standard formatting and strip trailing zeros
        let s = format!("{}", f);
        return s;
    }
    // Fallback: use the number's Display
    n.to_string()
}

/// Format a string value in key: value context.
/// Only quotes when the string contains structural characters.
fn format_string_value(s: &str) -> String {
    if needs_quoting(s) {
        format!("\"{}\"", escape_string(s))
    } else {
        s.to_string()
    }
}

/// Format a string value as a tabular/inline cell.
/// Additionally quotes if the value contains comma (the active delimiter).
fn format_cell_string(s: &str) -> String {
    if needs_quoting(s) || s.contains(',') {
        format!("\"{}\"", escape_string(s))
    } else {
        s.to_string()
    }
}

/// Format an object key. Quotes only when required by TOON §7.3.
/// Unquoted keys match: ^[A-Za-z_][A-Za-z0-9_.]*$
fn format_key(key: &str) -> String {
    if key.is_empty() || needs_key_quoting(key) {
        format!("\"{}\"", escape_string(key))
    } else {
        key.to_string()
    }
}

/// Check if a string value requires quoting in TOON.
/// A value must be quoted when it:
/// - Is empty
/// - Contains structural chars: colon, newline, tab
/// - Contains quote or backslash
/// - Has leading/trailing whitespace
/// - Looks like a TOON keyword (true, false, null)
/// - Looks like a number
/// - Starts with "- " (list item marker)
/// - Contains "[" or "{" that could be parsed as header syntax
fn needs_quoting(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    if s.starts_with(' ') || s.ends_with(' ') {
        return true;
    }
    if matches!(s, "true" | "false" | "null") {
        return true;
    }
    if s.starts_with("- ") || s == "-" {
        return true;
    }
    // If it looks like a number, quote it so decoder treats it as string
    if looks_like_number(s) {
        return true;
    }
    s.contains(':')
        || s.contains('\n')
        || s.contains('\t')
        || s.contains('"')
        || s.contains('\\')
        || s.contains('[')
        || s.contains('{')
}

/// Check if a key requires quoting.
/// Unquoted keys must match ^[A-Za-z_][A-Za-z0-9_.]*$
fn needs_key_quoting(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return true,
    }
    for c in chars {
        if !(c.is_ascii_alphanumeric() || c == '_' || c == '.') {
            return true;
        }
    }
    false
}

/// Rough check: does this string look like a JSON number?
fn looks_like_number(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let s = s.strip_prefix('-').unwrap_or(s);
    if s.is_empty() {
        return false;
    }
    // Must start with a digit
    let first = s.as_bytes()[0];
    if !first.is_ascii_digit() {
        return false;
    }
    // Check all remaining chars are digits, dots, e, E, +, -
    s.bytes().all(|b| {
        b.is_ascii_digit() || b == b'.' || b == b'e' || b == b'E' || b == b'+' || b == b'-'
    })
}

/// Escape a string per TOON §7.1.
fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_simple_object() {
        let val = json!({"name": "Alice", "age": 30});
        let toon = json_to_toon(&val);
        assert!(toon.contains("name: Alice"));
        assert!(toon.contains("age: 30"));
    }

    #[test]
    fn test_tabular_array() {
        let val = json!({
            "users": [
                {"id": 1, "name": "Alice"},
                {"id": 2, "name": "Bob"}
            ]
        });
        let toon = json_to_toon(&val);
        // Should use tabular form
        assert!(toon.contains("users[2]{id,name}:"), "got: {}", toon);
        assert!(toon.contains("1,Alice"), "got: {}", toon);
        assert!(toon.contains("2,Bob"), "got: {}", toon);
    }

    #[test]
    fn test_primitive_array() {
        let val = json!({"tags": ["rust", "cli", "llm"]});
        let toon = json_to_toon(&val);
        assert!(toon.contains("tags[3]: rust,cli,llm"), "got: {}", toon);
    }

    #[test]
    fn test_nested_object() {
        let val = json!({
            "config": {
                "debug": true,
                "database": {
                    "host": "localhost",
                    "port": 5432
                }
            }
        });
        let toon = json_to_toon(&val);
        assert!(toon.contains("config:"), "got: {}", toon);
        assert!(toon.contains("  debug: true"), "got: {}", toon);
        assert!(toon.contains("  database:"), "got: {}", toon);
        assert!(toon.contains("    host: localhost"), "got: {}", toon);
        assert!(toon.contains("    port: 5432"), "got: {}", toon);
    }

    #[test]
    fn test_empty_object() {
        let val = json!({});
        assert_eq!(json_to_toon(&val), "{}");
    }

    #[test]
    fn test_empty_array() {
        let val = json!([]);
        assert_eq!(json_to_toon(&val), "[]");
    }

    #[test]
    fn test_null_value() {
        let val = json!(null);
        assert_eq!(json_to_toon(&val), "null");
    }

    #[test]
    fn test_boolean_values() {
        let val = json!({"enabled": true, "debug": false});
        let toon = json_to_toon(&val);
        assert!(toon.contains("enabled: true"));
        assert!(toon.contains("debug: false"));
    }

    #[test]
    fn test_string_needing_quotes() {
        let val = json!({"msg": "hello: world", "path": "a,b"});
        let toon = json_to_toon(&val);
        // Colon in value requires quoting
        assert!(toon.contains("msg: \"hello: world\""), "got: {}", toon);
    }

    #[test]
    fn test_string_looking_like_keyword() {
        let val = json!({"status": "true", "value": "null"});
        let toon = json_to_toon(&val);
        assert!(toon.contains("status: \"true\""), "got: {}", toon);
        assert!(toon.contains("value: \"null\""), "got: {}", toon);
    }

    #[test]
    fn test_number_canonicalization() {
        assert_eq!(format_number(&serde_json::Number::from(42)), "42");
        assert_eq!(format_number(&serde_json::Number::from(0)), "0");
        // -0 is tricky in serde_json; typically parsed as 0
    }

    #[test]
    fn test_root_primitive_array() {
        let val = json!([1, 2, 3]);
        let toon = json_to_toon(&val);
        assert!(toon.contains("[3]: 1,2,3"), "got: {}", toon);
    }

    #[test]
    fn test_root_tabular_array() {
        let val = json!([
            {"id": 1, "name": "Alice"},
            {"id": 2, "name": "Bob"}
        ]);
        let toon = json_to_toon(&val);
        assert!(toon.contains("[2]{id,name}:"), "got: {}", toon);
        assert!(toon.contains("  1,Alice"), "got: {}", toon);
        assert!(toon.contains("  2,Bob"), "got: {}", toon);
    }

    #[test]
    fn test_mixed_array_expanded() {
        // Array with non-uniform objects should use expanded form
        let val = json!({
            "items": [
                {"id": 1, "name": "Alice"},
                {"id": 2, "name": "Bob", "extra": true}
            ]
        });
        let toon = json_to_toon(&val);
        // Should NOT be tabular (different key sets)
        assert!(!toon.contains("{id,name}"), "got: {}", toon);
        // Should use expanded list items
        assert!(toon.contains("- id: 1"), "got: {}", toon);
    }

    #[test]
    fn test_key_quoting() {
        assert_eq!(format_key("simple"), "simple");
        assert_eq!(format_key("with space"), "\"with space\"");
        assert_eq!(format_key("has:colon"), "\"has:colon\"");
        assert_eq!(format_key("under_score"), "under_score");
        assert_eq!(format_key("dot.path"), "dot.path");
        assert_eq!(format_key("123start"), "\"123start\"");
        assert_eq!(format_key(""), "\"\"");
    }

    #[test]
    fn test_string_escaping() {
        assert_eq!(escape_string("hello"), "hello");
        assert_eq!(escape_string("line\nnew"), "line\\nnew");
        assert_eq!(escape_string("tab\there"), "tab\\there");
        assert_eq!(escape_string("quote\"here"), "quote\\\"here");
        assert_eq!(escape_string("back\\slash"), "back\\\\slash");
    }

    #[test]
    fn test_cell_string_quoting_with_comma() {
        // Comma in cell requires quoting
        assert_eq!(format_cell_string("a,b"), "\"a,b\"");
        assert_eq!(format_cell_string("simple"), "simple");
    }

    #[test]
    fn test_json_str_to_toon() {
        let json = r#"{"name":"Alice","age":30}"#;
        let toon = json_str_to_toon(json).unwrap();
        assert!(toon.contains("name: Alice"));
        assert!(toon.contains("age: 30"));
    }

    #[test]
    fn test_json_str_to_toon_invalid() {
        assert!(json_str_to_toon("not json").is_err());
    }

    #[test]
    fn test_tabular_savings() {
        // A realistic API response — tabular encoding should save significantly
        let val = json!({
            "results": [
                {"id": 1, "name": "Product A", "price": 29.99, "stock": 100},
                {"id": 2, "name": "Product B", "price": 49.99, "stock": 50},
                {"id": 3, "name": "Product C", "price": 19.99, "stock": 200},
                {"id": 4, "name": "Product D", "price": 99.99, "stock": 10},
                {"id": 5, "name": "Product E", "price": 14.99, "stock": 500}
            ]
        });
        let json_str = serde_json::to_string(&val).unwrap();
        let toon = json_to_toon(&val);

        assert!(
            toon.len() < json_str.len(),
            "TOON ({}) should be shorter than JSON ({})",
            toon.len(),
            json_str.len()
        );

        let savings_pct = (1.0 - (toon.len() as f64 / json_str.len() as f64)) * 100.0;
        assert!(
            savings_pct > 30.0,
            "Expected >30% savings, got {:.1}%",
            savings_pct
        );
    }

    #[test]
    fn test_nested_array_of_arrays() {
        let val = json!({
            "matrix": [[1, 2], [3, 4]]
        });
        let toon = json_to_toon(&val);
        assert!(toon.contains("matrix[2]:"), "got: {}", toon);
    }

    #[test]
    fn test_object_with_nested_array_and_object() {
        let val = json!({
            "meta": {"version": 1},
            "data": [1, 2, 3]
        });
        let toon = json_to_toon(&val);
        assert!(toon.contains("meta:"), "got: {}", toon);
        assert!(toon.contains("  version: 1"), "got: {}", toon);
        assert!(toon.contains("data[3]: 1,2,3"), "got: {}", toon);
    }

    #[test]
    fn test_empty_string_quoted() {
        let val = json!({"key": ""});
        let toon = json_to_toon(&val);
        assert!(toon.contains("key: \"\""), "got: {}", toon);
    }

    #[test]
    fn test_numeric_string_quoted() {
        // A string that looks like a number must be quoted
        let val = json!({"version": "42"});
        let toon = json_to_toon(&val);
        assert!(toon.contains("version: \"42\""), "got: {}", toon);
    }
}
