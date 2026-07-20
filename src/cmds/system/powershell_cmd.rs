//! Compact output for PowerShell and WSL-to-Windows PowerShell invocations.

use crate::core::runner::{self, RunOptions};
use crate::core::truncate::CAP_INVENTORY;
use crate::core::utils::resolved_command;
use anyhow::Result;
use regex::Regex;
use std::ffi::OsString;
use std::sync::OnceLock;

pub fn run(executable: &str, args: &[String], verbose: u8) -> Result<i32> {
    if should_passthrough(args) {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        return runner::run_passthrough(executable, &args, verbose);
    }

    let mut cmd = resolved_command(executable);
    cmd.args(args);
    if verbose > 0 {
        eprintln!("Running: {} {}", executable, args.join(" "));
    }

    runner::run_filtered(
        cmd,
        executable,
        &args.join(" "),
        filter_output,
        RunOptions::default()
            .inherit_stdin()
            .early_exit_on_failure(),
    )
}

fn should_passthrough(args: &[String]) -> bool {
    if args.is_empty() {
        return true;
    }

    if args.iter().any(|arg| {
        matches!(
            arg.to_ascii_lowercase().as_str(),
            "-noexit" | "-help" | "--help" | "-?" | "-version"
        )
    }) {
        return true;
    }

    args.iter()
        .any(|arg| arg.to_ascii_lowercase().contains("convertto-json"))
}

fn filter_output(raw: &str) -> String {
    let normalized = normalize_output(raw);
    let filtered = if looks_like_table(&normalized) {
        compact_table(&normalized)
    } else if looks_like_format_list(&normalized) {
        compact_format_list(&normalized)
    } else {
        normalized.clone()
    };

    cap_large_output(&filtered, raw, None)
}

fn normalize_output(raw: &str) -> String {
    raw.trim_start_matches('\u{feff}')
        .lines()
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end_matches(['\r', '\n'])
        .to_string()
}

fn multi_space_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"\s{2,}").expect("valid PowerShell spacing regex"))
}

fn format_list_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^\s*[^:]+?\s+:\s*.*$").expect("valid PowerShell format-list regex")
    })
}

fn is_table_separator(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && trimmed.contains("--")
        && trimmed.chars().all(|ch| ch == '-' || ch.is_whitespace())
}

fn looks_like_table(output: &str) -> bool {
    let lines: Vec<&str> = output.lines().collect();
    lines
        .windows(2)
        .any(|pair| !pair[0].trim().is_empty() && is_table_separator(pair[1]))
}

fn compact_table(output: &str) -> String {
    output
        .lines()
        .filter(|line| !is_table_separator(line))
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                multi_space_regex()
                    .split(line.trim())
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" | ")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn looks_like_format_list(output: &str) -> bool {
    output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter(|line| format_list_regex().is_match(line))
        .take(2)
        .count()
        >= 2
}

fn compact_format_list(output: &str) -> String {
    output
        .lines()
        .map(|line| {
            if let Some((key, value)) = line.split_once(':') {
                if !key.trim().is_empty() {
                    return format!("{}: {}", key.trim(), value.trim());
                }
            }
            line.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn cap_large_output(filtered: &str, raw: &str, recovery_hint: Option<&str>) -> String {
    let lines: Vec<&str> = filtered.lines().collect();
    if lines.len() <= CAP_INVENTORY {
        return filtered.to_string();
    }

    let head_count = CAP_INVENTORY / 2;
    let tail_count = CAP_INVENTORY - head_count;
    let omitted = lines.len() - CAP_INVENTORY;
    let mut output = Vec::with_capacity(CAP_INVENTORY + 2);
    output.extend(lines.iter().take(head_count).copied());
    let marker = format!("... {omitted} lines omitted ...");
    output.push(&marker);
    output.extend(lines.iter().skip(lines.len() - tail_count).copied());

    let mut output = output.join("\n");
    let hint = recovery_hint
        .map(str::to_string)
        .or_else(|| crate::core::tee::force_tee_hint(raw, "powershell"));
    if let Some(hint) = hint {
        output.push('\n');
        output.push_str(&hint);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_covers_interactive_and_json_modes() {
        for args in [
            vec![],
            vec!["-NoExit".into()],
            vec!["-Version".into()],
            vec!["-Command".into(), "Get-Process | ConvertTo-Json".into()],
        ] {
            assert!(should_passthrough(&args), "expected passthrough for {args:?}");
        }
        assert!(!should_passthrough(&[
            "-NoProfile".into(),
            "-Command".into(),
            "Get-Process".into()
        ]));
    }

    #[test]
    fn normalizes_utf8_bom_and_crlf() {
        assert_eq!(normalize_output("\u{feff}Name\r\nValue\r\n"), "Name\nValue");
    }

    #[test]
    fn compacts_format_table_padding() {
        let raw = concat!(
            "Name                           Id   CPU\n",
            "----                           --   ---\n",
            "alpha process                 123   1.2\n",
            "beta                          456   3.4\n"
        );
        assert!(looks_like_table(raw));
        assert_eq!(
            compact_table(raw),
            "Name | Id | CPU\nalpha process | 123 | 1.2\nbeta | 456 | 3.4"
        );
    }

    #[test]
    fn compacts_format_list_colons() {
        let raw = "Name        : alpha\nIdentifier  : 42\n\nName        : beta\nIdentifier  : 43\n";
        assert!(looks_like_format_list(raw));
        assert_eq!(
            compact_format_list(raw),
            "Name: alpha\nIdentifier: 42\n\nName: beta\nIdentifier: 43"
        );
    }

    #[test]
    fn large_output_keeps_head_tail_and_recovery() {
        let raw = (1..=60)
            .map(|line| format!("row {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let filtered = cap_large_output(
            &raw,
            &raw,
            Some("[full output: /tmp/powershell.log]"),
        );

        assert!(filtered.starts_with("row 1\n"));
        assert!(filtered.contains("... 10 lines omitted ..."));
        assert!(filtered.contains("\nrow 60\n"));
        assert!(filtered.ends_with("[full output: /tmp/powershell.log]"));
    }

    #[test]
    fn short_generic_output_is_preserved() {
        assert_eq!(filter_output("hello\r\nworld\r\n"), "hello\nworld");
    }
}
