//! Filter for `sf apex run test --json` output.

use super::common::{
    compact_json, envelope_result, is_async_apex_result, parse_envelope, slim_apex_test_failure,
    FilterOptions, FilterOutcome, MAX_FAILURES,
};
use crate::parser::{FormatMode, TestFailure, TestResult, TokenFormatter};
use serde_json::{json, Map, Value};

const COVERAGE_THRESHOLD: u64 = 75;

pub fn filter_apex_test(raw: &str, opts: FilterOptions) -> FilterOutcome {
    let envelope = match parse_envelope(raw) {
        Ok(v) => v,
        Err(_) => return FilterOutcome::passthrough(raw.to_string()),
    };

    let result = match envelope_result(&envelope) {
        Some(r) => r,
        None => return FilterOutcome::passthrough(raw.to_string()),
    };

    if is_async_apex_result(result) {
        return FilterOutcome::passthrough(raw.to_string());
    }

    if opts.ultra_compact {
        return FilterOutcome::ok(format_apex_text(result));
    }

    let mut out = envelope.clone();
    if let Some(result_obj) = out.get_mut("result").and_then(Value::as_object_mut) {
        slim_apex_result(result_obj);
    }

    FilterOutcome::ok(compact_json(&out))
}

fn slim_apex_result(result: &mut Map<String, Value>) {
    for key in ["hostname", "orgId", "userId", "username"] {
        result.remove(key);
    }

    if let Some(summary) = result.get_mut("summary").and_then(Value::as_object_mut) {
        for key in [
            "hostname",
            "orgId",
            "userId",
            "username",
            "commandTime",
            "testStartTime",
        ] {
            summary.remove(key);
        }
    }

    if let Some(tests) = result.get("tests").and_then(Value::as_array) {
        let total_tests = tests.len();
        let failed: Vec<Value> = tests
            .iter()
            .filter(|t| {
                t.get("Outcome")
                    .and_then(Value::as_str)
                    .is_some_and(|o| !o.eq_ignore_ascii_case("Pass"))
            })
            .map(slim_apex_test_failure)
            .collect();
        let failed_count = failed.len();
        if failed.is_empty() {
            result.remove("tests");
            result.insert(
                "testsSummary".to_string(),
                json!({ "total": total_tests, "failed": 0 }),
            );
        } else {
            result.insert("tests".to_string(), Value::Array(failed));
            result.insert(
                "testsSummary".to_string(),
                json!({ "total": total_tests, "failed": failed_count }),
            );
        }
    }

    if let Some(coverage) = result.get_mut("coverage").and_then(Value::as_object_mut) {
        coverage.remove("records");

        if let Some(classes) = coverage.get("coverage").and_then(Value::as_array) {
            let low: Vec<Value> = classes
                .iter()
                .filter_map(|c| {
                    let pct = c.get("coveredPercent").and_then(Value::as_u64)?;
                    if pct >= COVERAGE_THRESHOLD {
                        return None;
                    }
                    Some(json!({
                        "name": c.get("name"),
                        "coveredPercent": pct,
                    }))
                })
                .collect();
            coverage.insert("lowCoverageClasses".to_string(), Value::Array(low));
            coverage.remove("coverage");
        }
    }
}

fn format_apex_text(result: &Value) -> String {
    let test_result = apex_to_test_result(result);
    test_result.format(FormatMode::Compact)
}

fn apex_to_test_result(result: &Value) -> TestResult {
    let summary = result.get("summary");
    let total = summary
        .and_then(|s| s.get("testsRan"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let passed = summary
        .and_then(|s| s.get("passing"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let failed = summary
        .and_then(|s| s.get("failing"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let skipped = summary
        .and_then(|s| s.get("skipped"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;

    let failures: Vec<TestFailure> = result
        .get("tests")
        .and_then(Value::as_array)
        .map(|tests| {
            tests
                .iter()
                .filter(|t| {
                    t.get("Outcome")
                        .and_then(Value::as_str)
                        .is_some_and(|o| !o.eq_ignore_ascii_case("Pass"))
                })
                .take(MAX_FAILURES)
                .map(|t| TestFailure {
                    test_name: t
                        .get("FullName")
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                        .to_string(),
                    file_path: t
                        .get("ApexClass")
                        .and_then(|c| c.get("Name"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    error_message: t
                        .get("Message")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    stack_trace: t
                        .get("StackTrace")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
                .collect()
        })
        .unwrap_or_default();

    TestResult {
        total,
        passed,
        failed,
        skipped,
        duration_ms: None,
        failures,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::utils::count_tokens;

    #[test]
    fn apex_pass_with_coverage_reduces_tokens() {
        let raw = include_str!("../../../tests/fixtures/salesforce/apex_test_pass_with_coverage.json");
        let out = filter_apex_test(raw, FilterOptions { ultra_compact: false });
        assert!(!out.passthrough);
        assert!(out.text.contains("lowCoverageClasses"));
        assert!(!out.text.contains("\"lines\""));
        assert!(!out.text.contains("test-user@example.com"));
        assert!(!out.text.contains("scratch.example"));
        let savings =
            100.0 - (count_tokens(&out.text) as f64 / count_tokens(raw) as f64 * 100.0);
        assert!(
            savings >= 20.0,
            "expected >=20% savings, got {savings:.1}%"
        );
    }

    #[test]
    fn apex_failed_reduces_tokens() {
        let raw = include_str!("../../../tests/fixtures/salesforce/apex_test_failed.json");
        let out = filter_apex_test(raw, FilterOptions { ultra_compact: false });
        assert!(!out.passthrough);
        assert!(out.text.contains("AccountServiceTest"));
        assert!(!out.text.contains("\"RunTime\""));
        assert!(!out.text.contains("\"lines\""));
        let savings =
            100.0 - (count_tokens(&out.text) as f64 / count_tokens(raw) as f64 * 100.0);
        assert!(
            savings >= 20.0,
            "expected >=20% savings, got {savings:.1}%"
        );
    }

    #[test]
    fn apex_async_passthrough() {
        let raw = include_str!("../../../tests/fixtures/salesforce/apex_test_async.json");
        let out = filter_apex_test(raw, FilterOptions { ultra_compact: false });
        assert!(out.passthrough);
    }

    #[test]
    fn apex_ultra_compact_text_mode() {
        let raw = include_str!("../../../tests/fixtures/salesforce/apex_test_failed.json");
        let out = filter_apex_test(raw, FilterOptions { ultra_compact: true });
        assert!(out.text.contains("FAIL"));
    }
}
