//! Filter Registry — basic token reduction for `rtk run` native execution.
//!
//! This module provides **basic filtering (20-40% savings)** for commands
//! executed through rtk run. It is a **fallback** for commands
//! without dedicated RTK implementations.
//!
//! For **specialized filtering (60-90% savings)**, use dedicated modules:
//! - `src/git.rs` — git commands (diff, log, status, etc.)
//! - `src/runner.rs` — test commands (cargo test, pytest, etc.)
//! - `src/grep_cmd.rs` — code search (grep, ripgrep)
//! - `src/pnpm_cmd.rs` — package managers

use crate::stream::{FilterMode, LineFilter};
use crate::utils;

/// Filter cargo output: remove verbose "Compiling" lines
fn filter_cargo_output(output: &str) -> String {
    output
        .lines()
        .filter(|line| {
            let line = line.trim();
            !line.starts_with("Compiling ") || line.contains("error") || line.contains("warning")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Filter test output: remove passing tests, keep failures
fn filter_test_output(output: &str) -> String {
    output
        .lines()
        .filter(|line| {
            let line = line.trim();
            line.contains("FAILED")
                || line.contains("error")
                || line.contains("Error")
                || line.contains("failed")
                || line.contains("test result:")
                || line.starts_with("----")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Map a binary name to a [`FilterMode`] for use with [`crate::stream::run_streaming`].
///
/// Used by `spawn_with_filter` in exec.rs for external commands without dedicated modules.
pub fn get_filter_mode(binary: &str) -> FilterMode {
    match binary {
        // Streaming: per-line ANSI strip + line truncation (low savings, low overhead)
        "ls" | "find" | "grep" | "rg" | "fd" => {
            FilterMode::Streaming(Box::new(LineFilter::new(|l| {
                let stripped = utils::strip_ansi(l);
                let truncated = if stripped.len() > 120 {
                    format!("{}...", &stripped[..117])
                } else {
                    stripped
                };
                Some(format!("{}\n", truncated))
            })))
        }
        // Buffered: cargo, git, and test runners use simple filters here
        // (dedicated modules like cargo_cmd.rs / go_cmd.rs provide 60-90% savings)
        "cargo" => FilterMode::Buffered(filter_cargo_output),
        "pytest" | "jest" | "mocha" | "vitest" | "mypy" | "ruff" | "golangci-lint" => {
            FilterMode::Buffered(filter_test_output)
        }
        // git: ANSI strip per-line (dedicated git.rs handles git subcommands)
        "git" => FilterMode::Streaming(Box::new(LineFilter::new(|l| {
            Some(format!("{}\n", utils::strip_ansi(l)))
        }))),
        // npm/pnpm: ANSI strip (dedicated pnpm_cmd.rs handles specific subcommands)
        "npm" | "npx" | "pnpm" => FilterMode::Streaming(Box::new(LineFilter::new(|l| {
            Some(format!("{}\n", utils::strip_ansi(l)))
        }))),
        // Unknown commands: passthrough (no filtering, preserves all output)
        _ => FilterMode::Passthrough,
    }
}
