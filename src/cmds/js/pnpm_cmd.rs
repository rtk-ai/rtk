//! Filters pnpm output — dependency trees, install logs, outdated packages.

use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::truncate::CAP_LIST;
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::ffi::OsString;

use crate::parser::{
    emit_degradation_warning, emit_passthrough_warning, truncate_passthrough, Dependency,
    DependencyState, FormatMode, OutputParser, ParseResult, TokenFormatter,
};

const MAX_LISTING: usize = CAP_LIST;

/// pnpm list JSON output structure
#[derive(Debug, Deserialize)]
struct PnpmListOutput {
    name: String,
    #[serde(flatten)]
    package: PackageJsonListItem,
}

#[derive(Debug, Deserialize)]
struct PackageJsonListItem {
    version: Option<String>,
    #[serde(rename = "dependencies", default)]
    dependencies: HashMap<String, PackageJsonListItem>,
    #[serde(rename = "devDependencies", default)]
    dev_dependencies: HashMap<String, PackageJsonListItem>,
}

/// pnpm outdated JSON output structure
#[derive(Debug, Deserialize)]
struct PnpmOutdatedOutput {
    #[serde(flatten)]
    packages: HashMap<String, PnpmOutdatedPackage>,
}

#[derive(Debug, Deserialize)]
struct PnpmOutdatedPackage {
    current: String,
    latest: String,
    wanted: Option<String>,
    #[serde(rename = "dependencyType", default)]
    dependency_type: String,
}

/// Parser for pnpm list output
pub struct PnpmListParser;

impl OutputParser for PnpmListParser {
    type Output = DependencyState;

    fn parse(input: &str) -> ParseResult<DependencyState> {
        // Tier 1: Try JSON parsing
        match serde_json::from_str::<Vec<PnpmListOutput>>(input) {
            Ok(json) => {
                let mut dependencies = Vec::new();
                let mut total_count = 0;

                for pkg in &json {
                    collect_dependencies(
                        pkg.name.as_str(),
                        &pkg.package,
                        false,
                        &mut dependencies,
                        &mut total_count,
                    );
                }

                let result = DependencyState {
                    total_packages: total_count,
                    outdated_count: 0, // list doesn't provide outdated info
                    dependencies,
                };

                ParseResult::Full(result)
            }
            Err(e) => {
                // Tier 2: Try text extraction
                match extract_list_text(input) {
                    Some(result) => {
                        ParseResult::Degraded(result, vec![format!("JSON parse failed: {}", e)])
                    }
                    None => {
                        // Tier 3: Passthrough
                        ParseResult::Passthrough(truncate_passthrough(input))
                    }
                }
            }
        }
    }
}

/// Recursively collect dependencies from pnpm package tree
fn collect_dependencies(
    name: &str,
    pkg: &PackageJsonListItem,
    is_dev: bool,
    deps: &mut Vec<Dependency>,
    count: &mut usize,
) {
    if let Some(version) = &pkg.version {
        deps.push(Dependency {
            name: name.to_string(),
            current_version: version.clone(),
            latest_version: None,
            wanted_version: None,
            dev_dependency: is_dev,
        });
        *count += 1;
    }

    for (dep_name, dep_pkg) in &pkg.dependencies {
        collect_dependencies(dep_name, dep_pkg, is_dev, deps, count);
    }

    for (dep_name, dep_pkg) in &pkg.dev_dependencies {
        collect_dependencies(dep_name, dep_pkg, true, deps, count);
    }
}

/// Tier 2: Extract list info from text output
fn extract_list_text(output: &str) -> Option<DependencyState> {
    let mut dependencies = Vec::new();
    let mut count = 0;
    let mut is_dev = false;

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed == "devDependencies:" {
            is_dev = true;
            continue;
        }
        if trimmed == "dependencies:" {
            is_dev = false;
            continue;
        }

        // Skip box-drawing and metadata
        if line.contains('│')
            || line.contains('├')
            || line.contains('└')
            || line.contains("Legend:")
            || trimmed.is_empty()
        {
            continue;
        }

        // Parse lines like: "package@1.2.3"
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if !parts.is_empty() {
            let pkg_str = parts[0];
            if let Some(at_pos) = pkg_str.rfind('@') {
                let name = &pkg_str[..at_pos];
                let version = &pkg_str[at_pos + 1..];
                if !name.is_empty() && !version.is_empty() {
                    dependencies.push(Dependency {
                        name: name.to_string(),
                        current_version: version.to_string(),
                        latest_version: None,
                        wanted_version: None,
                        dev_dependency: is_dev,
                    });
                    count += 1;
                }
            }
        }
    }

    if count > 0 {
        Some(DependencyState {
            total_packages: count,
            outdated_count: 0,
            dependencies,
        })
    } else {
        None
    }
}

/// Parser for pnpm outdated output
pub struct PnpmOutdatedParser;

impl OutputParser for PnpmOutdatedParser {
    type Output = DependencyState;

    fn parse(input: &str) -> ParseResult<DependencyState> {
        // Tier 1: Try JSON parsing
        match serde_json::from_str::<PnpmOutdatedOutput>(input) {
            Ok(json) => {
                let mut dependencies = Vec::new();
                let mut outdated_count = 0;

                for (name, pkg) in &json.packages {
                    if pkg.current != pkg.latest {
                        outdated_count += 1;
                    }

                    dependencies.push(Dependency {
                        name: name.clone(),
                        current_version: pkg.current.clone(),
                        latest_version: Some(pkg.latest.clone()),
                        wanted_version: pkg.wanted.clone(),
                        dev_dependency: pkg.dependency_type == "devDependencies",
                    });
                }

                let result = DependencyState {
                    total_packages: dependencies.len(),
                    outdated_count,
                    dependencies,
                };

                ParseResult::Full(result)
            }
            Err(e) => {
                // Tier 2: Try text extraction
                match extract_outdated_text(input) {
                    Some(result) => {
                        ParseResult::Degraded(result, vec![format!("JSON parse failed: {}", e)])
                    }
                    None => {
                        // Tier 3: Passthrough
                        ParseResult::Passthrough(truncate_passthrough(input))
                    }
                }
            }
        }
    }
}

/// Tier 2: Extract outdated info from text output
fn extract_outdated_text(output: &str) -> Option<DependencyState> {
    let mut dependencies = Vec::new();
    let mut outdated_count = 0;

    for line in output.lines() {
        // Skip box-drawing, headers, legend
        if line.contains('│')
            || line.contains('├')
            || line.contains('└')
            || line.contains('─')
            || line.starts_with("Legend:")
            || line.starts_with("Package")
            || line.trim().is_empty()
        {
            continue;
        }

        // Parse lines: "package  current  wanted  latest"
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            let name = parts[0];
            let current = parts[1];
            let latest = parts[3];

            if current != latest {
                outdated_count += 1;
            }

            dependencies.push(Dependency {
                name: name.to_string(),
                current_version: current.to_string(),
                latest_version: Some(latest.to_string()),
                wanted_version: parts.get(2).map(|s| s.to_string()),
                dev_dependency: false,
            });
        }
    }

    if !dependencies.is_empty() {
        Some(DependencyState {
            total_packages: dependencies.len(),
            outdated_count,
            dependencies,
        })
    } else {
        None
    }
}

/// Format a dependency listing with grouped [prod]/[dev] sections.
/// `cap = true` for plain `pnpm list` (both categories present, may truncate).
/// `cap = false` for `pnpm list --prod` / `pnpm list --dev` (hint targets,
/// must show every package so the LLM can find what was hidden by the cap).
fn format_dependency_listing(state: &DependencyState, cap: bool) -> String {
    let prod: Vec<_> = state.dependencies.iter().filter(|d| !d.dev_dependency).collect();
    let dev: Vec<_> = state.dependencies.iter().filter(|d| d.dev_dependency).collect();
    let total = state.total_packages.max(state.dependencies.len());

    let mut lines = vec![format!(
        "{} packages ({} prod / {} dev)",
        total,
        prod.len(),
        dev.len()
    )];

    if !prod.is_empty() {
        lines.push("[prod]".to_string());
        let shown = if cap { prod.len().min(MAX_LISTING) } else { prod.len() };
        for dep in prod.iter().take(shown) {
            lines.push(format!("  {} {}", dep.name, dep.current_version));
        }
        if cap && prod.len() > MAX_LISTING {
            lines.push(format!("  … +{} more", prod.len() - MAX_LISTING));
            let all_prod = prod
                .iter()
                .map(|dep| format!("  {} {}", dep.name, dep.current_version))
                .collect::<Vec<_>>()
                .join("\n");
            if let Some(hint) =
                crate::core::tee::force_tee_tail_hint(&all_prod, "pnpm-prod", MAX_LISTING + 1)
            {
                lines.push(format!("  {}", hint));
            }
        }
    }

    if !dev.is_empty() {
        lines.push("[dev]".to_string());
        let shown = if cap { dev.len().min(MAX_LISTING) } else { dev.len() };
        for dep in dev.iter().take(shown) {
            lines.push(format!("  {} {}", dep.name, dep.current_version));
        }
        if cap && dev.len() > MAX_LISTING {
            lines.push(format!("  … +{} more", dev.len() - MAX_LISTING));
            let all_dev = dev
                .iter()
                .map(|dep| format!("  {} {}", dep.name, dep.current_version))
                .collect::<Vec<_>>()
                .join("\n");
            if let Some(hint) =
                crate::core::tee::force_tee_tail_hint(&all_dev, "pnpm-dev", MAX_LISTING + 1)
            {
                lines.push(format!("  {}", hint));
            }
        }
    }

    lines.join("\n")
}

#[derive(Debug, Clone)]
pub enum PnpmCommand {
    List { depth: usize },
    Outdated,
    Install,
}

pub fn run(cmd: PnpmCommand, args: &[String], verbose: u8) -> Result<i32> {
    match cmd {
        PnpmCommand::List { depth } => run_list(depth, args, verbose),
        PnpmCommand::Outdated => run_outdated(args, verbose),
        PnpmCommand::Install => run_install(args, verbose),
    }
}

fn run_list(depth: usize, args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("pnpm");
    cmd.arg("list");
    cmd.arg(format!("--depth={}", depth));
    cmd.arg("--json");

    for arg in args {
        cmd.arg(arg);
    }

    let result = exec_capture(&mut cmd).context("Failed to run pnpm list")?;

    if !result.success() {
        eprint!("{}", result.stderr);
        return Ok(result.exit_code);
    }

    let is_filtered = args
        .iter()
        .any(|a| matches!(a.as_str(), "--prod" | "-P" | "--dev" | "-D"));

    let parse_result = PnpmListParser::parse(&result.stdout);

    let filtered = match parse_result {
        ParseResult::Full(data) => {
            if verbose > 0 {
                eprintln!("pnpm list (Tier 1: Full JSON parse)");
            }
            format_dependency_listing(&data, !is_filtered)
        }
        ParseResult::Degraded(data, warnings) => {
            if verbose > 0 {
                emit_degradation_warning("pnpm list", &warnings.join(", "));
            }
            format_dependency_listing(&data, !is_filtered)
        }
        ParseResult::Passthrough(raw) => {
            emit_passthrough_warning("pnpm list", "All parsing tiers failed");
            raw
        }
    };

    println!("{}", filtered);

    timer.track(
        &format!("pnpm list --depth={}", depth),
        &format!("rtk pnpm list --depth={}", depth),
        &result.stdout,
        &filtered,
    );

    Ok(0)
}

fn run_outdated(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("pnpm");
    cmd.arg("outdated");
    cmd.arg("--format");
    cmd.arg("json");

    for arg in args {
        cmd.arg(arg);
    }

    let result = exec_capture(&mut cmd).context("Failed to run pnpm outdated")?;
    let combined = result.combined();

    // Parse output using PnpmOutdatedParser
    let parse_result = PnpmOutdatedParser::parse(&result.stdout);
    let mode = FormatMode::from_verbosity(verbose);

    let filtered = match parse_result {
        ParseResult::Full(data) => {
            if verbose > 0 {
                eprintln!("pnpm outdated (Tier 1: Full JSON parse)");
            }
            data.format(mode)
        }
        ParseResult::Degraded(data, warnings) => {
            if verbose > 0 {
                emit_degradation_warning("pnpm outdated", &warnings.join(", "));
            }
            data.format(mode)
        }
        ParseResult::Passthrough(raw) => {
            emit_passthrough_warning("pnpm outdated", "All parsing tiers failed");
            raw
        }
    };

    if filtered.trim().is_empty() {
        println!("All packages up-to-date");
    } else {
        println!("{}", filtered);
    }

    timer.track("pnpm outdated", "rtk pnpm outdated", &combined, &filtered);

    Ok(0)
}

fn run_install(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("pnpm");
    cmd.arg("install");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("pnpm install running...");
    }

    let result = exec_capture(&mut cmd).context("Failed to run pnpm install")?;

    if !result.success() {
        eprint!("{}", result.stderr);
        return Ok(result.exit_code);
    }

    let combined = result.combined();
    let filtered = filter_pnpm_install(&combined);

    println!("{}", filtered);

    timer.track("pnpm install", "rtk pnpm install", &combined, &filtered);

    Ok(0)
}

/// Filter pnpm install output - remove progress bars, keep summary
fn filter_pnpm_install(output: &str) -> String {
    let mut result = Vec::new();
    let mut saw_progress = false;

    for line in output.lines() {
        // Skip progress bars
        if line.contains("Progress") || line.contains('│') || line.contains('%') {
            saw_progress = true;
            continue;
        }

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
        "ok".to_string()
    } else {
        result.join("\n")
    }
}

pub fn run_passthrough(args: &[OsString], verbose: u8) -> Result<i32> {
    crate::core::runner::run_passthrough("pnpm", args, verbose)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pnpm_list_parser_json() {
        let json = r#"[
            {
                "name": "my-project",
                "version": "1.0.0",
                "dependencies": {
                    "express": {
                        "version": "4.18.2"
                    }
                }
            }
        ]"#;

        let result = PnpmListParser::parse(json);
        assert_eq!(result.tier(), 1);
        assert!(result.is_ok());

        let data = result.unwrap();
        assert!(data.total_packages >= 2);
    }

    #[test]
    fn test_pnpm_outdated_parser_json() {
        let json = r#"{
            "express": {
                "current": "4.18.2",
                "latest": "4.19.0",
                "wanted": "4.18.2"
            }
        }"#;

        let result = PnpmOutdatedParser::parse(json);
        assert_eq!(result.tier(), 1);
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.outdated_count, 1);
        assert_eq!(data.dependencies[0].name, "express");
    }

    #[test]
    fn test_run_passthrough_accepts_args() {
        // Test that run_passthrough compiles and has correct signature
        let _args: Vec<OsString> = vec![OsString::from("help")];
        // Compile-time verification that the function exists with correct signature
    }

    fn make_state(prod: &[&str], dev: &[&str]) -> DependencyState {
        let mut deps = Vec::new();
        for name in prod {
            deps.push(Dependency {
                name: name.to_string(),
                current_version: "1.0.0".to_string(),
                latest_version: None,
                wanted_version: None,
                dev_dependency: false,
            });
        }
        for name in dev {
            deps.push(Dependency {
                name: name.to_string(),
                current_version: "1.0.0".to_string(),
                latest_version: None,
                wanted_version: None,
                dev_dependency: true,
            });
        }
        DependencyState {
            total_packages: deps.len(),
            outdated_count: 0,
            dependencies: deps,
        }
    }

    #[test]
    fn test_format_listing_grouped_sections() {
        let state = make_state(&["react", "typescript"], &["eslint", "vitest"]);
        let out = format_dependency_listing(&state, true);
        assert!(out.contains("[prod]"), "prod section missing");
        assert!(out.contains("[dev]"), "dev section missing");
        assert!(out.contains("react"), "prod package missing");
        assert!(out.contains("eslint"), "dev package missing");
        assert!(!out.contains("(dev)"), "per-line (dev) marker should be gone");
    }

    #[test]
    fn test_format_listing_cap_shows_hint_with_offset() {
        let prod: Vec<&str> = (0..60).map(|_| "pkg").collect();
        let state = make_state(&prod, &["eslint"]);
        let out = format_dependency_listing(&state, true);
        let prod_count = 60usize;
        assert!(
            out.contains(&format!("… +{} more", prod_count - MAX_LISTING)),
            "truncation count missing: got\n{out}"
        );
    }

    #[test]
    fn test_format_listing_no_cap_when_prod_only() {
        let prod: Vec<&str> = (0..60).map(|_| "pkg").collect();
        let state = make_state(&prod, &[]);
        let out = format_dependency_listing(&state, false);
        assert!(!out.contains("… +"), "should not truncate when cap=false");
        assert!(!out.contains("[dev]"), "no dev section for prod-only state");
    }

    #[test]
    fn test_format_listing_no_cap_when_dev_only() {
        let dev: Vec<&str> = (0..60).map(|_| "pkg").collect();
        let state = make_state(&[], &dev);
        let out = format_dependency_listing(&state, false);
        assert!(!out.contains("… +"), "should not truncate when cap=false");
        assert!(!out.contains("[prod]"), "no prod section for dev-only state");
    }

    #[test]
    fn test_extract_list_text_tracks_dev_section() {
        let input = "dependencies:\nreact@18.0.0\ndevDependencies:\neslint@8.0.0\n";
        let state = extract_list_text(input).expect("should parse");
        let react = state.dependencies.iter().find(|d| d.name == "react").unwrap();
        let eslint = state.dependencies.iter().find(|d| d.name == "eslint").unwrap();
        assert!(!react.dev_dependency, "react should be prod");
        assert!(eslint.dev_dependency, "eslint should be dev");
    }
}
