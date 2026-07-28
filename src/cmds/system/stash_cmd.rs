//! `rtk stash` — park stdin behind a content-addressed recall handle, plus
//! `--list` and `--gc` management of the stash store.

use anyhow::Result;
use colored::Colorize;
use std::io::Read;

use crate::core::stash::{StashRow, StashStore};
use crate::core::stream::RAW_CAP;

/// Entry point for the `rtk stash` subcommand.
pub fn run(
    list: bool,
    gc: bool,
    limit: usize,
    command_label: Option<String>,
    content_type: Option<String>,
) -> Result<()> {
    let store = StashStore::open()?;

    if gc {
        let stats = store.gc()?;
        println!(
            "stash gc: {} expired, {} evicted, {} missing pruned · {} remaining",
            stats.expired,
            stats.evicted,
            stats.pruned_missing,
            human_bytes(stats.remaining_bytes),
        );
        return Ok(());
    }

    if list {
        return print_list(&store, limit);
    }

    // Sink mode: read stdin, park it, print the recall handle.
    let mut buf = String::new();
    std::io::stdin()
        .take((RAW_CAP + 1) as u64)
        .read_to_string(&mut buf)
        .map_err(|e| anyhow::anyhow!("Failed to read stdin: {}", e))?;
    if buf.len() > RAW_CAP {
        anyhow::bail!("stdin exceeds {} byte limit", RAW_CAP);
    }
    if buf.trim().is_empty() {
        anyhow::bail!("nothing on stdin to stash");
    }

    let label = command_label.unwrap_or_else(|| "stdin".to_string());
    let entry = store.put(&buf, &label, content_type.as_deref().unwrap_or(""))?;

    let dedup = if entry.deduped { " (dedup)" } else { "" };
    println!(
        "{}  ·  {} · ~{} tokens parked{}",
        format!("⟐ rtk:{}", entry.handle).bold(),
        human_bytes(entry.bytes as u64),
        fmt_int(entry.tokens),
        dedup,
    );
    println!(
        "   {}",
        format!(
            "recall: rtk retrieve {}  [--grep PAT | --lines A-B]",
            entry.handle
        )
        .dimmed()
    );
    Ok(())
}

fn print_list(store: &StashStore, limit: usize) -> Result<()> {
    let rows = store.list(limit)?;
    if rows.is_empty() {
        println!("stash is empty");
        return Ok(());
    }

    let hlen = store.handle_len().max(8);
    let total_bytes: u64 = rows.iter().map(|r| r.bytes as u64).sum();

    println!(
        "{:<width$}  {:>8}  {:>9}  {:<6}  {}",
        "HANDLE".bold(),
        "AGE".bold(),
        "TOKENS".bold(),
        "TYPE".bold(),
        "COMMAND".bold(),
        width = hlen,
    );
    for r in &rows {
        println!(
            "{:<width$}  {:>8}  {:>9}  {:<6}  {}",
            r.handle(hlen),
            humanize_age(&r.created),
            fmt_int(r.tokens),
            truncate(&r.content_type, 6),
            truncate(&r.command, 48),
            width = hlen,
        );
    }
    println!(
        "{}",
        format!(
            "({} entr{} · {})",
            rows.len(),
            if rows.len() == 1 { "y" } else { "ies" },
            human_bytes(total_bytes)
        )
        .dimmed()
    );
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

fn fmt_int(n: usize) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut val = bytes as f64;
    let mut unit = 0;
    while val >= 1024.0 && unit < UNITS.len() - 1 {
        val /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{:.1} {}", val, UNITS[unit])
    }
}

fn humanize_age(created_rfc3339: &str) -> String {
    let Ok(created) = chrono::DateTime::parse_from_rfc3339(created_rfc3339) else {
        return "?".to_string();
    };
    let secs = (chrono::Utc::now() - created.with_timezone(&chrono::Utc)).num_seconds();
    if secs < 0 {
        return "now".to_string();
    }
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

/// Print a compact metadata block for a resolved row (shared with `rtk retrieve --meta`).
pub fn print_meta(row: &StashRow, hlen: usize) {
    println!("handle:       {}", row.handle(hlen));
    println!("hash:         {}", row.hash);
    println!("command:      {}", row.command);
    println!("content_type: {}", row.content_type);
    println!("bytes:        {}", fmt_int(row.bytes));
    println!("tokens:       ~{}", fmt_int(row.tokens));
    println!("created:      {} ({})", row.created, humanize_age(&row.created));
    println!("last_access:  {}", row.last_accessed);
    println!("access_count: {}", row.access_count);
    println!("path:         {}", row.path.display());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fmt_int_thousands() {
        assert_eq!(fmt_int(0), "0");
        assert_eq!(fmt_int(999), "999");
        assert_eq!(fmt_int(1000), "1,000");
        assert_eq!(fmt_int(1234567), "1,234,567");
    }

    #[test]
    fn test_human_bytes() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(1_572_864), "1.5 MB");
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("verylongstring", 6), "veryl…");
    }
}
