use crate::tracking;
use anyhow::{Context, Result};
use std::process::Command;

/// Validates npm package name according to official rules
/// https://docs.npmjs.com/cli/v9/configuring-npm/package-json#name
fn is_valid_package_name(name: &str) -> bool {
    // Basic validation: alphanumeric, @, /, -, _, .
    // Reject: path traversal (..), shell metacharacters, excessive length
    if name.is_empty() || name.len() > 214 {
        return false;
    }

    // No path traversal
    if name.contains("..") {
        return false;
    }

    // Only safe characters
    name.chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '@' | '/' | '-' | '_' | '.'))
}

#[derive(Debug, Clone)]
pub enum PnpmCommand {
    List { depth: usize },
    Outdated,
    Install { packages: Vec<String> },
}

pub fn run(cmd: PnpmCommand, args: &[String], verbose: u8) -> Result<()> {
    match cmd {
        PnpmCommand::List { depth } => run_list(depth, args, verbose),
        PnpmCommand::Outdated => run_outdated(args, verbose),
        PnpmCommand::Install { packages } => run_install(&packages, args, verbose),
    }
}

fn run_list(depth: usize, args: &[String], verbose: u8) -> Result<()> {
    let mut cmd = Command::new("pnpm");
    cmd.arg("list");
    cmd.arg(format!("--depth={}", depth));

    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run pnpm list")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("pnpm list failed: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let filtered = filter_pnpm_list(&stdout);

    if verbose > 0 {
        eprintln!("pnpm list (filtered):");
    }

    println!("{}", filtered);

    tracking::track(
        &format!("pnpm list --depth={}", depth),
        &format!("rtk pnpm list --depth={}", depth),
        &stdout,
        &filtered,
    );

    Ok(())
}

fn run_outdated(args: &[String], verbose: u8) -> Result<()> {
    let mut cmd = Command::new("pnpm");
    cmd.arg("outdated");

    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run pnpm outdated")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // pnpm outdated returns exit code 1 when there are outdated packages
    // This is expected behavior, not an error
    let combined = format!("{}{}", stdout, stderr);
    let filtered = filter_pnpm_outdated(&combined);

    if verbose > 0 {
        eprintln!("pnpm outdated (filtered):");
    }

    if filtered.trim().is_empty() {
        println!("All packages up-to-date ✓");
    } else {
        println!("{}", filtered);
    }

    tracking::track("pnpm outdated", "rtk pnpm outdated", &combined, &filtered);

    Ok(())
}

fn run_install(packages: &[String], args: &[String], verbose: u8) -> Result<()> {
    // Validate package names to prevent command injection
    for pkg in packages {
        if !is_valid_package_name(pkg) {
            anyhow::bail!(
                "Invalid package name: '{}' (contains unsafe characters)",
                pkg
            );
        }
    }

    let mut cmd = Command::new("pnpm");
    cmd.arg("install");

    for pkg in packages {
        cmd.arg(pkg);
    }

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("pnpm install running...");
    }

    let output = cmd.output().context("Failed to run pnpm install")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        anyhow::bail!("pnpm install failed: {}", stderr);
    }

    let combined = format!("{}{}", stdout, stderr);
    let filtered = filter_pnpm_install(&combined);

    println!("{}", filtered);

    tracking::track(
        &format!("pnpm install {}", packages.join(" ")),
        &format!("rtk pnpm install {}", packages.join(" ")),
        &combined,
        &filtered,
    );

    Ok(())
}

/// Filter pnpm list output - remove box drawing, keep package tree
fn filter_pnpm_list(output: &str) -> String {
    let mut result = Vec::new();

    for line in output.lines() {
        // Skip box-drawing characters
        if line.contains("│")
            || line.contains("├")
            || line.contains("└")
            || line.contains("┌")
            || line.contains("┐")
        {
            continue;
        }

        // Skip legend and metadata
        if line.starts_with("Legend:") || line.trim().is_empty() {
            continue;
        }

        // Skip paths
        if line.contains("node_modules/.pnpm/") {
            continue;
        }

        result.push(line.trim().to_string());
    }

    result.join("\n")
}

/// Filter pnpm outdated output - extract package upgrades only
fn filter_pnpm_outdated(output: &str) -> String {
    let mut upgrades = Vec::new();

    for line in output.lines() {
        // Skip box-drawing characters
        if line.contains("│")
            || line.contains("├")
            || line.contains("└")
            || line.contains("┌")
            || line.contains("┐")
            || line.contains("─")
        {
            continue;
        }

        // Skip headers and legend
        if line.starts_with("Legend:") || line.starts_with("Package") || line.trim().is_empty() {
            continue;
        }

        // Parse package lines: "package  current  wanted  latest"
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            let package = parts[0];
            let current = parts[1];
            let latest = parts[3];

            // Only show if there's an actual upgrade
            if current != latest {
                upgrades.push(format!("{}: {} → {}", package, current, latest));
            }
        }
    }

    upgrades.join("\n")
}

/// Filter pnpm install output - remove progress bars, keep summary
fn filter_pnpm_install(output: &str) -> String {
    let mut result = Vec::new();
    let mut saw_progress = false;

    for line in output.lines() {
        // Skip progress bars (contain: Progress, │, %)
        if line.contains("Progress") || line.contains("│") || line.contains('%') {
            saw_progress = true;
            continue;
        }

        // Skip empty lines after progress
        if saw_progress && line.trim().is_empty() {
            continue;
        }

        // Keep error lines
        if line.contains("ERR") || line.contains("error") || line.contains("ERROR") {
            result.push(line.to_string());
            continue;
        }

        // Keep summary lines
        if line.contains("packages in")
            || line.contains("dependencies")
            || line.starts_with('+')
            || line.starts_with('-')
        {
            result.push(line.trim().to_string());
        }
    }

    if result.is_empty() {
        "ok ✓".to_string()
    } else {
        result.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_outdated() {
        let output = r#"
┌─────────────────────────────────────────────────────────┐
│ Package         Current  Wanted   Latest                │
├─────────────────────────────────────────────────────────┤
└─────────────────────────────────────────────────────────┘
@clerk/express    1.7.53   1.7.53   1.7.65
next              15.1.4   15.1.4   15.2.0
Legend: <outdated> ...
"#;
        let result = filter_pnpm_outdated(output);
        assert!(result.contains("@clerk/express: 1.7.53 → 1.7.65"));
        assert!(result.contains("next: 15.1.4 → 15.2.0"));
        assert!(!result.contains("┌"));
        assert!(!result.contains("Legend:"));
    }

    #[test]
    fn test_filter_list() {
        let output = r#"
project@1.0.0 /path/to/project
├── express@4.18.2
│   └── accepts@1.3.8
└── next@15.1.4
    └── react@18.2.0
"#;
        let result = filter_pnpm_list(output);
        assert!(!result.contains("├"));
        assert!(!result.contains("└"));
    }

    #[test]
    fn test_package_name_validation_valid() {
        assert!(is_valid_package_name("lodash"));
        assert!(is_valid_package_name("@clerk/express"));
        assert!(is_valid_package_name("my-package"));
        assert!(is_valid_package_name("package_name"));
        assert!(is_valid_package_name("package.js"));
    }

    #[test]
    fn test_package_name_validation_invalid() {
        assert!(!is_valid_package_name("lodash; rm -rf /"));
        assert!(!is_valid_package_name("../../../etc/passwd"));
        assert!(!is_valid_package_name("package$name"));
        assert!(!is_valid_package_name("pack age"));
        assert!(!is_valid_package_name(&"a".repeat(215))); // Too long
    }
}
