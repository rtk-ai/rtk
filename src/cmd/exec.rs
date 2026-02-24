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
fn spawn_with_filter(binary: &str, args: &[String], _verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

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
    let orig_cmd = format!("{} {}", binary, args.join(" "));

    // Check if this command could be routed - if so, track with the routed name
    // This ensures tracking accuracy even when commands are executed directly
    let rtk_cmd = if binary == "rtk" {
        // RTK calling itself - track as "rtk <subcommand>" not "rtk run rtk <subcommand>"
        format!("rtk {}", args.join(" "))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::hook;
    use crate::cmd::test_helpers::EnvGuard;

    // === RAII GUARD TESTS ===

    #[test]
    fn test_is_rtk_active_default() {
        let _env = EnvGuard::new();
        assert!(!is_rtk_active());
    }

    #[test]
    fn test_raii_guard_sets_and_clears() {
        let _env = EnvGuard::new();
        {
            let _guard = RtkActiveGuard::new();
            assert!(is_rtk_active());
        }
        assert!(
            !is_rtk_active(),
            "RTK_ACTIVE must be cleared when guard drops"
        );
    }

    #[test]
    fn test_raii_guard_clears_on_panic() {
        let _env = EnvGuard::new();
        let result = std::panic::catch_unwind(|| {
            let _guard = RtkActiveGuard::new();
            assert!(is_rtk_active());
            panic!("simulated panic");
        });
        assert!(result.is_err());
        assert!(
            !is_rtk_active(),
            "RTK_ACTIVE must be cleared even after panic"
        );
    }

    // === EXECUTE TESTS ===

    #[test]
    fn test_execute_empty() {
        assert_eq!(execute("", 0).unwrap(), 0);
    }

    #[test]
    fn test_execute_whitespace_only() {
        assert_eq!(execute("   ", 0).unwrap(), 0);
    }

    #[test]
    fn test_execute_simple_command() {
        assert_eq!(execute("echo hello", 0).unwrap(), 0);
    }

    #[test]
    fn test_execute_builtin_cd() {
        let original = std::env::current_dir().unwrap();
        assert_eq!(execute("cd /tmp", 0).unwrap(), 0);
        // On macOS, /tmp might be a symlink to /private/tmp
        let _ = std::env::set_current_dir(&original);
    }

    #[test]
    fn test_execute_builtin_pwd() {
        assert_eq!(execute("pwd", 0).unwrap(), 0);
    }

    #[test]
    fn test_execute_builtin_true() {
        assert_eq!(execute("true", 0).unwrap(), 0);
    }

    #[test]
    fn test_execute_builtin_false() {
        // `false` returns exit code 1
        assert_ne!(execute("false", 0).unwrap(), 0);
    }

    #[test]
    fn test_execute_chain_and_success() {
        assert_eq!(execute("true && echo success", 0).unwrap(), 0);
    }

    #[test]
    fn test_execute_chain_and_failure() {
        // Chain stops at false; exit code is non-zero
        assert_ne!(execute("false && echo should_not_run", 0).unwrap(), 0);
    }

    #[test]
    fn test_execute_chain_or_success() {
        // true succeeds; || skips second command; exit code 0
        assert_eq!(execute("true || echo should_not_run", 0).unwrap(), 0);
    }

    #[test]
    fn test_execute_chain_or_failure() {
        // false fails; || runs fallback (echo), which succeeds
        assert_eq!(execute("false || echo fallback", 0).unwrap(), 0);
    }

    #[test]
    fn test_execute_chain_semicolon() {
        // Both run; last result (false) is non-zero
        assert_ne!(execute("true ; false", 0).unwrap(), 0);
    }

    #[test]
    fn test_execute_passthrough_for_glob() {
        assert_eq!(execute("echo *", 0).unwrap(), 0);
    }

    #[test]
    fn test_execute_passthrough_for_pipe() {
        assert_eq!(execute("echo hello | cat", 0).unwrap(), 0);
    }

    #[test]
    fn test_execute_quoted_operator() {
        assert_eq!(execute(r#"echo "hello && world""#, 0).unwrap(), 0);
    }

    #[test]
    fn test_execute_binary_not_found() {
        // 127 = standard "command not found" exit code
        assert_eq!(execute("nonexistent_command_xyz_123", 0).unwrap(), 127);
    }

    #[test]
    fn test_execute_chain_and_three_commands() {
        // true && false && true: stops at false → non-zero
        assert_ne!(execute("true && false && true", 0).unwrap(), 0);
    }

    #[test]
    fn test_execute_chain_semicolon_last_wins() {
        // false ; true: last command succeeds → 0
        assert_eq!(execute("false ; true", 0).unwrap(), 0);
    }

    // === INTEGRATION TESTS (moved from edge_cases.rs) ===

    #[test]
    fn test_chain_mixed_operators() {
        // false || true && echo works → 0
        assert_eq!(execute("false || true && echo works", 0).unwrap(), 0);
    }

    #[test]
    fn test_passthrough_redirect() {
        assert_eq!(execute("echo test > /dev/null", 0).unwrap(), 0);
    }

    #[test]
    fn test_integration_cd_tilde() {
        let original = std::env::current_dir().unwrap();
        assert_eq!(execute("cd ~", 0).unwrap(), 0);
        let _ = std::env::set_current_dir(&original);
    }

    #[test]
    fn test_integration_export() {
        assert_eq!(execute("export TEST_VAR=value", 0).unwrap(), 0);
        std::env::remove_var("TEST_VAR");
    }

    #[test]
    fn test_integration_env_prefix() {
        let result = execute("TEST=1 echo hello", 0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_integration_dash_args() {
        assert_eq!(execute("echo --help -v --version", 0).unwrap(), 0);
    }

    #[test]
    fn test_integration_quoted_empty() {
        assert_eq!(execute(r#"echo """#, 0).unwrap(), 0);
    }

    // === RECURRENCE PREVENTION TESTS ===

    #[test]
    fn test_execute_rtk_recursion() {
        // This should flatten, not infinitely recurse
        let result = execute("rtk run \"echo hello\"", 0);
        assert!(result.is_ok());
    }

    // === EXIT CODE ACCURACY TESTS ===

    #[test]
    fn test_execute_returns_real_exit_code() {
        // sh -c "exit 42" must return 42, not 0 or 1
        let code = execute("sh -c \"exit 42\"", 0).unwrap();
        assert_eq!(code, 42, "exit code must be propagated exactly");
    }

    #[test]
    fn test_execute_success_returns_zero() {
        assert_eq!(execute("true", 0).unwrap(), 0);
    }

    #[test]
    fn test_run_native_and_chain_exit_code() {
        // "true && false" — last command fails, exit code non-zero
        assert_ne!(execute("true && false", 0).unwrap(), 0);
    }

    // === TRACKING LABEL TESTS ===
    // These tests verify that spawn_with_filter tracks commands with the correct
    // RTK command name, not always "rtk run <cmd>"

    /// Helper to test what rtk_cmd would be tracked for a given binary/args
    fn compute_rtk_cmd_label(binary: &str, args: &[&str]) -> String {
        let native_cmd = analysis::NativeCommand {
            binary: binary.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            operator: None,
        };
        let orig_cmd = if args.is_empty() {
            binary.to_string()
        } else {
            format!("{} {}", binary, args.join(" "))
        };

        if binary == "rtk" {
            format!("rtk {}", args.join(" "))
        } else {
            match hook::try_route_native_command(&native_cmd, &orig_cmd) {
                Some(routed) => routed,
                None => format!("rtk run {}", orig_cmd),
            }
        }
    }

    #[test]
    fn test_tracking_routed_command_uses_rtk_prefix() {
        // Commands that have an RTK equivalent should be tracked as "rtk <cmd>"
        // NOT as "rtk run <cmd>"
        let label = compute_rtk_cmd_label("ls", &["-F"]);
        assert!(
            label == "rtk ls -F",
            "Expected 'rtk ls -F', got '{}'",
            label
        );
    }

    #[test]
    fn test_tracking_git_status_uses_rtk_git() {
        // git status should be tracked as "rtk git status"
        let label = compute_rtk_cmd_label("git", &["status"]);
        assert!(
            label == "rtk git status",
            "Expected 'rtk git status', got '{}'",
            label
        );
    }

    #[test]
    fn test_tracking_cargo_test_uses_rtk_cargo() {
        // cargo test should be tracked as "rtk cargo test"
        let label = compute_rtk_cmd_label("cargo", &["test"]);
        assert!(
            label == "rtk cargo test",
            "Expected 'rtk cargo test', got '{}'",
            label
        );
    }

    #[test]
    fn test_tracking_unknown_command_uses_rtk_run() {
        // Commands without an RTK equivalent should be tracked as "rtk run <cmd>"
        let label = compute_rtk_cmd_label("python3", &["--version"]);
        assert!(
            label == "rtk run python3 --version",
            "Expected 'rtk run python3 --version', got '{}'",
            label
        );
    }

    #[test]
    fn test_tracking_rtk_self_reference_no_double_rtk() {
        // When RTK calls itself, track as "rtk <subcommand>" not "rtk run rtk <subcommand>"
        let label = compute_rtk_cmd_label("rtk", &["git", "status"]);
        assert!(
            label == "rtk git status",
            "Expected 'rtk git status', got '{}'",
            label
        );
        assert!(
            !label.contains("rtk run rtk"),
            "Should NOT contain 'rtk run rtk', got '{}'",
            label
        );
    }

    #[test]
    fn test_tracking_find_uses_rtk_run() {
        // NOTE: find is NOT in the ROUTES lookup table, so it falls through to rtk run
        // This is intentional - find commands often have complex arguments that need shell
        let label = compute_rtk_cmd_label("find", &[".", "-name", "*.rs"]);
        assert!(
            label.starts_with("rtk run"),
            "Expected 'rtk run ...' (find not in ROUTES), got '{}'",
            label
        );
    }

    #[test]
    fn test_tracking_grep_uses_rtk_grep() {
        // grep should be tracked as "rtk grep"
        let label = compute_rtk_cmd_label("grep", &["-r", "pattern"]);
        assert!(
            label.starts_with("rtk grep"),
            "Expected 'rtk grep ...', got '{}'",
            label
        );
    }
}
