//! Claude Code Economics: Spending vs Savings Analysis
//!
//! Combines ccusage (tokens spent) with rtk tracking (tokens saved) to provide
//! dual-metric economic impact reporting with blended and active cost-per-token.

use anyhow::{Context, Result};
use chrono::NaiveDate;
use serde::Serialize;
use std::collections::HashMap;

use crate::ccusage::{self, CcusagePeriod, Granularity};
use crate::tracking::{DayStats, MonthStats, Tracker, WeekStats};
use crate::utils::{format_tokens, format_usd};

// ── Constants ──

const BILLION: f64 = 1e9;

// ── Types ──

#[derive(Debug, Serialize)]
pub struct PeriodEconomics {
    pub label: String,
    // ccusage metrics (Option for graceful degradation)
    pub cc_cost: Option<f64>,
    pub cc_total_tokens: Option<u64>,
    pub cc_active_tokens: Option<u64>, // input + output only (excluding cache)
    // rtk metrics
    pub rtk_commands: Option<usize>,
    pub rtk_saved_tokens: Option<usize>,
    pub rtk_savings_pct: Option<f64>,
    // Dual metrics
    pub blended_cpt: Option<f64>, // cost / total_tokens (diluted by cache)
    pub active_cpt: Option<f64>,  // cost / active_tokens (realistic input cost)
    pub savings_blended: Option<f64>, // saved * blended_cpt
    pub savings_active: Option<f64>, // saved * active_cpt
}

impl PeriodEconomics {
    fn new(label: &str) -> Self {
        Self {
            label: label.to_string(),
            cc_cost: None,
            cc_total_tokens: None,
            cc_active_tokens: None,
            rtk_commands: None,
            rtk_saved_tokens: None,
            rtk_savings_pct: None,
            blended_cpt: None,
            active_cpt: None,
            savings_blended: None,
            savings_active: None,
        }
    }

    fn set_ccusage(&mut self, metrics: &ccusage::CcusageMetrics) {
        self.cc_cost = Some(metrics.total_cost);
        self.cc_total_tokens = Some(metrics.total_tokens);
        let active = metrics.input_tokens + metrics.output_tokens;
        self.cc_active_tokens = Some(active);
    }

    fn set_rtk_from_day(&mut self, stats: &DayStats) {
        self.rtk_commands = Some(stats.commands);
        self.rtk_saved_tokens = Some(stats.saved_tokens);
        self.rtk_savings_pct = Some(stats.savings_pct);
    }

    fn set_rtk_from_week(&mut self, stats: &WeekStats) {
        self.rtk_commands = Some(stats.commands);
        self.rtk_saved_tokens = Some(stats.saved_tokens);
        self.rtk_savings_pct = Some(stats.savings_pct);
    }

    fn set_rtk_from_month(&mut self, stats: &MonthStats) {
        self.rtk_commands = Some(stats.commands);
        self.rtk_saved_tokens = Some(stats.saved_tokens);
        self.rtk_savings_pct = Some(if stats.input_tokens + stats.output_tokens > 0 {
            stats.saved_tokens as f64
                / (stats.saved_tokens + stats.input_tokens + stats.output_tokens) as f64
                * 100.0
        } else {
            0.0
        });
    }

    fn compute_dual_metrics(&mut self) {
        if let (Some(cost), Some(saved)) = (self.cc_cost, self.rtk_saved_tokens) {
            // Blended CPT (cost / total_tokens including cache)
            if let Some(total) = self.cc_total_tokens {
                if total > 0 {
                    self.blended_cpt = Some(cost / total as f64);
                    self.savings_blended = Some(saved as f64 * (cost / total as f64));
                }
            }

            // Active CPT (cost / active_tokens = input+output only)
            if let Some(active) = self.cc_active_tokens {
                if active > 0 {
                    self.active_cpt = Some(cost / active as f64);
                    self.savings_active = Some(saved as f64 * (cost / active as f64));
                }
            }
        }
    }
}

#[derive(Debug, Serialize)]
struct Totals {
    cc_cost: f64,
    cc_total_tokens: u64,
    cc_active_tokens: u64,
    rtk_commands: usize,
    rtk_saved_tokens: usize,
    rtk_avg_savings_pct: f64,
    blended_cpt: Option<f64>,
    active_cpt: Option<f64>,
    savings_blended: Option<f64>,
    savings_active: Option<f64>,
}

// ── Public API ──

pub fn run(
    daily: bool,
    weekly: bool,
    monthly: bool,
    all: bool,
    format: &str,
    _verbose: u8,
) -> Result<()> {
    let tracker = Tracker::new().context("Failed to initialize tracking database")?;

    match format {
        "json" => export_json(&tracker, daily, weekly, monthly, all),
        "csv" => export_csv(&tracker, daily, weekly, monthly, all),
        _ => display_text(&tracker, daily, weekly, monthly, all),
    }
}

// ── Merge Logic ──

fn merge_daily(cc: Option<Vec<CcusagePeriod>>, rtk: Vec<DayStats>) -> Vec<PeriodEconomics> {
    let mut map: HashMap<String, PeriodEconomics> = HashMap::new();

    // Insert ccusage data
    if let Some(cc_data) = cc {
        for entry in cc_data {
            let crate::ccusage::CcusagePeriod { key, metrics } = entry;
            map.entry(key)
                .or_insert_with_key(|k| PeriodEconomics::new(k))
                .set_ccusage(&metrics);
        }
    }

    // Merge rtk data
    for entry in rtk {
        map.entry(entry.date.clone())
            .or_insert_with_key(|k| PeriodEconomics::new(k))
            .set_rtk_from_day(&entry);
    }

    // Compute dual metrics and sort
    let mut result: Vec<_> = map.into_values().collect();
    for period in &mut result {
        period.compute_dual_metrics();
    }
    result.sort_by(|a, b| a.label.cmp(&b.label));
    result
}

fn merge_weekly(cc: Option<Vec<CcusagePeriod>>, rtk: Vec<WeekStats>) -> Vec<PeriodEconomics> {
    let mut map: HashMap<String, PeriodEconomics> = HashMap::new();

    // Insert ccusage data (key = ISO Monday "2026-01-20")
    if let Some(cc_data) = cc {
        for entry in cc_data {
            let crate::ccusage::CcusagePeriod { key, metrics } = entry;
            map.entry(key)
                .or_insert_with_key(|k| PeriodEconomics::new(k))
                .set_ccusage(&metrics);
        }
    }

    // Merge rtk data (week_start = legacy Saturday "2026-01-18")
    // Convert Saturday to Monday for alignment
    for entry in rtk {
        let monday_key = match convert_saturday_to_monday(&entry.week_start) {
            Some(m) => m,
            None => {
                eprintln!("⚠️  Invalid week_start format: {}", entry.week_start);
                continue;
            }
        };

        map.entry(monday_key)
            .or_insert_with_key(|key| PeriodEconomics::new(key))
            .set_rtk_from_week(&entry);
    }

    let mut result: Vec<_> = map.into_values().collect();
    for period in &mut result {
        period.compute_dual_metrics();
    }
    result.sort_by(|a, b| a.label.cmp(&b.label));
    result
}

fn merge_monthly(cc: Option<Vec<CcusagePeriod>>, rtk: Vec<MonthStats>) -> Vec<PeriodEconomics> {
    let mut map: HashMap<String, PeriodEconomics> = HashMap::new();

    // Insert ccusage data
    if let Some(cc_data) = cc {
        for entry in cc_data {
            let crate::ccusage::CcusagePeriod { key, metrics } = entry;
            map.entry(key)
                .or_insert_with_key(|k| PeriodEconomics::new(k))
                .set_ccusage(&metrics);
        }
    }

    // Merge rtk data
    for entry in rtk {
        map.entry(entry.month.clone())
            .or_insert_with_key(|k| PeriodEconomics::new(k))
            .set_rtk_from_month(&entry);
    }

    let mut result: Vec<_> = map.into_values().collect();
    for period in &mut result {
        period.compute_dual_metrics();
    }
    result.sort_by(|a, b| a.label.cmp(&b.label));
    result
}

// ── Helpers ──

/// Convert Saturday week_start (legacy rtk) to ISO Monday
/// Example: "2026-01-18" (Sat) -> "2026-01-20" (Mon)
fn convert_saturday_to_monday(saturday: &str) -> Option<String> {
    let sat_date = NaiveDate::parse_from_str(saturday, "%Y-%m-%d").ok()?;

    // rtk uses Saturday as week start, ISO uses Monday
    // Saturday + 2 days = Monday
    let monday = sat_date + chrono::TimeDelta::try_days(2)?;

    Some(monday.format("%Y-%m-%d").to_string())
}

fn compute_totals(periods: &[PeriodEconomics]) -> Totals {
    let mut totals = Totals {
        cc_cost: 0.0,
        cc_total_tokens: 0,
        cc_active_tokens: 0,
        rtk_commands: 0,
        rtk_saved_tokens: 0,
        rtk_avg_savings_pct: 0.0,
        blended_cpt: None,
        active_cpt: None,
        savings_blended: None,
        savings_active: None,
    };

    let mut pct_sum = 0.0;
    let mut pct_count = 0;

    for p in periods {
        if let Some(cost) = p.cc_cost {
            totals.cc_cost += cost;
        }
        if let Some(total) = p.cc_total_tokens {
            totals.cc_total_tokens += total;
        }
        if let Some(active) = p.cc_active_tokens {
            totals.cc_active_tokens += active;
        }
        if let Some(cmds) = p.rtk_commands {
            totals.rtk_commands += cmds;
        }
        if let Some(saved) = p.rtk_saved_tokens {
            totals.rtk_saved_tokens += saved;
        }
        if let Some(pct) = p.rtk_savings_pct {
            pct_sum += pct;
            pct_count += 1;
        }
    }

    if pct_count > 0 {
        totals.rtk_avg_savings_pct = pct_sum / pct_count as f64;
    }

    // Compute global dual metrics
    if totals.cc_total_tokens > 0 {
        totals.blended_cpt = Some(totals.cc_cost / totals.cc_total_tokens as f64);
        totals.savings_blended = Some(totals.rtk_saved_tokens as f64 * totals.blended_cpt.unwrap());
    }
    if totals.cc_active_tokens > 0 {
        totals.active_cpt = Some(totals.cc_cost / totals.cc_active_tokens as f64);
        totals.savings_active = Some(totals.rtk_saved_tokens as f64 * totals.active_cpt.unwrap());
    }

    totals
}

// ── Display ──

fn display_text(
    tracker: &Tracker,
    daily: bool,
    weekly: bool,
    monthly: bool,
    all: bool,
) -> Result<()> {
    // Default: summary view
    if !daily && !weekly && !monthly && !all {
        display_summary(tracker)?;
        return Ok(());
    }

    if all || daily {
        display_daily(tracker)?;
    }
    if all || weekly {
        display_weekly(tracker)?;
    }
    if all || monthly {
        display_monthly(tracker)?;
    }

    Ok(())
}

fn display_summary(tracker: &Tracker) -> Result<()> {
    let cc_monthly =
        ccusage::fetch(Granularity::Monthly).context("Failed to fetch ccusage monthly data")?;
    let rtk_monthly = tracker
        .get_by_month()
        .context("Failed to load monthly token savings from database")?;
    let periods = merge_monthly(cc_monthly, rtk_monthly);

    if periods.is_empty() {
        println!("No data available. Run some rtk commands to start tracking.");
        return Ok(());
    }

    let totals = compute_totals(&periods);

    println!("💰 Claude Code Economics");
    println!("════════════════════════════════════════════════════");
    println!();

    println!(
        "  Spent (ccusage):              {}",
        format_usd(totals.cc_cost)
    );
    println!(
        "  Active tokens (in+out):       {}",
        format_tokens(totals.cc_active_tokens as usize)
    );
    println!(
        "  Total tokens (incl. cache):   {}",
        format_tokens(totals.cc_total_tokens as usize)
    );
    println!();

    println!("  RTK commands:                 {}", totals.rtk_commands);
    println!(
        "  Tokens saved:                 {}",
        format_tokens(totals.rtk_saved_tokens)
    );
    println!();

    println!("  Estimated Savings:");
    println!("  ┌─────────────────────────────────────────────────┐");

    if let Some(active_savings) = totals.savings_active {
        let active_pct = if totals.cc_cost > 0.0 {
            (active_savings / totals.cc_cost) * 100.0
        } else {
            0.0
        };
        println!(
            "  │ Active token pricing:  {}  ({:.1}%)         │ ← most representative",
            format_usd(active_savings).trim_end(),
            active_pct
        );
    } else {
        println!("  │ Active token pricing:  —                         │");
    }

    if let Some(blended_savings) = totals.savings_blended {
        let blended_pct = if totals.cc_cost > 0.0 {
            (blended_savings / totals.cc_cost) * 100.0
        } else {
            0.0
        };
        println!(
            "  │ Blended pricing:       {}  ({:.2}%)          │",
            format_usd(blended_savings).trim_end(),
            blended_pct
        );
    } else {
        println!("  │ Blended pricing:       —                         │");
    }

    println!("  └─────────────────────────────────────────────────┘");
    println!();

    println!("  Why two numbers?");
    println!("  RTK prevents tokens from entering the LLM context (input tokens).");
    println!("  \"Active\" uses cost/(input+output) — reflects actual input token cost.");
    println!(
        "  \"Blended\" uses cost/all_tokens — diluted by {:.1}B cheap cache reads.",
        (totals.cc_total_tokens - totals.cc_active_tokens) as f64 / BILLION
    );
    println!();

    Ok(())
}

fn display_daily(tracker: &Tracker) -> Result<()> {
    let cc_daily =
        ccusage::fetch(Granularity::Daily).context("Failed to fetch ccusage daily data")?;
    let rtk_daily = tracker
        .get_all_days()
        .context("Failed to load daily token savings from database")?;
    let periods = merge_daily(cc_daily, rtk_daily);

    println!("📅 Daily Economics");
    println!("════════════════════════════════════════════════════");
    print_period_table(&periods);
    Ok(())
}

fn display_weekly(tracker: &Tracker) -> Result<()> {
    let cc_weekly =
        ccusage::fetch(Granularity::Weekly).context("Failed to fetch ccusage weekly data")?;
    let rtk_weekly = tracker
        .get_by_week()
        .context("Failed to load weekly token savings from database")?;
    let periods = merge_weekly(cc_weekly, rtk_weekly);

    println!("📅 Weekly Economics");
    println!("════════════════════════════════════════════════════");
    print_period_table(&periods);
    Ok(())
}

fn display_monthly(tracker: &Tracker) -> Result<()> {
    let cc_monthly =
        ccusage::fetch(Granularity::Monthly).context("Failed to fetch ccusage monthly data")?;
    let rtk_monthly = tracker
        .get_by_month()
        .context("Failed to load monthly token savings from database")?;
    let periods = merge_monthly(cc_monthly, rtk_monthly);

    println!("📅 Monthly Economics");
    println!("════════════════════════════════════════════════════");
    print_period_table(&periods);
    Ok(())
}

fn print_period_table(periods: &[PeriodEconomics]) {
    println!();
    println!(
        "{:<12} {:>10} {:>10} {:>10} {:>12} {:>12}",
        "Period", "Spent", "Saved", "Active$", "Blended$", "RTK Cmds"
    );
    println!(
        "{:-<12} {:-<10} {:-<10} {:-<10} {:-<12} {:-<12}",
        "", "", "", "", "", ""
    );

    for p in periods {
        let spent = p.cc_cost.map(format_usd).unwrap_or_else(|| "—".to_string());
        let saved = p
            .rtk_saved_tokens
            .map(format_tokens)
            .unwrap_or_else(|| "—".to_string());
        let active = p
            .savings_active
            .map(format_usd)
            .unwrap_or_else(|| "—".to_string());
        let blended = p
            .savings_blended
            .map(format_usd)
            .unwrap_or_else(|| "—".to_string());
        let cmds = p
            .rtk_commands
            .map(|c| c.to_string())
            .unwrap_or_else(|| "—".to_string());

        println!(
            "{:<12} {:>10} {:>10} {:>10} {:>12} {:>12}",
            p.label, spent, saved, active, blended, cmds
        );
    }
    println!();
}

// ── Export ──

fn export_json(
    tracker: &Tracker,
    daily: bool,
    weekly: bool,
    monthly: bool,
    all: bool,
) -> Result<()> {
    #[derive(Serialize)]
    struct Export {
        daily: Option<Vec<PeriodEconomics>>,
        weekly: Option<Vec<PeriodEconomics>>,
        monthly: Option<Vec<PeriodEconomics>>,
        totals: Option<Totals>,
    }

    let mut export = Export {
        daily: None,
        weekly: None,
        monthly: None,
        totals: None,
    };

    if all || daily {
        let cc = ccusage::fetch(Granularity::Daily)
            .context("Failed to fetch ccusage daily data for JSON export")?;
        let rtk = tracker
            .get_all_days()
            .context("Failed to load daily token savings for JSON export")?;
        export.daily = Some(merge_daily(cc, rtk));
    }

    if all || weekly {
        let cc = ccusage::fetch(Granularity::Weekly)
            .context("Failed to fetch ccusage weekly data for export")?;
        let rtk = tracker
            .get_by_week()
            .context("Failed to load weekly token savings for export")?;
        export.weekly = Some(merge_weekly(cc, rtk));
    }

    if all || monthly {
        let cc = ccusage::fetch(Granularity::Monthly)
            .context("Failed to fetch ccusage monthly data for export")?;
        let rtk = tracker
            .get_by_month()
            .context("Failed to load monthly token savings for export")?;
        let periods = merge_monthly(cc, rtk);
        export.totals = Some(compute_totals(&periods));
        export.monthly = Some(periods);
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&export)
            .context("Failed to serialize economics data to JSON")?
    );
    Ok(())
}

fn export_csv(
    tracker: &Tracker,
    daily: bool,
    weekly: bool,
    monthly: bool,
    all: bool,
) -> Result<()> {
    // Header
    println!("period,spent,active_tokens,total_tokens,saved_tokens,active_savings,blended_savings,rtk_commands");

    if all || daily {
        let cc = ccusage::fetch(Granularity::Daily)
            .context("Failed to fetch ccusage daily data for JSON export")?;
        let rtk = tracker
            .get_all_days()
            .context("Failed to load daily token savings for JSON export")?;
        let periods = merge_daily(cc, rtk);
        for p in periods {
            print_csv_row(&p);
        }
    }

    if all || weekly {
        let cc = ccusage::fetch(Granularity::Weekly)
            .context("Failed to fetch ccusage weekly data for export")?;
        let rtk = tracker
            .get_by_week()
            .context("Failed to load weekly token savings for export")?;
        let periods = merge_weekly(cc, rtk);
        for p in periods {
            print_csv_row(&p);
        }
    }

    if all || monthly {
        let cc = ccusage::fetch(Granularity::Monthly)
            .context("Failed to fetch ccusage monthly data for export")?;
        let rtk = tracker
            .get_by_month()
            .context("Failed to load monthly token savings for export")?;
        let periods = merge_monthly(cc, rtk);
        for p in periods {
            print_csv_row(&p);
        }
    }

    Ok(())
}

fn print_csv_row(p: &PeriodEconomics) {
    let spent = p.cc_cost.map(|c| format!("{:.4}", c)).unwrap_or_default();
    let active_tokens = p
        .cc_active_tokens
        .map(|t| t.to_string())
        .unwrap_or_default();
    let total_tokens = p.cc_total_tokens.map(|t| t.to_string()).unwrap_or_default();
    let saved_tokens = p
        .rtk_saved_tokens
        .map(|t| t.to_string())
        .unwrap_or_default();
    let active_savings = p
        .savings_active
        .map(|s| format!("{:.4}", s))
        .unwrap_or_default();
    let blended_savings = p
        .savings_blended
        .map(|s| format!("{:.4}", s))
        .unwrap_or_default();
    let cmds = p.rtk_commands.map(|c| c.to_string()).unwrap_or_default();

    println!(
        "{},{},{},{},{},{},{},{}",
        p.label,
        spent,
        active_tokens,
        total_tokens,
        saved_tokens,
        active_savings,
        blended_savings,
        cmds
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_saturday_to_monday() {
        // Saturday Jan 18 -> Monday Jan 20
        assert_eq!(
            convert_saturday_to_monday("2026-01-18"),
            Some("2026-01-20".to_string())
        );

        // Invalid format
        assert_eq!(convert_saturday_to_monday("invalid"), None);
    }

    #[test]
    fn test_period_economics_new() {
        let p = PeriodEconomics::new("2026-01");
        assert_eq!(p.label, "2026-01");
        assert!(p.cc_cost.is_none());
        assert!(p.rtk_commands.is_none());
    }

    #[test]
    fn test_compute_dual_metrics_with_data() {
        let mut p = PeriodEconomics::new("2026-01");
        p.cc_cost = Some(100.0);
        p.cc_total_tokens = Some(1_000_000);
        p.cc_active_tokens = Some(10_000);
        p.rtk_saved_tokens = Some(5_000);

        p.compute_dual_metrics();

        assert!(p.blended_cpt.is_some());
        assert_eq!(p.blended_cpt.unwrap(), 100.0 / 1_000_000.0);

        assert!(p.active_cpt.is_some());
        assert_eq!(p.active_cpt.unwrap(), 100.0 / 10_000.0);

        assert!(p.savings_blended.is_some());
        assert!(p.savings_active.is_some());
    }

    #[test]
    fn test_compute_dual_metrics_zero_tokens() {
        let mut p = PeriodEconomics::new("2026-01");
        p.cc_cost = Some(100.0);
        p.cc_total_tokens = Some(0);
        p.cc_active_tokens = Some(0);
        p.rtk_saved_tokens = Some(5_000);

        p.compute_dual_metrics();

        assert!(p.blended_cpt.is_none());
        assert!(p.active_cpt.is_none());
        assert!(p.savings_blended.is_none());
        assert!(p.savings_active.is_none());
    }

    #[test]
    fn test_compute_dual_metrics_no_ccusage_data() {
        let mut p = PeriodEconomics::new("2026-01");
        p.rtk_saved_tokens = Some(5_000);

        p.compute_dual_metrics();

        assert!(p.blended_cpt.is_none());
        assert!(p.active_cpt.is_none());
    }

    #[test]
    fn test_merge_monthly_both_present() {
        let cc = vec![CcusagePeriod {
            key: "2026-01".to_string(),
            metrics: ccusage::CcusageMetrics {
                input_tokens: 1000,
                output_tokens: 500,
                cache_creation_tokens: 100,
                cache_read_tokens: 200,
                total_tokens: 1800,
                total_cost: 12.34,
            },
        }];

        let rtk = vec![MonthStats {
            month: "2026-01".to_string(),
            commands: 10,
            input_tokens: 800,
            output_tokens: 400,
            saved_tokens: 5000,
            savings_pct: 50.0,
        }];

        let merged = merge_monthly(Some(cc), rtk);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].label, "2026-01");
        assert_eq!(merged[0].cc_cost, Some(12.34));
        assert_eq!(merged[0].rtk_commands, Some(10));
    }

    #[test]
    fn test_merge_monthly_only_ccusage() {
        let cc = vec![CcusagePeriod {
            key: "2026-01".to_string(),
            metrics: ccusage::CcusageMetrics {
                input_tokens: 1000,
                output_tokens: 500,
                cache_creation_tokens: 100,
                cache_read_tokens: 200,
                total_tokens: 1800,
                total_cost: 12.34,
            },
        }];

        let merged = merge_monthly(Some(cc), vec![]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].cc_cost, Some(12.34));
        assert!(merged[0].rtk_commands.is_none());
    }

    #[test]
    fn test_merge_monthly_only_rtk() {
        let rtk = vec![MonthStats {
            month: "2026-01".to_string(),
            commands: 10,
            input_tokens: 800,
            output_tokens: 400,
            saved_tokens: 5000,
            savings_pct: 50.0,
        }];

        let merged = merge_monthly(None, rtk);
        assert_eq!(merged.len(), 1);
        assert!(merged[0].cc_cost.is_none());
        assert_eq!(merged[0].rtk_commands, Some(10));
    }

    #[test]
    fn test_merge_monthly_sorted() {
        let rtk = vec![
            MonthStats {
                month: "2026-03".to_string(),
                commands: 5,
                input_tokens: 100,
                output_tokens: 50,
                saved_tokens: 1000,
                savings_pct: 40.0,
            },
            MonthStats {
                month: "2026-01".to_string(),
                commands: 10,
                input_tokens: 200,
                output_tokens: 100,
                saved_tokens: 2000,
                savings_pct: 60.0,
            },
        ];

        let merged = merge_monthly(None, rtk);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].label, "2026-01");
        assert_eq!(merged[1].label, "2026-03");
    }

    #[test]
    fn test_compute_totals() {
        let periods = vec![
            PeriodEconomics {
                label: "2026-01".to_string(),
                cc_cost: Some(100.0),
                cc_total_tokens: Some(1_000_000),
                cc_active_tokens: Some(10_000),
                rtk_commands: Some(5),
                rtk_saved_tokens: Some(2000),
                rtk_savings_pct: Some(50.0),
                blended_cpt: None,
                active_cpt: None,
                savings_blended: None,
                savings_active: None,
            },
            PeriodEconomics {
                label: "2026-02".to_string(),
                cc_cost: Some(200.0),
                cc_total_tokens: Some(2_000_000),
                cc_active_tokens: Some(20_000),
                rtk_commands: Some(10),
                rtk_saved_tokens: Some(3000),
                rtk_savings_pct: Some(60.0),
                blended_cpt: None,
                active_cpt: None,
                savings_blended: None,
                savings_active: None,
            },
        ];

        let totals = compute_totals(&periods);
        assert_eq!(totals.cc_cost, 300.0);
        assert_eq!(totals.cc_total_tokens, 3_000_000);
        assert_eq!(totals.cc_active_tokens, 30_000);
        assert_eq!(totals.rtk_commands, 15);
        assert_eq!(totals.rtk_saved_tokens, 5000);
        assert_eq!(totals.rtk_avg_savings_pct, 55.0);

        assert!(totals.blended_cpt.is_some());
        assert!(totals.active_cpt.is_some());
    }
}
