//! Compact output for common tmux automation commands.

use crate::core::runner::{self, RunOptions};
use crate::core::truncate::CAP_INVENTORY;
use crate::core::utils::resolved_command;
use anyhow::Result;
use std::ffi::OsString;

const GLOBAL_OPTIONS_WITH_VALUE: &[&str] = &["-c", "-f", "-L", "-S", "-T"];

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let Some((subcommand_index, subcommand)) = find_subcommand(args) else {
        return run_passthrough(args, verbose);
    };

    match subcommand {
        "list-sessions" | "ls"
            if !has_custom_format(&args[subcommand_index.saturating_add(1)..]) =>
        {
            let mut cmd = resolved_command("tmux");
            cmd.args(args);
            if verbose > 0 {
                eprintln!("Running: tmux {}", args.join(" "));
            }
            runner::run_filtered(
                cmd,
                "tmux",
                &args.join(" "),
                filter_list_sessions,
                RunOptions::default().early_exit_on_failure(),
            )
        }
        "capture-pane" | "capturep" if capture_prints_stdout(&args[subcommand_index + 1..]) => {
            let mut cmd = resolved_command("tmux");
            cmd.args(args);
            if verbose > 0 {
                eprintln!("Running: tmux {}", args.join(" "));
            }
            runner::run_filtered(
                cmd,
                "tmux",
                &args.join(" "),
                filter_capture_pane,
                RunOptions::default().early_exit_on_failure(),
            )
        }
        _ => run_passthrough(args, verbose),
    }
}

fn run_passthrough(args: &[String], verbose: u8) -> Result<i32> {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    runner::run_passthrough("tmux", &args, verbose)
}

fn find_subcommand(args: &[String]) -> Option<(usize, &str)> {
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if GLOBAL_OPTIONS_WITH_VALUE.contains(&arg) {
            index += 2;
            continue;
        }
        if GLOBAL_OPTIONS_WITH_VALUE
            .iter()
            .any(|option| arg.starts_with(option) && arg.len() > option.len())
        {
            index += 1;
            continue;
        }
        if arg == "--" {
            index += 1;
            return args.get(index).map(|command| (index, command.as_str()));
        }
        if arg.starts_with('-') {
            index += 1;
            continue;
        }
        return Some((index, arg));
    }
    None
}

fn has_custom_format(args: &[String]) -> bool {
    args.iter().any(|arg| {
        arg == "-F"
            || arg == "--format"
            || arg.starts_with("-F")
            || arg.starts_with("--format=")
    })
}

fn capture_prints_stdout(args: &[String]) -> bool {
    args.iter().any(|arg| {
        arg == "-p"
            || (arg.starts_with('-')
                && !arg.starts_with("--")
                && arg.chars().skip(1).any(|flag| flag == 'p'))
    })
}

fn filter_list_sessions(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().filter(|line| !line.trim().is_empty()).collect();
    if lines.is_empty() {
        return String::new();
    }

    let compact: Option<Vec<String>> = lines
        .iter()
        .map(|line| compact_session_line(line))
        .collect();
    compact
        .map(|lines| lines.join("\n"))
        .unwrap_or_else(|| raw.trim_end_matches(['\r', '\n']).to_string())
}

fn compact_session_line(line: &str) -> Option<String> {
    let (name, details) = line.split_once(": ")?;
    let (window_count, remainder) = details.split_once(' ')?;
    window_count.parse::<usize>().ok()?;
    if !(remainder.starts_with("window (created ")
        || remainder.starts_with("windows (created "))
    {
        return None;
    }

    let attached = details.ends_with(" (attached)");
    Some(format!(
        "{} {}w{}",
        name,
        window_count,
        if attached { " attached" } else { "" }
    ))
}

fn filter_capture_pane(raw: &str) -> String {
    let hint = (visible_capture_line_count(raw) > CAP_INVENTORY)
        .then(|| crate::core::tee::force_tee_hint(raw, "tmux_capture_pane"))
        .flatten();
    compact_capture_pane(raw, hint.as_deref())
}

fn visible_capture_line_count(raw: &str) -> usize {
    let lines: Vec<&str> = raw.lines().collect();
    let Some(start) = lines.iter().position(|line| !line.trim().is_empty()) else {
        return 0;
    };
    let end = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .unwrap_or(start);
    end - start + 1
}

fn compact_capture_pane(raw: &str, recovery_hint: Option<&str>) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    let Some(start) = lines.iter().position(|line| !line.trim().is_empty()) else {
        return String::new();
    };
    let end = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .unwrap_or(start);
    let visible = &lines[start..=end];

    if visible.len() <= CAP_INVENTORY {
        return visible.join("\n");
    }

    let head_count = CAP_INVENTORY / 2;
    let tail_count = CAP_INVENTORY - head_count;
    let omitted = visible.len() - CAP_INVENTORY;
    let mut output = Vec::with_capacity(CAP_INVENTORY + 2);
    output.extend(visible.iter().take(head_count).copied());
    let marker = format!("... {omitted} lines omitted ...");
    output.push(&marker);
    output.extend(visible.iter().skip(visible.len() - tail_count).copied());

    let mut output = output.join("\n");
    if let Some(hint) = recovery_hint {
        output.push('\n');
        output.push_str(hint);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_subcommand_after_global_options() {
        let args = vec![
            "-L".to_string(),
            "agent".to_string(),
            "-fconfig.conf".to_string(),
            "list-sessions".to_string(),
        ];
        assert_eq!(find_subcommand(&args), Some((3, "list-sessions")));
    }

    #[test]
    fn compacts_standard_session_rows() {
        let raw = concat!(
            "build: 2 windows (created Sun Jul 19 10:00:00 2026) (attached)\n",
            "worker: 1 window (created Sun Jul 19 11:00:00 2026)\n"
        );
        assert_eq!(filter_list_sessions(raw), "build 2w attached\nworker 1w");
    }

    #[test]
    fn preserves_unknown_session_formats() {
        let raw = "build|2|attached\n";
        assert_eq!(filter_list_sessions(raw), raw.trim());
    }

    #[test]
    fn detects_custom_list_format() {
        assert!(has_custom_format(&[
            "-F".into(),
            "#{session_name}".into()
        ]));
        assert!(has_custom_format(&["--format=#{session_name}".into()]));
        assert!(!has_custom_format(&["-t".into(), "build".into()]));
    }

    #[test]
    fn capture_requires_print_flag() {
        assert!(capture_prints_stdout(&[
            "-ep".into(),
            "-t".into(),
            "build".into()
        ]));
        assert!(!capture_prints_stdout(&["-t".into(), "build".into()]));
    }

    #[test]
    fn capture_trims_padding_and_preserves_short_output() {
        assert_eq!(visible_capture_line_count("\n\nline 1\nline 2\n\n"), 2);
        assert_eq!(
            compact_capture_pane("\n\nline 1\nline 2\n\n", None),
            "line 1\nline 2"
        );
    }

    #[test]
    fn capture_caps_output_with_head_tail_and_recovery() {
        let raw = (1..=60)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let filtered = compact_capture_pane(&raw, Some("[full output: /tmp/tmux.log]"));

        assert!(filtered.starts_with("line 1\n"));
        assert!(filtered.contains("... 10 lines omitted ..."));
        assert!(filtered.contains("\nline 60\n"));
        assert!(filtered.ends_with("[full output: /tmp/tmux.log]"));
        assert!(!filtered.contains("line 30\n"));
    }
}
