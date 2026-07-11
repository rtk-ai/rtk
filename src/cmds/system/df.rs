//! Disk free-space summary with Windows-native compact output.

use anyhow::Result;
#[cfg(not(target_os = "windows"))]
use anyhow::Context;
#[cfg(not(target_os = "windows"))]
use crate::core::utils::{exit_code_from_status, resolved_command};

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    #[cfg(target_os = "windows")]
    {
        run_native(args, verbose)
    }

    #[cfg(not(target_os = "windows"))]
    {
        run_external(args, verbose)
    }
}

#[cfg(not(target_os = "windows"))]
fn run_external(args: &[String], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("Running: df {}", args.join(" "));
    }
    let status = resolved_command("df").args(args).status().context("Failed to run df")?;
    Ok(exit_code_from_status(&status, "df"))
}

#[cfg(target_os = "windows")]
fn run_native(args: &[String], verbose: u8) -> Result<i32> {
    if args.iter().any(|a| a == "--help") {
        print_help();
        return Ok(0);
    }

    let human = match args {
        [] => false,
        [flag] if flag == "-h" || flag == "--human-readable" => true,
        _ => {
            eprintln!(
                "rtk df: unsupported arguments '{}'; use rtk proxy df ... for native df semantics",
                args.join(" ")
            );
            return Ok(2);
        }
    };

    if verbose > 0 {
        eprintln!("Running native df {}", args.join(" "));
    }

    print!("{}", native_df_output(human));
    Ok(0)
}

#[cfg(target_os = "windows")]
fn native_df_output(human: bool) -> String {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut rows = Vec::new();

    for disk in disks.list() {
        let total = disk.total_space();
        let available = disk.available_space();
        let used = total.saturating_sub(available);
        let mount = disk.mount_point().display().to_string();
        rows.push(DfRow {
            mount,
            total,
            used,
            available,
        });
    }

    format_df_rows(rows, human)
}

#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct DfRow {
    mount: String,
    total: u64,
    used: u64,
    available: u64,
}

#[cfg(any(target_os = "windows", test))]
fn format_df_rows(mut rows: Vec<DfRow>, human: bool) -> String {
    rows.sort_by_key(|row| row.mount.to_lowercase());

    let mut output = String::from("Filesystem Size Used Avail Use%\n");
    for row in rows {
        let use_percent = row
            .used
            .saturating_mul(100)
            .checked_div(row.total)
            .map(|percent| format!("{percent}%"))
            .unwrap_or_else(|| "-".to_string());
        let size = format_size(row.total, human);
        let used = format_size(row.used, human);
        let available = format_size(row.available, human);
        output.push_str(&format!(
            "{} {size} {used} {available} {use_percent}\n",
            row.mount
        ));
    }
    output
}

#[cfg(target_os = "windows")]
fn print_help() {
    println!(
        "Disk free space summary with compact output (native Windows)\n\n\
Usage: rtk df [OPTIONS]\n\n\
Options:\n  -h, --human-readable  compact sizes such as 1.2G\n      --help            print help\n\n\
Unsupported: inode/type/filter flags. Use `rtk proxy df ...` for native df semantics."
    );
}

#[cfg(any(target_os = "windows", test))]
fn format_size(bytes: u64, human: bool) -> String {
    if !human {
        return bytes.to_string();
    }

    compact_size(bytes)
}

#[cfg(any(target_os = "windows", test))]
fn compact_size(bytes: u64) -> String {
    const K: f64 = 1024.0;
    const M: f64 = K * 1024.0;
    const G: f64 = M * 1024.0;
    const T: f64 = G * 1024.0;

    let bytes_f = bytes as f64;
    if bytes_f >= T {
        format!("{:.1}T", bytes_f / T)
    } else if bytes_f >= G {
        format!("{:.1}G", bytes_f / G)
    } else if bytes_f >= M {
        format!("{:.1}M", bytes_f / M)
    } else if bytes_f >= K {
        format!("{:.1}K", bytes_f / K)
    } else {
        format!("{bytes}B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compact_size_style() {
        assert_eq!(compact_size(978), "978B");
        assert_eq!(compact_size(1234), "1.2K");
        assert_eq!(compact_size(1_234_567_890), "1.1G");
    }

    #[test]
    fn test_format_df_rows_sorts_and_formats_usage() {
        let output = format_df_rows(
            vec![
                DfRow {
                    mount: "Z:\\".to_string(),
                    total: 0,
                    used: 0,
                    available: 0,
                },
                DfRow {
                    mount: "C:\\".to_string(),
                    total: 1024,
                    used: 256,
                    available: 768,
                },
            ],
            true,
        );

        assert_eq!(
            output,
            "Filesystem Size Used Avail Use%\nC:\\ 1.0K 256B 768B 25%\nZ:\\ 0B 0B 0B -\n"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn external_df_spawn_has_actionable_context() {
        let source = include_str!("df.rs");
        assert!(source.contains(".status().context(\"Failed to run df\")?"));
    }
}
