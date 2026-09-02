use anyhow::Result;
use serde_json::Value;
use std::ffi::OsString;

use crate::core::runner;
use crate::core::stream::StreamFilter;
use crate::core::utils::{human_bytes, resolved_command, strip_ansi};

#[derive(Debug)]
struct ProcessSummary {
    name: String,
    status: String,
    cpu: String,
    memory: String,
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    match args.first().map(String::as_str) {
        Some("list" | "ls" | "status") => run_list(args, verbose),
        Some("logs") => run_logs(args, verbose),
        _ => run_passthrough(args, verbose),
    }
}

fn run_list(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("pm2");

    if args.len() == 1 {
        // jlist avoids parsing PM2's version-dependent box-drawing table and
        // lets us omit the large environment payload safely.
        cmd.arg("jlist");
        if verbose > 0 {
            eprintln!("Running: pm2 jlist (compact form of pm2 {})", args[0]);
        }
        runner::run_filtered(
            cmd,
            "pm2",
            &args.join(" "),
            filter_pm2_list,
            runner::RunOptions::stdout_only().tee("pm2 list"),
        )
    } else {
        cmd.args(args);
        if verbose > 0 {
            eprintln!("Running: pm2 {}", args.join(" "));
        }
        runner::run_filtered(
            cmd,
            "pm2",
            &args.join(" "),
            strip_ansi,
            runner::RunOptions::default(),
        )
    }
}

fn run_logs(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("pm2");
    cmd.args(args);
    if verbose > 0 {
        eprintln!("Running: pm2 {}", args.join(" "));
    }

    runner::run_streamed(
        cmd,
        "pm2",
        &args.join(" "),
        Box::new(AnsiStripFilter),
        runner::RunOptions::with_tee("pm2 logs"),
    )
}

fn run_passthrough(args: &[String], verbose: u8) -> Result<i32> {
    let args = args.iter().map(OsString::from).collect::<Vec<_>>();
    runner::run_passthrough("pm2", &args, verbose)
}

fn filter_pm2_list(raw: &str) -> String {
    let clean = strip_ansi(raw);
    let Some(processes) = extract_processes(&clean) else {
        return clean;
    };

    if processes.is_empty() {
        return "PM2: no processes\n".to_string();
    }

    let rows = processes
        .iter()
        .map(|process| ProcessSummary {
            name: process
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("(unnamed)")
                .to_string(),
            status: process
                .pointer("/pm2_env/status")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            cpu: process
                .pointer("/monit/cpu")
                .and_then(Value::as_f64)
                .map(format_cpu)
                .unwrap_or_else(|| "?".to_string()),
            memory: process
                .pointer("/monit/memory")
                .and_then(Value::as_u64)
                .map(human_bytes)
                .unwrap_or_else(|| "?".to_string()),
        })
        .collect::<Vec<_>>();

    format_process_table(&rows)
}

fn extract_processes(output: &str) -> Option<Vec<Value>> {
    output.match_indices('[').find_map(|(start, _)| {
        let mut values = serde_json::Deserializer::from_str(&output[start..]).into_iter::<Vec<Value>>();
        let processes = values.next()?.ok()?;
        if processes
            .iter()
            .all(|process| process.get("name").is_some() && process.get("pm2_env").is_some())
        {
            Some(processes)
        } else {
            None
        }
    })
}

fn format_cpu(cpu: f64) -> String {
    if cpu.fract() == 0.0 {
        format!("{cpu:.0}%")
    } else {
        format!("{cpu:.1}%")
    }
}

fn format_process_table(rows: &[ProcessSummary]) -> String {
    let name_width = rows
        .iter()
        .map(|row| row.name.chars().count())
        .max()
        .unwrap_or(4)
        .max(4);
    let status_width = rows
        .iter()
        .map(|row| row.status.chars().count())
        .max()
        .unwrap_or(6)
        .max(6);
    let cpu_width = rows.iter().map(|row| row.cpu.len()).max().unwrap_or(3).max(3);

    let mut output = format!(
        "{:<name_width$}  {:<status_width$}  {:>cpu_width$}  MEM\n",
        "NAME", "STATUS", "CPU"
    );
    for row in rows {
        output.push_str(&format!(
            "{:<name_width$}  {:<status_width$}  {:>cpu_width$}  {}\n",
            row.name, row.status, row.cpu, row.memory
        ));
    }
    output
}

struct AnsiStripFilter;

impl StreamFilter for AnsiStripFilter {
    fn feed_line(&mut self, line: &str) -> Option<String> {
        Some(format!("{}\n", strip_ansi(line)))
    }

    fn flush(&mut self) -> String {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_compacts_real_sanitized_jlist_output() {
        let raw = include_str!("../../../tests/fixtures/pm2_jlist.json");
        let output = filter_pm2_list(raw);

        assert_eq!(
            output,
            "NAME         STATUS    CPU  MEM\n\
             rtk-fixture  online     0%  1.1 MB\n\
             api-worker   stopped  2.5%  64.0 MB\n"
        );
        assert!(!output.contains("SECRET_TOKEN"));

        let savings = 100.0 - (output.len() as f64 / raw.len() as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "PM2 list filter: expected at least 60% savings, got {savings:.1}%"
        );
    }

    #[test]
    fn list_reports_empty_process_set() {
        assert_eq!(filter_pm2_list("[]"), "PM2: no processes\n");
    }

    #[test]
    fn list_extracts_json_after_cold_daemon_preamble() {
        let raw = "[PM2] Spawning PM2 daemon\n[PM2] PM2 Successfully daemonized\n[]\n";
        assert_eq!(filter_pm2_list(raw), "PM2: no processes\n");
    }

    #[test]
    fn list_preserves_non_json_errors() {
        assert_eq!(
            filter_pm2_list("\u{1b}[31m[PM2][ERROR] daemon unavailable\u{1b}[0m\n"),
            "[PM2][ERROR] daemon unavailable\n"
        );
    }

    #[test]
    fn logs_strip_ansi_without_buffering() {
        let mut filter = AnsiStripFilter;
        assert_eq!(
            filter.feed_line("\u{1b}[32m0|api | 2026-07-20 ready\u{1b}[0m"),
            Some("0|api | 2026-07-20 ready\n".to_string())
        );
    }
}
