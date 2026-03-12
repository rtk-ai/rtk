//! Command executor: runs simple chains natively, delegates complex shell to /bin/sh.

use anyhow::{Context, Result};
use std::process::Command;

use super::{analysis, builtins, filters, lexer};
use crate::stream::{FilterMode, LineFilter, StdinMode};
use crate::tracking;

/// Check if RTK is already active (recursion guard)
fn is_rtk_active() -> bool {
    std::env::var("RTK_ACTIVE").is_ok()
}

/// RAII guard: sets RTK_ACTIVE on creation, removes on drop (even on panic).
struct RtkActiveGuard;

impl RtkActiveGuard {
    fn new() -> Self {
        std::env::set_var("RTK_ACTIVE", "1");
        RtkActiveGuard
    }
}

impl Drop for RtkActiveGuard {
    fn drop(&mut self) {
        std::env::remove_var("RTK_ACTIVE");
    }
}

/// Execute a raw command string.
///
/// Returns the exit code: 0 = success, non-zero = failure.
pub fn execute(raw: &str, verbose: u8) -> Result<i32> {
    // Recursion guard
    if is_rtk_active() {
        if verbose > 0 {
            eprintln!("rtk: Recursion detected, passing through");
        }
        return run_passthrough(raw, verbose);
    }

    // Handle empty input
    if raw.trim().is_empty() {
        return Ok(0);
    }

    let _guard = RtkActiveGuard::new();
    execute_inner(raw, verbose)
}

fn execute_inner(raw: &str, verbose: u8) -> Result<i32> {
    // PR 2 adds: crate::config::rules::try_remap() alias expansion

    let tokens = lexer::tokenize(raw);

    // === STEP 1: Decide Native vs Passthrough ===
    if analysis::needs_shell(&tokens) {
        // PR 2 adds: safety::check_raw(raw) before passthrough
        return run_passthrough(raw, verbose);
    }

    // === STEP 2: Parse into native command chain ===
    let commands =
        analysis::parse_chain(tokens).map_err(|e| anyhow::anyhow!("Parse error: {}", e))?;

    // === STEP 3: Execute native chain ===
    run_native(&commands, verbose)
}

/// Run commands in native mode (iterate, check safety, filter output)
fn run_native(commands: &[analysis::NativeCommand], verbose: u8) -> Result<i32> {
    let mut last_exit: i32 = 0;
    let mut prev_operator: Option<&str> = None;

    for cmd in commands {
        // === SHORT-CIRCUIT LOGIC ===
        // Check if we should run based on PREVIOUS operator and result
        // The operator stored in cmd is the one AFTER it, so we use prev_operator
        if !analysis::should_run(prev_operator, last_exit == 0) {
            // For && with failure or || with success, skip this command
            prev_operator = cmd.operator.as_deref();
            continue;
        }

        // === RECURSION PREVENTION ===
        // Handle "rtk run" or "rtk" binary specially
        if cmd.binary == "rtk" && cmd.args.first().map(|s| s.as_str()) == Some("run") {
            // Flatten: execute the inner command directly
            // rtk run -c "git status" → args = ["run", "-c", "git status"]
            let inner = if cmd.args.get(1).map(|s| s.as_str()) == Some("-c") {
                cmd.args.get(2).cloned().unwrap_or_default()
            } else {
                cmd.args.get(1).cloned().unwrap_or_default()
            };
            if verbose > 0 {
                eprintln!("rtk: Flattening nested rtk run");
            }
            return execute(&inner, verbose);
        }
        // Other rtk commands: spawn as external (they have their own filters)

        // PR 2 adds: safety::check() dispatch block

        // === BUILTINS ===
        if builtins::is_builtin(&cmd.binary) {
            let ok = builtins::execute(&cmd.binary, &cmd.args)?;
            last_exit = if ok { 0 } else { 1 };
            prev_operator = cmd.operator.as_deref();
            continue;
        }

        // === EXTERNAL COMMAND WITH FILTERING ===
        last_exit = spawn_with_filter(&cmd.binary, &cmd.args, verbose)?;
        prev_operator = cmd.operator.as_deref();
    }

    Ok(last_exit)
}

/// Spawn external command and apply appropriate filter.
///
/// Returns the real exit code (0–254) or 128+N for signal-killed processes.
fn spawn_with_filter(binary: &str, args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 1 {
        eprintln!(
            "[rtk exec] binary={} interactive={} unstaged={}",
            binary,
            super::predicates::is_interactive(),
            super::predicates::has_unstaged_changes(),
        );
    }

    // Try to find the binary in PATH
    let binary_path = match which::which(binary) {
        Ok(path) => path,
        Err(_) => {
            eprintln!("rtk: {}: command not found", binary);
            return Ok(127); // standard "command not found" exit code
        }
    };

    let mut cmd = Command::new(&binary_path);
    cmd.args(args);

    let mode = filters::get_filter_mode(binary);
    let result = crate::stream::run_streaming(&mut cmd, StdinMode::Inherit, mode)
        .with_context(|| format!("Failed to execute: {}", binary))?;

    // Track usage with raw vs filtered for accurate savings
    let orig_cmd = if args.is_empty() {
        binary.to_string()
    } else {
        format!("{} {}", binary, args.join(" "))
    };

    // Check if this command could be routed - if so, track with the routed name
    // This ensures tracking accuracy even when commands are executed directly
    let rtk_cmd = if binary == "rtk" {
        // RTK calling itself - track as "rtk <subcommand>" not "rtk run rtk <subcommand>"
        if args.is_empty() {
            "rtk".to_string()
        } else {
            format!("rtk {}", args.join(" "))
        }
    } else {
        // Try to route the command to see if it has an RTK equivalent
        let native_cmd = analysis::NativeCommand {
            binary: binary.to_string(),
            args: args.to_vec(),
            operator: None,
        };
        match super::hook::try_route_native_command(&native_cmd, &orig_cmd) {
            Some(routed) => routed,
            None => format!("rtk run {}", orig_cmd),
        }
    };
    timer.track(&orig_cmd, &rtk_cmd, &result.raw, &result.filtered);

    Ok(result.exit_code)
}

/// Run command via system shell (passthrough mode — complex shell expressions).
///
/// Returns the real exit code propagated from the shell.
pub fn run_passthrough(raw: &str, verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("rtk: Passthrough mode for complex command");
    }

    let timer = tracking::TimedExecution::start();

    let shell = if cfg!(windows) { "cmd" } else { "sh" };
    let flag = if cfg!(windows) { "/C" } else { "-c" };

    let mut cmd = Command::new(shell);
    cmd.arg(flag).arg(raw);

    // Per-line ANSI strip while streaming — no full-buffer wait
    let filter = LineFilter::new(|l| Some(format!("{}\n", crate::utils::strip_ansi(l))));
    let result = crate::stream::run_streaming(
        &mut cmd,
        StdinMode::Inherit,
        FilterMode::Streaming(Box::new(filter)),
    )
    .context("Failed to execute passthrough")?;

    timer.track(
        raw,
        &format!("rtk passthrough {}", raw),
        &result.raw,
        &result.filtered,
    );

    Ok(result.exit_code)
}
