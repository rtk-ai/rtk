//! First-class Composer support: compact dependency logs and JSON-backed reports.

use crate::core::runner;
use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::utils::{resolved_command, strip_ansi, truncate};
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashSet;
use std::ffi::OsString;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComposerMode {
    DependencyLog,
    JsonReport,
    Passthrough,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ComposerPlan {
    mode: ComposerMode,
    command: String,
    command_index: usize,
}

const COMPOSER_GLOBAL_FLAGS_WITH_VALUE: &[&str] = &[
    "-d",
    "--working-dir",
    "--profile",
    "--ansi",
    "--no-ansi",
    "--no-interaction",
    "--no-plugins",
    "--no-scripts",
    "--no-cache",
    "--quiet",
    "--verbose",
    "-q",
    "-v",
    "-vv",
    "-vvv",
];

const COMPOSER_PASSTHROUGH_FLAGS: &[&str] = &["--help", "-h"];

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let plan = classify_args(args);

    match plan.mode {
        ComposerMode::Passthrough => run_passthrough(args, verbose),
        ComposerMode::DependencyLog => run_filtered(args, verbose),
        ComposerMode::JsonReport => run_json_report(args, &plan, verbose),
    }
}

fn run_passthrough(args: &[String], verbose: u8) -> Result<i32> {
    let os_args: Vec<OsString> = args.iter().map(OsString::from).collect();
    runner::run_passthrough("composer", &os_args, verbose)
}

fn run_filtered(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let mut cmd = resolved_command("composer");
    cmd.args(args);

    if verbose > 0 {
        eprintln!("Running: composer {}", args.join(" "));
    }

    let result = exec_capture(&mut cmd).context("Failed to run composer")?;
    let raw = result.combined();
    let filtered = filter_dependency_log_result(&raw, result.exit_code);
    println!("{}", filtered);

    timer.track(
        &format!("composer {}", args.join(" ")),
        &format!("rtk composer {}", args.join(" ")),
        &raw,
        &filtered,
    );

    Ok(result.exit_code)
}

fn run_json_report(args: &[String], plan: &ComposerPlan, verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let mut injected_args = args.to_vec();
    injected_args.insert(plan.command_index + 1, "--format=json".to_string());

    let mut cmd = resolved_command("composer");
    cmd.args(&injected_args);

    if verbose > 0 {
        eprintln!("Running: composer {}", injected_args.join(" "));
    }

    let result = exec_capture(&mut cmd).context("Failed to run composer")?;
    let raw = result.combined();
    let filtered = match serde_json::from_str::<Value>(result.stdout.trim()) {
        Ok(json) => format_json_report(&plan.command, &json),
        Err(_) => fallback_tail(&raw),
    };

    println!("{}", filtered);
    timer.track(
        &format!("composer {}", args.join(" ")),
        &format!("rtk composer {}", args.join(" ")),
        &raw,
        &filtered,
    );

    Ok(result.exit_code)
}

fn classify_args(args: &[String]) -> ComposerPlan {
    if args.is_empty()
        || has_user_format(args)
        || has_user_passthrough_flag(args)
        || has_show_tree_flag(args)
    {
        return passthrough_plan();
    }

    let Some((cmd, idx)) = find_command(args, 0) else {
        return passthrough_plan();
    };

    if cmd == "global" {
        let Some((inner, inner_idx)) = find_command(args, idx + 1) else {
            return passthrough_plan();
        };
        return plan_for_command(inner, inner_idx);
    }

    plan_for_command(cmd, idx)
}

fn find_command(args: &[String], start: usize) -> Option<(&str, usize)> {
    let mut i = start;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--" {
            return None;
        }
        if is_global_flag_with_optional_value(arg) {
            if flag_takes_separate_value(arg) {
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if arg.starts_with('-') {
            i += 1;
            continue;
        }
        return Some((arg, i));
    }
    None
}

fn is_global_flag_with_optional_value(arg: &str) -> bool {
    let flag = arg.split_once('=').map(|(flag, _)| flag).unwrap_or(arg);
    COMPOSER_GLOBAL_FLAGS_WITH_VALUE.contains(&flag)
}

fn flag_takes_separate_value(arg: &str) -> bool {
    matches!(arg, "-d" | "--working-dir")
}

fn has_user_format(args: &[String]) -> bool {
    args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "--format" | "-f" | "--json" | "--raw" | "--audit-format"
        ) || arg.starts_with("--format=")
            || arg.starts_with("--audit-format=")
    })
}

fn has_user_passthrough_flag(args: &[String]) -> bool {
    args.iter().any(|arg| COMPOSER_PASSTHROUGH_FLAGS.contains(&arg.as_str()))
}

fn has_show_tree_flag(args: &[String]) -> bool {
    let Some((cmd, idx)) = find_command(args, 0) else {
        return false;
    };
    let (cmd, idx) = if cmd == "global" {
        let Some((inner, inner_idx)) = find_command(args, idx + 1) else {
            return false;
        };
        (inner, inner_idx)
    } else {
        (cmd, idx)
    };

    matches!(cmd, "show" | "info") && args[idx + 1..].iter().any(|arg| matches!(arg.as_str(), "--tree" | "-t"))
}

fn passthrough_plan() -> ComposerPlan {
    ComposerPlan {
        mode: ComposerMode::Passthrough,
        command: String::new(),
        command_index: 0,
    }
}

fn plan_for_command(command: &str, command_index: usize) -> ComposerPlan {
    let canonical = canonical_command(command);
    let mode = if is_dependency_log(canonical) {
        ComposerMode::DependencyLog
    } else if is_json_report(canonical) {
        ComposerMode::JsonReport
    } else {
        ComposerMode::Passthrough
    };

    ComposerPlan {
        mode,
        command: canonical.to_string(),
        command_index,
    }
}

fn canonical_command(command: &str) -> &str {
    match command {
        "i" => "install",
        "u" | "upgrade" => "update",
        "r" => "require",
        "dumpautoload" => "dump-autoload",
        "info" => "show",
        other => other,
    }
}

fn is_dependency_log(command: &str) -> bool {
    matches!(command, "install" | "update" | "require" | "dump-autoload")
}

fn is_json_report(command: &str) -> bool {
    matches!(
        command,
        "show" | "licenses" | "check-platform-reqs" | "fund"
    )
}

fn filter_dependency_log(output: &str) -> String {
    let mut items = Vec::new();
    let mut saw_meaningful = false;
    let mut saw_action = false;
    let mut saw_noop = false;

    for raw in strip_ansi(output).lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }

        if is_dependency_noop(line) {
            saw_noop = true;
            continue;
        }

        if is_dependency_noise(line) {
            continue;
        }

        let normalized = normalize_dependency_line(line);

        if let Some(action) = compact_dependency_action(normalized) {
            saw_meaningful = true;
            saw_action = true;
            items.push(DependencyLogItem::Action(action));
            continue;
        }

        if is_dependency_keep(normalized) {
            saw_meaningful = true;
            items.push(DependencyLogItem::Line(normalized.to_string()));
        }
    }

    if !saw_meaningful || (saw_noop && !saw_action && items.iter().all(is_noop_boilerplate_item)) {
        return "ok (up to date)".to_string();
    }

    let mut lines = compact_dependency_items(items);
    if saw_action {
        lines.insert(0, action_legend().to_string());
    }

    cap_lines(lines, 80)
}

fn filter_dependency_log_result(output: &str, exit_code: i32) -> String {
    if exit_code == 0 {
        filter_dependency_log(output)
    } else {
        fallback_tail(output)
    }
}

#[derive(Debug, Clone)]
enum DependencyLogItem {
    Line(String),
    Action(DependencyAction),
}

#[derive(Debug, Clone)]
struct DependencyAction {
    code: &'static str,
    package: String,
    detail: String,
}

fn compact_dependency_items(items: Vec<DependencyLogItem>) -> Vec<String> {
    let installed_or_changed: HashSet<String> = items
        .iter()
        .filter_map(|item| match item {
            DependencyLogItem::Action(action) if action.code != "L" => Some(action.package.clone()),
            _ => None,
        })
        .collect();

    let mut seen_actions = HashSet::new();
    let mut lines = Vec::new();

    for item in items {
        match item {
            DependencyLogItem::Line(line) => lines.push(line),
            DependencyLogItem::Action(action) => {
                if action.code == "L" && installed_or_changed.contains(&action.package) {
                    continue;
                }

                let key = format!("{} {}", action.code, action.package);
                if !seen_actions.insert(key) {
                    continue;
                }

                lines.push(format!("{} {}", action.code, action.detail));
            }
        }
    }

    lines
}

fn is_dependency_noise(line: &str) -> bool {
    line.starts_with("Loading composer")
        || line.starts_with("Downloading ")
        || line.starts_with("Installing dependencies")
        || line.starts_with("Updating dependencies")
        || line.starts_with("Use the `composer fund`")
        || line.contains("packages you are using are looking for funding")
        || line.starts_with("Nothing to install")
        || line.starts_with("Nothing to modify")
        || line.starts_with("Nothing to update")
        || line.starts_with("No security vulnerability advisories found")
}

fn is_dependency_noop(line: &str) -> bool {
    line.starts_with("Nothing to install")
        || line.starts_with("Nothing to modify")
        || line.starts_with("Nothing to update")
}

fn is_noop_boilerplate_item(item: &DependencyLogItem) -> bool {
    match item {
        DependencyLogItem::Line(line) => is_noop_boilerplate_line(line),
        DependencyLogItem::Action(_) => false,
    }
}

fn is_noop_boilerplate_line(line: &str) -> bool {
    line.starts_with("Verifying lock file contents")
        || line.starts_with("Generating autoload")
        || line.starts_with("Generating optimized autoload")
}

fn normalize_dependency_line(line: &str) -> &str {
    let line = line.trim_start();
    if let Some(rest) = line.strip_prefix("- ") {
        rest
    } else if let Some(rest) = line.strip_prefix("+ ") {
        rest
    } else {
        line
    }
}

fn compact_dependency_action(line: &str) -> Option<DependencyAction> {
    const ACTION_PREFIXES: [(&str, &str); 6] = [
        ("Locking ", "L"),
        ("Installing ", "I"),
        ("Updating ", "U"),
        ("Upgrading ", "U"),
        ("Downgrading ", "D"),
        ("Removing ", "R"),
    ];

    for (action, code) in ACTION_PREFIXES {
        if let Some(rest) = line.strip_prefix(action) {
            let detail = rest.split(':').next().unwrap_or(line).trim();
            let package = detail
                .split_whitespace()
                .next()
                .unwrap_or(detail)
                .to_string();
            return Some(DependencyAction {
                code,
                package,
                detail: detail.to_string(),
            });
        }
    }

    None
}

fn action_legend() -> &'static str {
    "L=lock I=install U=upgrade D=downgrade R=remove"
}

fn is_autoload_class_warning(line: &str) -> bool {
    line.starts_with("Class ") && line.contains("does not comply")
}

fn is_dependency_keep(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("lock file")
        || lower.contains("warning")
        || lower.contains("error")
        || lower.contains("abandoned")
        || lower.contains("vulnerab")
        || lower.contains("requires")
        || lower.contains("conflict")
        || lower.contains("failed")
        || lower.contains("composer.json has been updated")
        || is_autoload_class_warning(line)
        || line.starts_with("Package operations:")
        || line.starts_with("Generating autoload")
        || line.starts_with("Generating optimized autoload")
        || line.starts_with("Installing ")
        || line.starts_with("Updating ")
        || line.starts_with("Removing ")
        || line.starts_with("Downgrading ")
        || line.starts_with("Upgrading ")
}

fn format_json_report(command: &str, json: &Value) -> String {
    match command {
        "show" => format_show(collect_packages(json)),
        "licenses" => format_licenses(json),
        "check-platform-reqs" => format_platform_reqs(json),
        "fund" => format_fund(json),
        _ => fallback_tail(&json.to_string()),
    }
}

#[derive(Debug, Clone)]
struct PackageRow {
    name: String,
    current: Option<String>,
    latest: Option<String>,
    detail: Option<String>,
}

fn collect_packages(json: &Value) -> Vec<PackageRow> {
    let mut rows = match json {
        Value::Array(items) => items.iter().filter_map(package_row_from_value).collect(),
        Value::Object(map) => {
            if is_package_object(json) {
                vec![package_row_from_value(json)]
                    .into_iter()
                    .flatten()
                    .collect()
            } else {
                ["installed", "packages", "platform"]
                    .iter()
                    .filter_map(|key| map.get(*key).and_then(Value::as_array))
                    .flat_map(|items| items.iter().filter_map(package_row_from_value))
                    .collect()
            }
        }
        _ => Vec::new(),
    };
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows.dedup_by(|a, b| a.name == b.name && a.current == b.current && a.latest == b.latest);
    rows
}

fn is_package_object(value: &Value) -> bool {
    string_field(value, &["name", "package"]).is_some()
        && (string_field(value, &["version", "current", "installed", "latest", "wanted"]).is_some()
            || value.get("versions").and_then(Value::as_array).is_some()
            || string_field(value, &["description"]).is_some())
}

fn package_row_from_value(value: &Value) -> Option<PackageRow> {
    if !is_package_object(value) {
        return None;
    }

    let name = string_field(value, &["name", "package"])?;
    let current = string_field(value, &["version", "current", "installed"])
        .or_else(|| selected_version_from_versions(value));
    let latest = string_field(value, &["latest", "latest-status", "wanted"]);
    let detail = string_field(value, &["description", "license", "status"]);

    Some(PackageRow {
        name,
        current,
        latest,
        detail,
    })
}

fn selected_version_from_versions(value: &Value) -> Option<String> {
    let versions = value.get("versions").and_then(Value::as_array)?;
    versions
        .iter()
        .filter_map(Value::as_str)
        .find_map(|version| version.strip_prefix("* ").or_else(|| version.strip_prefix('*')))
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .map(str::to_string)
        .or_else(|| {
            versions
                .iter()
                .filter_map(Value::as_str)
                .find(|version| !version.trim().is_empty())
                .map(|version| version.trim().to_string())
        })
}

fn string_field(value: &Value, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        value
            .get(*name)
            .and_then(Value::as_str)
            .map(|s| s.to_string())
    })
}

fn string_array_field(value: &Value, name: &str) -> Vec<String> {
    value
        .get(name)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn format_show(rows: Vec<PackageRow>) -> String {
    if rows.is_empty() {
        return "0 packages".to_string();
    }

    format_package_rows(rows)
}

fn format_package_rows(rows: Vec<PackageRow>) -> String {
    let total = rows.len();
    let mut out = String::new();
    for (idx, row) in rows.into_iter().take(30).enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        out.push_str(&row.name);
        match (row.current.as_deref(), row.latest.as_deref()) {
            (Some(current), Some(latest)) if current != latest => {
                out.push_str(&format!(": {} -> {}", current, latest));
            }
            (Some(current), _) => out.push_str(&format!(": {}", current)),
            _ => {}
        }
        if let Some(detail) = row.detail {
            out.push_str(&format!(" ({})", truncate(&detail, 80)));
        }
    }
    if total > 30 {
        out.push_str(&format!("\n... +{} more packages", total - 30));
    }
    out
}

fn format_platform_reqs(json: &Value) -> String {
    let Value::Array(items) = json else {
        return fallback_tail(&json.to_string());
    };

    if items.is_empty() {
        return "composer check-platform-reqs: ok".to_string();
    }

    let mut out = "S=success F=failed".to_string();
    for item in items.iter().take(30) {
        let name = string_field(item, &["name"]).unwrap_or_else(|| "requirement".to_string());
        let version = string_field(item, &["version"]).unwrap_or_else(|| "?".to_string());
        let status = string_field(item, &["status"]).unwrap_or_else(|| "unknown".to_string());
        let code = platform_status_code(&status);

        out.push('\n');
        out.push_str(code);
        out.push(' ');
        out.push_str(&name);
        out.push(' ');
        out.push_str(&version);

        if status != "success" && status != "failed" {
            out.push_str(&format!(" ({})", truncate(&status, 40)));
        }

        if let Some(provider) = string_field(item, &["provider"]) {
            out.push_str(&format!(" ({})", truncate(&provider, 60)));
        }

        if status != "success" {
            if let Some(req) = failed_requirement_summary(item) {
                out.push_str(&format!(" ({})", truncate(&req, 80)));
            }
        }
    }
    if items.len() > 30 {
        out.push_str(&format!("\n... +{} more requirements", items.len() - 30));
    }
    out
}

fn platform_status_code(status: &str) -> &'static str {
    match status {
        "success" => "S",
        "failed" | "missing" => "F",
        _ => "?",
    }
}

fn failed_requirement_summary(item: &Value) -> Option<String> {
    let req = item.get("failed_requirement")?;
    let source = string_field(req, &["source"])?;
    let kind = string_field(req, &["type"]).unwrap_or_else(|| "requires".to_string());
    let target = string_field(req, &["target"]).unwrap_or_default();
    let constraint = string_field(req, &["constraint"]).unwrap_or_default();
    Some(format!("{} {} {} {}", source, kind, target, constraint)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" "))
}

fn format_licenses(json: &Value) -> String {
    let Value::Object(map) = json else {
        return fallback_tail(&json.to_string());
    };

    let mut lines = Vec::new();
    if let Some(root) = license_line(json, None) {
        lines.push(root);
    }

    if let Some(deps) = map.get("dependencies").and_then(Value::as_object) {
        if !deps.is_empty() {
            lines.push(format!("deps {}", deps.len()));
            let mut names: Vec<_> = deps.keys().collect();
            names.sort();
            for name in names {
                if let Some(line) = deps
                    .get(name)
                    .and_then(|dep| license_line(dep, Some(name)))
                {
                    lines.push(line);
                }
            }
        }
    }

    if lines.is_empty() {
        "0 packages".to_string()
    } else {
        cap_items(lines, 42, "packages")
    }
}

fn license_line(value: &Value, fallback_name: Option<&str>) -> Option<String> {
    let name = string_field(value, &["name"])
        .or_else(|| fallback_name.map(str::to_string))?;
    let version = string_field(value, &["version"]).unwrap_or_else(|| "?".to_string());
    let licenses = license_summary(value);
    Some(format!("{} {} {}", name, version, licenses))
}

fn license_summary(value: &Value) -> String {
    let licenses = string_array_field(value, "license");
    if licenses.is_empty() {
        "?".to_string()
    } else {
        licenses.join(",")
    }
}

fn format_fund(json: &Value) -> String {
    let Value::Object(groups) = json else {
        if matches!(json, Value::Array(items) if items.is_empty()) {
            return "0 funding links".to_string();
        }
        return fallback_tail(&json.to_string());
    };

    let mut lines = Vec::new();
    let mut vendors: Vec<_> = groups.keys().collect();
    vendors.sort();

    for vendor in vendors {
        let Some(links) = groups.get(vendor).and_then(Value::as_object) else {
            continue;
        };

        let mut urls: Vec<_> = links.keys().collect();
        urls.sort();

        let mut parts = Vec::new();
        for url in urls {
            let packages = links
                .get(url)
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();

            if packages.is_empty() {
                parts.push(short_url(url));
            } else {
                parts.push(format!("{} ({})", short_url(url), packages));
            }
        }

        if !parts.is_empty() {
            lines.push(truncate(&format!("{}: {}", vendor, parts.join("; ")), 180));
        }
    }

    if lines.is_empty() {
        "0 funding links".to_string()
    } else {
        lines.insert(
            0,
            "gh=GitHub tl=Tidelift oc=OpenCollective pp=PayPal pt=Patreon lp=Liberapay"
                .to_string(),
        );
        cap_items(lines, 30, "vendors")
    }
}

fn short_url(url: &str) -> String {
    let mut short = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    short = short.strip_prefix("www.").unwrap_or(short);
    let end = short.find(['?', '#']).unwrap_or(short.len());
    let path = &short[..end];
    short = path.trim_end_matches('/');

    const PREFIXES: [(&str, &str); 8] = [
        ("github.com/sponsors/", "gh:"),
        ("tidelift.com/funding/github/packagist/", "tl:"),
        ("tidelift.com/subscription/pkg/packagist-", "tl:"),
        ("opencollective.com/", "oc:"),
        ("paypal.com/paypalme/", "pp:"),
        ("paypal.me/", "pp:"),
        ("patreon.com/", "pt:"),
        ("liberapay.com/", "lp:"),
    ];

    for (prefix, code) in PREFIXES {
        if let Some(rest) = short.strip_prefix(prefix) {
            return format!("{}{}", code, rest.replace("%2F", "/"));
        }
    }

    short.to_string()
}

fn fallback_tail(output: &str) -> String {
    let mut lines: Vec<String> = strip_ansi(output)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| truncate(line, 180))
        .collect();
    if lines.is_empty() {
        return "composer: no output".to_string();
    }
    if lines.len() > 40 {
        lines = lines.split_off(lines.len() - 40);
    }
    lines.join("\n")
}

fn cap_lines(mut lines: Vec<String>, max: usize) -> String {
    if lines.len() > max {
        let omitted = lines.len() - max;
        lines.truncate(max);
        lines.push(format!("... +{} more lines", omitted));
    }
    lines.join("\n")
}

fn cap_items(mut lines: Vec<String>, max: usize, label: &str) -> String {
    if lines.len() > max {
        let omitted = lines.len() - max;
        lines.truncate(max);
        lines.push(format!("... +{} more {}", omitted, label));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn classifies_dependency_log_commands_and_aliases() {
        for command in [
            "install",
            "i",
            "update",
            "u",
            "upgrade",
            "require",
            "r",
            "dump-autoload",
            "dumpautoload",
        ] {
            assert_eq!(
                classify_args(&args(&[command])).mode,
                ComposerMode::DependencyLog,
                "{command} should use dependency-log mode"
            );
        }

        let plan = classify_args(&args(&["-d", "app", "i"]));
        assert_eq!(plan.mode, ComposerMode::DependencyLog);
        assert_eq!(plan.command, "install");
        assert_eq!(plan.command_index, 2);

        let plan = classify_args(&args(&["u"]));
        assert_eq!(plan.command, "update");

        let plan = classify_args(&args(&["r"]));
        assert_eq!(plan.command, "require");
    }

    #[test]
    fn classifies_json_report_commands_and_aliases() {
        for command in ["show", "info", "licenses", "fund", "check-platform-reqs"] {
            assert_eq!(
                classify_args(&args(&["--working-dir=app", command])).mode,
                ComposerMode::JsonReport,
                "{command} should use JSON-report mode"
            );
        }
    }

    #[test]
    fn classifies_global_inner_command() {
        let plan = classify_args(&args(&["global", "show"]));
        assert_eq!(plan.mode, ComposerMode::JsonReport);
        assert_eq!(plan.command, "show");
        assert_eq!(plan.command_index, 1);

        let plan = classify_args(&args(&["global", "info"]));
        assert_eq!(plan.mode, ComposerMode::JsonReport);
        assert_eq!(plan.command, "show");
        assert_eq!(plan.command_index, 1);

        let plan = classify_args(&args(&["global", "require", "vendor/package"]));
        assert_eq!(plan.mode, ComposerMode::DependencyLog);
        assert_eq!(plan.command, "require");
        assert_eq!(plan.command_index, 1);
    }

    #[test]
    fn passthrough_for_dropped_commands_user_format_unknown_and_help() {
        for command in [
            "remove",
            "reinstall",
            "create-project",
            "outdated",
            "audit",
            "search",
            "validate",
            "diagnose",
            "status",
            "suggests",
            "depends",
            "prohibits",
            "bump",
            "clear-cache",
            "self-update",
        ] {
            assert_eq!(
                classify_args(&args(&[command])).mode,
                ComposerMode::Passthrough,
                "{command} should passthrough"
            );
        }
        assert_eq!(
            classify_args(&args(&["show", "--format=json"])).mode,
            ComposerMode::Passthrough
        );
        assert_eq!(
            classify_args(&args(&["show", "--tree"])).mode,
            ComposerMode::Passthrough
        );
        assert_eq!(
            classify_args(&args(&["show", "vendor/package", "-t"])).mode,
            ComposerMode::Passthrough
        );
        assert_eq!(
            classify_args(&args(&["--working-dir=app", "show", "--tree"])).mode,
            ComposerMode::Passthrough
        );
        assert_eq!(
            classify_args(&args(&["global", "show", "--tree"])).mode,
            ComposerMode::Passthrough
        );
        assert_eq!(classify_args(&args(&["exec", "phpunit"])).mode, ComposerMode::Passthrough);
        assert_eq!(classify_args(&args(&["--help"])).mode, ComposerMode::Passthrough);
    }

    #[test]
    fn filters_dependency_noop_and_keeps_warnings() {
        let out = "\x1b[32mLoading composer repositories\x1b[0m\nNothing to install, update or remove\n";
        assert_eq!(filter_dependency_log(out), "ok (up to date)");

        let out = "Installing dependencies from lock file (including require-dev)\nVerifying lock file contents can be installed on current platform.\nNothing to install, update or remove\nGenerating autoload files\n";
        assert_eq!(filter_dependency_log(out), "ok (up to date)");

        let out = "Package foo/bar is abandoned, you should avoid using it.\n  - Installing a/b (1.0.0)\n";
        let filtered = filter_dependency_log(out);
        assert!(filtered.contains("abandoned"));
        assert!(filtered.contains(action_legend()));
        assert!(filtered.contains("I a/b"));

        let out = "  - Locking a/b (1.0.0)\n  - Upgrading c/d (1.0.0 => 2.0.0): Extracting archive\n  - Downgrading e/f (2.0.0 => 1.0.0)\n  - Removing g/h (1.0.0)\n";
        let filtered = filter_dependency_log(out);
        assert!(filtered.contains("L a/b"));
        assert!(filtered.contains("U c/d"));
        assert!(filtered.contains("D e/f"));
        assert!(filtered.contains("R g/h"));

        let out = "Lock file operations: 1 install, 0 updates, 0 removals\nWriting lock file\nGenerating autoload files\n";
        let filtered = filter_dependency_log(out);
        assert!(filtered.contains("Lock file operations"));
        assert!(filtered.contains("Writing lock file"));
        assert!(filtered.contains("Generating autoload"));

        let out = "Generating optimized autoload files\nClass App\\Bar located in ./app/Foo.php does not comply with psr-4 autoloading standard. Skipping.\n";
        let filtered = filter_dependency_log(out);
        assert!(filtered.contains("Generating optimized autoload files"));
        assert!(filtered.contains("Class App\\Bar located"));
        assert!(filtered.contains("does not comply"));
    }

    #[test]
    fn failed_dependency_log_falls_back_to_raw_output() {
        let out = "Loading composer repositories with package information\nCould not find package no/such-package-xyz.\n";
        let filtered = filter_dependency_log_result(out, 1);

        assert!(filtered.contains("Could not find package no/such-package-xyz."));
        assert_ne!(filtered, "ok (up to date)");
    }

    #[test]
    fn filters_require_log_to_package_actions() {
        let out = "./composer.json has been updated\nRunning composer update monolog/monolog\nLoading composer repositories with package information\nUpdating dependencies\nLock file operations: 2 installs, 0 updates, 0 removals\n  - Locking monolog/monolog (3.8.1)\n  - Locking psr/log (3.0.2)\nWriting lock file\nInstalling dependencies from lock file (including require-dev)\nPackage operations: 2 installs, 0 updates, 0 removals\n  - Downloading psr/log (3.0.2)\n  - Downloading monolog/monolog (3.8.1)\n  - Installing psr/log (3.0.2): Extracting archive\n  - Installing monolog/monolog (3.8.1): Extracting archive\nGenerating autoload files\n12 packages you are using are looking for funding.\nUse the `composer fund` command to find out more!\nNo security vulnerability advisories found.\n";
        let filtered = filter_dependency_log(out);

        assert!(filtered.contains("composer.json has been updated"));
        assert!(filtered.contains("Lock file operations: 2 installs, 0 updates, 0 removals"));
        assert!(filtered.contains("Package operations: 2 installs, 0 updates, 0 removals"));
        assert!(filtered.contains("I psr/log (3.0.2)"));
        assert!(filtered.contains("I monolog/monolog (3.8.1)"));
        assert!(!filtered.contains("L psr/log"));
        assert!(!filtered.contains("L monolog/monolog"));
        assert!(!filtered.contains("Updating dependencies"));
        assert!(!filtered.contains("funding"));
    }

    #[test]
    fn filters_update_log_deduplicates_lock_and_install_rows() {
        let out = "Loading composer repositories with package information\nUpdating dependencies\nLock file operations: 0 installs, 2 updates, 0 removals\n  - Upgrading symfony/console (6.4.8 => 6.4.12)\n  - Downgrading doctrine/dbal (4.1.0 => 3.9.3)\nWriting lock file\nInstalling dependencies from lock file (including require-dev)\nPackage operations: 0 installs, 2 updates, 0 removals\n  - Downloading symfony/console (6.4.12)\n  - Downloading doctrine/dbal (3.9.3)\n  - Upgrading symfony/console (6.4.8 => 6.4.12): Extracting archive\n  - Downgrading doctrine/dbal (4.1.0 => 3.9.3): Extracting archive\nGenerating optimized autoload files\nPackage doctrine/cache is abandoned, you should avoid using it.\n";
        let filtered = filter_dependency_log(out);

        assert_eq!(filtered.matches("U symfony/console").count(), 1);
        assert_eq!(filtered.matches("D doctrine/dbal").count(), 1);
        assert!(filtered.contains("Package doctrine/cache is abandoned"));
        assert!(filtered.contains("Generating optimized autoload files"));
        assert!(!filtered.contains("Updating dependencies"));
        assert!(!filtered.contains("Downloading "));
    }

    #[test]
    fn formats_show_without_header() {
        let json = serde_json::json!({
            "installed": [
                {"name": "symfony/console", "version": "6.4.0", "description": "Console tools"}
            ]
        });
        let out = format_json_report("show", &json);
        assert!(!out.contains("composer show:"));
        assert_eq!(out, "symfony/console: 6.4.0 (Console tools)");
    }

    #[test]
    fn formats_single_show_package_without_nested_license_objects() {
        let json = serde_json::json!({
            "name": "psr/log",
            "description": "Common interface for logging libraries",
            "versions": ["* 3.0.2"],
            "licenses": [
                {"name": "MIT License", "url": "https://opensource.org/licenses/MIT"}
            ]
        });

        let out = format_json_report("show", &json);

        assert_eq!(
            out,
            "psr/log: 3.0.2 (Common interface for logging libraries)"
        );
        assert!(!out.contains("MIT License"));
    }

    #[test]
    fn formats_show_with_omitted_package_count() {
        let installed: Vec<serde_json::Value> = (0..31)
            .map(|idx| {
                serde_json::json!({
                    "name": format!("vendor/package-{idx:02}"),
                    "version": "1.0.0"
                })
            })
            .collect();
        let json = serde_json::json!({ "installed": installed });

        let out = format_json_report("show", &json);

        assert_eq!(out.lines().count(), 31);
        assert!(out.contains("vendor/package-29: 1.0.0"));
        assert!(!out.contains("vendor/package-30: 1.0.0"));
        assert!(out.ends_with("... +1 more packages"));
    }

    #[test]
    fn formats_platform_reqs_with_status_legend() {
        let json = serde_json::json!([
            {"name": "php", "version": "8.5.0", "status": "success"},
            {
                "name": "ext-foo",
                "version": "n/a",
                "status": "failed",
                "failed_requirement": {
                    "source": "vendor/pkg",
                    "type": "requires",
                    "target": "ext-foo",
                    "constraint": "*"
                }
            }
        ]);
        let out = format_json_report("check-platform-reqs", &json);
        assert!(out.contains("S=success F=failed"));
        assert!(out.contains("S php 8.5.0"));
        assert!(out.contains("F ext-foo n/a (vendor/pkg requires ext-foo *)"));
    }

    #[test]
    fn formats_missing_platform_reqs_as_failures_with_requirement_details() {
        let json = serde_json::json!([
            {
                "name": "ext-xdebug",
                "version": "n/a",
                "status": "missing",
                "failed_requirement": {
                    "source": "vendor/debug-tools",
                    "type": "requires",
                    "target": "ext-xdebug",
                    "constraint": "^3.0"
                }
            }
        ]);

        let out = format_json_report("check-platform-reqs", &json);

        assert!(out.contains("F ext-xdebug n/a (missing)"));
        assert!(out.contains("vendor/debug-tools requires ext-xdebug ^3.0"));
        assert!(!out.contains("? ext-xdebug"));
    }

    #[test]
    fn formats_licenses_with_dependency_rows() {
        let json = serde_json::json!({
            "name": "root/app",
            "version": "1.0.0",
            "license": ["MIT"],
            "dependencies": {
                "vendor/b": {"version": "2.0.0", "license": ["BSD-3-Clause"]},
                "vendor/a": {"version": "1.0.0", "license": ["MIT", "Apache-2.0"]}
            }
        });
        let out = format_json_report("licenses", &json);
        assert!(out.contains("root/app 1.0.0 MIT"));
        assert!(out.contains("deps 2"));
        assert!(out.contains("vendor/a 1.0.0 MIT,Apache-2.0"));
        assert!(out.contains("vendor/b 2.0.0 BSD-3-Clause"));
        assert!(!out.contains("composer licenses:"));
    }

    #[test]
    fn formats_fund_groups_with_compact_urls() {
        let json = serde_json::json!({
            "vendor": {
                "https://github.com/sponsors/acme/": ["one", "two"],
                "https://www.example.com/fund": ["one"]
            }
        });
        let out = format_json_report("fund", &json);
        assert!(out.contains("vendor:"));
        assert!(out.contains("gh:acme (one,two)"));
        assert!(out.contains("example.com/fund (one)"));
        assert!(!out.contains("composer fund:"));
    }

    #[test]
    fn formats_empty_fund_array() {
        let json = serde_json::json!([]);
        let out = format_json_report("fund", &json);
        assert_eq!(out, "0 funding links");
    }
}
