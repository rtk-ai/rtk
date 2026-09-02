//! Claude Code Economics: Spending vs Savings Analysis
//!
//! Combines ccusage (tokens spent) with rtk tracking (tokens saved) to provide
//! dual-metric economic impact reporting with blended and active cost-per-token.

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use super::ccusage::{self, CcusagePeriod, Granularity};
use crate::core::tracking::{DayStats, MonthStats, Tracker, WeekStats};
use crate::core::utils::{format_cpt, format_tokens, format_usd};

// ── Constants ──

// API pricing ratios (verified Feb 2026, consistent across Claude models <=200K context)
// Source: https://docs.anthropic.com/en/docs/about-claude/models
const WEIGHT_OUTPUT: f64 = 5.0; // Output = 5x input
const WEIGHT_CACHE_CREATE: f64 = 1.25; // Cache write = 1.25x input
const WEIGHT_CACHE_READ: f64 = 0.1; // Cache read = 0.1x input

// ── Types ──

#[derive(Debug, Serialize)]
pub struct PeriodEconomics {
    pub label: String,
    // ccusage metrics (Option for graceful degradation)
    pub cc_cost: Option<f64>,
    pub cc_total_tokens: Option<u64>,
    pub cc_active_tokens: Option<u64>, // input + output only (excluding cache)
    // Per-type token breakdown
    pub cc_input_tokens: Option<u64>,
    pub cc_output_tokens: Option<u64>,
    pub cc_cache_create_tokens: Option<u64>,
    pub cc_cache_read_tokens: Option<u64>,
    // rtk metrics
    pub rtk_commands: Option<usize>,
    pub rtk_saved_tokens: Option<usize>,
    pub rtk_savings_pct: Option<f64>,
    // Primary metric (weighted input CPT)
    pub weighted_input_cpt: Option<f64>, // Derived input CPT using API ratios
    pub savings_weighted: Option<f64>,   // saved * weighted_input_cpt (PRIMARY)
    // Legacy metrics (verbose mode only)
    pub blended_cpt: Option<f64>, // cost / total_tokens (diluted by cache)
    pub active_cpt: Option<f64>,  // cost / active_tokens (OVERESTIMATES)
    pub savings_blended: Option<f64>, // saved * blended_cpt (UNDERESTIMATES)
    pub savings_active: Option<f64>, // saved * active_cpt (OVERESTIMATES)
}

impl PeriodEconomics {
    fn new(label: &str) -> Self {
        Self {
            label: label.to_string(),
            cc_cost: None,
            cc_total_tokens: None,
            cc_active_tokens: None,
            cc_input_tokens: None,
            cc_output_tokens: None,
            cc_cache_create_tokens: None,
            cc_cache_read_tokens: None,
            rtk_commands: None,
            rtk_saved_tokens: None,
            rtk_savings_pct: None,
            weighted_input_cpt: None,
            savings_weighted: None,
            blended_cpt: None,
            active_cpt: None,
            savings_blended: None,
            savings_active: None,
        }
    }

    fn set_ccusage(&mut self, metrics: &ccusage::CcusageMetrics) {
        self.cc_cost = Some(metrics.total_cost);
        self.cc_total_tokens = Some(metrics.total_tokens);

        // Store per-type tokens
        self.cc_input_tokens = Some(metrics.input_tokens);
        self.cc_output_tokens = Some(metrics.output_tokens);
        self.cc_cache_create_tokens = Some(metrics.cache_creation_tokens);
        self.cc_cache_read_tokens = Some(metrics.cache_read_tokens);

        // Active tokens (legacy)
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

    fn compute_weighted_metrics(&mut self) {
        // Weighted input CPT derivation using API price ratios
        if let (Some(cost), Some(saved)) = (self.cc_cost, self.rtk_saved_tokens) {
            if let (Some(input), Some(output), Some(cache_create), Some(cache_read)) = (
                self.cc_input_tokens,
                self.cc_output_tokens,
                self.cc_cache_create_tokens,
                self.cc_cache_read_tokens,
            ) {
                // Weighted units = input + 5*output + 1.25*cache_create + 0.1*cache_read
                let weighted_units = input as f64
                    + WEIGHT_OUTPUT * output as f64
                    + WEIGHT_CACHE_CREATE * cache_create as f64
                    + WEIGHT_CACHE_READ * cache_read as f64;

                if weighted_units > 0.0 {
                    let input_cpt = cost / weighted_units;
                    let savings = saved as f64 * input_cpt;

                    self.weighted_input_cpt = Some(input_cpt);
                    self.savings_weighted = Some(savings);
                }
            }
        }
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
    cc_input_tokens: u64,
    cc_output_tokens: u64,
    cc_cache_create_tokens: u64,
    cc_cache_read_tokens: u64,
    rtk_commands: usize,
    rtk_saved_tokens: usize,
    rtk_avg_savings_pct: f64,
    weighted_input_cpt: Option<f64>,
    savings_weighted: Option<f64>,
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
    verbose: u8,
    audit: bool,
) -> Result<()> {
    let tracker = Tracker::new().context("Failed to initialize tracking database")?;

    match format {
        "json" => export_json(&tracker, daily, weekly, monthly, all),
        "csv" => export_csv(&tracker, daily, weekly, monthly, all),
        _ => display_text(&tracker, daily, weekly, monthly, all, verbose, audit),
    }
}

// ── Merge Logic ──

fn merge_daily(cc: Option<Vec<CcusagePeriod>>, rtk: Vec<DayStats>) -> Vec<PeriodEconomics> {
    let mut map: HashMap<String, PeriodEconomics> = HashMap::new();

    // Insert ccusage data
    if let Some(cc_data) = cc {
        for entry in cc_data {
            let super::ccusage::CcusagePeriod { key, metrics } = entry;
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
        period.compute_weighted_metrics();
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
            let super::ccusage::CcusagePeriod { key, metrics } = entry;
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
                eprintln!("[warn] Invalid week_start format: {}", entry.week_start);
                continue;
            }
        };

        map.entry(monday_key)
            .or_insert_with_key(|key| PeriodEconomics::new(key))
            .set_rtk_from_week(&entry);
    }

    let mut result: Vec<_> = map.into_values().collect();
    for period in &mut result {
        period.compute_weighted_metrics();
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
            let super::ccusage::CcusagePeriod { key, metrics } = entry;
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
        period.compute_weighted_metrics();
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
        cc_input_tokens: 0,
        cc_output_tokens: 0,
        cc_cache_create_tokens: 0,
        cc_cache_read_tokens: 0,
        rtk_commands: 0,
        rtk_saved_tokens: 0,
        rtk_avg_savings_pct: 0.0,
        weighted_input_cpt: None,
        savings_weighted: None,
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
        if let Some(input) = p.cc_input_tokens {
            totals.cc_input_tokens += input;
        }
        if let Some(output) = p.cc_output_tokens {
            totals.cc_output_tokens += output;
        }
        if let Some(cache_create) = p.cc_cache_create_tokens {
            totals.cc_cache_create_tokens += cache_create;
        }
        if let Some(cache_read) = p.cc_cache_read_tokens {
            totals.cc_cache_read_tokens += cache_read;
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

    // Compute global weighted metrics
    let weighted_units = totals.cc_input_tokens as f64
        + WEIGHT_OUTPUT * totals.cc_output_tokens as f64
        + WEIGHT_CACHE_CREATE * totals.cc_cache_create_tokens as f64
        + WEIGHT_CACHE_READ * totals.cc_cache_read_tokens as f64;

    if weighted_units > 0.0 {
        let input_cpt = totals.cc_cost / weighted_units;
        totals.weighted_input_cpt = Some(input_cpt);
        totals.savings_weighted = Some(totals.rtk_saved_tokens as f64 * input_cpt);
    }

    // Compute global dual metrics (legacy)
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
    verbose: u8,
    audit: bool,
) -> Result<()> {
    // Default: summary view
    if !daily && !weekly && !monthly && !all {
        display_summary(tracker, verbose, audit)?;
        return Ok(());
    }

    if all || daily {
        display_daily(tracker, verbose)?;
    }
    if all || weekly {
        display_weekly(tracker, verbose)?;
    }
    if all || monthly {
        display_monthly(tracker, verbose)?;
    }

    Ok(())
}

fn display_summary(tracker: &Tracker, verbose: u8, audit: bool) -> Result<()> {
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

    println!("[cost] Claude Code Economics");
    println!("════════════════════════════════════════════════════");
    println!();

    println!(
        "  Spent (ccusage):              {}",
        format_usd(totals.cc_cost)
    );
    println!("  Token breakdown:");
    println!(
        "    Input:                      {}",
        format_tokens(totals.cc_input_tokens as usize)
    );
    println!(
        "    Output:                     {}",
        format_tokens(totals.cc_output_tokens as usize)
    );
    println!(
        "    Cache writes:               {}",
        format_tokens(totals.cc_cache_create_tokens as usize)
    );
    println!(
        "    Cache reads:                {}",
        format_tokens(totals.cc_cache_read_tokens as usize)
    );
    println!();

    println!("  RTK commands:                 {}", totals.rtk_commands);
    println!(
        "  Tokens saved:                 {}",
        format_tokens(totals.rtk_saved_tokens)
    );
    println!();

    let audit_s = if audit {
        let projects_dir = dirs::home_dir().map(|h| h.join(".claude").join("projects"));
        match projects_dir {
            Some(dir) => match audit_precise_savings(tracker, &dir) {
                Ok(s) => Some(s),
                Err(e) => {
                    eprintln!("rtk: --audit failed, showing weighted estimate: {e:#}");
                    None
                }
            },
            None => {
                eprintln!("rtk: --audit failed: could not resolve home directory");
                None
            }
        }
    } else {
        None
    };
    let parsed_audited = audit_s.is_some() && totals.weighted_input_cpt.is_some();

    if parsed_audited {
        println!("  Audited Savings (Prompt Caching):");
    } else {
        println!("  Estimated Savings (Weighted):");
    }
    println!("  ┌─────────────────────────────────────────────────┐");

    if let (Some(s), Some(input_cpt)) = (audit_s, totals.weighted_input_cpt) {
        let p_write = input_cpt * WEIGHT_CACHE_CREATE;
        let p_read = input_cpt * WEIGHT_CACHE_READ;
        let pct = |x: f64| {
            if totals.cc_cost > 0.0 {
                x / totals.cc_cost * 100.0
            } else {
                0.0
            }
        };

        let write_savings = s.write_tokens as f64 * p_write;
        let read_savings = s.read_tokens as f64 * p_read;
        let recovery_cost = s.recovery_write as f64 * p_write + s.recovery_read as f64 * p_read;
        let total_savings = write_savings + read_savings - recovery_cost;

        let print_row = |content: &str| {
            println!("  │ {:<47} │", content);
        };
        print_row(&format!(
            "Cache write savings:   {}  ({:.1}%)",
            format_usd(write_savings).trim(),
            pct(write_savings)
        ));
        print_row(&format!(
            "Cache read savings:    {}  ({:.1}%)",
            format_usd(read_savings).trim(),
            pct(read_savings)
        ));
        if recovery_cost > 0.0 {
            print_row(&format!(
                "Recovery (re-read):   -{}  ({:.1}%)",
                format_usd(recovery_cost).trim(),
                pct(recovery_cost)
            ));
        }
        print_row("───────────────────────────────────────────────");
        print_row(&format!(
            "Total savings:         {}  ({:.1}%)",
            format_usd(total_savings).trim(),
            pct(total_savings)
        ));
        print_row(&format!(
            "Claude rtk calls:      {}/{} matched{}",
            s.matched,
            s.claude_invocations,
            if s.recoveries > 0 {
                format!(" \u{00b7} {} tee recoveries", s.recoveries)
            } else {
                String::new()
            }
        ));
        print_row(&format!(
            "Derived input CPT:     {}",
            format_cpt(input_cpt).trim()
        ));
    } else if let Some(weighted_savings) = totals.savings_weighted {
        let weighted_pct = if totals.cc_cost > 0.0 {
            (weighted_savings / totals.cc_cost) * 100.0
        } else {
            0.0
        };
        println!(
            "  │ Input token pricing:   {}  ({:.1}%)           │",
            format_usd(weighted_savings).trim_end(),
            weighted_pct
        );
        if let Some(input_cpt) = totals.weighted_input_cpt {
            println!(
                "  │ Derived input CPT:     {}               │",
                format_cpt(input_cpt)
            );
        }
    } else {
        println!("  │ Input token pricing:   —                         │");
    }

    println!("  └─────────────────────────────────────────────────┘");
    if !audit {
        println!(
            "  💡 Tip: Run with --audit to perform a precise session-log audit of prompt caching."
        );
    }
    println!();

    println!("  How it works:");
    println!("  RTK compresses CLI outputs before they enter Claude's context.");
    println!("  Savings derived using API price ratios (out=5x, cache_w=1.25x, cache_r=0.1x).");
    println!();

    // Verbose mode: legacy metrics
    if verbose > 0 {
        println!("  Legacy metrics (reference only):");
        if let Some(active_savings) = totals.savings_active {
            let active_pct = if totals.cc_cost > 0.0 {
                (active_savings / totals.cc_cost) * 100.0
            } else {
                0.0
            };
            println!(
                "    Active (OVERESTIMATES):  {}  ({:.1}%)",
                format_usd(active_savings),
                active_pct
            );
        }
        if let Some(blended_savings) = totals.savings_blended {
            let blended_pct = if totals.cc_cost > 0.0 {
                (blended_savings / totals.cc_cost) * 100.0
            } else {
                0.0
            };
            println!(
                "    Blended (UNDERESTIMATES): {}  ({:.2}%)",
                format_usd(blended_savings),
                blended_pct
            );
        }
        println!("  Note: Saved tokens estimated via chars/4 heuristic, not exact tokenizer.");
        println!();
    }

    Ok(())
}

fn display_daily(tracker: &Tracker, verbose: u8) -> Result<()> {
    let cc_daily =
        ccusage::fetch(Granularity::Daily).context("Failed to fetch ccusage daily data")?;
    let rtk_daily = tracker
        .get_all_days()
        .context("Failed to load daily token savings from database")?;
    let periods = merge_daily(cc_daily, rtk_daily);

    println!("Daily Economics");
    println!("════════════════════════════════════════════════════");
    print_period_table(&periods, verbose);
    Ok(())
}

fn display_weekly(tracker: &Tracker, verbose: u8) -> Result<()> {
    let cc_weekly =
        ccusage::fetch(Granularity::Weekly).context("Failed to fetch ccusage weekly data")?;
    let rtk_weekly = tracker
        .get_by_week()
        .context("Failed to load weekly token savings from database")?;
    let periods = merge_weekly(cc_weekly, rtk_weekly);

    println!("Weekly Economics");
    println!("════════════════════════════════════════════════════");
    print_period_table(&periods, verbose);
    Ok(())
}

fn display_monthly(tracker: &Tracker, verbose: u8) -> Result<()> {
    let cc_monthly =
        ccusage::fetch(Granularity::Monthly).context("Failed to fetch ccusage monthly data")?;
    let rtk_monthly = tracker
        .get_by_month()
        .context("Failed to load monthly token savings from database")?;
    let periods = merge_monthly(cc_monthly, rtk_monthly);

    println!("Monthly Economics");
    println!("════════════════════════════════════════════════════");
    print_period_table(&periods, verbose);
    Ok(())
}

fn print_period_table(periods: &[PeriodEconomics], verbose: u8) {
    println!();

    if verbose > 0 {
        // Verbose: include legacy metrics
        println!(
            "{:<12} {:>10} {:>10} {:>10} {:>10} {:>12} {:>12}",
            "Period", "Spent", "Saved", "Savings", "Active$", "Blended$", "RTK Cmds"
        );
        println!(
            "{:-<12} {:-<10} {:-<10} {:-<10} {:-<10} {:-<12} {:-<12}",
            "", "", "", "", "", "", ""
        );

        for p in periods {
            let spent = p.cc_cost.map(format_usd).unwrap_or_else(|| "—".to_string());
            let saved = p
                .rtk_saved_tokens
                .map(format_tokens)
                .unwrap_or_else(|| "—".to_string());
            let weighted = p
                .savings_weighted
                .map(format_usd)
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
                "{:<12} {:>10} {:>10} {:>10} {:>10} {:>12} {:>12}",
                p.label, spent, saved, weighted, active, blended, cmds
            );
        }
    } else {
        // Default: single Savings column
        println!(
            "{:<12} {:>10} {:>10} {:>10} {:>12}",
            "Period", "Spent", "Saved", "Savings", "RTK Cmds"
        );
        println!(
            "{:-<12} {:-<10} {:-<10} {:-<10} {:-<12}",
            "", "", "", "", ""
        );

        for p in periods {
            let spent = p.cc_cost.map(format_usd).unwrap_or_else(|| "—".to_string());
            let saved = p
                .rtk_saved_tokens
                .map(format_tokens)
                .unwrap_or_else(|| "—".to_string());
            let weighted = p
                .savings_weighted
                .map(format_usd)
                .unwrap_or_else(|| "—".to_string());
            let cmds = p
                .rtk_commands
                .map(|c| c.to_string())
                .unwrap_or_else(|| "—".to_string());

            println!(
                "{:<12} {:>10} {:>10} {:>10} {:>12}",
                p.label, spent, saved, weighted, cmds
            );
        }
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
    // Header (new columns: input_tokens, output_tokens, cache_create, cache_read, weighted_savings)
    println!("period,spent,input_tokens,output_tokens,cache_create,cache_read,active_tokens,total_tokens,saved_tokens,weighted_savings,active_savings,blended_savings,rtk_commands");

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
    let input_tokens = p.cc_input_tokens.map(|t| t.to_string()).unwrap_or_default();
    let output_tokens = p
        .cc_output_tokens
        .map(|t| t.to_string())
        .unwrap_or_default();
    let cache_create = p
        .cc_cache_create_tokens
        .map(|t| t.to_string())
        .unwrap_or_default();
    let cache_read = p
        .cc_cache_read_tokens
        .map(|t| t.to_string())
        .unwrap_or_default();
    let active_tokens = p
        .cc_active_tokens
        .map(|t| t.to_string())
        .unwrap_or_default();
    let total_tokens = p.cc_total_tokens.map(|t| t.to_string()).unwrap_or_default();
    let saved_tokens = p
        .rtk_saved_tokens
        .map(|t| t.to_string())
        .unwrap_or_default();
    let weighted_savings = p
        .savings_weighted
        .map(|s| format!("{:.4}", s))
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
        "{},{},{},{},{},{},{},{},{},{},{},{},{}",
        p.label,
        spent,
        input_tokens,
        output_tokens,
        cache_create,
        cache_read,
        active_tokens,
        total_tokens,
        saved_tokens,
        weighted_savings,
        active_savings,
        blended_savings,
        cmds
    );
}

struct SessionEvent {
    dt: DateTime<Utc>,
    /// True only for Bash tool_use events whose command invokes `rtk` - the
    /// authoritative signal that Claude drove an rtk-wrapped command.
    is_rtk: bool,
    /// True when the Bash command reads a tee file, re-introducing truncated output.
    is_recovery: bool,
    cwd: Option<String>,
    cache_write: u64,
    cache_read: u64,
}

#[derive(Debug, Deserialize)]
struct LogEntry {
    timestamp: String,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    message: Option<LogMessage>,
}

#[derive(Debug, Deserialize)]
struct LogMessage {
    content: Option<Vec<LogContent>>,
    usage: Option<LogUsage>,
}

#[derive(Debug, Deserialize)]
struct LogContent {
    #[serde(rename = "type")]
    content_type: String,
    name: Option<String>,
    input: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct LogUsage {
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

/// Per-bucket token savings attributed to rtk compression across audited sessions.
struct AuditSavings {
    /// Cache-write-rate billing (1.25x): tool-result appearance + eviction rebuilds.
    write_tokens: usize,
    /// Cache-read-rate billing (0.1x): steady cached turns.
    read_tokens: usize,
    /// Tokens re-introduced by the agent reading a truncated tee file back into
    /// context -- a debit against the savings the producing command claimed.
    /// Split by the same write/read turn buckets as the credit.
    recovery_write: usize,
    recovery_read: usize,
    /// rtk DB commands matched to a Claude session Bash call.
    matched: usize,
    /// Distinct `rtk ...` Bash events seen in Claude session logs (the authoritative
    /// cap; `matched` should be ~= this).
    claude_invocations: usize,
    /// Bash turns that read a tee file (rtk read or raw cat/tail/head), recovering
    /// output a producer command had truncated.
    recoveries: usize,
}

/// Substrings whose presence in a Bash command mark it as a *recovery read* -- the
/// agent reading a truncated tee file back into context. Covers the default tee dir
/// (`<data_local>/rtk/tee/`) on POSIX and Windows plus an `RTK_TEE_DIR` override.
/// Detection is loose by design: a non-read command (e.g. `rm`) touching the path
/// is filtered downstream because its result turn carries ~no cache_write debit.
fn tee_path_signals() -> Vec<String> {
    let mut sigs: Vec<String> = vec!["/rtk/tee/".into(), "\\rtk\\tee\\".into()];
    if let Ok(d) = std::env::var("RTK_TEE_DIR") {
        sigs.push(d);
    }
    sigs
}

fn is_tee_recovery(cmd: &str, signals: &[String]) -> bool {
    signals.iter().any(|s| cmd.contains(s))
}

struct RtkCommandAudit {
    dt: DateTime<Utc>,
    project: String,
    saved_tokens: usize,
    matched: bool,
}

fn audit_precise_savings(tracker: &Tracker, projects_dir: &Path) -> Result<AuditSavings> {
    let raw_cmds = tracker
        .get_raw_commands()
        .context("Failed to query raw commands")?;
    if raw_cmds.is_empty() {
        return Ok(AuditSavings {
            write_tokens: 0,
            read_tokens: 0,
            recovery_write: 0,
            recovery_read: 0,
            matched: 0,
            claude_invocations: 0,
            recoveries: 0,
        });
    }

    let mut rtk_cmds: Vec<RtkCommandAudit> = raw_cmds
        .into_iter()
        .map(|(dt, project, saved)| RtkCommandAudit {
            dt,
            project,
            saved_tokens: saved,
            matched: false,
        })
        .collect();

    // Earliest command timestamp minus 1 hour (mtime prune window). try_hours(1) is
    // infallible for a 1-hour delta; fall back to no offset rather than panic.
    let min_dt =
        rtk_cmds[0].dt - chrono::TimeDelta::try_hours(1).unwrap_or_else(chrono::TimeDelta::zero);

    let mut write_tokens = 0usize;
    let mut read_tokens = 0usize;
    let mut recovery_write = 0usize;
    let mut recovery_read = 0usize;
    let mut claude_invocations = 0usize;
    let mut recoveries = 0usize;
    // Built once: substrings that mark a Bash command as a tee-file recovery read.
    let tee_signals = tee_path_signals();

    if projects_dir.exists() {
        for entry in walkdir::WalkDir::new(projects_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "jsonl") {
                continue;
            }
            if let Ok(metadata) = entry.metadata() {
                if let Ok(mtime) = metadata.modified() {
                    let mtime_dt: DateTime<Utc> = mtime.into();
                    if mtime_dt < min_dt {
                        continue;
                    }
                }
            }

            // Parse + dedup by requestId. Assistant turns echo across streamed chunks,
            // so the same requestId/usage repeats; keep one turn per requestId.
            let mut by_req: HashMap<String, SessionEvent> = HashMap::new();
            if let Ok(file) = File::open(path) {
                for line in BufReader::new(file).lines().map_while(Result::ok) {
                    if !line.contains("\"requestId\"") {
                        continue;
                    }
                    let entry: LogEntry = match serde_json::from_str(&line) {
                        Ok(e) => e,
                        Err(_) => continue,
                    };
                    let Some(req_id) = entry.request_id else {
                        continue;
                    };
                    let cwd = entry.cwd;
                    let Ok(dt) = DateTime::parse_from_rfc3339(&entry.timestamp) else {
                        continue;
                    };
                    let dt_utc = dt.with_timezone(&Utc);
                    let mut is_rtk = false;
                    let mut is_recovery = false;
                    let mut cache_write = 0u64;
                    let mut cache_read = 0u64;
                    if let Some(msg) = entry.message {
                        if let Some(content_list) = msg.content {
                            for item in content_list {
                                if item.content_type == "tool_use" {
                                    let name = item.name.as_deref();
                                    // Field holding the target path/command: Bash ->
                                    // "command" (may be an rtk invocation), Read ->
                                    // "file_path"/"path" (never rtk). Both can recover a
                                    // tee file, so detection is tool-agnostic.
                                    let probe = match name {
                                        Some("Bash") => {
                                            item.input.as_ref().and_then(|i| i.get("command"))
                                        }
                                        Some("Read") => item.input.as_ref().and_then(|i| {
                                            i.get("file_path").or_else(|| i.get("path"))
                                        }),
                                        _ => None,
                                    };
                                    if let Some(v) = probe.and_then(|c| c.as_str()) {
                                        if name == Some("Bash") {
                                            let first = v.split_whitespace().next().unwrap_or("");
                                            is_rtk = first == "rtk" || first.ends_with("/rtk");
                                        }
                                        is_recovery = is_tee_recovery(v, &tee_signals);
                                    }
                                    if probe.is_some() {
                                        break;
                                    }
                                }
                            }
                        }
                        if let Some(u) = msg.usage {
                            cache_write = u.cache_creation_input_tokens;
                            cache_read = u.cache_read_input_tokens;
                        }
                    }
                    match by_req.get_mut(&req_id) {
                        // Usage is identical across echoes; OR the rtk flag, fill cwd.
                        Some(ev) => {
                            ev.is_rtk |= is_rtk;
                            ev.is_recovery |= is_recovery;
                            if ev.cwd.is_none() {
                                ev.cwd = cwd;
                            }
                        }
                        None => {
                            by_req.insert(
                                req_id,
                                SessionEvent {
                                    dt: dt_utc,
                                    is_rtk,
                                    is_recovery,
                                    cwd,
                                    cache_write,
                                    cache_read,
                                },
                            );
                        }
                    }
                }
            }

            claude_invocations += by_req.values().filter(|e| e.is_rtk).count();
            recoveries += by_req.values().filter(|e| e.is_recovery).count();
            let mut turns: Vec<SessionEvent> = by_req.into_values().collect();
            turns.sort_by_key(|e| e.dt);

            // Match each Bash turn to the nearest unmatched rtk command (<=5s), then
            // credit its saved tokens across every subsequent turn by that turn's bucket.
            for (i, ev) in turns.iter().enumerate() {
                // Only Bash events Claude invoked as `rtk ...` can match an rtk
                // command record - this is the authoritative cap on Claude-driven use.
                // Recovery reads are skipped here: crediting a tee-read would
                // double-count the producing command's truncation savings.
                if !ev.is_rtk || ev.is_recovery {
                    continue;
                }
                let mut best_idx: Option<usize> = None;
                // rtk stamps its record AFTER the command finishes, so the DB timestamp
                // lags the Claude Bash event by the command runtime. 60s recovers
                // ~99.9% of invocations while keeping the window tight enough to avoid
                // cross-session record stealing; the rtk-invocation + project filters
                // pin each match, and the rtk-invocation cap means we can never exceed
                // the number of events Claude actually drove.
                let mut best_diff = 60.0f64;
                for (j, cmd) in rtk_cmds.iter().enumerate() {
                    if cmd.matched {
                        continue;
                    }
                    // Scope to the same project: a Bash event may only claim an rtk
                    // command that ran in this session's cwd. Stops cross-project
                    // false matches (e.g. codex/terminal cmds near a Claude turn).
                    if ev.cwd.as_deref() != Some(cmd.project.as_str()) {
                        continue;
                    }
                    let diff = (cmd.dt - ev.dt).num_milliseconds().abs() as f64 / 1000.0;
                    if diff < best_diff {
                        best_diff = diff;
                        best_idx = Some(j);
                    }
                }
                let Some(j) = best_idx else { continue };
                rtk_cmds[j].matched = true;
                let m = rtk_cmds[j].saved_tokens;

                // Appearance turn (i+1): the tool result enters the cached prefix,
                // billed as a cache write. Validated via `claude -p`: the result's
                // turn always shows cache_creation > 0, even for tiny outputs.
                write_tokens += m;

                // Every later turn: the prefix (incl. the compressed region) is either
                // re-written (eviction rebuild: cache_write > cache_read) or re-read.
                for t in turns.iter().skip(i + 2) {
                    if t.cache_write > t.cache_read {
                        write_tokens += m;
                    } else {
                        read_tokens += m;
                    }
                }
            }

            // Recovery debit: a tee-read re-introduces tokens the producing command's
            // truncation had "saved", so net them back out. Mirrors the credit pass --
            // the result turn (i+1) enters the cached prefix as a write, then each
            // later turn re-reads or re-writes it. `r` is the result turn's cache_write
            // (tokens re-introduced); the r==0 gate drops non-read commands.
            for (i, ev) in turns.iter().enumerate() {
                if !ev.is_recovery {
                    continue;
                }
                let Some(result) = turns.get(i + 1) else {
                    continue;
                };
                let r = result.cache_write as usize;
                if r == 0 {
                    continue;
                }
                recovery_write += r;
                for t in turns.iter().skip(i + 2) {
                    if t.cache_write > t.cache_read {
                        recovery_write += r;
                    } else {
                        recovery_read += r;
                    }
                }
            }
        }
    }

    let matched = rtk_cmds.iter().filter(|c| c.matched).count();

    // Unmatched commands ran outside any Claude session (terminal/codex) and never
    // entered an LLM context, so they earn no savings under the authoritative-cap model.

    Ok(AuditSavings {
        write_tokens,
        read_tokens,
        recovery_write,
        recovery_read,
        matched,
        claude_invocations,
        recoveries,
    })
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
        let mut p = PeriodEconomics {
            label: "2026-01".to_string(),
            cc_cost: Some(100.0),
            cc_total_tokens: Some(1_000_000),
            cc_active_tokens: Some(10_000),
            rtk_saved_tokens: Some(5_000),
            ..PeriodEconomics::new("2026-01")
        };

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
        let mut p = PeriodEconomics {
            label: "2026-01".to_string(),
            cc_cost: Some(100.0),
            cc_total_tokens: Some(0),
            cc_active_tokens: Some(0),
            rtk_saved_tokens: Some(5_000),
            ..PeriodEconomics::new("2026-01")
        };

        p.compute_dual_metrics();

        assert!(p.blended_cpt.is_none());
        assert!(p.active_cpt.is_none());
        assert!(p.savings_blended.is_none());
        assert!(p.savings_active.is_none());
    }

    #[test]
    fn test_compute_dual_metrics_no_ccusage_data() {
        let mut p = PeriodEconomics {
            label: "2026-01".to_string(),
            rtk_saved_tokens: Some(5_000),
            ..PeriodEconomics::new("2026-01")
        };

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
            total_time_ms: 0,
            avg_time_ms: 0,
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
            total_time_ms: 0,
            avg_time_ms: 0,
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
                total_time_ms: 0,
                avg_time_ms: 0,
            },
            MonthStats {
                month: "2026-01".to_string(),
                commands: 10,
                input_tokens: 200,
                output_tokens: 100,
                saved_tokens: 2000,
                savings_pct: 60.0,
                total_time_ms: 0,
                avg_time_ms: 0,
            },
        ];

        let merged = merge_monthly(None, rtk);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].label, "2026-01");
        assert_eq!(merged[1].label, "2026-03");
    }

    #[test]
    fn test_compute_weighted_input_cpt() {
        let mut p = PeriodEconomics::new("2026-01");
        p.cc_cost = Some(100.0);
        p.cc_input_tokens = Some(1000);
        p.cc_output_tokens = Some(500);
        p.cc_cache_create_tokens = Some(200);
        p.cc_cache_read_tokens = Some(5000);
        p.rtk_saved_tokens = Some(10_000);

        p.compute_weighted_metrics();

        // weighted_units = 1000 + 5*500 + 1.25*200 + 0.1*5000 = 1000 + 2500 + 250 + 500 = 4250
        // input_cpt = 100 / 4250 = 0.0235294...
        // savings = 10000 * 0.0235294... = 235.29...

        assert!(p.weighted_input_cpt.is_some());
        let cpt = p.weighted_input_cpt.unwrap();
        assert!((cpt - (100.0 / 4250.0)).abs() < 1e-6);

        assert!(p.savings_weighted.is_some());
        let savings = p.savings_weighted.unwrap();
        assert!((savings - 235.294).abs() < 0.01);
    }

    #[test]
    fn test_compute_weighted_metrics_zero_tokens() {
        let mut p = PeriodEconomics::new("2026-01");
        p.cc_cost = Some(100.0);
        p.cc_input_tokens = Some(0);
        p.cc_output_tokens = Some(0);
        p.cc_cache_create_tokens = Some(0);
        p.cc_cache_read_tokens = Some(0);
        p.rtk_saved_tokens = Some(5000);

        p.compute_weighted_metrics();

        assert!(p.weighted_input_cpt.is_none());
        assert!(p.savings_weighted.is_none());
    }

    #[test]
    fn test_compute_weighted_metrics_no_cache() {
        let mut p = PeriodEconomics::new("2026-01");
        p.cc_cost = Some(60.0);
        p.cc_input_tokens = Some(1000);
        p.cc_output_tokens = Some(1000);
        p.cc_cache_create_tokens = Some(0);
        p.cc_cache_read_tokens = Some(0);
        p.rtk_saved_tokens = Some(3000);

        p.compute_weighted_metrics();

        // weighted_units = 1000 + 5*1000 = 6000
        // input_cpt = 60 / 6000 = 0.01
        // savings = 3000 * 0.01 = 30

        assert!(p.weighted_input_cpt.is_some());
        let cpt = p.weighted_input_cpt.unwrap();
        assert!((cpt - 0.01).abs() < 1e-6);

        assert!(p.savings_weighted.is_some());
        let savings = p.savings_weighted.unwrap();
        assert!((savings - 30.0).abs() < 0.01);
    }

    #[test]
    fn test_set_ccusage_stores_per_type_tokens() {
        let mut p = PeriodEconomics::new("2026-01");
        let metrics = ccusage::CcusageMetrics {
            input_tokens: 1000,
            output_tokens: 500,
            cache_creation_tokens: 200,
            cache_read_tokens: 3000,
            total_tokens: 4700,
            total_cost: 50.0,
        };

        p.set_ccusage(&metrics);

        assert_eq!(p.cc_input_tokens, Some(1000));
        assert_eq!(p.cc_output_tokens, Some(500));
        assert_eq!(p.cc_cache_create_tokens, Some(200));
        assert_eq!(p.cc_cache_read_tokens, Some(3000));
        assert_eq!(p.cc_total_tokens, Some(4700));
        assert_eq!(p.cc_cost, Some(50.0));
    }

    #[test]
    fn test_compute_totals() {
        let periods = vec![
            PeriodEconomics {
                label: "2026-01".to_string(),
                cc_cost: Some(100.0),
                cc_total_tokens: Some(1_000_000),
                cc_active_tokens: Some(10_000),
                cc_input_tokens: Some(5000),
                cc_output_tokens: Some(5000),
                cc_cache_create_tokens: Some(100),
                cc_cache_read_tokens: Some(984_900),
                rtk_commands: Some(5),
                rtk_saved_tokens: Some(2000),
                rtk_savings_pct: Some(50.0),
                weighted_input_cpt: None,
                savings_weighted: None,
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
                cc_input_tokens: Some(10_000),
                cc_output_tokens: Some(10_000),
                cc_cache_create_tokens: Some(200),
                cc_cache_read_tokens: Some(1_979_800),
                rtk_commands: Some(10),
                rtk_saved_tokens: Some(3000),
                rtk_savings_pct: Some(60.0),
                weighted_input_cpt: None,
                savings_weighted: None,
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
        assert_eq!(totals.cc_input_tokens, 15_000);
        assert_eq!(totals.cc_output_tokens, 15_000);
        assert_eq!(totals.rtk_commands, 15);
        assert_eq!(totals.rtk_saved_tokens, 5000);
        assert_eq!(totals.rtk_avg_savings_pct, 55.0);

        assert!(totals.weighted_input_cpt.is_some());
        assert!(totals.savings_weighted.is_some());
        assert!(totals.blended_cpt.is_some());
        assert!(totals.active_cpt.is_some());
    }

    /// Build one Claude session log line for tests.
    fn ev_read(
        ts: &str,
        req: &str,
        cwd: Option<&str>,
        file_path: &str,
        cw: u64,
        cr: u64,
    ) -> String {
        let cwd_field = match cwd {
            Some(c) => format!(",\"cwd\":\"{}\"", c),
            None => String::new(),
        };
        format!(
            "{{\"timestamp\":\"{}\",\"requestId\":\"{}\"{},\"message\":{{\"content\":[{{\"type\":\"tool_use\",\"name\":\"Read\",\"input\":{{\"file_path\":\"{}\"}}}}],\"usage\":{{\"cache_creation_input_tokens\":{},\"cache_read_input_tokens\":{}}}}}}}",
            ts, req, cwd_field, file_path, cw, cr
        )
    }

    fn ev(ts: &str, req: &str, cwd: Option<&str>, bash: Option<&str>, cw: u64, cr: u64) -> String {
        let content = match bash {
            Some(c) => format!(
                "[{{\"type\":\"tool_use\",\"name\":\"Bash\",\"input\":{{\"command\":\"{}\"}}}}]",
                c
            ),
            None => "[]".to_string(),
        };
        let cwd_field = match cwd {
            Some(c) => format!(",\"cwd\":\"{}\"", c),
            None => String::new(),
        };
        format!(
            "{{\"timestamp\":\"{}\",\"requestId\":\"{}\"{},\"message\":{{\"content\":{},\"usage\":{{\"cache_creation_input_tokens\":{},\"cache_read_input_tokens\":{}}}}}}}",
            ts, req, cwd_field, content, cw, cr
        )
    }

    #[test]
    fn test_audit_precise_savings() {
        let tracker = Tracker::new_in_memory().unwrap();
        tracker
            .record("cargo check", "rtk cargo check", 1000, 100, 50)
            .unwrap();
        let rows = tracker.get_raw_commands().unwrap();
        let cmd_time = rows[0].0;
        let project = rows[0].1.clone();
        let t0 = cmd_time.to_rfc3339();
        let t1 = (cmd_time + chrono::TimeDelta::try_seconds(1).unwrap()).to_rfc3339();
        let t2 = (cmd_time + chrono::TimeDelta::try_seconds(2).unwrap()).to_rfc3339();
        let t3 = (cmd_time + chrono::TimeDelta::try_seconds(3).unwrap()).to_rfc3339();

        let dir = tempfile::tempdir().unwrap();
        let projects_dir = dir.path().join("proj");
        std::fs::create_dir_all(&projects_dir).unwrap();
        // t0: rtk Bash call (match anchor). t1: appearance, cache_write>0 -> WRITE.
        // t2: read-dominant -> READ. t3: rebuild, cache_write>cache_read -> WRITE.
        std::fs::write(
            projects_dir.join("s.jsonl"),
            format!(
                "{}\n{}\n{}\n{}\n",
                ev(
                    &t0,
                    "req0",
                    Some(&project),
                    Some("rtk cargo check"),
                    0,
                    5000
                ),
                ev(&t1, "req1", None, None, 200, 6000),
                ev(&t2, "req2", None, None, 50, 7000),
                ev(&t3, "req3", None, None, 90000, 1000),
            ),
        )
        .unwrap();

        let s = audit_precise_savings(&tracker, &projects_dir).unwrap();
        assert_eq!(s.matched, 1);
        assert_eq!(s.claude_invocations, 1);
        assert_eq!(s.write_tokens, 1800); // appearance + rebuild
        assert_eq!(s.read_tokens, 900); // one read turn
    }

    #[test]
    fn test_audit_rtk_call_is_last_turn() {
        // The rtk Bash call is the last logged turn: no appearance/read turns follow,
        // so the saved tokens are credited once at the appearance (write) rate.
        let tracker = Tracker::new_in_memory().unwrap();
        tracker.record("c", "rtk c", 1000, 100, 1).unwrap();
        let rows = tracker.get_raw_commands().unwrap();
        let cmd_time = rows[0].0;
        let project = rows[0].1.clone();
        let t0 = cmd_time.to_rfc3339();
        let dir = tempfile::tempdir().unwrap();
        let projects_dir = dir.path().join("proj");
        std::fs::create_dir_all(&projects_dir).unwrap();
        std::fs::write(
            projects_dir.join("s.jsonl"),
            ev(&t0, "req0", Some(&project), Some("rtk c"), 0, 5000),
        )
        .unwrap();
        let s = audit_precise_savings(&tracker, &projects_dir).unwrap();
        assert_eq!(s.matched, 1);
        assert_eq!(s.write_tokens, 1000 - 100); // appearance only, no later turns
        assert_eq!(s.read_tokens, 0);
    }

    #[test]
    fn test_audit_recovery_debit_raw_cat() {
        // grep truncates output (credited), then the agent raw-cats the tee file
        // back into context. The re-read tokens debit the savings; the cat itself
        // earns no credit (it is not rtk and its tokens were "saved" by the grep).
        let tracker = Tracker::new_in_memory().unwrap();
        tracker
            .record("grep foo src/", "rtk grep foo src/", 1000, 200, 50)
            .unwrap();
        let rows = tracker.get_raw_commands().unwrap();
        let cmd_time = rows[0].0;
        let project = rows[0].1.clone();
        let t0 = cmd_time.to_rfc3339();
        let t1 = (cmd_time + chrono::TimeDelta::try_seconds(1).unwrap()).to_rfc3339();
        let t2 = (cmd_time + chrono::TimeDelta::try_seconds(2).unwrap()).to_rfc3339();
        let t3 = (cmd_time + chrono::TimeDelta::try_seconds(3).unwrap()).to_rfc3339();
        let tee = "/home/u/.local/share/rtk/tee/grep_0_src.log";

        let dir = tempfile::tempdir().unwrap();
        let projects_dir = dir.path().join("proj");
        std::fs::create_dir_all(&projects_dir).unwrap();
        std::fs::write(
            projects_dir.join("s.jsonl"),
            format!(
                "{}\n{}\n{}\n{}\n",
                ev(
                    &t0,
                    "req0",
                    Some(&project),
                    Some("rtk grep foo src/"),
                    0,
                    5000
                ),
                ev(&t1, "req1", None, None, 200, 6000),
                ev(
                    &t2,
                    "req2",
                    Some(&project),
                    Some(&format!("cat {}", tee)),
                    0,
                    7000
                ),
                ev(&t3, "req3", None, None, 600, 8000),
            ),
        )
        .unwrap();

        let s = audit_precise_savings(&tracker, &projects_dir).unwrap();
        assert_eq!(s.matched, 1);
        assert_eq!(s.recoveries, 1);
        assert_eq!(s.write_tokens, 800); // grep credited at the appearance turn (t1)
        assert_eq!(s.read_tokens, 1600); // t2, t3 read-bucket the saved prefix
        assert_eq!(s.recovery_write, 600); // cat result (t3) cache_write re-entered
        assert_eq!(s.recovery_read, 0);
    }

    #[test]
    fn test_audit_recovery_rtk_read_not_credited() {
        // An rtk-read of a tee file is rtk-driven BUT a recovery: crediting it would
        // double-count the grep's truncation, so it must be skipped and debited.
        let tracker = Tracker::new_in_memory().unwrap();
        tracker
            .record(
                "cat /home/u/.local/share/rtk/tee/x.log",
                "rtk read /home/u/.local/share/rtk/tee/x.log",
                800,
                600,
                50,
            )
            .unwrap();
        let rows = tracker.get_raw_commands().unwrap();
        let cmd_time = rows[0].0;
        let project = rows[0].1.clone();
        let t0 = cmd_time.to_rfc3339();
        let t1 = (cmd_time + chrono::TimeDelta::try_seconds(1).unwrap()).to_rfc3339();

        let dir = tempfile::tempdir().unwrap();
        let projects_dir = dir.path().join("proj");
        std::fs::create_dir_all(&projects_dir).unwrap();
        std::fs::write(
            projects_dir.join("s.jsonl"),
            format!(
                "{}\n{}\n",
                ev(
                    &t0,
                    "req0",
                    Some(&project),
                    Some("rtk read /home/u/.local/share/rtk/tee/x.log"),
                    0,
                    5000
                ),
                ev(&t1, "req1", None, None, 600, 6000),
            ),
        )
        .unwrap();

        let s = audit_precise_savings(&tracker, &projects_dir).unwrap();
        assert_eq!(s.matched, 0, "recovery read must not be credited");
        assert_eq!(s.recoveries, 1);
        assert_eq!(s.write_tokens, 0);
        assert_eq!(s.recovery_write, 600);
    }

    #[test]
    fn test_audit_recovery_read_tool() {
        // Same scenario as the raw-cat case, but the agent follows the breadcrumb via
        // Claude's Read tool instead of Bash. Recovery detection must be tool-agnostic.
        let tracker = Tracker::new_in_memory().unwrap();
        tracker
            .record("grep foo src/", "rtk grep foo src/", 1000, 200, 50)
            .unwrap();
        let rows = tracker.get_raw_commands().unwrap();
        let cmd_time = rows[0].0;
        let project = rows[0].1.clone();
        let t0 = cmd_time.to_rfc3339();
        let t1 = (cmd_time + chrono::TimeDelta::try_seconds(1).unwrap()).to_rfc3339();
        let t2 = (cmd_time + chrono::TimeDelta::try_seconds(2).unwrap()).to_rfc3339();
        let t3 = (cmd_time + chrono::TimeDelta::try_seconds(3).unwrap()).to_rfc3339();
        let tee = "/home/u/.local/share/rtk/tee/grep_0_src.log";

        let dir = tempfile::tempdir().unwrap();
        let projects_dir = dir.path().join("proj");
        std::fs::create_dir_all(&projects_dir).unwrap();
        std::fs::write(
            projects_dir.join("s.jsonl"),
            format!(
                "{}\n{}\n{}\n{}\n",
                ev(
                    &t0,
                    "req0",
                    Some(&project),
                    Some("rtk grep foo src/"),
                    0,
                    5000
                ),
                ev(&t1, "req1", None, None, 200, 6000),
                ev_read(&t2, "req2", Some(&project), tee, 0, 7000),
                ev(&t3, "req3", None, None, 600, 8000),
            ),
        )
        .unwrap();

        let s = audit_precise_savings(&tracker, &projects_dir).unwrap();
        assert_eq!(s.matched, 1);
        assert_eq!(
            s.recoveries, 1,
            "Read-tool tee read must count as a recovery"
        );
        assert_eq!(s.write_tokens, 800);
        assert_eq!(s.recovery_write, 600);
    }
}
