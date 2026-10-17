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
    timer.track(&super::read::cat_label(file), "rtk json", &content, &shown);
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
/// `schema_only` skips the fallback entirely rather than falling back through
/// it — see the comment on that branch.
///
/// Returns `Cow` rather than an owned `String`: the raw-fallback case can
/// then stay a zero-copy borrow of `content` instead of paying for another
/// full copy of the (potentially large) input on every fallback.
fn render_json<'a>(content: &'a str, max_depth: usize, schema_only: bool) -> Result<Cow<'a, str>> {
    let content = strip_leading_bom(content);
    if schema_only {
        // `--keys-only` asks for the structure without the values, so the raw
        // fallback prints every value back. Raw JSON is not a rendering of the
        // answer, so there is no pair for never_worse to compare: the schema
        // is the answer, and in compact form it is also smaller than the input
        // in practice. Bypassing the guard is therefore the only correct path.
        return Ok(Cow::Owned(filter_json_string(content, max_depth)?));
    }
    let output = filter_json_compact(content, max_depth)?;
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
/// Long strings are truncated, arrays are summarized.
pub fn filter_json_compact(json_str: &str, max_depth: usize) -> Result<String> {
    let value: Value = from_json_str(json_str).context("Failed to parse JSON")?;
    Ok(compact_json(&value, 0, max_depth))
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
    let value: Value = from_json_str(json_str).context("Failed to parse JSON")?;
    Ok(extract_schema(&value, 0, max_depth))
}

/// Keys rendered per object; any remaining keys are summarized as `...+N`.
const MAX_SCHEMA_KEYS: usize = 16;

/// Render `value` as a compact schema (`{a:int,b:{c:string}}`), with no
/// indentation or newlines so it stays at or under the minified input that
/// `--keys-only` replaces.
fn extract_schema(value: &Value, depth: usize, max_depth: usize) -> String {
    if depth > max_depth {
        return "...".to_string();
    }

    match value {
        Value::Null => "null".to_string(),
        Value::Bool(_) => "bool".to_string(),
        Value::Number(n) => {
            if n.is_i64() {
                "int".to_string()
            } else {
                "float".to_string()
            }
        }
        Value::String(s) => {
            if s.len() > 50 {
                format!("string[{}]", s.len())
            } else if s.is_empty() {
                "string".to_string()
            } else if s.starts_with("http") {
                "url".to_string()
            } else if s.contains('-') && s.len() == 10 {
                "date?".to_string()
            } else {
                "string".to_string()
            }
        }
        Value::Array(arr) => {
            if arr.is_empty() {
                "[]".to_string()
            } else {
                let first_schema = extract_schema(&arr[0], depth + 1, max_depth);
                if arr.len() == 1 {
                    format!("[{}]", first_schema)
                } else {
                    format!("[{}] ({})", first_schema, arr.len())
                }
            }
        }
        Value::Object(map) => {
            if map.is_empty() {
                "{}".to_string()
            } else {
                let mut keys: Vec<_> = map.keys().collect();
                keys.sort();

                let mut parts = Vec::new();
                for (i, key) in keys.iter().enumerate() {
                    if i >= MAX_SCHEMA_KEYS {
                        parts.push(format!("...+{}", keys.len() - i));
                        break;
                    }
                    let val_schema = extract_schema(&map[*key], depth + 1, max_depth);
                    parts.push(format!("{}:{}", key, val_schema));
                }
                format!("{{{}}}", parts.join(","))
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

    #[test]
    fn test_keys_only_never_prints_values() {
        // `--keys-only` promises to strip values, but never_worse used to hand
        // the raw JSON back whenever the schema was longer — printing the very
        // values the flag exists to hide. Short values make that the common
        // case, so a small config or credential file leaked while a large one
        // did not. Assert on quotes: extract_schema emits keys unquoted, so a
        // `"` anywhere in the output can only have come from a raw value.
        let short_values = format!(
            "{{{}}}",
            (0..11)
                .map(|i| format!("\"k{i}\":\"v{i}\""))
                .collect::<Vec<_>>()
                .join(",")
        );

        for raw in [r#"{"a":1,"b":"secretvalue","c":3}"#, short_values.as_str()] {
            let shown = render_json(raw, 5, true).expect("must render");
            assert!(
                !shown.contains('"'),
                "keys-only output quoted a value: {shown:?}"
            );
            assert!(
                shown.contains("string"),
                "expected a schema, got: {shown:?}"
            );
        }
    }

    #[test]
    fn test_keys_only_schema_is_no_longer_than_input() {
        // `--keys-only` always prints the schema, so it must not cost more
        // than the minified input it replaces. The old indented layout did:
        // leak.json 31 → 36 B and the 4-level chain 30 → 79 B.
        for raw in [
            r#"{"a":1,"b":"secretvalue","c":3}"#,
            r#"{"a":{"b":{"c":{"d":"deep"}}}}"#,
        ] {
            let shown = render_json(raw, 5, true).expect("must render");
            assert!(
                shown.len() <= raw.len(),
                "schema is {} B for a {} B input: {shown:?}",
                shown.len(),
                raw.len()
            );
            assert!(
                !shown.contains('"'),
                "keys-only output quoted a value: {shown:?}"
            );
        }
    }

    #[test]
    fn test_keys_only_schema_beats_real_captures() {
        for raw in [
            include_str!("../../../tests/fixtures/glab_mr_list_raw.json"),
            include_str!("../../../tests/fixtures/ast_grep_lazylock_raw.json"),
        ] {
            let shown = render_json(raw, 5, true).expect("must render");
            assert!(
                shown.len() * 2 < raw.len(),
                "schema is {} B for a {} B capture",
                shown.len(),
                raw.len()
            );
            assert!(
                !shown.contains('"'),
                "keys-only output quoted a value: {shown:?}"
            );
        }
    }

    #[test]
    fn test_keys_only_key_cap_never_reports_zero() {
        let make = |n: usize| {
            format!(
                "{{{}}}",
                (0..n)
                    .map(|i| format!("\"k{i:02}\":1"))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };

        let sixteen = filter_json_string(&make(16), 5).unwrap();
        assert_eq!(sixteen.matches(":int").count(), 16, "{sixteen:?}");
        assert!(
            !sixteen.contains("+0"),
            "16-key object reported an empty cap: {sixteen:?}"
        );

        let seventeen = filter_json_string(&make(17), 5).unwrap();
        assert_eq!(seventeen.matches(":int").count(), 16, "{seventeen:?}");
        assert!(
            seventeen.contains("...+1"),
            "17-key object should show 16 keys plus +1: {seventeen:?}"
        );
    }
}
