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
use std::path::{Path, PathBuf};

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
        if let Some(home) = dirs::home_dir() {
            let paths = vec![
                home.join(".claude").join("projects"),
                home.join(".pi").join("agent").join("sessions"),
                home.join(".gemini").join("antigravity-cli").join("brain"),
                home.join(".codex").join("sessions"),
            ];
            match audit_precise_savings(tracker, &paths) {
                Ok(s) => Some(s),
                Err(e) => {
                    eprintln!("rtk: --audit failed, showing weighted estimate: {e:#}");
                    None
                }
            }
        } else {
            eprintln!("rtk: --audit failed: could not resolve home directory");
            None
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
        let total_savings = write_savings + read_savings;

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
        print_row("───────────────────────────────────────────────");
        print_row(&format!(
            "Total savings:         {}  ({:.1}%)",
            format_usd(total_savings).trim(),
            pct(total_savings)
        ));
        print_row(&format!(
            "Audited rtk calls:     {}/{} matched",
            s.matched, s.claude_invocations
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
    /// True only for Bash/run_command/exec tool_use events whose command invokes `rtk` - the
    /// authoritative signal that an agent drove an rtk-wrapped command.
    is_rtk: bool,
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

#[derive(Debug, Deserialize)]
struct PiLogEntry {
    #[serde(rename = "type")]
    entry_type: String,
    id: Option<String>,
    timestamp: Option<String>,
    cwd: Option<String>,
    message: Option<PiMessage>,
    usage: Option<PiUsage>,
}

#[derive(Debug, Deserialize)]
struct PiMessage {
    role: Option<String>,
    content: Option<Vec<PiContent>>,
}

#[derive(Debug, Deserialize)]
struct PiContent {
    #[serde(rename = "type")]
    content_type: String,
    name: Option<String>,
    arguments: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct PiUsage {
    #[serde(rename = "cacheRead", default)]
    cache_read: u64,
    #[serde(rename = "cacheWrite", default)]
    cache_write: u64,
}

#[derive(Debug, Deserialize)]
struct AgyLogEntry {
    step_index: usize,
    created_at: String,
    tool_calls: Option<Vec<AgyToolCall>>,
}

#[derive(Debug, Deserialize)]
struct AgyToolCall {
    name: String,
    args: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct CodexLogEntry {
    timestamp: String,
    #[serde(rename = "type")]
    entry_type: String,
    payload: Option<CodexPayload>,
}

#[derive(Debug, Deserialize)]
struct CodexPayload {
    #[serde(rename = "type")]
    payload_type: Option<String>,
    id: Option<String>,
    call_id: Option<String>,
    role: Option<String>,
    name: Option<String>,
    input: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogFormat {
    Claude,
    Pi,
    Agy,
    Codex,
    Unknown,
}

fn detect_log_format(path: &Path) -> LogFormat {
    // Scan the whole file: Claude transcripts prefix many meta lines
    // (last-prompt, mode, summaries) before the first requestId, so a tiny
    // peek would misclassify 90%+ of Claude files as Unknown and skip them.
    // Each `return` stops at the first matching line, so well-formed files
    // bail out near the top regardless of size.
    if let Ok(file) = File::open(path) {
        let reader = BufReader::new(file);
        for line in reader.lines().map_while(Result::ok) {
            if line.contains("\"step_index\"") || line.contains("\"PLANNER_RESPONSE\"") {
                return LogFormat::Agy;
            }
            if line.contains("\"type\":\"session\"") || line.contains("\"type\":\"model_change\"") {
                return LogFormat::Pi;
            }
            if line.contains("\"session_meta\"") {
                return LogFormat::Codex;
            }
            if line.contains("\"requestId\"") {
                return LogFormat::Claude;
            }
        }
    }
    LogFormat::Unknown
}

fn extract_exec_command_cmd(input: &str) -> Option<(String, Option<String>)> {
    let start_pat = "tools.exec_command(";
    let idx = input.find(start_pat)?;
    let start_obj = idx + start_pat.len();
    let slice = &input[start_obj..];
    let end_obj = slice.rfind(')')?;
    let obj_str = slice[..end_obj].trim();
    let val: serde_json::Value = serde_json::from_str(obj_str).ok()?;
    let cmd = val.get("cmd")?.as_str()?.to_string();
    let workdir = val
        .get("workdir")
        .and_then(|w| w.as_str())
        .map(|s| s.to_string());
    Some((cmd, workdir))
}

fn is_rtk_or_proxied(cmd: &str) -> bool {
    let first = cmd.split_whitespace().next().unwrap_or("");
    first == "rtk"
        || first.ends_with("/rtk")
        || crate::discover::registry::rewrite_command(cmd, &[], &[]).is_some()
}

/// ponytail: canonicalize via filesystem to unify symlink-equivalent paths
/// (e.g. /var/services/homes vs /volume1/homes on Synology). Falls back to the
/// raw string when the path no longer exists (deleted worktrees).
fn norm_cwd(p: &str) -> String {
    std::fs::canonicalize(p)
        .map(|x| x.to_string_lossy().into_owned())
        .unwrap_or_else(|_| p.to_string())
}

/// Maximum clock skew tolerated when matching a session turn to an rtk DB
/// command. Session logs and the rtk DB stamp the same event at different
/// phases (model emit vs proxy execute); empirically the gap clusters in
/// 60-120s, so 180s gives margin without inviting cross-command false matches.
const MATCH_WINDOW_SECS: f64 = 120.0;

/// Per-bucket token savings attributed to rtk compression across audited sessions.
struct AuditSavings {
    /// Cache-write-rate billing (1.25x): tool-result appearance + eviction rebuilds.
    write_tokens: usize,
    /// Cache-read-rate billing (0.1x): steady cached turns.
    read_tokens: usize,
    /// rtk DB commands matched to an agent session Bash/command call.
    matched: usize,
    /// Distinct `rtk ...` Bash/command events seen in session logs (the authoritative
    /// cap; `matched` should be ~= this).
    claude_invocations: usize,
}

struct RtkCommandAudit {
    dt: DateTime<Utc>,
    project: String,
    saved_tokens: usize,
    matched: bool,
}

fn audit_precise_savings(tracker: &Tracker, projects_dirs: &[PathBuf]) -> Result<AuditSavings> {
    let raw_cmds = tracker
        .get_raw_commands()
        .context("Failed to query raw commands")?;
    if raw_cmds.is_empty() {
        return Ok(AuditSavings {
            write_tokens: 0,
            read_tokens: 0,
            matched: 0,
            claude_invocations: 0,
        });
    }

    let mut rtk_cmds: Vec<RtkCommandAudit> = raw_cmds
        .into_iter()
        .map(|(dt, project, saved)| RtkCommandAudit {
            dt,
            project: norm_cwd(&project),
            saved_tokens: saved,
            matched: false,
        })
        .collect();

    let min_dt =
        rtk_cmds[0].dt - chrono::TimeDelta::try_hours(1).unwrap_or_else(chrono::TimeDelta::zero);

    let mut write_tokens = 0usize;
    let mut read_tokens = 0usize;
    let mut claude_invocations = 0usize;

    // Turns gathered per file; matching is done globally after the walk so an
    // early file can't steal a command that belongs to a turn in a later file.
    let mut all_files: Vec<(LogFormat, Vec<SessionEvent>)> = Vec::new();

    for projects_dir in projects_dirs {
        if !projects_dir.exists() {
            continue;
        }
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

            let format = detect_log_format(path);
            if matches!(format, LogFormat::Unknown) {
                continue;
            }

            let mut by_req: HashMap<String, SessionEvent> = HashMap::new();
            let mut session_cwd: Option<String> = None;

            if let Ok(file) = File::open(path) {
                for line in BufReader::new(file).lines().map_while(Result::ok) {
                    match format {
                        LogFormat::Claude => {
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
                            let mut cache_write = 0u64;
                            let mut cache_read = 0u64;
                            if let Some(msg) = entry.message {
                                if let Some(content_list) = msg.content {
                                    for item in content_list {
                                        if item.content_type == "tool_use"
                                            && item.name.as_deref() == Some("Bash")
                                        {
                                            if let Some(cmd) = item
                                                .input
                                                .as_ref()
                                                .and_then(|i| i.get("command"))
                                                .and_then(|c| c.as_str())
                                            {
                                                let segments =
                                                    crate::discover::registry::split_command_chain(
                                                        cmd,
                                                    );
                                                for seg in segments {
                                                    if is_rtk_or_proxied(seg) {
                                                        is_rtk = true;
                                                        break;
                                                    }
                                                }
                                            }
                                            break;
                                        }
                                    }
                                }
                                if let Some(u) = msg.usage {
                                    cache_write = u.cache_creation_input_tokens;
                                    cache_read = u.cache_read_input_tokens;
                                }
                            }
                            match by_req.get_mut(&req_id) {
                                Some(ev) => {
                                    ev.is_rtk |= is_rtk;
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
                                            cwd,
                                            cache_write,
                                            cache_read,
                                        },
                                    );
                                }
                            }
                        }
                        LogFormat::Pi => {
                            let entry: PiLogEntry = match serde_json::from_str(&line) {
                                Ok(e) => e,
                                Err(_) => continue,
                            };
                            if let Some(cwd_str) = entry.cwd {
                                session_cwd = Some(cwd_str);
                            }
                            if entry.entry_type != "message" {
                                continue;
                            }
                            let Some(id) = entry.id else {
                                continue;
                            };
                            let Some(ts_str) = entry.timestamp else {
                                continue;
                            };
                            let Ok(dt) = DateTime::parse_from_rfc3339(&ts_str) else {
                                continue;
                            };
                            let dt_utc = dt.with_timezone(&Utc);
                            let mut is_rtk = false;
                            let mut cache_write = 0u64;
                            let mut cache_read = 0u64;
                            if let Some(msg) = &entry.message {
                                if msg.role.as_deref() == Some("assistant") {
                                    if let Some(content_list) = &msg.content {
                                        for content in content_list {
                                            if content.content_type == "toolCall"
                                                && content.name.as_deref() == Some("bash")
                                            {
                                                if let Some(args) = &content.arguments {
                                                    if let Some(cmd) =
                                                        args.get("command").and_then(|c| c.as_str())
                                                    {
                                                        let segments = crate::discover::registry::split_command_chain(cmd);
                                                        for seg in segments {
                                                            if is_rtk_or_proxied(seg) {
                                                                is_rtk = true;
                                                                break;
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            if let Some(u) = &entry.usage {
                                cache_write = u.cache_write;
                                cache_read = u.cache_read;
                            }
                            match by_req.get_mut(&id) {
                                Some(ev) => {
                                    ev.is_rtk |= is_rtk;
                                    if ev.cwd.is_none() {
                                        ev.cwd = session_cwd.clone();
                                    }
                                }
                                None => {
                                    by_req.insert(
                                        id,
                                        SessionEvent {
                                            dt: dt_utc,
                                            is_rtk,
                                            cwd: session_cwd.clone(),
                                            cache_write,
                                            cache_read,
                                        },
                                    );
                                }
                            }
                        }
                        LogFormat::Agy => {
                            let entry: AgyLogEntry = match serde_json::from_str(&line) {
                                Ok(e) => e,
                                Err(_) => continue,
                            };
                            let key = entry.step_index.to_string();
                            let Ok(dt) = DateTime::parse_from_rfc3339(&entry.created_at) else {
                                continue;
                            };
                            let dt_utc = dt.with_timezone(&Utc);
                            let mut is_rtk = false;
                            if let Some(tc) = &entry.tool_calls {
                                for call in tc {
                                    if call.name == "run_command" {
                                        if let Some(cmd) =
                                            call.args.get("CommandLine").and_then(|c| c.as_str())
                                        {
                                            let segments =
                                                crate::discover::registry::split_command_chain(cmd);
                                            for seg in segments {
                                                if is_rtk_or_proxied(seg) {
                                                    is_rtk = true;
                                                    break;
                                                }
                                            }
                                        }
                                        if let Some(c) =
                                            call.args.get("Cwd").and_then(|c| c.as_str())
                                        {
                                            session_cwd = Some(c.trim_matches('"').to_string());
                                        }
                                    }
                                }
                            }
                            by_req.insert(
                                key,
                                SessionEvent {
                                    dt: dt_utc,
                                    is_rtk,
                                    cwd: session_cwd.clone(),
                                    cache_write: 0,
                                    cache_read: 0,
                                },
                            );
                        }
                        LogFormat::Codex => {
                            let entry: CodexLogEntry = match serde_json::from_str(&line) {
                                Ok(e) => e,
                                Err(_) => continue,
                            };
                            let Ok(dt) = DateTime::parse_from_rfc3339(&entry.timestamp) else {
                                continue;
                            };
                            let dt_utc = dt.with_timezone(&Utc);
                            let mut is_rtk = false;
                            let mut should_record = false;
                            let mut key = String::new();

                            if entry.entry_type == "response_item" {
                                if let Some(payload) = &entry.payload {
                                    let p_type = payload.payload_type.as_deref();
                                    if p_type == Some("custom_tool_call")
                                        && payload.name.as_deref() == Some("exec")
                                    {
                                        should_record = true;
                                        key = payload
                                            .call_id
                                            .clone()
                                            .or_else(|| payload.id.clone())
                                            .unwrap_or_else(|| Utc::now().to_rfc3339());
                                        if let Some(input) = &payload.input {
                                            if let Some((cmd, workdir)) =
                                                extract_exec_command_cmd(input)
                                            {
                                                if let Some(wd) = workdir {
                                                    session_cwd = Some(wd);
                                                }
                                                let segments =
                                                    crate::discover::registry::split_command_chain(
                                                        &cmd,
                                                    );
                                                for seg in segments {
                                                    if is_rtk_or_proxied(seg) {
                                                        is_rtk = true;
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                    } else if p_type == Some("message")
                                        && payload.role.as_deref() == Some("assistant")
                                    {
                                        should_record = true;
                                        key = payload
                                            .id
                                            .clone()
                                            .unwrap_or_else(|| Utc::now().to_rfc3339());
                                    }
                                }
                            } else if entry.entry_type == "event_msg" {
                                if let Some(payload) = &entry.payload {
                                    if payload.payload_type.as_deref() == Some("user_message") {
                                        should_record = true;
                                        key = payload
                                            .id
                                            .clone()
                                            .unwrap_or_else(|| Utc::now().to_rfc3339());
                                    }
                                }
                            }

                            if should_record {
                                by_req.insert(
                                    key,
                                    SessionEvent {
                                        dt: dt_utc,
                                        is_rtk,
                                        cwd: session_cwd.clone(),
                                        cache_write: 0,
                                        cache_read: 0,
                                    },
                                );
                            }
                        }
                        LogFormat::Unknown => {}
                    }
                }
            }

            claude_invocations += by_req.values().filter(|e| e.is_rtk).count();
            let mut turns: Vec<SessionEvent> = by_req.into_values().collect();
            turns.sort_by_key(|e| e.dt);
            all_files.push((format, turns));
        }
    }

    // Global matching: pair each rtk session turn with the closest rtk DB command
    // that shares its (canonicalized) cwd and lies within MATCH_WINDOW_SECS. We
    // build every candidate edge, sort by ascending gap, and greedily assign so
    // the closest pairs lock in first — a per-file greedy mis-assigns because an
    // early file consumes commands that truly belong to turns in a later file.
    // edges: (gap_secs, file_idx, turn_idx, cmd_idx)
    let mut edges: Vec<(f64, usize, usize, usize)> = Vec::new();
    for (fi, (_fmt, turns)) in all_files.iter().enumerate() {
        for (ti, ev) in turns.iter().enumerate() {
            if !ev.is_rtk {
                continue;
            }
            let ev_cwd = ev.cwd.as_deref().map(norm_cwd);
            for (ci, cmd) in rtk_cmds.iter().enumerate() {
                if ev_cwd.as_deref() != Some(cmd.project.as_str()) {
                    continue;
                }
                let gap = (cmd.dt - ev.dt).num_milliseconds().abs() as f64 / 1000.0;
                if gap < MATCH_WINDOW_SECS {
                    edges.push((gap, fi, ti, ci));
                }
            }
        }
    }
    edges.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut turn_used: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    let mut assign: std::collections::HashMap<(usize, usize), usize> =
        std::collections::HashMap::new();
    for (_, fi, ti, ci) in edges {
        if rtk_cmds[ci].matched || turn_used.contains(&(fi, ti)) {
            continue;
        }
        rtk_cmds[ci].matched = true;
        turn_used.insert((fi, ti));
        assign.insert((fi, ti), ci);
    }

    // Per-file cache-lifecycle accounting: a matched command's saved tokens are
    // billed at the cache-write rate on appearance, then read/write on later
    // turns depending on each turn's cache_read vs cache_write balance.
    for (fi, (_fmt, turns)) in all_files.iter().enumerate() {
        for (i, ev) in turns.iter().enumerate() {
            if !ev.is_rtk {
                continue;
            }
            let Some(&ci) = assign.get(&(fi, i)) else {
                continue;
            };
            let m = rtk_cmds[ci].saved_tokens;
            write_tokens += m;
            for t in turns.iter().skip(i + 2) {
                if t.cache_write > t.cache_read {
                    write_tokens += m;
                } else {
                    read_tokens += m;
                }
            }
        }
    }

    let matched = rtk_cmds.iter().filter(|c| c.matched).count();

    Ok(AuditSavings {
        write_tokens,
        read_tokens,
        matched,
        claude_invocations,
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

        let s = audit_precise_savings(&tracker, std::slice::from_ref(&projects_dir)).unwrap();
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
        let s = audit_precise_savings(&tracker, std::slice::from_ref(&projects_dir)).unwrap();
        assert_eq!(s.matched, 1);
        assert_eq!(s.write_tokens, 1000 - 100); // appearance only, no later turns
        assert_eq!(s.read_tokens, 0);
    }

    #[test]
    fn test_audit_pi_session_logs() {
        let tracker = Tracker::new_in_memory().unwrap();
        tracker
            .record("git status", "rtk git status", 500, 50, 20)
            .unwrap();
        let rows = tracker.get_raw_commands().unwrap();
        let cmd_time = rows[0].0;
        let project = rows[0].1.clone();
        let t0 = cmd_time.to_rfc3339();
        let t1 = (cmd_time + chrono::TimeDelta::try_seconds(1).unwrap()).to_rfc3339();
        let t2 = (cmd_time + chrono::TimeDelta::try_seconds(2).unwrap()).to_rfc3339();

        let dir = tempfile::tempdir().unwrap();
        let sessions_dir = dir.path().join("proj");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        // Pi format JSONL
        let line0 = format!(
            r#"{{"type":"session","version":3,"id":"sess1","timestamp":"{}","cwd":"{}"}}"#,
            t0, project
        );
        let line1 = format!(
            r#"{{"type":"message","id":"msg1","timestamp":"{}","message":{{"role":"assistant","content":[{{"type":"toolCall","id":"c1","name":"bash","arguments":{{"command":"rtk git status"}}}}]}},"usage":{{"cacheRead":0,"cacheWrite":100}}}}"#,
            t0
        );
        let line2 = format!(
            r#"{{"type":"message","id":"msg2","timestamp":"{}","message":{{"role":"assistant","content":[]}},"usage":{{"cacheRead":200,"cacheWrite":0}}}}"#,
            t1
        );
        let line3 = format!(
            r#"{{"type":"message","id":"msg3","timestamp":"{}","message":{{"role":"assistant","content":[]}},"usage":{{"cacheRead":200,"cacheWrite":0}}}}"#,
            t2
        );

        std::fs::write(
            sessions_dir.join("pi_session.jsonl"),
            format!("{}\n{}\n{}\n{}\n", line0, line1, line2, line3),
        )
        .unwrap();

        let s = audit_precise_savings(&tracker, std::slice::from_ref(&sessions_dir)).unwrap();
        assert_eq!(s.matched, 1);
        assert_eq!(s.claude_invocations, 1);
        assert_eq!(s.write_tokens, 450); // appearance
        assert_eq!(s.read_tokens, 450); // subsequent turn cache read
    }

    #[test]
    fn test_audit_agy_session_logs() {
        let tracker = Tracker::new_in_memory().unwrap();
        tracker
            .record("cargo check", "rtk cargo check", 1000, 100, 30)
            .unwrap();
        let rows = tracker.get_raw_commands().unwrap();
        let cmd_time = rows[0].0;
        let project = rows[0].1.clone();
        let t0 = cmd_time.to_rfc3339();
        let t1 = (cmd_time + chrono::TimeDelta::try_seconds(1).unwrap()).to_rfc3339();
        let t2 = (cmd_time + chrono::TimeDelta::try_seconds(2).unwrap()).to_rfc3339();

        let dir = tempfile::tempdir().unwrap();
        let brain_dir = dir.path().join("brain");
        std::fs::create_dir_all(&brain_dir).unwrap();

        // Agy format JSONL
        let line1 = format!(
            r#"{{"step_index":0,"source":"MODEL","type":"PLANNER_RESPONSE","created_at":"{}","tool_calls":[{{"name":"run_command","args":{{"CommandLine":"rtk cargo check","Cwd":"{}"}}}}]}}"#,
            t0, project
        );
        let line2 = format!(
            r#"{{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","created_at":"{}","tool_calls":[]}}"#,
            t1
        );
        let line3 = format!(
            r#"{{"step_index":2,"source":"MODEL","type":"PLANNER_RESPONSE","created_at":"{}","tool_calls":[]}}"#,
            t2
        );

        std::fs::write(
            brain_dir.join("transcript.jsonl"),
            format!("{}\n{}\n{}\n", line1, line2, line3),
        )
        .unwrap();

        let s = audit_precise_savings(&tracker, std::slice::from_ref(&brain_dir)).unwrap();
        assert_eq!(s.matched, 1);
        assert_eq!(s.claude_invocations, 1);
        assert_eq!(s.write_tokens, 900); // appearance write
        assert_eq!(s.read_tokens, 900); // subsequent turn read (heuristic default)
    }

    #[test]
    fn test_audit_codex_session_logs() {
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

        let dir = tempfile::tempdir().unwrap();
        let sessions_dir = dir.path().join("proj");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        // Codex format JSONL
        let line0 = format!(
            r#"{{"timestamp":"{}","type":"session_meta","payload":{{"session_id":"s1","cwd":"{}"}}}}"#,
            t0, project
        );
        let line1 = format!(
            r#"{{"timestamp":"{}","type":"response_item","payload":{{"type":"custom_tool_call","call_id":"c1","name":"exec","input":"const r = await tools.exec_command({{\"cmd\":\"rtk cargo check\",\"workdir\":\"{}\"}});"}}}}"#,
            t0, project
        );
        let line2 = format!(
            r#"{{"timestamp":"{}","type":"response_item","payload":{{"type":"message","id":"msg1","role":"assistant"}}}}"#,
            t1
        );
        let line3 = format!(
            r#"{{"timestamp":"{}","type":"response_item","payload":{{"type":"message","id":"msg2","role":"assistant"}}}}"#,
            t2
        );

        std::fs::write(
            sessions_dir.join("codex_session.jsonl"),
            format!("{}\n{}\n{}\n{}\n", line0, line1, line2, line3),
        )
        .unwrap();

        let s = audit_precise_savings(&tracker, std::slice::from_ref(&sessions_dir)).unwrap();
        assert_eq!(s.matched, 1);
        assert_eq!(s.claude_invocations, 1);
        assert_eq!(s.write_tokens, 900); // appearance write
    }
}
