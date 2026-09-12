//! Filter for `sf project deploy start --json` output.

use super::common::{
    compact_json, envelope_result, envelope_status, parse_envelope, slim_component_failure,
    slim_test_failure, take_array_cap, FilterOptions, FilterOutcome, MAX_FAILURES,
};
use serde_json::{json, Map, Value};

pub fn filter_deploy(raw: &str, opts: FilterOptions) -> FilterOutcome {
    let envelope = match parse_envelope(raw) {
        Ok(v) => v,
        Err(_) => return FilterOutcome::passthrough(raw.to_string()),
    };

    if opts.ultra_compact {
        return FilterOutcome::ok(format_deploy_text(&envelope));
    }

    let mut out = envelope.clone();
    let mut truncated = false;
    if let Some(result) = out.get_mut("result").and_then(Value::as_object_mut) {
        truncated = slim_deploy_result(result);
    }
    if let Some(obj) = out.as_object_mut() {
        obj.remove("stack");
    }

    if truncated {
        FilterOutcome::truncated(compact_json(&out))
    } else {
        FilterOutcome::ok(compact_json(&out))
    }
}

fn slim_deploy_result(result: &mut Map<String, Value>) -> bool {
    let mut truncated = false;
    strip_deploy_noise(result);

    if let Some(details) = result.get_mut("details").and_then(Value::as_object_mut) {
        details.remove("componentSuccesses");

        if let Some(failures) = details.get("componentFailures").and_then(Value::as_array) {
            let total_failures = failures.len();
            let (slim, was_truncated) = take_array_cap(failures, MAX_FAILURES);
            truncated |= was_truncated;
            let mapped: Vec<Value> = slim.iter().map(slim_component_failure).collect();
            details.insert("componentFailures".to_string(), Value::Array(mapped));
            if was_truncated {
                details.insert(
                    "componentFailuresTruncated".to_string(),
                    json!(total_failures - MAX_FAILURES),
                );
            }
        }

        if let Some(run_test) = details.get_mut("runTestResult").and_then(Value::as_object_mut) {
            slim_run_test_result(run_test);
        }
    }

    if let Some(files) = result.get("files").and_then(Value::as_array) {
        let file_count = files.len();
        let failed: Vec<Value> = files
            .iter()
            .filter(|f| {
                f.get("state")
                    .and_then(Value::as_str)
                    .is_some_and(|s| s.eq_ignore_ascii_case("Failed"))
            })
            .cloned()
            .collect();
        if failed.is_empty() {
            result.remove("files");
            result.insert("filesCount".to_string(), json!(file_count));
        } else {
            result.insert("files".to_string(), Value::Array(failed));
        }
    }

    truncated
}

fn strip_deploy_noise(result: &mut Map<String, Value>) {
    for key in [
        "zipSize",
        "zipFileCount",
        "deployUrl",
        "createdBy",
        "createdById",
        "createdByName",
        "createdDate",
        "lastModifiedDate",
        "startDate",
        "completedDate",
        "rollbackOnError",
        "ignoreWarnings",
        "checkOnly",
        "numberFiles",
        "numberTestErrors",
        "runTestsEnabled",
    ] {
        result.remove(key);
    }
}

fn slim_run_test_result(run_test: &mut Map<String, Value>) {
    run_test.remove("successes");
    run_test.remove("codeCoverage");

    if let Some(failures) = run_test.get("failures").and_then(Value::as_array) {
        let mapped: Vec<Value> = failures.iter().map(slim_test_failure).collect();
        run_test.insert("failures".to_string(), Value::Array(mapped));
    }

    if let Some(warnings) = run_test.get("codeCoverageWarnings").and_then(Value::as_array) {
        let slim: Vec<Value> = warnings
            .iter()
            .map(|w| {
                json!({
                    "name": w.get("name"),
                    "message": w.get("message"),
                })
            })
            .collect();
        run_test.insert("codeCoverageWarnings".to_string(), Value::Array(slim));
    }
}

fn format_deploy_text(envelope: &Value) -> String {
    let status = envelope_status(envelope).unwrap_or(-1);
    let result = envelope_result(envelope);
    let success = result
        .and_then(|r| r.get("success"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let deploy_status = result
        .and_then(|r| r.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("Unknown");
    let total = result
        .and_then(|r| r.get("numberComponentsTotal"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let errors = result
        .and_then(|r| r.get("numberComponentErrors"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let test_failures = result
        .and_then(|r| r.get("numberTestsFailed"))
        .and_then(Value::as_u64)
        .unwrap_or(0);

    let mut lines = vec![format!(
        "sf deploy: status={status} {deploy_status} success={success} components={total} componentErrors={errors} testFailures={test_failures}"
    )];

    if let Some(details) = result.and_then(|r| r.get("details")) {
        if let Some(failures) = details.get("componentFailures").and_then(Value::as_array) {
            for f in failures.iter().take(MAX_FAILURES) {
                let name = f
                    .get("fullName")
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                let problem = f
                    .get("problem")
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                lines.push(format!("  {name}: {problem}"));
            }
        }
        if let Some(run_test) = details.get("runTestResult") {
            if let Some(failures) = run_test.get("failures").and_then(Value::as_array) {
                for f in failures.iter().take(MAX_FAILURES) {
                    let name = f.get("name").and_then(Value::as_str).unwrap_or("?");
                    let method = f
                        .get("methodName")
                        .and_then(Value::as_str)
                        .unwrap_or("?");
                    let msg = f.get("message").and_then(Value::as_str).unwrap_or("?");
                    lines.push(format!("  test {name}.{method}: {msg}"));
                }
            }
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::utils::count_tokens;

    #[test]
    fn deploy_success_verbose_reduces_tokens() {
        let raw = include_str!("../../../tests/fixtures/salesforce/deploy_success_verbose.json");
        let out = filter_deploy(raw, FilterOptions { ultra_compact: false });
        assert!(!out.passthrough);
        assert!(!out.text.contains("componentSuccesses"));
        let savings =
            100.0 - (count_tokens(&out.text) as f64 / count_tokens(raw) as f64 * 100.0);
        assert!(
            savings >= 20.0,
            "expected >=20% savings, got {savings:.1}%"
        );
    }

    #[test]
    fn deploy_success_concise_filters_without_successes() {
        let raw = include_str!("../../../tests/fixtures/salesforce/deploy_success_concise.json");
        let out = filter_deploy(raw, FilterOptions { ultra_compact: false });
        assert!(!out.passthrough);
        assert!(!out.text.contains("componentSuccesses"));
        assert!(out.text.contains("\"success\":true") || out.text.contains("\"success\": true"));
    }

    #[test]
    fn deploy_failed_keeps_component_failures() {
        let raw = include_str!("../../../tests/fixtures/salesforce/deploy_failed.json");
        let out = filter_deploy(raw, FilterOptions { ultra_compact: false });
        assert!(out.text.contains("componentFailures"));
        assert!(out.text.contains("BadDeploy"));
        assert!(!out.text.contains("createdBy"));
    }

    #[test]
    fn deploy_ultra_compact_text_mode() {
        let raw = include_str!("../../../tests/fixtures/salesforce/deploy_failed.json");
        let out = filter_deploy(raw, FilterOptions { ultra_compact: true });
        assert!(out.text.starts_with("sf deploy:"));
        assert!(out.text.contains("BadDeploy"));
    }

    #[test]
    fn deploy_invalid_json_passthrough() {
        let out = filter_deploy("not json", FilterOptions { ultra_compact: false });
        assert!(out.passthrough);
    }
}
