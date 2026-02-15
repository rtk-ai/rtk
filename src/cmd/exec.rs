//! Command executor: runs simple chains natively, delegates complex shell to /bin/sh.

use anyhow::{Context, Result};
use std::process::{Command, Stdio};

use super::{analysis, builtins, filters, lexer, safety, trash_cmd};
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

/// Execute a raw command string
pub fn execute(raw: &str, verbose: u8) -> Result<bool> {
    // Recursion guard
    if is_rtk_active() {
        if verbose > 0 {
            eprintln!("rtk: Recursion detected, passing through");
        }
        return run_passthrough(raw, verbose);
    }

    // Handle empty input
    if raw.trim().is_empty() {
        return Ok(true);
    }

    let _guard = RtkActiveGuard::new();
    execute_inner(raw, verbose)
}

fn execute_inner(raw: &str, verbose: u8) -> Result<bool> {
    // === STEP 0: Remap expansion (aliases like "t" → "cargo test") ===
    if let Some(expanded) = crate::config::rules::try_remap(raw) {
        if verbose > 0 {
            eprintln!(
                "rtk remap: {} → {}",
                raw.split_whitespace().next().unwrap_or(raw),
                expanded
            );
        }
        return execute_inner(&expanded, verbose);
    }

    let tokens = lexer::tokenize(raw);

    // === STEP 1: Decide Native vs Passthrough ===
    if analysis::needs_shell(&tokens) {
        // Even in passthrough, check safety on raw string
        if let safety::SafetyResult::Blocked(msg) = safety::check_raw(raw) {
            eprintln!("{}", msg);
            return Ok(false);
        }
        return run_passthrough(raw, verbose);
    }

    // === STEP 2: Parse into native command chain ===
    let commands =
        analysis::parse_chain(tokens).map_err(|e| anyhow::anyhow!("Parse error: {}", e))?;

    // === STEP 3: Execute native chain ===
    run_native(&commands, verbose)
}

/// Run commands in native mode (iterate, check safety, filter output)
fn run_native(commands: &[analysis::NativeCommand], verbose: u8) -> Result<bool> {
    let mut last_success = true;
    let mut prev_operator: Option<&str> = None;

    for cmd in commands {
        // === SHORT-CIRCUIT LOGIC ===
        // Check if we should run based on PREVIOUS operator and result
        // The operator stored in cmd is the one AFTER it, so we use prev_operator
        if !analysis::should_run(prev_operator, last_success) {
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

        // === SAFETY CHECK ===
        match safety::check(&cmd.binary, &cmd.args) {
            safety::SafetyResult::Blocked(msg) => {
                eprintln!("{}", msg);
                return Ok(false);
            }
            safety::SafetyResult::Rewritten(new_cmd) => {
                // Re-execute the rewritten command
                if verbose > 0 {
                    eprintln!("rtk safety: Rewrote command");
                }
                return execute(&new_cmd, verbose);
            }
            safety::SafetyResult::TrashRequested(paths) => {
                last_success = trash_cmd::execute(&paths)?;
                prev_operator = cmd.operator.as_deref();
                continue;
            }
            safety::SafetyResult::Safe => {}
        }

        // === BUILTINS ===
        if builtins::is_builtin(&cmd.binary) {
            last_success = builtins::execute(&cmd.binary, &cmd.args)?;
            prev_operator = cmd.operator.as_deref();
            continue;
        }

        // === EXTERNAL COMMAND WITH FILTERING ===
        last_success = spawn_with_filter(&cmd.binary, &cmd.args, verbose)?;
        prev_operator = cmd.operator.as_deref();
    }

    Ok(last_success)
}

/// Spawn external command and apply appropriate filter
fn spawn_with_filter(binary: &str, args: &[String], _verbose: u8) -> Result<bool> {
    let timer = tracking::TimedExecution::start();

    // Try to find the binary in PATH
    let binary_path = match which::which(binary) {
        Ok(path) => path,
        Err(_) => {
            // Binary not found
            eprintln!("rtk: {}: command not found", binary);
            return Ok(false);
        }
    };

    // Use wait_with_output() to avoid deadlock when child output exceeds
    // pipe buffer (~64KB Linux, ~16KB macOS). This reads stdout/stderr in
    // separate threads internally before calling wait().
    let output = Command::new(&binary_path)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("Failed to execute: {}", binary))?;

    let raw_out = String::from_utf8_lossy(&output.stdout);
    let raw_err = String::from_utf8_lossy(&output.stderr);

    // Determine filter type and apply
    let filter_type = filters::get_filter_type(binary);
    let filtered_out = filters::apply_to_string(filter_type, &raw_out);
    let filtered_err = crate::utils::strip_ansi(&raw_err);

    // Print filtered output
    print!("{}", filtered_out);
    eprint!("{}", filtered_err);

    // Track usage with raw vs filtered for accurate savings
    let raw_output = format!("{}{}", raw_out, raw_err);
    let filtered_output = format!("{}{}", filtered_out, filtered_err);
    timer.track(
        &format!("{} {}", binary, args.join(" ")),
        &format!("rtk run {} {}", binary, args.join(" ")),
        &raw_output,
        &filtered_output,
    );

    Ok(output.status.success())
}

/// Run command via system shell (passthrough mode)
pub fn run_passthrough(raw: &str, verbose: u8) -> Result<bool> {
    if verbose > 0 {
        eprintln!("rtk: Passthrough mode for complex command");
    }

    let timer = tracking::TimedExecution::start();

    let shell = if cfg!(windows) { "cmd" } else { "sh" };
    let flag = if cfg!(windows) { "/C" } else { "-c" };

    let output = Command::new(shell)
        .arg(flag)
        .arg(raw)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("Failed to execute passthrough")?;

    let raw_out = String::from_utf8_lossy(&output.stdout);
    let raw_err = String::from_utf8_lossy(&output.stderr);

    // Basic filtering even in passthrough (strip ANSI)
    let filtered_out = crate::utils::strip_ansi(&raw_out);
    let filtered_err = crate::utils::strip_ansi(&raw_err);
    print!("{}", filtered_out);
    eprint!("{}", filtered_err);

    let raw_output = format!("{}{}", raw_out, raw_err);
    let filtered_output = format!("{}{}", filtered_out, filtered_err);
    timer.track(
        raw,
        &format!("rtk passthrough {}", raw),
        &raw_output,
        &filtered_output,
    );

    Ok(output.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let result = execute("", 0).unwrap();
        assert!(result);
    }

    #[test]
    fn test_execute_whitespace_only() {
        let result = execute("   ", 0).unwrap();
        assert!(result);
    }

    #[test]
    fn test_execute_simple_command() {
        let result = execute("echo hello", 0).unwrap();
        assert!(result);
    }

    #[test]
    fn test_execute_builtin_cd() {
        let original = std::env::current_dir().unwrap();
        let result = execute("cd /tmp", 0).unwrap();
        assert!(result);
        // On macOS, /tmp might be a symlink to /private/tmp
        // Just verify the command succeeded (the cd happened)
        let _ = std::env::set_current_dir(&original);
    }

    #[test]
    fn test_execute_builtin_pwd() {
        let result = execute("pwd", 0).unwrap();
        assert!(result);
    }

    #[test]
    fn test_execute_builtin_true() {
        let result = execute("true", 0).unwrap();
        assert!(result);
    }

    #[test]
    fn test_execute_builtin_false() {
        let result = execute("false", 0).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_execute_chain_and_success() {
        let result = execute("true && echo success", 0).unwrap();
        assert!(result);
    }

    #[test]
    fn test_execute_chain_and_failure() {
        let result = execute("false && echo should_not_run", 0).unwrap();
        // Chain stops at false, so result is false
        assert!(!result);
    }

    #[test]
    fn test_execute_chain_or_success() {
        let result = execute("true || echo should_not_run", 0).unwrap();
        // true succeeds, || doesn't run second command
        assert!(result);
    }

    #[test]
    fn test_execute_chain_or_failure() {
        let result = execute("false || echo fallback", 0).unwrap();
        // false fails, || runs fallback
        assert!(result);
    }

    #[test]
    fn test_execute_chain_semicolon() {
        let result = execute("true ; false", 0).unwrap();
        // Both run, last result is false
        assert!(!result);
    }

    #[test]
    fn test_execute_passthrough_for_glob() {
        let result = execute("echo *", 0).unwrap();
        // Should work via passthrough
        assert!(result);
    }

    #[test]
    fn test_execute_passthrough_for_pipe() {
        let result = execute("echo hello | cat", 0).unwrap();
        assert!(result);
    }

    #[test]
    fn test_execute_quoted_operator() {
        let result = execute(r#"echo "hello && world""#, 0).unwrap();
        assert!(result);
    }

    #[test]
    fn test_execute_binary_not_found() {
        let result = execute("nonexistent_command_xyz_123", 0).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_execute_chain_and_three_commands() {
        // 3-command chain: true succeeds, false fails, stops before third
        let result = execute("true && false && true", 0).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_execute_chain_semicolon_last_wins() {
        // Semicolon runs all; last result (true) determines outcome
        let result = execute("false ; true", 0).unwrap();
        assert!(result);
    }

    // === INTEGRATION TESTS (moved from edge_cases.rs) ===

    #[test]
    fn test_chain_mixed_operators() {
        // false -> || runs true -> true && runs echo
        let result = execute("false || true && echo works", 0).unwrap();
        assert!(result);
    }

    #[test]
    fn test_passthrough_redirect() {
        let result = execute("echo test > /dev/null", 0).unwrap();
        assert!(result);
    }

    #[test]
    fn test_integration_cd_tilde() {
        let original = std::env::current_dir().unwrap();
        let result = execute("cd ~", 0).unwrap();
        assert!(result);
        let _ = std::env::set_current_dir(&original);
    }

    #[test]
    fn test_integration_export() {
        let result = execute("export TEST_VAR=value", 0).unwrap();
        assert!(result);
        std::env::remove_var("TEST_VAR");
    }

    #[test]
    fn test_integration_env_prefix() {
        let result = execute("TEST=1 echo hello", 0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_integration_dash_args() {
        let result = execute("echo --help -v --version", 0).unwrap();
        assert!(result);
    }

    #[test]
    fn test_integration_quoted_empty() {
        let result = execute(r#"echo """#, 0).unwrap();
        assert!(result);
    }

    // === RECURRENCE PREVENTION TESTS ===

    #[test]
    fn test_execute_rtk_recursion() {
        // This should flatten, not infinitely recurse
        let result = execute("rtk run \"echo hello\"", 0);
        assert!(result.is_ok());
    }
}
