//! ccusage CLI integration module
//!
//! Provides isolated interface to ccusage (npm package) for fetching
//! Claude Code API usage metrics. Handles subprocess execution, JSON parsing,
//! and graceful degradation when ccusage is unavailable.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::process::Command;

// ── Public Types ──

/// Metrics from ccusage for a single period (day/week/month)
#[derive(Debug, Deserialize)]
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
    pub key: String, // "2026-01-30" (daily), "2026-01" (monthly), "2026-01-20" (weekly ISO monday)
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
    week: String, // ISO week start (Monday)
    #[serde(flatten)]
    metrics: CcusageMetrics,
}

#[derive(Debug, Deserialize)]
struct MonthlyResponse {
    monthly: Vec<MonthlyEntry>,
}

#[derive(Debug, Deserialize)]
struct MonthlyEntry {
    month: String,
    #[serde(flatten)]
    metrics: CcusageMetrics,
}

// ── Public API ──

/// Check if ccusage binary exists in PATH
fn binary_exists() -> bool {
    Command::new("which")
        .arg("ccusage")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Build the ccusage command, falling back to npx if binary not in PATH
fn build_command() -> Option<Command> {
    if binary_exists() {
        return Some(Command::new("ccusage"));
    }

    // Fallback: try npx
    let npx_check = Command::new("npx")
        .arg("ccusage")
        .arg("--help")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    if npx_check.map(|s| s.success()).unwrap_or(false) {
        let mut cmd = Command::new("npx");
        cmd.arg("ccusage");
        return Some(cmd);
    }

    None
}

/// Check if ccusage CLI is available (binary or via npx)
pub fn is_available() -> bool {
    build_command().is_some()
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
            eprintln!("⚠️  ccusage not found. Install: npm i -g ccusage (or use npx ccusage)");
            return Ok(None);
        }
    };

    let subcommand = match granularity {
        Granularity::Daily => "daily",
        Granularity::Weekly => "weekly",
        Granularity::Monthly => "monthly",
    };

    let output = cmd
        .arg(subcommand)
        .arg("--json")
        .arg("--since")
        .arg("20250101") // 90 days back approx
        .output();

    let output = match output {
        Err(e) => {
            eprintln!("⚠️  ccusage execution failed: {}", e);
            return Ok(None);
        }
        Ok(o) => o,
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!(
            "⚠️  ccusage exited with {}: {}",
            output.status,
            stderr.trim()
        );
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let periods =
        parse_json(&stdout, granularity).context("Failed to parse ccusage JSON output")?;

    Ok(Some(periods))
}

// ── Internal Helpers ──

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

    #[test]
    fn test_is_available() {
        // Just smoke test - actual availability depends on system
        let _available = is_available();
        // No assertion - just ensure it doesn't panic
    }
}
