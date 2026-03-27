//! Azure CLI output compression.
//!
//! Focused optimizations for Azure DevOps pipelines (`az pipelines ...`),
//! with safe generic handling for other `az` commands.

use crate::core::tracking;
use crate::core::utils::{truncate, truncate_iso_date};
use crate::json_cmd;
use anyhow::{Context, Result};
use serde_json::Value;
use std::process::{Command, ExitStatus};

const MAX_ITEMS: usize = 20;
const JSON_COMPRESS_DEPTH: usize = 4;

/// Run an Azure CLI command with token-optimized output.
pub fn run(subcommand: &str, args: &[String], verbose: u8) -> Result<()> {
    let full_sub = if args.is_empty() {
        subcommand.to_string()
    } else {
        format!("{} {}", subcommand, args.join(" "))
    };

    if subcommand == "account" {
        if !args.is_empty() && args[0] == "show" {
            return run_specialized(
                "az account show",
                "account",
                &["show"],
                &args[1..],
                verbose,
                filter_account_show,
            );
        }
        if !args.is_empty() && args[0] == "list" {
            return run_specialized(
                "az account list",
                "account",
                &["list"],
                &args[1..],
                verbose,
                filter_account_list,
            );
        }
    }

    if subcommand == "group" {
        if !args.is_empty() && args[0] == "list" {
            return run_specialized(
                "az group list",
                "group",
                &["list"],
                &args[1..],
                verbose,
                filter_group_list,
            );
        }
        if !args.is_empty() && args[0] == "show" {
            return run_specialized(
                "az group show",
                "group",
                &["show"],
                &args[1..],
                verbose,
                filter_group_show,
            );
        }
    }

    if subcommand == "resource" && !args.is_empty() && args[0] == "list" {
        return run_specialized(
            "az resource list",
            "resource",
            &["list"],
            &args[1..],
            verbose,
            filter_resource_list,
        );
    }

    if subcommand == "devops" && args.len() >= 2 && args[0] == "project" && args[1] == "list" {
        return run_specialized(
            "az devops project list",
            "devops",
            &["project", "list"],
            &args[2..],
            verbose,
            filter_devops_projects_list,
        );
    }

    if subcommand == "repos" {
        if !args.is_empty() && args[0] == "list" {
            return run_specialized(
                "az repos list",
                "repos",
                &["list"],
                &args[1..],
                verbose,
                filter_repos_list,
            );
        }
        if !args.is_empty() && args[0] == "show" {
            return run_specialized(
                "az repos show",
                "repos",
                &["show"],
                &args[1..],
                verbose,
                filter_repo_show,
            );
        }
        if args.len() >= 2 && args[0] == "pr" && args[1] == "list" {
            return run_specialized(
                "az repos pr list",
                "repos",
                &["pr", "list"],
                &args[2..],
                verbose,
                filter_repos_pr_list,
            );
        }
        if args.len() >= 2 && args[0] == "pr" && args[1] == "show" {
            return run_specialized(
                "az repos pr show",
                "repos",
                &["pr", "show"],
                &args[2..],
                verbose,
                filter_repos_pr_show,
            );
        }
    }

    if subcommand == "acr" && args.len() >= 2 && args[0] == "repository" && args[1] == "list" {
        return run_specialized(
            "az acr repository list",
            "acr",
            &["repository", "list"],
            &args[2..],
            verbose,
            filter_acr_repository_list,
        );
    }

    if subcommand == "acr" && args.len() >= 2 && args[0] == "repository" && args[1] == "show-tags" {
        return run_specialized(
            "az acr repository show-tags",
            "acr",
            &["repository", "show-tags"],
            &args[2..],
            verbose,
            filter_acr_repository_tags,
        );
    }

    if subcommand == "aks" {
        if !args.is_empty() && args[0] == "list" {
            return run_specialized(
                "az aks list",
                "aks",
                &["list"],
                &args[1..],
                verbose,
                filter_aks_list,
            );
        }
        if !args.is_empty() && args[0] == "show" {
            return run_specialized(
                "az aks show",
                "aks",
                &["show"],
                &args[1..],
                verbose,
                filter_aks_show,
            );
        }
    }

    if subcommand == "webapp" {
        if !args.is_empty() && args[0] == "list" {
            return run_specialized(
                "az webapp list",
                "webapp",
                &["list"],
                &args[1..],
                verbose,
                filter_webapp_list,
            );
        }
        if !args.is_empty() && args[0] == "show" {
            return run_specialized(
                "az webapp show",
                "webapp",
                &["show"],
                &args[1..],
                verbose,
                filter_webapp_show,
            );
        }
    }

    if subcommand == "monitor" {
        if args.len() >= 2 && args[0] == "metrics" && args[1] == "list" {
            return run_specialized(
                "az monitor metrics list",
                "monitor",
                &["metrics", "list"],
                &args[2..],
                verbose,
                filter_monitor_metrics_list,
            );
        }
        if args.len() >= 2 && args[0] == "activity-log" && args[1] == "list" {
            return run_specialized(
                "az monitor activity-log list",
                "monitor",
                &["activity-log", "list"],
                &args[2..],
                verbose,
                filter_monitor_activity_log_list,
            );
        }
        if args.len() >= 3
            && args[0] == "app-insights"
            && args[1] == "component"
            && args[2] == "list"
        {
            return run_specialized(
                "az monitor app-insights component list",
                "monitor",
                &["app-insights", "component", "list"],
                &args[3..],
                verbose,
                filter_app_insights_component_list,
            );
        }
        if args.len() >= 3
            && args[0] == "app-insights"
            && args[1] == "component"
            && args[2] == "show"
        {
            return run_specialized(
                "az monitor app-insights component show",
                "monitor",
                &["app-insights", "component", "show"],
                &args[3..],
                verbose,
                filter_app_insights_component_show,
            );
        }
        if args.len() >= 2 && args[0] == "app-insights" && args[1] == "query" {
            return run_specialized(
                "az monitor app-insights query",
                "monitor",
                &["app-insights", "query"],
                &args[2..],
                verbose,
                filter_app_insights_query,
            );
        }
    }

    if subcommand == "pipelines" {
        if !args.is_empty() && args[0] == "list" {
            return run_pipelines_list(&args[1..], verbose);
        }
        if !args.is_empty() && args[0] == "show" {
            return run_pipeline_show(&args[1..], verbose);
        }
        if args.len() >= 2 && args[0] == "runs" && args[1] == "list" {
            return run_pipeline_runs_list(&args[2..], verbose);
        }
        if args.len() >= 2 && args[0] == "runs" && args[1] == "show" {
            return run_pipeline_run_show(&args[2..], verbose);
        }
    }

    run_generic(subcommand, args, verbose, &full_sub)
}

fn run_generic(subcommand: &str, args: &[String], verbose: u8, full_sub: &str) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let (raw, stderr, status) = run_az(
        subcommand,
        args,
        verbose,
        is_structured_operation(args),
        false,
    )?;

    if !status.success() {
        timer.track(
            &format!("az {}", full_sub),
            &format!("rtk az {}", full_sub),
            &stderr,
            &stderr,
        );
        eprintln!("{}", stderr.trim());
        std::process::exit(status.code().unwrap_or(1));
    }

    let filtered = filter_generic_az_json(&raw).unwrap_or_else(|| fallback_json_or_raw(&raw));
    if !filtered.is_empty() {
        println!("{}", filtered);
    }

    timer.track(
        &format!("az {}", full_sub),
        &format!("rtk az {}", full_sub),
        &raw,
        &filtered,
    );
    Ok(())
}

fn run_specialized(
    display_cmd: &str,
    subcommand: &str,
    base_args: &[&str],
    extra_args: &[String],
    verbose: u8,
    filter: fn(&str) -> Option<String>,
) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let (raw, stderr, status) =
        run_az_service_command(subcommand, base_args, extra_args, verbose, true, true)?;

    if !status.success() {
        timer.track(
            display_cmd,
            &format!("rtk {}", display_cmd),
            &stderr,
            &stderr,
        );
        eprintln!("{}", stderr.trim());
        std::process::exit(status.code().unwrap_or(1));
    }

    let filtered = filter(&raw)
        .or_else(|| filter_generic_az_json(&raw))
        .unwrap_or_else(|| fallback_json_or_raw(&raw));
    if !filtered.is_empty() {
        println!("{}", filtered);
    }

    timer.track(
        display_cmd,
        &format!("rtk {}", display_cmd),
        &raw,
        &filtered,
    );
    Ok(())
}

fn run_pipelines_list(extra_args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let (raw, stderr, status) = run_az_pipeline_command(&["list"], extra_args, verbose)?;

    if !status.success() {
        timer.track(
            "az pipelines list",
            "rtk az pipelines list",
            &stderr,
            &stderr,
        );
        eprintln!("{}", stderr.trim());
        std::process::exit(status.code().unwrap_or(1));
    }

    let filtered = filter_pipelines_list(&raw).unwrap_or_else(|| fallback_json_or_raw(&raw));
    if !filtered.is_empty() {
        println!("{}", filtered);
    }

    timer.track(
        "az pipelines list",
        "rtk az pipelines list",
        &raw,
        &filtered,
    );
    Ok(())
}

fn run_pipeline_show(extra_args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let (raw, stderr, status) = run_az_pipeline_command(&["show"], extra_args, verbose)?;

    if !status.success() {
        timer.track(
            "az pipelines show",
            "rtk az pipelines show",
            &stderr,
            &stderr,
        );
        eprintln!("{}", stderr.trim());
        std::process::exit(status.code().unwrap_or(1));
    }

    let filtered = filter_pipeline_show(&raw).unwrap_or_else(|| fallback_json_or_raw(&raw));
    if !filtered.is_empty() {
        println!("{}", filtered);
    }

    timer.track(
        "az pipelines show",
        "rtk az pipelines show",
        &raw,
        &filtered,
    );
    Ok(())
}

fn run_pipeline_runs_list(extra_args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let (raw, stderr, status) = run_az_pipeline_command(&["runs", "list"], extra_args, verbose)?;

    if !status.success() {
        timer.track(
            "az pipelines runs list",
            "rtk az pipelines runs list",
            &stderr,
            &stderr,
        );
        eprintln!("{}", stderr.trim());
        std::process::exit(status.code().unwrap_or(1));
    }

    let filtered = filter_pipeline_runs_list(&raw).unwrap_or_else(|| fallback_json_or_raw(&raw));
    if !filtered.is_empty() {
        println!("{}", filtered);
    }

    timer.track(
        "az pipelines runs list",
        "rtk az pipelines runs list",
        &raw,
        &filtered,
    );
    Ok(())
}

fn run_pipeline_run_show(extra_args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let (raw, stderr, status) = run_az_pipeline_command(&["runs", "show"], extra_args, verbose)?;

    if !status.success() {
        timer.track(
            "az pipelines runs show",
            "rtk az pipelines runs show",
            &stderr,
            &stderr,
        );
        eprintln!("{}", stderr.trim());
        std::process::exit(status.code().unwrap_or(1));
    }

    let filtered = filter_pipeline_run_show(&raw).unwrap_or_else(|| fallback_json_or_raw(&raw));
    if !filtered.is_empty() {
        println!("{}", filtered);
    }

    timer.track(
        "az pipelines runs show",
        "rtk az pipelines runs show",
        &raw,
        &filtered,
    );
    Ok(())
}

fn run_az_pipeline_command(
    base_args: &[&str],
    extra_args: &[String],
    verbose: u8,
) -> Result<(String, String, ExitStatus)> {
    run_az_service_command("pipelines", base_args, extra_args, verbose, true, true)
}

fn run_az_service_command(
    subcommand: &str,
    base_args: &[&str],
    extra_args: &[String],
    verbose: u8,
    force_json: bool,
    replace_output: bool,
) -> Result<(String, String, ExitStatus)> {
    let mut args: Vec<String> = base_args.iter().map(|a| a.to_string()).collect();
    args.extend(extra_args.iter().cloned());
    run_az(subcommand, &args, verbose, force_json, replace_output)
}

fn run_az(
    subcommand: &str,
    args: &[String],
    verbose: u8,
    force_json: bool,
    replace_output: bool,
) -> Result<(String, String, ExitStatus)> {
    let mut cmd = Command::new("az");
    cmd.arg(subcommand);

    let mut has_output_flag = false;
    if replace_output {
        let mut skip_next = false;
        for arg in args {
            if skip_next {
                skip_next = false;
                continue;
            }
            if arg == "--output" || arg == "-o" {
                has_output_flag = true;
                skip_next = true;
                continue;
            }
            if arg.starts_with("--output=") || arg.starts_with("-o=") {
                has_output_flag = true;
                continue;
            }
            cmd.arg(arg);
        }
    } else {
        for arg in args {
            if arg == "--output"
                || arg == "-o"
                || arg.starts_with("--output=")
                || arg.starts_with("-o=")
            {
                has_output_flag = true;
            }
            cmd.arg(arg);
        }
    }

    if force_json && (replace_output || !has_output_flag) {
        cmd.args(["--output", "json"]);
    }

    if verbose > 0 {
        eprintln!("Running: az {} {}", subcommand, args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run az CLI (is Azure CLI installed?)")?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok((stdout, stderr, output.status))
}

fn fallback_json_or_raw(raw: &str) -> String {
    if raw.trim().is_empty() {
        return String::new();
    }
    match json_cmd::filter_json_string(raw, JSON_COMPRESS_DEPTH) {
        Ok(schema) => schema,
        Err(_) => raw.to_string(),
    }
}

/// Returns true for operations that are usually read/list commands and safe to coerce to JSON.
fn is_structured_operation(args: &[String]) -> bool {
    if args.is_empty() {
        return false;
    }

    let first = args[0].as_str();
    if matches!(first, "list" | "show" | "get") {
        return true;
    }

    if args.len() >= 2 && matches!(args[1].as_str(), "list" | "show" | "get" | "query") {
        return true;
    }

    args.len() >= 2
        && matches!(first, "runs" | "builds")
        && matches!(args[1].as_str(), "list" | "show" | "get")
}

fn filter_generic_az_json(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;

    if let Some(items) = extract_items(&value) {
        return Some(format_generic_items("AZ items", "items", items));
    }

    if value.is_object() {
        return Some(format_generic_object("AZ object", &value));
    }

    None
}

fn filter_account_show(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let name = get_string_field(&value, &["name"]).unwrap_or_else(|| "-".to_string());
    let id = get_string_field(&value, &["id"]).unwrap_or_else(|| "-".to_string());
    let tenant = get_string_field(&value, &["tenantId"]).unwrap_or_else(|| "-".to_string());
    let env = get_string_field(&value, &["environmentName"]).unwrap_or_else(|| "-".to_string());
    let user = get_string_field(&value, &["/user/name", "/user/userName"]).unwrap_or_default();

    let mut lines = Vec::new();
    lines.push(format!(
        "AZ account: {} id={} tenant={} env={}",
        truncate(&name, 36),
        short_id(&id),
        short_id(&tenant),
        env
    ));
    if !user.is_empty() {
        lines.push(format!("user: {}", user));
    }
    Some(lines.join("\n"))
}

fn filter_account_list(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let accounts = extract_items(&value)?;
    if accounts.is_empty() {
        return Some("AZ accounts: 0".to_string());
    }

    let mut lines = Vec::new();
    lines.push(format!("AZ accounts: {}", accounts.len()));
    for account in accounts.iter().take(MAX_ITEMS) {
        let name = get_string_field(account, &["name"]).unwrap_or_else(|| "-".to_string());
        let id = get_string_field(account, &["id"]).unwrap_or_else(|| "-".to_string());
        let tenant = get_string_field(account, &["tenantId"]).unwrap_or_else(|| "-".to_string());
        let default = account
            .get("isDefault")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let marker = if default { " default" } else { "" };
        lines.push(format!(
            "{} id={} tenant={}{}",
            truncate(&name, 30),
            short_id(&id),
            short_id(&tenant),
            marker
        ));
    }
    if accounts.len() > MAX_ITEMS {
        lines.push(format!("... +{} more accounts", accounts.len() - MAX_ITEMS));
    }
    Some(lines.join("\n"))
}

fn filter_group_list(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let groups = extract_items(&value)?;
    if groups.is_empty() {
        return Some("AZ resource groups: 0".to_string());
    }

    let mut lines = Vec::new();
    lines.push(format!("AZ resource groups: {}", groups.len()));
    for group in groups.iter().take(MAX_ITEMS) {
        let name = get_string_field(group, &["name"]).unwrap_or_else(|| "-".to_string());
        let location = get_string_field(group, &["location"]).unwrap_or_else(|| "-".to_string());
        let state = get_string_field(
            group,
            &[
                "properties.provisioningState",
                "/properties/provisioningState",
            ],
        )
        .unwrap_or_else(|| "-".to_string());
        lines.push(format!("{} [{}] {}", truncate(&name, 36), state, location));
    }
    if groups.len() > MAX_ITEMS {
        lines.push(format!("... +{} more groups", groups.len() - MAX_ITEMS));
    }
    Some(lines.join("\n"))
}

fn filter_group_show(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let name = get_string_field(&value, &["name"]).unwrap_or_else(|| "-".to_string());
    let location = get_string_field(&value, &["location"]).unwrap_or_else(|| "-".to_string());
    let state = get_string_field(&value, &["/properties/provisioningState"])
        .unwrap_or_else(|| "-".to_string());
    Some(format!(
        "AZ group: {} [{}] {}",
        truncate(&name, 40),
        state,
        location
    ))
}

fn filter_resource_list(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let resources = extract_items(&value)?;
    if resources.is_empty() {
        return Some("AZ resources: 0".to_string());
    }

    let mut lines = Vec::new();
    lines.push(format!("AZ resources: {}", resources.len()));
    for res in resources.iter().take(MAX_ITEMS) {
        let name = get_string_field(res, &["name"]).unwrap_or_else(|| "-".to_string());
        let ty = get_string_field(res, &["type"]).unwrap_or_else(|| "-".to_string());
        let rg = get_string_field(res, &["resourceGroup"]).unwrap_or_else(|| "-".to_string());
        lines.push(format!(
            "{} type={} rg={}",
            truncate(&name, 28),
            truncate(&ty, 36),
            truncate(&rg, 24)
        ));
    }
    if resources.len() > MAX_ITEMS {
        lines.push(format!(
            "... +{} more resources",
            resources.len() - MAX_ITEMS
        ));
    }
    Some(lines.join("\n"))
}

fn filter_devops_projects_list(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let projects = extract_items(&value)?;
    if projects.is_empty() {
        return Some("AZ DevOps projects: 0".to_string());
    }

    let mut lines = Vec::new();
    lines.push(format!("AZ DevOps projects: {}", projects.len()));
    for p in projects.iter().take(MAX_ITEMS) {
        let name = get_string_field(p, &["name"]).unwrap_or_else(|| "-".to_string());
        let id = get_string_field(p, &["id"]).unwrap_or_else(|| "-".to_string());
        let state =
            get_string_field(p, &["state", "/visibility"]).unwrap_or_else(|| "-".to_string());
        lines.push(format!(
            "{} [{}] {}",
            truncate(&name, 34),
            state,
            short_id(&id)
        ));
    }
    if projects.len() > MAX_ITEMS {
        lines.push(format!("... +{} more projects", projects.len() - MAX_ITEMS));
    }
    Some(lines.join("\n"))
}

fn filter_repos_list(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let repos = extract_items(&value)?;
    if repos.is_empty() {
        return Some("AZ repos: 0".to_string());
    }

    let mut lines = Vec::new();
    lines.push(format!("AZ repos: {}", repos.len()));
    for repo in repos.iter().take(MAX_ITEMS) {
        let name = get_string_field(repo, &["name"]).unwrap_or_else(|| "-".to_string());
        let id = get_string_field(repo, &["id"]).unwrap_or_else(|| "-".to_string());
        let branch = get_string_field(repo, &["defaultBranch"]).unwrap_or_default();
        let project = get_string_field(repo, &["/project/name"]).unwrap_or_default();
        let mut line = format!("{} {}", truncate(&name, 34), short_id(&id));
        if !branch.is_empty() {
            line.push_str(&format!(" {}", strip_branch_prefix(&branch)));
        }
        if !project.is_empty() {
            line.push_str(&format!(" project={}", truncate(&project, 18)));
        }
        lines.push(line);
    }
    if repos.len() > MAX_ITEMS {
        lines.push(format!("... +{} more repos", repos.len() - MAX_ITEMS));
    }
    Some(lines.join("\n"))
}

fn filter_repo_show(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let name = get_string_field(&value, &["name"]).unwrap_or_else(|| "-".to_string());
    let id = get_string_field(&value, &["id"]).unwrap_or_else(|| "-".to_string());
    let branch = get_string_field(&value, &["defaultBranch"]).unwrap_or_default();
    let remote_url = get_string_field(&value, &["remoteUrl"]).unwrap_or_default();
    let mut lines = Vec::new();
    lines.push(format!(
        "AZ repo: {} {}",
        truncate(&name, 42),
        short_id(&id)
    ));
    if !branch.is_empty() {
        lines.push(format!("defaultBranch: {}", strip_branch_prefix(&branch)));
    }
    if !remote_url.is_empty() {
        lines.push(format!("remoteUrl: {}", remote_url));
    }
    Some(lines.join("\n"))
}

fn filter_repos_pr_list(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let prs = extract_items(&value)?;
    if prs.is_empty() {
        return Some("AZ pull requests: 0".to_string());
    }

    let mut lines = Vec::new();
    lines.push(format!("AZ pull requests: {}", prs.len()));
    for pr in prs.iter().take(MAX_ITEMS) {
        let id = get_string_field(pr, &["pullRequestId", "id"]).unwrap_or_else(|| "-".to_string());
        let status = get_string_field(pr, &["status"]).unwrap_or_else(|| "-".to_string());
        let created = get_string_field(pr, &["creationDate", "createdDate"])
            .unwrap_or_else(|| "-".to_string());
        let source = get_string_field(pr, &["sourceRefName"]).unwrap_or_default();
        let target = get_string_field(pr, &["targetRefName"]).unwrap_or_default();
        let title = get_string_field(pr, &["title"]).unwrap_or_else(|| "-".to_string());
        let source = strip_branch_prefix(&source);
        let target = strip_branch_prefix(&target);
        let refs = if !source.is_empty() && !target.is_empty() {
            format!(" {}->{}", source, target)
        } else {
            String::new()
        };
        lines.push(format!(
            "{} [{}] {}{} {}",
            id,
            status,
            truncate_iso_date(&created),
            refs,
            truncate(&title, 44)
        ));
    }
    if prs.len() > MAX_ITEMS {
        lines.push(format!("... +{} more pull requests", prs.len() - MAX_ITEMS));
    }
    Some(lines.join("\n"))
}

fn filter_repos_pr_show(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let id = get_string_field(&value, &["pullRequestId", "id"]).unwrap_or_else(|| "-".to_string());
    let status = get_string_field(&value, &["status"]).unwrap_or_else(|| "-".to_string());
    let title = get_string_field(&value, &["title"]).unwrap_or_else(|| "-".to_string());
    let created = get_string_field(&value, &["creationDate", "createdDate"])
        .unwrap_or_else(|| "-".to_string());
    let source = get_string_field(&value, &["sourceRefName"]).unwrap_or_default();
    let target = get_string_field(&value, &["targetRefName"]).unwrap_or_default();
    let mut lines = Vec::new();
    lines.push(format!(
        "AZ pull request: {} [{}] {}",
        id,
        status,
        truncate(&title, 56)
    ));
    lines.push(format!("created: {}", truncate_iso_date(&created)));
    if !source.is_empty() || !target.is_empty() {
        lines.push(format!(
            "branches: {} -> {}",
            strip_branch_prefix(&source),
            strip_branch_prefix(&target)
        ));
    }
    Some(lines.join("\n"))
}

fn filter_acr_repository_list(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let repos = extract_items(&value)?;
    Some(format_string_items(
        "AZ ACR repositories",
        "repositories",
        repos,
    ))
}

fn filter_acr_repository_tags(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let tags = extract_items(&value)?;
    Some(format_string_items("AZ ACR tags", "tags", tags))
}

fn filter_aks_list(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let clusters = extract_items(&value)?;
    if clusters.is_empty() {
        return Some("AZ AKS clusters: 0".to_string());
    }

    let mut lines = Vec::new();
    lines.push(format!("AZ AKS clusters: {}", clusters.len()));
    for c in clusters.iter().take(MAX_ITEMS) {
        let name = get_string_field(c, &["name"]).unwrap_or_else(|| "-".to_string());
        let rg = get_string_field(c, &["resourceGroup"]).unwrap_or_else(|| "-".to_string());
        let version =
            get_string_field(c, &["kubernetesVersion"]).unwrap_or_else(|| "-".to_string());
        let power = get_string_field(c, &["/powerState/code", "provisioningState"])
            .unwrap_or_else(|| "-".to_string());
        lines.push(format!(
            "{} [{}] k8s={} rg={}",
            truncate(&name, 28),
            power,
            version,
            truncate(&rg, 22)
        ));
    }
    if clusters.len() > MAX_ITEMS {
        lines.push(format!("... +{} more clusters", clusters.len() - MAX_ITEMS));
    }
    Some(lines.join("\n"))
}

fn filter_aks_show(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let name = get_string_field(&value, &["name"]).unwrap_or_else(|| "-".to_string());
    let rg = get_string_field(&value, &["resourceGroup"]).unwrap_or_else(|| "-".to_string());
    let version =
        get_string_field(&value, &["kubernetesVersion"]).unwrap_or_else(|| "-".to_string());
    let power = get_string_field(&value, &["/powerState/code", "provisioningState"])
        .unwrap_or_else(|| "-".to_string());
    let location = get_string_field(&value, &["location"]).unwrap_or_else(|| "-".to_string());
    Some(format!(
        "AZ AKS: {} [{}] k8s={} rg={} {}",
        truncate(&name, 34),
        power,
        version,
        truncate(&rg, 24),
        location
    ))
}

fn filter_webapp_list(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let apps = extract_items(&value)?;
    if apps.is_empty() {
        return Some("AZ webapps: 0".to_string());
    }

    let mut lines = Vec::new();
    lines.push(format!("AZ webapps: {}", apps.len()));
    for app in apps.iter().take(MAX_ITEMS) {
        let name = get_string_field(app, &["name"]).unwrap_or_else(|| "-".to_string());
        let state = get_string_field(app, &["state"]).unwrap_or_else(|| "-".to_string());
        let host = get_string_field(app, &["defaultHostName"]).unwrap_or_default();
        let rg = get_string_field(app, &["resourceGroup"]).unwrap_or_else(|| "-".to_string());
        let host_part = if host.is_empty() {
            String::new()
        } else {
            format!(" host={}", truncate(&host, 36))
        };
        lines.push(format!(
            "{} [{}] rg={}{}",
            truncate(&name, 28),
            state,
            truncate(&rg, 22),
            host_part
        ));
    }
    if apps.len() > MAX_ITEMS {
        lines.push(format!("... +{} more webapps", apps.len() - MAX_ITEMS));
    }
    Some(lines.join("\n"))
}

fn filter_webapp_show(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let name = get_string_field(&value, &["name"]).unwrap_or_else(|| "-".to_string());
    let state = get_string_field(&value, &["state"]).unwrap_or_else(|| "-".to_string());
    let host = get_string_field(&value, &["defaultHostName"]).unwrap_or_default();
    let location = get_string_field(&value, &["location"]).unwrap_or_else(|| "-".to_string());
    let mut lines = Vec::new();
    lines.push(format!(
        "AZ webapp: {} [{}] {}",
        truncate(&name, 36),
        state,
        location
    ));
    if !host.is_empty() {
        lines.push(format!("host: {}", host));
    }
    Some(lines.join("\n"))
}

fn filter_monitor_metrics_list(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let metrics = value
        .get("value")
        .and_then(Value::as_array)
        .or_else(|| value.as_array())?;
    if metrics.is_empty() {
        return Some("AZ monitor metrics: 0".to_string());
    }

    let mut lines = Vec::new();
    lines.push(format!("AZ monitor metrics: {}", metrics.len()));
    for metric in metrics.iter().take(MAX_ITEMS) {
        let name = get_string_field(metric, &["/name/localizedValue", "/name/value", "name"])
            .unwrap_or_else(|| "-".to_string());
        let mut points = 0usize;
        let mut non_null_points = 0usize;
        if let Some(series) = metric.get("timeseries").and_then(Value::as_array) {
            for ts in series {
                if let Some(data) = ts.get("data").and_then(Value::as_array) {
                    points += data.len();
                    for item in data {
                        if item.get("average").is_some()
                            || item.get("total").is_some()
                            || item.get("minimum").is_some()
                            || item.get("maximum").is_some()
                            || item.get("count").is_some()
                        {
                            non_null_points += 1;
                        }
                    }
                }
            }
        }
        lines.push(format!(
            "{} points={} nonEmpty={}",
            truncate(&name, 42),
            points,
            non_null_points
        ));
    }
    if metrics.len() > MAX_ITEMS {
        lines.push(format!("... +{} more metrics", metrics.len() - MAX_ITEMS));
    }
    Some(lines.join("\n"))
}

fn filter_monitor_activity_log_list(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let events = extract_items(&value)?;
    if events.is_empty() {
        return Some("AZ activity log events: 0".to_string());
    }

    let mut lines = Vec::new();
    lines.push(format!("AZ activity log events: {}", events.len()));
    for ev in events.iter().take(MAX_ITEMS) {
        let when = get_string_field(ev, &["eventTimestamp", "submissionTimestamp"])
            .unwrap_or_else(|| "-".to_string());
        let op = get_string_field(
            ev,
            &[
                "/operationName/localizedValue",
                "/operationName/value",
                "operationName",
            ],
        )
        .unwrap_or_else(|| "-".to_string());
        let status = get_string_field(ev, &["/status/localizedValue", "/status/value", "status"])
            .unwrap_or_else(|| "-".to_string());
        let rg = get_string_field(ev, &["resourceGroupName", "resourceGroup"]).unwrap_or_default();
        let mut line = format!(
            "{} {} [{}]",
            truncate_iso_date(&when),
            truncate(&op, 40),
            status
        );
        if !rg.is_empty() {
            line.push_str(&format!(" rg={}", truncate(&rg, 24)));
        }
        lines.push(line);
    }
    if events.len() > MAX_ITEMS {
        lines.push(format!("... +{} more events", events.len() - MAX_ITEMS));
    }
    Some(lines.join("\n"))
}

fn filter_app_insights_component_list(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let components = extract_items(&value)?;
    if components.is_empty() {
        return Some("AZ App Insights components: 0".to_string());
    }

    let mut lines = Vec::new();
    lines.push(format!("AZ App Insights components: {}", components.len()));
    for comp in components.iter().take(MAX_ITEMS) {
        let name = get_string_field(comp, &["name"]).unwrap_or_else(|| "-".to_string());
        let app_id = get_string_field(comp, &["appId"]).unwrap_or_else(|| "-".to_string());
        let location = get_string_field(comp, &["location"]).unwrap_or_else(|| "-".to_string());
        lines.push(format!(
            "{} appId={} {}",
            truncate(&name, 28),
            short_id(&app_id),
            location
        ));
    }
    if components.len() > MAX_ITEMS {
        lines.push(format!(
            "... +{} more components",
            components.len() - MAX_ITEMS
        ));
    }
    Some(lines.join("\n"))
}

fn filter_app_insights_component_show(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let name = get_string_field(&value, &["name"]).unwrap_or_else(|| "-".to_string());
    let app_id = get_string_field(&value, &["appId"]).unwrap_or_else(|| "-".to_string());
    let location = get_string_field(&value, &["location"]).unwrap_or_else(|| "-".to_string());
    let kind = get_string_field(&value, &["kind"]).unwrap_or_else(|| "-".to_string());
    Some(format!(
        "AZ App Insights: {} appId={} kind={} {}",
        truncate(&name, 30),
        short_id(&app_id),
        kind,
        location
    ))
}

fn filter_app_insights_query(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let tables = value.get("tables").and_then(Value::as_array)?;
    if tables.is_empty() {
        return Some("AZ App Insights query: 0 tables".to_string());
    }

    let mut lines = Vec::new();
    lines.push(format!("AZ App Insights query tables: {}", tables.len()));
    for table in tables.iter().take(MAX_ITEMS) {
        let name = get_string_field(table, &["name"]).unwrap_or_else(|| "-".to_string());
        let columns = table
            .get("columns")
            .and_then(Value::as_array)
            .map(|cols| cols.len())
            .unwrap_or(0);
        let rows = table
            .get("rows")
            .and_then(Value::as_array)
            .map(|r| r.len())
            .unwrap_or(0);
        lines.push(format!(
            "{} rows={} cols={}",
            truncate(&name, 32),
            rows,
            columns
        ));
    }
    if tables.len() > MAX_ITEMS {
        lines.push(format!("... +{} more tables", tables.len() - MAX_ITEMS));
    }
    Some(lines.join("\n"))
}

fn extract_items(value: &Value) -> Option<&Vec<Value>> {
    if let Some(items) = value.as_array() {
        Some(items)
    } else {
        value.get("value").and_then(Value::as_array)
    }
}

fn format_generic_items(header: &str, label: &str, items: &[Value]) -> String {
    if items.is_empty() {
        return format!("{}: 0", header);
    }

    let mut lines = Vec::new();
    lines.push(format!("{}: {}", header, items.len()));

    let scalar_items = items
        .iter()
        .all(|v| v.is_string() || v.is_number() || v.is_boolean());
    if scalar_items {
        for item in items.iter().take(MAX_ITEMS) {
            lines.push(truncate(&value_as_compact_string(item), 80));
        }
    } else {
        for item in items.iter().take(MAX_ITEMS) {
            lines.push(format_generic_object_line(item));
        }
    }

    if items.len() > MAX_ITEMS {
        lines.push(format!("... +{} more {}", items.len() - MAX_ITEMS, label));
    }
    lines.join("\n")
}

fn format_string_items(header: &str, label: &str, items: &[Value]) -> String {
    let mut lines = Vec::new();
    lines.push(format!("{}: {}", header, items.len()));
    for item in items.iter().take(MAX_ITEMS) {
        lines.push(truncate(&value_as_compact_string(item), 80));
    }
    if items.len() > MAX_ITEMS {
        lines.push(format!("... +{} more {}", items.len() - MAX_ITEMS, label));
    }
    lines.join("\n")
}

fn format_generic_object(header: &str, value: &Value) -> String {
    let mut parts = Vec::new();
    for key in [
        "name",
        "id",
        "status",
        "state",
        "provisioningState",
        "location",
        "resourceGroup",
        "type",
    ] {
        if let Some(v) = get_string_field(value, &[key]) {
            let display = if key == "id" {
                short_id(&v)
            } else {
                truncate(&v, 40)
            };
            parts.push(format!("{}={}", key, display));
        }
    }

    if parts.is_empty() {
        if let Some(obj) = value.as_object() {
            for (k, v) in obj.iter().take(6) {
                if v.is_string() || v.is_number() || v.is_boolean() {
                    parts.push(format!(
                        "{}={}",
                        k,
                        truncate(&value_as_compact_string(v), 40)
                    ));
                }
            }
        }
    }

    if parts.is_empty() {
        header.to_string()
    } else {
        format!("{}: {}", header, parts.join(" "))
    }
}

fn format_generic_object_line(value: &Value) -> String {
    let id = get_string_field(value, &["id"]).unwrap_or_default();
    let name = get_string_field(value, &["name", "displayName"]).unwrap_or_default();
    let state = get_string_field(
        value,
        &[
            "status",
            "state",
            "provisioningState",
            "queueStatus",
            "result",
        ],
    )
    .unwrap_or_default();
    let location = get_string_field(value, &["location"]).unwrap_or_default();
    let rg = get_string_field(value, &["resourceGroup"]).unwrap_or_default();
    let ty = get_string_field(value, &["type"]).unwrap_or_default();

    let mut parts = Vec::new();
    if !name.is_empty() {
        parts.push(truncate(&name, 34));
    }
    if !id.is_empty() {
        parts.push(short_id(&id));
    }
    if !state.is_empty() {
        parts.push(format!("[{}]", state));
    }
    if !location.is_empty() {
        parts.push(location);
    }
    if !rg.is_empty() {
        parts.push(format!("rg={}", truncate(&rg, 20)));
    }
    if !ty.is_empty() {
        parts.push(format!("type={}", truncate(&ty, 26)));
    }

    if parts.is_empty() {
        format_generic_object("item", value)
    } else {
        parts.join(" ")
    }
}

fn get_string_field(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        let candidate = if key.starts_with('/') {
            value.pointer(key)
        } else if key.contains('.') {
            let pointer = format!("/{}", key.replace('.', "/"));
            value.pointer(&pointer)
        } else {
            value.get(*key)
        };

        if let Some(v) = candidate {
            if let Some(s) = v.as_str() {
                return Some(s.to_string());
            }
            if v.is_number() || v.is_boolean() {
                return Some(value_as_compact_string(v));
            }
        }
    }
    None
}

fn short_id(value: &str) -> String {
    if value.len() > 18 {
        format!("{}...", &value[..15])
    } else {
        value.to_string()
    }
}

fn filter_pipelines_list(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let pipelines = value.as_array()?;

    if pipelines.is_empty() {
        return Some("AZ pipelines: 0".to_string());
    }

    let mut lines = Vec::new();
    lines.push(format!("AZ pipelines: {}", pipelines.len()));

    for item in pipelines.iter().take(MAX_ITEMS) {
        let id = extract_id(item);
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .map(|s| truncate(s, 40))
            .unwrap_or_else(|| "-".to_string());
        let status = item
            .get("queueStatus")
            .and_then(Value::as_str)
            .or_else(|| item.get("status").and_then(Value::as_str))
            .or_else(|| item.get("state").and_then(Value::as_str))
            .unwrap_or("-");
        let folder = item
            .get("folder")
            .and_then(Value::as_str)
            .or_else(|| item.get("path").and_then(Value::as_str))
            .unwrap_or("");

        if folder.is_empty() {
            lines.push(format!("{} {} [{}]", id, name, status));
        } else {
            lines.push(format!("{} {} [{}] {}", id, name, status, folder));
        }
    }

    if pipelines.len() > MAX_ITEMS {
        lines.push(format!(
            "... +{} more pipelines",
            pipelines.len() - MAX_ITEMS
        ));
    }

    Some(lines.join("\n"))
}

fn filter_pipeline_show(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;

    let id = extract_id(&value);
    let name = value.get("name").and_then(Value::as_str).unwrap_or("-");
    let status = value
        .get("queueStatus")
        .and_then(Value::as_str)
        .or_else(|| value.get("status").and_then(Value::as_str))
        .or_else(|| value.get("state").and_then(Value::as_str))
        .unwrap_or("-");
    let folder = value
        .get("folder")
        .and_then(Value::as_str)
        .or_else(|| value.get("path").and_then(Value::as_str))
        .unwrap_or("");
    let revision = value
        .get("revision")
        .map(value_as_compact_string)
        .unwrap_or_else(|| "-".to_string());
    let url = value
        .get("url")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/_links/web/href").and_then(Value::as_str))
        .unwrap_or("");

    let mut lines = Vec::new();
    lines.push(format!(
        "AZ pipeline: {} {} [{}] rev={}",
        id,
        truncate(name, 50),
        status,
        revision
    ));

    if !folder.is_empty() {
        lines.push(format!("folder: {}", folder));
    }
    if !url.is_empty() {
        lines.push(format!("url: {}", url));
    }

    Some(lines.join("\n"))
}

fn filter_pipeline_runs_list(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let runs = value.as_array()?;

    if runs.is_empty() {
        return Some("AZ pipeline runs: 0".to_string());
    }

    let mut lines = Vec::new();
    lines.push(format!("AZ pipeline runs: {}", runs.len()));

    for run in runs.iter().take(MAX_ITEMS) {
        let id = extract_id(run);
        let pipeline_name = run
            .pointer("/pipeline/name")
            .and_then(Value::as_str)
            .or_else(|| run.pointer("/definition/name").and_then(Value::as_str))
            .or_else(|| run.get("pipelineName").and_then(Value::as_str))
            .unwrap_or("-");
        let status = run
            .get("status")
            .and_then(Value::as_str)
            .or_else(|| run.get("state").and_then(Value::as_str))
            .unwrap_or("-");
        let result = run.get("result").and_then(Value::as_str).unwrap_or("-");
        let status_compact = if result != "-" {
            format!("{}/{}", status, result)
        } else {
            status.to_string()
        };
        let created = run
            .get("createdDate")
            .and_then(Value::as_str)
            .or_else(|| run.get("queueTime").and_then(Value::as_str))
            .or_else(|| run.get("createdOn").and_then(Value::as_str))
            .unwrap_or("-");
        let branch = run
            .get("sourceBranch")
            .and_then(Value::as_str)
            .unwrap_or("");
        let branch = strip_branch_prefix(branch);

        let mut line = format!(
            "{} {} {} {}",
            id,
            truncate(pipeline_name, 28),
            status_compact,
            truncate_iso_date(created)
        );
        if !branch.is_empty() {
            line.push_str(&format!(" {}", branch));
        }
        lines.push(line);
    }

    if runs.len() > MAX_ITEMS {
        lines.push(format!("... +{} more runs", runs.len() - MAX_ITEMS));
    }

    Some(lines.join("\n"))
}

fn filter_pipeline_run_show(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;

    let id = extract_id(&value);
    let pipeline_name = value
        .pointer("/pipeline/name")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/definition/name").and_then(Value::as_str))
        .or_else(|| value.get("pipelineName").and_then(Value::as_str))
        .unwrap_or("-");
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .or_else(|| value.get("state").and_then(Value::as_str))
        .unwrap_or("-");
    let result = value
        .get("result")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let created = value
        .get("createdDate")
        .and_then(Value::as_str)
        .or_else(|| value.get("queueTime").and_then(Value::as_str))
        .or_else(|| value.get("createdOn").and_then(Value::as_str))
        .unwrap_or("-");
    let branch = value
        .get("sourceBranch")
        .and_then(Value::as_str)
        .map(strip_branch_prefix)
        .unwrap_or("");
    let reason = value.get("reason").and_then(Value::as_str).unwrap_or("");
    let url = value
        .get("url")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/_links/web/href").and_then(Value::as_str))
        .unwrap_or("");

    let mut lines = Vec::new();
    lines.push(format!(
        "AZ pipeline run: {} {} {}/{} {}",
        id,
        truncate(pipeline_name, 45),
        status,
        result,
        truncate_iso_date(created)
    ));
    if !branch.is_empty() {
        lines.push(format!("branch: {}", branch));
    }
    if !reason.is_empty() {
        lines.push(format!("reason: {}", reason));
    }
    if !url.is_empty() {
        lines.push(format!("url: {}", url));
    }

    Some(lines.join("\n"))
}

fn extract_id(value: &Value) -> String {
    value
        .get("id")
        .map(value_as_compact_string)
        .unwrap_or_else(|| "-".to_string())
}

fn value_as_compact_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => "-".to_string(),
    }
}

fn strip_branch_prefix(branch: &str) -> &str {
    branch.strip_prefix("refs/heads/").unwrap_or(branch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_structured_operation() {
        assert!(is_structured_operation(&["list".to_string()]));
        assert!(is_structured_operation(&[
            "runs".to_string(),
            "show".to_string()
        ]));
        assert!(is_structured_operation(&[
            "metrics".to_string(),
            "list".to_string()
        ]));
        assert!(is_structured_operation(&[
            "app-insights".to_string(),
            "query".to_string()
        ]));
        assert!(!is_structured_operation(&["run".to_string()]));
        assert!(!is_structured_operation(&[]));
    }

    #[test]
    fn test_filter_account_show() {
        let input = r#"
{
  "name": "Engineering Subscription",
  "id": "11111111-2222-3333-4444-555555555555",
  "tenantId": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
  "environmentName": "AzureCloud",
  "user": {"name": "dev@contoso.com"}
}
"#;

        let out = filter_account_show(input).unwrap();
        assert!(out.contains("AZ account: Engineering Subscription"));
        assert!(out.contains("env=AzureCloud"));
        assert!(out.contains("user: dev@contoso.com"));
    }

    #[test]
    fn test_filter_pipelines_list() {
        let input = r#"
[
  {"id": 101, "name": "build-ci", "queueStatus": "enabled", "folder": "\\Project"},
  {"id": 102, "name": "deploy-prod", "queueStatus": "paused", "folder": "\\Project"}
]
"#;

        let out = filter_pipelines_list(input).unwrap();
        assert!(out.contains("AZ pipelines: 2"));
        assert!(out.contains("101 build-ci [enabled]"));
        assert!(out.contains("102 deploy-prod [paused]"));
    }

    #[test]
    fn test_filter_pipeline_show() {
        let input = r#"
{
  "id": 101,
  "name": "build-ci",
  "queueStatus": "enabled",
  "folder": "\\Project",
  "revision": 7,
  "url": "https://dev.azure.com/org/project/_build?definitionId=101"
}
"#;

        let out = filter_pipeline_show(input).unwrap();
        assert!(out.contains("AZ pipeline: 101 build-ci [enabled] rev=7"));
        assert!(out.contains("folder: \\Project"));
    }

    #[test]
    fn test_filter_pipeline_runs_list() {
        let input = r#"
[
  {
    "id": 2345,
    "pipeline": {"name": "build-ci"},
    "status": "completed",
    "result": "succeeded",
    "createdDate": "2026-01-20T10:30:00Z",
    "sourceBranch": "refs/heads/main"
  }
]
"#;

        let out = filter_pipeline_runs_list(input).unwrap();
        assert!(out.contains("AZ pipeline runs: 1"));
        assert!(out.contains("2345 build-ci completed/succeeded 2026-01-20 main"));
    }

    #[test]
    fn test_filter_pipeline_run_show() {
        let input = r#"
{
  "id": 2345,
  "pipeline": {"name": "build-ci"},
  "status": "completed",
  "result": "failed",
  "createdDate": "2026-01-20T10:30:00Z",
  "sourceBranch": "refs/heads/feature/az-provider",
  "reason": "manual"
}
"#;

        let out = filter_pipeline_run_show(input).unwrap();
        assert!(out.contains("AZ pipeline run: 2345 build-ci completed/failed 2026-01-20"));
        assert!(out.contains("branch: feature/az-provider"));
        assert!(out.contains("reason: manual"));
    }

    #[test]
    fn test_filter_webapp_list() {
        let input = r#"
[
  {
    "name": "portal-api",
    "state": "Running",
    "defaultHostName": "portal-api.azurewebsites.net",
    "resourceGroup": "rg-apps"
  }
]
"#;

        let out = filter_webapp_list(input).unwrap();
        assert!(out.contains("AZ webapps: 1"));
        assert!(out.contains("portal-api [Running]"));
        assert!(out.contains("rg=rg-apps"));
    }

    #[test]
    fn test_filter_monitor_metrics_list() {
        let input = r#"
{
  "value": [
    {
      "name": {"localizedValue": "Requests"},
      "timeseries": [
        {
          "data": [
            {"total": 12},
            {"total": 8}
          ]
        }
      ]
    }
  ]
}
"#;

        let out = filter_monitor_metrics_list(input).unwrap();
        assert!(out.contains("AZ monitor metrics: 1"));
        assert!(out.contains("Requests points=2 nonEmpty=2"));
    }

    #[test]
    fn test_filter_app_insights_query() {
        let input = r#"
{
  "tables": [
    {
      "name": "PrimaryResult",
      "columns": [{"name":"timestamp"}, {"name":"count"}],
      "rows": [["2026-03-01T00:00:00Z", 15], ["2026-03-01T00:01:00Z", 20]]
    }
  ]
}
"#;

        let out = filter_app_insights_query(input).unwrap();
        assert!(out.contains("AZ App Insights query tables: 1"));
        assert!(out.contains("PrimaryResult rows=2 cols=2"));
    }

    #[test]
    fn test_filter_generic_az_json_value_list() {
        let input = r#"{ "value": ["repo-a", "repo-b"] }"#;
        let out = filter_generic_az_json(input).unwrap();
        assert!(out.contains("AZ items: 2"));
        assert!(out.contains("repo-a"));
    }

    #[test]
    fn test_strip_branch_prefix() {
        assert_eq!(strip_branch_prefix("refs/heads/main"), "main");
        assert_eq!(strip_branch_prefix("refs/tags/v1"), "refs/tags/v1");
    }
}
