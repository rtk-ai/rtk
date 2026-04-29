//! Minimal handler for PowerShell `Remove-Item` rewrites.
//!
//! Executes the deletion via `pwsh -Command "Remove-Item ..."` if `pwsh` is on
//! PATH. Falls back to `powershell` (Windows PowerShell 5.1) on Windows. Emits
//! `ok <basename>` on success, the error line + non-zero exit code on failure.

use crate::core::tracking;
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let raw_cmd = format!("Remove-Item {}", args.join(" "));
    if verbose > 0 {
        eprintln!("Running: {}", raw_cmd);
    }

    let pwsh = locate_pwsh();
    let mut cmd = Command::new(&pwsh);
    cmd.arg("-NoProfile").arg("-Command").arg(&raw_cmd);

    let output = cmd
        .output()
        .with_context(|| format!("Failed to execute {}", pwsh))?;
    let exit_code = output.status.code().unwrap_or(1);
    let raw = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let display = if exit_code == 0 {
        let target = first_target(args);
        let basename = target
            .map(|t| {
                Path::new(t)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(t)
                    .to_string()
            })
            .unwrap_or_else(|| "(target)".to_string());
        format!("ok  {}", basename)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let first_line = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        format!("Remove-Item failed (exit {}): {}", exit_code, first_line.trim())
    };

    println!("{}", display);
    timer.track(&raw_cmd, "rtk remove-item", &raw, &display);

    Ok(exit_code)
}

fn locate_pwsh() -> String {
    if which::which("pwsh").is_ok() {
        return "pwsh".to_string();
    }
    if cfg!(target_os = "windows") {
        return "powershell".to_string();
    }
    // Last-resort fallback — caller will get a sensible error.
    "pwsh".to_string()
}

/// Return the first user-supplied path argument (after stripping flag tokens).
fn first_target(args: &[String]) -> Option<&str> {
    let mut iter = args.iter().peekable();
    while let Some(a) = iter.next() {
        let lower = a.to_ascii_lowercase();
        if lower == "-literalpath" || lower == "-path" {
            if let Some(next) = iter.peek() {
                return Some(next.as_str());
            }
        }
        if let Some(rest) = a.strip_prefix("-LiteralPath:") {
            return Some(rest);
        }
        if let Some(rest) = a.strip_prefix("-Path:") {
            return Some(rest);
        }
        // Skip flags
        if a.starts_with('-') {
            continue;
        }
        return Some(a.as_str());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_target_literalpath() {
        let args = vec![
            "-LiteralPath".to_string(),
            "D:\\MyProject\\lib\\Debug\\MyLib.obj".to_string(),
            "-Force".to_string(),
        ];
        assert_eq!(first_target(&args), Some("D:\\MyProject\\lib\\Debug\\MyLib.obj"));
    }

    #[test]
    fn test_first_target_path() {
        let args = vec!["-Path".to_string(), ".\\build\\".to_string(), "-Recurse".to_string()];
        assert_eq!(first_target(&args), Some(".\\build\\"));
    }

    #[test]
    fn test_first_target_positional() {
        let args = vec!["-Force".to_string(), "myfile.txt".to_string()];
        assert_eq!(first_target(&args), Some("myfile.txt"));
    }

    #[test]
    fn test_first_target_colon_form() {
        let args = vec!["-LiteralPath:C:\\x\\y.obj".to_string(), "-Force".to_string()];
        assert_eq!(first_target(&args), Some("C:\\x\\y.obj"));
    }
}
