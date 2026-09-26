//! Inspects JSON structure without showing values, saving tokens on large payloads.

use crate::core::guard::never_worse;
use crate::core::tracking;
use crate::core::utils::{from_json_str, strip_leading_bom};
use anyhow::{Context, Result, bail};
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

    let shown = render_json(&content, max_depth, schema_only)?;
    println!("{}", shown);
    timer.track(
        &format!("cat {}", file.display()),
        "rtk json",
        &content,
        &shown,
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

    let shown = render_json(&content, max_depth, schema_only)?;
    println!("{}", shown);
    timer.track("cat - (stdin)", "rtk json -", &content, &shown);
    Ok(())
}

/// Filter `content` and fall back to it verbatim if the filtered form isn't
/// smaller. Strips a leading BOM once, up front, and compares/falls back
/// against the *stripped* content — otherwise a raw fallback would still
/// carry the BOM into piped output (`rtk json foo.json | jq .` failing to
/// parse it) even though `filter_json_*` already tolerates a BOM on input.
///
/// Returns `Cow` rather than an owned `String`: the raw-fallback case can
/// then stay a zero-copy borrow of `content` instead of paying for another
/// full copy of the (potentially large) input on every fallback.
fn render_json<'a>(content: &'a str, max_depth: usize, schema_only: bool) -> Result<Cow<'a, str>> {
    let content = strip_leading_bom(content);
    let output = if schema_only {
        filter_json_string(content, max_depth)?
    } else {
        filter_json_compact(content, max_depth)?
    };
    let shown = never_worse(content, &output);
    // never_worse hands back one of its two inputs (no allocation); compare
    // the `&str` fat pointers to tell which, instead of re-deriving the
    // decision. Comparing the whole slice, not just `as_ptr()`, so a future
    // never_worse that returns a sub-slice of `content` is not mistaken for
    // `content` itself -- that would silently re-expand the output.
    Ok(if std::ptr::eq(shown, content) {
        Cow::Borrowed(content)
    } else {
        Cow::Owned(output)
    })
}

/// Parse a JSON string and return compact representation with values preserved.
/// Long strings are truncated and large arrays are summarized. Bounded arrays of
/// short scalars keep every element (see `SCALAR_ARRAY_PRESERVE_LIMIT`).
pub fn filter_json_compact(json_str: &str, max_depth: usize) -> Result<String> {
    let value: Value = from_json_str(json_str).context("Failed to parse JSON")?;
    Ok(compact_json(&value, 0, max_depth))
}

/// Preserve arrays of at most this many short scalars instead of collapsing
/// them to `[first, ... +N more]`. 32 is the #4073 policy: large enough for
/// typical enums, small enough that the extra bytes stay cheap. Arrays with
/// nested containers, long strings, or more elements keep the legacy paths.
const SCALAR_ARRAY_PRESERVE_LIMIT: usize = 32;

/// Matches the existing short-string boundary in `compact_json` (`s.len() > 80`).
const SHORT_STRING_MAX_BYTES: usize = 80;

fn is_short_scalar(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => true,
        Value::String(s) => s.len() <= SHORT_STRING_MAX_BYTES,
        _ => false,
    }
}

/// ISSUE #4073: small enums (role lists, status codes) were summarized after
/// five items, hiding legitimate values. Render every qualifying scalar with
/// serde_json so spelling and escaping round-trip; do not drop duplicates or
/// key off property names. `depth < max_depth` keeps the child-element depth
/// gate: at the depth boundary children still become `...` via the legacy path.
fn format_bounded_scalar_array(arr: &[Value], indent: &str) -> String {
    let items: Vec<String> = arr.iter().map(Value::to_string).collect();
    format!("{}[{}]", indent, items.join(", "))
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
            if s.len() > SHORT_STRING_MAX_BYTES {
                let end = s.floor_char_boundary(77);
                format!("{}\"{}...\"", indent, &s[..end])
            } else {
                format!("{}\"{}\"", indent, s)
            }
        }
        Value::Array(arr) => {
            if arr.is_empty() {
                format!("{}[]", indent)
            } else if arr.len() <= SCALAR_ARRAY_PRESERVE_LIMIT
                && depth < max_depth
                && arr.iter().all(is_short_scalar)
            {
                format_bounded_scalar_array(arr, &indent)
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
    let value: Value = from_json_str(json_str).context("Failed to parse JSON")?;
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
    fn test_compact_parses_bom_prefixed_json() {
        let json = "\u{feff}{\"name\": \"test\", \"count\": 42}";
        let output = filter_json_compact(json, 5).expect("BOM-prefixed JSON must parse");
        assert!(output.contains("name"));
        assert!(output.contains("42"));
        // Same input without BOM produces identical output.
        assert_eq!(output, filter_json_compact(&json[3..], 5).unwrap());
    }

    #[test]
    fn test_schema_parses_bom_prefixed_json() {
        let json = "\u{feff}{\"name\": \"test\", \"count\": 42}";
        let output = filter_json_string(json, 5).expect("BOM-prefixed JSON must parse");
        assert!(output.contains("string"));
        assert!(output.contains("int"));
        assert_eq!(output, filter_json_string(&json[3..], 5).unwrap());
    }

    #[test]
    fn test_render_json_fallback_strips_bom_when_filtered_is_larger() {
        // A minified BOM-prefixed package.json: compact_json pretty-prints
        // with indentation, so `filtered` ends up larger than the minified
        // raw and never_worse falls back to raw. If that comparison (and the
        // returned string) still carries the BOM, `rtk json foo.json | jq .`
        // fails to parse — the exact bug this fix closes.
        let raw = "\u{feff}{\"a\":1,\"b\":2,\"c\":3,\"d\":4,\"e\":5}";
        let stripped = strip_leading_bom(raw);

        // render_json takes the raw, un-stripped content directly — the same
        // shape run()/run_stdin() hand it after fs::read_to_string /
        // reading stdin. It must strip internally, not rely on the caller.
        let shown = render_json(raw, 5, false).expect("must render");

        // Sanity: this exercises the raw-fallback path (a zero-copy borrow
        // of the stripped content), not the filtered/owned one.
        assert!(
            matches!(shown, Cow::Borrowed(_)),
            "raw fallback should borrow, not allocate: {shown:?}"
        );
        assert_eq!(shown, stripped, "expected the raw (BOM-stripped) fallback");
        assert!(
            !shown.starts_with('\u{feff}'),
            "fallback output must not carry a BOM into piped output: {shown:?}"
        );
        let _: Value = serde_json::from_str(&shown).expect("fallback output must parse as JSON");
    }

    #[test]
    fn test_compact_truncates_pure_multibyte_string() {
        assert_value_truncated(&"日本語テスト".repeat(85));
    }

    #[test]
    fn test_compact_truncates_mixed_ascii_multibyte_string() {
        assert_value_truncated(&("a".repeat(76) + &"日本語".repeat(5)));
    }

    // --- #4073: bounded scalar arrays keep every value ---

    /// Issue body example (`roles.json` as a single object line).
    const ISSUE_ROLES_MINIFIED: &str = r#"{ "status": "ok", "allowed_roles": ["admin", "editor", "reviewer", "publisher", "auditor", "moderator", "archivist", "guest"], "count": 8 }"#;
    const ISSUE_ROLES: [&str; 8] = [
        "admin",
        "editor",
        "reviewer",
        "publisher",
        "auditor",
        "moderator",
        "archivist",
        "guest",
    ];

    /// Follow-up pretty-printed reproduction from the issue thread.
    const ISSUE_ROLES_PRETTY: &str = r#"{
  "status": "ok",
  "count": 8,
  "allowed_roles": ["admin", "editor", "viewer", "owner", "member", "guest", "auditor", "support"]
}"#;
    const ISSUE_ROLES_PRETTY_VALUES: [&str; 8] = [
        "admin", "editor", "viewer", "owner", "member", "guest", "auditor", "support",
    ];

    fn assert_no_array_omission(output: &str) {
        assert!(
            !output.contains("... +"),
            "scalar array must not use an omission marker, got: {output}"
        );
    }

    fn assert_values_in_order(output: &str, serialized: &[String]) {
        let mut rest = output;
        for needle in serialized {
            match rest.find(needle.as_str()) {
                Some(i) => rest = &rest[i + needle.len()..],
                None => panic!("missing {needle} in order in: {output}"),
            }
        }
    }

    fn serialized_strings(values: &[&str]) -> Vec<String> {
        values
            .iter()
            .map(|v| Value::String((*v).to_string()).to_string())
            .collect()
    }

    fn numbered_array_json(n: usize) -> String {
        let items: Vec<String> = (0..n).map(|i| format!("\"val_{i}\"")).collect();
        format!("[{}]", items.join(","))
    }

    fn numbered_object_json(key: &str, n: usize) -> String {
        format!(r#"{{"{key}":{}}}"#, numbered_array_json(n))
    }

    fn numbered_values(n: usize) -> Vec<String> {
        (0..n)
            .map(|i| Value::String(format!("val_{i}")).to_string())
            .collect()
    }

    fn assert_preserves_scalar_array(json: &str, serialized: &[String]) {
        let compact = filter_json_compact(json, 5).expect("filter_json_compact must parse");
        let rendered = render_json(json, 5, false).expect("render_json must parse");
        for output in [compact.as_str(), rendered.as_ref()] {
            assert_no_array_omission(output);
            assert_values_in_order(output, serialized);
        }
    }

    #[test]
    fn test_issue_eight_roles_minified_preserves_every_value() {
        let expected = serialized_strings(&ISSUE_ROLES);
        assert_preserves_scalar_array(ISSUE_ROLES_MINIFIED, &expected);
    }

    #[test]
    fn test_issue_eight_roles_pretty_preserves_every_value() {
        let expected = serialized_strings(&ISSUE_ROLES_PRETTY_VALUES);
        assert_preserves_scalar_array(ISSUE_ROLES_PRETTY, &expected);
        let rendered = render_json(ISSUE_ROLES_PRETTY, 5, false).expect("render");
        assert!(
            matches!(rendered, Cow::Owned(_)),
            "pretty-printed input should take the compact path, got: {rendered:?}"
        );
    }

    #[test]
    fn test_scalar_array_size_table_root_and_nested() {
        for n in [0usize, 1, 5, 6, 8, 32, 33] {
            let root = numbered_array_json(n);
            let nested = numbered_object_json("widget_ids", n);
            let compact_root = filter_json_compact(&root, 5).expect("root");
            let compact_nested = filter_json_compact(&nested, 5).expect("nested");
            let rendered_root = render_json(&root, 5, false).expect("render root");
            let rendered_nested = render_json(&nested, 5, false).expect("render nested");

            if n == 0 {
                assert!(compact_root.contains("[]"), "{compact_root}");
                assert!(compact_nested.contains("[]"), "{compact_nested}");
                continue;
            }

            if n <= SCALAR_ARRAY_PRESERVE_LIMIT {
                let expected = numbered_values(n);
                for output in [
                    compact_root.as_str(),
                    compact_nested.as_str(),
                    rendered_root.as_ref(),
                    rendered_nested.as_ref(),
                ] {
                    assert_no_array_omission(output);
                    assert_values_in_order(output, &expected);
                }
            } else {
                let omitted = n - 1;
                let marker = format!("... +{omitted} more");
                for output in [compact_root.as_str(), compact_nested.as_str()] {
                    assert!(
                        output.contains(&marker),
                        "33-element array must keep the legacy summary ({marker}): {output}"
                    );
                    assert!(
                        output.contains("\"val_0\""),
                        "summary should keep the first value: {output}"
                    );
                    assert!(
                        !output.contains(&format!("\"val_{}\"", n - 1)),
                        "summarized array must omit the last value: {output}"
                    );
                }
            }
        }
    }

    #[test]
    fn test_mixed_short_scalars_preserve_order_types_and_duplicates() {
        let json = r#"[1, true, null, "dup", "dup", false, 0, "dup"]"#;
        let compact = filter_json_compact(json, 5).expect("compact");
        assert_no_array_omission(&compact);
        let parsed: Value =
            serde_json::from_str(compact.trim()).expect("root scalar array must be valid JSON");
        let expected: Value = serde_json::from_str(json).expect("fixture");
        assert_eq!(parsed, expected);
        assert_eq!(
            render_json(json, 5, false).expect("render").as_ref(),
            compact.as_str()
        );
    }

    #[test]
    fn test_numeric_enum_and_unrelated_property_name() {
        let json = r#"{"http_status":[200,201,400,401,403,404,500,503]}"#;
        let expected: Vec<String> = [200, 201, 400, 401, 403, 404, 500, 503]
            .into_iter()
            .map(|n| n.to_string())
            .collect();
        assert_preserves_scalar_array(json, &expected);
    }

    #[test]
    fn test_escaped_strings_round_trip() {
        let json = r#"["quote\"here","back\\slash","new\nline","uni\u00e9","日本語"]"#;
        let compact = filter_json_compact(json, 5).expect("compact");
        assert_no_array_omission(&compact);
        let parsed: Value =
            serde_json::from_str(compact.trim()).expect("escaped scalars must round-trip");
        let expected: Value = serde_json::from_str(json).expect("fixture");
        assert_eq!(parsed, expected);
    }

    #[test]
    fn test_string_length_boundary_80_vs_81_bytes() {
        let s80 = "a".repeat(SHORT_STRING_MAX_BYTES);
        let s81 = "a".repeat(SHORT_STRING_MAX_BYTES + 1);
        let keep = Value::Array(vec![
            Value::String(s80.clone()),
            Value::String("x".into()),
            Value::String("y".into()),
            Value::String("z".into()),
            Value::String("p".into()),
            Value::String("q".into()),
        ]);
        let drop_long = Value::Array(vec![
            Value::String(s81.clone()),
            Value::String("x".into()),
            Value::String("y".into()),
            Value::String("z".into()),
            Value::String("p".into()),
            Value::String("q".into()),
        ]);
        let keep_json = keep.to_string();
        let drop_json = drop_long.to_string();

        let kept = filter_json_compact(&keep_json, 5).expect("80-byte strings qualify");
        assert_no_array_omission(&kept);
        let parsed: Value = serde_json::from_str(kept.trim()).expect("80-byte array round-trip");
        assert_eq!(parsed, keep);

        let summarized = filter_json_compact(&drop_json, 5).expect("81-byte string is not short");
        assert!(
            summarized.contains("... +5 more"),
            "array containing an 81-byte string must not take the preservation exception: {summarized}"
        );
    }

    #[test]
    fn test_unicode_80_byte_string_qualifies() {
        // "é" is 2 UTF-8 bytes; 40 of them is exactly 80 bytes.
        let s80 = "é".repeat(40);
        assert_eq!(s80.len(), SHORT_STRING_MAX_BYTES);
        let json = Value::Array(vec![
            Value::String(s80.clone()),
            Value::String("a".into()),
            Value::String("b".into()),
            Value::String("c".into()),
            Value::String("d".into()),
            Value::String("e".into()),
        ])
        .to_string();
        let compact = filter_json_compact(&json, 5).expect("unicode 80-byte");
        assert_no_array_omission(&compact);
        let parsed: Value = serde_json::from_str(compact.trim()).expect("round-trip");
        assert_eq!(parsed[0], Value::String(s80));
    }

    #[test]
    fn test_arrays_of_objects_and_nested_arrays_keep_summary() {
        let objects = r#"[{"a":1},{"a":2},{"a":3},{"a":4},{"a":5},{"a":6}]"#;
        let nested = r#"[[1,2],[3,4],[5,6],[7,8],[9,10],[11,12]]"#;
        for json in [objects, nested] {
            let output = filter_json_compact(json, 5).expect("compact");
            assert!(
                output.contains("... +5 more"),
                "container arrays must keep the legacy summary: {output}"
            );
        }
    }

    #[test]
    fn test_larger_array_keeps_exact_omitted_count() {
        let json = numbered_object_json("batch", 40);
        let output = filter_json_compact(&json, 5).expect("compact");
        assert!(
            output.contains("... +39 more"),
            "omitted count must stay exact: {output}"
        );
    }

    #[test]
    fn test_depth_gate_root_array_children_exceed_max_depth() {
        let json = numbered_array_json(8);
        let output = filter_json_compact(&json, 0).expect("depth 0");
        assert_eq!(
            output, "[..., ... +7 more]",
            "root array at max_depth 0 must still depth-gate children"
        );
    }

    #[test]
    fn test_depth_gate_nested_array_children_exceed_max_depth() {
        let json = numbered_object_json("payload", 8);
        let output = filter_json_compact(&json, 1).expect("depth 1");
        assert!(
            output.contains("[..., ... +7 more]"),
            "nested array children past max depth must stay summarized: {output}"
        );
        assert!(
            !output.contains("\"val_1\""),
            "depth-gated children must not leak later values: {output}"
        );
    }

    #[test]
    fn test_depth_gate_nested_array_within_budget_preserves() {
        let json = numbered_object_json("payload", 8);
        let output = filter_json_compact(&json, 2).expect("depth 2");
        assert_no_array_omission(&output);
        assert_values_in_order(&output, &numbered_values(8));
    }

    #[test]
    fn test_keys_only_schema_unchanged_for_eight_roles() {
        let output = filter_json_string(ISSUE_ROLES_PRETTY, 5).expect("schema");
        assert!(
            output.contains("[string] (8)"),
            "keys-only should still summarize the array type: {output}"
        );
        for role in ISSUE_ROLES_PRETTY_VALUES {
            assert!(
                !output.contains(role),
                "keys-only must not leak role values ({role}): {output}"
            );
        }
        let rendered = render_json(ISSUE_ROLES_PRETTY, 5, true).expect("render schema");
        assert_eq!(rendered.as_ref(), output.as_str());
    }

    #[test]
    fn test_pretty_roles_bom_stripped_and_never_worse() {
        let raw = format!("\u{feff}{ISSUE_ROLES_PRETTY}");
        let shown = render_json(&raw, 5, false).expect("render BOM pretty");
        assert!(
            !shown.starts_with('\u{feff}'),
            "BOM must not leak into output: {shown:?}"
        );
        assert_no_array_omission(shown.as_ref());
        assert_values_in_order(
            shown.as_ref(),
            &serialized_strings(&ISSUE_ROLES_PRETTY_VALUES),
        );
        assert!(
            crate::core::tracking::estimate_tokens(shown.as_ref())
                <= crate::core::tracking::estimate_tokens(strip_leading_bom(&raw)),
            "never_worse must still hold"
        );
    }

    #[test]
    fn test_minified_roles_bom_never_worse() {
        let minified = r#"{"status":"ok","allowed_roles":["admin","editor","reviewer","publisher","auditor","moderator","archivist","guest"],"count":8}"#;
        let raw = format!("\u{feff}{minified}");
        let shown = render_json(&raw, 5, false).expect("render BOM minified");
        assert!(!shown.starts_with('\u{feff}'));
        assert_values_in_order(shown.as_ref(), &serialized_strings(&ISSUE_ROLES));
        assert!(
            crate::core::tracking::estimate_tokens(shown.as_ref())
                <= crate::core::tracking::estimate_tokens(strip_leading_bom(&raw)),
            "never_worse must still hold for minified input"
        );
    }

    #[test]
    fn test_malformed_json_still_errors() {
        let err = filter_json_compact("{not json", 5).unwrap_err();
        assert!(
            err.to_string().contains("Failed to parse JSON"),
            "parse error must stay wrapped: {err}"
        );
    }
}
