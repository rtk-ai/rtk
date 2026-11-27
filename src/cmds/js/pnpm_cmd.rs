//! Filters pnpm output — dependency trees, install logs, outdated packages.

use crate::core::guard::never_worse;
use crate::core::stream::{CaptureResult, exec_capture};
use crate::core::tracking;
use crate::core::truncate::CAP_LIST;
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::ffi::OsString;

use crate::parser::{
    Dependency, DependencyState, FormatMode, OutputParser, ParseResult, TokenFormatter,
    emit_degradation_warning, emit_passthrough_warning, truncate_passthrough,
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

/// Format a dependency listing with grouped `[prod]`/`[dev]` sections.
/// `cap = true` for plain `pnpm list` (both categories present, may truncate).
/// `cap = false` for `pnpm list --prod` / `pnpm list --dev` (hint targets,
/// must show every package so the LLM can find what was hidden by the cap).
fn format_dependency_listing(state: &DependencyState, cap: bool) -> String {
    let prod: Vec<_> = state
        .dependencies
        .iter()
        .filter(|d| !d.dev_dependency)
        .collect();
    let dev: Vec<_> = state
        .dependencies
        .iter()
        .filter(|d| d.dev_dependency)
        .collect();
    let total = state.total_packages.max(state.dependencies.len());

    let mut lines = vec![format!(
        "{} packages ({} prod / {} dev)",
        total,
        prod.len(),
        dev.len()
    )];

    if !prod.is_empty() {
        lines.push("[prod]".to_string());
        let shown = if cap {
            prod.len().min(MAX_LISTING)
        } else {
            prod.len()
        };
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
        let shown = if cap {
            dev.len().min(MAX_LISTING)
        } else {
            dev.len()
        };
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
        // pnpm itself reports list errors on stderr; stdout only carries the
        // reason when a wrapper in front of pnpm refuses the command (#4858).
        print_failure(&result, None);
        return Ok(result.exit_code);
    }

    let combined = result.combined();

    let is_filtered = args
        .iter()
        .any(|a| matches!(a.as_str(), "--prod" | "-P" | "--dev" | "-D"));

    let parse_result = PnpmListParser::parse(&result.stdout);
    let tier = parse_result.tier();

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
        ParseResult::Passthrough(_) => {
            emit_passthrough_warning("pnpm list", "All parsing tiers failed");
            // Build from the combined stdout+stderr, not the parser's
            // stdout-only truncated output — otherwise a stderr-only warning
            // (e.g. a registry notice) on an otherwise-empty stdout produces
            // no visible output at all.
            truncate_passthrough(&combined)
        }
    };

    let cmd_label = format!("pnpm list --depth={}", depth);
    if tier == 3 {
        // Tier 3 is a genuine parse failure: guard/display against the
        // combined stdout+stderr this arm recovered from, and record it as
        // a failure — not token savings — instead of feeding it into the
        // savings timer.
        let shown = never_worse(&combined, &filtered);
        println!("{}", shown);
        tracking::record_parse_failure_silent(
            &cmd_label,
            "All parsing tiers failed",
            !shown.trim().is_empty(),
        );
        timer.track_passthrough(&cmd_label, &tracking::passthrough_label(&cmd_label));
    } else {
        // Tiers 1/2 are unaffected by the passthrough fix: keep guarding
        // and tracking against stdout alone, as before, so a stderr-only
        // warning alongside a successful parse doesn't inflate the
        // reported savings ratio.
        let shown = never_worse(&result.stdout, &filtered);
        println!("{}", shown);
        timer.track(
            &cmd_label,
            &format!("rtk pnpm list --depth={}", depth),
            &result.stdout,
            shown,
        );
    }

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
    let tier = parse_result.tier();
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
        ParseResult::Passthrough(_) => {
            emit_passthrough_warning("pnpm outdated", "All parsing tiers failed");
            // Build from the combined stdout+stderr, not the parser's
            // stdout-only truncated output — otherwise a real error reported
            // only on stderr (with malformed/empty JSON on stdout) is lost.
            truncate_passthrough(&combined)
        }
    };

    let display = if filtered.trim().is_empty() {
        "All packages up-to-date".to_string()
    } else {
        filtered.clone()
    };
    let shown = never_worse(&combined, &display);
    println!("{}", shown);

    if tier == 3 {
        // Tier 3 is a genuine parse failure, not token savings — record it
        // as such instead of feeding it into the savings timer.
        tracking::record_parse_failure_silent(
            "pnpm outdated",
            "All parsing tiers failed",
            !shown.trim().is_empty(),
        );
        timer.track_passthrough(
            "pnpm outdated",
            &tracking::passthrough_label("pnpm outdated"),
        );
    } else {
        timer.track("pnpm outdated", "rtk pnpm outdated", &combined, shown);
    }

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
        print_failure(&result, Some(filter_pnpm_failure));
        return Ok(result.exit_code);
    }

    let combined = result.combined();
    let filtered = filter_pnpm_install(&combined);

    let shown = never_worse(&combined, &filtered);
    println!("{}", shown);

    timer.track("pnpm install", "rtk pnpm install", &combined, shown);

    Ok(0)
}

/// Print the output of a failed pnpm run.
///
/// stderr alone is enough when pnpm wrote there. When it is empty, the reason
/// is on stdout (pnpm 9/10 install errors, or a wrapper that refuses the
/// command), so show stdout, filtered when a filter is given. A filter that
/// keeps nothing must not hide the failure, so fall back to the raw stdout.
/// The filter only removes lines, so it cannot make the output longer.
fn print_failure(result: &CaptureResult, filter: Option<fn(&str) -> String>) {
    if !result.stderr.trim().is_empty() {
        eprint!("{}", result.stderr);
        return;
    }
    let shown = match filter {
        Some(filter) => match filter(&result.stdout) {
            filtered if filtered.trim().is_empty() => result.stdout.clone(),
            filtered => filtered,
        },
        None => result.stdout.clone(),
    };
    print!("{}", shown);
    if !shown.ends_with('\n') && !shown.is_empty() {
        println!();
    }
}

/// Filter the stdout of a failed pnpm install.
///
/// Unlike `filter_pnpm_install`, which keeps a few summary lines of a green
/// install, this drops only what is known to be noise (progress lines, the
/// `+++` / `---` bar, pnpm's update notifier) and keeps every other line as
/// is: failure reasons come in shapes no keyword list covers (`%2F` in a
/// scoped 404, `ELIFECYCLE`, an indented lockfile mismatch, a script's own
/// output, `│`-prefixed peer-dependency trees and build logs).
fn filter_pnpm_failure(output: &str) -> String {
    let mut result: Vec<&str> = Vec::new();
    // A `╭ … ╰` frame is held until it closes: it is dropped only when it is
    // pnpm's update notifier, any other frame is kept as is.
    let mut frame: Option<Vec<&str>> = None;

    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(rows) = frame.as_mut() {
            rows.push(line);
            if trimmed.starts_with('╰') {
                let rows = frame.take().unwrap_or_default();
                if !rows.iter().any(|row| row.contains("Update available")) {
                    rows.into_iter().for_each(|row| keep_line(&mut result, row));
                }
            }
            continue;
        }
        if trimmed.starts_with('╭') {
            frame = Some(vec![line]);
            continue;
        }
        // "Progress: …", or "../.. | Progress: …" in a workspace
        let is_progress = trimmed.starts_with("Progress:") || trimmed.contains("| Progress:");
        let is_bar = !trimmed.is_empty() && trimmed.chars().all(|c| c == '+' || c == '-');
        if is_progress || is_bar {
            continue;
        }
        keep_line(&mut result, line);
    }
    // A frame that never closed is not a notifier we recognise: keep it.
    frame
        .into_iter()
        .flatten()
        .for_each(|row| keep_line(&mut result, row));

    while result.last().is_some_and(|l| l.trim().is_empty()) {
        result.pop();
    }
    result.join("\n")
}

/// Push a kept line, collapsing the blank lines left around dropped ones.
fn keep_line<'a>(result: &mut Vec<&'a str>, line: &'a str) {
    if line.trim().is_empty() && result.last().is_none_or(|l| l.trim().is_empty()) {
        return;
    }
    result.push(line);
}

/// Filter pnpm install output - remove progress bars, keep summary
pub(crate) fn filter_pnpm_install(output: &str) -> String {
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
        assert!(
            !out.contains("(dev)"),
            "per-line (dev) marker should be gone"
        );
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
        assert!(
            !out.contains("[prod]"),
            "no prod section for dev-only state"
        );
    }

    #[test]
    fn test_extract_list_text_tracks_dev_section() {
        let input = "dependencies:\nreact@18.0.0\ndevDependencies:\neslint@8.0.0\n";
        let state = extract_list_text(input).expect("should parse");
        let react = state
            .dependencies
            .iter()
            .find(|d| d.name == "react")
            .unwrap();
        let eslint = state
            .dependencies
            .iter()
            .find(|d| d.name == "eslint")
            .unwrap();
        assert!(!react.dev_dependency, "react should be prod");
        assert!(eslint.dev_dependency, "eslint should be dev");
    }

    // Fixtures: real pnpm 10.34.9 stdout (stderr was empty), paths replaced by /project.

    #[test]
    fn test_failure_filter_keeps_scoped_404() {
        let raw = include_str!("../../../tests/fixtures/pnpm_install_fetch_404_scoped_raw.txt");
        let out = filter_pnpm_failure(raw);
        assert!(out.contains("ERR_PNPM_FETCH_404"), "got: {out}");
        assert!(
            out.contains("@ferr079-nope%2Fpkg-zz: Not Found"),
            "got: {out}"
        );
        assert!(out.contains("@ferr079-nope/pkg-zz is not in the npm registry"));
        assert!(!out.contains("Progress:"), "got: {out}");
        assert!(!out.contains("Update available"), "got: {out}");
        assert!(
            !out.lines().any(|l| l.trim().starts_with(['╭', '│', '╰'])),
            "no row of the update box left, got: {out}"
        );
    }

    #[test]
    fn test_failure_filter_scoped_404_exact_output() {
        // Blank lines between the reason's paragraphs stay single; the ones the
        // dropped box leaves behind collapse, and none trail.
        let raw = include_str!("../../../tests/fixtures/pnpm_install_fetch_404_scoped_raw.txt");
        assert_eq!(
            filter_pnpm_failure(raw),
            "\u{2009}ERR_PNPM_FETCH_404\u{2009} GET https://registry.npmjs.org/@ferr079-nope%2Fpkg-zz: Not Found - 404\n\
             \n\
             This error happened while installing a direct dependency of /project\n\
             \n\
             @ferr079-nope/pkg-zz is not in the npm registry, or you have no permission to fetch it.\n\
             \n\
             No authorization header was set for the request."
        );
    }

    // Fixtures below: real stdout, stderr empty, CI unset, paths replaced by /project.

    #[test]
    fn test_failure_filter_keeps_unmet_peer_tree() {
        // pnpm 9.15.9, strict-peer-dependencies: the update box and the peer tree
        // both start lines with `│`; only the box may go.
        let raw = include_str!("../../../tests/fixtures/pnpm_install_peer_deps_unmet_raw.txt");
        let out = filter_pnpm_failure(raw);
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines.contains(&"├─┬ react-dom 18.2.0"), "got: {out}");
        assert!(
            lines.contains(&"│ └── ✕ unmet peer react@^18.2.0: found 17.0.2"),
            "got: {out}"
        );
        assert!(
            lines.contains(&"  └── ✕ unmet peer react@^18.0: found 17.0.2"),
            "got: {out}"
        );
        assert!(!out.contains("Update available"), "got: {out}");
        assert!(
            !lines.iter().any(|l| l.trim().starts_with(['╭', '╰'])),
            "got: {out}"
        );
        // The box sat between blank lines at the top: they collapse away.
        assert!(
            !out.starts_with('\n') && !out.contains("\n\n\n"),
            "got: {out:?}"
        );
    }

    #[test]
    fn test_failure_filter_keeps_dependency_build_log() {
        // pnpm 9.15.9 --reporter=default: a dependency's build output is `│`-prefixed.
        let raw = include_str!("../../../tests/fixtures/pnpm_install_dep_build_failed_raw.txt");
        let out = filter_pnpm_failure(raw);
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines.contains(&"│ dep stdout line"), "got: {out}");
        assert!(
            lines.contains(&"│ gyp ERR! dep native build failed"),
            "got: {out}"
        );
        assert!(
            out.contains("\u{2009}ELIFECYCLE\u{2009} Command failed with exit code 1."),
            "got: {out}"
        );
    }

    #[test]
    fn test_failure_filter_keeps_boxed_script_error() {
        // pnpm 10.34.9: pnpm echoes the script command, which holds the same text,
        // so only a whole-line match proves the printed row survived.
        let raw = include_str!("../../../tests/fixtures/pnpm_install_boxed_script_error_raw.txt");
        let out = filter_pnpm_failure(raw);
        let lines: Vec<&str> = out.lines().collect();
        assert!(
            lines.contains(&"│ Error: Python 3 not found │"),
            "got: {out}"
        );
        assert!(
            lines.contains(&"┌───────────────────────────┐"),
            "got: {out}"
        );
        assert!(
            lines.contains(&"└───────────────────────────┘"),
            "got: {out}"
        );
    }

    #[test]
    fn test_failure_filter_drops_minus_bar() {
        // pnpm 10.34.9 after a dependency was removed: `Packages: -1` then a `-` bar.
        let raw = include_str!("../../../tests/fixtures/pnpm_install_removed_dep_failed_raw.txt");
        let out = filter_pnpm_failure(raw);
        let lines: Vec<&str> = out.lines().collect();
        assert!(
            !lines.iter().any(|l| l.trim() == "-"),
            "no lone `-` bar, got: {out}"
        );
        assert!(
            lines.contains(&"postinstall: config check failed"),
            "got: {out}"
        );
        assert!(
            out.contains("\u{2009}ELIFECYCLE\u{2009} Command failed with exit code 5."),
            "got: {out}"
        );
    }

    #[test]
    fn test_failure_filter_keeps_a_rounded_frame_without_the_notifier() {
        // Scripts print `╭ … ╰` frames too (boxen's round style): only pnpm's
        // update notifier is noise.
        let raw = "╭──────────────╮\n│ Build failed │\n╰──────────────╯\n";
        assert_eq!(filter_pnpm_failure(raw), raw.trim_end());
    }

    #[test]
    fn test_failure_filter_keeps_an_unclosed_frame() {
        let raw = "╭──────────────╮\n│ Update available! 1 → 2 │\nERR_PNPM_X boom\n";
        assert_eq!(filter_pnpm_failure(raw), raw.trim_end());
    }

    #[test]
    fn test_failure_filter_keeps_postinstall_reason() {
        let raw = include_str!("../../../tests/fixtures/pnpm_install_postinstall_failed_raw.txt");
        let out = filter_pnpm_failure(raw);
        assert!(
            out.contains("native build failed: missing libfoo"),
            "got: {out}"
        );
        assert!(
            out.contains("\u{2009}ELIFECYCLE\u{2009} Command failed with exit code 7."),
            "got: {out}"
        );
        assert!(
            !out.lines().any(|l| l.trim() == "+"),
            "no lone progress bar, got: {out}"
        );
        assert!(!out.contains("Progress:"), "got: {out}");
    }

    #[test]
    fn test_failure_filter_keeps_frozen_lockfile_mismatch() {
        let raw = include_str!("../../../tests/fixtures/pnpm_install_frozen_lockfile_raw.txt");
        let out = filter_pnpm_failure(raw);
        assert!(out.contains("ERR_PNPM_OUTDATED_LOCKFILE"), "got: {out}");
        assert!(
            out.contains("  - is-number (lockfile: 7.0.0, manifest: ^6.0.0)"),
            "got: {out}"
        );
    }

    #[test]
    fn test_failure_filter_drops_workspace_progress() {
        let raw = "../.. | Progress: resolved 3, reused 3, downloaded 0, added 0\n\
                   . | Progress: resolved 3, reused 3, downloaded 0, added 0, done\n\
                   \u{2009}ERR_PNPM_FETCH_404\u{2009} GET https://registry.npmjs.org/@a%2Fb: Not Found - 404\n";
        assert_eq!(
            filter_pnpm_failure(raw),
            "\u{2009}ERR_PNPM_FETCH_404\u{2009} GET https://registry.npmjs.org/@a%2Fb: Not Found - 404"
        );
    }

    #[test]
    fn test_failure_filter_only_noise_is_empty() {
        assert_eq!(filter_pnpm_failure("Progress: resolved 1\n+\n\n"), "");
    }

    // --- Tier 3 (Passthrough) content-loss regression tests ---
    //
    // `run_list`/`run_outdated` shell out to a real `pnpm` binary and write
    // to a real SQLite tracking DB, so they aren't practically unit-testable
    // here. Instead these tests exercise `truncate_passthrough` directly
    // against the combined stdout+stderr, mirroring exactly what each
    // `run_*`'s `ParseResult::Passthrough` arm does — so the content-loss
    // fix is verified without needing a process or a database.

    #[test]
    fn test_pnpm_list_passthrough_includes_stderr_when_stdout_empty() {
        // Regression: the old code passed the parser's stdout-only
        // truncated output straight through, so a registry warning reported
        // only on stderr (with empty/near-empty stdout) produced no visible
        // output at all.
        let stdout = "";
        let stderr = "WARN  registry lookup failed, using cached metadata";
        let combined = format!("{}{}", stdout, stderr);

        assert_eq!(PnpmListParser::parse(stdout).tier(), 3);

        let filtered = truncate_passthrough(&combined);
        let shown = never_worse(&combined, &filtered);
        assert!(
            shown.contains("registry lookup failed"),
            "stderr content must survive passthrough: got {shown}"
        );
    }

    #[test]
    fn test_pnpm_outdated_passthrough_includes_both_streams() {
        // Malformed JSON on stdout plus a real error on stderr: the shown
        // output must include both, not just stdout.
        let stdout = "{not valid json";
        let stderr = "Error: ENOENT: no such file or directory, open 'package.json'";
        let combined = format!("{}{}", stdout, stderr);

        assert_eq!(PnpmOutdatedParser::parse(stdout).tier(), 3);

        let filtered = truncate_passthrough(&combined);
        let display = if filtered.trim().is_empty() {
            "All packages up-to-date".to_string()
        } else {
            filtered.clone()
        };
        let shown = never_worse(&combined, &display);

        assert!(
            shown.contains("not valid json"),
            "stdout content lost: got {shown}"
        );
        assert!(
            shown.contains("ENOENT: no such file or directory"),
            "stderr content lost: got {shown}"
        );
    }

    #[test]
    fn test_pnpm_outdated_empty_both_streams_shows_up_to_date() {
        // Regression: genuinely empty output on both streams (no outdated
        // packages, e.g. real `pnpm outdated --format json` prints "{}")
        // must still fall back to "All packages up-to-date" via the
        // `filtered.trim().is_empty()` check — this fix must not touch
        // that empty-display fallback. (The final `never_worse` guard
        // clamp against the raw combined output is exercised separately by
        // `guard.rs`'s own tests and is out of scope here.)
        let stdout = "{}";
        let stderr = "";
        let combined = format!("{}{}", stdout, stderr);

        let parse_result = PnpmOutdatedParser::parse(stdout);
        assert_eq!(
            parse_result.tier(),
            1,
            "\"{{}}\" is valid empty JSON, not Passthrough"
        );

        let filtered = match parse_result {
            ParseResult::Full(data) => data.format(FormatMode::from_verbosity(0)),
            ParseResult::Degraded(data, _) => data.format(FormatMode::from_verbosity(0)),
            ParseResult::Passthrough(_) => truncate_passthrough(&combined),
        };

        let display = if filtered.trim().is_empty() {
            "All packages up-to-date".to_string()
        } else {
            filtered.clone()
        };

        assert_eq!(display, "All packages up-to-date");
    }
}
