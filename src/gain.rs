use anyhow::Result;
use crate::tracking::Tracker;

pub fn run(graph: bool, history: bool, quota: bool, tier: &str, _verbose: u8) -> Result<()> {
    let tracker = Tracker::new()?;
    let summary = tracker.get_summary()?;

    if summary.total_commands == 0 {
        println!("No tracking data yet.");
        println!("Run some rtk commands to start tracking savings.");
        return Ok(());
    }

    println!("📊 RTK Token Savings");
    println!("════════════════════════════════════════");
    println!();

    println!("Total commands:    {}", summary.total_commands);
    println!("Input tokens:      {}", format_tokens(summary.total_input));
    println!("Output tokens:     {}", format_tokens(summary.total_output));
    println!("Tokens saved:      {} ({:.1}%)",
        format_tokens(summary.total_saved),
        summary.avg_savings_pct
    );
    println!();

    if !summary.by_command.is_empty() {
        println!("By Command:");
        println!("────────────────────────────────────────");
        println!("{:<20} {:>6} {:>10} {:>8}", "Command", "Count", "Saved", "Avg%");
        for (cmd, count, saved, pct) in &summary.by_command {
            let cmd_short = if cmd.len() > 18 {
                format!("{}...", &cmd[..15])
            } else {
                cmd.clone()
            };
            println!("{:<20} {:>6} {:>10} {:>7.1}%", cmd_short, count, format_tokens(*saved), pct);
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
                println!("{} {:<25} -{:.0}% ({})",
                    time,
                    cmd_short,
                    rec.savings_pct,
                    format_tokens(rec.saved_tokens)
                );
            }
        }
    }

    if quota {
        const ESTIMATED_PRO_MONTHLY: usize = 6_000_000; // ~6M tokens/month (heuristic: ~44K/5h × 6 periods/day × 30 days)

        let (quota_tokens, tier_name) = match tier {
            "pro" => (ESTIMATED_PRO_MONTHLY, "Pro ($20/mo)"),
            "5x" => (ESTIMATED_PRO_MONTHLY * 5, "Max 5x ($100/mo)"),
            "20x" => (ESTIMATED_PRO_MONTHLY * 20, "Max 20x ($200/mo)"),
            _ => (ESTIMATED_PRO_MONTHLY, "Pro ($20/mo)"), // default fallback
        };

        let quota_pct = (summary.total_saved as f64 / quota_tokens as f64) * 100.0;

        println!("Monthly Quota Analysis:");
        println!("────────────────────────────────────────");
        println!("Subscription tier:        {}", tier_name);
        println!("Estimated monthly quota:  {}", format_tokens(quota_tokens));
        println!("Tokens saved (lifetime):  {}", format_tokens(summary.total_saved));
        println!("Quota preserved:          {:.1}%", quota_pct);
        println!();
        println!("Note: Heuristic estimate based on ~44K tokens/5h (Pro baseline)");
        println!("      Actual limits use rolling 5-hour windows, not monthly caps.");
    }

    Ok(())
}

fn format_tokens(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        format!("{}", n)
    }
}

fn print_ascii_graph(data: &[(String, usize)]) {
    if data.is_empty() {
        return;
    }

    let max_val = data.iter().map(|(_, v)| *v).max().unwrap_or(1);
    let width = 40;

    for (date, value) in data {
        let date_short = if date.len() >= 10 {
            &date[5..10]
        } else {
            date
        };

        let bar_len = if max_val > 0 {
            ((*value as f64 / max_val as f64) * width as f64) as usize
        } else {
            0
        };

        let bar: String = "█".repeat(bar_len);
        let spaces: String = " ".repeat(width - bar_len);

        println!("{} │{}{} {}", date_short, bar, spaces, format_tokens(*value));
    }
}

pub fn run_compact(verbose: u8) -> Result<()> {
    let tracker = Tracker::new()?;
    let summary = tracker.get_summary()?;

    if summary.total_commands == 0 {
        println!("0 cmds tracked");
        return Ok(());
    }

    println!("{}cmds {}in {}out {}saved ({:.0}%)",
        summary.total_commands,
        format_tokens(summary.total_input),
        format_tokens(summary.total_output),
        format_tokens(summary.total_saved),
        summary.avg_savings_pct
    );

    Ok(())
}
