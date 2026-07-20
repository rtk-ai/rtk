//! Filters pip and uv package manager output.

use crate::core::guard::never_worse;
use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::truncate::{CAP_INVENTORY, CAP_LIST};
use crate::core::utils::{resolved_command, strip_ansi, tool_exists};
use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Package {
    name: String,
    version: String,
    #[serde(default)]
    latest_version: Option<String>,
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    // The user ran `pip` — run `pip` so RTK stays transparent and reports the
    // *same* environment the bare command would. Only fall back to `uv pip` when
    // `pip` genuinely isn't on PATH (uv-only environments). Auto-substituting
    // `uv pip` unconditionally made `pip list` show uv's discovered env instead
    // of the active one — often just the 2-package base interpreter.
    let use_uv = !tool_exists("pip") && tool_exists("uv");
    let base_cmd = if use_uv { "uv" } else { "pip" };

    if verbose > 0 && use_uv {
        eprintln!("pip not found — falling back to `uv pip`");
    }

    // Detect subcommand
    let subcommand = args.first().map(|s| s.as_str()).unwrap_or("");

    let (cmd_str, filtered, exit_code) = match subcommand {
        "list" => run_list(base_cmd, &args[1..], verbose)?,
        "outdated" => run_outdated(base_cmd, &args[1..], verbose)?,
        "install" if !has_install_verbose_flag(&args[1..]) => {
            run_install(base_cmd, &args[1..], verbose)?
        }
        "install" | "uninstall" | "show" => {
            run_passthrough(base_cmd, args, verbose)?
        }
        _ => {
            // Unknown subcommand: passthrough to pip/uv
            run_passthrough(base_cmd, args, verbose)?
        }
    };

    timer.track(
        &format!("{} {}", base_cmd, args.join(" ")),
        &format!("rtk {} {}", base_cmd, args.join(" ")),
        &cmd_str,
        &filtered,
    );

    Ok(exit_code)
}

pub(crate) fn has_install_verbose_flag(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg == "--verbose" || arg == "-v" || arg.starts_with("-vv"))
}

fn run_install(base_cmd: &str, args: &[String], verbose: u8) -> Result<(String, String, i32)> {
    let mut cmd = resolved_command(base_cmd);
    if base_cmd == "uv" {
        cmd.arg("pip");
    }
    cmd.arg("install").args(args);

    if verbose > 0 {
        eprintln!("Running: {} pip install {}", base_cmd, args.join(" "));
    }

    let result = exec_capture(&mut cmd)
        .with_context(|| format!("Failed to run {} pip install", base_cmd))?;
    let raw = format!("{}\n{}", result.stdout, result.stderr);

    if !result.success() {
        print!("{}", result.stdout);
        eprint!("{}", result.stderr);
        return Ok((raw.clone(), raw, result.exit_code));
    }

    let filtered = filter_pip_install_success(&raw);
    println!("{}", filtered);
    Ok((raw, filtered, result.exit_code))
}

pub(crate) fn filter_pip_install_success(output: &str) -> String {
    let clean = strip_ansi(output);
    let selected = clean
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.starts_with("Successfully installed ")
                || line.starts_with("Resolved ")
                || line.starts_with("Prepared ")
                || line.starts_with("Downloaded ")
                || line.starts_with("Installed ")
                || line.starts_with("Uninstalled ")
                || line.starts_with("WARNING:")
                || line.starts_with("+ ")
                || line.starts_with("- ")
        })
        .collect::<Vec<_>>();

    if selected.is_empty() {
        return clean.trim().to_string();
    }

    never_worse(&clean, &selected.join("\n")).to_string()
}

fn run_list(base_cmd: &str, args: &[String], verbose: u8) -> Result<(String, String, i32)> {
    let mut cmd = resolved_command(base_cmd);

    if base_cmd == "uv" {
        cmd.arg("pip");
    }

    cmd.arg("list").arg("--format=json");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: {} pip list --format=json", base_cmd);
    }

    let result = exec_capture(&mut cmd)
        .with_context(|| format!("Failed to run {} pip list", base_cmd))?;

    let raw = format!("{}\n{}", result.stdout, result.stderr);

    let filtered = never_worse(&raw, &filter_pip_list(&result.stdout)).to_string();
    println!("{}", filtered);

    Ok((raw, filtered, result.exit_code))
}

fn run_outdated(base_cmd: &str, args: &[String], verbose: u8) -> Result<(String, String, i32)> {
    let mut cmd = resolved_command(base_cmd);

    if base_cmd == "uv" {
        cmd.arg("pip");
    }

    cmd.arg("list").arg("--outdated").arg("--format=json");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: {} pip list --outdated --format=json", base_cmd);
    }

    let result = exec_capture(&mut cmd)
        .with_context(|| format!("Failed to run {} pip list --outdated", base_cmd))?;

    let raw = format!("{}\n{}", result.stdout, result.stderr);

    let filtered = never_worse(&raw, &filter_pip_outdated(&result.stdout)).to_string();
    println!("{}", filtered);

    Ok((raw, filtered, result.exit_code))
}

fn run_passthrough(base_cmd: &str, args: &[String], verbose: u8) -> Result<(String, String, i32)> {
    let mut cmd = resolved_command(base_cmd);

    if base_cmd == "uv" {
        cmd.arg("pip");
    }

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: {} pip {}", base_cmd, args.join(" "));
    }

    let result = exec_capture(&mut cmd)
        .with_context(|| format!("Failed to run {} pip {}", base_cmd, args.join(" ")))?;

    let raw = format!("{}\n{}", result.stdout, result.stderr);

    print!("{}", result.stdout);
    eprint!("{}", result.stderr);

    Ok((raw.clone(), raw, result.exit_code))
}

/// Filter pip list JSON output
fn filter_pip_list(output: &str) -> String {
    let packages: Vec<Package> = match serde_json::from_str(output) {
        Ok(p) => p,
        Err(e) => {
            return format!("pip list (JSON parse failed: {})", e);
        }
    };

    if packages.is_empty() {
        return "pip list: No packages installed".to_string();
    }

    let mut result = String::new();
    result.push_str(&format!("pip list: {} packages\n", packages.len()));

    // Group by first letter for easier scanning
    let mut by_letter: std::collections::HashMap<char, Vec<&Package>> =
        std::collections::HashMap::new();

    for pkg in &packages {
        let first_char = pkg.name.chars().next().unwrap_or('?').to_ascii_lowercase();
        by_letter.entry(first_char).or_default().push(pkg);
    }

    let mut letters: Vec<_> = by_letter.keys().collect();
    letters.sort();

    // `pip list` is an inventory query — dependency audits need every package
    // visible. The compression here is structural (drop the alignment padding,
    // group by initial); the per-group cap is just a safety bound for
    // pathological environments, not a normal-case truncation.
    const MAX_PER_LETTER: usize = CAP_INVENTORY;
    for letter in letters {
        let pkgs = by_letter.get(letter).unwrap();
        result.push_str(&format!("\n[{}]\n", letter.to_uppercase()));

        for pkg in pkgs.iter().take(MAX_PER_LETTER) {
            result.push_str(&format!("  {} ({})\n", pkg.name, pkg.version));
        }

        if pkgs.len() > MAX_PER_LETTER {
            result.push_str(&format!("  ... +{} more\n", pkgs.len() - MAX_PER_LETTER));
        }
    }

    result.trim().to_string()
}

/// Filter pip outdated JSON output
fn filter_pip_outdated(output: &str) -> String {
    let packages: Vec<Package> = match serde_json::from_str(output) {
        Ok(p) => p,
        Err(e) => {
            return format!("pip outdated (JSON parse failed: {})", e);
        }
    };

    if packages.is_empty() {
        return "pip outdated: All packages up to date".to_string();
    }

    let mut result = String::new();
    result.push_str(&format!("pip outdated: {} packages\n", packages.len()));

    const MAX_PIP_PACKAGES: usize = CAP_LIST;
    for (i, pkg) in packages.iter().take(MAX_PIP_PACKAGES).enumerate() {
        let latest = pkg.latest_version.as_deref().unwrap_or("unknown");
        result.push_str(&format!(
            "{}. {} ({} → {})\n",
            i + 1,
            pkg.name,
            pkg.version,
            latest
        ));
    }

    if packages.len() > MAX_PIP_PACKAGES {
        result.push_str(&format!(
            "\n... +{} more packages\n",
            packages.len() - MAX_PIP_PACKAGES
        ));
    }

    result.push_str("\n[hint] Run `pip install --upgrade <package>` to update\n");

    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_pip_list() {
        let output = r#"[
  {"name": "requests", "version": "2.31.0"},
  {"name": "pytest", "version": "7.4.0"},
  {"name": "rich", "version": "13.0.0"}
]"#;

        let result = filter_pip_list(output);
        assert!(result.contains("3 packages"));
        assert!(result.contains("requests"));
        assert!(result.contains("2.31.0"));
        assert!(result.contains("pytest"));
    }

    #[test]
    fn test_filter_pip_list_empty() {
        let output = "[]";
        let result = filter_pip_list(output);
        assert!(result.contains("No packages installed"));
    }

    #[test]
    fn test_filter_pip_outdated_none() {
        let output = "[]";
        let result = filter_pip_outdated(output);
        assert!(result.contains("All packages up to date"));
    }

    #[test]
    fn test_filter_pip_outdated_some() {
        let output = r#"[
  {"name": "requests", "version": "2.31.0", "latest_version": "2.32.0"},
  {"name": "pytest", "version": "7.4.0", "latest_version": "8.0.0"}
]"#;

        let result = filter_pip_outdated(output);
        assert!(result.contains("2 packages"));
        assert!(result.contains("requests"));
        assert!(result.contains("2.31.0 → 2.32.0"));
        assert!(result.contains("pytest"));
        assert!(result.contains("7.4.0 → 8.0.0"));
    }

    #[test]
    fn test_filter_pip_install_success_keeps_summary() {
        let output = "Collecting pytest\nDownloading pytest.whl (1.2 MB)\nInstalling collected packages: pytest\nSuccessfully installed pytest-8.0.0\n";
        assert_eq!(
            filter_pip_install_success(output),
            "Successfully installed pytest-8.0.0"
        );
    }

    #[test]
    fn test_filter_uv_pip_install_success_keeps_phases_and_packages() {
        let output = "Resolved 3 packages in 10ms\nPrepared 2 packages in 20ms\nInstalled 2 packages in 5ms\n + pytest==8.0.0\n + pluggy==1.5.0\n";
        let filtered = filter_pip_install_success(output);
        assert!(filtered.contains("Resolved 3 packages"));
        assert!(filtered.contains("Installed 2 packages"));
        assert!(filtered.contains("+ pytest==8.0.0"));
    }

    #[test]
    fn test_install_verbose_flags_request_passthrough() {
        assert!(has_install_verbose_flag(&["-v".to_string()]));
        assert!(has_install_verbose_flag(&["-vvv".to_string()]));
        assert!(has_install_verbose_flag(&["--verbose".to_string()]));
        assert!(!has_install_verbose_flag(&["pytest".to_string()]));
    }
}
