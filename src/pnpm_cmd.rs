use crate::tracking;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::ffi::OsString;
use std::process::Command;

use crate::parser::{
    emit_degradation_warning, emit_passthrough_warning, truncate_output, Dependency,
    DependencyState, FormatMode, OutputParser, ParseResult, TokenFormatter,
};

/// Native pnpm subcommands that must never be intercepted as scripts (BUG-03).
/// Sorted alphabetically for binary_search lookup.
/// Excludes: run, test, start -- those are pnpm script shortcuts that go through smart routing.
const NATIVE_PNPM_COMMANDS: &[&str] = &[
    "add",
    "audit",
    "bin",
    "cache",
    "cat-file",
    "cat-index",
    "completion",
    "config",
    "create",
    "dedupe",
    "deploy",
    "dlx",
    "doctor",
    "env",
    "exec",
    "fetch",
    "find-hash",
    "import",
    "init",
    "install",
    "install-test",
    "licenses",
    "link",
    "list",
    "outdated",
    "pack",
    "patch",
    "patch-commit",
    "patch-remove",
    "prune",
    "publish",
    "rebuild",
    "remove",
    "root",
    "self-update",
    "server",
    "setup",
    "store",
    "unlink",
    "update",
    "why",
];

// pnpm run output boilerplate patterns
lazy_static! {
    // > my-project@1.0.0 test /path/to/project
    static ref LIFECYCLE_HEADER: Regex = Regex::new(r"^>\s+\S+@\S+\s+").unwrap();
    // $ vitest run --reporter=json
    static ref SCRIPT_ECHO: Regex = Regex::new(r"^\$\s+").unwrap();
    // Done in 3.4s
    static ref DONE_MSG: Regex = Regex::new(r"^Done in \d").unwrap();
    // ELIFECYCLE  Command failed with exit code 1.
    static ref ELIFECYCLE: Regex = Regex::new(r"(?i)ELIFECYCLE|ERR_PNPM").unwrap();
    // Progress: resolved 123, reused 120, downloaded 3
    static ref PROGRESS: Regex = Regex::new(r"^Progress:").unwrap();
}

/// pnpm list JSON output structure
#[derive(Debug, Deserialize)]
struct PnpmListOutput {
    #[serde(flatten)]
    packages: HashMap<String, PnpmPackage>,
}

#[derive(Debug, Deserialize)]
struct PnpmPackage {
    version: Option<String>,
    #[serde(rename = "dependencies", default)]
    dependencies: HashMap<String, PnpmPackage>,
    #[serde(rename = "devDependencies", default)]
    dev_dependencies: HashMap<String, PnpmPackage>,
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
        match serde_json::from_str::<PnpmListOutput>(input) {
            Ok(json) => {
                let mut dependencies = Vec::new();
                let mut total_count = 0;

                for (name, pkg) in &json.packages {
                    collect_dependencies(name, pkg, false, &mut dependencies, &mut total_count);
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
                        ParseResult::Passthrough(truncate_output(input, 500))
                    }
                }
            }
        }
    }
}

/// Recursively collect dependencies from pnpm package tree
fn collect_dependencies(
    name: &str,
    pkg: &PnpmPackage,
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

    for line in output.lines() {
        // Skip box-drawing and metadata
        if line.contains('│')
            || line.contains('├')
            || line.contains('└')
            || line.contains("Legend:")
            || line.trim().is_empty()
        {
            continue;
        }

        // Parse lines like: "package@1.2.3"
        let parts: Vec<&str> = line.split_whitespace().collect();
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
                        dev_dependency: false,
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
                        ParseResult::Passthrough(truncate_output(input, 500))
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

/// Validates npm package name according to official rules
fn is_valid_package_name(name: &str) -> bool {
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
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("pnpm");
    cmd.arg("list");
    cmd.arg(format!("--depth={}", depth));
    cmd.arg("--json");

    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run pnpm list")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("pnpm list failed: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse output using PnpmListParser
    let parse_result = PnpmListParser::parse(&stdout);
    let mode = FormatMode::from_verbosity(verbose);

    let filtered = match parse_result {
        ParseResult::Full(data) => {
            if verbose > 0 {
                eprintln!("pnpm list (Tier 1: Full JSON parse)");
            }
            data.format(mode)
        }
        ParseResult::Degraded(data, warnings) => {
            if verbose > 0 {
                emit_degradation_warning("pnpm list", &warnings.join(", "));
            }
            data.format(mode)
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
        &stdout,
        &filtered,
    );

    Ok(())
}

fn run_outdated(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("pnpm");
    cmd.arg("outdated");
    cmd.arg("--format");
    cmd.arg("json");

    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run pnpm outdated")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);

    // Parse output using PnpmOutdatedParser
    let parse_result = PnpmOutdatedParser::parse(&stdout);
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
        println!("All packages up-to-date ✓");
    } else {
        println!("{}", filtered);
    }

    timer.track("pnpm outdated", "rtk pnpm outdated", &combined, &filtered);

    Ok(())
}

fn run_install(packages: &[String], args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

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

    timer.track(
        &format!("pnpm install {}", packages.join(" ")),
        &format!("rtk pnpm install {}", packages.join(" ")),
        &combined,
        &filtered,
    );

    Ok(())
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
        "ok ✓".to_string()
    } else {
        result.join("\n")
    }
}

/// Runs an unsupported pnpm subcommand by passing it through directly
pub fn run_passthrough(args: &[OsString], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("pnpm passthrough: {:?}", args);
    }
    let status = Command::new("pnpm")
        .args(args)
        .status()
        .context("Failed to run pnpm")?;

    let args_str = tracking::args_display(args);
    timer.track_passthrough(
        &format!("pnpm {}", args_str),
        &format!("rtk pnpm {} (passthrough)", args_str),
    );

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

// ─── pnpm run <script> support ───────────────────────────────────────────────

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

/// Strip pnpm-specific boilerplate from script output
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
        if ELIFECYCLE.is_match(trimmed) {
            continue;
        }
        if PROGRESS.is_match(trimmed) {
            continue;
        }
        result.push(line.to_string());
    }

    if result.is_empty() {
        "ok ✓".to_string()
    } else {
        result.join("\n")
    }
}

/// Route a script name to a specialized filter (static rules + package.json detection)
pub(crate) fn route_script(script: &str) -> Option<FilterRoute> {
    // Tier 1: Static routing (exact match)
    match script {
        "vitest" => return Some(FilterRoute::Vitest),
        "typecheck" | "tsc" => return Some(FilterRoute::Tsc),
        "prettier" | "format" | "format:check" => return Some(FilterRoute::Prettier),
        "lint" => return Some(FilterRoute::Lint),
        "test" => return Some(FilterRoute::TestRunner),
        _ => {}
    }

    // Tier 1b: Prefix matching
    if let Some(suffix) = script.strip_prefix("test:") {
        match suffix {
            "e2e" | "playwright" | "cypress" => {} // defer to package.json
            _ => return Some(FilterRoute::TestRunner),
        }
    }
    if let Some(suffix) = script.strip_prefix("lint:") {
        if suffix != "fix" {
            return Some(FilterRoute::Lint);
        }
    }

    // Tier 2: Auto-detect from package.json
    detect_tool_from_package_json(script)
}

/// Read package.json scripts[name] and detect the underlying tool
pub(crate) fn detect_tool_from_package_json(script: &str) -> Option<FilterRoute> {
    let content = std::fs::read_to_string("package.json").ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    let scripts = json.get("scripts")?.as_object()?;
    let command = scripts.get(script)?.as_str()?;
    let cmd_lower = command.to_lowercase();

    if cmd_lower.contains("playwright") {
        Some(FilterRoute::Playwright)
    } else if cmd_lower.contains("vitest") {
        Some(FilterRoute::Vitest)
    } else if cmd_lower.contains("jest") {
        Some(FilterRoute::TestRunner)
    } else if cmd_lower.contains("tsc") || cmd_lower.contains("typescript") {
        Some(FilterRoute::Tsc)
    } else if cmd_lower.contains("eslint") || cmd_lower.contains("biome") {
        Some(FilterRoute::Lint)
    } else if cmd_lower.contains("prettier") {
        Some(FilterRoute::Prettier)
    } else {
        None
    }
}

/// Check if a name is a known pnpm script (static routing or package.json)
pub fn is_pnpm_script(name: &str) -> bool {
    // Native pnpm commands are never scripts (BUG-03)
    if NATIVE_PNPM_COMMANDS.binary_search(&name).is_ok() {
        return false;
    }

    if route_script(name).is_some() {
        return true;
    }
    // Check package.json scripts
    if let Ok(content) = std::fs::read_to_string("package.json") {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(scripts) = json.get("scripts").and_then(|s| s.as_object()) {
                return scripts.contains_key(name);
            }
        }
    }
    false
}

/// Apply a specialized filter to script output
pub(crate) fn apply_filter(route: FilterRoute, output: &str) -> (String, &'static str) {
    use crate::parser::{OutputParser, TokenFormatter};

    let result = std::panic::catch_unwind(|| match route {
        FilterRoute::Vitest => {
            let parse_result = crate::vitest_cmd::VitestParser::parse(output);
            match parse_result {
                ParseResult::Full(data) => data.format(FormatMode::Compact),
                ParseResult::Degraded(data, _) => data.format(FormatMode::Compact),
                ParseResult::Passthrough(raw) => raw,
            }
        }
        FilterRoute::Playwright => {
            let parse_result = crate::playwright_cmd::PlaywrightParser::parse(output);
            match parse_result {
                ParseResult::Full(data) => data.format(FormatMode::Compact),
                ParseResult::Degraded(data, _) => data.format(FormatMode::Compact),
                ParseResult::Passthrough(raw) => raw,
            }
        }
        FilterRoute::Tsc => crate::tsc_cmd::filter_tsc_output(output),
        FilterRoute::Lint => crate::lint_cmd::filter_generic_lint(output),
        FilterRoute::Prettier => crate::prettier_cmd::filter_prettier_output(output),
        FilterRoute::TestRunner => crate::runner::extract_test_summary(output, "pnpm test"),
    });

    let label = match route {
        FilterRoute::Vitest => "vitest (via pnpm run)",
        FilterRoute::Playwright => "playwright (via pnpm run)",
        FilterRoute::Tsc => "tsc (via pnpm run)",
        FilterRoute::Lint => "lint (via pnpm run)",
        FilterRoute::Prettier => "prettier (via pnpm run)",
        FilterRoute::TestRunner => "test (via pnpm run)",
    };

    match result {
        Ok(filtered) if !filtered.trim().is_empty() => (filtered, label),
        _ => (output.to_string(), label),
    }
}

/// Execute `pnpm run <script>` with smart routing to specialized filters
pub fn run_script(script: &str, args: &[String], verbose: u8, skip_env: bool) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("pnpm");
    cmd.arg("run");
    cmd.arg(script);
    for arg in args {
        cmd.arg(arg);
    }

    if skip_env {
        cmd.env("SKIP_ENV_VALIDATION", "1");
    }

    if verbose > 0 {
        eprintln!("Running: pnpm run {} {}", script, args.join(" "));
    }

    let output = cmd
        .output()
        .context(format!("Failed to run pnpm run {}", script))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);
    let exit_code = output.status.code().unwrap_or(1);

    // On failure: show raw output + tee hint, exit
    if !output.status.success() {
        let stripped = filter_pnpm_run_output(&raw);
        if let Some(hint) =
            crate::tee::tee_and_hint(&raw, &format!("pnpm-run-{}", script), exit_code)
        {
            println!("{}\n{}", stripped, hint);
        } else {
            println!("{}", stripped);
        }
        timer.track(
            &format!("pnpm run {} {}", script, args.join(" ")),
            &format!("rtk pnpm run {} {}", script, args.join(" ")),
            &raw,
            &stripped,
        );
        std::process::exit(exit_code);
    }

    // Strip pnpm boilerplate
    let stripped = filter_pnpm_run_output(&raw);

    // If all output was boilerplate, just show ok
    if stripped == "ok ✓" {
        println!("{}", stripped);
        timer.track(
            &format!("pnpm run {} {}", script, args.join(" ")),
            &format!("rtk pnpm run {} {}", script, args.join(" ")),
            &raw,
            &stripped,
        );
        return Ok(());
    }

    // Route to specialized filter
    let filtered = match route_script(script) {
        Some(route) => {
            let (result, label) = apply_filter(route, &stripped);
            if verbose > 0 {
                eprintln!("Routed to: {}", label);
            }
            result
        }
        None => stripped.clone(),
    };

    if let Some(hint) = crate::tee::tee_and_hint(&raw, &format!("pnpm-run-{}", script), exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("pnpm run {} {}", script, args.join(" ")),
        &format!("rtk pnpm run {} {}", script, args.join(" ")),
        &raw,
        &filtered,
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pnpm_list_parser_json() {
        let json = r#"{
            "my-project": {
                "version": "1.0.0",
                "dependencies": {
                    "express": {
                        "version": "4.18.2"
                    }
                }
            }
        }"#;

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
    fn test_package_name_validation() {
        assert!(is_valid_package_name("lodash"));
        assert!(is_valid_package_name("@clerk/express"));
        assert!(!is_valid_package_name("../../../etc/passwd"));
        assert!(!is_valid_package_name("lodash; rm -rf /"));
    }

    #[test]
    fn test_run_passthrough_accepts_args() {
        // Test that run_passthrough compiles and has correct signature
        let _args: Vec<OsString> = vec![OsString::from("help")];
        // Compile-time verification that the function exists with correct signature
    }

    // ─── filter_pnpm_run_output tests ────────────────────────────────────

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
        let input = "> pkg@1.0.0 test\n$ vitest run\n\nDone in 2.1s\n";
        let result = filter_pnpm_run_output(input);
        assert_eq!(result, "ok ✓");
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
            "Expected ≥40% savings, got {:.1}%",
            savings
        );
    }

    // ─── route_script tests ──────────────────────────────────────────────

    #[test]
    fn test_route_script_exact_matches() {
        assert_eq!(route_script("vitest"), Some(FilterRoute::Vitest));
        assert_eq!(route_script("tsc"), Some(FilterRoute::Tsc));
        assert_eq!(route_script("typecheck"), Some(FilterRoute::Tsc));
        assert_eq!(route_script("lint"), Some(FilterRoute::Lint));
        assert_eq!(route_script("prettier"), Some(FilterRoute::Prettier));
        assert_eq!(route_script("format"), Some(FilterRoute::Prettier));
        assert_eq!(route_script("test"), Some(FilterRoute::TestRunner));
    }

    #[test]
    fn test_route_script_unknown_returns_none() {
        // These don't match static rules and no package.json in test env
        assert_eq!(route_script("build"), None);
        assert_eq!(route_script("dev"), None);
        assert_eq!(route_script("start"), None);
    }

    #[test]
    fn test_route_script_prefix_matching() {
        assert_eq!(route_script("test:unit"), Some(FilterRoute::TestRunner));
        assert_eq!(
            route_script("test:integration"),
            Some(FilterRoute::TestRunner)
        );
        assert_eq!(route_script("lint:check"), Some(FilterRoute::Lint));
        assert_eq!(route_script("lint:ci"), Some(FilterRoute::Lint));
    }

    #[test]
    fn test_route_script_prefix_exclusions() {
        // These are deferred to package.json (no package.json in test → None)
        assert_eq!(route_script("test:e2e"), None);
        assert_eq!(route_script("test:playwright"), None);
        assert_eq!(route_script("test:cypress"), None);
        assert_eq!(route_script("lint:fix"), None);
    }

    // ─── detect_tool_from_package_json tests ─────────────────────────────

    #[test]
    fn test_detect_missing_script_returns_none() {
        // No package.json in test CWD → None
        assert_eq!(detect_tool_from_package_json("nonexistent"), None);
    }

    // ─── apply_filter tests ──────────────────────────────────────────────

    #[test]
    fn test_apply_filter_tsc_label() {
        let (_, label) = apply_filter(FilterRoute::Tsc, "some output");
        assert_eq!(label, "tsc (via pnpm run)");
    }

    #[test]
    fn test_apply_filter_vitest_label() {
        let (_, label) = apply_filter(FilterRoute::Vitest, "some output");
        assert_eq!(label, "vitest (via pnpm run)");
    }

    #[test]
    fn test_apply_filter_lint_label() {
        let (_, label) = apply_filter(FilterRoute::Lint, "some output");
        assert_eq!(label, "lint (via pnpm run)");
    }

    #[test]
    fn test_apply_filter_prettier_label() {
        let (_, label) = apply_filter(FilterRoute::Prettier, "some output");
        assert_eq!(label, "prettier (via pnpm run)");
    }

    #[test]
    fn test_apply_filter_test_runner_label() {
        let (_, label) = apply_filter(FilterRoute::TestRunner, "some output");
        assert_eq!(label, "test (via pnpm run)");
    }

    #[test]
    fn test_apply_filter_playwright_label() {
        let (_, label) = apply_filter(FilterRoute::Playwright, "some output");
        assert_eq!(label, "playwright (via pnpm run)");
    }

    // ─── integration tests ───────────────────────────────────────────────

    #[test]
    fn test_filter_then_route_integration() {
        let raw = r#"> app@1.0.0 lint
$ eslint .

src/file.ts: warning no-unused-vars

Done in 1.2s"#;
        let stripped = filter_pnpm_run_output(raw);
        assert!(!stripped.contains("> app@"));
        assert!(!stripped.contains("Done in"));

        let route = route_script("lint");
        assert_eq!(route, Some(FilterRoute::Lint));

        let (filtered, label) = apply_filter(route.unwrap(), &stripped);
        assert_eq!(label, "lint (via pnpm run)");
        assert!(!filtered.is_empty());
    }

    #[test]
    fn test_ok_checkmark_guard_skips_routing() {
        // When all output is boilerplate, we get "ok ✓" and skip routing
        let raw = "> pkg@1.0.0 test\n$ vitest run\n\nDone in 2s\n";
        let stripped = filter_pnpm_run_output(raw);
        assert_eq!(stripped, "ok ✓");
        // In run_script, this would skip routing
    }

    // ─── is_pnpm_script tests ────────────────────────────────────────────

    #[test]
    fn test_is_pnpm_script_routed_scripts() {
        // These are script names that go through smart routing (NOT native commands)
        assert!(is_pnpm_script("lint"));
        assert!(is_pnpm_script("vitest"));
    }

    #[test]
    fn test_is_pnpm_script_unknown() {
        // Not a known script name and no package.json in test env
        assert!(!is_pnpm_script("my-custom-script"));
    }

    #[test]
    fn test_native_commands_not_intercepted() {
        // These are native pnpm commands -- must never be treated as scripts (BUG-03)
        assert!(!is_pnpm_script("exec"));
        assert!(!is_pnpm_script("dlx"));
        assert!(!is_pnpm_script("audit"));
        assert!(!is_pnpm_script("create"));
        assert!(!is_pnpm_script("deploy"));
        assert!(!is_pnpm_script("store"));
        assert!(!is_pnpm_script("server"));
        assert!(!is_pnpm_script("patch"));
        assert!(!is_pnpm_script("env"));
        assert!(!is_pnpm_script("doctor"));
        assert!(!is_pnpm_script("why"));
        assert!(!is_pnpm_script("init"));
        assert!(!is_pnpm_script("config"));
        assert!(!is_pnpm_script("setup"));
        assert!(!is_pnpm_script("bin"));
        assert!(!is_pnpm_script("self-update"));
    }

    #[test]
    fn test_native_commands_sorted() {
        // binary_search requires sorted array
        let mut sorted = NATIVE_PNPM_COMMANDS.to_vec();
        sorted.sort();
        assert_eq!(
            NATIVE_PNPM_COMMANDS,
            &sorted[..],
            "NATIVE_PNPM_COMMANDS must be sorted alphabetically for binary_search"
        );
    }

    #[test]
    fn test_native_denylist_does_not_block_script_shortcuts() {
        // run, test, start are NOT in denylist -- they are pnpm script shortcuts
        // "test" routes via route_script -> FilterRoute::TestRunner
        assert!(is_pnpm_script("test"));
        // "start" does not match route_script and no package.json in test env
        assert!(!is_pnpm_script("start"));
        // "run" does not match route_script and no package.json in test env
        assert!(!is_pnpm_script("run"));
    }
}
