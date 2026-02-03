use crate::tracking;
use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

/// Show JSON structure without values
pub fn run(file: &Path, max_depth: usize, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Analyzing JSON: {}", file.display());
    }

    let content = fs::read_to_string(file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let schema = filter_json_string(&content, max_depth)?;
    println!("{}", schema);
    timer.track(
        &format!("cat {}", file.display()),
        "rtk json",
        &content,
        &schema,
    );
    Ok(())
}

/// Show JSON structure from stdin
pub fn run_stdin(max_depth: usize, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Analyzing JSON from stdin");
    }

    let mut content = String::new();
    io::stdin()
        .lock()
        .read_to_string(&mut content)
        .context("Failed to read from stdin")?;

    let schema = filter_json_string(&content, max_depth)?;
    println!("{}", schema);
    timer.track("cat - (stdin)", "rtk json -", &content, &schema);
    Ok(())
}

/// Parse a JSON string and return its schema representation.
/// Useful for piping JSON from other commands (e.g., `gh api`, `curl`).
pub fn filter_json_string(json_str: &str, max_depth: usize) -> Result<String> {
    let value: Value = serde_json::from_str(json_str).context("Failed to parse JSON")?;
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
}
