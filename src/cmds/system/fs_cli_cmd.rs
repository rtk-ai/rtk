//! Compact FreeSWITCH `fs_cli -x` command output.

use crate::core::runner::{self, RunOptions};
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

static FOOTER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+)\s+total\.$").unwrap());
static WIDE_GAP_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s{2,}").unwrap());

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("fs_cli");
    cmd.args(args);
    let args_display = args.join(" ");

    if verbose > 0 {
        eprintln!("Running: fs_cli {args_display}");
    }

    runner::run_filtered(
        cmd,
        "fs_cli",
        &args_display,
        filter_fs_cli,
        RunOptions::with_tee("fs_cli").early_exit_on_failure(),
    )
}

fn filter_fs_cli(raw: &str) -> String {
    let clean = strip_ansi(raw);
    let nonempty: Vec<&str> = clean.lines().filter(|line| !line.trim().is_empty()).collect();
    if nonempty.len() <= 1 || nonempty.iter().any(|line| line.trim_start().starts_with("-ERR")) {
        return clean.trim_end().to_string();
    }

    let footer_count = nonempty.last().and_then(|line| {
        FOOTER_RE
            .captures(line.trim())
            .and_then(|captures| captures[1].parse::<usize>().ok())
    });
    let mut output = Vec::new();

    for (index, line) in nonempty.iter().enumerate() {
        let trimmed = line.trim();
        if is_separator(trimmed) {
            continue;
        }
        if index + 1 == nonempty.len() && footer_count.is_some() {
            continue;
        }
        output.push(compact_table_line(trimmed));
    }

    if let Some(count) = footer_count {
        let derived_rows = output.len().saturating_sub(1);
        if derived_rows != count {
            output.push(format!("{count} total."));
        }
    }

    output.join("\n")
}

fn is_separator(line: &str) -> bool {
    let visible: String = line.chars().filter(|ch| !ch.is_whitespace()).collect();
    visible.len() >= 3
        && visible
            .chars()
            .all(|ch| matches!(ch, '=' | '-' | '+' | '|'))
}

fn compact_table_line(line: &str) -> String {
    if line.matches('|').count() >= 2 {
        return line
            .split('|')
            .map(str::trim)
            .filter(|cell| !cell.is_empty())
            .collect::<Vec<_>>()
            .join(" | ");
    }
    WIDE_GAP_RE.replace_all(line, " | ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_decorative_rows_and_derivable_footer() {
        let raw = "Name                 Type       Data       State\n=================================================\nexternal             profile    10.0.0.1   RUNNING\ninternal             profile    10.0.0.2   RUNNING\n2 total.\n";
        let filtered = filter_fs_cli(raw);

        assert_eq!(
            filtered,
            "Name | Type | Data | State\nexternal | profile | 10.0.0.1 | RUNNING\ninternal | profile | 10.0.0.2 | RUNNING"
        );
        assert!(!filtered.contains("===="));
        assert!(!filtered.contains("2 total"));
    }

    #[test]
    fn compacts_pipe_bordered_table() {
        let raw = "+------+----------+\n| uuid | state    |\n+------+----------+\n| a-1  | ACTIVE   |\n+------+----------+\n1 total.\n";
        assert_eq!(filter_fs_cli(raw), "uuid | state\na-1 | ACTIVE");
    }

    #[test]
    fn keeps_footer_when_row_count_cannot_be_derived() {
        let raw = "Name Type\nonly one visible row\n5 total.\n";
        assert!(filter_fs_cli(raw).ends_with("5 total."));
    }

    #[test]
    fn small_and_error_outputs_are_unchanged() {
        assert_eq!(filter_fs_cli("+OK accepted\n"), "+OK accepted");
        let error = "-ERR command not found\nUsage: show channels\n";
        assert_eq!(filter_fs_cli(error), error.trim_end());
    }

    #[test]
    fn wide_table_saves_at_least_fifteen_percent() {
        let mut lines = vec![
            "UUID                                 Direction       State          Codec"
                .to_string(),
            "=========================================================================="
                .to_string(),
        ];
        lines.extend((0..50).map(|index| {
            format!(
                "00000000-0000-0000-0000-{index:012}    inbound         ACTIVE         PCMU"
            )
        }));
        lines.push("50 total.".to_string());
        let raw = lines.join("\n");
        let filtered = filter_fs_cli(&raw);

        assert!(filtered.len() * 20 <= raw.len() * 17);
    }
}
