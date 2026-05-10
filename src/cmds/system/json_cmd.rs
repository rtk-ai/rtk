//! Inspects JSON structure without showing values, saving tokens on large payloads.

use crate::core::content_hint::save_output_and_hint;
use crate::core::tracking;
use anyhow::{bail, Context, Result};
use serde_json::Value;
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

    let (output, truncated) = if schema_only {
        filter_json_schema(&content, max_depth).map(|s| (s, false))?
    } else {
        filter_json_compact(&content, max_depth)?
    };

    println!("{}", output);

    if truncated {
        let value: Value = serde_json::from_str(&content).context("Failed to parse JSON")?;
        if let Ok(full_json) = serde_json::to_string(&value) {
            if let Some(hint) = save_output_and_hint(&full_json, "json", ".json") {
                println!("{}", hint);
            }
        }
    }

    timer.track(
        &format!("cat {}", file.display()),
        "rtk json",
        &content,
        &output,
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

    let (output, truncated) = if schema_only {
        filter_json_schema(&content, max_depth).map(|s| (s, false))?
    } else {
        filter_json_compact(&content, max_depth)?
    };

    println!("{}", output);

    if truncated {
        let value: Value = serde_json::from_str(&content).context("Failed to parse JSON")?;
        if let Ok(full_json) = serde_json::to_string(&value) {
            if let Some(hint) = save_output_and_hint(&full_json, "json", ".json") {
                println!("{}", hint);
            }
        }
    }

    timer.track("cat - (stdin)", "rtk json -", &content, &output);
    Ok(())
}

/// Parse a JSON string and return compact representation with values preserved.
/// Long strings are truncated, arrays are summarized.
pub fn filter_json_compact(json_str: &str, max_depth: usize) -> Result<(String, bool)> {
    let value: Value = serde_json::from_str(json_str).context("Failed to parse JSON")?;
    let mut truncated = false;
    let output = compact_json(&value, 0, max_depth, &mut truncated);
    Ok((output, truncated))
}

const STRING_CHARS_LIMIT: usize = 80;

/// Compact JSON output - single-line with truncation tracking.
fn compact_json(value: &Value, depth: usize, max_depth: usize, truncated: &mut bool) -> String {
    if depth > max_depth {
        *truncated = true;
        return "…".to_string();
    }

    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => {
            if s.chars().count() > STRING_CHARS_LIMIT {
                *truncated = true;
                let truncated_str: String = s.chars().take(STRING_CHARS_LIMIT - 1).collect();
                format!(r#""{}…""#, truncated_str)
            } else {
                format!(r#""{}""#, s)
            }
        }
        Value::Array(arr) => {
            if arr.is_empty() {
                "[]".to_string()
            } else if arr.len() > 5 {
                *truncated = true;
                let first = compact_json(&arr[0], depth + 1, max_depth, truncated);
                format!("[{}, … +{} more]", first, arr.len() - 1)
            } else {
                let items: Vec<String> = arr
                    .iter()
                    .map(|v| compact_json(v, depth + 1, max_depth, truncated))
                    .collect();
                format!("[{}]", items.join(","))
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
                    if i >= 20 {
                        *truncated = true;
                        parts.push(format!("… +{} more keys", keys.len() - i));
                        break;
                    }
                    let val = &map[*key];
                    let val_str = compact_json(val, depth + 1, max_depth, truncated);
                    parts.push(format!(r#""{}":{}"#, key, val_str));
                }
                format!("{{{}}}", parts.join(","))
            }
        }
    }
}

/// Parse a JSON string and return its schema representation (types only, no values).
/// Useful for piping JSON from other commands (e.g., `gh api`, `curl`).
pub fn filter_json_schema(json_str: &str, max_depth: usize) -> Result<String> {
    let value: Value = serde_json::from_str(json_str).context("Failed to parse JSON")?;
    Ok(extract_schema(&value, 0, max_depth))
}

/// Schema extraction - single-line output with char-based truncation.
fn extract_schema(value: &Value, depth: usize, max_depth: usize) -> String {
    if depth > max_depth {
        return "…".to_string();
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
            if s.chars().count() > 50 {
                format!("string[{}]", s.chars().count())
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
                    if i >= 15 {
                        parts.push(format!("… +{} more keys", keys.len() - i));
                        break;
                    }
                    let val = &map[*key];
                    let val_schema = extract_schema(val, depth + 1, max_depth);
                    parts.push(format!(r#""{}":{}"#, key, val_schema));
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
    fn test_filter_json_compact_single_line() {
        let (output, truncated) = filter_json_compact(
            r#"{
              "name": "test",
              "count": 42
            }"#,
            5,
        )
        .unwrap();
        assert!(output.lines().count() == 1);
        assert!(output.contains("\"name\":\"test\""));
        assert!(output.contains("\"count\":42"));
        assert!(!truncated);
    }

    #[test]
    fn test_filter_json_compact_string_limit() {
        let long_string = "a".repeat(STRING_CHARS_LIMIT);
        let json = format!(r#"{{"name": "{}"}}"#, long_string);
        let (output, truncated) = filter_json_compact(&json, 5).unwrap();
        assert!(!output.contains('…'));
        assert!(!truncated);
    }

    #[test]
    fn test_filter_json_compact_string_truncation() {
        let long_string = "a".repeat(STRING_CHARS_LIMIT + 20);
        let json = format!(r#"{{"name": "{}"}}"#, long_string);
        let (output, truncated) = filter_json_compact(&json, 5).unwrap();
        assert!(output.contains('…'));
        assert!(truncated);
    }

    #[test]
    fn test_filter_json_compact_depth_truncation() {
        let json = r#"{"a": {"b": {"c": 1}}}"#;
        let (output, truncated) = filter_json_compact(json, 1).unwrap();
        assert!(output.contains('…'));
        assert!(truncated);
    }

    #[test]
    fn test_filter_json_compact_array_truncation() {
        let json = r#"{"items": [1, 2, 3, 4, 5, 6]}"#;
        let (output, truncated) = filter_json_compact(json, 5).unwrap();
        assert!(output.contains("… +5 more"));
        assert!(truncated);
    }

    #[test]
    fn test_extract_schema_single_line() {
        let json: Value = serde_json::from_str(
            r#"{
                "name": "test",
                "count": 42
            }"#,
        )
        .unwrap();
        let schema = extract_schema(&json, 0, 5);
        assert!(schema.lines().count() == 1);
        assert!(schema.contains("\"name\":string"));
        assert!(schema.contains("\"count\":int"));
    }

    #[test]
    fn test_extract_schema_array() {
        let json: Value = serde_json::from_str(r#"{"items": [1, 2, 3]}"#).unwrap();
        let schema = extract_schema(&json, 0, 5);
        assert!(schema.lines().count() == 1);
        assert!(schema.contains("[int] (3)"));
    }

    #[test]
    fn test_extract_schema_utf8_truncation() {
        let long_string = "a".repeat(100);
        let json = format!(r#"{{"name": "{}"}}"#, long_string);
        let value: Value = serde_json::from_str(&json).unwrap();
        let schema = extract_schema(&value, 0, 5);
        assert!(schema.contains("string[100]"));
    }

    fn assert_value_truncated(payload: &str) {
        let json = format!(r#"{{"key":"{}"}}"#, payload);
        let (output, truncated) = filter_json_compact(&json, 5)
            .expect("filter_json_compact must not error on valid JSON");

        let value: Value = serde_json::from_str(output.as_str()).expect("Failed to parse JSON");
        let s = value
            .get("key")
            .and_then(|val| val.as_str())
            .expect("Output JSON should contain 'key' as a string");

        assert!(
            truncated,
            "Expected truncation for payload of length {}, got: {}",
            payload.len(),
            output
        );
        assert!(
            s.chars().count() == STRING_CHARS_LIMIT,
            "Truncated string should be {} chars, got {}: {}",
            STRING_CHARS_LIMIT,
            s.chars().count(),
            s
        );
        assert!(
            s.ends_with("…"),
            "Truncated string should end with '…', got: {}",
            s
        )
    }

    #[test]
    fn test_compact_truncates_pure_multibyte_string() {
        assert_value_truncated(&"日本語テスト".repeat(85));
    }

    #[test]
    fn test_compact_truncates_mixed_ascii_multibyte_string() {
        assert_value_truncated(&("a".repeat(76) + &"日本語".repeat(5)));
    }
}
