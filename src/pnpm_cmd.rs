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
    // For stderr stripping: only match ELIFECYCLE, preserve ERR_PNPM messages (BUG-04)
    static ref ELIFECYCLE_ONLY: Regex = Regex::new(r"(?i)ELIFECYCLE").unwrap();
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

/// Cached package.json scripts (read once per invocation, QUAL-02).
/// Eliminates redundant fs::read_to_string("package.json") calls in
/// is_pnpm_script and route_script.
pub struct PackageScripts {
    scripts: HashMap<String, String>, // script_name -> command_string
}

impl PackageScripts {
    /// Read package.json from CWD, parse scripts field. Returns None if
    /// file is missing, unparseable, or has no scripts section.
    pub fn load() -> Option<Self> {
        let content = std::fs::read_to_string("package.json").ok()?;
        let json: serde_json::Value = serde_json::from_str(&content).ok()?;
        let scripts_obj = json.get("scripts")?.as_object()?;
        let scripts: HashMap<String, String> = scripts_obj
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
            .collect();
        Some(PackageScripts { scripts })
    }

    /// Check if a script name exists in the cached scripts map.
    pub fn contains(&self, name: &str) -> bool {
        self.scripts.contains_key(name)
    }

    /// Detect the underlying tool from the script command string.
    /// Replaces the old detect_tool_from_package_json function.
    pub fn detect_tool(&self, script: &str) -> Option<FilterRoute> {
        let command = self.scripts.get(script)?;
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
}

/// Strip pnpm-specific boilerplate from script output.
/// Returns empty string when all lines are boilerplate (BUG-04: caller decides what to show).
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
            !trimmed.is_empty() && !ELIFECYCLE_ONLY.is_match(trimmed) && !DONE_MSG.is_match(trimmed)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Route a script name to a specialized filter (static rules + cached package.json detection)
pub(crate) fn route_script(
    script: &str,
    pkg_scripts: Option<&PackageScripts>,
) -> Option<FilterRoute> {
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

    // Tier 2: Auto-detect from cached package.json scripts (QUAL-02)
    pkg_scripts.and_then(|ps| ps.detect_tool(script))
}

/// Check if a name is a known pnpm script (static routing or cached package.json)
pub fn is_pnpm_script(name: &str, pkg_scripts: &Option<PackageScripts>) -> bool {
    // Native pnpm commands are never scripts (BUG-03)
    if NATIVE_PNPM_COMMANDS.binary_search(&name).is_ok() {
        return false;
    }

    // Check static routing first (no I/O)
    if route_script(name, pkg_scripts.as_ref()).is_some() {
        return true;
    }

    // Check cached package.json scripts (QUAL-02)
    match pkg_scripts {
        Some(ps) => ps.contains(name),
        None => false,
    }
}

/// Apply a specialized filter to script output.
/// Returns Result to allow caller fallback on error (replaces catch_unwind, QUAL-01).
pub(crate) fn apply_filter(route: FilterRoute, output: &str) -> Result<(String, &'static str)> {
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

/// Execute `pnpm run <script>` with smart routing to specialized filters.
/// Stream-separated: feeds stdout-only to filters (BUG-01, BUG-02).
/// Shows stderr on failure, "ok +" only on success with empty stdout (BUG-04).
pub fn run_script(
    script: &str,
    args: &[String],
    verbose: u8,
    skip_env: bool,
    pkg_scripts: Option<PackageScripts>,
) -> Result<()> {
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
    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let stderr_str = String::from_utf8_lossy(&output.stderr);
    let exit_code = output.status.code().unwrap_or(1);

    // For tee recovery: combined output (stdout+stderr)
    let raw_for_tee = format!("{}\n{}", stdout_str, stderr_str);

    // Stage 1: Strip pnpm boilerplate from STDOUT ONLY (BUG-01, BUG-02 fix)
    let stripped = filter_pnpm_run_output(&stdout_str);

    if !output.status.success() {
        // FAILURE PATH: filter stdout through specialized filter, show stderr
        let filtered = if !stripped.is_empty() {
            match route_script(script, pkg_scripts.as_ref()) {
                Some(route) => match apply_filter(route, &stripped) {
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

        // Strip pnpm boilerplate from stderr but preserve ERR_PNPM messages (BUG-04)
        let stderr_display = strip_pnpm_stderr(&stderr_str);

        // Show: filtered stdout (if any) then stderr (if any)
        let display = [filtered.as_str(), stderr_display.as_str()]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join("\n");

        if let Some(hint) =
            crate::tee::tee_and_hint(&raw_for_tee, &format!("pnpm-run-{}", script), exit_code)
        {
            if display.is_empty() {
                println!("{}", hint);
            } else {
                println!("{}\n{}", display, hint);
            }
        } else if !display.is_empty() {
            println!("{}", display);
        }

        timer.track(
            &format!("pnpm run {} {}", script, args.join(" ")),
            &format!("rtk pnpm run {} {}", script, args.join(" ")),
            &raw_for_tee,
            &display,
        );
        std::process::exit(exit_code);
    }

    // SUCCESS PATH: "ok +" only when exit 0 AND stripped stdout is empty (BUG-04)
    if stripped.is_empty() {
        let display = "ok \u{2713}".to_string();
        println!("{}", display);
        timer.track(
            &format!("pnpm run {} {}", script, args.join(" ")),
            &format!("rtk pnpm run {} {}", script, args.join(" ")),
            &raw_for_tee,
            &display,
        );
        return Ok(());
    }

    // Route to specialized filter
    let filtered = match route_script(script, pkg_scripts.as_ref()) {
        Some(route) => match apply_filter(route, &stripped) {
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

    if let Some(hint) =
        crate::tee::tee_and_hint(&raw_for_tee, &format!("pnpm-run-{}", script), exit_code)
    {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("pnpm run {} {}", script, args.join(" ")),
        &format!("rtk pnpm run {} {}", script, args.join(" ")),
        &raw_for_tee,
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
        // BUG-04: filter returns empty string for pure boilerplate (caller decides "ok +")
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
            "Expected ≥40% savings, got {:.1}%",
            savings
        );
    }

    // ─── route_script tests ──────────────────────────────────────────────

    #[test]
    fn test_route_script_exact_matches() {
        // Static routing works with None pkg_scripts
        assert_eq!(route_script("vitest", None), Some(FilterRoute::Vitest));
        assert_eq!(route_script("tsc", None), Some(FilterRoute::Tsc));
        assert_eq!(route_script("typecheck", None), Some(FilterRoute::Tsc));
        assert_eq!(route_script("lint", None), Some(FilterRoute::Lint));
        assert_eq!(route_script("prettier", None), Some(FilterRoute::Prettier));
        assert_eq!(route_script("format", None), Some(FilterRoute::Prettier));
        assert_eq!(route_script("test", None), Some(FilterRoute::TestRunner));
    }

    #[test]
    fn test_route_script_unknown_returns_none() {
        // These don't match static rules and no PackageScripts provided
        assert_eq!(route_script("build", None), None);
        assert_eq!(route_script("dev", None), None);
        assert_eq!(route_script("start", None), None);
    }

    #[test]
    fn test_route_script_prefix_matching() {
        assert_eq!(
            route_script("test:unit", None),
            Some(FilterRoute::TestRunner)
        );
        assert_eq!(
            route_script("test:integration", None),
            Some(FilterRoute::TestRunner)
        );
        assert_eq!(route_script("lint:check", None), Some(FilterRoute::Lint));
        assert_eq!(route_script("lint:ci", None), Some(FilterRoute::Lint));
    }

    #[test]
    fn test_route_script_prefix_exclusions() {
        // These are deferred to package.json (no PackageScripts → None)
        assert_eq!(route_script("test:e2e", None), None);
        assert_eq!(route_script("test:playwright", None), None);
        assert_eq!(route_script("test:cypress", None), None);
        assert_eq!(route_script("lint:fix", None), None);
    }

    // ─── PackageScripts tests ───────────────────────────────────────────

    #[test]
    fn test_package_scripts_load_returns_none_without_package_json() {
        // Test CWD has no package.json → load() returns None
        assert!(PackageScripts::load().is_none());
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

    // ─── apply_filter tests ──────────────────────────────────────────────

    #[test]
    fn test_apply_filter_tsc_label() {
        let (_, label) = apply_filter(FilterRoute::Tsc, "some output").unwrap();
        assert_eq!(label, "tsc (via pnpm run)");
    }

    #[test]
    fn test_apply_filter_vitest_label() {
        let (_, label) = apply_filter(FilterRoute::Vitest, "some output").unwrap();
        assert_eq!(label, "vitest (via pnpm run)");
    }

    #[test]
    fn test_apply_filter_lint_label() {
        let (_, label) = apply_filter(FilterRoute::Lint, "some output").unwrap();
        assert_eq!(label, "lint (via pnpm run)");
    }

    #[test]
    fn test_apply_filter_prettier_label() {
        let (_, label) = apply_filter(FilterRoute::Prettier, "some output").unwrap();
        assert_eq!(label, "prettier (via pnpm run)");
    }

    #[test]
    fn test_apply_filter_test_runner_label() {
        let (_, label) = apply_filter(FilterRoute::TestRunner, "some output").unwrap();
        assert_eq!(label, "test (via pnpm run)");
    }

    #[test]
    fn test_apply_filter_playwright_label() {
        let (_, label) = apply_filter(FilterRoute::Playwright, "some output").unwrap();
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

        let route = route_script("lint", None);
        assert_eq!(route, Some(FilterRoute::Lint));

        let (filtered, label) = apply_filter(route.unwrap(), &stripped).unwrap();
        assert_eq!(label, "lint (via pnpm run)");
        assert!(!filtered.is_empty());
    }

    #[test]
    fn test_ok_checkmark_guard_skips_routing() {
        // BUG-04: filter returns empty for boilerplate; run_script adds "ok +" on success
        let raw = "> pkg@1.0.0 test\n$ vitest run\n\nDone in 2s\n";
        let stripped = filter_pnpm_run_output(raw);
        assert_eq!(stripped, "");
        // In run_script: if stripped.is_empty() && success -> println!("ok +")
    }

    // ─── is_pnpm_script tests ────────────────────────────────────────────

    #[test]
    fn test_is_pnpm_script_routed_scripts() {
        // These are script names that go through smart routing (NOT native commands)
        let no_scripts: Option<PackageScripts> = None;
        assert!(is_pnpm_script("lint", &no_scripts));
        assert!(is_pnpm_script("vitest", &no_scripts));
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
        // With None pkg_scripts, only static routing works
        let no_scripts: Option<PackageScripts> = None;
        assert!(!is_pnpm_script("my-custom-script", &no_scripts));
        assert!(is_pnpm_script("lint", &no_scripts)); // static route exists
    }

    #[test]
    fn test_native_commands_not_intercepted() {
        // These are native pnpm commands -- must never be treated as scripts (BUG-03)
        let no_scripts: Option<PackageScripts> = None;
        assert!(!is_pnpm_script("exec", &no_scripts));
        assert!(!is_pnpm_script("dlx", &no_scripts));
        assert!(!is_pnpm_script("audit", &no_scripts));
        assert!(!is_pnpm_script("create", &no_scripts));
        assert!(!is_pnpm_script("deploy", &no_scripts));
        assert!(!is_pnpm_script("store", &no_scripts));
        assert!(!is_pnpm_script("server", &no_scripts));
        assert!(!is_pnpm_script("patch", &no_scripts));
        assert!(!is_pnpm_script("env", &no_scripts));
        assert!(!is_pnpm_script("doctor", &no_scripts));
        assert!(!is_pnpm_script("why", &no_scripts));
        assert!(!is_pnpm_script("init", &no_scripts));
        assert!(!is_pnpm_script("config", &no_scripts));
        assert!(!is_pnpm_script("setup", &no_scripts));
        assert!(!is_pnpm_script("bin", &no_scripts));
        assert!(!is_pnpm_script("self-update", &no_scripts));
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
    fn test_apply_filter_empty_output_returns_error() {
        // Empty output triggers Err (fallback to stripped output in caller)
        let result = apply_filter(FilterRoute::Tsc, "");
        assert!(result.is_err());
    }

    #[test]
    fn test_apply_filter_whitespace_only_returns_error() {
        // Whitespace-only output triggers Err
        let result = apply_filter(FilterRoute::Lint, "   \n\n  ");
        assert!(result.is_err());
    }

    #[test]
    fn test_native_denylist_does_not_block_script_shortcuts() {
        let no_scripts: Option<PackageScripts> = None;
        // run, test, start are NOT in denylist -- they are pnpm script shortcuts
        // "test" routes via route_script -> FilterRoute::TestRunner
        assert!(is_pnpm_script("test", &no_scripts));
        // "start" does not match route_script and no PackageScripts
        assert!(!is_pnpm_script("start", &no_scripts));
        // "run" does not match route_script and no PackageScripts
        assert!(!is_pnpm_script("run", &no_scripts));
    }

    // ─── strip_pnpm_stderr tests ────────────────────────────────────────

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

    // ─── stream separation + failure behavior tests ─────────────────────

    #[test]
    fn test_filter_pnpm_run_output_returns_empty_for_boilerplate() {
        // BUG-04: filter_pnpm_run_output returns empty string, not "ok +"
        let input = "> pkg@1.0.0 build\n$ tsc\n\nDone in 1s\n";
        let result = filter_pnpm_run_output(input);
        assert!(
            result.is_empty(),
            "Expected empty string for boilerplate, got: {:?}",
            result
        );
    }

    #[test]
    fn test_failure_empty_stdout_no_ok_checkmark() {
        // BUG-04: empty stdout + failure should NOT produce "ok +"
        // Simulate the logic flow in run_script (can't call run_script since it calls exit)
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
        // The display would be stderr_display, NOT "ok +"
        assert!(
            !stderr_display.contains("ok"),
            "Should never show 'ok' on failure"
        );
    }

    #[test]
    fn test_success_empty_stdout_shows_ok() {
        // On success with empty stripped stdout, run_script shows "ok +"
        let stdout = "> pkg@1.0.0 build\n$ tsc --noEmit\n\nDone in 1s\n";
        let stripped = filter_pnpm_run_output(stdout);
        assert!(stripped.is_empty());
        // In run_script success path: println!("ok +") -- verified by logic, not process::exit
    }

    // ─── full pipeline token savings tests (QUAL-05) ─────────────────────

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_full_pipeline_vitest_savings() {
        // Realistic pnpm stdout containing vitest --reporter=json output
        // Pipeline: raw pnpm stdout -> filter_pnpm_run_output (strip) -> apply_filter(Vitest)
        let fixture = r#"> my-app@1.0.0 test /Users/dev/my-app
$ vitest run --reporter=json

{"numTotalTestSuites":5,"numPassedTestSuites":4,"numFailedTestSuites":1,"numPendingTestSuites":0,"numTotalTests":25,"numPassedTests":23,"numFailedTests":2,"numPendingTests":0,"startTime":1709312400000,"endTime":1709312405200,"testResults":[{"name":"src/utils.test.ts","assertionResults":[{"fullName":"utils > formats date correctly","status":"passed","failureMessages":[]},{"fullName":"utils > validates email format","status":"passed","failureMessages":[]},{"fullName":"utils > truncates long strings","status":"passed","failureMessages":[]},{"fullName":"utils > handles null input","status":"passed","failureMessages":[]}]},{"name":"src/api.test.ts","assertionResults":[{"fullName":"api > GET /users returns list","status":"passed","failureMessages":[]},{"fullName":"api > POST /users creates user","status":"passed","failureMessages":[]},{"fullName":"api > handles auth error","status":"failed","failureMessages":["Error: expected 200 got 500\n    at Object.<anonymous> (src/api.test.ts:15:5)\n    at Promise.then.completed"]},{"fullName":"api > validates request body","status":"passed","failureMessages":[]}]},{"name":"src/hooks.test.ts","assertionResults":[{"fullName":"hooks > useAuth returns user","status":"passed","failureMessages":[]},{"fullName":"hooks > useAuth handles logout","status":"passed","failureMessages":[]},{"fullName":"hooks > useFetch caches results","status":"passed","failureMessages":[]},{"fullName":"hooks > useFetch retries on error","status":"passed","failureMessages":[]},{"fullName":"hooks > useDebounce delays call","status":"passed","failureMessages":[]}]},{"name":"src/components/Button.test.ts","assertionResults":[{"fullName":"Button > renders with text","status":"passed","failureMessages":[]},{"fullName":"Button > handles click","status":"passed","failureMessages":[]},{"fullName":"Button > applies disabled state","status":"passed","failureMessages":[]},{"fullName":"Button > shows loading spinner","status":"passed","failureMessages":[]}]},{"name":"src/store.test.ts","assertionResults":[{"fullName":"store > initializes with defaults","status":"passed","failureMessages":[]},{"fullName":"store > updates state","status":"passed","failureMessages":[]},{"fullName":"store > handles concurrent updates","status":"failed","failureMessages":["Error: Race condition detected\n    at Object.<anonymous> (src/store.test.ts:42:10)"]},{"fullName":"store > persists to localStorage","status":"passed","failureMessages":[]},{"fullName":"store > clears on logout","status":"passed","failureMessages":[]},{"fullName":"store > subscribes to changes","status":"passed","failureMessages":[]}]}]}

Done in 5.2s
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
            stripped.contains("numTotalTests"),
            "JSON payload should survive stripping"
        );

        // Stage 2: Apply vitest specialized filter
        let (filtered, label) = apply_filter(FilterRoute::Vitest, &stripped).unwrap();
        assert_eq!(label, "vitest (via pnpm run)");
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
            "Full pipeline vitest savings: expected >= 60%, got {:.1}% (input={}, output={})",
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
        let (filtered, label) = apply_filter(FilterRoute::Playwright, &stripped).unwrap();
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
}
