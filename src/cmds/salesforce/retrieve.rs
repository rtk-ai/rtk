//! Filter for `sf project retrieve start --json` output.

use super::common::{
    compact_json, envelope_result, envelope_status, parse_envelope, summarize_file_properties,
    take_array_cap, FilterOptions, FilterOutcome, MAX_MESSAGES,
};
use serde_json::{json, Map, Value};

pub fn filter_retrieve(raw: &str, opts: FilterOptions) -> FilterOutcome {
    let envelope = match parse_envelope(raw) {
        Ok(v) => v,
        Err(_) => return FilterOutcome::passthrough(raw.to_string()),
    };

    if opts.ultra_compact {
        return FilterOutcome::ok(format_retrieve_text(&envelope));
    }

    let mut out = envelope.clone();
    let mut truncated = false;

    if let Some(result) = out.get_mut("result").and_then(Value::as_object_mut) {
        truncated = slim_retrieve_result(result);
    }

    if truncated {
        FilterOutcome::truncated(compact_json(&out))
    } else {
        FilterOutcome::ok(compact_json(&out))
    }
}

fn slim_retrieve_result(result: &mut Map<String, Value>) -> bool {
    for key in ["zipFile", "zipFilePath"] {
        result.remove(key);
    }

    let mut truncated = false;

    if let Some(props) = result.get("fileProperties").and_then(Value::as_array) {
        result.insert(
            "filePropertiesSummary".to_string(),
            summarize_file_properties(props),
        );
        result.remove("fileProperties");
    }

    if let Some(files) = result.get("files").and_then(Value::as_array) {
        let problems: Vec<Value> = files
            .iter()
            .filter(|f| {
                f.get("state")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.eq_ignore_ascii_case("Changed"))
            })
            .cloned()
            .collect();
        if problems.is_empty() {
            result.insert("filesCount".to_string(), json!(files.len()));
            result.remove("files");
        } else {
            result.insert("files".to_string(), Value::Array(problems));
        }
    }

    if let Some(messages) = result.get("messages").and_then(Value::as_array) {
        let total_messages = messages.len();
        let (kept, was_truncated) = take_array_cap(messages, MAX_MESSAGES);
        truncated |= was_truncated;
        result.insert("messages".to_string(), Value::Array(kept));
        if was_truncated {
            result.insert(
                "messagesTruncated".to_string(),
                json!(total_messages - MAX_MESSAGES),
            );
        }
    }

    truncated
}

fn format_retrieve_text(envelope: &Value) -> String {
    let status = envelope_status(envelope).unwrap_or(-1);
    let result = envelope_result(envelope);
    let success = result
        .and_then(|r| r.get("success"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let retrieve_status = result
        .and_then(|r| r.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("Unknown");

    let file_count = result
        .and_then(|r| r.get("fileProperties"))
        .and_then(Value::as_array)
        .map(|a| a.len())
        .or_else(|| {
            result
                .and_then(|r| r.get("files"))
                .and_then(Value::as_array)
                .map(|a| a.len())
        })
        .or_else(|| {
            result
                .and_then(|r| r.get("filesCount"))
                .and_then(Value::as_u64)
                .map(|n| n as usize)
        })
        .unwrap_or(0);

    let mut lines = vec![format!(
        "sf retrieve: status={status} {retrieve_status} success={success} files={file_count}"
    )];

    if let Some(messages) = result.and_then(|r| r.get("messages")).and_then(Value::as_array) {
        for msg in messages.iter().take(MAX_MESSAGES) {
            let problem = msg.get("problem").and_then(Value::as_str).unwrap_or("?");
            let file = msg
                .get("fileName")
                .and_then(Value::as_str)
                .unwrap_or("?");
            lines.push(format!("  {file}: {problem}"));
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::utils::count_tokens;

    #[test]
    fn retrieve_success_verbose_reduces_tokens() {
        let raw = include_str!("../../../tests/fixtures/salesforce/retrieve_success.json");
        let out = filter_retrieve(raw, FilterOptions { ultra_compact: false });
        assert!(!out.passthrough);
        assert!(out.text.contains("filePropertiesSummary"));
        assert!(!out.text.contains("createdById"));
        assert!(!out.text.contains("/Users/"));
        let savings =
            100.0 - (count_tokens(&out.text) as f64 / count_tokens(raw) as f64 * 100.0);
        assert!(
            savings >= 20.0,
            "expected >=20% savings, got {savings:.1}%"
        );
    }

    #[test]
    fn retrieve_ultra_compact_text_mode() {
        let raw = include_str!("../../../tests/fixtures/salesforce/retrieve_success.json");
        let out = filter_retrieve(raw, FilterOptions { ultra_compact: true });
        assert!(out.text.starts_with("sf retrieve:"));
    }
}
