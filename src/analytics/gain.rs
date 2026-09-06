//! Shows users how many tokens RTK has saved them over time.

use crate::core::display_helpers::{format_duration, print_period_table};
use crate::core::tracking::{DayStats, MonthStats, Tracker, WeekStats};
use crate::core::utils::{format_tokens, truncate};
use crate::hooks::hook_check;
use anyhow::{bail, Context, Result};
use chrono::Local;
use colored::Colorize;
use serde::Serialize;
use std::fs;
use std::io::{IsTerminal, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;

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
    web: bool,
    serve: bool,
    open: bool,
    port: u16,
    web_output: Option<&Path>,
    failures: bool,
    reset: bool,
    yes: bool,
    _verbose: u8,
) -> Result<()> {
    let tracker = Tracker::new().context("Failed to initialize tracking database")?;
    let project_scope = resolve_project_scope(project)?; // added: resolve project path

    if open && !web && !serve {
        bail!("--open requires --web or --serve");
    }

    if reset {
        if !yes && !confirm_reset()? {
            println!("Aborted.");
            return Ok(());
        }
        tracker
            .reset_all()
            .context("Failed to reset token savings")?;
        println!("{}", styled("Token savings stats reset to zero.", true));
        return Ok(());
    }

    if failures {
        return show_failures(&tracker);
    }

    if serve {
        return serve_web_dashboard(project_scope, open, port);
    }

    if web {
        return export_web_dashboard(&tracker, project_scope.as_deref(), open, web_output);
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

    if summary.total_commands == 0 {
        println!("No tracking data yet.");
        println!("Run some rtk commands to start tracking savings.");
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
        println!();

        // Warn about hook issues that silently kill savings (stderr, not stdout)
        match hook_check::status() {
            hook_check::HookStatus::Missing => {
                eprintln!(
                    "{}",
                    "[warn] No hook installed — run `rtk init -g` for automatic token savings"
                        .yellow()
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
            hook_check::HookStatus::Ok => {}
        }

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
                "#", "Command", "Count", "Saved", "Avg%", "Time", "Impact",
                cmd_width = cmd_width, count_width = count_width,
                saved_width = saved_width, time_width = time_width,
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

/// Resolve project scope from --project flag. // added
fn resolve_project_scope(project: bool) -> Result<Option<String>> {
    if !project {
        return Ok(None);
    }
    let cwd = std::env::current_dir().context("Failed to resolve current working directory")?;
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
}

#[derive(Serialize)]
struct WebDashboardData {
    generated_at: String,
    scope: WebScope,
    summary: ExportSummary,
    by_command: Vec<WebCommandStats>,
    daily: Vec<DayStats>,
    weekly: Vec<WeekStats>,
    monthly: Vec<MonthStats>,
}

#[derive(Serialize)]
struct WebScope {
    kind: String,
    path: Option<String>,
}

#[derive(Serialize)]
struct WebCommandStats {
    command: String,
    count: usize,
    saved_tokens: usize,
    avg_savings_pct: f64,
    avg_time_ms: u64,
}

fn export_web_dashboard(
    tracker: &Tracker,
    project_scope: Option<&str>,
    open: bool,
    output: Option<&Path>,
) -> Result<()> {
    let data = build_web_dashboard_data(tracker, project_scope)?;
    let html = render_web_dashboard(&data, false)?;
    let path = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::env::temp_dir().join("rtk-gain-dashboard.html"));

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create dashboard output directory {}",
                parent.display()
            )
        })?;
    }

    fs::write(&path, html)
        .with_context(|| format!("Failed to write web dashboard to {}", path.display()))?;

    println!("Web dashboard written to {}", path.display());

    if open {
        open_dashboard(&path)?;
    }

    Ok(())
}

fn build_web_dashboard_data(
    tracker: &Tracker,
    project_scope: Option<&str>,
) -> Result<WebDashboardData> {
    let summary = tracker
        .get_summary_filtered(project_scope)
        .context("Failed to load token savings summary from database")?;

    Ok(WebDashboardData {
        generated_at: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        scope: WebScope {
            kind: if project_scope.is_some() {
                "Project".to_string()
            } else {
                "Global".to_string()
            },
            path: project_scope.map(|scope| scope.to_string()),
        },
        summary: ExportSummary {
            total_commands: summary.total_commands,
            total_input: summary.total_input,
            total_output: summary.total_output,
            total_saved: summary.total_saved,
            avg_savings_pct: summary.avg_savings_pct,
            total_time_ms: summary.total_time_ms,
            avg_time_ms: summary.avg_time_ms,
        },
        by_command: summary
            .by_command
            .into_iter()
            .map(
                |(command, count, saved_tokens, avg_savings_pct, avg_time_ms)| WebCommandStats {
                    command,
                    count,
                    saved_tokens,
                    avg_savings_pct,
                    avg_time_ms,
                },
            )
            .collect(),
        daily: tracker.get_all_days_filtered(project_scope)?,
        weekly: tracker.get_by_week_filtered(project_scope)?,
        monthly: tracker.get_by_month_filtered(project_scope)?,
    })
}

fn render_web_dashboard(data: &WebDashboardData, live: bool) -> Result<String> {
    let json = serde_json::to_string(data)?
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    let live_endpoint = if live { "\"/api/gain\"" } else { "null" };
    Ok(format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>RTK Gain Dashboard</title>
<style>
:root {{
  color-scheme: light;
  --bg: #f7f8fb;
  --panel: #ffffff;
  --ink: #18202f;
  --muted: #687386;
  --line: #dce1ea;
  --accent: #0f8b8d;
  --accent-2: #d95f3d;
  --good: #278452;
  --shadow: 0 14px 40px rgba(24, 32, 47, 0.08);
}}
* {{ box-sizing: border-box; }}
body {{
  margin: 0;
  min-height: 100vh;
  background: var(--bg);
  color: var(--ink);
  font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
}}
main {{
  width: min(1180px, calc(100% - 32px));
  margin: 0 auto;
  padding: 32px 0 44px;
}}
header {{
  display: flex;
  justify-content: space-between;
  gap: 20px;
  align-items: flex-start;
  margin-bottom: 24px;
}}
h1 {{
  margin: 0 0 8px;
  font-size: clamp(28px, 4vw, 46px);
  line-height: 1.02;
  letter-spacing: 0;
}}
h2 {{
  margin: 0 0 16px;
  font-size: 18px;
  letter-spacing: 0;
}}
p {{ margin: 0; color: var(--muted); }}
.meta {{ text-align: right; font-size: 14px; line-height: 1.5; }}
.grid {{
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 12px;
  margin-bottom: 18px;
}}
.stat, .panel {{
  background: var(--panel);
  border: 1px solid var(--line);
  border-radius: 8px;
  box-shadow: var(--shadow);
}}
.stat {{ padding: 16px; min-width: 0; }}
.label {{
  display: block;
  color: var(--muted);
  font-size: 12px;
  text-transform: uppercase;
  letter-spacing: 0.08em;
  margin-bottom: 8px;
}}
.value {{
  font-size: clamp(22px, 3vw, 32px);
  font-weight: 760;
  overflow-wrap: anywhere;
}}
.panel {{
  padding: 20px;
  margin-top: 18px;
}}
.chart-wrap {{ width: 100%; overflow-x: auto; }}
svg {{ display: block; width: 100%; min-width: 620px; height: 340px; }}
.axis {{ stroke: var(--line); stroke-width: 1; }}
.bar {{ fill: var(--accent); }}
.bar:hover {{ fill: var(--accent-2); }}
.tick {{ fill: var(--muted); font-size: 12px; }}
.table {{ width: 100%; border-collapse: collapse; }}
th, td {{
  padding: 11px 8px;
  border-bottom: 1px solid var(--line);
  text-align: right;
  font-size: 14px;
}}
th:first-child, td:first-child {{ text-align: left; }}
th {{ color: var(--muted); font-weight: 650; }}
.empty {{
  min-height: 260px;
  display: grid;
  place-items: center;
  text-align: center;
  color: var(--muted);
  border: 1px dashed var(--line);
  border-radius: 8px;
}}
@media (max-width: 780px) {{
  main {{ width: min(100% - 20px, 1180px); padding-top: 20px; }}
  header {{ display: block; }}
  .meta {{ text-align: left; margin-top: 12px; }}
  .grid {{ grid-template-columns: repeat(2, minmax(0, 1fr)); }}
  .panel {{ padding: 14px; }}
}}
@media (max-width: 480px) {{
  .grid {{ grid-template-columns: 1fr; }}
}}
</style>
</head>
<body>
<main>
  <header>
    <div>
      <h1>RTK Gain</h1>
      <p id="subtitle"></p>
    </div>
    <p class="meta" id="meta"></p>
  </header>
  <section class="grid" id="stats"></section>
  <section class="panel">
    <h2>Daily Token Savings</h2>
    <div class="chart-wrap" id="dailyChart"></div>
  </section>
  <section class="panel">
    <h2>Top Commands</h2>
    <div id="commandTable"></div>
  </section>
</main>
<script>
let data = {json};
const liveEndpoint = {live_endpoint};

const fmt = new Intl.NumberFormat();
const short = value => {{
  if (value >= 1_000_000) return `${{(value / 1_000_000).toFixed(1)}}M`;
  if (value >= 1_000) return `${{(value / 1_000).toFixed(1)}}K`;
  return fmt.format(value);
}};
const pct = value => `${{Number(value || 0).toFixed(1)}}%`;
const ms = value => value >= 1000 ? `${{(value / 1000).toFixed(1)}}s` : `${{fmt.format(value || 0)}}ms`;
const esc = value => String(value).replace(/[&<>"']/g, char => ({{
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
  "'": "&#39;",
}}[char]));

function renderSummary() {{
  document.getElementById("subtitle").textContent =
    data.scope.path ? `${{data.scope.kind}} scope: ${{data.scope.path}}` : "Global token savings analytics";
  document.getElementById("meta").innerHTML =
    `Updated ${{data.generated_at}}<br>${{data.daily.length}} daily points`;

  const stats = [
    ["Commands", fmt.format(data.summary.total_commands)],
    ["Input Tokens", short(data.summary.total_input)],
    ["Output Tokens", short(data.summary.total_output)],
    ["Tokens Saved", `${{short(data.summary.total_saved)}} (${{
      pct(data.summary.avg_savings_pct)
    }})`],
  ];
  document.getElementById("stats").innerHTML = stats.map(([label, value]) =>
    `<article class="stat"><span class="label">${{label}}</span><div class="value">${{value}}</div></article>`
  ).join("");
}}

function renderDailyChart() {{
  const host = document.getElementById("dailyChart");
  const rows = data.daily;
  if (!rows.length) {{
    host.innerHTML = '<div class="empty">No tracking data yet. Run RTK commands and refresh this dashboard.</div>';
    return;
  }}

  const width = Math.max(620, rows.length * 34 + 90);
  const height = 340;
  const left = 54;
  const right = 20;
  const top = 20;
  const bottom = 58;
  const innerW = width - left - right;
  const innerH = height - top - bottom;
  const max = Math.max(...rows.map(row => row.saved_tokens), 1);
  const gap = 8;
  const barW = Math.max(8, (innerW - gap * (rows.length - 1)) / rows.length);
  const bars = rows.map((row, index) => {{
    const barH = Math.max(2, (row.saved_tokens / max) * innerH);
    const x = left + index * (barW + gap);
    const y = top + innerH - barH;
    const label = row.date.length >= 10 ? row.date.slice(5) : row.date;
    return `
      <rect class="bar" x="${{x}}" y="${{y}}" width="${{barW}}" height="${{barH}}" rx="3">
        <title>${{row.date}}: ${{fmt.format(row.saved_tokens)}} tokens saved</title>
      </rect>
      <text class="tick" x="${{x + barW / 2}}" y="${{height - 24}}" text-anchor="middle" transform="rotate(-35 ${{x + barW / 2}} ${{height - 24}})">${{label}}</text>
    `;
  }}).join("");
  const yTicks = [0, 0.5, 1].map(part => {{
    const y = top + innerH - innerH * part;
    const value = Math.round(max * part);
    return `<line class="axis" x1="${{left}}" y1="${{y}}" x2="${{width - right}}" y2="${{y}}"></line>
      <text class="tick" x="${{left - 8}}" y="${{y + 4}}" text-anchor="end">${{short(value)}}</text>`;
  }}).join("");
  host.innerHTML = `<svg viewBox="0 0 ${{width}} ${{height}}" role="img" aria-label="Daily token savings chart">
    ${{yTicks}}
    <line class="axis" x1="${{left}}" y1="${{top + innerH}}" x2="${{width - right}}" y2="${{top + innerH}}"></line>
    ${{bars}}
  </svg>`;
}}

function renderCommandTable() {{
  const host = document.getElementById("commandTable");
  if (!data.by_command.length) {{
    host.innerHTML = '<div class="empty">No command breakdown available yet.</div>';
    return;
  }}
  host.innerHTML = `<table class="table">
    <thead><tr><th>Command</th><th>Count</th><th>Saved</th><th>Avg Save</th><th>Avg Time</th></tr></thead>
    <tbody>${{data.by_command.map(row => `<tr>
      <td>${{esc(row.command)}}</td>
      <td>${{fmt.format(row.count)}}</td>
      <td>${{short(row.saved_tokens)}}</td>
      <td>${{pct(row.avg_savings_pct)}}</td>
      <td>${{ms(row.avg_time_ms)}}</td>
    </tr>`).join("")}}</tbody>
  </table>`;
}}

function renderDashboard() {{
  renderSummary();
  renderDailyChart();
  renderCommandTable();
}}

async function refreshDashboard() {{
  if (!liveEndpoint) return;
  try {{
    const response = await fetch(liveEndpoint, {{ cache: "no-store" }});
    if (!response.ok) return;
    data = await response.json();
    renderDashboard();
  }} catch (_error) {{
  }}
}}

renderDashboard();
if (liveEndpoint) {{
  setInterval(refreshDashboard, 2000);
}}
</script>
</body>
</html>
"#,
        json = json,
        live_endpoint = live_endpoint
    ))
}

fn serve_web_dashboard(project_scope: Option<String>, open: bool, port: u16) -> Result<()> {
    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr)
        .with_context(|| format!("Failed to start dashboard server on http://{addr}"))?;
    let url = format!("http://{addr}/");

    println!("RTK Gain live dashboard serving at {url}");
    println!("Press Ctrl+C to stop.");

    if open {
        open_url(&url)?;
    }

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(err) = handle_dashboard_request(stream, project_scope.as_deref()) {
                    eprintln!("dashboard request failed: {err}");
                }
            }
            Err(err) => eprintln!("dashboard connection failed: {err}"),
        }
    }

    Ok(())
}

fn handle_dashboard_request(mut stream: TcpStream, project_scope: Option<&str>) -> Result<()> {
    let mut buffer = [0_u8; 2048];
    let read = stream
        .read(&mut buffer)
        .context("Failed to read dashboard request")?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");

    match path {
        "/" | "/index.html" => {
            let tracker = Tracker::new().context("Failed to initialize tracking database")?;
            let data = build_web_dashboard_data(&tracker, project_scope)?;
            let html = render_web_dashboard(&data, true)?;
            write_http_response(&mut stream, "200 OK", "text/html; charset=utf-8", &html)?;
        }
        "/api/gain" => {
            let tracker = Tracker::new().context("Failed to initialize tracking database")?;
            let data = build_web_dashboard_data(&tracker, project_scope)?;
            let json = serde_json::to_string(&data)?;
            write_http_response(&mut stream, "200 OK", "application/json", &json)?;
        }
        _ => {
            write_http_response(
                &mut stream,
                "404 Not Found",
                "text/plain; charset=utf-8",
                "not found",
            )?;
        }
    }

    Ok(())
}

fn write_http_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.as_bytes().len()
    );
    stream
        .write_all(response.as_bytes())
        .context("Failed to write dashboard response")?;
    Ok(())
}

fn open_dashboard(path: &Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "start", ""]);
        cmd.arg(path);
        cmd
    };

    #[cfg(target_os = "macos")]
    let mut command = {
        let mut cmd = Command::new("open");
        cmd.arg(path);
        cmd
    };

    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut cmd = Command::new("xdg-open");
        cmd.arg(path);
        cmd
    };

    command
        .spawn()
        .with_context(|| format!("Failed to open dashboard {}", path.display()))?;

    Ok(())
}

fn open_url(url: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "start", "", url]);
        cmd
    };

    #[cfg(target_os = "macos")]
    let mut command = {
        let mut cmd = Command::new("open");
        cmd.arg(url);
        cmd
    };

    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut cmd = Command::new("xdg-open");
        cmd.arg(url);
        cmd
    };

    command
        .spawn()
        .with_context(|| format!("Failed to open dashboard {url}"))?;

    Ok(())
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
        println!("date,commands,input_tokens,output_tokens,saved_tokens,savings_pct,total_time_ms,avg_time_ms");
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
        println!("month,commands,input_tokens,output_tokens,saved_tokens,savings_pct,total_time_ms,avg_time_ms");
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
    use crate::discover::registry::cmd_has_rtk_disabled_prefix;

    let provider = ClaudeProvider;

    // Quick scan: last 7 days only
    let sessions = provider.discover_sessions(None, Some(7)).ok()?;

    // Early bail if no sessions or too many (avoid slow scan)
    if sessions.is_empty() || sessions.len() > 200 {
        return None;
    }

    let mut total_bash: usize = 0;
    let mut bypassed: usize = 0;

    for session_path in &sessions {
        let extracted = match provider.extract_commands(session_path) {
            Ok(cmds) => cmds,
            Err(_) => continue,
        };

        for ext_cmd in &extracted {
            total_bash += 1;
            if cmd_has_rtk_disabled_prefix(&ext_cmd.command) {
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

    eprint!("This will permanently delete all tracking data. Continue? [y/N] ");
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
