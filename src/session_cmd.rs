use crate::discover::provider::{ClaudeProvider, SessionProvider};
use crate::utils::format_tokens;
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

/// A summarized session for display.
struct SessionSummary {
    id: String,
    date: String,
    total_cmds: usize,
    rtk_cmds: usize,
    output_tokens: usize,
}

impl SessionSummary {
    fn adoption_pct(&self) -> f64 {
        if self.total_cmds == 0 {
            return 0.0;
        }
        self.rtk_cmds as f64 / self.total_cmds as f64 * 100.0
    }
}

fn progress_bar(pct: f64, width: usize) -> String {
    let filled = ((pct / 100.0) * width as f64).round() as usize;
    let empty = width.saturating_sub(filled);
    format!("{}{}", "@".repeat(filled), ".".repeat(empty))
}

pub fn run(_verbose: u8) -> Result<()> {
    let provider = ClaudeProvider;
    let sessions = provider
        .discover_sessions(None, Some(30))
        .context("Failed to discover Claude Code sessions")?;

    if sessions.is_empty() {
        println!("No Claude Code sessions found in the last 30 days.");
        println!("Make sure Claude Code has been used at least once.");
        return Ok(());
    }

    // Group JSONL files by parent session (ignore subagent files)
    let mut session_files: Vec<PathBuf> = sessions
        .into_iter()
        .filter(|p| {
            // Skip subagent files — only top-level session JSONL
            !p.to_string_lossy().contains("subagents")
        })
        .collect();

    // Sort by mtime desc
    session_files.sort_by(|a, b| {
        let ma = fs::metadata(a)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let mb = fs::metadata(b)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        mb.cmp(&ma)
    });

    // Take top 10
    session_files.truncate(10);

    let mut summaries: Vec<SessionSummary> = Vec::new();

    for path in &session_files {
        let cmds = match provider.extract_commands(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        if cmds.is_empty() {
            continue;
        }

        let total_cmds = cmds.len();
        let rtk_cmds = cmds
            .iter()
            .filter(|c| c.command.starts_with("rtk "))
            .count();
        let output_tokens: usize = cmds.iter().filter_map(|c| c.output_len).sum();

        // Extract session ID from filename
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        let short_id = if id.len() > 8 { &id[..8] } else { id };

        // Extract date from mtime
        let date = fs::metadata(path)
            .and_then(|m| m.modified())
            .map(|t| {
                let elapsed = std::time::SystemTime::now()
                    .duration_since(t)
                    .unwrap_or_default();
                let days = elapsed.as_secs() / 86400;
                if days == 0 {
                    "Today".to_string()
                } else if days == 1 {
                    "Yesterday".to_string()
                } else {
                    format!("{}d ago", days)
                }
            })
            .unwrap_or_else(|_| "?".to_string());

        summaries.push(SessionSummary {
            id: short_id.to_string(),
            date,
            total_cmds,
            rtk_cmds,
            output_tokens,
        });
    }

    if summaries.is_empty() {
        println!("No sessions with Bash commands found.");
        return Ok(());
    }

    // Display table
    let header = "RTK Session Overview (last 10)";
    println!("{}", header);
    println!("{}", "-".repeat(70));
    println!(
        "{:<12} {:<12} {:>5} {:>5} {:>9} {:<7} {:>8}",
        "Session", "Date", "Cmds", "RTK", "Adoption", "", "Output"
    );
    println!("{}", "-".repeat(70));

    let mut total_cmds = 0;
    let mut total_rtk = 0;

    for s in &summaries {
        let pct = s.adoption_pct();
        let bar = progress_bar(pct, 5);
        total_cmds += s.total_cmds;
        total_rtk += s.rtk_cmds;

        println!(
            "{:<12} {:<12} {:>5} {:>5} {:>8.0}% {:<7} {:>8}",
            s.id,
            s.date,
            s.total_cmds,
            s.rtk_cmds,
            pct,
            bar,
            format_tokens(s.output_tokens),
        );
    }

    println!("{}", "-".repeat(70));

    let avg_adoption = if total_cmds > 0 {
        total_rtk as f64 / total_cmds as f64 * 100.0
    } else {
        0.0
    };
    println!("Average adoption: {:.0}%", avg_adoption);
    println!("Tip: Run `rtk discover` to find missed RTK opportunities");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_bar_empty() {
        assert_eq!(progress_bar(0.0, 5), ".....");
    }

    #[test]
    fn test_progress_bar_full() {
        assert_eq!(progress_bar(100.0, 5), "@@@@@");
    }

    #[test]
    fn test_progress_bar_half() {
        assert_eq!(progress_bar(50.0, 5), "@@@..");
    }

    #[test]
    fn test_progress_bar_partial() {
        assert_eq!(progress_bar(80.0, 5), "@@@@.");
    }

    #[test]
    fn test_session_summary_adoption_zero_cmds() {
        let s = SessionSummary {
            id: "test".to_string(),
            date: "Today".to_string(),
            total_cmds: 0,
            rtk_cmds: 0,
            output_tokens: 0,
        };
        assert_eq!(s.adoption_pct(), 0.0);
    }

    #[test]
    fn test_session_summary_adoption_all_rtk() {
        let s = SessionSummary {
            id: "test".to_string(),
            date: "Today".to_string(),
            total_cmds: 10,
            rtk_cmds: 10,
            output_tokens: 5000,
        };
        assert_eq!(s.adoption_pct(), 100.0);
    }

    #[test]
    fn test_session_summary_adoption_partial() {
        let s = SessionSummary {
            id: "test".to_string(),
            date: "Today".to_string(),
            total_cmds: 20,
            rtk_cmds: 15,
            output_tokens: 8000,
        };
        assert_eq!(s.adoption_pct(), 75.0);
    }
}
