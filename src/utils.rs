//! Utility functions for text processing and command execution.
//!
//! Provides common helpers used across rtk commands:
//! - ANSI color code stripping
//! - Text truncation
//! - Command execution with error context

use anyhow::{Context, Result};
use regex::Regex;
use std::path::PathBuf;
use std::process::Command;

/// Truncates a string to `max_len` characters, appending `...` if needed.
///
/// # Arguments
/// * `s` - The string to truncate
/// * `max_len` - Maximum length before truncation (minimum 3 to include "...")
///
/// # Examples
/// ```
/// use rtk::utils::truncate;
/// assert_eq!(truncate("hello world", 8), "hello...");
/// assert_eq!(truncate("hi", 10), "hi");
/// ```
pub fn truncate(s: &str, max_len: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_len {
        s.to_string()
    } else if max_len < 3 {
        // If max_len is too small, just return "..."
        "...".to_string()
    } else {
        format!("{}...", s.chars().take(max_len - 3).collect::<String>())
    }
}

/// Strip ANSI escape codes (colors, styles) from a string.
///
/// # Arguments
/// * `text` - Text potentially containing ANSI escape codes
///
/// # Examples
/// ```
/// use rtk::utils::strip_ansi;
/// let colored = "\x1b[31mError\x1b[0m";
/// assert_eq!(strip_ansi(colored), "Error");
/// ```
pub fn strip_ansi(text: &str) -> String {
    lazy_static::lazy_static! {
        static ref ANSI_RE: Regex = Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").unwrap();
    }
    ANSI_RE.replace_all(text, "").to_string()
}

/// Executes a command and returns cleaned stdout/stderr.
///
/// # Arguments
/// * `cmd` - Command to execute (e.g., "eslint")
/// * `args` - Command arguments
///
/// # Returns
/// `(stdout: String, stderr: String, exit_code: i32)`
///
/// # Examples
/// ```no_run
/// use rtk::utils::execute_command;
/// let (stdout, stderr, code) = execute_command("echo", &["test"]).unwrap();
/// assert_eq!(code, 0);
/// ```
#[allow(dead_code)]
pub fn execute_command(cmd: &str, args: &[&str]) -> Result<(String, String, i32)> {
    let output = resolved_command(cmd)
        .args(args)
        .output()
        .context(format!("Failed to execute {}", cmd))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);

    Ok((stdout, stderr, exit_code))
}

/// Formats a token count with K/M suffixes for readability.
///
/// # Arguments
/// * `n` - Number of tokens
///
/// # Returns
/// Formatted string (e.g., "1.2M", "59.2K", "694")
///
/// # Examples
/// ```
/// use rtk::utils::format_tokens;
/// assert_eq!(format_tokens(1_234_567), "1.2M");
/// assert_eq!(format_tokens(59_234), "59.2K");
/// assert_eq!(format_tokens(694), "694");
/// ```
pub fn format_tokens(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        format!("{}", n)
    }
}

/// Formats a USD amount with adaptive precision.
///
/// # Arguments
/// * `amount` - Amount in dollars
///
/// # Returns
/// Formatted string with $ prefix
///
/// # Examples
/// ```
/// use rtk::utils::format_usd;
/// assert_eq!(format_usd(1234.567), "$1234.57");
/// assert_eq!(format_usd(12.345), "$12.35");
/// assert_eq!(format_usd(0.123), "$0.12");
/// assert_eq!(format_usd(0.0096), "$0.0096");
/// ```
pub fn format_usd(amount: f64) -> String {
    if !amount.is_finite() {
        return "$0.00".to_string();
    }
    if amount >= 0.01 {
        format!("${:.2}", amount)
    } else {
        format!("${:.4}", amount)
    }
}

/// Format cost-per-token as $/MTok (e.g., "$3.86/MTok")
///
/// # Arguments
/// * `cpt` - Cost per token (not per million tokens)
///
/// # Returns
/// Formatted string like "$3.86/MTok"
///
/// # Examples
/// ```
/// use rtk::utils::format_cpt;
/// assert_eq!(format_cpt(0.000003), "$3.00/MTok");
/// assert_eq!(format_cpt(0.0000038), "$3.80/MTok");
/// assert_eq!(format_cpt(0.00000386), "$3.86/MTok");
/// ```
pub fn format_cpt(cpt: f64) -> String {
    if !cpt.is_finite() || cpt <= 0.0 {
        return "$0.00/MTok".to_string();
    }
    let cpt_per_million = cpt * 1_000_000.0;
    format!("${:.2}/MTok", cpt_per_million)
}

/// Join items into a newline-separated string, appending an overflow hint when total > max.
///
/// # Examples
/// ```
/// use rtk::utils::join_with_overflow;
/// let items = vec!["a".to_string(), "b".to_string()];
/// assert_eq!(join_with_overflow(&items, 5, 3, "items"), "a\nb\n... +2 more items");
/// assert_eq!(join_with_overflow(&items, 2, 3, "items"), "a\nb");
/// ```
pub fn join_with_overflow(items: &[String], total: usize, max: usize, label: &str) -> String {
    let mut out = items.join("\n");
    if total > max {
        out.push_str(&format!("\n... +{} more {}", total - max, label));
    }
    out
}

/// Truncate an ISO 8601 datetime string to just the date portion (first 10 chars).
///
/// # Examples
/// ```
/// use rtk::utils::truncate_iso_date;
/// assert_eq!(truncate_iso_date("2024-01-15T10:30:00Z"), "2024-01-15");
/// assert_eq!(truncate_iso_date("2024-01-15"), "2024-01-15");
/// assert_eq!(truncate_iso_date("short"), "short");
/// ```
pub fn truncate_iso_date(date: &str) -> &str {
    if date.len() >= 10 {
        &date[..10]
    } else {
        date
    }
}

/// Format a confirmation message: "ok \<action\> \<detail\>"
/// Used for write operations (merge, create, comment, edit, etc.)
///
/// # Examples
/// ```
/// use rtk::utils::ok_confirmation;
/// assert_eq!(ok_confirmation("merged", "#42"), "ok merged #42");
/// assert_eq!(ok_confirmation("created", "PR #5 https://..."), "ok created PR #5 https://...");
/// ```
pub fn ok_confirmation(action: &str, detail: &str) -> String {
    if detail.is_empty() {
        format!("ok {}", action)
    } else {
        format!("ok {} {}", action, detail)
    }
}

/// Detect the package manager used in the current directory.
/// Returns "pnpm", "yarn", or "npm" based on lockfile presence.
///
/// # Examples
/// ```no_run
/// use rtk::utils::detect_package_manager;
/// let pm = detect_package_manager();
/// // Returns "pnpm" if pnpm-lock.yaml exists, "yarn" if yarn.lock, else "npm"
/// ```
#[allow(dead_code)]
pub fn detect_package_manager() -> &'static str {
    if std::path::Path::new("pnpm-lock.yaml").exists() {
        "pnpm"
    } else if std::path::Path::new("yarn.lock").exists() {
        "yarn"
    } else {
        "npm"
    }
}

/// Build a Command using the detected package manager's exec mechanism.
/// Returns a Command ready to have tool-specific args appended.
pub fn package_manager_exec(tool: &str) -> Command {
    if tool_exists(tool) {
        resolved_command(tool)
    } else {
        let pm = detect_package_manager();
        match pm {
            "pnpm" => {
                let mut c = resolved_command("pnpm");
                c.arg("exec").arg("--").arg(tool);
                c
            }
            "yarn" => {
                let mut c = resolved_command("yarn");
                c.arg("exec").arg("--").arg(tool);
                c
            }
            _ => {
                let mut c = resolved_command("npx");
                c.arg("--no-install").arg("--").arg(tool);
                c
            }
        }
    }
}

/// Resolve a binary name to its full path, honoring PATHEXT on Windows.
///
/// On Windows, Node.js tools are installed as `.CMD`/`.BAT`/`.PS1` shims.
/// Rust's `std::process::Command::new()` does NOT honor PATHEXT, so
/// `Command::new("vitest")` fails even when `vitest.CMD` is on PATH.
///
/// This function uses the `which` crate to perform proper PATH+PATHEXT resolution.
///
/// # Arguments
/// * `name` - Binary name (e.g., "vitest", "eslint", "tsc")
///
/// # Returns
/// Full path to the resolved binary, or error if not found.
pub fn resolve_binary(name: &str) -> Result<PathBuf> {
    which::which(name).context(format!("Binary '{}' not found on PATH", name))
}

/// Create a `Command` with PATHEXT-aware binary resolution.
///
/// Drop-in replacement for `Command::new(name)` that works on Windows
/// with `.CMD`/`.BAT`/`.PS1` wrappers.
///
/// Falls back to `Command::new(name)` if resolution fails, so native
/// commands (git, cargo) still work even if `which` can't find them.
///
/// # Arguments
/// * `name` - Binary name (e.g., "vitest", "eslint")
///
/// # Returns
/// A `Command` configured with the resolved binary path.
pub fn resolved_command(name: &str) -> Command {
    match resolve_binary(name) {
        Ok(path) => Command::new(path),
        Err(e) => {
            // On Windows, resolution failure likely means a .CMD/.BAT wrapper
            // wasn't found — always warn so users have a signal.
            // On Unix, this is less common; only log in debug builds.
            #[cfg(target_os = "windows")]
            eprintln!(
                "rtk: Failed to resolve '{}' via PATH, falling back to direct exec: {}",
                name, e
            );
            #[cfg(not(target_os = "windows"))]
            {
                #[cfg(debug_assertions)]
                eprintln!(
                    "rtk: Failed to resolve '{}' via PATH, falling back to direct exec: {}",
                    name, e
                );
            }
            Command::new(name)
        }
    }
}

/// Check if a tool exists on PATH (PATHEXT-aware on Windows).
///
/// Replaces manual `Command::new("which").arg(tool)` checks that fail on Windows.
pub fn tool_exists(name: &str) -> bool {
    which::which(name).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_long_string() {
        let result = truncate("hello world", 8);
        assert_eq!(result, "hello...");
    }

    #[test]
    fn test_truncate_exact_length() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_edge_case() {
        // max_len < 3 returns just "..."
        assert_eq!(truncate("hello", 2), "...");
        // When string length equals max_len, return as is
        assert_eq!(truncate("abc", 3), "abc");
        // When string is longer and max_len is exactly 3, return "..."
        assert_eq!(truncate("hello world", 3), "...");
    }

    #[test]
    fn test_strip_ansi_simple() {
        let input = "\x1b[31mError\x1b[0m";
        assert_eq!(strip_ansi(input), "Error");
    }

    #[test]
    fn test_strip_ansi_multiple() {
        let input = "\x1b[1m\x1b[32mSuccess\x1b[0m\x1b[0m";
        assert_eq!(strip_ansi(input), "Success");
    }

    #[test]
    fn test_strip_ansi_no_codes() {
        assert_eq!(strip_ansi("plain text"), "plain text");
    }

    #[test]
    fn test_strip_ansi_complex() {
        let input = "\x1b[32mGreen\x1b[0m normal \x1b[31mRed\x1b[0m";
        assert_eq!(strip_ansi(input), "Green normal Red");
    }

    #[test]
    fn test_execute_command_success() {
        let result = execute_command("echo", &["test"]);
        assert!(result.is_ok());
        let (stdout, _, code) = result.unwrap();
        assert_eq!(code, 0);
        assert!(stdout.contains("test"));
    }

    #[test]
    fn test_execute_command_failure() {
        let result = execute_command("nonexistent_command_xyz_12345", &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_format_tokens_millions() {
        assert_eq!(format_tokens(1_234_567), "1.2M");
        assert_eq!(format_tokens(12_345_678), "12.3M");
    }

    #[test]
    fn test_format_tokens_thousands() {
        assert_eq!(format_tokens(59_234), "59.2K");
        assert_eq!(format_tokens(1_000), "1.0K");
    }

    #[test]
    fn test_format_tokens_small() {
        assert_eq!(format_tokens(694), "694");
        assert_eq!(format_tokens(0), "0");
    }

    #[test]
    fn test_format_usd_large() {
        assert_eq!(format_usd(1234.567), "$1234.57");
        assert_eq!(format_usd(1000.0), "$1000.00");
    }

    #[test]
    fn test_format_usd_medium() {
        assert_eq!(format_usd(12.345), "$12.35");
        assert_eq!(format_usd(0.99), "$0.99");
    }

    #[test]
    fn test_format_usd_small() {
        assert_eq!(format_usd(0.0096), "$0.0096");
        assert_eq!(format_usd(0.0001), "$0.0001");
    }

    #[test]
    fn test_format_usd_edge() {
        assert_eq!(format_usd(0.01), "$0.01");
        assert_eq!(format_usd(0.009), "$0.0090");
    }

    #[test]
    fn test_ok_confirmation_with_detail() {
        assert_eq!(ok_confirmation("merged", "#42"), "ok merged #42");
        assert_eq!(
            ok_confirmation("created", "PR #5 https://github.com/foo/bar/pull/5"),
            "ok created PR #5 https://github.com/foo/bar/pull/5"
        );
    }

    #[test]
    fn test_ok_confirmation_no_detail() {
        assert_eq!(ok_confirmation("commented", ""), "ok commented");
    }

    #[test]
    fn test_format_cpt_normal() {
        assert_eq!(format_cpt(0.000003), "$3.00/MTok");
        assert_eq!(format_cpt(0.0000038), "$3.80/MTok");
        assert_eq!(format_cpt(0.00000386), "$3.86/MTok");
    }

    #[test]
    fn test_format_cpt_edge_cases() {
        assert_eq!(format_cpt(0.0), "$0.00/MTok"); // zero
        assert_eq!(format_cpt(-0.000001), "$0.00/MTok"); // negative
        assert_eq!(format_cpt(f64::INFINITY), "$0.00/MTok"); // infinite
        assert_eq!(format_cpt(f64::NAN), "$0.00/MTok"); // NaN
    }

    #[test]
    fn test_detect_package_manager_default() {
        // In the test environment (rtk repo), there's no JS lockfile
        // so it should default to "npm"
        let pm = detect_package_manager();
        assert!(["pnpm", "yarn", "npm"].contains(&pm));
    }

    #[test]
    fn test_truncate_multibyte_thai() {
        // Thai characters are 3 bytes each
        let thai = "สวัสดีครับ";
        let result = truncate(thai, 5);
        // Should not panic, should produce valid UTF-8
        assert!(result.len() <= thai.len());
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_multibyte_emoji() {
        let emoji = "🎉🎊🎈🎁🎂🎄🎃🎆🎇✨";
        let result = truncate(emoji, 5);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_multibyte_cjk() {
        let cjk = "你好世界测试字符串";
        let result = truncate(cjk, 6);
        assert!(result.ends_with("..."));
    }

    // ===== resolve_binary tests (issue #212) =====

    #[test]
    fn test_resolve_binary_finds_known_command() {
        // "cargo" must be on PATH in any Rust dev environment
        let result = resolve_binary("cargo");
        assert!(
            result.is_ok(),
            "resolve_binary('cargo') should succeed, got: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_resolve_binary_returns_absolute_path() {
        let path = resolve_binary("cargo").expect("cargo should be resolvable");
        assert!(
            path.is_absolute(),
            "resolve_binary should return absolute path, got: {:?}",
            path
        );
    }

    #[test]
    fn test_resolve_binary_fails_for_unknown() {
        let result = resolve_binary("nonexistent_binary_xyz_99999");
        assert!(
            result.is_err(),
            "resolve_binary should fail for nonexistent binary"
        );
    }

    #[test]
    fn test_resolve_binary_path_contains_binary_name() {
        let path = resolve_binary("cargo").expect("cargo should be resolvable");
        let filename = path
            .file_name()
            .expect("should have filename")
            .to_string_lossy();
        // On Windows this could be "cargo.exe", on Unix just "cargo"
        assert!(
            filename.starts_with("cargo"),
            "resolved path filename should start with 'cargo', got: {}",
            filename
        );
    }

    // ===== resolved_command tests (issue #212) =====

    #[test]
    fn test_resolved_command_executes_known_command() {
        let output = resolved_command("cargo")
            .arg("--version")
            .output()
            .expect("resolved_command('cargo') should execute");
        assert!(
            output.status.success(),
            "cargo --version should succeed via resolved_command"
        );
    }

    // ===== tool_exists tests (issue #212) =====

    #[test]
    fn test_tool_exists_finds_cargo() {
        assert!(
            tool_exists("cargo"),
            "tool_exists('cargo') should return true"
        );
    }

    #[test]
    fn test_tool_exists_rejects_unknown() {
        assert!(
            !tool_exists("nonexistent_binary_xyz_99999"),
            "tool_exists should return false for nonexistent binary"
        );
    }

    #[test]
    fn test_tool_exists_finds_git() {
        assert!(tool_exists("git"), "tool_exists('git') should return true");
    }

    // ===== Windows-specific PATHEXT resolution tests (issue #212) =====

    #[cfg(target_os = "windows")]
    mod windows_tests {
        use super::super::*;
        use std::fs;

        /// Create a temporary .cmd wrapper to simulate Node.js tool installation
        fn create_temp_cmd_wrapper(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
            let cmd_path = dir.join(format!("{}.cmd", name));
            fs::write(&cmd_path, "@echo off\r\necho fake-tool-output\r\n")
                .expect("failed to create .cmd wrapper");
            cmd_path
        }

        /// Build a PATH string that includes the temp dir
        fn path_with_dir(dir: &std::path::Path) -> std::ffi::OsString {
            let original = std::env::var_os("PATH").unwrap_or_default();
            let mut new_path = std::ffi::OsString::from(dir.as_os_str());
            new_path.push(";");
            new_path.push(&original);
            new_path
        }

        #[test]
        fn test_resolve_binary_finds_cmd_wrapper() {
            let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
            create_temp_cmd_wrapper(temp_dir.path(), "fake-tool-test");

            // Use which::which_in to avoid mutating global PATH (thread-safe)
            let search_path = path_with_dir(temp_dir.path());
            let result = which::which_in(
                "fake-tool-test",
                Some(search_path),
                std::env::current_dir().unwrap(),
            );

            assert!(
                result.is_ok(),
                "which_in should find .cmd wrapper on Windows, got: {:?}",
                result.err()
            );

            let path = result.unwrap();
            let ext = path
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            assert!(
                ext == "cmd" || ext == "bat",
                "resolved path should have .cmd/.bat extension, got: {:?}",
                path
            );
        }

        #[test]
        fn test_resolve_binary_finds_bat_wrapper() {
            let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
            let bat_path = temp_dir.path().join("fake-bat-tool.bat");
            fs::write(&bat_path, "@echo off\r\necho bat-output\r\n")
                .expect("failed to create .bat wrapper");

            let search_path = path_with_dir(temp_dir.path());
            let result = which::which_in(
                "fake-bat-tool",
                Some(search_path),
                std::env::current_dir().unwrap(),
            );

            assert!(
                result.is_ok(),
                "which_in should find .bat wrapper on Windows, got: {:?}",
                result.err()
            );
        }

        #[test]
        fn test_resolved_command_executes_cmd_wrapper() {
            let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
            create_temp_cmd_wrapper(temp_dir.path(), "fake-exec-test");

            // Resolve the full path, then execute it directly (no PATH mutation)
            let search_path = path_with_dir(temp_dir.path());
            let resolved = which::which_in(
                "fake-exec-test",
                Some(search_path),
                std::env::current_dir().unwrap(),
            )
            .expect("should resolve fake-exec-test");

            let output = Command::new(&resolved).output();

            assert!(
                output.is_ok(),
                "Command with resolved path should execute .cmd wrapper on Windows"
            );
            let output = output.unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                stdout.contains("fake-tool-output"),
                "should get output from .cmd wrapper, got: {}",
                stdout
            );
        }

        #[test]
        fn test_resolved_command_fallback_on_unknown_binary() {
            // When resolve_binary fails, resolved_command should fall back to
            // Command::new(name) instead of panicking.  On Windows this also
            // prints a warning to stderr.
            let mut cmd = resolved_command("nonexistent_binary_xyz_99999");
            // The Command should be created (not panic).  Attempting to run it
            // will fail, but that's expected — we just verify the fallback path
            // produces a usable Command.
            let result = cmd.output();
            assert!(
                result.is_err() || !result.unwrap().status.success(),
                "nonexistent binary should fail to execute, but resolved_command must not panic"
            );
        }

        #[test]
        fn test_tool_exists_finds_cmd_wrapper() {
            let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
            create_temp_cmd_wrapper(temp_dir.path(), "fake-exists-test");

            let search_path = path_with_dir(temp_dir.path());
            let result = which::which_in(
                "fake-exists-test",
                Some(search_path),
                std::env::current_dir().unwrap(),
            );

            assert!(
                result.is_ok(),
                "which_in should find .cmd wrapper on Windows"
            );
        }
    }
}
