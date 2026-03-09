//! Utility functions for text processing and command execution.
//!
//! Provides common helpers used across rtk commands:
//! - ANSI color code stripping
//! - Text truncation
//! - Command execution with error context

use anyhow::{Context, Result};
use regex::Regex;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Truncates a string to `max_len` characters, appending "..." if needed.
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

/// Strips ANSI escape codes (colors, styles) from a string.
///
/// # Arguments
/// * `text` - Text potentially containing ANSI codes
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

/// Execute a command and return cleaned stdout/stderr.
///
/// # Arguments
/// * `cmd` - Command to execute (e.g. "eslint")
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
    let output = Command::new(cmd)
        .args(args)
        .output()
        .context(format!("Failed to execute {}", cmd))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);

    Ok((stdout, stderr, exit_code))
}

fn is_executable(path: &Path) -> bool {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn is_single_normal_component(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(
        (components.next(), components.next()),
        (Some(std::path::Component::Normal(_)), None)
    )
}

fn is_same_executable(candidate: &Path, current_exe: &Path) -> bool {
    let candidate_canon = fs::canonicalize(candidate).ok();
    let current_canon = fs::canonicalize(current_exe).ok();

    if let (Some(a), Some(b)) = (&candidate_canon, &current_canon) {
        if a == b {
            return true;
        }
        #[cfg(unix)]
        {
            if let (Ok(ma), Ok(mb)) = (fs::metadata(a), fs::metadata(b)) {
                if ma.dev() == mb.dev() && ma.ino() == mb.ino() {
                    return true;
                }
            }
        }
    }

    #[cfg(unix)]
    {
        if let (Ok(ma), Ok(mb)) = (fs::metadata(candidate), fs::metadata(current_exe)) {
            if ma.dev() == mb.dev() && ma.ino() == mb.ino() {
                return true;
            }
        }
    }

    false
}

#[cfg(windows)]
fn command_candidates(program: &str) -> Vec<String> {
    let has_ext = Path::new(program).extension().is_some();
    if has_ext {
        return vec![program.to_string()];
    }
    let pathext = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    pathext
        .split(';')
        .filter(|s| !s.trim().is_empty())
        .map(|ext| format!("{program}{ext}"))
        .collect()
}

#[cfg(not(windows))]
fn command_candidates(program: &str) -> Vec<String> {
    vec![program.to_string()]
}

fn resolve_non_self_from_paths(
    program: &str,
    paths: &[PathBuf],
    current_exe: &Path,
) -> Option<PathBuf> {
    let candidates = command_candidates(program);
    for dir in paths {
        for candidate_name in &candidates {
            let candidate = dir.join(candidate_name);
            if !is_executable(&candidate) {
                continue;
            }
            if is_same_executable(&candidate, current_exe) {
                continue;
            }
            return Some(candidate);
        }
    }
    None
}

/// Resolve a command from PATH while skipping any entry that points to current rtk executable.
pub fn resolve_non_self_command(program: &str) -> Result<PathBuf> {
    let current_exe = env::current_exe().context("Failed to resolve current executable path")?;

    if program.contains('/') || program.contains('\\') {
        let candidate = PathBuf::from(program);
        if is_same_executable(&candidate, &current_exe) {
            anyhow::bail!(
                "Resolved command '{}' points to current rtk executable; refusing recursive invocation",
                program
            );
        }
        return Ok(candidate);
    }

    let path_env = env::var_os("PATH").unwrap_or_default();
    let paths: Vec<PathBuf> = env::split_paths(&path_env).collect();
    if let Some(path) = resolve_non_self_from_paths(program, &paths, &current_exe) {
        return Ok(path);
    }

    anyhow::bail!(
        "Unable to find native command '{}' in PATH without matching current rtk executable",
        program
    )
}

/// Build a command using the resolved native executable path (skips self-recursive links).
pub fn native_command(program: &str) -> Result<Command> {
    let resolved = resolve_non_self_command(program)?;
    Ok(Command::new(resolved))
}

pub fn default_shim_bin_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
        .join("rtk-shims")
        .join("bin")
}

pub fn resolve_shim_operational_commands(
    requested: &[String],
    allowed: &[String],
) -> Result<Vec<String>> {
    if requested.is_empty() {
        return Ok(allowed.to_vec());
    }

    let mut operational_commands = Vec::with_capacity(requested.len());
    for operational_command in requested {
        if !is_single_normal_component(operational_command) {
            anyhow::bail!(
                "invalid operational_command name: '{}'",
                operational_command
            );
        }
        if !allowed
            .iter()
            .any(|allowed_operational_command| allowed_operational_command == operational_command)
        {
            anyhow::bail!(
                "operational_command '{}' is not Shim-eligible in this rtk binary",
                operational_command
            );
        }
        operational_commands.push(operational_command.clone());
    }

    Ok(operational_commands)
}

fn create_operational_command_shim(target: &Path, link: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).with_context(|| {
            format!(
                "failed to create symlink '{}' -> '{}'",
                link.display(),
                target.display()
            )
        })?;
        Ok(())
    }

    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link).with_context(|| {
            format!(
                "failed to create symlink '{}' -> '{}'",
                link.display(),
                target.display()
            )
        })?;
        Ok(())
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        anyhow::bail!("symlink creation is not supported on this platform");
    }
}

fn is_rtk_named_path(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|s| s.to_str()),
        Some("rtk") | Some("rtk.exe")
    )
}

fn is_rtk_shim_symlink(link_path: &Path, target_rtk: &Path) -> bool {
    let Ok(link_target) = fs::read_link(link_path) else {
        return false;
    };

    let resolved_target = if link_target.is_absolute() {
        link_target
    } else {
        link_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(link_target)
    };

    if is_same_executable(&resolved_target, target_rtk) {
        return true;
    }

    if is_rtk_named_path(&resolved_target) {
        return true;
    }

    fs::canonicalize(&resolved_target)
        .map(|p| is_rtk_named_path(&p))
        .unwrap_or(false)
}

pub fn install_operational_command_shims(
    bin_dir: Option<PathBuf>,
    rtk_bin: Option<PathBuf>,
    force: bool,
    force_all: bool,
    operational_commands: &[String],
) -> Result<()> {
    let bin_dir = bin_dir.unwrap_or_else(default_shim_bin_dir);
    fs::create_dir_all(&bin_dir).with_context(|| {
        format!(
            "failed to create operational_command-shim bin dir '{}'",
            bin_dir.display()
        )
    })?;

    let rtk_target = match rtk_bin {
        Some(path) => path,
        None => env::current_exe().context("failed to resolve current executable")?,
    };

    if !rtk_target.exists() {
        anyhow::bail!("rtk binary does not exist: {}", rtk_target.display());
    }
    if !rtk_target.is_file() {
        anyhow::bail!("rtk binary is not a file: {}", rtk_target.display());
    }
    if !is_executable(&rtk_target) {
        anyhow::bail!("rtk binary is not executable: {}", rtk_target.display());
    }

    let rtk_abs = fs::canonicalize(&rtk_target).unwrap_or(rtk_target);
    let mut created = 0usize;
    let mut replaced = 0usize;
    let mut skipped = 0usize;

    for operational_command in operational_commands {
        let link_path = bin_dir.join(operational_command);
        match fs::symlink_metadata(&link_path) {
            Ok(meta) => {
                if force_all {
                    if meta.file_type().is_dir() {
                        anyhow::bail!(
                            "refusing to replace existing directory: {}",
                            link_path.display()
                        );
                    }
                    fs::remove_file(&link_path).with_context(|| {
                        format!("failed to remove existing entry '{}'", link_path.display())
                    })?;
                    create_operational_command_shim(&rtk_abs, &link_path)?;
                    println!("replaced: {} -> {}", link_path.display(), rtk_abs.display());
                    replaced += 1;
                } else if force {
                    if meta.file_type().is_dir() {
                        anyhow::bail!(
                            "refusing to replace existing directory: {}",
                            link_path.display()
                        );
                    }
                    if !meta.file_type().is_symlink() || !is_rtk_shim_symlink(&link_path, &rtk_abs)
                    {
                        anyhow::bail!(
                            "refusing to replace non-rtk shim '{}'; rerun with --force-all to replace arbitrary files",
                            link_path.display()
                        );
                    }
                    fs::remove_file(&link_path).with_context(|| {
                        format!("failed to remove existing entry '{}'", link_path.display())
                    })?;
                    create_operational_command_shim(&rtk_abs, &link_path)?;
                    println!("replaced: {} -> {}", link_path.display(), rtk_abs.display());
                    replaced += 1;
                } else {
                    println!("skipped (exists): {}", link_path.display());
                    skipped += 1;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                create_operational_command_shim(&rtk_abs, &link_path)?;
                println!("created: {} -> {}", link_path.display(), rtk_abs.display());
                created += 1;
            }
            Err(e) => {
                return Err(e).with_context(|| {
                    format!("failed to inspect existing path '{}'", link_path.display())
                });
            }
        }
    }

    println!(
        "done: created={} replaced={} skipped={}",
        created, replaced, skipped
    );

    Ok(())
}

/// Format a token count with K/M suffixes for readability.
///
/// # Arguments
/// * `n` - Token count
///
/// # Returns
/// Formatted string (e.g. "1.2M", "59.2K", "694")
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

/// Format a USD amount with adaptive precision.
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
    let tool_exists = Command::new("which")
        .arg(tool)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if tool_exists {
        Command::new(tool)
    } else {
        let pm = detect_package_manager();
        match pm {
            "pnpm" => {
                let mut c = Command::new("pnpm");
                c.arg("exec").arg("--").arg(tool);
                c
            }
            "yarn" => {
                let mut c = Command::new("yarn");
                c.arg("exec").arg("--").arg(tool);
                c
            }
            _ => {
                let mut c = Command::new("npx");
                c.arg("--no-install").arg("--").arg(tool);
                c
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

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

    #[test]
    fn test_resolve_shim_operational_commands_rejects_non_normal_component() {
        let allowed = vec![
            "git".to_string(),
            ".".to_string(),
            "..".to_string(),
            "foo/bar".to_string(),
        ];
        assert!(resolve_shim_operational_commands(&["git".to_string()], &allowed).is_ok());

        for invalid in [".", "..", "foo/bar"] {
            let err = resolve_shim_operational_commands(&[invalid.to_string()], &allowed)
                .expect_err("expected invalid operational_command name to fail");
            assert!(
                err.to_string()
                    .contains(&format!("invalid operational_command name: '{}'", invalid)),
                "unexpected error: {}",
                err
            );
        }
    }

    #[cfg(unix)]
    fn create_executable_file(path: &std::path::Path) {
        let mut f = fs::File::create(path).unwrap();
        writeln!(f, "#!/bin/sh").unwrap();
        writeln!(f, "exit 0").unwrap();
        drop(f);

        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn test_resolve_non_self_from_paths_skips_self_symlink() {
        let current_exe = env::current_exe().unwrap();

        let first = tempdir().unwrap();
        let second = tempdir().unwrap();

        let fake_self = first.path().join("git");
        std::os::unix::fs::symlink(&current_exe, &fake_self).unwrap();

        let native_git = second.path().join("git");
        let mut f = fs::File::create(&native_git).unwrap();
        writeln!(f, "#!/bin/sh").unwrap();
        writeln!(f, "echo native").unwrap();
        drop(f);

        let mut perms = fs::metadata(&native_git).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&native_git, perms).unwrap();

        let paths = vec![first.path().to_path_buf(), second.path().to_path_buf()];
        let resolved = resolve_non_self_from_paths("git", &paths, &current_exe).unwrap();
        assert_eq!(resolved, native_git);
    }

    #[cfg(unix)]
    #[test]
    fn test_resolve_non_self_command_errors_when_only_self_exists() {
        let current_exe = env::current_exe().unwrap();
        let dir = tempdir().unwrap();
        let fake_self = dir.path().join("ls");
        std::os::unix::fs::symlink(&current_exe, &fake_self).unwrap();

        let resolved = resolve_non_self_from_paths("ls", &[dir.path().to_path_buf()], &current_exe);
        assert!(resolved.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn test_install_operational_command_shims_create_skip_replace() {
        let dir = tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        let primary_dir = dir.path().join("primary");
        let secondary_dir = dir.path().join("secondary");
        fs::create_dir_all(&primary_dir).unwrap();
        fs::create_dir_all(&secondary_dir).unwrap();

        let primary_rtk = primary_dir.join("rtk");
        let secondary_rtk = secondary_dir.join("rtk");
        create_executable_file(&primary_rtk);
        create_executable_file(&secondary_rtk);
        let primary_abs = fs::canonicalize(&primary_rtk).unwrap();

        let commands = vec!["git".to_string(), "curl".to_string()];
        install_operational_command_shims(
            Some(bin_dir.clone()),
            Some(primary_rtk.clone()),
            false,
            false,
            &commands,
        )
        .unwrap();

        let git_link = bin_dir.join("git");
        assert!(fs::symlink_metadata(&git_link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(&git_link).unwrap(), primary_abs);

        install_operational_command_shims(
            Some(bin_dir.clone()),
            Some(primary_rtk.clone()),
            false,
            false,
            &commands,
        )
        .unwrap();
        assert_eq!(fs::read_link(&git_link).unwrap(), primary_abs);

        fs::remove_file(&git_link).unwrap();
        std::os::unix::fs::symlink(&secondary_rtk, &git_link).unwrap();

        install_operational_command_shims(
            Some(bin_dir.clone()),
            Some(primary_rtk),
            true,
            false,
            &["git".to_string()],
        )
        .unwrap();
        assert_eq!(fs::read_link(&git_link).unwrap(), primary_abs);
    }

    #[cfg(unix)]
    #[test]
    fn test_install_operational_command_shims_force_refuses_directory() {
        let dir = tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        fs::create_dir_all(bin_dir.join("git")).unwrap();

        let rtk_bin = dir.path().join("rtk");
        create_executable_file(&rtk_bin);

        let err = install_operational_command_shims(
            Some(bin_dir),
            Some(rtk_bin),
            true,
            false,
            &["git".to_string()],
        )
        .expect_err("expected replacing directory to fail");
        assert!(
            err.to_string()
                .contains("refusing to replace existing directory"),
            "unexpected error: {}",
            err
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_install_operational_command_shims_force_refuses_non_shim_without_force_all() {
        let dir = tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join("git"), "plain file").unwrap();

        let rtk_bin = dir.path().join("rtk");
        create_executable_file(&rtk_bin);

        let err = install_operational_command_shims(
            Some(bin_dir.clone()),
            Some(rtk_bin.clone()),
            true,
            false,
            &["git".to_string()],
        )
        .expect_err("expected non-shim replacement to require --force-all");
        assert!(
            err.to_string().contains("--force-all"),
            "unexpected error: {}",
            err
        );

        install_operational_command_shims(
            Some(bin_dir.clone()),
            Some(rtk_bin),
            false,
            true,
            &["git".to_string()],
        )
        .unwrap();
        assert!(fs::symlink_metadata(bin_dir.join("git"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn test_install_operational_command_shims_rejects_non_executable_rtk_bin() {
        let dir = tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        let non_exec_rtk = dir.path().join("rtk");
        fs::write(&non_exec_rtk, "#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = fs::metadata(&non_exec_rtk).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&non_exec_rtk, perms).unwrap();

        let err = install_operational_command_shims(
            Some(bin_dir),
            Some(non_exec_rtk),
            false,
            false,
            &["git".to_string()],
        )
        .expect_err("expected non-executable rtk bin to fail");
        assert!(
            err.to_string().contains("not executable"),
            "unexpected error: {}",
            err
        );
    }
}
