//! Shared helpers for Salesforce CLI (`sf`) JSON envelope filtering.

use crate::core::truncate::{CAP_ERRORS, CAP_WARNINGS};
use serde_json::{json, Value};

pub const MAX_FAILURES: usize = CAP_ERRORS;
pub const MAX_MESSAGES: usize = CAP_WARNINGS;
const MAX_STACK_LINES: usize = 5;

#[derive(Debug, Clone, Copy)]
pub struct FilterOptions {
    pub ultra_compact: bool,
}

#[derive(Debug)]
pub struct FilterOutcome {
    pub text: String,
    pub truncated: bool,
    /// When true, emit raw output unchanged (async apex job id only, parse failure).
    pub passthrough: bool,
}

impl FilterOutcome {
    pub fn passthrough(text: String) -> Self {
        Self {
            text,
            truncated: false,
            passthrough: true,
        }
    }

    pub fn ok(text: String) -> Self {
        Self {
            text,
            truncated: false,
            passthrough: false,
        }
    }

    pub fn truncated(text: String) -> Self {
        Self {
            text,
            truncated: true,
            passthrough: false,
        }
    }
}

pub fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag || a.starts_with(&format!("{flag}=")))
}

pub fn truncate_stack_trace(stack: &str) -> String {
    let lines: Vec<&str> = stack.lines().collect();
    if lines.len() <= MAX_STACK_LINES {
        return stack.to_string();
    }
    let head: Vec<&str> = lines.into_iter().take(MAX_STACK_LINES).collect();
    format!("{}\n  …", head.join("\n"))
}

pub fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

pub fn parse_envelope(raw: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(raw.trim())
}

/// True when apex result is an async job handle with no test payload yet.
pub fn is_async_apex_result(result: &Value) -> bool {
    let obj = match result.as_object() {
        Some(o) => o,
        None => return false,
    };
    if obj.len() != 1 {
        return false;
    }
    obj.get("testRunId")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty())
}

pub fn envelope_status(envelope: &Value) -> Option<i64> {
    envelope.get("status").and_then(Value::as_i64)
}

pub fn envelope_result(envelope: &Value) -> Option<&Value> {
    envelope.get("result")
}

pub fn take_array_cap(items: &[Value], cap: usize) -> (Vec<Value>, bool) {
    if items.len() <= cap {
        return (items.to_vec(), false);
    }
    (items.iter().take(cap).cloned().collect(), true)
}

pub fn summarize_file_properties(props: &[Value]) -> Value {
    let mut types = std::collections::BTreeSet::new();
    for prop in props {
        if let Some(t) = prop.get("type").and_then(Value::as_str) {
            types.insert(t.to_string());
        }
    }
    json!({
        "count": props.len(),
        "types": types.into_iter().collect::<Vec<_>>(),
    })
}

pub fn slim_component_failure(item: &Value) -> Value {
    json!({
        "componentType": item.get("componentType"),
        "fullName": item.get("fullName"),
        "problem": item.get("problem"),
        "problemType": item.get("problemType"),
        "lineNumber": item.get("lineNumber"),
        "columnNumber": item.get("columnNumber"),
        "fileName": item.get("fileName"),
    })
}

pub fn slim_test_failure(item: &Value) -> Value {
    let stack = item
        .get("stackTrace")
        .or_else(|| item.get("StackTrace"))
        .and_then(Value::as_str)
        .map(truncate_stack_trace)
        .unwrap_or_default();

    json!({
        "name": item.get("name").or_else(|| item.get("FullName")),
        "methodName": item.get("methodName").or_else(|| item.get("MethodName")),
        "message": item.get("message").or_else(|| item.get("Message")),
        "stackTrace": if stack.is_empty() { Value::Null } else { Value::String(stack) },
    })
}

pub fn slim_apex_test_failure(item: &Value) -> Value {
    let stack = item
        .get("StackTrace")
        .and_then(Value::as_str)
        .map(truncate_stack_trace)
        .unwrap_or_default();

    json!({
        "FullName": item.get("FullName"),
        "MethodName": item.get("MethodName"),
        "Outcome": item.get("Outcome"),
        "Message": item.get("Message"),
        "StackTrace": if stack.is_empty() { Value::Null } else { Value::String(stack) },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_flag_detects_exact_and_equals_form() {
        let args = vec!["--json".to_string(), "--wait=30".to_string()];
        assert!(has_flag(&args, "--json"));
        assert!(has_flag(&args, "--wait"));
        assert!(!has_flag(&args, "--concise"));
    }

    #[test]
    fn is_async_apex_result_true_for_lone_test_run_id() {
        let result = json!({ "testRunId": "707xx000000abc" });
        assert!(is_async_apex_result(&result));
    }

    #[test]
    fn is_async_apex_result_false_when_summary_present() {
        let result = json!({
            "testRunId": "707xx000000abc",
            "summary": { "outcome": "Passed" }
        });
        assert!(!is_async_apex_result(&result));
    }

    #[test]
    fn truncate_stack_trace_caps_lines() {
        let stack = (1..=10)
            .map(|i| format!("Class.test: line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = truncate_stack_trace(&stack);
        assert!(out.contains("line 5"));
        assert!(out.contains("…"));
        assert!(!out.contains("line 10"));
    }
}
