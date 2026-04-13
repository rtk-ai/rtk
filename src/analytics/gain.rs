//! Shows users how many tokens RTK has saved them over time.

use crate::core::display_helpers::{
    format_duration, print_period_table, start_output_capture, take_output_capture,
};
use crate::core::tracking::{DayStats, MonthStats, Tracker, WeekStats};
use crate::core::utils::format_tokens;
use crate::hooks::hook_check;
use anyhow::{bail, Context, Result};
use chrono::Local;
use colored::Colorize;
use serde::Serialize;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// Shadow `println!` to route through the capture system.
// When capture is active (watch mode) → buffer; when inactive → regular stdout.
macro_rules! println {
    () => { crate::core::display_helpers::captured_println(format_args!("")) };
    ($($arg:tt)*) => { crate::core::display_helpers::captured_println(format_args!($($arg)*)) };
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
    watch: bool,
    interval: u64,
    _verbose: u8,
) -> Result<()> {
    if watch && (format == "json" || format == "csv") {
        bail!("--watch is not compatible with --format {format} (only text format is supported)");
    }

    if watch && !std::io::stdout().is_terminal() {
        bail!("--watch requires an interactive terminal (stdout is not a TTY)");
    }

    if watch {
        return run_watch_loop(
            project, graph, history, quota, tier, daily, weekly, monthly, all, failures, interval,
        );
    }

    run_once(
        project, graph, history, quota, tier, daily, weekly, monthly, all, format, failures,
    )
}

/// RAII guard that switches to the alternate screen buffer, hides the cursor,
/// and restores everything on drop (including panic).
struct CursorGuard;

impl CursorGuard {
    fn new() -> Self {
        // Alternate screen buffer + hide cursor + disable line wrapping
        print!("\x1b[?1049h\x1b[?25l\x1b[?7l");
        let _ = std::io::stdout().flush();
        Self
    }
}

impl Drop for CursorGuard {
    fn drop(&mut self) {
        // Re-enable line wrapping + show cursor + leave alternate screen buffer
        let _ = write!(std::io::stdout(), "\x1b[?7h\x1b[?25h\x1b[?1049l");
        let _ = std::io::stdout().flush();
    }
}

/// Returns (width, height) of the terminal, with sensible defaults.
fn terminal_dimensions() -> (usize, usize) {
    terminal_size::terminal_size()
        .map(|(w, h)| (w.0 as usize, h.0 as usize))
        .unwrap_or((80, 24))
}

#[allow(clippy::too_many_arguments)]
fn run_watch_loop(
    project: bool,
    graph: bool,
    history: bool,
    quota: bool,
    tier: &str,
    daily: bool,
    weekly: bool,
    monthly: bool,
    all: bool,
    failures: bool,
    interval: u64,
) -> Result<()> {
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })
    .context("Failed to set Ctrl+C handler")?;

    // Build the command description for the watch header
    let mut flag_parts: Vec<String> = vec!["rtk gain".into()];
    for (on, name) in [
        (project, "--project"),
        (graph, "--graph"),
        (history, "--history"),
        (quota, "--quota"),
        (daily, "--daily"),
        (weekly, "--weekly"),
        (monthly, "--monthly"),
        (all, "--all"),
        (failures, "--failures"),
    ] {
        if on {
            flag_parts.push(name.into());
        }
    }
    if quota && tier != "20x" {
        flag_parts.push(format!("--tier {tier}"));
    }
    if interval != 2 {
        flag_parts.push(format!("-n {interval}"));
    }
    let flags = flag_parts.join(" ");

    let tracker = Tracker::new().context("Failed to initialize tracking database")?;
    let project_scope = resolve_project_scope(project)?;
    let _cursor_guard = CursorGuard::new();

    while running.load(Ordering::SeqCst) {
        let (term_w, term_h) = terminal_dimensions();

        // Capture all println! output to a buffer (via the shadow macro).
        start_output_capture();

        let now = Local::now().format("%Y-%m-%d %H:%M:%S");
        println!("Every {}s: {:<40} {}", interval, flags, now);
        println!();

        // Reserve lines for: watch header(2) + title(1) + sep(1) + blank(1) +
        // KPIs(6) + blank(1) + section header(1) + table header(3) + table footer(2)
        // = ~18 lines of overhead before command rows
        let overhead = 18;
        let max_cmd_rows = term_h.saturating_sub(overhead).max(3);

        let _ = run_display(
            &tracker,
            project_scope.as_deref(),
            graph,
            history,
            quota,
            tier,
            daily,
            weekly,
            monthly,
            all,
            failures,
            Some(max_cmd_rows),
            Some(term_w),
        );

        // Flush the entire frame in a single write: home + content + clear-to-end.
        // The terminal processes this atomically — no visible blank frame.
        // \x1b[K (clear to end of line) is already embedded in each line by captured_println.
        if let Some(content) = take_output_capture() {
            print!("\x1b[H{}\x1b[J", content);
        }
        let _ = std::io::stdout().flush();

        for _ in 0..(interval * 10) {
            if !running.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_once(
    project: bool,
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
) -> Result<()> {
    let tracker = Tracker::new().context("Failed to initialize tracking database")?;
    let project_scope = resolve_project_scope(project)?;

    if failures {
        return show_failures(&tracker);
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
                project_scope.as_deref(),
            );
        }
        "csv" => {
            return export_csv(
                &tracker,
                daily,
                weekly,
                monthly,
                all,
                project_scope.as_deref(),
            );
        }
        _ => {}
    }

    run_display(
        &tracker,
        project_scope.as_deref(),
        graph,
        history,
        quota,
        tier,
        daily,
        weekly,
        monthly,
        all,
        failures,
        None,
        None,
    )
}

/// Core display logic, separated to allow Tracker reuse in watch mode.
#[allow(clippy::too_many_arguments)]
fn run_display(
    tracker: &Tracker,
    project_scope: Option<&str>,
    graph: bool,
    history: bool,
    quota: bool,
    tier: &str,
    daily: bool,
    weekly: bool,
    monthly: bool,
    all: bool,
    failures: bool,
    max_cmd_rows: Option<usize>,
    term_width: Option<usize>,
) -> Result<()> {
    if failures {
        return show_failures(tracker);
    }

    let summary = tracker
        .get_summary_filtered(project_scope)
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
        let sep_width = term_width.unwrap_or(80).min(60);
        println!("{}", styled(title, true));
        println!("{}", "═".repeat(sep_width));
        // added: show project path when scoped
        if let Some(scope) = project_scope {
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

        if !summary.by_command.is_empty() {
            // added: styled section header
            println!("{}", styled("By Command", true));

            // Dynamic column widths for clean alignment
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

            // Adapt cmd_width and impact_width to fit terminal width
            // Fixed overhead: "#"(3) + 6×gap(12) + count + saved + "%"(6) + time
            let fixed_cols = 3 + 12 + count_width + saved_width + 6 + time_width;
            let available = term_width.unwrap_or(120).saturating_sub(fixed_cols);
            // Split available space: ~70% command, ~30% impact bar
            let cmd_width = (available * 7 / 10).clamp(10, 30);
            let impact_width = available.saturating_sub(cmd_width).clamp(4, 14);

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

            let display_rows = max_cmd_rows.unwrap_or(summary.by_command.len());
            let total_cmds = summary.by_command.len();
            let visible = display_rows.min(total_cmds);

            for (idx, (cmd, count, saved, pct, avg_time)) in
                summary.by_command.iter().take(visible).enumerate()
            {
                let row_idx = format!("{:>2}.", idx + 1);
                let cmd_cell = style_command_cell(&truncate_for_column(cmd, cmd_width));
                let count_cell = format!("{:>count_width$}", count, count_width = count_width);
                let saved_cell = format!(
                    "{:>saved_width$}",
                    format_tokens(*saved),
                    saved_width = saved_width
                );
                let pct_plain = format!("{:>6}", format!("{pct:.1}%"));
                let pct_cell = colorize_pct_cell(*pct, &pct_plain);
                let time_cell = format!(
                    "{:>time_width$}",
                    format_duration(*avg_time),
                    time_width = time_width
                );
                let impact = mini_bar(*saved, max_saved, impact_width);
                println!(
                    "{}  {}  {}  {}  {}  {}  {}",
                    row_idx, cmd_cell, count_cell, saved_cell, pct_cell, time_cell, impact
                );
            }
            if visible < total_cmds {
                println!(
                    "     ... +{} more (enlarge terminal to see all)",
                    total_cmds - visible
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
            let recent = tracker.get_recent_filtered(10, project_scope)?;
            if !recent.is_empty() {
                println!("{}", styled("Recent Commands", true)); // added: styled header
                println!("──────────────────────────────────────────────────────────");
                for rec in recent {
                    let time = rec.timestamp.with_timezone(&Local).format("%m-%d %H:%M");
                    let cmd_short = if rec.rtk_cmd.len() > 25 {
                        format!("{}...", &rec.rtk_cmd[..22])
                    } else {
                        rec.rtk_cmd.clone()
                    };
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
        print_daily_full(tracker, project_scope)?;
    }

    if all || weekly {
        print_weekly(tracker, project_scope)?;
    }

    if all || monthly {
        print_monthly(tracker, project_scope)?;
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
    use crate::discover::registry::has_rtk_disabled_prefix;

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
            if has_rtk_disabled_prefix(&ext_cmd.command) {
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
            let cmd_display = if cmd.len() > 50 {
                format!("{}...", &cmd[..47])
            } else {
                cmd.clone()
            };
            println!("  {:>4}x  {}", count, cmd_display);
        }
        println!();
    }

    if !summary.recent.is_empty() {
        println!("{}", styled("Recent Failures (last 10)", true));
        println!("{}", "─".repeat(60));
        for rec in &summary.recent {
            let ts_short = if rec.timestamp.len() >= 16 {
                &rec.timestamp[..16]
            } else {
                &rec.timestamp
            };
            let status = if rec.fallback_succeeded { "ok" } else { "FAIL" };
            let cmd_display = if rec.raw_command.len() > 40 {
                format!("{}...", &rec.raw_command[..37])
            } else {
                rec.raw_command.clone()
            };
            println!("  {} [{}] {}", ts_short, status, cmd_display);
        }
        println!();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    #[derive(Parser)]
    #[command()]
    struct TestCli {
        #[command(subcommand)]
        command: TestCommands,
    }

    #[derive(clap::Subcommand)]
    enum TestCommands {
        Gain {
            #[arg(short, long)]
            project: bool,
            #[arg(short, long)]
            graph: bool,
            #[arg(short = 'H', long)]
            history: bool,
            #[arg(short, long)]
            quota: bool,
            #[arg(short, long, default_value = "20x", requires = "quota")]
            tier: String,
            #[arg(short, long)]
            daily: bool,
            #[arg(short, long)]
            weekly: bool,
            #[arg(short, long)]
            monthly: bool,
            #[arg(short, long)]
            all: bool,
            #[arg(short, long, default_value = "text")]
            format: String,
            #[arg(short = 'F', long)]
            failures: bool,
            #[arg(short = 'W', long)]
            watch: bool,
            #[arg(short = 'n', long, default_value = "2", requires = "watch", value_parser = clap::value_parser!(u64).range(1..=3600))]
            interval: u64,
        },
    }

    #[test]
    fn test_watch_rejects_json_format() {
        let result = super::run(
            false, false, false, false, "20x", false, false, false, false, "json", false, true, 2,
            0,
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not compatible"),
            "Expected format incompatibility error, got: {err}"
        );
    }

    #[test]
    fn test_watch_rejects_csv_format() {
        let result = super::run(
            false, false, false, false, "20x", false, false, false, false, "csv", false, true, 2, 0,
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not compatible"),
            "Expected format incompatibility error, got: {err}"
        );
    }

    #[test]
    fn test_watch_flag_requires_watch_for_interval() {
        let result = TestCli::try_parse_from(["test", "gain", "--interval", "5"]);
        assert!(result.is_err(), "--interval without --watch should fail");
    }

    #[test]
    fn test_watch_interval_bounds() {
        let result = TestCli::try_parse_from(["test", "gain", "--watch", "--interval", "0"]);
        assert!(result.is_err(), "interval=0 should be rejected");

        let result = TestCli::try_parse_from(["test", "gain", "--watch", "--interval", "3601"]);
        assert!(result.is_err(), "interval=3601 should be rejected");

        let result = TestCli::try_parse_from(["test", "gain", "--watch", "--interval", "1"]);
        assert!(result.is_ok(), "interval=1 should be accepted");

        let result = TestCli::try_parse_from(["test", "gain", "--watch", "--interval", "3600"]);
        assert!(result.is_ok(), "interval=3600 should be accepted");
    }

    #[test]
    fn test_watch_parses_with_default_interval() {
        let result = TestCli::try_parse_from(["test", "gain", "--watch"]);
        assert!(result.is_ok());
        if let Ok(cli) = result {
            match cli.command {
                TestCommands::Gain {
                    watch, interval, ..
                } => {
                    assert!(watch);
                    assert_eq!(interval, 2, "default interval should be 2s");
                }
            }
        }
    }

    #[test]
    fn test_watch_flag_header_reconstruction() {
        let bool_flags = [
            (true, "--project"),
            (true, "--graph"),
            (false, "--history"),
            (false, "--quota"),
            (true, "--daily"),
            (false, "--weekly"),
            (false, "--monthly"),
            (false, "--all"),
            (false, "--failures"),
        ];
        let mut parts: Vec<&str> = vec!["rtk gain"];
        for (on, name) in &bool_flags {
            if *on {
                parts.push(name);
            }
        }
        let flags = parts.join(" ");
        assert_eq!(flags, "rtk gain --project --graph --daily");
    }

    #[test]
    fn test_watch_flag_header_with_non_default_tier_and_interval() {
        let mut parts: Vec<&str> = vec!["rtk gain"];
        let bool_flags = [(true, "--quota"), (false, "--daily")];
        for (on, name) in &bool_flags {
            if *on {
                parts.push(name);
            }
        }
        let tier = "pro";
        let tier_flag = format!("--tier {tier}");
        if tier != "20x" {
            parts.push(&tier_flag);
        }
        let interval = 5u64;
        let interval_flag = format!("-n {interval}");
        if interval != 2 {
            parts.push(&interval_flag);
        }
        let flags = parts.join(" ");
        assert_eq!(flags, "rtk gain --quota --tier pro -n 5");
    }
}
