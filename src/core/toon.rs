//! TOON (Token-Oriented Object Notation) encoder for `serde_json::Value`.
//!
//! TOON drops JSON's quotes/braces/commas and represents arrays of uniform
//! objects in tabular form, typically saving 30-60% tokens vs. JSON. See:
//! <https://github.com/toon-format/toon>
//!
//! ## Two interchangeable backends
//!
//! This module exposes a stable RTK-facing API — `encode(&Value) -> String`
//! and `try_from_json(&str) -> Option<String>` — that delegates to one of:
//!
//! 1. **Built-in** (default): a self-contained subset of the TOON spec
//!    sufficient for RTK's JSON filter outputs (`gh --json`,
//!    `kubectl -o json`, `aws --output json`, `pnpm list --json`, etc.).
//!    Preserves JSON key insertion order. No external dependency.
//! 2. **External crate** (`--features toon-crate`): delegates to the
//!    upstream [`toon`](https://crates.io/crates/toon) crate. Sorts object
//!    keys alphabetically and supports more spec features (custom
//!    delimiters, length markers — currently unused by RTK).
//!
//! Both are one-way encoders (RTK never reads TOON back). The fallback
//! contract — invalid JSON → `try_from_json` returns `None` — is identical
//! in both backends.
//!
//! ## Swapping backends
//!
//! ```bash
//! cargo build                          # built-in (default)
//! cargo build --features toon-crate    # external `toon` crate
//! ```
//!
//! No call site needs to change.
//!
//! ## Design rules (per RTK conventions)
//!
//! - No async, no panics in production paths — `unwrap()` only on writes to
//!   `String` (infallible) inside the built-in encoder.
//! - Lossless for the JSON subset we encode: round-tripping via `decode` is
//!   not implemented because RTK never reads TOON back.

use serde_json::Value;

/// Encode a JSON value as TOON. Backend is selected at compile time by the
/// `toon-crate` Cargo feature (see module docs).
#[cfg(feature = "toon-crate")]
pub fn encode(value: &Value) -> String {
    ::toon::encode(value, None)
}

/// Encode a JSON value as TOON (built-in backend).
///
/// - `null`, booleans and numbers are emitted bare.
/// - Strings are bare when they match `[A-Za-z0-9_./@:+-]+` and are not a
///   reserved literal (`true`/`false`/`null`); otherwise double-quoted with
///   minimal JSON-style escaping.
/// - Objects emit `key: value` per line, nested with 2-space indent.
/// - Arrays of primitives use inline `[a,b,c]`.
/// - Arrays of objects sharing the **same** flat-primitive key set use the
///   TOON tabular form: `key[N]{f1,f2,f3}:` followed by CSV rows.
/// - Other arrays fall back to a `- ` bulleted list.
#[cfg(not(feature = "toon-crate"))]
pub fn encode(value: &Value) -> String {
    let mut out = String::new();
    builtin::write_value(&mut out, value, 0, true);
    // Strip leading newline if value started with a block at root.
    if out.starts_with('\n') {
        out.remove(0);
    }
    out
}

/// Convenience: parse JSON then encode as TOON. Returns `None` on parse
/// failure so callers can fall back transparently. Behavior is identical
/// across both backends.
pub fn try_from_json(json_str: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    Some(encode(&v))
}

// ── Built-in encoder ──────────────────────────────────────────
#[cfg(not(feature = "toon-crate"))]
mod builtin {
    use super::Value;
    use std::fmt::Write;

    pub(super) fn write_value(
        out: &mut String,
        value: &Value,
        indent: usize,
        at_block_start: bool,
    ) {
        match value {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) => write!(out, "{n}").unwrap(),
            Value::String(s) => out.push_str(&encode_string(s)),
            Value::Array(arr) => write_array(out, arr, indent, at_block_start),
            Value::Object(map) => write_object(out, map, indent, at_block_start),
        }
    }

    fn write_object(
        out: &mut String,
        map: &serde_json::Map<String, Value>,
        indent: usize,
        at_block_start: bool,
    ) {
        if map.is_empty() {
            out.push_str("{}");
            return;
        }

        // When an object is the value of a parent `key:`, start on a new line.
        let mut first = true;
        for (k, v) in map {
            if !first || !at_block_start {
                out.push('\n');
                push_indent(out, indent);
            }
            first = false;
            out.push_str(&encode_key(k));
            out.push_str(": ");
            match v {
                Value::Object(inner) if !inner.is_empty() => {
                    // Nested object: header on this line, body indented.
                    // Replace the trailing space after ':' nothing else needed —
                    // the recursion will emit a newline + indent for each child.
                    out.pop(); // remove trailing space; child will newline
                    write_value(out, v, indent + 1, false);
                }
                Value::Array(arr) if is_tabular(arr) => {
                    // Tabular form supplies its own `[N]{fields}:` header — strip
                    // the just-emitted `: ` so we get `users[N]{...}` not `users: [N]{...}`.
                    out.pop(); // ' '
                    out.pop(); // ':'
                    write_tabular(out, arr, indent);
                }
                Value::Array(arr) if !is_inline_primitive_array(arr) && !arr.is_empty() => {
                    out.pop();
                    write_value(out, v, indent + 1, false);
                }
                _ => write_value(out, v, indent + 1, true),
            }
        }
    }

    fn write_array(out: &mut String, arr: &[Value], indent: usize, at_block_start: bool) {
        if arr.is_empty() {
            out.push_str("[]");
            return;
        }

        if is_inline_primitive_array(arr) {
            out.push('[');
            for (i, v) in arr.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(out, v, indent, true);
            }
            out.push(']');
            return;
        }

        if is_tabular(arr) {
            // Tabular at the root: synthesize a leading "[N]{f1,f2}:" header.
            write_tabular_root(out, arr, indent);
            return;
        }

        // Bulleted list fallback.
        let mut first = true;
        for v in arr {
            if !first || !at_block_start {
                out.push('\n');
                push_indent(out, indent);
            }
            first = false;
            out.push_str("- ");
            write_value(out, v, indent + 1, true);
        }
    }

    // ── Tabular form ──────────────────────────────────────────────

    /// True if `arr` is a non-empty array of objects all sharing the same key
    /// set (in the same order) and whose values are all flat primitives.
    fn is_tabular(arr: &[Value]) -> bool {
        if arr.len() < 2 {
            return false;
        }
        let Some(Value::Object(first)) = arr.first() else {
            return false;
        };
        if first.is_empty() {
            return false;
        }
        if !first.values().all(is_flat_primitive) {
            return false;
        }
        let keys: Vec<&String> = first.keys().collect();
        arr.iter().all(|v| match v {
            Value::Object(m) => {
                m.len() == keys.len()
                    && m.keys().zip(&keys).all(|(a, b)| a == *b)
                    && m.values().all(is_flat_primitive)
            }
            _ => false,
        })
    }

    fn is_flat_primitive(v: &Value) -> bool {
        matches!(
            v,
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
        )
    }

    fn is_inline_primitive_array(arr: &[Value]) -> bool {
        !arr.is_empty() && arr.iter().all(is_flat_primitive)
    }

    fn write_tabular(out: &mut String, arr: &[Value], indent: usize) {
        let Some(Value::Object(first)) = arr.first() else {
            return;
        };
        let fields: Vec<&String> = first.keys().collect();
        write!(out, "[{}]{{", arr.len()).unwrap();
        for (i, f) in fields.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&encode_key(f));
        }
        out.push_str("}:");
        for row in arr {
            let Value::Object(m) = row else { continue };
            out.push('\n');
            push_indent(out, indent + 1);
            for (i, f) in fields.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&encode_csv_cell(&m[f.as_str()]));
            }
        }
    }

    fn write_tabular_root(out: &mut String, arr: &[Value], indent: usize) {
        let Some(Value::Object(first)) = arr.first() else {
            return;
        };
        let fields: Vec<&String> = first.keys().collect();
        write!(out, "[{}]{{", arr.len()).unwrap();
        for (i, f) in fields.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&encode_key(f));
        }
        out.push_str("}:");
        for row in arr {
            let Value::Object(m) = row else { continue };
            out.push('\n');
            push_indent(out, indent + 1);
            for (i, f) in fields.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&encode_csv_cell(&m[f.as_str()]));
            }
        }
    }

    fn encode_csv_cell(v: &Value) -> String {
        match v {
            Value::Null => "null".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            Value::String(s) => encode_csv_string(s),
            _ => "?".to_string(), // unreachable in tabular form
        }
    }

    /// CSV-style cell encoding: bare when safe, double-quoted (with `""` escape)
    /// when the string contains commas, quotes, newlines, or leading/trailing
    /// whitespace.
    fn encode_csv_string(s: &str) -> String {
        let needs_quote = s.is_empty()
            || s.contains(',')
            || s.contains('"')
            || s.contains('\n')
            || s.contains('\r')
            || s.starts_with(' ')
            || s.ends_with(' ');
        if !needs_quote {
            return s.to_string();
        }
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            if c == '"' {
                out.push('"');
            }
            out.push(c);
        }
        out.push('"');
        out
    }

    // ── String / key encoding ─────────────────────────────────────

    fn encode_key(s: &str) -> String {
        if is_bare_safe(s) {
            s.to_string()
        } else {
            encode_string(s)
        }
    }

    fn encode_string(s: &str) -> String {
        if is_bare_safe(s) && !is_reserved(s) {
            return s.to_string();
        }
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32).unwrap(),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    fn is_bare_safe(s: &str) -> bool {
        !s.is_empty()
            && s.chars().all(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '@' | ':' | '+')
            })
    }

    fn is_reserved(s: &str) -> bool {
        matches!(s, "true" | "false" | "null")
    }

    fn push_indent(out: &mut String, indent: usize) {
        for _ in 0..indent {
            out.push_str("  ");
        }
    }
} // mod builtin

// ── Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    // ── Backend-agnostic tests ───────────────────────────────────
    // These run with both the built-in encoder and `--features toon-crate`.

    #[test]
    fn primitives() {
        assert_eq!(encode(&json!(null)), "null");
        assert_eq!(encode(&json!(true)), "true");
        assert_eq!(encode(&json!(42)), "42");
        // Float formatting may differ slightly between backends; just assert
        // the digit prefix is present.
        assert!(encode(&json!(2.5)).starts_with("2.5"));
        assert_eq!(encode(&json!("hello")), "hello");
    }

    #[test]
    fn empty_containers() {
        // Both backends must round-trip empty containers somehow; we just
        // require they don't panic and produce non-error output.
        let _ = encode(&json!({}));
        let _ = encode(&json!([]));
    }

    #[test]
    fn round_trip_via_json_parse() {
        let raw = r#"{"k": [1, 2, {"nested": true}], "s": "hi"}"#;
        let toon = try_from_json(raw).expect("valid JSON");
        assert!(toon.contains("nested"));
        assert!(toon.contains("hi"));
    }

    #[test]
    fn invalid_json_returns_none() {
        assert!(try_from_json("not json").is_none());
    }

    #[test]
    fn deeply_nested_does_not_panic() {
        let mut v = json!(0);
        for _ in 0..100 {
            v = json!({ "n": v });
        }
        let _ = encode(&v);
    }

    #[test]
    fn tabular_saves_tokens_vs_json() {
        let users: Vec<Value> = (0..20)
            .map(|i| {
                json!({
                    "id": i,
                    "name": format!("user{i}"),
                    "email": format!("user{i}@example.com"),
                    "active": i % 2 == 0
                })
            })
            .collect();
        let v = json!({ "users": users });
        let json_str = serde_json::to_string_pretty(&v).unwrap();
        let toon_str = encode(&v);

        let savings =
            100.0 - (count_tokens(&toon_str) as f64 / count_tokens(&json_str) as f64) * 100.0;
        assert!(
            savings >= 30.0,
            "expected ≥30% savings on tabular data, got {savings:.1}%"
        );
        assert!(
            toon_str.len() < json_str.len() / 2,
            "TOON should be <50% size of pretty JSON; toon={} json={}",
            toon_str.len(),
            json_str.len()
        );
    }

    #[test]
    fn gh_pr_list_fixture_savings() {
        let raw = include_str!("../../tests/fixtures/gh_pr_list.json");
        let toon = try_from_json(raw).expect("fixture must parse");

        let raw_tokens = count_tokens(raw);
        let toon_tokens = count_tokens(&toon);
        let savings = 100.0 - (toon_tokens as f64 / raw_tokens as f64) * 100.0;

        assert!(
            savings >= 30.0,
            "gh-pr-list fixture: expected ≥30% savings, got {savings:.1}% (raw={raw_tokens}, toon={toon_tokens})"
        );
        // Backend-agnostic sanity: tabular header for 10 rows is present.
        assert!(
            toon.contains("[10]"),
            "expected tabular header for 10 rows, got:\n{toon}"
        );
    }

    // ── Built-in-only format assertions ──────────────────────────
    // These pin the exact byte output of the built-in encoder. The external
    // `toon` crate sorts keys alphabetically and uses slightly different
    // punctuation, so format-equality tests are gated to the built-in.

    #[cfg(not(feature = "toon-crate"))]
    #[test]
    fn builtin_quoted_strings() {
        assert_eq!(encode(&json!("hello world")), "\"hello world\"");
        assert_eq!(encode(&json!("true")), "\"true\"");
        assert_eq!(encode(&json!("a\"b")), "\"a\\\"b\"");
        assert_eq!(encode(&json!("")), "\"\"");
    }

    #[cfg(not(feature = "toon-crate"))]
    #[test]
    fn builtin_empty_containers_format() {
        assert_eq!(encode(&json!({})), "{}");
        assert_eq!(encode(&json!([])), "[]");
    }

    #[cfg(not(feature = "toon-crate"))]
    #[test]
    fn builtin_flat_object_preserves_order() {
        let v = json!({"name": "alice", "age": 30});
        assert_eq!(encode(&v), "name: alice\nage: 30");
    }

    #[cfg(not(feature = "toon-crate"))]
    #[test]
    fn builtin_nested_object() {
        let v = json!({"user": {"name": "alice", "age": 30}});
        assert_eq!(encode(&v), "user:\n  name: alice\n  age: 30");
    }

    #[cfg(not(feature = "toon-crate"))]
    #[test]
    fn builtin_inline_primitive_array() {
        let v = json!({"tags": ["red", "green", "blue"]});
        assert_eq!(encode(&v), "tags: [red,green,blue]");
    }

    #[cfg(not(feature = "toon-crate"))]
    #[test]
    fn builtin_tabular_array() {
        let v = json!({
            "users": [
                {"id": 1, "name": "Alice", "role": "admin"},
                {"id": 2, "name": "Bob",   "role": "user"},
                {"id": 3, "name": "Carol", "role": "admin"}
            ]
        });
        let expected = "users[3]{id,name,role}:\n  1,Alice,admin\n  2,Bob,user\n  3,Carol,admin";
        assert_eq!(encode(&v), expected);
    }

    #[cfg(not(feature = "toon-crate"))]
    #[test]
    fn builtin_tabular_with_quoted_cell() {
        let v = json!({
            "rows": [
                {"a": "x,y", "b": "ok"},
                {"a": "plain", "b": "with \"quote\""}
            ]
        });
        let toon = encode(&v);
        assert!(toon.contains("\"x,y\""));
        assert!(toon.contains("\"with \"\"quote\"\"\""));
    }

    #[cfg(not(feature = "toon-crate"))]
    #[test]
    fn builtin_array_of_objects_non_uniform_falls_back_to_bullets() {
        let v = json!([
            {"a": 1, "b": 2},
            {"a": 1, "c": 3}
        ]);
        let toon = encode(&v);
        assert!(toon.starts_with("- "));
    }

    #[cfg(not(feature = "toon-crate"))]
    #[test]
    fn builtin_gh_pr_list_field_order_preserved() {
        let raw = include_str!("../../tests/fixtures/gh_pr_list.json");
        let toon = try_from_json(raw).expect("fixture must parse");
        // Built-in preserves JSON insertion order.
        assert!(toon.contains("[10]{number,title,author,state,isDraft,additions,deletions}:"));
    }
}
