//! Compact `jar` archive listing with lossless passthrough for mutations.

use crate::core::runner::{self, RunOptions};
use crate::core::truncate::CAP_INVENTORY;
use crate::core::utils::resolved_command;
use anyhow::Result;
use std::ffi::OsString;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JarMode {
    List,
    Passthrough,
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if detect_mode(args) == JarMode::Passthrough {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        return runner::run_passthrough("jar", &args, verbose);
    }

    if verbose > 0 {
        eprintln!("Running: jar {}", args.join(" "));
    }

    let mut cmd = resolved_command("jar");
    cmd.args(args);
    runner::run_filtered(
        cmd,
        "jar",
        &args.join(" "),
        filter_listing,
        RunOptions::with_tee("jar").early_exit_on_failure(),
    )
}

fn detect_mode(args: &[String]) -> JarMode {
    let Some(mode) = args.first() else {
        return JarMode::Passthrough;
    };

    if mode == "--list" {
        return JarMode::List;
    }

    let short = mode.trim_start_matches('-');
    let operation_count = short
        .chars()
        .filter(|ch| matches!(ch, 'c' | 'i' | 't' | 'u' | 'x'))
        .count();
    if operation_count == 1 && short.contains('t') {
        JarMode::List
    } else {
        JarMode::Passthrough
    }
}

fn filter_listing(raw: &str) -> String {
    let entries: Vec<&str> = raw.lines().filter(|line| !line.trim().is_empty()).collect();
    if entries.len() <= CAP_INVENTORY {
        return raw.trim_end().to_string();
    }

    let mut output = entries[..CAP_INVENTORY].join("\n");
    output.push_str(&format!(
        "\n... +{} more entries (showing {} of {})",
        entries.len() - CAP_INVENTORY,
        CAP_INVENTORY,
        entries.len()
    ));
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_traditional_list_modes() {
        assert_eq!(detect_mode(&["tf".into(), "app.jar".into()]), JarMode::List);
        assert_eq!(
            detect_mode(&["-tvf".into(), "app.jar".into()]),
            JarMode::List
        );
        assert_eq!(
            detect_mode(&["--list".into(), "--file".into(), "app.jar".into()]),
            JarMode::List
        );
    }

    #[test]
    fn mutation_and_help_modes_are_passthrough() {
        for mode in ["xf", "-xf", "cf", "uf", "--extract", "--help"] {
            assert_eq!(
                detect_mode(&[mode.into(), "app.jar".into()]),
                JarMode::Passthrough,
                "mode {mode}"
            );
        }
    }

    #[test]
    fn short_listing_is_unchanged() {
        let raw = "META-INF/\nMETA-INF/MANIFEST.MF\ncom/example/App.class\n";
        assert_eq!(filter_listing(raw), raw.trim_end());
    }

    #[test]
    fn large_listing_uses_inventory_cap_and_summary() {
        let raw = (0..75)
            .map(|index| format!("com/example/Class{index}.class"))
            .collect::<Vec<_>>()
            .join("\n");
        let filtered = filter_listing(&raw);

        assert!(filtered.contains("com/example/Class49.class"));
        assert!(!filtered.contains("com/example/Class50.class"));
        assert!(filtered.ends_with("... +25 more entries (showing 50 of 75)"));
        assert_eq!(filtered.lines().count(), CAP_INVENTORY + 1);
    }
}
