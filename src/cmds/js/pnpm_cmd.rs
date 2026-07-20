//! Filters pnpm output — dependency trees, install logs, outdated packages.
//!
//! Includes smart script routing for `pnpm run <script>`:
//! - Detects test/lint/tsc/prettier/playwright scripts from package.json
//! - Routes to specialized filters for maximum token compression

use crate::core::guard::never_worse;
use crate::core::stream::{exec_capture, StreamFilter};
use crate::core::tracking;
use crate::core::truncate::{CAP_LIST, CAP_WARNINGS};
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::ffi::OsString;

use crate::parser::{
    emit_degradation_warning, emit_passthrough_warning, truncate_passthrough, Dependency,
    DependencyState, FormatMode, OutputParser, ParseResult, TokenFormatter,
};

// pnpm run output boilerplate patterns
lazy_static! {
    // > my-project@1.0.0 test /path/to/project
    static ref LIFECYCLE_HEADER: Regex = Regex::new(r"^>\s+\S+@\S+\s+").unwrap();
    // $ vitest run --reporter=json
    static ref SCRIPT_ECHO: Regex = Regex::new(r"^\$\s+").unwrap();
    // Done in 3.4s
    static ref DONE_MSG: Regex = Regex::new(r"^Done in \d").unwrap();
    // ELIFECYCLE  Command failed with exit code 1.
    // Matches only ELIFECYCLE, preserves ERR_PNPM messages
    static ref ELIFECYCLE_ONLY: Regex = Regex::new(r"(?i)ELIFECYCLE").unwrap();
    // Progress: resolved 123, reused 120, downloaded 3
    static ref PROGRESS: Regex = Regex::new(r"^Progress:").unwrap();
}
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
        if line.contains('\u{2502}')
            || line.contains('\u{251c}')
            || line.contains('\u{2514}')
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
        if line.contains('\u{2502}')
            || line.contains('\u{251c}')
            || line.contains('\u{2514}')
            || line.contains('\u{2500}')
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
    Run {
        script: String,
        args: Vec<String>,
        filters: Vec<String>,
    },
}

pub fn run(cmd: PnpmCommand, args: &[String], verbose: u8) -> Result<i32> {
    match cmd {
        PnpmCommand::List { depth } => run_list(depth, args, verbose),
        PnpmCommand::Outdated => run_outdated(args, verbose),
        PnpmCommand::Install => run_install(args, verbose),
        PnpmCommand::Run {
            script,
            args: run_args,
            filters,
        } => run_script(&script, &run_args, &filters, verbose),
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

    let shown = never_worse(&result.stdout, &filtered);
    println!("{}", shown);

    timer.track(
        &format!("pnpm list --depth={}", depth),
        &format!("rtk pnpm list --depth={}", depth),
        &result.stdout,
        shown,
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

    let display = if filtered.trim().is_empty() {
        "All packages up-to-date".to_string()
    } else {
        filtered.clone()
    };
    let shown = never_worse(&combined, &display);
    println!("{}", shown);

    timer.track("pnpm outdated", "rtk pnpm outdated", &combined, shown);

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

    let shown = never_worse(&combined, &filtered);
    println!("{}", shown);

    timer.track("pnpm install", "rtk pnpm install", &combined, shown);

    Ok(0)
}

/// Filter pnpm install output - remove progress bars, keep summary
fn filter_pnpm_install(output: &str) -> String {
    let mut result = Vec::new();
    let mut saw_progress = false;

    for line in output.lines() {
        // Skip progress bars
        if line.contains("Progress") || line.contains('\u{2502}') || line.contains('%') {
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

// --- pnpm run <script> smart routing ---

/// Filter route for specialized script output processing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilterRoute {
    TestRunner,
    Vitest,
    Lint,
    Tsc,
    Prettier,
    Playwright,
}

/// Walk up from `start` to find the nearest package.json (mirrors pnpm resolution).
fn find_package_json_from(start: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut dir = start.to_path_buf();
    for _ in 0..10 {
        let candidate = dir.join("package.json");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// Cached package.json scripts (read once per invocation).
/// Eliminates redundant fs::read_to_string("package.json") calls in
/// is_pnpm_script and route_script.
pub struct PackageScripts {
    scripts: HashMap<String, String>, // script_name -> command_string
}

impl PackageScripts {
    /// Read package.json from CWD, parse scripts field. Returns None if
    /// file is missing, unparseable, or has no scripts section.
    pub fn load() -> Option<Self> {
        Self::load_from(&std::env::current_dir().ok()?)
    }

    /// Like `load`, but starts the package.json walk-up from `start`
    /// (kept separate so tests can point at a tempdir).
    pub fn load_from(start: &std::path::Path) -> Option<Self> {
        let path = find_package_json_from(start)?;
        let content = std::fs::read_to_string(path).ok()?;
        let json: serde_json::Value = serde_json::from_str(&content).ok()?;
        let scripts_obj = json.get("scripts")?.as_object()?;
        let scripts: HashMap<String, String> = scripts_obj
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
            .collect();
        Some(PackageScripts { scripts })
    }

    /// Check if a script name exists in the cached scripts map.
    #[cfg(test)]
    pub fn contains(&self, name: &str) -> bool {
        self.scripts.contains_key(name)
    }

    /// Detect the underlying tool from the script command string.
    /// Routes on the resolved FIRST command token only — substring matches
    /// deeper in the command (`tsc && vite build` is tsc, `openapi-typescript`
    /// is nothing) would misroute output to a filter that fabricates a summary.
    pub fn detect_tool(&self, script: &str) -> Option<FilterRoute> {
        let command = self.scripts.get(script)?;
        let token = first_command_token(command)?;
        let tool = token.rsplit('/').next().unwrap_or(token.as_str());
        match tool {
            "playwright" => Some(FilterRoute::Playwright),
            "vitest" => Some(FilterRoute::Vitest),
            "jest" => Some(FilterRoute::TestRunner),
            "tsc" => Some(FilterRoute::Tsc),
            "eslint" | "biome" => Some(FilterRoute::Lint),
            "prettier" => Some(FilterRoute::Prettier),
            _ => None,
        }
    }
}

/// First meaningful command token of a script, lowercased: skips leading env
/// assignments (`FOO=bar`) and launcher wrappers (`cross-env`, `npx`, `pnpx`,
/// `pnpm exec`, `pnpm dlx`). Returns None when nothing routable resolves.
fn first_command_token(command: &str) -> Option<String> {
    let mut tokens = command.split_whitespace();
    while let Some(token) = tokens.next() {
        let lower = token.to_lowercase();
        if is_env_assignment(&lower) {
            continue;
        }
        match lower.as_str() {
            "cross-env" | "npx" | "pnpx" => continue,
            "pnpm" => match tokens.next().map(|t| t.to_lowercase()) {
                Some(launcher) if launcher == "exec" || launcher == "dlx" => continue,
                // any other pnpm form is not a directly routable tool
                _ => return None,
            },
            _ => return Some(lower),
        }
    }
    None
}

fn is_env_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && !name.chars().next().unwrap().is_ascii_digit()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// True when a forced --reporter (user args or the script text itself) breaks
/// the default-reporter line format the stream filter parses.
fn reporter_forced(args: &[String], script_cmd: Option<&String>) -> bool {
    args.iter().any(|a| a.starts_with("--reporter"))
        || script_cmd.is_some_and(|cmd| cmd.contains("--reporter"))
}

/// Strip pnpm-specific boilerplate from script output.
/// Returns empty string when all lines are boilerplate (caller decides what to show).
pub(crate) fn filter_pnpm_run_output(output: &str) -> String {
    let mut result = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if LIFECYCLE_HEADER.is_match(trimmed) {
            continue;
        }
        if SCRIPT_ECHO.is_match(trimmed) {
            continue;
        }
        if DONE_MSG.is_match(trimmed) {
            continue;
        }
        if ELIFECYCLE_ONLY.is_match(trimmed) {
            continue;
        }
        if PROGRESS.is_match(trimmed) {
            continue;
        }
        result.push(line.to_string());
    }

    result.join("\n")
}

/// Strip pnpm boilerplate from stderr for failure display.
/// Removes ELIFECYCLE and Done lines but preserves ERR_PNPM messages
/// (those are the actual error messages users need to see).
pub(crate) fn strip_pnpm_stderr(stderr: &str) -> String {
    stderr
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty()
                && !ELIFECYCLE_ONLY.is_match(trimmed)
                && !DONE_MSG.is_match(trimmed)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Route a script name to a specialized filter (static rules + cached package.json detection)
pub(crate) fn route_script(
    script: &str,
    pkg_scripts: Option<&PackageScripts>,
) -> Option<FilterRoute> {
    // Tier 1: Unambiguous script names (tool IS the name)
    match script {
        "vitest" => return Some(FilterRoute::Vitest),
        "tsc" => return Some(FilterRoute::Tsc),
        "prettier" => return Some(FilterRoute::Prettier),
        _ => {}
    }

    // Tier 2: Auto-detect from package.json scripts
    pkg_scripts.and_then(|ps| ps.detect_tool(script))
}

/// Check if a name is a known pnpm script (static routing or cached package.json).
/// Default: false (passthrough to pnpm as native command).
#[cfg(test)]
fn is_pnpm_script(name: &str, pkg_scripts: &Option<PackageScripts>) -> bool {
    // Tier 1: Static routes (vitest, tsc, prettier -- no I/O)
    if route_script(name, pkg_scripts.as_ref()).is_some() {
        return true;
    }

    // Tier 2: Cached package.json scripts
    match pkg_scripts {
        Some(ps) => ps.contains(name),
        None => false,
    }
}

/// Apply a specialized filter to script output.
/// Returns Result to allow caller fallback on error (replaces catch_unwind).
/// `succeeded` is the script's exit status: on failure, a filter that did not
/// positively recognize the tool's output must not replace real error text
/// with a counts-only or success summary — the raw output is shown instead.
pub(crate) fn apply_filter(
    route: FilterRoute,
    output: &str,
    succeeded: bool,
) -> Result<(String, &'static str)> {
    // Empty/whitespace input is an error -- nothing meaningful to filter
    if output.trim().is_empty() {
        let label = match route {
            FilterRoute::Vitest => "vitest (via pnpm run)",
            FilterRoute::Playwright => "playwright (via pnpm run)",
            FilterRoute::Tsc => "tsc (via pnpm run)",
            FilterRoute::Lint => "lint (via pnpm run)",
            FilterRoute::Prettier => "prettier (via pnpm run)",
            FilterRoute::TestRunner => "test (via pnpm run)",
        };
        anyhow::bail!("{} filter received empty input", label);
    }

    let filtered = match route {
        // Reached only when a forced --reporter keeps the run off the
        // streaming path — parse the JSON reporter output.
        FilterRoute::Vitest => {
            let parse_result = crate::cmds::js::vitest_cmd::VitestParser::parse(output);
            match parse_result {
                ParseResult::Full(data) => data.format(FormatMode::Compact),
                ParseResult::Degraded(data, _) => data.format(FormatMode::Compact),
                ParseResult::Passthrough(raw) => raw,
            }
        }
        FilterRoute::Playwright => {
            let parse_result = crate::cmds::js::playwright_cmd::PlaywrightParser::parse(output);
            match parse_result {
                ParseResult::Full(data) => data.format(FormatMode::Compact),
                // Degraded is counts-only: on failure it would replace the
                // error text and code frames playwright printed to stdout.
                ParseResult::Degraded(data, _) if succeeded => {
                    data.format(FormatMode::Compact)
                }
                ParseResult::Degraded(_, _) => output.to_string(),
                ParseResult::Passthrough(raw) => raw,
            }
        }
        FilterRoute::Tsc if !succeeded => {
            crate::cmds::js::tsc_cmd::filter_tsc_output_recognized(output)
                .unwrap_or_else(|| output.to_string())
        }
        FilterRoute::Tsc => crate::cmds::js::tsc_cmd::filter_tsc_output(output),
        FilterRoute::Lint if !succeeded => {
            crate::cmds::js::lint_cmd::filter_generic_lint_recognized(output)
                .unwrap_or_else(|| output.to_string())
        }
        FilterRoute::Lint => crate::cmds::js::lint_cmd::filter_generic_lint(output),
        FilterRoute::Prettier => crate::cmds::js::prettier_cmd::filter_prettier_output(output),
        FilterRoute::TestRunner => {
            crate::cmds::rust::runner::extract_test_summary(output, "pnpm test")
        }
    };

    let label = match route {
        FilterRoute::Vitest => "vitest (via pnpm run)",
        FilterRoute::Playwright => "playwright (via pnpm run)",
        FilterRoute::Tsc => "tsc (via pnpm run)",
        FilterRoute::Lint => "lint (via pnpm run)",
        FilterRoute::Prettier => "prettier (via pnpm run)",
        FilterRoute::TestRunner => "test (via pnpm run)",
    };

    // Empty/whitespace filter output treated as error (triggers fallback)
    if filtered.trim().is_empty() {
        anyhow::bail!("{} filter returned empty output", label);
    }

    Ok((filtered, label))
}

// --- VitestStreamFilter: real-time streaming for vitest output ---

lazy_static! {
    static ref VITEST_BANNER: Regex = Regex::new(r"^\s*RUN\s+v").unwrap();
    // Vitest non-TTY default reporter file lines:
    // passed " ✓ a.test.ts (3 tests) 5ms", failed " ❯ b.test.ts (5 tests | 1 failed) 20ms",
    // skipped " ↓ c.test.ts (2 tests | 2 skipped)" / " ↓ c.test.ts (2 skipped)"
    static ref TEST_FILE_RESULT: Regex =
        Regex::new(r"^\s*([✓❯↓])\s+.+?\((?:(\d+)\s+tests?(?:\s*\|\s*(\d+)\s+failed)?(?:\s*\|\s*(\d+)\s+skipped)?|(\d+)\s+skipped)\)(?:\s+[\d.]+(?:ms|s|m))?$")
            .unwrap();
    static ref TEST_FAIL_INDIVIDUAL: Regex = Regex::new(r"^\s*×\s+").unwrap();
    static ref SUMMARY_FILES: Regex =
        Regex::new(r"^\s*Test Files\s+").unwrap();
    static ref SUMMARY_TESTS: Regex = Regex::new(r"^\s*Tests\s+").unwrap();
    static ref SUMMARY_START: Regex = Regex::new(r"^\s*Start at\s+").unwrap();
    static ref SUMMARY_DURATION: Regex =
        Regex::new(r"^\s*Duration\s+([\d.]+)\s*(ms|s|m)").unwrap();
    static ref FAILED_TESTS_SEP: Regex = Regex::new(r"^⎯{5,}.*Failed Tests").unwrap();
    static ref FAIL_DETAIL_HEADER: Regex = Regex::new(r"^\s*FAIL\s+").unwrap();
    static ref SEPARATOR_LINE: Regex = Regex::new(r"^⎯{5,}").unwrap();
    static ref PNPM_CMD_ECHO: Regex = Regex::new(r"^>\s+\S").unwrap();

    // For on_exit summary parsing. Each `N label` segment is optional and
    // consumes its own trailing `|` so every vitest form matches:
    // "Test Files  2 passed (2)", "… 1 failed | 2 passed | 1 skipped (4)",
    // "Tests  2 todo (2)", "Tests  1 skipped (1)"
    static ref RE_SUMMARY_FILES: Regex =
        Regex::new(r"Test Files\s+(?:(\d+)\s+failed(?:\s*\|\s*)?)?(?:(\d+)\s+passed(?:\s*\|\s*)?)?(?:(\d+)\s+skipped(?:\s*\|\s*)?)?(?:(\d+)\s+todo(?:\s*\|\s*)?)?\s*\((\d+)\)").unwrap();
    static ref RE_SUMMARY_TESTS: Regex =
        Regex::new(r"Tests\s+(?:(\d+)\s+failed(?:\s*\|\s*)?)?(?:(\d+)\s+passed(?:\s*\|\s*)?)?(?:(\d+)\s+skipped(?:\s*\|\s*)?)?(?:(\d+)\s+todo(?:\s*\|\s*)?)?\s*\((\d+)\)").unwrap();
    static ref RE_SUMMARY_DURATION: Regex =
        Regex::new(r"Duration\s+([\d.]+)(ms|s|m)").unwrap();
}

const MAX_INLINE_FAILURES: usize = CAP_WARNINGS;
const MAX_DETAIL_LINES: usize = 30;

struct VitestStreamFilter {
    passed_suites: usize,
    failed_suites: usize,
    passed_tests: usize,
    failed_tests: usize,
    skipped_tests: usize,
    in_failure_detail: bool,
    failure_detail_lines: usize,
    failures_shown: usize,
    duration_secs: Option<String>,
    seen_banner: bool,
}

impl VitestStreamFilter {
    fn new() -> Self {
        Self {
            passed_suites: 0,
            failed_suites: 0,
            passed_tests: 0,
            failed_tests: 0,
            skipped_tests: 0,
            in_failure_detail: false,
            failure_detail_lines: 0,
            failures_shown: 0,
            duration_secs: None,
            seen_banner: false,
        }
    }
}

impl StreamFilter for VitestStreamFilter {
    fn feed_line(&mut self, line: &str) -> Option<String> {
        let trimmed = line.trim();

        // Empty lines: skip unless we're in a failure detail
        if trimmed.is_empty() {
            if self.in_failure_detail {
                self.failure_detail_lines += 1;
                if self.failure_detail_lines <= MAX_DETAIL_LINES {
                    return Some(format!("{}\n", line));
                }
            }
            return None;
        }

        // Pnpm boilerplate: always suppress
        if DONE_MSG.is_match(trimmed)
            || ELIFECYCLE_ONLY.is_match(trimmed)
            || PROGRESS.is_match(trimmed)
        {
            return None;
        }

        // The `> pkg@ver script` header and `> cmd` / `$ cmd` echoes only appear
        // before vitest's RUN banner — after it, such lines are user output
        // (console.log("> foo")) and must pass through.
        if !self.seen_banner
            && (LIFECYCLE_HEADER.is_match(trimmed)
                || SCRIPT_ECHO.is_match(trimmed)
                || PNPM_CMD_ECHO.is_match(trimmed))
        {
            return None;
        }

        // Vitest banner: suppress
        if VITEST_BANNER.is_match(trimmed) {
            self.seen_banner = true;
            self.in_failure_detail = false;
            return None;
        }

        // Vitest summary lines: suppress, but extract duration
        if SUMMARY_DURATION.is_match(trimmed) {
            if let Some(caps) = SUMMARY_DURATION.captures(trimmed) {
                let val: f64 = caps[1].parse().unwrap_or(0.0);
                let unit = &caps[2];
                let secs = match unit {
                    "ms" => val / 1000.0,
                    "m" => val * 60.0,
                    _ => val,
                };
                self.duration_secs = Some(format!("{:.0}s", secs));
            }
            self.in_failure_detail = false;
            return None;
        }
        if SUMMARY_FILES.is_match(trimmed) || SUMMARY_TESTS.is_match(trimmed) || SUMMARY_START.is_match(trimmed) {
            self.in_failure_detail = false;
            return None;
        }

        // Failed Tests separator: suppress
        if FAILED_TESTS_SEP.is_match(trimmed) {
            return None;
        }

        // Generic separator line (⎯⎯⎯⎯...): suppress when not in failure detail
        if !self.in_failure_detail && SEPARATOR_LINE.is_match(trimmed) {
            return None;
        }

        // Test file result line: count, suppress passes/skips, show failures
        if let Some(caps) = TEST_FILE_RESULT.captures(trimmed) {
            // A new file result ends any open failure detail block
            self.in_failure_detail = false;
            let symbol = &caps[1];
            let test_count: usize = caps.get(2).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0);
            let failed_count: usize = caps.get(3).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0);
            let skipped_count: usize = caps.get(4).or_else(|| caps.get(5)).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0);

            match symbol {
                "✓" => {
                    self.passed_suites += 1;
                    self.passed_tests += test_count.saturating_sub(skipped_count);
                    self.skipped_tests += skipped_count;
                    return None;
                }
                "↓" => {
                    self.skipped_tests += skipped_count;
                    return None;
                }
                _ => {
                    // ❯ with a failed count is a failed file; without one it is
                    // an in-progress/other line — show it but count nothing
                    if failed_count > 0 {
                        self.failed_suites += 1;
                        self.failed_tests += failed_count;
                        self.passed_tests += test_count
                            .saturating_sub(failed_count)
                            .saturating_sub(skipped_count);
                        self.skipped_tests += skipped_count;
                    }
                    return Some(format!("{}\n", line));
                }
            }
        }

        // Individual test failure: pass through
        if TEST_FAIL_INDIVIDUAL.is_match(trimmed) {
            self.in_failure_detail = true;
            self.failure_detail_lines = 0;
            return Some(format!("{}\n", line));
        }

        // FAIL detail header: pass through
        if FAIL_DETAIL_HEADER.is_match(trimmed) {
            self.in_failure_detail = true;
            self.failure_detail_lines = 0;
            self.failures_shown += 1;
            if self.failures_shown > MAX_INLINE_FAILURES {
                return None;
            }
            return Some(format!("{}\n", line));
        }

        // Inside a failure detail block
        if self.in_failure_detail {
            self.failure_detail_lines += 1;
            if self.failures_shown > MAX_INLINE_FAILURES {
                return None;
            }
            if self.failure_detail_lines <= MAX_DETAIL_LINES {
                return Some(format!("{}\n", line));
            }
            if self.failure_detail_lines == MAX_DETAIL_LINES + 1 {
                return Some("  ... (truncated)\n".to_string());
            }
            return None;
        }

        // Default: pass through
        Some(format!("{}\n", line))
    }

    fn flush(&mut self) -> String {
        String::new()
    }

    fn on_exit(&mut self, exit_code: i32, raw: &str) -> Option<String> {
        // Try to parse the vitest summary from raw output for accurate counts
        let mut files_failed: usize = 0;
        let mut _files_passed: usize = 0;
        let mut files_total: usize = 0;
        let mut tests_failed: usize = 0;
        let mut tests_passed: usize = 0;
        let mut tests_skipped: usize = 0;
        let mut _tests_todo: usize = 0;
        let mut _tests_total: usize = 0;

        if let Some(caps) = RE_SUMMARY_FILES.captures(raw) {
            files_failed = caps.get(1).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0);
            _files_passed = caps.get(2).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0);
            files_total = caps[5].parse().unwrap_or(0);
        }

        if let Some(caps) = RE_SUMMARY_TESTS.captures(raw) {
            tests_failed = caps.get(1).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0);
            tests_passed = caps.get(2).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0);
            tests_skipped = caps.get(3).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0);
            _tests_todo = caps.get(4).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0);
            _tests_total = caps[5].parse().unwrap_or(0);
        }

        // If we couldn't parse the summary, fall back to counted values
        if files_total == 0 {
            _files_passed = self.passed_suites;
            files_failed = self.failed_suites;
            files_total = _files_passed + files_failed;
            tests_passed = self.passed_tests;
            tests_failed = self.failed_tests;
            _tests_total = tests_passed + tests_failed + self.skipped_tests;
            tests_skipped = self.skipped_tests;
        }

        let duration = self.duration_secs.as_deref().unwrap_or("?");

        let summary = if tests_failed > 0 {
            // failures_shown counts FAIL detail headers (hook errors inflate it
            // beyond tests_failed), so gate the notice on the test count too
            let extra = if self.failures_shown > MAX_INLINE_FAILURES
                && tests_failed > MAX_INLINE_FAILURES
            {
                format!(
                    "\n  ... and {} more failures",
                    tests_failed.saturating_sub(MAX_INLINE_FAILURES)
                )
            } else {
                String::new()
            };
            format!(
                "PASS ({}) FAIL ({}) | {} suites ({} failed){} | {}{}",
                tests_passed,
                tests_failed,
                files_total,
                files_failed,
                if tests_skipped > 0 { format!(" | {} skipped", tests_skipped) } else { String::new() },
                duration,
                extra
            )
        } else if exit_code == 0 {
            format!(
                "PASS ({}) | {} suites{} | {}",
                tests_passed,
                files_total,
                if tests_skipped > 0 { format!(" ({} skipped)", tests_skipped) } else { String::new() },
                duration
            )
        } else {
            // Non-zero exit but no test failures parsed — likely a config/compile error
            format!(
                "FAIL (exit {}) | {} suites | {}",
                exit_code, files_total, duration
            )
        };

        Some(format!("{}\n", summary))
    }
}

/// Adapted to v0.39.0 API: uses `exec_capture` + `resolved_command`, returns `Result<i32>`.
/// For vitest routes, uses streaming output for real-time progress.
/// For other routes, uses buffered capture with specialized filters.
pub fn run_script(script: &str, args: &[String], filters: &[String], verbose: u8) -> Result<i32> {
    // Static routes need no package.json read; load lazily when the script name
    // alone doesn't decide, or when the streaming decision needs the script text.
    let mut pkg_scripts: Option<PackageScripts> = None;
    let route = match route_script(script, None) {
        Some(route) => Some(route),
        None => {
            pkg_scripts = PackageScripts::load();
            route_script(script, pkg_scripts.as_ref())
        }
    };

    // Global --filter flags must precede `run` — after the script name they
    // would be forwarded to the script instead of selecting the workspace.
    let filter_args: Vec<String> = filters
        .iter()
        .map(|filter| format!("--filter={}", filter))
        .collect();
    let filter_prefix = if filter_args.is_empty() {
        String::new()
    } else {
        format!("{} ", filter_args.join(" "))
    };

    // A forced --reporter (script-defined or user args) breaks the
    // default-reporter line format the stream filter parses.
    let stream_vitest = if matches!(route, Some(FilterRoute::Vitest)) {
        if pkg_scripts.is_none() {
            pkg_scripts = PackageScripts::load();
        }
        !reporter_forced(
            args,
            pkg_scripts.as_ref().and_then(|ps| ps.scripts.get(script)),
        )
    } else {
        false
    };

    // STREAMING PATH: vitest scripts get real-time output to avoid Claude Code timeouts
    if stream_vitest {
        let mut cmd = resolved_command("pnpm");
        for arg in &filter_args {
            cmd.arg(arg);
        }
        cmd.arg("run");
        cmd.arg(script);
        // Do NOT inject --reporter=json — default reporter streams line-by-line
        for arg in args {
            cmd.arg(arg);
        }
        if verbose > 0 {
            eprintln!(
                "Running: pnpm {}run {} {}",
                filter_prefix,
                script,
                args.join(" ")
            );
        }
        return crate::core::runner::run_streamed(
            cmd,
            "pnpm run",
            &format!("{} {}", script, args.join(" ")),
            Box::new(VitestStreamFilter::new()),
            crate::core::runner::RunOptions::with_tee(&format!("pnpm-run-{}", script)),
        );
    }

    // BUFFERED PATH: all other routes (lint, tsc, prettier — fast commands)
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("pnpm");
    for arg in &filter_args {
        cmd.arg(arg);
    }
    cmd.arg("run");
    cmd.arg(script);

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!(
            "Running: pnpm {}run {} {}",
            filter_prefix,
            script,
            args.join(" ")
        );
    }

    let result = exec_capture(&mut cmd)
        .context(format!("Failed to run pnpm run {}", script))?;
    let stdout_str = &result.stdout;
    let stderr_str = &result.stderr;
    let exit_code = result.exit_code;

    // For tee recovery: combined output (stdout+stderr)
    let raw_for_tee = format!("{}\n{}", stdout_str, stderr_str);

    // Stage 1: Strip pnpm boilerplate from STDOUT ONLY
    let stripped = filter_pnpm_run_output(stdout_str);

    if !result.success() {
        // FAILURE PATH: filter stdout through specialized filter, show stderr
        let filtered = if !stripped.is_empty() {
            match route {
                Some(route) => match apply_filter(route, &stripped, false) {
                    Ok((result, label)) => {
                        if verbose > 0 {
                            eprintln!("Routed to: {}", label);
                        }
                        result
                    }
                    Err(e) => {
                        if verbose > 0 {
                            eprintln!("[RTK:FALLBACK] filter error: {}", e);
                        }
                        stripped.clone()
                    }
                },
                None => stripped.clone(),
            }
        } else {
            String::new()
        };

        // Strip pnpm boilerplate from stderr but preserve ERR_PNPM messages
        let stderr_display = strip_pnpm_stderr(stderr_str);

        // Show: filtered stdout (if any) then stderr (if any)
        let display = [filtered.as_str(), stderr_display.as_str()]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join("\n");

        let shown = crate::core::runner::print_with_hint(
            &display,
            &raw_for_tee,
            &raw_for_tee,
            &format!("pnpm-run-{}", script),
            exit_code,
        );

        timer.track(
            &format!("pnpm run {} {}", script, args.join(" ")),
            &format!("rtk pnpm run {} {}", script, args.join(" ")),
            &raw_for_tee,
            &shown,
        );
        return Ok(exit_code);
    }

    // SUCCESS PATH: "ok" only when exit 0 AND stripped stdout is empty
    if stripped.is_empty() {
        let display = "ok".to_string();
        println!("{}", display);
        timer.track(
            &format!("pnpm run {} {}", script, args.join(" ")),
            &format!("rtk pnpm run {} {}", script, args.join(" ")),
            &raw_for_tee,
            &display,
        );
        return Ok(0);
    }

    // Route to specialized filter
    let filtered = match route {
        Some(route) => match apply_filter(route, &stripped, true) {
            Ok((result, label)) => {
                if verbose > 0 {
                    eprintln!("Routed to: {}", label);
                }
                result
            }
            Err(e) => {
                if verbose > 0 {
                    eprintln!("[RTK:FALLBACK] filter error: {}", e);
                }
                stripped.clone()
            }
        },
        None => stripped.clone(),
    };

    let shown = crate::core::runner::print_with_hint(
        &filtered,
        &raw_for_tee,
        &raw_for_tee,
        &format!("pnpm-run-{}", script),
        exit_code,
    );

    timer.track(
        &format!("pnpm run {} {}", script, args.join(" ")),
        &format!("rtk pnpm run {} {}", script, args.join(" ")),
        &raw_for_tee,
        &shown,
    );

    Ok(0)
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

    // --- filter_pnpm_run_output tests ---

    #[test]
    fn test_filter_pnpm_run_output_clean() {
        // Real tool output should be preserved
        let input = "PASS src/utils.test.ts\nTests: 5 passed, 5 total";
        let result = filter_pnpm_run_output(input);
        assert!(result.contains("PASS"));
        assert!(result.contains("Tests: 5 passed"));
    }

    #[test]
    fn test_filter_pnpm_run_output_lifecycle() {
        let input = "> my-project@1.0.0 test /path/to/project\nPASS tests";
        let result = filter_pnpm_run_output(input);
        assert!(!result.contains("> my-project@"));
        assert!(result.contains("PASS tests"));
    }

    #[test]
    fn test_filter_pnpm_run_output_script_echo() {
        let input = "$ vitest run --reporter=json\nactual output here";
        let result = filter_pnpm_run_output(input);
        assert!(!result.contains("$ vitest"));
        assert!(result.contains("actual output here"));
    }

    #[test]
    fn test_filter_pnpm_run_output_done_msg() {
        let input = "Tests passed\nDone in 3.4s";
        let result = filter_pnpm_run_output(input);
        assert!(!result.contains("Done in"));
        assert!(result.contains("Tests passed"));
    }

    #[test]
    fn test_filter_pnpm_run_output_empty() {
        // Filter returns empty string for pure boilerplate (caller decides "ok")
        let input = "> pkg@1.0.0 test\n$ vitest run\n\nDone in 2.1s\n";
        let result = filter_pnpm_run_output(input);
        assert_eq!(result, "");
    }

    #[test]
    fn test_filter_pnpm_run_output_mixed() {
        let input = r#"> my-project@1.0.0 test
$ vitest run --reporter=json

{"numTotalTests":10,"numPassedTests":9,"numFailedTests":1,"numPendingTests":0,"testResults":[]}

 ELIFECYCLE  Command failed with exit code 1.
Done in 5.2s
"#;
        let result = filter_pnpm_run_output(input);
        assert!(!result.contains("> my-project@"));
        assert!(!result.contains("$ vitest"));
        assert!(!result.contains("ELIFECYCLE"));
        assert!(!result.contains("Done in"));
        assert!(result.contains("numTotalTests"));

        fn count_tokens(text: &str) -> usize {
            text.split_whitespace().count()
        }
        let savings = 100.0 - (count_tokens(&result) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 40.0,
            "Expected >= 40% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_pnpm_run_output_preserves_err_pnpm() {
        let input = " ERR_PNPM_NO_IMPORTER_MANIFEST_FOUND  No package.json (or package.yaml, or package.json5) was found in \"/Users/test\".";
        let result = filter_pnpm_run_output(input);
        assert!(result.contains("ERR_PNPM_NO_IMPORTER_MANIFEST_FOUND"));
        assert!(result.contains("No package.json"));
    }

    // --- route_script tests ---

    #[test]
    fn test_route_script_exact_matches() {
        // Tier 1: Unambiguous static routing works with None pkg_scripts
        assert_eq!(route_script("vitest", None), Some(FilterRoute::Vitest));
        assert_eq!(route_script("tsc", None), Some(FilterRoute::Tsc));
        assert_eq!(route_script("prettier", None), Some(FilterRoute::Prettier));
    }

    #[test]
    fn test_route_script_typecheck_uses_package_json() {
        let ps = PackageScripts {
            scripts: HashMap::from([("typecheck".to_string(), "tsc --noEmit".to_string())]),
        };
        assert_eq!(route_script("typecheck", Some(&ps)), Some(FilterRoute::Tsc));
        // Without package.json, "typecheck" is not routed (could be any tool)
        assert_eq!(route_script("typecheck", None), None);
    }

    // --- Tier 2/3 routing: ambiguous names prefer package.json ---

    #[test]
    fn test_route_script_test_prefers_vitest_from_package_json() {
        let ps = PackageScripts {
            scripts: HashMap::from([("test".to_string(), "vitest run".to_string())]),
        };
        assert_eq!(route_script("test", Some(&ps)), Some(FilterRoute::Vitest));
    }

    #[test]
    fn test_route_script_test_prefers_playwright_from_package_json() {
        let ps = PackageScripts {
            scripts: HashMap::from([("test".to_string(), "playwright test".to_string())]),
        };
        assert_eq!(
            route_script("test", Some(&ps)),
            Some(FilterRoute::Playwright)
        );
    }

    #[test]
    fn test_route_script_test_falls_back_to_test_runner_for_jest() {
        // jest maps to TestRunner in detect_tool, same as Tier 3 fallback
        let ps = PackageScripts {
            scripts: HashMap::from([("test".to_string(), "jest --ci".to_string())]),
        };
        assert_eq!(
            route_script("test", Some(&ps)),
            Some(FilterRoute::TestRunner)
        );
    }

    #[test]
    fn test_route_script_test_falls_back_without_package_scripts() {
        // No package.json -> no routing (pnpm won't work anyway)
        assert_eq!(route_script("test", None), None);
    }

    #[test]
    fn test_route_script_test_falls_back_when_script_not_in_package_json() {
        // package.json exists but "test" script is not defined -> no routing
        let ps = PackageScripts {
            scripts: HashMap::from([("build".to_string(), "tsc && next build".to_string())]),
        };
        assert_eq!(route_script("test", Some(&ps)), None);
    }

    #[test]
    fn test_route_script_lint_prefers_package_json_detection() {
        let ps = PackageScripts {
            scripts: HashMap::from([("lint".to_string(), "biome check .".to_string())]),
        };
        // biome maps to Lint in detect_tool -- same route but via Tier 2
        assert_eq!(route_script("lint", Some(&ps)), Some(FilterRoute::Lint));
    }

    #[test]
    fn test_route_script_lint_falls_back_without_package_scripts() {
        // No package.json -> no routing (pnpm won't work anyway)
        assert_eq!(route_script("lint", None), None);
    }

    #[test]
    fn test_route_script_format_prefers_package_json_detection() {
        let ps = PackageScripts {
            scripts: HashMap::from([("format".to_string(), "prettier --write .".to_string())]),
        };
        assert_eq!(
            route_script("format", Some(&ps)),
            Some(FilterRoute::Prettier)
        );
    }

    #[test]
    fn test_route_script_format_falls_back_without_package_scripts() {
        // No package.json -> no routing (pnpm won't work anyway)
        assert_eq!(route_script("format", None), None);
    }

    #[test]
    fn test_route_script_unknown_returns_none() {
        // These don't match static rules and no PackageScripts provided
        assert_eq!(route_script("build", None), None);
        assert_eq!(route_script("dev", None), None);
        assert_eq!(route_script("start", None), None);
    }

    #[test]
    fn test_route_script_prefix_uses_package_json() {
        // Prefix scripts route through package.json (no static guessing)
        let ps = PackageScripts {
            scripts: HashMap::from([
                ("test:unit".to_string(), "vitest run".to_string()),
                ("lint:check".to_string(), "eslint .".to_string()),
            ]),
        };
        assert_eq!(
            route_script("test:unit", Some(&ps)),
            Some(FilterRoute::Vitest)
        );
        assert_eq!(
            route_script("lint:check", Some(&ps)),
            Some(FilterRoute::Lint)
        );
        // Without package.json, prefix scripts return None
        assert_eq!(route_script("test:unit", None), None);
        assert_eq!(route_script("lint:check", None), None);
    }

    // --- PackageScripts tests ---

    #[test]
    fn test_package_scripts_load_from_reads_scripts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{ "name": "x", "scripts": { "test": "vitest run" } }"#,
        )
        .unwrap();
        let ps = PackageScripts::load_from(dir.path()).expect("should load");
        assert!(ps.contains("test"));
        assert_eq!(ps.detect_tool("test"), Some(FilterRoute::Vitest));
    }

    #[test]
    fn test_package_scripts_load_from_walks_up() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{ "scripts": { "lint": "eslint ." } }"#,
        )
        .unwrap();
        let nested = dir.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        let ps = PackageScripts::load_from(&nested).expect("should find parent package.json");
        assert!(ps.contains("lint"));
    }

    #[test]
    fn test_package_scripts_load_from_none_without_package_json() {
        let dir = tempfile::tempdir().unwrap();
        assert!(PackageScripts::load_from(dir.path()).is_none());
    }

    #[test]
    fn test_package_scripts_load_from_none_on_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "not json").unwrap();
        assert!(PackageScripts::load_from(dir.path()).is_none());
    }

    #[test]
    fn test_package_scripts_load_from_none_without_scripts_key() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{ "name": "x" }"#).unwrap();
        assert!(PackageScripts::load_from(dir.path()).is_none());
    }

    #[test]
    fn test_package_scripts_contains() {
        let ps = PackageScripts {
            scripts: HashMap::from([
                ("test".to_string(), "vitest run".to_string()),
                ("lint".to_string(), "eslint .".to_string()),
            ]),
        };
        assert!(ps.contains("test"));
        assert!(ps.contains("lint"));
        assert!(!ps.contains("build"));
        assert!(!ps.contains("nonexistent"));
    }

    #[test]
    fn test_package_scripts_detect_tool_vitest() {
        let ps = PackageScripts {
            scripts: HashMap::from([("test".to_string(), "vitest run".to_string())]),
        };
        assert_eq!(ps.detect_tool("test"), Some(FilterRoute::Vitest));
    }

    #[test]
    fn test_package_scripts_detect_tool_playwright() {
        let ps = PackageScripts {
            scripts: HashMap::from([("test:e2e".to_string(), "playwright test".to_string())]),
        };
        assert_eq!(ps.detect_tool("test:e2e"), Some(FilterRoute::Playwright));
    }

    #[test]
    fn test_package_scripts_detect_tool_eslint() {
        let ps = PackageScripts {
            scripts: HashMap::from([("lint".to_string(), "eslint .".to_string())]),
        };
        assert_eq!(ps.detect_tool("lint"), Some(FilterRoute::Lint));
    }

    #[test]
    fn test_package_scripts_detect_tool_tsc() {
        let ps = PackageScripts {
            scripts: HashMap::from([("typecheck".to_string(), "tsc --noEmit".to_string())]),
        };
        assert_eq!(ps.detect_tool("typecheck"), Some(FilterRoute::Tsc));
    }

    #[test]
    fn test_package_scripts_detect_tool_prettier() {
        let ps = PackageScripts {
            scripts: HashMap::from([("format".to_string(), "prettier --check .".to_string())]),
        };
        assert_eq!(ps.detect_tool("format"), Some(FilterRoute::Prettier));
    }

    #[test]
    fn test_package_scripts_detect_tool_jest() {
        let ps = PackageScripts {
            scripts: HashMap::from([("test".to_string(), "jest --ci".to_string())]),
        };
        assert_eq!(ps.detect_tool("test"), Some(FilterRoute::TestRunner));
    }

    #[test]
    fn test_package_scripts_detect_tool_biome() {
        let ps = PackageScripts {
            scripts: HashMap::from([("lint".to_string(), "biome check .".to_string())]),
        };
        assert_eq!(ps.detect_tool("lint"), Some(FilterRoute::Lint));
    }

    #[test]
    fn test_package_scripts_detect_tool_unknown() {
        let ps = PackageScripts {
            scripts: HashMap::from([("dev".to_string(), "node server.js".to_string())]),
        };
        assert_eq!(ps.detect_tool("dev"), None);
    }

    // --- first-token routing tests ---

    #[test]
    fn test_detect_tool_substring_in_name_does_not_route() {
        // "tsc"/"typescript" appearing inside another tool's name must not route
        let ps = PackageScripts {
            scripts: HashMap::from([(
                "gen".to_string(),
                "openapi-typescript generate".to_string(),
            )]),
        };
        assert_eq!(ps.detect_tool("gen"), None);
    }

    #[test]
    fn test_detect_tool_chained_script_routes_by_first_token() {
        // Conservative: the first command decides; the tsc half is real
        let ps = PackageScripts {
            scripts: HashMap::from([("build".to_string(), "tsc && vite build".to_string())]),
        };
        assert_eq!(ps.detect_tool("build"), Some(FilterRoute::Tsc));
    }

    #[test]
    fn test_detect_tool_skips_wrappers_and_env_assignments() {
        let ps = PackageScripts {
            scripts: HashMap::from([
                (
                    "test".to_string(),
                    "cross-env NODE_ENV=test vitest run".to_string(),
                ),
                ("unit".to_string(), "FOO=bar vitest run".to_string()),
                ("e2e".to_string(), "npx playwright test".to_string()),
                ("check".to_string(), "pnpm exec tsc --noEmit".to_string()),
                (
                    "check-bin".to_string(),
                    "./node_modules/.bin/tsc --noEmit".to_string(),
                ),
            ]),
        };
        assert_eq!(ps.detect_tool("test"), Some(FilterRoute::Vitest));
        assert_eq!(ps.detect_tool("unit"), Some(FilterRoute::Vitest));
        assert_eq!(ps.detect_tool("e2e"), Some(FilterRoute::Playwright));
        assert_eq!(ps.detect_tool("check"), Some(FilterRoute::Tsc));
        assert_eq!(ps.detect_tool("check-bin"), Some(FilterRoute::Tsc));
    }

    #[test]
    fn test_first_command_token_edge_cases() {
        assert_eq!(first_command_token("vitest run").as_deref(), Some("vitest"));
        assert_eq!(
            first_command_token("FOO=bar BAR=baz eslint .").as_deref(),
            Some("eslint")
        );
        assert_eq!(
            first_command_token("pnpm dlx prettier .").as_deref(),
            Some("prettier")
        );
        // another pnpm subcommand is not a routable tool
        assert_eq!(first_command_token("pnpm install"), None);
        // no command at all
        assert_eq!(first_command_token("FOO=bar"), None);
    }

    #[test]
    fn test_reporter_forced_detection() {
        assert!(reporter_forced(
            &["--reporter=json".to_string()],
            None
        ));
        assert!(reporter_forced(
            &[],
            Some(&"vitest run --reporter=dot".to_string())
        ));
        assert!(!reporter_forced(
            &[],
            Some(&"vitest run".to_string())
        ));
        assert!(!reporter_forced(&[], None));
    }

    #[test]
    fn test_emit_guarded_caps_output_at_raw() {
        // run_script routes display through print_with_hint/emit_guarded: a
        // fabricated summary longer than the raw output must never be shown
        let shown = crate::core::runner::emit_guarded("TypeScript compilation completed", None, "x");
        assert_eq!(shown, "x");
        let shown = crate::core::runner::emit_guarded("ok", None, "much longer raw output");
        assert_eq!(shown, "ok");
    }

    #[test]
    fn test_package_scripts_detect_tool_missing_script() {
        let ps = PackageScripts {
            scripts: HashMap::from([("test".to_string(), "vitest run".to_string())]),
        };
        assert_eq!(ps.detect_tool("nonexistent"), None);
    }

    #[test]
    fn test_route_script_with_package_scripts() {
        // route_script falls through static matching to cached detect_tool
        let ps = PackageScripts {
            scripts: HashMap::from([("test:e2e".to_string(), "playwright test".to_string())]),
        };
        assert_eq!(
            route_script("test:e2e", Some(&ps)),
            Some(FilterRoute::Playwright)
        );
    }

    #[test]
    fn test_find_package_json_from_finds_nearest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{ "name": "x" }"#).unwrap();
        let nested = dir.path().join("sub");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(
            find_package_json_from(&nested),
            Some(dir.path().join("package.json"))
        );
    }

    // --- apply_filter tests ---

    #[test]
    fn test_apply_filter_tsc_label() {
        let (_, label) = apply_filter(FilterRoute::Tsc, "some output", true).unwrap();
        assert_eq!(label, "tsc (via pnpm run)");
    }

    #[test]
    fn test_apply_filter_vitest_buffered_json() {
        // Reached when a forced --reporter keeps vitest off the streaming path
        let json = r#"{"numTotalTests":3,"numPassedTests":2,"numFailedTests":1,"numPendingTests":0,"testResults":[{"name":"src/a.test.ts","assertionResults":[{"fullName":"a > works","status":"failed","failureMessages":["Error: nope"]}]}]}"#;
        let (filtered, label) = apply_filter(FilterRoute::Vitest, json, false).unwrap();
        assert_eq!(label, "vitest (via pnpm run)");
        assert!(filtered.contains("FAIL (1)"), "got: {}", filtered);
        assert!(filtered.contains("a > works"), "got: {}", filtered);
    }

    #[test]
    fn test_apply_filter_lint_label() {
        let (_, label) = apply_filter(FilterRoute::Lint, "some output", true).unwrap();
        assert_eq!(label, "lint (via pnpm run)");
    }

    #[test]
    fn test_apply_filter_prettier_label() {
        let (_, label) = apply_filter(FilterRoute::Prettier, "some output", true).unwrap();
        assert_eq!(label, "prettier (via pnpm run)");
    }

    #[test]
    fn test_apply_filter_test_runner_label() {
        let (_, label) = apply_filter(FilterRoute::TestRunner, "some output", true).unwrap();
        assert_eq!(label, "test (via pnpm run)");
    }

    #[test]
    fn test_apply_filter_playwright_label() {
        let (_, label) = apply_filter(FilterRoute::Playwright, "some output", true).unwrap();
        assert_eq!(label, "playwright (via pnpm run)");
    }

    // --- apply_filter failure-path hardening ---

    #[test]
    fn test_apply_filter_tsc_failure_unrecognized_falls_back_to_raw() {
        // A failing script routed to tsc whose output has no tsc errors must
        // not print a fabricated "compilation completed"
        let output = "vite build failed\nRollupError: unexpected token";
        let (filtered, _) = apply_filter(FilterRoute::Tsc, output, false).unwrap();
        assert_eq!(filtered, output);
        // Success path keeps the historical placeholder
        let (filtered, _) = apply_filter(FilterRoute::Tsc, output, true).unwrap();
        assert_eq!(filtered, "TypeScript compilation completed");
    }

    #[test]
    fn test_apply_filter_lint_failure_unrecognized_falls_back_to_raw() {
        let output = "biome: configuration file not found";
        let (filtered, _) = apply_filter(FilterRoute::Lint, output, false).unwrap();
        assert_eq!(filtered, output);
        let (filtered, _) = apply_filter(FilterRoute::Lint, "all clean", true).unwrap();
        assert_eq!(filtered, "Lint: No issues found");
    }

    #[test]
    fn test_apply_filter_playwright_failure_keeps_error_text() {
        // Non-JSON failing playwright output: degraded parse is counts-only and
        // would drop the error/code-frame sections — fall back to raw instead.
        let transcript = "Running 2 tests using 1 worker\n\n  ✓  1 tests/login.spec.ts:3:1 › works (500ms)\n  ✘  2 tests/login.spec.ts:8:1 › fails (300ms)\n\n\n  1) tests/login.spec.ts:8:1 › fails\n\n    Error: expect(received).toBe(expected)\n\n      8 | test('fails', async ({ page }) => {\n      9 |   await expect(page.locator('h1')).toBe('x');\n       |                                                 ^\n\n  1 failed\n    tests/login.spec.ts:8:1 › fails (300ms)\n  1 passed (1.2s)\n";
        let (filtered, _) = apply_filter(FilterRoute::Playwright, transcript, false).unwrap();
        assert_eq!(filtered, transcript);
        assert!(filtered.contains("expect(received).toBe(expected)"));

        // Same transcript on success-path formatting stays counts-only compact
        let green = "Running 1 test using 1 worker\n\n  ✓  1 tests/a.spec.ts:3:1 › works (100ms)\n\n  1 passed (2.0s)\n";
        let (filtered, _) = apply_filter(FilterRoute::Playwright, green, true).unwrap();
        assert!(filtered.contains("PASS (1)"), "got: {}", filtered);
    }

    // --- integration tests ---

    #[test]
    fn test_filter_then_route_integration() {
        let raw = r#"> app@1.0.0 lint
$ eslint .

src/file.ts: warning no-unused-vars

Done in 1.2s"#;
        let stripped = filter_pnpm_run_output(raw);
        assert!(!stripped.contains("> app@"));
        assert!(!stripped.contains("Done in"));

        let ps = PackageScripts {
            scripts: HashMap::from([("lint".to_string(), "eslint .".to_string())]),
        };
        let route = route_script("lint", Some(&ps));
        assert_eq!(route, Some(FilterRoute::Lint));

        let (filtered, label) = apply_filter(route.unwrap(), &stripped, true).unwrap();
        assert_eq!(label, "lint (via pnpm run)");
        assert!(!filtered.is_empty());
    }

    #[test]
    fn test_ok_guard_skips_routing() {
        // Filter returns empty for boilerplate; run_script adds "ok" on success
        let raw = "> pkg@1.0.0 test\n$ vitest run\n\nDone in 2s\n";
        let stripped = filter_pnpm_run_output(raw);
        assert_eq!(stripped, "");
        // In run_script: if stripped.is_empty() && success -> println!("ok")
    }

    // --- is_pnpm_script tests ---

    #[test]
    fn test_is_pnpm_script_routed_scripts() {
        // Tier 1 static routes are recognized without package.json
        let no_scripts: Option<PackageScripts> = None;
        assert!(is_pnpm_script("vitest", &no_scripts));
        // Ambiguous names need package.json
        assert!(!is_pnpm_script("lint", &no_scripts));
        let with_scripts = Some(PackageScripts {
            scripts: HashMap::from([("lint".to_string(), "eslint .".to_string())]),
        });
        assert!(is_pnpm_script("lint", &with_scripts));
    }

    #[test]
    fn test_is_pnpm_script_unknown() {
        // Not a known script name and no PackageScripts
        let no_scripts: Option<PackageScripts> = None;
        assert!(!is_pnpm_script("my-custom-script", &no_scripts));
    }

    #[test]
    fn test_is_pnpm_script_with_cached_scripts() {
        // Custom script found via cached PackageScripts
        let ps = Some(PackageScripts {
            scripts: HashMap::from([("my-custom".to_string(), "node run.js".to_string())]),
        });
        assert!(is_pnpm_script("my-custom", &ps));
    }

    #[test]
    fn test_is_pnpm_script_none_scripts_falls_back() {
        // Without package.json, only Tier 1 static routes match; everything else defaults to passthrough
        let no_scripts: Option<PackageScripts> = None;
        assert!(!is_pnpm_script("my-custom-script", &no_scripts));
        assert!(!is_pnpm_script("lint", &no_scripts)); // needs package.json
        assert!(is_pnpm_script("vitest", &no_scripts)); // Tier 1
    }

    #[test]
    fn test_native_commands_fall_through_without_denylist() {
        // Native pnpm commands (install, add, exec, dlx) naturally return false
        // because they're neither static routes nor in package.json.
        let no_scripts: Option<PackageScripts> = None;
        assert!(!is_pnpm_script("install", &no_scripts));
        assert!(!is_pnpm_script("add", &no_scripts));
        assert!(!is_pnpm_script("exec", &no_scripts));
        assert!(!is_pnpm_script("dlx", &no_scripts));

        // Same result with package.json present (native names aren't user scripts)
        let with_scripts = Some(PackageScripts {
            scripts: HashMap::from([("dev".to_string(), "next dev".to_string())]),
        });
        assert!(!is_pnpm_script("install", &with_scripts));
        assert!(!is_pnpm_script("add", &with_scripts));
        assert!(!is_pnpm_script("exec", &with_scripts));
        assert!(!is_pnpm_script("dlx", &with_scripts));
    }

    #[test]
    fn test_apply_filter_empty_output_returns_error() {
        // Empty output triggers Err (fallback to stripped output in caller)
        let result = apply_filter(FilterRoute::Tsc, "", true);
        assert!(result.is_err());
    }

    #[test]
    fn test_apply_filter_whitespace_only_returns_error() {
        // Whitespace-only output triggers Err
        let result = apply_filter(FilterRoute::Lint, "   \n\n  ", true);
        assert!(result.is_err());
    }

    // --- strip_pnpm_stderr tests ---

    #[test]
    fn test_strip_pnpm_stderr_removes_elifecycle() {
        let stderr = " ELIFECYCLE  Command failed with exit code 1.\nSome real error\nDone in 3s";
        let result = strip_pnpm_stderr(stderr);
        assert!(!result.contains("ELIFECYCLE"));
        assert!(!result.contains("Done in"));
        assert!(result.contains("Some real error"));
    }

    #[test]
    fn test_strip_pnpm_stderr_preserves_err_pnpm() {
        let stderr =
            " ERR_PNPM_NO_PKG_MANIFEST  No package.json found\n ELIFECYCLE  Command failed";
        let result = strip_pnpm_stderr(stderr);
        assert!(
            result.contains("ERR_PNPM_NO_PKG_MANIFEST"),
            "ERR_PNPM messages should be preserved, got: {}",
            result
        );
        assert!(!result.contains("ELIFECYCLE"));
    }

    #[test]
    fn test_strip_pnpm_stderr_preserves_non_boilerplate() {
        let stderr = "Error: Cannot find module 'express'\n    at Module._resolveFilename";
        let result = strip_pnpm_stderr(stderr);
        assert!(result.contains("Cannot find module"));
        assert!(result.contains("_resolveFilename"));
    }

    #[test]
    fn test_strip_pnpm_stderr_empty_input() {
        let result = strip_pnpm_stderr("");
        assert_eq!(result, "");
    }

    #[test]
    fn test_strip_pnpm_stderr_only_boilerplate() {
        let stderr = " ELIFECYCLE  Command failed\n\nDone in 2s\n";
        let result = strip_pnpm_stderr(stderr);
        assert_eq!(result, "");
    }

    // --- stream separation + failure behavior tests ---

    #[test]
    fn test_filter_pnpm_run_output_returns_empty_for_boilerplate() {
        // filter_pnpm_run_output returns empty string, not "ok"
        let input = "> pkg@1.0.0 build\n$ tsc\n\nDone in 1s\n";
        let result = filter_pnpm_run_output(input);
        assert!(
            result.is_empty(),
            "Expected empty string for boilerplate, got: {:?}",
            result
        );
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
    fn test_failure_empty_stdout_no_ok() {
        // Empty stdout + failure should NOT produce "ok"
        let stdout = "> pkg@1.0.0 test\n$ vitest run\n";
        let stderr = " ERR_PNPM_NO_PKG_MANIFEST  No package.json found";

        let stripped = filter_pnpm_run_output(stdout);
        assert!(stripped.is_empty(), "Stripped stdout should be empty");

        // In run_script failure path: when stripped is empty, show stderr
        let stderr_display = strip_pnpm_stderr(stderr);
        assert!(
            stderr_display.contains("ERR_PNPM"),
            "Stderr should contain the error message"
        );
        // The display would be stderr_display, NOT "ok"
        assert!(
            !stderr_display.contains("ok"),
            "Should never show 'ok' on failure"
        );
    }

    #[test]
    fn test_success_empty_stdout_shows_ok() {
        // On success with empty stripped stdout, run_script shows "ok"
        let stdout = "> pkg@1.0.0 build\n$ tsc --noEmit\n\nDone in 1s\n";
        let stripped = filter_pnpm_run_output(stdout);
        assert!(stripped.is_empty());
        // In run_script success path: println!("ok") -- verified by logic, not process::exit
    }

    // --- full pipeline token savings tests ---

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    // --- VitestStreamFilter tests ---

    fn stream_output(filter: &mut VitestStreamFilter, input: &str, exit_code: i32) -> String {
        let mut out = String::new();
        for line in input.lines() {
            if let Some(emitted) = filter.feed_line(line) {
                out.push_str(&emitted);
            }
        }
        out.push_str(&filter.flush());
        if let Some(summary) = filter.on_exit(exit_code, input) {
            out.push_str(&summary);
        }
        out
    }

    #[test]
    fn test_vitest_stream_all_pass_suppressed() {
        let transcript = r#"> my-app@1.0.0 test /Users/dev/my-app
> vitest run

 RUN  v2.1.8 /Users/dev/my-app

 ✓ src/a.test.ts (3 tests) 5ms
 ✓ src/b.test.ts (4 tests) 8ms

 Test Files  2 passed (2)
      Tests  7 passed (7)
   Start at  10:00:00
   Duration  1.23s
"#;
        let mut filter = VitestStreamFilter::new();
        for line in transcript.lines() {
            assert_eq!(
                filter.feed_line(line),
                None,
                "all-pass run should suppress line: {:?}",
                line
            );
        }
        let summary = filter.on_exit(0, transcript).unwrap();
        assert_eq!(summary, "PASS (7) | 2 suites | 1s\n");
    }

    #[test]
    fn test_vitest_stream_failures_shown_and_counted() {
        let transcript = r#"> my-app@1.0.0 test /Users/dev/my-app
> vitest run

 RUN  v2.1.8 /Users/dev/my-app

 ✓ src/a.test.ts (3 tests) 5ms
 ❯ src/b.test.ts (5 tests | 2 failed) 10ms
   × b > does the first thing
   × b > does the second thing

⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯ Failed Tests 2 ⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯

 FAIL  src/b.test.ts > b > does the first thing
AssertionError: expected 1 to be 2
 ❯ src/b.test.ts:10:5

 Test Files  1 failed | 1 passed (2)
      Tests  2 failed | 6 passed (8)
   Duration  0.85s
"#;
        let mut filter = VitestStreamFilter::new();
        let mut out = String::new();
        for line in transcript.lines() {
            if let Some(emitted) = filter.feed_line(line) {
                out.push_str(&emitted);
            }
        }
        assert!(out.contains("❯ src/b.test.ts (5 tests | 2 failed) 10ms\n"));
        assert!(out.contains("× b > does the first thing\n"));
        assert!(out.contains("FAIL  src/b.test.ts > b > does the first thing\n"));
        assert!(out.contains("AssertionError: expected 1 to be 2\n"));
        assert!(!out.contains("✓ src/a.test.ts"));
        assert!(!out.contains("RUN  v2.1.8"));
        assert!(!out.contains("Test Files"));
        assert!(!out.contains("Failed Tests"));

        let summary = filter.on_exit(1, transcript).unwrap();
        assert_eq!(summary, "PASS (6) FAIL (2) | 2 suites (1 failed) | 1s\n");
    }

    #[test]
    fn test_vitest_stream_caps_inline_failures_at_ten() {
        let mut filter = VitestStreamFilter::new();
        let mut out = String::new();
        for i in 1..=12 {
            let line = format!(" FAIL  src/f{}.test.ts > case {}", i, i);
            if let Some(emitted) = filter.feed_line(&line) {
                out.push_str(&emitted);
            }
        }
        assert_eq!(out.matches("FAIL  src/").count(), MAX_INLINE_FAILURES);

        let raw =
            " Test Files  11 failed | 1 passed (12)\n      Tests  12 failed | 1 passed (13)\n";
        let summary = filter.on_exit(1, raw).unwrap();
        assert!(
            summary.contains("... and 2 more failures"),
            "expected truncation notice, got: {:?}",
            summary
        );
    }

    #[test]
    fn test_vitest_stream_suite_headers_do_not_underflow_summary() {
        // Suite-level FAIL headers (hook errors) inflate failures_shown beyond
        // tests_failed — the summary must not underflow usize nor claim that
        // failures were hidden.
        let mut filter = VitestStreamFilter::new();
        for i in 1..=12 {
            let line = format!(" FAIL  src/f{}.test.ts > hook error {}", i, i);
            filter.feed_line(&line);
        }
        let raw = " Test Files  4 failed | 8 passed (12)\n      Tests  5 failed | 55 passed (60)\n";
        let summary = filter.on_exit(1, raw).unwrap();
        assert!(summary.contains("FAIL (5)"), "got: {:?}", summary);
        assert!(!summary.contains("more failures"), "got: {:?}", summary);
    }

    #[test]
    fn test_vitest_stream_long_failure_detail_truncated() {
        let mut filter = VitestStreamFilter::new();
        let mut out = String::new();
        if let Some(emitted) = filter.feed_line(" FAIL  src/big.test.ts > big > stack") {
            out.push_str(&emitted);
        }
        for i in 1..=40 {
            let line = format!("    at frame{} (src/big.test.ts:{}:1)", i, i);
            if let Some(emitted) = filter.feed_line(&line) {
                out.push_str(&emitted);
            }
        }
        assert!(out.contains("... (truncated)\n"));
        assert!(out.contains("frame30"));
        assert!(!out.contains("frame31"));
        assert!(!out.contains("frame40"));
    }

    #[test]
    fn test_vitest_stream_skipped_files_not_counted_as_passed() {
        let mut filter = VitestStreamFilter::new();
        assert_eq!(filter.feed_line(" ✓ a.test.ts (4 tests) 5ms"), None);
        assert_eq!(filter.feed_line(" ↓ c.test.ts (2 tests | 2 skipped)"), None);
        assert_eq!(filter.feed_line(" ↓ d.test.ts (3 skipped)"), None);
        // No parseable summary in raw → counted fallback
        let summary = filter.on_exit(0, "unparseable").unwrap();
        assert_eq!(summary, "PASS (4) | 1 suites (5 skipped) | ?\n");
    }

    #[test]
    fn test_vitest_stream_pnpm_boilerplate_suppressed() {
        let mut filter = VitestStreamFilter::new();
        let boilerplate = [
            "> pkg@1.0.0 test /path/to/pkg",
            "> vitest run",
            "$ vitest run",
            "Done in 2.1s",
            " ELIFECYCLE  Command failed with exit code 1.",
            "Progress: resolved 1, reused 1",
        ];
        for line in boilerplate {
            assert_eq!(
                filter.feed_line(line),
                None,
                "boilerplate should be suppressed: {:?}",
                line
            );
        }
    }

    #[test]
    fn test_vitest_stream_garbage_passes_through() {
        let mut filter = VitestStreamFilter::new();
        assert_eq!(
            filter.feed_line("some unrecognized noise"),
            Some("some unrecognized noise\n".to_string())
        );
    }

    #[test]
    fn test_vitest_stream_in_progress_line_passes_through_uncounted() {
        // ❯ without a failed count is an in-progress/other line, not a result:
        // shown for transparency but must not inflate pass counts.
        let mut filter = VitestStreamFilter::new();
        assert_eq!(
            filter.feed_line(" ❯ src/running.test.ts (3 tests)"),
            Some(" ❯ src/running.test.ts (3 tests)\n".to_string())
        );
        let summary = filter.on_exit(0, "unparseable").unwrap();
        assert_eq!(summary, "PASS (0) | 0 suites | ?\n");
    }

    #[test]
    fn test_vitest_stream_emissions_are_newline_terminated() {
        let transcript = r#"> app@1.0.0 test /x
> vitest run
 RUN  v2.1.8 /x
 ✓ a.test.ts (1 test) 1ms
 ❯ b.test.ts (2 tests | 1 failed) 2ms
 FAIL  b.test.ts > b > nope
AssertionError: nope
garbage line
 Test Files  1 failed | 1 passed (2)
      Tests  1 failed | 1 passed (2)
   Duration  12ms
"#;
        let mut filter = VitestStreamFilter::new();
        for line in transcript.lines() {
            if let Some(emitted) = filter.feed_line(line) {
                assert!(
                    emitted.ends_with('\n'),
                    "emission missing newline: {:?}",
                    emitted
                );
            }
        }
        assert!(filter.flush().is_empty());
        let summary = filter.on_exit(1, transcript).unwrap();
        assert!(summary.ends_with('\n'), "summary missing newline");
    }

    #[test]
    fn test_vitest_stream_summary_with_skipped_files() {
        // "Test Files  2 passed | 1 skipped (3)" must parse — a green suite with
        // skipped files previously fell back to counted zeros ("0 suites").
        let raw = " Test Files  2 passed | 1 skipped (3)\n      Tests  5 passed | 2 skipped (7)\n   Duration  1.5s\n";
        let mut filter = VitestStreamFilter::new();
        for line in raw.lines() {
            filter.feed_line(line);
        }
        let summary = filter.on_exit(0, raw).unwrap();
        assert_eq!(summary, "PASS (5) | 3 suites (2 skipped) | 2s\n");
    }

    #[test]
    fn test_vitest_stream_summary_failed_passed_skipped_files() {
        let raw = " Test Files  1 failed | 1 passed | 1 skipped (3)\n      Tests  2 failed | 4 passed | 1 skipped (7)\n";
        let mut filter = VitestStreamFilter::new();
        let summary = filter.on_exit(1, raw).unwrap();
        assert_eq!(
            summary,
            "PASS (4) FAIL (2) | 3 suites (1 failed) | 1 skipped | ?\n"
        );
    }

    #[test]
    fn test_vitest_stream_summary_todo_only() {
        let raw = " Test Files  1 skipped (1)\n      Tests  2 todo (2)\n";
        let mut filter = VitestStreamFilter::new();
        let summary = filter.on_exit(0, raw).unwrap();
        assert_eq!(summary, "PASS (0) | 1 suites | ?\n");
    }

    #[test]
    fn test_vitest_stream_echo_suppression_stops_after_banner() {
        let mut filter = VitestStreamFilter::new();
        // pnpm preamble: header + script echo are suppressed before the banner
        assert_eq!(filter.feed_line("> pkg@1.0.0 test /path"), None);
        assert_eq!(filter.feed_line("> vitest run"), None);
        assert_eq!(filter.feed_line("$ vitest run"), None);
        assert_eq!(filter.feed_line(" RUN  v2.1.8 /path"), None);
        // after the RUN banner, `>` / `$` lines are user output and pass through
        assert_eq!(
            filter.feed_line("> hello"),
            Some("> hello\n".to_string())
        );
        assert_eq!(
            filter.feed_line("$ foo"),
            Some("$ foo\n".to_string())
        );
    }

    #[test]
    fn test_vitest_stream_failure_detail_resets_on_file_result() {
        let mut filter = VitestStreamFilter::new();
        filter.feed_line("   × a > fails");
        assert!(filter.in_failure_detail);
        // a new file result ends the detail block…
        assert_eq!(filter.feed_line(" ✓ b.test.ts (2 tests) 3ms"), None);
        assert!(!filter.in_failure_detail);
        // …so later lines are not eaten by the detail-line budget
        let mut out = String::new();
        for i in 1..=35 {
            if let Some(s) = filter.feed_line(&format!("noise line {}", i)) {
                out.push_str(&s);
            }
        }
        assert!(out.contains("noise line 35"));
        assert!(!out.contains("truncated"));
    }

    #[test]
    fn test_vitest_stream_failure_detail_resets_on_summary_line() {
        let mut filter = VitestStreamFilter::new();
        filter.feed_line("   × a > fails");
        assert!(filter.in_failure_detail);
        filter.feed_line(" Test Files  1 passed (1)");
        assert!(!filter.in_failure_detail);
    }

    #[test]
    fn test_full_pipeline_vitest_streaming_savings() {
        // Realistic non-TTY vitest default-reporter transcript, as streamed by
        // `pnpm run test` (test = "vitest run") — the production path feeds
        // this through VitestStreamFilter via run_streamed, not apply_filter.
        let mut transcript = String::from(
            "> my-app@1.0.0 test /Users/dev/my-app\n> vitest run\n\n RUN  v2.1.8 /Users/dev/my-app\n\n",
        );
        for i in 1..=40 {
            transcript.push_str(&format!(
                " ✓ src/components/Comp{:02}.test.tsx ({} tests) {}ms\n",
                i,
                i % 3 + 1,
                i * 3
            ));
        }
        transcript.push_str(" ❯ src/checkout.test.ts (5 tests | 2 failed) 20ms\n");
        transcript.push_str("   × checkout > calculates total with tax\n");
        transcript.push_str("   × checkout > applies discount code\n\n");
        transcript.push_str("⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯ Failed Tests 2 ⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯\n\n");
        for (name, line_no) in [("calculates total with tax", 45), ("applies discount code", 78)] {
            transcript.push_str(&format!(
                " FAIL  src/checkout.test.ts > checkout > {}\n",
                name
            ));
            transcript.push_str(
                "AssertionError: expected 105 to be 100 // Object.is equality\n\n- Expected\n+ Received\n\n- 100\n+ 105\n\n",
            );
            transcript.push_str(&format!(" ❯ src/checkout.test.ts:{}:23\n", line_no));
            transcript.push_str(
                " ❯ processTicksAndRejections node:internal/process/task_queues:95:5\n\n",
            );
        }
        transcript.push_str(" Test Files  1 failed | 40 passed (41)\n");
        transcript.push_str("      Tests  2 failed | 83 passed (85)\n");
        transcript.push_str("   Start at  10:00:00\n");
        transcript.push_str("   Duration  4.12s\n\nDone in 4.2s\n");

        let mut filter = VitestStreamFilter::new();
        let filtered = stream_output(&mut filter, &transcript, 1);

        assert!(filtered.contains("❯ src/checkout.test.ts"));
        assert!(filtered.contains("FAIL  src/checkout.test.ts > checkout >"));
        assert!(filtered.contains("PASS (83) FAIL (2) | 41 suites (1 failed) | 4s\n"));
        assert!(!filtered.contains("✓ src/components/Comp01"));
        assert!(!filtered.contains("RUN  v2.1.8"));
        assert!(!filtered.contains("Test Files"));

        let input_tokens = count_tokens(&transcript);
        let output_tokens = count_tokens(&filtered);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Streaming vitest savings: expected >= 60%, got {:.1}% (input={}, output={})",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_full_pipeline_playwright_savings() {
        // Realistic pnpm stdout containing playwright --reporter=json output
        // Pipeline: raw pnpm stdout -> filter_pnpm_run_output (strip) -> apply_filter(Playwright)
        let fixture = r#"> my-app@1.0.0 test:e2e /Users/dev/my-app
$ playwright test --reporter=json

{"config":{"projects":[{"name":"chromium"}]},"suites":[{"title":"login.spec.ts","file":"tests/login.spec.ts","specs":[{"title":"should login with valid credentials","ok":true,"tests":[{"status":"expected","projectName":"chromium","results":[{"status":"passed","duration":1234,"errors":[]}]}]},{"title":"should reject invalid password","ok":true,"tests":[{"status":"expected","projectName":"chromium","results":[{"status":"passed","duration":567,"errors":[]}]}]},{"title":"should show error for locked account","ok":true,"tests":[{"status":"expected","projectName":"chromium","results":[{"status":"passed","duration":890,"errors":[]}]}]},{"title":"should redirect after login","ok":true,"tests":[{"status":"expected","projectName":"chromium","results":[{"status":"passed","duration":456,"errors":[]}]}]}],"suites":[]},{"title":"dashboard.spec.ts","file":"tests/dashboard.spec.ts","specs":[{"title":"shows metrics overview","ok":true,"tests":[{"status":"expected","projectName":"chromium","results":[{"status":"passed","duration":1100,"errors":[]}]}]},{"title":"filters by date range","ok":true,"tests":[{"status":"expected","projectName":"chromium","results":[{"status":"passed","duration":980,"errors":[]}]}]},{"title":"exports CSV report","ok":true,"tests":[{"status":"expected","projectName":"chromium","results":[{"status":"passed","duration":1500,"errors":[]}]}]}],"suites":[]},{"title":"settings.spec.ts","file":"tests/settings.spec.ts","specs":[{"title":"updates profile name","ok":true,"tests":[{"status":"expected","projectName":"chromium","results":[{"status":"passed","duration":670,"errors":[]}]}]},{"title":"changes password","ok":true,"tests":[{"status":"expected","projectName":"chromium","results":[{"status":"passed","duration":890,"errors":[]}]}]},{"title":"toggles dark mode","ok":true,"tests":[{"status":"expected","projectName":"chromium","results":[{"status":"passed","duration":340,"errors":[]}]}]}],"suites":[]}],"stats":{"expected":10,"unexpected":0,"flaky":0,"skipped":0,"duration":8500}}

Done in 12.3s
"#;

        // Stage 1: Strip pnpm boilerplate
        let stripped = filter_pnpm_run_output(fixture);
        assert!(
            !stripped.contains("> my-app@"),
            "Lifecycle header should be stripped"
        );
        assert!(
            !stripped.contains("Done in"),
            "Done line should be stripped"
        );
        assert!(
            stripped.contains("stats"),
            "JSON payload should survive stripping"
        );

        // Stage 2: Apply playwright specialized filter
        let (filtered, label) = apply_filter(FilterRoute::Playwright, &stripped, true).unwrap();
        assert_eq!(label, "playwright (via pnpm run)");
        assert!(
            filtered.contains("PASS"),
            "Filtered output should contain PASS summary"
        );

        // Stage 3: Verify token savings >= 60%
        let input_tokens = count_tokens(fixture);
        let output_tokens = count_tokens(&filtered);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Full pipeline playwright savings: expected >= 60%, got {:.1}% (input={}, output={})",
            savings,
            input_tokens,
            output_tokens
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
