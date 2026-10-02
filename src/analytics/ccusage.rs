//! Parses Claude Code spending data for economics reporting.
//!
//! Provides isolated interface to ccusage (npm package) for fetching
//! Claude Code API usage metrics. Handles subprocess execution, JSON parsing,
//! and graceful degradation when ccusage is unavailable.

use crate::core::stream::exec_capture;
use crate::core::utils::{resolved_command, tool_exists};
use anyhow::{Context, Result};
use chrono::{Datelike, NaiveDate, Weekday};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::process::Command;

// ── Public Types ──

/// Metrics from ccusage for a single period (day/week/month)
#[derive(Debug, Default, Deserialize)]
pub struct CcusageMetrics {
    #[serde(rename = "inputTokens")]
    pub input_tokens: u64,
    #[serde(rename = "outputTokens")]
    pub output_tokens: u64,
    #[serde(rename = "cacheCreationTokens", default)]
    pub cache_creation_tokens: u64,
    #[serde(rename = "cacheReadTokens", default)]
    pub cache_read_tokens: u64,
    #[serde(rename = "totalTokens")]
    pub total_tokens: u64,
    #[serde(rename = "totalCost")]
    pub total_cost: f64,
}

/// Period data with key (date/month/week) and metrics
#[derive(Debug)]
pub struct CcusagePeriod {
    pub key: String, // "2026-01-30" (daily), "2026-01" (monthly), "2026-09-28" (weekly Monday)
    pub metrics: CcusageMetrics,
}

/// Time granularity for ccusage reports
#[derive(Debug, Clone, Copy)]
pub enum Granularity {
    Daily,
    Weekly,
    Monthly,
}

// ── Internal Types for JSON Deserialization ──

#[derive(Debug, Deserialize)]
struct DailyResponse {
    daily: Vec<DailyEntry>,
}

#[derive(Debug, Deserialize)]
struct DailyEntry {
    // Older ccusage emits "date"; current ccusage emits "period". Accept both.
    #[serde(alias = "period")]
    date: String,
    #[serde(flatten)]
    metrics: CcusageMetrics,
}

#[derive(Debug, Deserialize)]
struct WeeklyResponse {
    weekly: Vec<WeeklyEntry>,
}

#[derive(Debug, Deserialize)]
struct WeeklyEntry {
    // Older ccusage emits "week"; current ccusage emits "period". Accept both.
    #[serde(alias = "period")]
    week: String, // Calendar week start; legacy ccusage defaults to Sunday.
    #[serde(flatten)]
    metrics: CcusageMetrics,
}

#[derive(Debug, Deserialize)]
struct MonthlyResponse {
    monthly: Vec<MonthlyEntry>,
}

#[derive(Debug, Deserialize)]
struct MonthlyEntry {
    // Older ccusage emits "month"; current ccusage emits "period". Accept both.
    #[serde(alias = "period")]
    month: String,
    #[serde(flatten)]
    metrics: CcusageMetrics,
}

// ── Public API ──

/// Check if ccusage binary exists in PATH
fn binary_exists() -> bool {
    tool_exists("ccusage")
}

/// Build the ccusage command, falling back to npx if binary not in PATH
fn build_command() -> Option<Command> {
    if binary_exists() {
        return Some(resolved_command("ccusage"));
    }

    // Fallback: try npx
    eprintln!("[info] ccusage not installed globally, fetching via npx...");
    let npx_check = resolved_command("npx")
        .arg("--yes")
        .arg("ccusage")
        .arg("--help")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    if npx_check.map(|s| s.success()).unwrap_or(false) {
        let mut cmd = resolved_command("npx");
        cmd.arg("--yes");
        cmd.arg("ccusage");
        return Some(cmd);
    }

    None
}

/// Fetch usage data from ccusage for the last 90 days
///
/// Returns `Ok(None)` if ccusage is unavailable (graceful degradation)
/// Returns `Ok(Some(vec))` with parsed data on success
/// Returns `Err` only on unexpected failures (JSON parse, etc.)
pub fn fetch(granularity: Granularity) -> Result<Option<Vec<CcusagePeriod>>> {
    let mut cmd = match build_command() {
        Some(cmd) => cmd,
        None => {
            eprintln!("[warn] ccusage not found. Install: npm i -g ccusage (or use npx ccusage)");
            return Ok(None);
        }
    };

    let subcommand = match granularity {
        Granularity::Daily => "daily",
        Granularity::Weekly => "weekly",
        Granularity::Monthly => "monthly",
    };

    cmd.arg(subcommand)
        .arg("--json")
        .arg("--since")
        .arg("20250101"); // 90 days back approx

    let mut requested_monday = false;
    loop {
        let result = match exec_capture(&mut cmd) {
            Err(e) => {
                eprintln!("[warn] ccusage execution failed: {}", e);
                return Ok(None);
            }
            Ok(r) => r,
        };

        if !result.success() {
            eprintln!(
                "[warn] ccusage exited with {}: {}",
                result.exit_code,
                result.stderr.trim()
            );
            return Ok(None);
        }

        let mut periods = parse_json(&result.stdout, granularity)
            .context("Failed to parse ccusage JSON output")?;

        if matches!(granularity, Granularity::Weekly) {
            if !canonicalize_weekly_keys(&mut periods)? {
                anyhow::ensure!(
                    !requested_monday,
                    "ccusage returned non-Monday weeks after requesting --start-of-week monday"
                );
                // Legacy ccusage has configurable (default Sunday) weeks.
                // Current root ccusage uses Monday and rejects this flag.
                // Preserve weekly-specific config and re-fetch every bucket:
                // shifting labels cannot reallocate Sunday's spending.
                cmd.args(["--start-of-week", "monday"]);
                requested_monday = true;
                continue;
            }
            periods = aggregate_weekly_periods(periods)?;
        }

        return Ok(Some(periods));
    }
}

// ── Internal Helpers ──

/// Validate every bucket's start date and canonicalize its spelling without
/// moving it. False means the whole report must be requested again for Monday.
fn canonicalize_weekly_keys(periods: &mut [CcusagePeriod]) -> Result<bool> {
    let mut all_mondays = true;
    for period in periods {
        let date = NaiveDate::parse_from_str(&period.key, "%Y-%m-%d")
            .with_context(|| format!("Invalid ccusage week start: {}", period.key))?;
        all_mondays &= date.weekday() == Weekday::Mon;
        period.key = date.format("%Y-%m-%d").to_string();
    }
    Ok(all_mondays)
}

/// Combine the final Monday report's disjoint project rows. Legacy ccusage can
/// emit multiple rows per week while omitting project identity from JSON.
/// Sum totalTokens independently; some sources count cache tokens differently.
fn aggregate_weekly_periods(periods: Vec<CcusagePeriod>) -> Result<Vec<CcusagePeriod>> {
    let mut weeks: BTreeMap<String, CcusageMetrics> = BTreeMap::new();
    for period in periods {
        let week = weeks.entry(period.key).or_default();
        let metrics = period.metrics;
        week.input_tokens = week
            .input_tokens
            .checked_add(metrics.input_tokens)
            .context("ccusage weekly input tokens overflow")?;
        week.output_tokens = week
            .output_tokens
            .checked_add(metrics.output_tokens)
            .context("ccusage weekly output tokens overflow")?;
        week.cache_creation_tokens = week
            .cache_creation_tokens
            .checked_add(metrics.cache_creation_tokens)
            .context("ccusage weekly cache creation tokens overflow")?;
        week.cache_read_tokens = week
            .cache_read_tokens
            .checked_add(metrics.cache_read_tokens)
            .context("ccusage weekly cache read tokens overflow")?;
        week.total_tokens = week
            .total_tokens
            .checked_add(metrics.total_tokens)
            .context("ccusage weekly total tokens overflow")?;
        week.total_cost += metrics.total_cost;
        anyhow::ensure!(
            week.total_cost.is_finite(),
            "ccusage weekly total cost overflow"
        );
    }
    for week in weeks.values() {
        week.input_tokens
            .checked_add(week.output_tokens)
            .context("ccusage weekly active tokens overflow")?;
    }
    Ok(weeks
        .into_iter()
        .map(|(key, metrics)| CcusagePeriod { key, metrics })
        .collect())
}

fn parse_json(json: &str, granularity: Granularity) -> Result<Vec<CcusagePeriod>> {
    match granularity {
        Granularity::Daily => {
            let resp: DailyResponse =
                serde_json::from_str(json).context("Invalid JSON structure for daily data")?;
            Ok(resp
                .daily
                .into_iter()
                .map(|e| CcusagePeriod {
                    key: e.date,
                    metrics: e.metrics,
                })
                .collect())
        }
        Granularity::Weekly => {
            let resp: WeeklyResponse =
                serde_json::from_str(json).context("Invalid JSON structure for weekly data")?;
            Ok(resp
                .weekly
                .into_iter()
                .map(|e| CcusagePeriod {
                    key: e.week,
                    metrics: e.metrics,
                })
                .collect())
        }
        Granularity::Monthly => {
            let resp: MonthlyResponse =
                serde_json::from_str(json).context("Invalid JSON structure for monthly data")?;
            Ok(resp
                .monthly
                .into_iter()
                .map(|e| CcusagePeriod {
                    key: e.month,
                    metrics: e.metrics,
                })
                .collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weekly_payload(field: &str, dates: &[&str]) -> String {
        let weekly: Vec<_> = dates
            .iter()
            .map(|date| {
                serde_json::json!({
                    field: date,
                    "inputTokens": 100,
                    "outputTokens": 20,
                    "cacheCreationTokens": 0,
                    "cacheReadTokens": 10,
                    "totalTokens": 120,
                    "totalCost": 0.125
                })
            })
            .collect();
        serde_json::json!({"weekly": weekly}).to_string()
    }

    #[test]
    fn test_weekly_keys_validate_every_weekday_without_relabeling() {
        for field in ["week", "period"] {
            for (date, is_monday) in [
                ("2026-09-28", true),
                ("2026-09-29", false),
                ("2026-09-30", false),
                ("2026-10-01", false),
                ("2026-10-02", false),
                ("2026-10-03", false),
                ("2026-10-04", false),
                ("2020-12-28", true),
                ("2021-01-03", false),
                ("2024-02-26", true),
                ("2024-02-29", false),
                ("2025-12-29", true),
                ("2026-01-04", false),
            ] {
                let mut periods = parse_json(&weekly_payload(field, &[date]), Granularity::Weekly)
                    .expect("valid weekly payload");
                assert_eq!(
                    canonicalize_weekly_keys(&mut periods).expect("valid date"),
                    is_monday,
                    "{field}: {date}"
                );
                assert_eq!(periods[0].key, date, "never move pre-aggregated spending");
                assert_eq!(periods[0].metrics.total_tokens, 120);
                assert_eq!(periods[0].metrics.total_cost, 0.125);
            }
        }
    }

    #[test]
    fn test_weekly_keys_canonical_mixed_and_empty_cases() {
        let mut periods = parse_json(
            &weekly_payload("period", &["2026-9-28", "2025-12-29"]),
            Granularity::Weekly,
        )
        .expect("valid weekly payload");
        assert!(canonicalize_weekly_keys(&mut periods).expect("Monday dates"));
        assert_eq!(periods[0].key, "2026-09-28");
        assert_eq!(periods[1].key, "2025-12-29");
        for dates in [["2026-09-28", "2026-09-27"], ["2026-09-27", "2026-09-28"]] {
            let mut mixed = parse_json(&weekly_payload("week", &dates), Granularity::Weekly)
                .expect("valid weekly payload");
            assert!(!canonicalize_weekly_keys(&mut mixed).expect("valid dates"));
            assert_eq!(mixed[0].key, dates[0]);
            assert_eq!(mixed[1].key, dates[1]);
        }
        let mut empty = parse_json(r#"{"weekly": []}"#, Granularity::Weekly).expect("empty weekly");
        assert!(canonicalize_weekly_keys(&mut empty).expect("empty keys"));
        assert!(
            aggregate_weekly_periods(empty)
                .expect("empty metrics")
                .is_empty()
        );
    }

    #[test]
    fn test_weekly_keys_validate_invalid_dates_even_after_sunday() {
        for invalid in ["invalid", "", "2026-02-29", "2026-13-01", "2026-W40"] {
            let mut periods = parse_json(
                &weekly_payload("week", &["2026-09-27", invalid]),
                Granularity::Weekly,
            )
            .expect("syntactically valid JSON");
            let error =
                canonicalize_weekly_keys(&mut periods).expect_err("invalid date must error");
            assert!(
                error.to_string().contains("Invalid ccusage week start"),
                "{invalid}: {error}"
            );
        }
    }

    #[test]
    fn test_weekly_aggregation_sums_duplicate_project_rows_and_sorts() {
        for field in ["week", "period"] {
            let mut json: serde_json::Value = serde_json::from_str(&weekly_payload(
                field,
                &["2026-10-05", "2026-9-28", "2026-09-28"],
            ))
            .expect("payload");
            json["weekly"][0]["totalCost"] = serde_json::json!(3.0);
            json["weekly"][1]["totalCost"] = serde_json::json!(11.0);
            json["weekly"][2]["totalCost"] = serde_json::json!(29.0);
            let mut periods =
                parse_json(&json.to_string(), Granularity::Weekly).expect("weekly rows");
            assert!(canonicalize_weekly_keys(&mut periods).expect("Monday dates"));
            let result = aggregate_weekly_periods(periods).expect("safe weekly sums");
            assert_eq!(result.len(), 2);
            assert_eq!(result[0].key, "2026-09-28");
            assert_eq!(result[0].metrics.input_tokens, 200);
            assert_eq!(result[0].metrics.output_tokens, 40);
            assert_eq!(result[0].metrics.cache_creation_tokens, 0);
            assert_eq!(result[0].metrics.cache_read_tokens, 20);
            assert_eq!(
                result[0].metrics.total_tokens, 240,
                "reported total is independent of categories"
            );
            assert_eq!(result[0].metrics.total_cost, 40.0);
            assert_eq!(result[1].key, "2026-10-05");
            assert_eq!(result[1].metrics.total_tokens, 120);
            assert_eq!(result[1].metrics.total_cost, 3.0);
        }
    }

    #[test]
    fn test_weekly_aggregation_rejects_overflow_without_partial_results() {
        for (field, label) in [
            ("inputTokens", "input tokens"),
            ("outputTokens", "output tokens"),
            ("cacheCreationTokens", "cache creation tokens"),
            ("cacheReadTokens", "cache read tokens"),
            ("totalTokens", "total tokens"),
        ] {
            let mut json: serde_json::Value =
                serde_json::from_str(&weekly_payload("week", &["2026-09-28", "2026-09-28"]))
                    .expect("payload");
            json["weekly"][0][field] = serde_json::json!(u64::MAX);
            json["weekly"][1][field] = serde_json::json!(1);
            let periods =
                parse_json(&json.to_string(), Granularity::Weekly).expect("valid u64 values");
            let error = aggregate_weekly_periods(periods).expect_err("sum must not overflow");
            assert!(
                error.to_string().contains(&format!("{label} overflow")),
                "{field}: {error}"
            );
        }
        let mut json: serde_json::Value =
            serde_json::from_str(&weekly_payload("week", &["2026-09-28", "2026-09-28"]))
                .expect("payload");
        json["weekly"][0]["totalCost"] = serde_json::json!(f64::MAX);
        json["weekly"][1]["totalCost"] = serde_json::json!(f64::MAX);
        let periods =
            parse_json(&json.to_string(), Granularity::Weekly).expect("finite source costs");
        assert!(
            aggregate_weekly_periods(periods)
                .expect_err("finite sum required")
                .to_string()
                .contains("total cost overflow")
        );

        let mut json: serde_json::Value =
            serde_json::from_str(&weekly_payload("week", &["2026-09-28"])).expect("payload");
        json["weekly"][0]["inputTokens"] = serde_json::json!(u64::MAX);
        json["weekly"][0]["outputTokens"] = serde_json::json!(1);
        let periods =
            parse_json(&json.to_string(), Granularity::Weekly).expect("valid source counters");
        assert!(
            aggregate_weekly_periods(periods)
                .expect_err("active sum must fit")
                .to_string()
                .contains("active tokens overflow")
        );
    }

    #[test]
    fn test_weekly_upstream_focused_snapshot_requires_regrouping() {
        let json = include_str!("../../tests/fixtures/ccusage_weekly_focused_v20_snapshot.json");
        let mut periods =
            parse_json(json, Granularity::Weekly).expect("upstream renderer snapshot");
        assert!(!canonicalize_weekly_keys(&mut periods).expect("valid Sunday dates"));
        assert_eq!(periods.len(), 3);
        for (p, (date, input, output, creation, read, total, cost)) in periods.iter().zip([
            ("2098-12-28", 60, 10, 15, 25, 110, 0.0001555),
            ("2099-01-11", 130, 20, 30, 40, 220, 0.0003224),
            ("2099-02-01", 40, 5, 0, 10, 55, 0.0000806),
        ]) {
            assert_eq!(p.key, date);
            assert_eq!(p.metrics.input_tokens, input);
            assert_eq!(p.metrics.output_tokens, output);
            assert_eq!(p.metrics.cache_creation_tokens, creation);
            assert_eq!(p.metrics.cache_read_tokens, read);
            assert_eq!(p.metrics.total_tokens, total);
            assert!((p.metrics.total_cost - cost).abs() < 1e-12);
        }
    }

    #[test]
    fn test_weekly_root_fixture_preserves_independent_total_tokens() {
        let json = include_str!("../../tests/fixtures/ccusage_weekly_root_v20_synthetic.json");
        let mut periods =
            parse_json(json, Granularity::Weekly).expect("source-derived synthetic fixture");
        assert!(canonicalize_weekly_keys(&mut periods).expect("valid Monday"));
        let periods = aggregate_weekly_periods(periods).expect("valid metrics");
        assert_eq!(periods.len(), 1);
        assert_eq!(periods[0].key, "2025-12-29");
        assert_eq!(periods[0].metrics.input_tokens, 100);
        assert_eq!(periods[0].metrics.output_tokens, 20);
        assert_eq!(periods[0].metrics.cache_creation_tokens, 0);
        assert_eq!(periods[0].metrics.cache_read_tokens, 10);
        assert_eq!(periods[0].metrics.total_tokens, 120);
        assert_eq!(periods[0].metrics.total_cost, 0.01);
    }

    #[test]
    fn test_parse_monthly_valid() {
        let json = r#"{
            "monthly": [
                {
                    "month": "2026-01",
                    "inputTokens": 1000,
                    "outputTokens": 500,
                    "cacheCreationTokens": 100,
                    "cacheReadTokens": 200,
                    "totalTokens": 1800,
                    "totalCost": 12.34
                }
            ]
        }"#;

        let result = parse_json(json, Granularity::Monthly);
        assert!(result.is_ok());
        let periods = result.unwrap();
        assert_eq!(periods.len(), 1);
        assert_eq!(periods[0].key, "2026-01");
        assert_eq!(periods[0].metrics.input_tokens, 1000);
        assert_eq!(periods[0].metrics.total_cost, 12.34);
    }

    #[test]
    fn test_parse_daily_valid() {
        let json = r#"{
            "daily": [
                {
                    "date": "2026-01-30",
                    "inputTokens": 100,
                    "outputTokens": 50,
                    "cacheCreationTokens": 0,
                    "cacheReadTokens": 0,
                    "totalTokens": 150,
                    "totalCost": 0.15
                }
            ]
        }"#;

        let result = parse_json(json, Granularity::Daily);
        assert!(result.is_ok());
        let periods = result.unwrap();
        assert_eq!(periods.len(), 1);
        assert_eq!(periods[0].key, "2026-01-30");
    }

    #[test]
    fn test_parse_weekly_valid() {
        let json = r#"{
            "weekly": [
                {
                    "week": "2026-01-20",
                    "inputTokens": 500,
                    "outputTokens": 250,
                    "cacheCreationTokens": 50,
                    "cacheReadTokens": 100,
                    "totalTokens": 900,
                    "totalCost": 5.67
                }
            ]
        }"#;

        let result = parse_json(json, Granularity::Weekly);
        assert!(result.is_ok());
        let periods = result.unwrap();
        assert_eq!(periods.len(), 1);
        assert_eq!(periods[0].key, "2026-01-20");
    }

    #[test]
    fn test_parse_malformed_json() {
        let json = r#"{ "monthly": [ { "broken": }"#;
        let result = parse_json(json, Granularity::Monthly);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_missing_required_fields() {
        let json = r#"{
            "monthly": [
                {
                    "month": "2026-01",
                    "inputTokens": 100
                }
            ]
        }"#;
        let result = parse_json(json, Granularity::Monthly);
        assert!(result.is_err()); // Missing required fields like totalTokens
    }

    #[test]
    fn test_parse_monthly_period_key() {
        // Current ccusage emits "period" instead of "month" for the record key.
        let json = r#"{
            "monthly": [
                {
                    "period": "2026-01",
                    "inputTokens": 1000,
                    "outputTokens": 500,
                    "totalTokens": 1800,
                    "totalCost": 12.34
                }
            ]
        }"#;

        let result = parse_json(json, Granularity::Monthly);
        assert!(result.is_ok());
        let periods = result.unwrap();
        assert_eq!(periods.len(), 1);
        assert_eq!(periods[0].key, "2026-01");
        assert_eq!(periods[0].metrics.total_cost, 12.34);
    }

    #[test]
    fn test_parse_daily_period_key() {
        let json = r#"{
            "daily": [
                {
                    "period": "2026-01-30",
                    "inputTokens": 100,
                    "outputTokens": 50,
                    "totalTokens": 150,
                    "totalCost": 0.15
                }
            ]
        }"#;

        let result = parse_json(json, Granularity::Daily);
        assert!(result.is_ok());
        let periods = result.unwrap();
        assert_eq!(periods.len(), 1);
        assert_eq!(periods[0].key, "2026-01-30");
    }

    #[test]
    fn test_parse_weekly_period_key() {
        let json = r#"{
            "weekly": [
                {
                    "period": "2026-01-20",
                    "inputTokens": 500,
                    "outputTokens": 250,
                    "totalTokens": 900,
                    "totalCost": 5.67
                }
            ]
        }"#;

        let result = parse_json(json, Granularity::Weekly);
        assert!(result.is_ok());
        let periods = result.unwrap();
        assert_eq!(periods.len(), 1);
        assert_eq!(periods[0].key, "2026-01-20");
    }

    #[test]
    fn test_parse_default_cache_fields() {
        let json = r#"{
            "monthly": [
                {
                    "month": "2026-01",
                    "inputTokens": 100,
                    "outputTokens": 50,
                    "totalTokens": 150,
                    "totalCost": 1.0
                }
            ]
        }"#;

        let result = parse_json(json, Granularity::Monthly);
        assert!(result.is_ok());
        let periods = result.unwrap();
        assert_eq!(periods[0].metrics.cache_creation_tokens, 0); // default
        assert_eq!(periods[0].metrics.cache_read_tokens, 0);
    }
}
