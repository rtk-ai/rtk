//! Filters pip and uv package manager output.

use crate::core::guard::never_worse;
use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::truncate::{CAP_INVENTORY, CAP_LIST};
use crate::core::utils::{resolved_command, tool_exists};
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
    // of the active one — often just the 2-package base interpreter. A `pip`
    // that is on PATH but cannot be started is handled below, after the spawn.
    let use_uv = !tool_exists("pip") && tool_exists("uv");
    let mut base_cmd = if use_uv { "uv" } else { "pip" };

    if verbose > 0 && use_uv {
        eprintln!("pip not found — falling back to `uv pip`");
    }

    let (cmd_str, filtered, exit_code) = match dispatch(base_cmd, args, verbose) {
        // `pip` was on PATH but the OS refused to start it — typically a stale
        // console script whose `#!` line names a deleted interpreter. Nothing
        // ran, so retrying through `uv pip` is safe even for `install`.
        Err(e) if base_cmd == "pip" && is_spawn_not_found(&e) && tool_exists("uv") => {
            eprintln!("rtk: `pip` is on PATH but could not be started — falling back to `uv pip`");
            base_cmd = "uv";
            dispatch(base_cmd, args, verbose)?
        }
        result => result?,
    };

    timer.track(
        &format!("{} {}", base_cmd, args.join(" ")),
        &format!("rtk {} {}", base_cmd, args.join(" ")),
        &cmd_str,
        &filtered,
    );

    Ok(exit_code)
}

/// Route a pip subcommand to its handler under `base_cmd` (`pip` or `uv`).
fn dispatch(base_cmd: &str, args: &[String], verbose: u8) -> Result<(String, String, i32)> {
    let subcommand = args.first().map(|s| s.as_str()).unwrap_or("");

    match subcommand {
        "list" => run_list(base_cmd, &args[1..], verbose),
        "outdated" => run_outdated(base_cmd, &args[1..], verbose),
        "install" | "uninstall" | "show" => {
            // Passthrough for write operations
            run_passthrough(base_cmd, args, verbose)
        }
        _ => {
            // Unknown subcommand: passthrough to pip/uv
            run_passthrough(base_cmd, args, verbose)
        }
    }
}

/// True when `err` bottoms out in the OS refusing to start the program
/// (ENOENT). On Unix a script with a dangling `#!` line fails the same way,
/// so the file can exist on PATH and still be unrunnable.
fn is_spawn_not_found(err: &anyhow::Error) -> bool {
    err.root_cause()
        .downcast_ref::<std::io::Error>()
        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
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
    fn test_is_spawn_not_found_matches_enoent_through_context() {
        let err = anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::NotFound))
            .context("Failed to execute command")
            .context("Failed to run pip list");
        assert!(is_spawn_not_found(&err));
    }

    #[test]
    fn test_is_spawn_not_found_ignores_other_errors() {
        let perm = anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert!(!is_spawn_not_found(&perm));
        assert!(!is_spawn_not_found(&anyhow::anyhow!("filter failed")));
    }

    /// A console script whose shebang points at a missing interpreter is on
    /// PATH and executable, yet the OS refuses to start it with ENOENT. This
    /// is the #4054 shape; `tool_exists` alone cannot tell it from a good pip.
    #[cfg(unix)]
    #[test]
    fn test_dead_shebang_script_is_spawn_not_found() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let script = dir.path().join("pip");
        std::fs::write(&script, "#!/nonexistent/python3.7\nprint('hi')\n").expect("write");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        let mut cmd = std::process::Command::new(&script);
        let err = exec_capture(&mut cmd)
            .err()
            .expect("dead shebang must fail to spawn");
        assert!(is_spawn_not_found(&err), "unexpected error: {err:#}");
    }

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
}
