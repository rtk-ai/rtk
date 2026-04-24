//! TOON — Token-Optimized Object Notation.
//! Lossless compact JSON encoding: simple alphanumeric keys lose their quotes,
//! no whitespace around separators, null fields stripped upstream.

const TOON_PREFIX: &str = "TOON:";
const MIN_INPUT_LEN: usize = 50;

/// Strip null fields recursively from a JSON value.
/// Returns None if input is not valid JSON.
pub fn strip_nulls(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let mut value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    remove_nulls_recursive(&mut value);
    serde_json::to_string(&value).ok()
}

fn remove_nulls_recursive(v: &mut serde_json::Value) {
    if let serde_json::Value::Object(map) = v {
        map.retain(|_, val| !val.is_null());
        for child in map.values_mut() {
            remove_nulls_recursive(child);
        }
    }
    if let serde_json::Value::Array(arr) = v {
        for item in arr.iter_mut() {
            remove_nulls_recursive(item);
        }
    }
}

/// Encode JSON to compact TOON notation.
/// Returns None if input is not valid JSON or is shorter than 50 chars.
pub fn toon_encode(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.len() < MIN_INPUT_LEN {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let mut buf = String::with_capacity(trimmed.len() / 2);
    buf.push_str(TOON_PREFIX);
    encode_value(&value, &mut buf);
    Some(buf)
}

fn encode_value(v: &serde_json::Value, buf: &mut String) {
    match v {
        serde_json::Value::Null => buf.push_str("null"),
        serde_json::Value::Bool(b) => buf.push_str(if *b { "true" } else { "false" }),
        serde_json::Value::Number(n) => buf.push_str(&n.to_string()),
        serde_json::Value::String(s) => encode_string(s, buf),
        serde_json::Value::Array(arr) => {
            buf.push('[');
            for (i, item) in arr.iter().enumerate() {
                if i > 0 {
                    buf.push(',');
                }
                encode_value(item, buf);
            }
            buf.push(']');
        }
        serde_json::Value::Object(map) => {
            buf.push('{');
            for (i, (k, val)) in map.iter().enumerate() {
                if i > 0 {
                    buf.push(',');
                }
                encode_key(k, buf);
                buf.push(':');
                encode_value(val, buf);
            }
            buf.push('}');
        }
    }
}

fn encode_key(key: &str, buf: &mut String) {
    let is_simple = !key.is_empty()
        && key
            .chars()
            .next()
            .map(|c| !c.is_ascii_digit())
            .unwrap_or(false)
        && key.chars().all(|c| c.is_alphanumeric() || c == '_');
    if is_simple {
        buf.push_str(key);
    } else {
        encode_string(key, buf);
    }
}

fn encode_string(s: &str, buf: &mut String) {
    buf.push('"');
    for c in s.chars() {
        match c {
            '"' => buf.push_str("\\\""),
            '\\' => buf.push_str("\\\\"),
            '\n' => buf.push_str("\\n"),
            '\r' => buf.push_str("\\r"),
            '\t' => buf.push_str("\\t"),
            c if (c as u32) < 0x20 => buf.push_str(&format!("\\u{:04x}", c as u32)),
            c => buf.push(c),
        }
    }
    buf.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_keys_no_quotes() {
        let json =
            r#"{"id":42,"name":"Alice","email":"alice@example.com","role":"admin","active":true}"#;
        let toon = toon_encode(json).unwrap();
        assert!(toon.starts_with("TOON:"), "got: {toon}");
        assert!(
            toon.contains("id:42"),
            "simple key should have no quotes: {toon}"
        );
        assert!(
            toon.contains(r#"name:"Alice""#),
            "string value keeps quotes: {toon}"
        );
    }

    #[test]
    fn test_short_json_returns_none() {
        assert!(
            toon_encode(r#"{"id":1}"#).is_none(),
            "< 50 chars should return None"
        );
    }

    #[test]
    fn test_strip_nulls_removes_null_fields() {
        let json = r#"{"id":1,"name":"Alice","deleted_at":null,"internal_id":null,"role":"admin"}"#;
        let stripped = strip_nulls(json).unwrap();
        assert!(!stripped.contains("null"));
        assert!(stripped.contains("Alice"));
        assert!(stripped.contains("admin"));
    }

    #[test]
    fn test_non_json_returns_none() {
        assert!(toon_encode("not json at all, just a plain text string here").is_none());
        assert!(strip_nulls("not json").is_none());
    }

    #[test]
    fn test_toon_produces_valid_roundtrip_concept() {
        let json = r#"{"id":42,"email":"alice@example.com","active":true,"score":98.6,"tags":["rust","cli"],"description":"token killer proxy"}"#;
        let toon = toon_encode(json).unwrap();
        assert!(toon.contains("42"), "id value: {toon}");
        assert!(toon.contains("alice@example.com"), "email: {toon}");
        assert!(toon.contains("98.6"), "score: {toon}");
    }

    #[test]
    fn test_token_savings_on_api_fixture() {
        let json = include_str!("../../tests/fixtures/api_response.json");
        if json.len() < 50 {
            return;
        }
        let cleaned = strip_nulls(json).unwrap_or_else(|| json.to_string());
        let toon = toon_encode(&cleaned).unwrap_or(cleaned.clone());
        // TOON removes quotes from simple alphanumeric keys → character savings even on compact JSON
        let savings = 100.0 * (1.0 - toon.len() as f64 / json.len() as f64);
        assert!(
            savings >= 3.0,
            "Expected ≥3% char savings on API fixture, got {savings:.1}%"
        );
    }
}
