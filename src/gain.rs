use crate::display_helpers::print_period_table;
use crate::tracking::{DayStats, MonthStats, Tracker, WeekStats};
use crate::utils::format_tokens;
use anyhow::{Context, Result};
use serde::Serialize;

pub fn run(
    graph: bool,
    history: bool,
    quota: bool,
    tier: &str,
    daily: bool,
    weekly: bool,
    monthly: bool,
    all: bool,
    format: &str,
    _verbose: u8,
) -> Result<()> {
    let tracker = Tracker::new().context("Failed to initialize tracking database")?;

    // Handle export formats
    match format {
        "json" => return export_json(&tracker, daily, weekly, monthly, all),
        "csv" => return export_csv(&tracker, daily, weekly, monthly, all),
        _ => {} // Continue with text format
    }

    let summary = tracker
        .get_summary()
        .context("Failed to load token savings summary from database")?;

    if summary.total_commands == 0 {
        println!("No tracking data yet.");
        println!("Run some rtk commands to start tracking savings.");
        return Ok(());
    }

    // Default view (summary)
    if !daily && !weekly && !monthly && !all {
        println!("📊 RTK Token Savings");
        println!("════════════════════════════════════════");
        println!();

        println!("Total commands:    {}", summary.total_commands);
        println!("Input tokens:      {}", format_tokens(summary.total_input));
        println!("Output tokens:     {}", format_tokens(summary.total_output));
        println!(
            "Tokens saved:      {} ({:.1}%)",
            format_tokens(summary.total_saved),
            summary.avg_savings_pct
        );
        println!();

        if !summary.by_command.is_empty() {
            println!("By Command:");
            println!("────────────────────────────────────────");
            println!(
                "{:<20} {:>6} {:>10} {:>8}",
                "Command", "Count", "Saved", "Avg%"
            );
            for (cmd, count, saved, pct) in &summary.by_command {
                let cmd_short = if cmd.len() > 18 {
                    format!("{}...", &cmd[..15])
                } else {
                    cmd.clone()
                };
                println!(
                    "{:<20} {:>6} {:>10} {:>7.1}%",
                    cmd_short,
                    count,
                    format_tokens(*saved),
                    pct
                );
            }
            println!();
        }

        if graph && !summary.by_day.is_empty() {
            println!("Daily Savings (last 30 days):");
            println!("────────────────────────────────────────");
            print_ascii_graph(&summary.by_day);
            println!();
        }

        if history {
            let recent = tracker.get_recent(10)?;
            if !recent.is_empty() {
                println!("Recent Commands:");
                println!("────────────────────────────────────────");
                for rec in recent {
                    let time = rec.timestamp.format("%m-%d %H:%M");
                    let cmd_short = if rec.rtk_cmd.len() > 25 {
                        format!("{}...", &rec.rtk_cmd[..22])
                    } else {
                        rec.rtk_cmd.clone()
                    };
                    println!(
                        "{} {:<25} -{:.0}% ({})",
                        time,
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

            println!("Monthly Quota Analysis:");
            println!("────────────────────────────────────────");
            println!("Subscription tier:        {}", tier_name);
            println!("Estimated monthly quota:  {}", format_tokens(quota_tokens));
            println!(
                "Tokens saved (lifetime):  {}",
                format_tokens(summary.total_saved)
            );
            println!("Quota preserved:          {:.1}%", quota_pct);
            println!();
            println!("Note: Heuristic estimate based on ~44K tokens/5h (Pro baseline)");
            println!("      Actual limits use rolling 5-hour windows, not monthly caps.");
        }

        return Ok(());
    }

    // Time breakdown views
    if all || daily {
        print_daily_full(&tracker)?;
    }

    if all || weekly {
        print_weekly(&tracker)?;
    }

    if all || monthly {
        print_monthly(&tracker)?;
    }

    Ok(())
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

fn print_daily_full(tracker: &Tracker) -> Result<()> {
    let days = tracker.get_all_days()?;
    print_period_table(&days);
    Ok(())
}

fn print_weekly(tracker: &Tracker) -> Result<()> {
    let weeks = tracker.get_by_week()?;
    print_period_table(&weeks);
    Ok(())
}

fn print_monthly(tracker: &Tracker) -> Result<()> {
    let months = tracker.get_by_month()?;
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
}

fn export_json(
    tracker: &Tracker,
    daily: bool,
    weekly: bool,
    monthly: bool,
    all: bool,
) -> Result<()> {
    let summary = tracker
        .get_summary()
        .context("Failed to load token savings summary from database")?;

    let export = ExportData {
        summary: ExportSummary {
            total_commands: summary.total_commands,
            total_input: summary.total_input,
            total_output: summary.total_output,
            total_saved: summary.total_saved,
            avg_savings_pct: summary.avg_savings_pct,
        },
        daily: if all || daily {
            Some(tracker.get_all_days()?)
        } else {
            None
        },
        weekly: if all || weekly {
            Some(tracker.get_by_week()?)
        } else {
            None
        },
        monthly: if all || monthly {
            Some(tracker.get_by_month()?)
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
) -> Result<()> {
    if all || daily {
        let days = tracker.get_all_days()?;
        println!("# Daily Data");
        println!("date,commands,input_tokens,output_tokens,saved_tokens,savings_pct");
        for day in days {
            println!(
                "{},{},{},{},{},{:.2}",
                day.date,
                day.commands,
                day.input_tokens,
                day.output_tokens,
                day.saved_tokens,
                day.savings_pct
            );
        }
        println!();
    }

    if all || weekly {
        let weeks = tracker.get_by_week()?;
        println!("# Weekly Data");
        println!(
            "week_start,week_end,commands,input_tokens,output_tokens,saved_tokens,savings_pct"
        );
        for week in weeks {
            println!(
                "{},{},{},{},{},{},{:.2}",
                week.week_start,
                week.week_end,
                week.commands,
                week.input_tokens,
                week.output_tokens,
                week.saved_tokens,
                week.savings_pct
            );
        }
        println!();
    }

    if all || monthly {
        let months = tracker.get_by_month()?;
        println!("# Monthly Data");
        println!("month,commands,input_tokens,output_tokens,saved_tokens,savings_pct");
        for month in months {
            println!(
                "{},{},{},{},{},{:.2}",
                month.month,
                month.commands,
                month.input_tokens,
                month.output_tokens,
                month.saved_tokens,
                month.savings_pct
            );
        }
    }

    Ok(())
}
