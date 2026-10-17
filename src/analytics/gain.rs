//! Shows users how many tokens RTK has saved them over time.

use crate::core::display_helpers::{format_duration, print_period_table};
use crate::core::tracking::{DayStats, MonthStats, Tracker, WeekStats};
use crate::core::user_dirs;
use crate::core::utils::{format_tokens, truncate};
use crate::hooks::hook_check;
use anyhow::{Context, Result};
use chrono::Local;
use colored::Colorize;
use serde::Serialize;
use std::io::IsTerminal;
use std::path::PathBuf;

/// `--format json` for the attribution views: the same rows the text view
/// prints, as data. Sections appear only when their flag was asked for.
fn attribution_json(
    tracker: &Tracker,
    by_method: bool,
    by_rule: bool,
    by_layer: bool,
    by_pipeline: bool,
    shadow: bool,
    days: u32,
) -> Result<()> {
    use serde_json::json;
    let rows = |site: &str, dim: &str| -> Result<Vec<serde_json::Value>> {
        Ok(tracker
            .attribution_site(site, dim, days)?
            .iter()
            .map(|r| {
                json!({
                    "name": r.name,
                    "calls": r.calls,
                    "tokens_in": r.tokens_in,
                    "tokens_out": r.tokens_out,
                    "saved": r.saved(),
                })
            })
            .collect())
    };
    let mut out = serde_json::Map::new();
    out.insert("window_days".into(), json!(days));
    if shadow {
        out.insert("shadow".into(), json!(rows("shadow", "rule")?));
    }
    if by_method {
        let mut section = serde_json::Map::new();
        section.insert("hook".into(), json!(rows("hook", "method")?));
        if let Some(proxy) = crate::llm_proxy::read_gain_summary() {
            let methods: Vec<serde_json::Value> = proxy
                .savings_by_method()
                .into_iter()
                .map(|(name, saved)| json!({"name": name, "saved": saved}))
                .collect();
            section.insert("proxy".into(), json!(methods));
            section.insert("retrieve_calls".into(), json!(proxy.retrieve_calls));
        }
        out.insert("by_method".into(), json!(section));
    }
    if by_rule {
        out.insert("by_rule".into(), json!(rows("hook", "rule")?));
    }
    if by_pipeline {
        out.insert("by_pipeline".into(), json!(rows("hook", "pipeline")?));
    }
    if by_layer {
        let mut per_layer: std::collections::BTreeMap<&'static str, (i64, i64)> =
            std::collections::BTreeMap::new();
        for row in tracker.attribution_site("hook", "method", days)? {
            #[cfg(feature = "engine")]
            let layer = rtk_engine::registry::info_of(&row.name)
                .and_then(|info| info.saves_on.first().copied())
                .map(|l| l.as_str())
                .unwrap_or("tool_result");
            #[cfg(not(feature = "engine"))]
            let layer = "tool_result";
            let slot = per_layer.entry(layer).or_default();
            slot.0 += row.calls;
            slot.1 += row.saved().max(0);
        }
        let layers: Vec<serde_json::Value> = per_layer
            .iter()
            .map(|(layer, (calls, saved))| json!({"layer": layer, "calls": calls, "saved": saved}))
            .collect();
        out.insert("by_layer".into(), json!(layers));
    }
    let tasks = rows("task", "task")?;
    if !tasks.is_empty() {
        out.insert("tasks".into(), json!(tasks));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::Value::Object(out))?
    );
    Ok(())
}

/// The `--reset` action, shared by every `rtk gain` arm so no flag
/// combination can silently drop it. Returns whether the reset ran.
pub fn run_reset(yes: bool) -> Result<bool> {
    if !yes && !confirm_reset()? {
        println!("Aborted.");
        return Ok(false);
    }
    let tracker = Tracker::new().context("Failed to initialize tracking database")?;
    tracker
        .reset_all()
        .context("Failed to reset token savings")?;
    crate::core::retriever::reset_stats().context("Failed to reset recall stats")?;
    println!(
        "{}",
        styled("Token savings and recall stats reset to zero.", true)
    );
    Ok(true)
}

pub fn run_attribution(
    by_method: bool,
    by_rule: bool,
    by_layer: bool,
    by_pipeline: bool,
    shadow: bool,
    format: &str,
) -> Result<()> {
    let tracker = Tracker::new().context("Failed to open tracking database")?;
    const DAYS: u32 = 30;

    if format == "json" {
        return attribution_json(
            &tracker,
            by_method,
            by_rule,
            by_layer,
            by_pipeline,
            shadow,
            DAYS,
        );
    }

    let print_rows = |title: &str, rows: &[crate::core::tracking::AttributionRow]| {
        println!("{}", title.bold());
        if rows.is_empty() {
            println!("  (no data in the last {DAYS} days)");
            println!();
            return;
        }
        for row in rows {
            let pct = if row.tokens_in > 0 {
                row.saved() as f64 / row.tokens_in as f64 * 100.0
            } else {
                0.0
            };
            println!(
                "  {:<28} {:>6} calls  {:>10} saved  ({:.0}%)",
                truncate(&row.name, 28),
                row.calls,
                format_tokens(row.saved().max(0) as usize),
                pct
            );
        }
        println!();
    };

    if shadow {
        print_rows(
            &format!("Shadow — recorded, nothing emitted (last {DAYS}d)"),
            &tracker.attribution_site("shadow", "rule", DAYS)?,
        );
        println!("turn `shadow` off on a rule when its numbers convince you.");
        println!();
    }

    if by_layer {
        let mut per_layer: std::collections::BTreeMap<&'static str, (i64, i64)> =
            std::collections::BTreeMap::new();
        for row in tracker.attribution_site("hook", "method", DAYS)? {
            #[cfg(feature = "engine")]
            let layer = rtk_engine::registry::info_of(&row.name)
                .and_then(|info| info.saves_on.first().copied())
                .map(|l| l.as_str())
                .unwrap_or("tool_result");
            #[cfg(not(feature = "engine"))]
            let layer = "tool_result";
            let slot = per_layer.entry(layer).or_default();
            slot.0 += row.calls;
            slot.1 += row.saved().max(0);
        }
        if let Some(proxy) = crate::llm_proxy::read_gain_summary() {
            for (name, saved) in proxy.savings_by_method() {
                #[cfg(not(feature = "engine"))]
                let _ = name;
                #[cfg(feature = "engine")]
                let layer = rtk_engine::registry::info_of(name)
                    .and_then(|info| info.saves_on.first().copied())
                    .map(|l| l.as_str())
                    .unwrap_or("history");
                #[cfg(not(feature = "engine"))]
                let layer = "history";
                let slot = per_layer.entry(layer).or_default();
                slot.1 += saved.max(0);
            }
        }
        println!("{}", format!("By Layer (last {DAYS}d)").bold());
        if per_layer.is_empty() {
            println!("  (no data)");
        }
        for (layer, (calls, saved)) in &per_layer {
            println!(
                "  {:<14} {:>6} calls  {:>10} saved",
                layer,
                calls,
                format_tokens((*saved).max(0) as usize)
            );
        }
        println!();
    }

    let tasks = tracker.attribution_site("task", "task", DAYS)?;
    if !tasks.is_empty() {
        // Context reduction: the corpus WOULD have put `tokens_in` into the
        // agent's context; the summary that came back is `tokens_out`. The
        // difference is context the delegation kept out — a pure token ratio,
        // no prices. The worker's own spend is a cost, shown on the proxy
        // Routed line, never netted against these.
        println!(
            "{}",
            format!("Tasks — context kept out (last {DAYS}d)").bold()
        );
        for row in &tasks {
            let kept = (row.tokens_in - row.tokens_out).max(0);
            let pct = if row.tokens_in > 0 {
                kept as f64 / row.tokens_in as f64 * 100.0
            } else {
                0.0
            };
            println!(
                "  {:<28} {:>6} calls  {:>10} of {:>9} kept out ({:.0}%) est.",
                truncate(&row.name, 28),
                row.calls,
                format_tokens(kept as usize),
                format_tokens(row.tokens_in.max(0) as usize),
                pct
            );
        }
        println!();
    }

    if by_method {
        print_rows(
            &format!("By Method — hook site (last {DAYS}d)"),
            &tracker.attribution_site("hook", "method", DAYS)?,
        );
        if let Some(proxy) = crate::llm_proxy::read_gain_summary() {
            let methods = proxy.savings_by_method();
            if !methods.is_empty() {
                println!("{}", "By Method — proxy site (all time)".bold());
                for (name, saved) in methods {
                    println!("  {:<28} {:>10} saved", name, format_tokens(saved as usize));
                }
                if proxy.retrieve_calls > 0 {
                    println!(
                        "  {:<28} {:>10} recovery calls",
                        "ccr (rtk_retrieve)", proxy.retrieve_calls
                    );
                }
                println!();
            }
        }
    }

    if by_rule {
        print_rows(
            &format!("By Rule — hook site (last {DAYS}d)"),
            &tracker.attribution_site("hook", "rule", DAYS)?,
        );
    }

    if by_pipeline {
        print_rows(
            &format!("By Pipeline — hook site (last {DAYS}d)"),
            &tracker.attribution_site("hook", "pipeline", DAYS)?,
        );
        if let Some(rows) = crate::llm_proxy::savings_by_pipeline()
            && !rows.is_empty()
        {
            println!("{}", "By Pipeline — proxy site (all time)".bold());
            for row in &rows {
                let name = if row.name.is_empty() {
                    "(unknown)"
                } else {
                    &row.name
                };
                let raw = row.tokens_in + row.tokens_saved;
                let pct = if raw > 0 {
                    row.tokens_saved as f64 / raw as f64 * 100.0
                } else {
                    0.0
                };
                println!(
                    "  {:<28} {:>6} req    {:>10} saved  ({pct:.0}%)",
                    truncate(name, 28),
                    row.requests,
                    format_tokens(row.tokens_saved.max(0) as usize),
                );
            }
            println!();
        }
    }

    let recoveries = tracker.attribution("recovery", DAYS)?;
    if !recoveries.is_empty() {
        println!(
            "{}",
            format!("Recovery reads — hook site (last {DAYS}d)").bold()
        );
        for row in &recoveries {
            println!("  {:<28} {:>6} recall reads", row.name, row.calls);
        }
        println!();
    }

    Ok(())
}

/// Reader-facing name for a hook slice subject — which kind of tool output
/// the pipeline trimmed before it re-entered the context window.
pub fn subject_slice_label(subject: &str) -> &str {
    match subject {
        "shell" => "shell output (bash)",
        "read" => "file reads",
        "search" => "search results",
        "web" => "web content",
        "mcp" => "MCP tool output",
        other => other,
    }
}

/// The hook-site slices recorded for one pipeline over the window:
/// `(subject label, calls, tokens_in, saved)`, largest savings first.
pub fn hook_slices_for(
    tracker: &Tracker,
    pipeline: &str,
    days: u32,
) -> Result<Vec<(String, i64, i64, i64)>> {
    let mut out: Vec<(String, i64, i64, i64)> = tracker
        .attribution_site("hook", "pipeline-slice", days)?
        .into_iter()
        .filter_map(|row| {
            let (pipe, subject) = crate::core::tracking::split_pipeline_slice(&row.name)?;
            (pipe == pipeline).then(|| {
                (
                    subject_slice_label(subject).to_string(),
                    row.calls,
                    row.tokens_in,
                    row.saved().max(0),
                )
            })
        })
        .collect();
    out.sort_by_key(|(_, _, _, saved)| std::cmp::Reverse(*saved));
    Ok(out)
}

/// `rtk gain --pipeline <name>`: one pipeline, sliced by where its savings
/// actually land — which tool outputs the hooks trimmed, and which methods
/// compressed billed input tokens at the proxy.
pub fn run_pipeline_detail(name: &str) -> Result<()> {
    let sanitized = crate::core::utils::sanitize_for_terminal(name);
    let name = sanitized.as_str();
    let tracker = Tracker::new().context("Failed to open tracking database")?;
    const DAYS: u32 = 30;

    let hook_total = tracker
        .attribution_site("hook", "pipeline", DAYS)?
        .into_iter()
        .find(|r| r.name == name);
    let slices = hook_slices_for(&tracker, name, DAYS)?;
    let tasks: Vec<_> = tracker
        .attribution_site("task", "task", DAYS)
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.tokens_in > 0)
        .collect();
    let proxy = crate::llm_proxy::savings_by_pipeline()
        .and_then(|rows| rows.into_iter().find(|r| r.name == name));

    if hook_total.is_none() && slices.is_empty() && proxy.is_none() && tasks.is_empty() {
        println!("no recorded savings for pipeline `{name}` (last {DAYS}d hook, all-time proxy)");
        let mut known: Vec<String> = tracker
            .attribution_site("hook", "pipeline", DAYS)?
            .into_iter()
            .map(|r| r.name)
            .collect();
        if let Some(rows) = crate::llm_proxy::savings_by_pipeline() {
            known.extend(
                rows.into_iter()
                    .filter(|r| !r.name.is_empty())
                    .map(|r| r.name),
            );
        }
        known.sort();
        known.dedup();
        if !known.is_empty() {
            println!("pipelines with data: {}", known.join(", "));
        }
        return Ok(());
    }

    println!("{}", styled(&format!("Pipeline: {name}"), true));
    println!("{}", "═".repeat(60));
    println!(
        "{}",
        "last 30d hooks and delegation · all-time proxy · figures est. (≈ bytes/4)".dimmed()
    );
    println!();

    if hook_total.is_some() || !slices.is_empty() {
        println!(
            "{}",
            styled(
                "Agent hooks — tool output trimmed before it re-enters context (last 30d)",
                true
            )
        );
        println!("{}", "─".repeat(60));
        for (label, calls, tokens_in, saved) in &slices {
            print_pipeline_row(
                &truncate(label, 22),
                format!("{calls} calls"),
                *saved,
                pct_of(*saved, *tokens_in),
            );
        }
        if let Some(total) = &hook_total {
            print_pipeline_row(
                "total",
                format!("{} calls", total.calls),
                total.saved().max(0),
                pct_of(total.saved().max(0), total.tokens_in),
            );
            let sliced: i64 = slices.iter().map(|(_, _, _, saved)| *saved).sum();
            if sliced < total.saved().max(0) {
                println!(
                    "  {}",
                    "(rows recorded before per-slice tracking count only in the total)".dimmed()
                );
            }
        }
        println!();
    }

    if !tasks.is_empty() {
        println!(
            "{}",
            styled(
                "Delegation — context a worker read for you, kept out of your window (session-wide, last 30d)",
                true
            )
        );
        println!("{}", "─".repeat(60));
        for t in &tasks {
            let kept = (t.tokens_in - t.tokens_out).max(0);
            println!(
                "  {:<22} {:>8}  {:>10} kept out of context",
                truncate(&t.name, 22),
                format!("{} reads", t.calls),
                format_tokens(kept as usize),
            );
        }
        println!();
    }

    if let Some(row) = &proxy {
        println!(
            "{}",
            styled(
                "LLM proxy — billed input tokens compressed before forwarding (all time)",
                true
            )
        );
        println!("{}", "─".repeat(60));
        let raw = row.tokens_in + row.tokens_saved;
        print_pipeline_row(
            "input tokens",
            format!("{} req", row.requests),
            row.tokens_saved.max(0),
            pct_of(row.tokens_saved.max(0), raw),
        );
        for (method, saved) in row.savings_by_method() {
            println!(
                "    {:<22} {:>10} saved",
                truncate(method, 22),
                format_tokens(saved.max(0) as usize)
            );
        }
        if row.cache_read + row.cache_created > 0 {
            let hit = row.cache_read as f64 / (row.cache_read + row.cache_created) as f64 * 100.0;
            print_kpi(
                "  prompt cache",
                format!(
                    "{} read · {} written  (hit {hit:.0}%)",
                    format_tokens(row.cache_read.max(0) as usize),
                    format_tokens(row.cache_created.max(0) as usize),
                ),
            );
            let cache_raw = row.tokens_in + row.cache_read + row.cache_created;
            if cache_raw > 0 {
                let billed = row.tokens_in as f64
                    + row.cache_read as f64 * 0.1
                    + row.cache_created as f64 * 1.25;
                let effect = (1.0 - billed / cache_raw as f64) * 100.0;
                print_kpi(
                    "  cache effect",
                    format!(
                        "input billed at ~{:.0}% of raw (-{:.0}%) est.",
                        100.0 - effect,
                        effect
                    ),
                );
            }
        }
        println!();
    }

    println!(
        "{}",
        "Output tokens are not compressed by pipelines; directives (e.g. terse-output) shape them \
         but are not metered per pipeline yet."
            .dimmed()
    );
    Ok(())
}

fn pct_of(saved: i64, base: i64) -> f64 {
    if base > 0 {
        saved as f64 / base as f64 * 100.0
    } else {
        0.0
    }
}

fn pipeline_meter(pct: f64) -> String {
    let width = 10usize;
    let filled = (((pct / 100.0) * width as f64).round() as usize).min(width);
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(width - filled));
    if std::io::stdout().is_terminal() {
        if pct >= 60.0 {
            bar.green().to_string()
        } else if pct >= 25.0 {
            bar.yellow().to_string()
        } else {
            bar.dimmed().to_string()
        }
    } else {
        bar
    }
}

fn print_pipeline_row(label: &str, activity: String, saved: i64, pct: f64) {
    println!(
        "  {:<22} {:>8}  {:>10} saved  {} {:>3.0}%",
        label,
        activity,
        format_tokens(saved.max(0) as usize),
        pipeline_meter(pct),
        pct,
    );
}

/// Reports a missing or outdated hook on stderr.
///
/// `suppress_hook_warning` hides the missing-hook arm: a user who runs rtk
/// without hooks on purpose reads this report most often. The outdated-hook
/// arm stays visible either way.
fn warn_hook_issues() {
    match hook_check::status() {
        hook_check::HookStatus::Missing if !crate::core::config::hook_warning_suppressed() => {
            eprintln!(
                "{}",
                "[warn] No hook installed — run `rtk init -g` for automatic token savings".yellow()
            );
            eprintln!();
        }
        hook_check::HookStatus::Outdated => {
            eprintln!(
                "{}",
                "[warn] Hook outdated — run `rtk init -g` to update".yellow()
            );
            eprintln!();
        }
        hook_check::HookStatus::Missing | hook_check::HookStatus::Ok => {}
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    project: bool, // added: per-project scope flag
    graph: bool,
    history: bool,
    quota: bool,
    tier: &str,
    daily: bool,
    weekly: bool,
    monthly: bool,
    all: bool,
    format: &str,
    failures: bool,
    recalls: bool,
    reset: bool,
    yes: bool,
    watch: bool,
    text: bool,
    _verbose: u8,
) -> Result<()> {
    if recalls && !reset {
        return show_recall_stats();
    }
    let tracker = Tracker::new().context("Failed to initialize tracking database")?;
    let project_scope = resolve_project_scope(project)?; // added: resolve project path

    if reset {
        run_reset(yes)?;
        return Ok(());
    }

    if failures {
        return show_failures(&tracker);
    }

    // A bare `rtk gain` auto-launches the live dashboard once the llm-proxy
    // is running -- any flag that asks for a specific static view (a time
    // breakdown, an export format, history/graph/quota, or an explicit
    // --text) opts back out, and so does a non-TTY stdout (piped/redirected).
    let default_view_requested =
        !daily && !weekly && !monthly && !all && !history && !graph && !quota && format == "text";
    let stdout_is_tty = std::io::stdout().is_terminal();
    let auto_watch =
        !text && default_view_requested && stdout_is_tty && crate::llm_proxy::is_reachable();

    match watch_dispatch(watch, auto_watch, stdout_is_tty) {
        WatchDispatch::Tui => return super::gain_tui::run(project_scope.as_deref()),
        WatchDispatch::TextFallback => {
            eprintln!("rtk: --watch needs a terminal; showing the text view instead");
        }
        WatchDispatch::Text => {}
    }

    // Handle export formats
    match format {
        "json" => {
            return export_json(
                &tracker,
                daily,
                weekly,
                monthly,
                all,
                project_scope.as_deref(), // added: pass project scope
            );
        }
        "csv" => {
            return export_csv(
                &tracker,
                daily,
                weekly,
                monthly,
                all,
                project_scope.as_deref(), // added: pass project scope
            );
        }
        _ => {} // Continue with text format
    }

    let summary = tracker
        .get_summary_filtered(project_scope.as_deref()) // changed: use filtered variant
        .context("Failed to load token savings summary from database")?;

    if crate::core::tee_file::legacy_tee_migration_pending() {
        eprintln!(
            "{}",
            format!("[rtk] {}", crate::core::tee_file::LEGACY_TEE_NOTICE).yellow()
        );
        eprintln!();
    }

    // Separate savings source (LLM request/response compression, not
    // command-output filtering) -- see `llm_proxy::read_gain_summary`.
    let proxy_summary = crate::llm_proxy::read_gain_summary().filter(|p| p.total_requests > 0);

    // Reported for every text view, including the empty one: with no hook
    // nothing is tracked, so a missing hook is the likeliest reason there is
    // nothing to show. The JSON and CSV exports return earlier and stay clean.
    warn_hook_issues();

    if summary.total_commands == 0 && proxy_summary.is_none() {
        println!("No tracking data yet.");
        println!("Run some rtk commands to start tracking savings.");
        // A reachable daemon with no rows is the confusing case: the user has
        // started it and expects numbers. Say what is actually missing. Printed
        // only when it is running, so this costs nothing for anyone who does
        // not use the proxy.
        if crate::llm_proxy::is_reachable() {
            println!("LLM proxy: {}", crate::llm_proxy::no_data_hint());
        }
        println!();
        #[cfg(feature = "engine")]
        print_efficiency_ledger(&tracker, crate::llm_proxy::read_gain_summary().as_ref());
        return Ok(());
    }

    // Default view (summary)
    if !daily && !weekly && !monthly && !all {
        // added: scope-aware styled header // changed: merged upstream styled + project scope
        let title = if project_scope.is_some() {
            "RTK Token Savings (Project Scope)"
        } else {
            "RTK Token Savings (Global Scope)"
        };
        println!("{}", styled(title, true));
        println!("{}", "═".repeat(60));
        // added: show project path when scoped
        if let Some(ref scope) = project_scope {
            println!("Scope: {}", shorten_path(scope));
        }
        println!();

        // added: KPI-style aligned output
        print_kpi("Total commands", summary.total_commands.to_string());
        print_kpi("Input tokens", format_tokens(summary.total_input));
        print_kpi("Output tokens", format_tokens(summary.total_output));
        print_kpi(
            "Tokens saved",
            format!(
                "{} ({:.1}%)",
                format_tokens(summary.total_saved),
                summary.avg_savings_pct
            ),
        );
        print_kpi(
            "Total exec time",
            format!(
                "{} (avg {})",
                format_duration(summary.total_time_ms),
                format_duration(summary.avg_time_ms)
            ),
        );
        print_efficiency_meter(summary.avg_savings_pct);
        if summary.oversized_excluded > 0 {
            print_kpi(
                "Excluded",
                format!(
                    "{} oversized outputs (input past the ~30k-char agent cap)",
                    summary.oversized_excluded
                ),
            );
        }
        for gap in crate::hooks::init::claude_hook_gaps() {
            print_kpi(
                "Hook needed",
                format!("{gap} — run `rtk init -g` so those pipeline rules fire"),
            );
        }
        // The CTA sits with the numbers it is about, not in a footer nobody
        // scrolls to.
        if share_cta_available() && std::io::stdout().is_terminal() {
            println!(
                "{} {}   {}",
                "▶ share these numbers:".green(),
                "rtk pipeline share".green().bold(),
                "(publish: rtk pipeline share publish)".dimmed()
            );
        }
        println!();

        if let Some(proxy) = &proxy_summary {
            println!(
                "{}",
                styled(
                    &format!("LLM Proxy ({} llm-proxy)", crate::llm_proxy::cli_name()),
                    true
                )
            );
            println!("{}", "─".repeat(60));
            print_kpi("Requests", proxy.total_requests.to_string());
            if proxy.failed_requests > 0 {
                print_kpi(
                    "Failed",
                    format!("{} (excluded from cost)", proxy.failed_requests),
                );
            }
            print_kpi("Tokens in", format_tokens(proxy.tokens_in.max(0) as usize));
            print_kpi(
                "Tokens out",
                format_tokens(proxy.tokens_out.max(0) as usize),
            );
            // Three levers, three units — each scored on what it touched.
            // There is no honest stored denominator for a trim percentage
            // (prefix-dominated tokens_in; history re-counted by the
            // composition sums), so this line stays absolute and the
            // per-mechanism ratios live in the pipeline ledger below.
            if proxy.trimmed_requests > 0 {
                print_kpi(
                    "Trimmed in flight",
                    format!(
                        "{} removed across the {} req it touched (per-mechanism ratios below)",
                        format_tokens(proxy.tokens_saved.max(0) as usize),
                        proxy.trimmed_requests,
                    ),
                );
            } else {
                print_kpi(
                    "Trimmed in flight",
                    format!(
                        "{} (transforms only)",
                        format_tokens(proxy.tokens_saved.max(0) as usize)
                    ),
                );
            }
            // Routing is the one proxy lever the trim figure cannot see: it
            // moves tokens to a cheaper biller instead of removing them. The
            // measured fact is tokens displaced; dollars appear only when the
            // operator configured prices (rtk ships none).
            if proxy.routed_requests > 0 {
                let mut line = format!(
                    "{} req · {} in ran on the task model, not the session model",
                    proxy.routed_requests,
                    format_tokens(proxy.routed_tokens_in.max(0) as usize),
                );
                if proxy.routed_cost_usd > 0.0 || proxy.routed_saved_usd > 0.0 {
                    let routed_pct = proxy.routed_saved_usd.max(0.0)
                        / (proxy.routed_cost_usd + proxy.routed_saved_usd).max(f64::MIN_POSITIVE)
                        * 100.0;
                    line.push_str(&format!(
                        " · ${:.4} spent · {} (-{:.0}%) vs the session model est.",
                        proxy.routed_cost_usd.max(0.0),
                        crate::llm_proxy::approx_usd(proxy.routed_saved_usd),
                        routed_pct
                    ));
                } else {
                    line.push_str(" (cheaper model, not fewer tokens)");
                }
                print_kpi("Routed", line);
            }
            // Per-method attribution, only for methods that actually ran --
            // a CCR-only daemon prints one line, not three with two zeroes.
            // Skipped entirely when there is only one, since the "Tokens
            // saved" line above already says the same number.
            let by_method = proxy.savings_by_method();
            if by_method.len() > 1 {
                for (method, saved) in by_method {
                    print_kpi(
                        &format!("  via {method}"),
                        format_tokens(saved.max(0) as usize),
                    );
                }
            }
            // Printed next to the savings because the two must be read
            // together: a compressed body that is not byte-stable across turns
            // invalidates the upstream prefix, and one such event costs more
            // than the compression saves.
            if proxy.retrieve_calls > 0 {
                print_kpi("Recovery calls", proxy.retrieve_calls.to_string());
            }
            if let Some(hit) = proxy.cache_hit_ratio() {
                print_kpi("Cache hit", format!("{:.1}%", hit * 100.0));
                // The cache is usually the largest saver in the whole session,
                // and the bare hit ratio hides that. Shown as a
                // percentage of raw input: user-agnostic, provider-reported
                // counts, published multipliers.
                if let Some(effect) = proxy.cache_effect_pct() {
                    print_kpi(
                        "Cache effect",
                        format!(
                            "input billed at ~{:.0}% of raw (-{:.0}%) est.",
                            100.0 - effect,
                            effect
                        ),
                    );
                }
                if proxy.cache_fractures > 0 {
                    print_kpi(
                        "  fractures",
                        format!("{} recent", proxy.cache_fractures)
                            .yellow()
                            .to_string(),
                    );
                }
            }
            println!();
        }

        #[cfg(feature = "engine")]
        print_efficiency_ledger(&tracker, proxy_summary.as_ref());

        // Lightweight RTK_DISABLED bypass check (best-effort, silent on failure)
        if let Some(warning) = check_rtk_disabled_bypass() {
            eprintln!("{}", warning.yellow());
            eprintln!();
        }

        let untrusted_filters = crate::hooks::trust::untrusted_active_filter_count();
        if untrusted_filters > 0 {
            eprintln!(
                "{}",
                format!(
                    "[rtk] {untrusted_filters} untrusted custom filter(s) not applied — run `rtk trust`"
                )
                .yellow()
            );
            eprintln!();
        }

        if let Some(status) = crate::core::pipeline_config::read_status() {
            let fell_back = status
                .get("fell_back")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let issues = status
                .get("diagnostics")
                .and_then(|d| d.as_array())
                .map(|d| d.len())
                .unwrap_or(0);
            if fell_back || issues > 0 {
                let detail = if fell_back {
                    "using the built-in default"
                } else {
                    "loaded with warnings"
                };
                eprintln!(
                    "{}",
                    format!(
                        "[rtk] pipeline.toml: {issues} issue(s), {detail} — run `rtk verify` or fix the file"
                    )
                    .yellow()
                );
                eprintln!();
            }
        }

        if !summary.by_command.is_empty() {
            // added: styled section header
            println!("{}", styled("By Command", true));

            // added: dynamic column widths for clean alignment
            let cmd_width = 24usize;
            let impact_width = 10usize;
            let count_width = summary
                .by_command
                .iter()
                .map(|(_, count, _, _, _)| count.to_string().len())
                .max()
                .unwrap_or(5)
                .max(5);
            let saved_width = summary
                .by_command
                .iter()
                .map(|(_, _, saved, _, _)| format_tokens(*saved).len())
                .max()
                .unwrap_or(5)
                .max(5);
            let time_width = summary
                .by_command
                .iter()
                .map(|(_, _, _, _, avg_time)| format_duration(*avg_time).len())
                .max()
                .unwrap_or(6)
                .max(6);

            let table_width = 3
                + 2
                + cmd_width
                + 2
                + count_width
                + 2
                + saved_width
                + 2
                + 6
                + 2
                + time_width
                + 2
                + impact_width;
            println!("{}", "─".repeat(table_width));
            println!(
                "{:>3}  {:<cmd_width$}  {:>count_width$}  {:>saved_width$}  {:>6}  {:>time_width$}  {:<impact_width$}",
                "#",
                "Command",
                "Count",
                "Saved",
                "Total%",
                "Time",
                "Impact",
                cmd_width = cmd_width,
                count_width = count_width,
                saved_width = saved_width,
                time_width = time_width,
                impact_width = impact_width
            );
            println!("{}", "─".repeat(table_width));

            let max_saved = summary
                .by_command
                .iter()
                .map(|(_, _, saved, _, _)| *saved)
                .max()
                .unwrap_or(1);

            for (idx, (cmd, count, saved, pct, avg_time)) in summary.by_command.iter().enumerate() {
                let row_idx = format!("{:>2}.", idx + 1);
                let cmd_cell = style_command_cell(&truncate_for_column(cmd, cmd_width)); // added: colored command
                let count_cell = format!("{:>count_width$}", count, count_width = count_width);
                let saved_cell = format!(
                    "{:>saved_width$}",
                    format_tokens(*saved),
                    saved_width = saved_width
                );
                let pct_plain = format!("{:>6}", format!("{pct:.1}%"));
                let pct_cell = colorize_pct_cell(*pct, &pct_plain); // added: color-coded percentage
                let time_cell = format!(
                    "{:>time_width$}",
                    format_duration(*avg_time),
                    time_width = time_width
                );
                let impact = mini_bar(*saved, max_saved, impact_width); // added: impact bar
                println!(
                    "{}  {}  {}  {}  {}  {}  {}",
                    row_idx, cmd_cell, count_cell, saved_cell, pct_cell, time_cell, impact
                );
            }
            println!("{}", "─".repeat(table_width));
            println!();
        }

        if graph && !summary.by_day.is_empty() {
            println!("{}", styled("Daily Savings (last 30 days)", true)); // added: styled header
            println!("──────────────────────────────────────────────────────────");
            print_ascii_graph(&summary.by_day);
            println!();
        }

        if history {
            let recent = tracker.get_recent_filtered(10, project_scope.as_deref())?; // changed: filtered
            if !recent.is_empty() {
                println!("{}", styled("Recent Commands", true)); // added: styled header
                println!("──────────────────────────────────────────────────────────");
                for rec in recent {
                    let time = rec.timestamp.with_timezone(&Local).format("%m-%d %H:%M");
                    let cmd_short = truncate(&rec.rtk_cmd, 25);
                    // added: tier indicators by savings level
                    let sign = if rec.savings_pct >= 70.0 {
                        "▲"
                    } else if rec.savings_pct >= 30.0 {
                        "■"
                    } else {
                        "•"
                    };
                    println!(
                        "{} {} {:<25} -{:.0}% ({})",
                        time,
                        sign,
                        cmd_short,
                        rec.savings_pct,
                        format_tokens(rec.saved_tokens)
                    );
                }
                println!();
            }
        }

        if quota {
            const ESTIMATED_PRO_MONTHLY: usize = 6_000_000;

            let (quota_tokens, tier_name) = match tier {
                "pro" => (ESTIMATED_PRO_MONTHLY, "Pro ($20/mo)"),
                "5x" => (ESTIMATED_PRO_MONTHLY * 5, "Max 5x ($100/mo)"),
                "20x" => (ESTIMATED_PRO_MONTHLY * 20, "Max 20x ($200/mo)"),
                _ => (ESTIMATED_PRO_MONTHLY, "Pro ($20/mo)"),
            };

            let quota_pct = (summary.total_saved as f64 / quota_tokens as f64) * 100.0;

            println!("{}", styled("Monthly Quota Analysis", true)); // added: styled header
            println!("──────────────────────────────────────────────────────────");
            print_kpi("Subscription tier", tier_name.to_string()); // added: KPI style
            print_kpi("Estimated monthly quota", format_tokens(quota_tokens));
            print_kpi(
                "Tokens saved (lifetime)",
                format_tokens(summary.total_saved),
            );
            print_kpi("Quota preserved", format!("{:.1}%", quota_pct));
            println!();
            println!("Note: Heuristic estimate based on ~44K tokens/5h (Pro baseline)");
            println!("      Actual limits use rolling 5-hour windows, not monthly caps.");
        }

        return Ok(());
    }

    // Time breakdown views
    if all || daily {
        print_daily_full(&tracker, project_scope.as_deref())?; // changed: pass project scope
    }

    if all || weekly {
        print_weekly(&tracker, project_scope.as_deref())?; // changed: pass project scope
    }

    if all || monthly {
        print_monthly(&tracker, project_scope.as_deref())?; // changed: pass project scope
    }

    Ok(())
}

#[cfg(feature = "engine")]
fn print_efficiency_ledger(tracker: &Tracker, proxy: Option<&crate::llm_proxy::ProxyGainSummary>) {
    const WINDOW_DAYS: u32 = 30;
    let sum = |kind: &str| -> (i64, i64, i64) {
        tracker
            .attribution_site("hook", kind, WINDOW_DAYS)
            .unwrap_or_default()
            .iter()
            .fold((0, 0, 0), |(c, i, o), r| {
                (c + r.calls, i + r.tokens_in, o + r.tokens_out)
            })
    };
    let (_, comp_in, comp_out) = sum("pipeline");
    let (_, _, llm_spent) = sum("llm-spend");
    let (recalls, _, _) = sum("recovery");
    let (shadow_calls, shadow_in, shadow_out) = {
        let rows = tracker
            .attribution_site("shadow", "pipeline", WINDOW_DAYS)
            .unwrap_or_default();
        rows.iter().fold((0, 0, 0), |(c, i, o), r| {
            (c + r.calls, i + r.tokens_in, o + r.tokens_out)
        })
    };
    let directive_tokens: usize = {
        let loaded = crate::core::pipeline_config::load();
        loaded
            .resolved
            .directives
            .iter()
            .filter(|d| d.def.enabled)
            .filter_map(crate::core::pipeline_config::directive_text)
            .map(|text| crate::core::tracking::estimate_tokens(&text))
            .sum()
    };
    let proxy_costs = proxy
        .filter(|p| p.cost_without_usd > 0.0)
        .map(|p| (p.cost_actual_usd, p.cost_without_usd));

    let has_hook_data = comp_in > 0 || llm_spent > 0 || recalls > 0;
    if !has_hook_data && proxy_costs.is_none() && shadow_calls == 0 {
        return;
    }

    println!(
        "{}",
        styled(
            &format!("Pipeline efficiency — last {WINDOW_DAYS} days · vs raw, same trajectory"),
            true
        )
    );
    println!("{}", "─".repeat(60));
    let mut notes: Vec<&str> = Vec::new();
    if comp_in > 0 {
        let pct = (comp_in - comp_out).max(0) as f64 / comp_in as f64 * 100.0;
        print_kpi(
            "Filter+compress",
            format!(
                "{} → {} tokens ({pct:.0}% removed)   est.",
                format_tokens(comp_in.max(0) as usize),
                format_tokens(comp_out.max(0) as usize),
            ),
        );
        notes.push("tokens est. ≈ bytes/4 — ratios reliable, absolutes approximate");
    }
    if llm_spent > 0 {
        print_kpi(
            "LLM-spend",
            format!(
                "−{} tokens (classifier/digest calls)   measured",
                format_tokens(llm_spent as usize)
            ),
        );
    }
    if recalls > 0 {
        print_kpi(
            "Recalled",
            format!("{recalls} compressed outputs pulled back in full"),
        );
    }
    if directive_tokens > 0 {
        print_kpi(
            "Directives",
            format!(
                "+{} tokens standing, every turn   est.",
                format_tokens(directive_tokens)
            ),
        );
    }
    match proxy_costs {
        Some((actual, without)) => {
            let pct = if without > 0.0 {
                (without - actual) / without * 100.0
            } else {
                0.0
            };
            print_kpi(
                "Bill (proxy)",
                format!("${actual:.2} actual vs ~${without:.2} without pipeline (−{pct:.0}%)"),
            );
            if let Some(p) = proxy
                && p.routed_requests > 0
            {
                let mut line = format!(
                    "{} requests ({} tokens in) to a cheaper model",
                    p.routed_requests,
                    format_tokens(p.routed_tokens_in.max(0) as usize),
                );
                if p.routed_saved_usd > 0.0 {
                    line.push_str(&format!(
                        " — {} saved est.",
                        crate::llm_proxy::approx_usd(p.routed_saved_usd)
                    ));
                }
                print_kpi("Routed", line);
            }
            notes.push(
                "'without' is priced from your models.toml prices; its cache behaviour is modeled",
            );
        }
        None => notes.push("no proxy cost data — size estimates only, not your bill"),
    }
    if shadow_calls > 0 {
        let pct = if shadow_in > 0 {
            (shadow_in - shadow_out).max(0) as f64 / shadow_in as f64 * 100.0
        } else {
            0.0
        };
        print_kpi(
            "Shadow rules",
            format!("would remove {pct:.0}% across {shadow_calls} outputs (changed nothing)"),
        );
    }
    for note in notes {
        println!("  ~ {note}");
    }
    println!();
}

// ── Display helpers (TTY-aware) ── // added: entire section

/// Format text with bold styling (TTY-aware). // added
fn styled(text: &str, strong: bool) -> String {
    if !std::io::stdout().is_terminal() {
        return text.to_string();
    }
    if strong {
        text.bold().green().to_string()
    } else {
        text.to_string()
    }
}

/// Print a key-value pair in KPI layout. // added
fn print_kpi(label: &str, value: String) {
    println!("{:<18} {}", format!("{label}:"), value);
}

/// Colorize percentage based on savings tier (TTY-aware). // added
fn colorize_pct_cell(pct: f64, padded: &str) -> String {
    if !std::io::stdout().is_terminal() {
        return padded.to_string();
    }
    if pct >= 70.0 {
        padded.green().bold().to_string()
    } else if pct >= 40.0 {
        padded.yellow().bold().to_string()
    } else {
        padded.red().bold().to_string()
    }
}

/// Truncate text to fit column width with ellipsis. // added
fn truncate_for_column(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let char_count = text.chars().count();
    if char_count <= width {
        return format!("{:<width$}", text, width = width);
    }
    if width <= 3 {
        return text.chars().take(width).collect();
    }
    let mut out: String = text.chars().take(width - 3).collect();
    out.push_str("...");
    out
}

/// Style command names with cyan+bold (TTY-aware). // added
fn style_command_cell(cmd: &str) -> String {
    if !std::io::stdout().is_terminal() {
        return cmd.to_string();
    }
    cmd.bright_cyan().bold().to_string()
}

/// Render a proportional bar chart segment (TTY-aware). // added
fn mini_bar(value: usize, max: usize, width: usize) -> String {
    if max == 0 || width == 0 {
        return String::new();
    }
    let filled = ((value as f64 / max as f64) * width as f64).round() as usize;
    let filled = filled.min(width);
    let mut bar = "█".repeat(filled);
    bar.push_str(&"░".repeat(width - filled));
    if std::io::stdout().is_terminal() {
        bar.cyan().to_string()
    } else {
        bar
    }
}

/// Print an efficiency meter with colored progress bar (TTY-aware). // added
fn print_efficiency_meter(pct: f64) {
    let width = 24usize;
    let filled = (((pct / 100.0) * width as f64).round() as usize).min(width);
    let meter = format!("{}{}", "█".repeat(filled), "░".repeat(width - filled));
    if std::io::stdout().is_terminal() {
        let pct_str = format!("{pct:.1}%");
        let colored_pct = if pct >= 70.0 {
            pct_str.green().bold().to_string()
        } else if pct >= 40.0 {
            pct_str.yellow().bold().to_string()
        } else {
            pct_str.red().bold().to_string()
        };
        println!("Efficiency meter: {} {}", meter.green(), colored_pct);
    } else {
        println!("Efficiency meter: {} {:.1}%", meter, pct);
    }
}

const VERDICT_MIN_ELISIONS: i64 = 5;
const VERDICT_WIDTH: usize = 14;
const ISSUES_URL: &str = "https://github.com/rtk-ai/rtk/issues";

/// Bands match `colorize_recall_rate` so the colour and the wording never disagree.
fn recall_verdict(elisions: i64, recalls: i64) -> &'static str {
    if elisions < VERDICT_MIN_ELISIONS {
        return "-";
    }
    match recalls * 100 / elisions {
        p if p >= 30 => "too aggressive",
        p if p >= 10 => "watch",
        _ => "ok",
    }
}

fn colorize_recall_rate(pct: i64, padded: &str) -> String {
    if !std::io::stdout().is_terminal() {
        return padded.to_string();
    }
    if pct >= 30 {
        padded.red().bold().to_string()
    } else if pct >= 10 {
        padded.yellow().bold().to_string()
    } else {
        padded.green().bold().to_string()
    }
}

fn show_recall_stats() -> Result<()> {
    use crate::core::retriever::RecoveryMode;

    let mode = crate::core::config::Config::load()
        .unwrap_or_default()
        .retriever
        .mode;
    let mode_label = match mode {
        RecoveryMode::Sqlite => "sqlite",
        RecoveryMode::Tee => "tee",
        RecoveryMode::Disabled => "disabled",
    };
    let stats = crate::core::retriever::stats_snapshot()?;

    println!("{}", styled("RTK Recall Efficiency", true));
    println!("{}", "═".repeat(60));
    println!("Mode: {mode_label}");
    println!();

    if stats.is_empty() {
        println!("No recall activity recorded yet.");
        println!("Stats appear once filters elide output (failures, trimmed lists).");
        return Ok(());
    }

    let slug_width = stats
        .iter()
        .map(|s| s.slug.chars().count())
        .max()
        .unwrap_or(6)
        .clamp(6, 24);
    let table_width = slug_width + 2 + 8 + 2 + 8 + 2 + 6 + 2 + VERDICT_WIDTH;

    let render = |title: &str, mode_key: &str, prefix: &str| {
        let rows: Vec<_> = stats.iter().filter(|s| s.mode == mode_key).collect();
        if rows.is_empty() {
            return;
        }
        println!("{}", styled(title, true));
        println!("{}", "─".repeat(table_width));
        println!(
            "{:<slug_width$}  {:>8}  {:>8}  {:>6}  Verdict",
            "Filter", "Elisions", "Recalled", "Rate"
        );
        println!("{}", "─".repeat(table_width));
        for s in rows {
            let (pct, rate) = if s.elisions > 0 {
                let pct = s.recalls * 100 / s.elisions;
                (pct, format!("{prefix}{pct}%"))
            } else {
                (0, "-".to_string())
            };
            println!(
                "{}  {:>8}  {:>8}  {}  {}",
                truncate_for_column(&s.slug, slug_width),
                s.elisions,
                s.recalls,
                colorize_recall_rate(pct, &format!("{rate:>6}")),
                colorize_recall_rate(pct, recall_verdict(s.elisions, s.recalls))
            );
        }
        println!();
    };

    render("Sqlite (exact — reads go through rtk recall)", "sqlite", "");
    render("Tee (approximate — bash-observed reads only)", "tee", "≥");

    println!("\"too aggressive\" means the filter hides output the agent comes back for.");
    println!("Each recall costs an extra API turn plus the full output, so it cancels");
    println!("the savings on that command instead of adding to them.");
    println!(
        "\"-\" means fewer than {VERDICT_MIN_ELISIONS} elisions so far: not enough data to judge."
    );
    println!();
    println!("Raise that filter's cap, or report it: {ISSUES_URL}");
    Ok(())
}

/// Resolve project scope from --project flag. // added
fn resolve_project_scope(project: bool) -> Result<Option<String>> {
    if !project {
        return Ok(None);
    }
    let cwd = user_dirs::current_dir().context("Failed to resolve current working directory")?;
    let canonical = cwd.canonicalize().unwrap_or(cwd);
    Ok(Some(canonical.to_string_lossy().to_string()))
}

/// Shorten long absolute paths for display. // added
fn shorten_path(path: &str) -> String {
    let path_buf = PathBuf::from(path);
    let comps: Vec<String> = path_buf
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    if comps.len() <= 4 {
        return path.to_string();
    }
    let root = comps[0].as_str();
    if root == "/" || root.is_empty() {
        format!("/.../{}/{}", comps[comps.len() - 2], comps[comps.len() - 1])
    } else {
        format!(
            "{}/.../{}/{}",
            root,
            comps[comps.len() - 2],
            comps[comps.len() - 1]
        )
    }
}

fn print_ascii_graph(data: &[(String, usize)]) {
    if data.is_empty() {
        return;
    }

    let max_val = data.iter().map(|(_, v)| *v).max().unwrap_or(1);
    let width = 40;

    for (date, value) in data {
        let date_short = if date.len() >= 10 { &date[5..10] } else { date };

        let bar_len = if max_val > 0 {
            ((*value as f64 / max_val as f64) * width as f64) as usize
        } else {
            0
        };

        let bar: String = "█".repeat(bar_len);
        let spaces: String = " ".repeat(width - bar_len);

        println!(
            "{} │{}{} {}",
            date_short,
            bar,
            spaces,
            format_tokens(*value)
        );
    }
}

fn print_daily_full(tracker: &Tracker, project_scope: Option<&str>) -> Result<()> {
    // changed: add project scope
    let days = tracker.get_all_days_filtered(project_scope)?; // changed: use filtered variant
    print_period_table(&days);
    Ok(())
}

fn print_weekly(tracker: &Tracker, project_scope: Option<&str>) -> Result<()> {
    // changed: add project scope
    let weeks = tracker.get_by_week_filtered(project_scope)?; // changed: use filtered variant
    print_period_table(&weeks);
    Ok(())
}

fn print_monthly(tracker: &Tracker, project_scope: Option<&str>) -> Result<()> {
    // changed: add project scope
    let months = tracker.get_by_month_filtered(project_scope)?; // changed: use filtered variant
    print_period_table(&months);
    Ok(())
}

#[derive(Serialize)]
struct ExportData {
    summary: ExportSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    daily: Option<Vec<DayStats>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    weekly: Option<Vec<WeekStats>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    monthly: Option<Vec<MonthStats>>,
}

#[derive(Serialize)]
struct ExportSummary {
    total_commands: usize,
    total_input: usize,
    total_output: usize,
    total_saved: usize,
    avg_savings_pct: f64,
    total_time_ms: u64,
    avg_time_ms: u64,
    oversized_excluded: usize,
}

fn export_json(
    tracker: &Tracker,
    daily: bool,
    weekly: bool,
    monthly: bool,
    all: bool,
    project_scope: Option<&str>, // added: project scope
) -> Result<()> {
    let summary = tracker
        .get_summary_filtered(project_scope) // changed: use filtered variant
        .context("Failed to load token savings summary from database")?;

    let export = ExportData {
        summary: ExportSummary {
            total_commands: summary.total_commands,
            total_input: summary.total_input,
            total_output: summary.total_output,
            total_saved: summary.total_saved,
            avg_savings_pct: summary.avg_savings_pct,
            total_time_ms: summary.total_time_ms,
            avg_time_ms: summary.avg_time_ms,
            oversized_excluded: summary.oversized_excluded,
        },
        daily: if all || daily {
            Some(tracker.get_all_days_filtered(project_scope)?) // changed: use filtered
        } else {
            None
        },
        weekly: if all || weekly {
            Some(tracker.get_by_week_filtered(project_scope)?) // changed: use filtered
        } else {
            None
        },
        monthly: if all || monthly {
            Some(tracker.get_by_month_filtered(project_scope)?) // changed: use filtered
        } else {
            None
        },
    };

    let json = serde_json::to_string_pretty(&export)?;
    println!("{}", json);

    Ok(())
}

fn export_csv(
    tracker: &Tracker,
    daily: bool,
    weekly: bool,
    monthly: bool,
    all: bool,
    project_scope: Option<&str>, // added: project scope
) -> Result<()> {
    if all || daily {
        let days = tracker.get_all_days_filtered(project_scope)?; // changed: use filtered
        println!("# Daily Data");
        println!(
            "date,commands,input_tokens,output_tokens,saved_tokens,savings_pct,total_time_ms,avg_time_ms"
        );
        for day in days {
            println!(
                "{},{},{},{},{},{:.2},{},{}",
                day.date,
                day.commands,
                day.input_tokens,
                day.output_tokens,
                day.saved_tokens,
                day.savings_pct,
                day.total_time_ms,
                day.avg_time_ms
            );
        }
        println!();
    }

    if all || weekly {
        let weeks = tracker.get_by_week_filtered(project_scope)?; // changed: use filtered
        println!("# Weekly Data");
        println!(
            "week_start,week_end,commands,input_tokens,output_tokens,saved_tokens,savings_pct,total_time_ms,avg_time_ms"
        );
        for week in weeks {
            println!(
                "{},{},{},{},{},{},{:.2},{},{}",
                week.week_start,
                week.week_end,
                week.commands,
                week.input_tokens,
                week.output_tokens,
                week.saved_tokens,
                week.savings_pct,
                week.total_time_ms,
                week.avg_time_ms
            );
        }
        println!();
    }

    if all || monthly {
        let months = tracker.get_by_month_filtered(project_scope)?; // changed: use filtered
        println!("# Monthly Data");
        println!(
            "month,commands,input_tokens,output_tokens,saved_tokens,savings_pct,total_time_ms,avg_time_ms"
        );
        for month in months {
            println!(
                "{},{},{},{},{},{:.2},{},{}",
                month.month,
                month.commands,
                month.input_tokens,
                month.output_tokens,
                month.saved_tokens,
                month.savings_pct,
                month.total_time_ms,
                month.avg_time_ms
            );
        }
    }

    Ok(())
}

/// Lightweight scan of recent Claude Code sessions for RTK_DISABLED= overuse.
/// Returns a warning string if bypass rate exceeds 10%, None otherwise.
/// Silently returns None on any error (missing dirs, permission issues, etc.).
fn check_rtk_disabled_bypass() -> Option<String> {
    use crate::discover::provider::{ClaudeProvider, SessionProvider};
    use crate::discover::{CoverageContext, line_has_counted_bypass};

    let provider = ClaudeProvider;

    // Quick scan: last 7 days only
    let sessions = provider.discover_sessions(None, Some(7)).ok()?;

    // Early bail if no sessions or too many (avoid slow scan)
    if sessions.is_empty() || sessions.len() > 200 {
        return None;
    }

    let mut total_bash: usize = 0;
    let mut bypassed: usize = 0;
    // Same per-segment gate as `rtk discover` (rtk-ai/rtk#4277). Loading it reads
    // config and permission files, so defer that until a line actually carries the
    // prefix — lines without it cost only a substring check.
    let mut coverage_ctx: Option<CoverageContext> = None;

    for session_path in &sessions {
        let extracted = match provider.extract_commands(session_path) {
            Ok(cmds) => cmds,
            Err(_) => continue,
        };

        for ext_cmd in &extracted {
            total_bash += 1;
            if ext_cmd.command.contains("RTK_DISABLED=")
                && line_has_counted_bypass(
                    &ext_cmd.command,
                    coverage_ctx.get_or_insert_with(CoverageContext::load_without_hook_log),
                )
            {
                bypassed += 1;
            }
        }
    }

    if total_bash == 0 {
        return None;
    }

    let pct = (bypassed as f64 / total_bash as f64) * 100.0;
    if pct > 10.0 {
        Some(format!(
            "[warn] {} commands ({:.0}%) used RTK_DISABLED=1 unnecessarily — run `rtk discover` for details",
            bypassed, pct
        ))
    } else {
        None
    }
}

fn show_failures(tracker: &Tracker) -> Result<()> {
    let summary = tracker
        .get_parse_failure_summary()
        .context("Failed to load parse failure data")?;

    if summary.total == 0 {
        println!("No parse failures recorded.");
        println!("This means all commands parsed successfully (or fallback hasn't triggered yet).");
        return Ok(());
    }

    println!("{}", styled("RTK Parse Failures", true));
    println!("{}", "═".repeat(60));
    println!();

    print_kpi("Total failures", summary.total.to_string());
    print_kpi("Recovery rate", format!("{:.1}%", summary.recovery_rate));
    println!();

    if !summary.top_commands.is_empty() {
        println!("{}", styled("Top Commands (by frequency)", true));
        println!("{}", "─".repeat(60));
        for (cmd, count) in &summary.top_commands {
            let cmd_display = truncate(cmd, 50);
            println!("  {:>4}x  {}", count, cmd_display);
        }
        println!();
    }

    if !summary.recent.is_empty() {
        println!("{}", styled("Recent Failures (last 10)", true));
        println!("{}", "─".repeat(60));
        for rec in &summary.recent {
            // ISSUE #2787: floor to the previous char boundary so the prefix
            // never exceeds 16 bytes and never lands mid-character
            let ts_short = &rec.timestamp[..rec.timestamp.floor_char_boundary(16)];
            let status = if rec.fallback_succeeded { "ok" } else { "FAIL" };
            let cmd_display = truncate(&rec.raw_command, 40);
            println!("  {} [{}] {}", ts_short, status, cmd_display);
        }
        println!();
    }

    Ok(())
}

/// Prompt the user to confirm a destructive reset operation.
/// Defaults to No in non-interactive (piped) environments.
fn confirm_reset() -> Result<bool> {
    use std::io::{self, BufRead, IsTerminal, Write};

    eprint!(
        "This will permanently delete all tracking data and recall counters (stored outputs are kept). Continue? [y/N] "
    );
    io::stderr().flush().ok();

    if !io::stdin().is_terminal() {
        eprintln!("(non-interactive mode, defaulting to N)");
        return Ok(false);
    }

    let stdin = io::stdin();
    let mut line = String::new();
    stdin
        .lock()
        .read_line(&mut line)
        .context("Failed to read confirmation")?;

    Ok(matches!(line.trim().to_lowercase().as_str(), "y" | "yes"))
}

enum WatchDispatch {
    Tui,
    TextFallback,
    Text,
}

fn watch_dispatch(watch: bool, auto_watch: bool, stdout_is_tty: bool) -> WatchDispatch {
    if watch && !stdout_is_tty {
        WatchDispatch::TextFallback
    } else if watch || auto_watch {
        WatchDispatch::Tui
    } else {
        WatchDispatch::Text
    }
}

pub(crate) fn share_cta_available() -> bool {
    cfg!(feature = "engine")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_cta_only_when_pipeline_command_exists() {
        use clap::CommandFactory;
        let has_pipeline = crate::Cli::command().find_subcommand("pipeline").is_some();
        assert_eq!(share_cta_available(), has_pipeline);
    }

    #[cfg(feature = "engine")]
    #[test]
    fn hook_slices_split_per_subject_and_ignore_other_pipelines() {
        let tracker = Tracker::new_in_memory().expect("tracker");
        let key = |p: &str, s: &str| crate::core::tracking::pipeline_slice_key(p, s);
        tracker
            .record_attribution(
                "hook",
                "pipeline-slice",
                &key("my-pipe", "shell"),
                1000,
                400,
            )
            .expect("record");
        tracker
            .record_attribution("hook", "pipeline-slice", &key("my-pipe", "shell"), 500, 200)
            .expect("record");
        tracker
            .record_attribution("hook", "pipeline-slice", &key("my-pipe", "read"), 300, 250)
            .expect("record");
        tracker
            .record_attribution("hook", "pipeline-slice", &key("other", "shell"), 900, 100)
            .expect("record");

        let slices = hook_slices_for(&tracker, "my-pipe", 7).expect("slices");
        assert_eq!(slices.len(), 2);
        assert_eq!(slices[0], ("shell output (bash)".to_string(), 2, 1500, 900));
        assert_eq!(slices[1], ("file reads".to_string(), 1, 300, 50));
    }

    #[cfg(feature = "engine")]
    #[test]
    fn slice_key_survives_a_round_trip() {
        let key = crate::core::tracking::pipeline_slice_key("shared-ccr", "web");
        assert_eq!(
            crate::core::tracking::split_pipeline_slice(&key),
            Some(("shared-ccr", "web"))
        );
        assert_eq!(
            crate::core::tracking::split_pipeline_slice("plain-name"),
            None
        );
    }

    #[test]
    fn watch_without_tty_falls_back_to_text() {
        assert!(matches!(
            watch_dispatch(true, false, false),
            WatchDispatch::TextFallback
        ));
        assert!(matches!(
            watch_dispatch(true, true, false),
            WatchDispatch::TextFallback
        ));
    }

    #[test]
    fn watch_with_tty_launches_tui() {
        assert!(matches!(
            watch_dispatch(true, false, true),
            WatchDispatch::Tui
        ));
        assert!(matches!(
            watch_dispatch(false, true, true),
            WatchDispatch::Tui
        ));
    }

    #[test]
    fn no_watch_request_stays_on_text_view() {
        assert!(matches!(
            watch_dispatch(false, false, true),
            WatchDispatch::Text
        ));
        assert!(matches!(
            watch_dispatch(false, false, false),
            WatchDispatch::Text
        ));
    }

    #[test]
    fn test_recall_verdict_needs_a_sample_before_judging() {
        assert_eq!(recall_verdict(4, 4), "-");
        assert_eq!(recall_verdict(0, 0), "-");
        assert_eq!(recall_verdict(5, 5), "too aggressive");
    }

    #[test]
    fn test_recall_verdict_bands_match_the_colour_bands() {
        assert_eq!(recall_verdict(100, 29), "watch");
        assert_eq!(recall_verdict(100, 30), "too aggressive");
        assert_eq!(recall_verdict(100, 9), "ok");
        assert_eq!(recall_verdict(100, 10), "watch");
        assert_eq!(recall_verdict(100, 0), "ok");
    }

    #[test]
    fn test_recall_verdict_fits_its_column() {
        for v in ["-", "ok", "watch", "too aggressive"] {
            assert!(v.len() <= VERDICT_WIDTH, "{v} overflows the column");
        }
    }
}
