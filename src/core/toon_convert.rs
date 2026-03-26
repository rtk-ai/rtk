//! TOON (Token-Oriented Object Notation) encoding for JSON output.
//!
//! Bypass with `RTK_NO_TOON=1`.
use serde_json::Value;

const TOON_BUDGET_BYTES: usize = 16_384; // 16 KB ≈ 4,000 tokens at RTK's 4-char estimate

/// Try converting a JSON string to TOON. Returns `None` if disabled,
/// parsing fails, encoding fails, or TOON output is not shorter.
/// Over-budget output is truncated at line boundaries.
pub fn json_to_toon(json_str: &str) -> Option<String> {
    if std::env::var("RTK_NO_TOON").ok().as_deref() == Some("1") {
        return None;
    }

    let value: Value = serde_json::from_str(json_str).ok()?;
    let toon = toon_rust::encode(&value, None).ok()?;

    if toon.len() >= json_str.len() {
        return None;
    }

    if toon.len() > TOON_BUDGET_BYTES {
        Some(truncate_at_line_boundary(&toon, TOON_BUDGET_BYTES))
    } else {
        Some(toon)
    }
}

fn truncate_at_line_boundary(toon: &str, budget: usize) -> String {
    let safe_budget = (0..=budget.min(toon.len()))
        .rev()
        .find(|&i| toon.is_char_boundary(i))
        .unwrap_or(0);

    let truncate_at = match toon[..safe_budget].rfind('\n') {
        Some(pos) => pos,
        None => safe_budget,
    };

    let kept = &toon[..truncate_at];
    let remaining_lines = toon[truncate_at..].lines().count().saturating_sub(1);

    format!(
        "{}\n... ({} more lines, {} bytes total)",
        kept,
        remaining_lines,
        toon.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toon_basic_object() {
        let json = r#"{"name": "Alice", "age": 30, "active": true, "role": "admin", "email": "alice@example.com"}"#;
        let toon = json_to_toon(json).expect("should encode object");
        assert!(toon.len() < json.len());
        assert!(toon.contains("Alice"));
    }

    #[test]
    fn test_toon_array_preserves_all_values() {
        let items: Vec<Value> = (0..20)
            .map(|i| {
                serde_json::json!({
                    "id": i,
                    "name": format!("item_{}", i),
                    "status": "active"
                })
            })
            .collect();
        let json = serde_json::to_string(&serde_json::json!({"items": items})).expect("serialize");
        let toon = json_to_toon(&json).expect("should encode array");
        for i in 0..20 {
            assert!(
                toon.contains(&format!("item_{}", i)),
                "should preserve item_{}",
                i
            );
        }
    }

    #[test]
    fn test_toon_byte_savings() {
        let rows: Vec<Value> = (0..50)
            .map(|i| {
                serde_json::json!({
                    "id": i,
                    "name": format!("service_{}", i),
                    "status": if i % 3 == 0 { "running" } else { "stopped" },
                    "port": 3000 + i,
                    "region": "us-east-1"
                })
            })
            .collect();
        let json =
            serde_json::to_string(&serde_json::json!({"services": rows})).expect("serialize");
        let toon = json_to_toon(&json).expect("should encode dataset");
        let savings_pct = 100.0 - (toon.len() as f64 / json.len() as f64 * 100.0);
        assert!(
            savings_pct >= 30.0,
            "expected >=30% byte savings, got {:.1}%",
            savings_pct
        );
    }

    #[test]
    fn test_toon_budget_truncation() {
        let rows: Vec<Value> = (0..500)
            .map(|i| {
                serde_json::json!({
                    "id": i,
                    "name": format!("long_service_name_for_testing_{}", i),
                    "description": format!("Description text for service {} to inflate output", i),
                    "status": "running",
                    "port": 3000 + i,
                    "region": "us-east-1"
                })
            })
            .collect();
        let json = serde_json::to_string(&serde_json::json!({"data": rows})).expect("serialize");
        let toon = json_to_toon(&json).expect("should encode large dataset");
        assert!(
            toon.len() <= TOON_BUDGET_BYTES + 80,
            "should be budget-truncated (got {} bytes)",
            toon.len()
        );
        assert!(toon.contains("more lines"));
    }

    #[test]
    fn test_toon_returns_none_on_invalid() {
        assert!(json_to_toon("not json").is_none());
        assert!(json_to_toon("").is_none());
        assert!(json_to_toon("{broken").is_none());
    }

    #[test]
    fn test_toon_returns_none_when_not_shorter() {
        assert!(json_to_toon("1").is_none());
    }

    #[test]
    fn test_truncate_at_line_boundary_basic() {
        let input = "line1\nline2\nline3\nline4\nline5\n";
        let result = truncate_at_line_boundary(input, 12);
        assert!(result.starts_with("line1\nline2"));
        assert!(result.contains("more lines"));
    }

    #[test]
    fn test_truncate_no_newline_within_budget() {
        let input = "a".repeat(20_000);
        let result = truncate_at_line_boundary(&input, 100);
        assert!(result.len() <= 200, "got {} bytes", result.len());
        assert!(result.contains("more lines"));
    }

    #[test]
    fn test_truncate_utf8_boundary() {
        let mut input = "a".repeat(98);
        input.push_str("é\n");
        input.push_str("after\n");
        let result = truncate_at_line_boundary(&input, 99);
        assert!(!result.is_empty());
    }
}
