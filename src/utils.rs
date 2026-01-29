//! Utility functions for text processing and command execution.
//!
//! Provides common helpers used across rtk commands:
//! - ANSI color code stripping
//! - Text truncation
//! - Command execution with error context

use anyhow::{Context, Result};
use regex::Regex;
use std::process::Command;

/// Tronque une chaîne à `max_len` caractères avec "..." si nécessaire.
///
/// # Arguments
/// * `s` - La chaîne à tronquer
/// * `max_len` - Longueur maximale avant troncature (minimum 3 pour inclure "...")
///
/// # Examples
/// ```
/// use rtk::utils::truncate;
/// assert_eq!(truncate("hello world", 8), "hello...");
/// assert_eq!(truncate("hi", 10), "hi");
/// ```
pub fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else if max_len < 3 {
        // If max_len is too small, just return "..."
        "...".to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

/// Supprime les codes ANSI d'une chaîne (couleurs, styles).
///
/// # Arguments
/// * `text` - Texte contenant potentiellement des codes ANSI
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

/// Exécute une commande et retourne stdout/stderr nettoyés.
///
/// # Arguments
/// * `cmd` - Commande à exécuter (ex: "eslint")
/// * `args` - Arguments de la commande
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
}
