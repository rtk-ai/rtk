//! AWS CLI output compression.
//!
//! Replaces verbose `--output table`/`text` with JSON, then compresses.
//! Specialized filters for high-frequency commands (STS, S3, EC2, ECS, RDS, CloudFormation).

use crate::core::tee::force_tee_hint;
use crate::core::tracking;
use crate::core::utils::{
    exit_code_from_output, exit_code_from_status, human_bytes, join_with_overflow,
    resolved_command, shorten_arn, truncate_iso_date,
};
use crate::json_cmd;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use serde_json::Value;

const MAX_ITEMS: usize = 20;
const JSON_COMPRESS_DEPTH: usize = 4;

/// Result of a filter function: filtered text + whether items were truncated.
/// When `truncated` is true, the shared runner force-tees the full raw output
/// so the LLM has a recovery path to access all data.
struct FilterResult {
    text: String,
    truncated: bool,
}

impl FilterResult {
    fn new(text: String) -> Self {
        Self {
            text,
            truncated: false,
        }
    }

    fn truncated(text: String) -> Self {
        Self {
            text,
            truncated: true,
        }
    }
}

/// Run an AWS CLI command with token-optimized output
pub fn run(subcommand: &str, args: &[String], verbose: u8) -> Result<i32> {
    // Build the full sub-path: e.g. "sts" + ["get-caller-identity"] -> "sts get-caller-identity"
    let full_sub = if args.is_empty() {
        subcommand.to_string()
    } else {
        format!("{} {}", subcommand, args.join(" "))
    };

    // Route to specialized handlers
    match subcommand {
        "sts" if !args.is_empty() && args[0] == "get-caller-identity" => run_aws_filtered(
            &["sts", "get-caller-identity"],
            &args[1..],
            verbose,
            filter_sts_identity,
        ),
        "s3" if !args.is_empty() && args[0] == "ls" => run_s3_ls(&args[1..], verbose),
        "ec2" if !args.is_empty() && args[0] == "describe-instances" => run_aws_filtered(
            &["ec2", "describe-instances"],
            &args[1..],
            verbose,
            filter_ec2_instances,
        ),
        "ecs" if !args.is_empty() && args[0] == "list-services" => run_aws_filtered(
            &["ecs", "list-services"],
            &args[1..],
            verbose,
            filter_ecs_list_services,
        ),
        "ecs" if !args.is_empty() && args[0] == "describe-services" => run_aws_filtered(
            &["ecs", "describe-services"],
            &args[1..],
            verbose,
            filter_ecs_describe_services,
        ),
        "rds" if !args.is_empty() && args[0] == "describe-db-instances" => run_aws_filtered(
            &["rds", "describe-db-instances"],
            &args[1..],
            verbose,
            filter_rds_instances,
        ),
        "cloudformation" if !args.is_empty() && args[0] == "list-stacks" => run_aws_filtered(
            &["cloudformation", "list-stacks"],
            &args[1..],
            verbose,
            filter_cfn_list_stacks,
        ),
        "cloudformation" if !args.is_empty() && args[0] == "describe-stacks" => run_aws_filtered(
            &["cloudformation", "describe-stacks"],
            &args[1..],
            verbose,
            filter_cfn_describe_stacks,
        ),
        "cloudformation" if !args.is_empty() && args[0] == "describe-stack-events" => {
            run_aws_filtered(
                &["cloudformation", "describe-stack-events"],
                &args[1..],
                verbose,
                filter_cfn_events,
            )
        }
        "logs"
            if !args.is_empty()
                && (args[0] == "get-log-events" || args[0] == "filter-log-events") =>
        {
            run_aws_filtered(&["logs", &args[0]], &args[1..], verbose, filter_logs_events)
        }
        "lambda" if !args.is_empty() && args[0] == "list-functions" => run_aws_filtered(
            &["lambda", "list-functions"],
            &args[1..],
            verbose,
            filter_lambda_list,
        ),
        "lambda" if !args.is_empty() && args[0] == "get-function" => run_aws_filtered(
            &["lambda", "get-function"],
            &args[1..],
            verbose,
            filter_lambda_get,
        ),
        "iam" if !args.is_empty() && args[0] == "list-roles" => run_aws_filtered(
            &["iam", "list-roles"],
            &args[1..],
            verbose,
            filter_iam_roles,
        ),
        "iam" if !args.is_empty() && args[0] == "list-users" => run_aws_filtered(
            &["iam", "list-users"],
            &args[1..],
            verbose,
            filter_iam_users,
        ),
        "dynamodb" if !args.is_empty() && (args[0] == "scan" || args[0] == "query") => {
            run_aws_filtered(
                &["dynamodb", &args[0]],
                &args[1..],
                verbose,
                filter_dynamodb_items,
            )
        }
        "ecs" if !args.is_empty() && args[0] == "describe-tasks" => run_aws_filtered(
            &["ecs", "describe-tasks"],
            &args[1..],
            verbose,
            filter_ecs_tasks,
        ),
        "ec2" if !args.is_empty() && args[0] == "describe-security-groups" => run_aws_filtered(
            &["ec2", "describe-security-groups"],
            &args[1..],
            verbose,
            filter_security_groups,
        ),
        "s3api" if !args.is_empty() && args[0] == "list-objects-v2" => run_aws_filtered(
            &["s3api", "list-objects-v2"],
            &args[1..],
            verbose,
            filter_s3_objects,
        ),
        "eks" if !args.is_empty() && args[0] == "describe-cluster" => run_aws_filtered(
            &["eks", "describe-cluster"],
            &args[1..],
            verbose,
            filter_eks_cluster,
        ),
        "sqs" if !args.is_empty() && args[0] == "receive-message" => run_aws_filtered(
            &["sqs", "receive-message"],
            &args[1..],
            verbose,
            filter_sqs_messages,
        ),
        "dynamodb" if !args.is_empty() && args[0] == "get-item" => run_aws_filtered(
            &["dynamodb", "get-item"],
            &args[1..],
            verbose,
            filter_dynamodb_get_item,
        ),
        "logs" if !args.is_empty() && args[0] == "get-query-results" => run_aws_filtered(
            &["logs", "get-query-results"],
            &args[1..],
            verbose,
            filter_logs_query_results,
        ),
        "s3" if !args.is_empty() && (args[0] == "sync" || args[0] == "cp") => {
            run_s3_transfer(&args[0], &args[1..], verbose)
        }
        "secretsmanager" if !args.is_empty() && args[0] == "get-secret-value" => run_aws_filtered(
            &["secretsmanager", "get-secret-value"],
            &args[1..],
            verbose,
            filter_secrets_get,
        ),
        _ => run_generic(subcommand, args, verbose, &full_sub),
    }
}

/// Returns true for operations that return structured JSON (describe-*, list-*, get-*).
/// Mutating/transfer operations (s3 cp, s3 sync, s3 mb, etc.) emit plain text progress
/// and do not accept --output json, so we must not inject it for them.
fn is_structured_operation(args: &[String]) -> bool {
    let op = args.first().map(|s| s.as_str()).unwrap_or("");
    // Exclude s3 sync/cp (they're text operations)
    if op == "sync" || op == "cp" {
        return false;
    }
    op.starts_with("describe-")
        || op.starts_with("list-")
        || op.starts_with("get-")
        || op == "scan"
        || op == "query"
        || op == "receive-message"
}

/// Generic strategy: force --output json for structured ops, compress via json_cmd schema
fn run_generic(subcommand: &str, args: &[String], verbose: u8, full_sub: &str) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("aws");
    cmd.arg(subcommand);

    let mut has_output_flag = false;
    for arg in args {
        if arg == "--output" {
            has_output_flag = true;
        }
        cmd.arg(arg);
    }

    // Only inject --output json for structured read operations.
    // Mutating/transfer operations (s3 cp, s3 sync, s3 mb, cloudformation deploy…)
    // emit plain-text progress and reject --output json.
    if !has_output_flag && is_structured_operation(args) {
        cmd.args(["--output", "json"]);
    }

    if verbose > 0 {
        eprintln!("Running: aws {}", full_sub);
    }

    let output = cmd.output().context("Failed to run aws CLI")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        timer.track(
            &format!("aws {}", full_sub),
            &format!("rtk aws {}", full_sub),
            &stderr,
            &stderr,
        );
        eprintln!("{}", stderr.trim());
        return Ok(crate::core::utils::exit_code_from_output(&output, "aws"));
    }

    let filtered = match json_cmd::filter_json_string(&raw, JSON_COMPRESS_DEPTH) {
        Ok(schema) => {
            println!("{}", schema);
            schema
        }
        Err(_) => {
            // Fallback: print raw (maybe not JSON)
            print!("{}", raw);
            raw.clone()
        }
    };

    timer.track(
        &format!("aws {}", full_sub),
        &format!("rtk aws {}", full_sub),
        &raw,
        &filtered,
    );

    Ok(0)
}

fn run_aws_json(
    sub_args: &[&str],
    extra_args: &[String],
    verbose: u8,
) -> Result<(String, String, std::process::ExitStatus)> {
    let mut cmd = resolved_command("aws");
    for arg in sub_args {
        cmd.arg(arg);
    }

    // Replace --output table/text with --output json
    let mut skip_next = false;
    for arg in extra_args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "--output" {
            skip_next = true;
            continue;
        }
        if arg.starts_with("--output=") {
            continue;
        }
        cmd.arg(arg);
    }
    cmd.args(["--output", "json"]);

    let cmd_desc = format!("aws {}", sub_args.join(" "));
    if verbose > 0 {
        eprintln!("Running: {}", cmd_desc);
    }

    let output = cmd
        .output()
        .context(format!("Failed to run {}", cmd_desc))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        eprintln!("{}", stderr.trim());
    }

    Ok((stdout, stderr, output.status))
}

/// Shared runner for AWS commands that return JSON.
/// Follows the six-phase contract: timer → execute → filter (fallback) → tee → track → exit code.
fn run_aws_filtered(
    sub_args: &[&str],
    extra_args: &[String],
    verbose: u8,
    filter_fn: fn(&str) -> Option<FilterResult>,
) -> Result<i32> {
    let cmd_label = format!("aws {}", sub_args.join(" "));
    let rtk_label = format!("rtk {}", cmd_label);
    let slug = cmd_label.replace(' ', "_");
    let timer = tracking::TimedExecution::start();
    let (stdout, stderr, status) = run_aws_json(sub_args, extra_args, verbose)?;

    // Combine stdout+stderr for accurate tracking (per contract)
    let raw = if stderr.is_empty() {
        stdout.clone()
    } else {
        format!("{}\n{}", stdout, stderr)
    };

    if !status.success() {
        let exit_code = exit_code_from_status(&status, "aws");
        if let Some(hint) = crate::core::tee::tee_and_hint(&raw, &slug, exit_code) {
            eprintln!("{}\n{}", stderr.trim(), hint);
        } else {
            eprintln!("{}", stderr.trim());
        }
        timer.track(&cmd_label, &rtk_label, &raw, &stderr);
        return Ok(exit_code);
    }

    let result = filter_fn(&stdout).unwrap_or_else(|| {
        eprintln!("rtk: filter warning: aws filter returned None, passing through raw output");
        FilterResult::new(stdout.clone())
    });

    if result.truncated {
        if let Some(hint) = crate::core::tee::force_tee_hint(&raw, &slug) {
            println!("{}\n{}", result.text, hint);
        } else {
            println!("{}", result.text);
        }
    } else if let Some(hint) = crate::core::tee::tee_and_hint(&raw, &slug, 0) {
        println!("{}\n{}", result.text, hint);
    } else {
        println!("{}", result.text);
    }

    timer.track(&cmd_label, &rtk_label, &raw, &result.text);
    Ok(0)
}

fn run_sts_identity(extra_args: &[String], verbose: u8) -> Result<i32> {
    run_aws_filtered(
        &["sts", "get-caller-identity"],
        extra_args,
        verbose,
        filter_sts_identity,
    )
}

fn run_s3_ls(extra_args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("aws");
    cmd.args(["s3", "ls"]);
    for arg in extra_args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: aws s3 ls {}", extra_args.join(" "));
    }

    let output = cmd.output().context("Failed to run aws s3 ls")?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let raw = if stderr.is_empty() {
        stdout.clone()
    } else {
        format!("{}\n{}", stdout, stderr)
    };
    if !output.status.success() {
        let exit_code = exit_code_from_output(&output, "aws");
        if let Some(hint) = crate::core::tee::tee_and_hint(&raw, "aws_s3_ls", exit_code) {
            eprintln!("{}\n{}", stderr.trim(), hint);
        } else {
            eprintln!("{}", stderr.trim());
        }
        timer.track("aws s3 ls", "rtk aws s3 ls", &raw, &stderr);
        return Ok(exit_code);
    }

    let result = filter_s3_ls(&stdout);
    if result.truncated {
        if let Some(hint) = crate::core::tee::force_tee_hint(&raw, "aws_s3_ls") {
            println!("{}\n{}", result.text, hint);
        } else {
            println!("{}", result.text);
        }
    } else {
        println!("{}", result.text);
    }

    timer.track("aws s3 ls", "rtk aws s3 ls", &raw, &result.text);
    Ok(0)
}

/// Run s3 sync/cp (text output, not JSON)
fn run_s3_transfer(operation: &str, extra_args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let cmd_label = format!("aws s3 {}", operation);
    let rtk_label = format!("rtk aws s3 {}", operation);
    let slug = format!("aws_s3_{}", operation);

    let mut cmd = resolved_command("aws");
    cmd.args(["s3", operation]);
    for arg in extra_args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: {} {}", cmd_label, extra_args.join(" "));
    }

    let output = cmd
        .output()
        .context(format!("Failed to run {}", cmd_label))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let raw = if stderr.is_empty() {
        stdout.clone()
    } else {
        format!("{}\n{}", stdout, stderr)
    };
    if !output.status.success() {
        let exit_code = exit_code_from_output(&output, "aws");
        if let Some(hint) = crate::core::tee::tee_and_hint(&raw, &slug, exit_code) {
            eprintln!("{}\n{}", stderr.trim(), hint);
        } else {
            eprintln!("{}", stderr.trim());
        }
        timer.track(&cmd_label, &rtk_label, &raw, &stderr);
        return Ok(exit_code);
    }

    let result = filter_s3_transfer(&stdout);
    if result.truncated {
        if let Some(hint) = force_tee_hint(&raw, &slug) {
            println!("{}\n{}", result.text, hint);
        } else {
            println!("{}", result.text);
        }
    } else {
        println!("{}", result.text);
    }

    timer.track(&cmd_label, &rtk_label, &raw, &result.text);
    Ok(0)
}

// --- Filter functions (all use serde_json::Value for resilience) ---
// Each returns Option<FilterResult>: Some = filtered, None = fallback to raw.
// FilterResult.truncated = true means items were cut; shared runner will tee full output.

fn filter_sts_identity(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let account = v["Account"].as_str().unwrap_or("?");
    let arn = v["Arn"].as_str().unwrap_or("?");
    Some(FilterResult::new(format!("AWS: {} {}", account, arn)))
}

fn filter_s3_ls(output: &str) -> FilterResult {
    let lines: Vec<&str> = output.lines().collect();
    let total = lines.len();
    let limit = MAX_ITEMS + 10;

    if total > limit {
        let text = format!(
            "{}\n... +{} more items",
            lines[..limit].join("\n"),
            total - limit
        );
        FilterResult::truncated(text)
    } else {
        FilterResult::new(lines.join("\n"))
    }
}

fn filter_ec2_instances(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let reservations = v["Reservations"].as_array()?;

    let mut instances: Vec<String> = Vec::new();
    for res in reservations {
        if let Some(insts) = res["Instances"].as_array() {
            for inst in insts {
                let id = inst["InstanceId"].as_str().unwrap_or("?");
                let state = inst["State"]["Name"].as_str().unwrap_or("?");
                let itype = inst["InstanceType"].as_str().unwrap_or("?");
                let private_ip = inst["PrivateIpAddress"].as_str().unwrap_or("-");
                let public_ip = inst["PublicIpAddress"].as_str().unwrap_or("-");
                let subnet = inst["SubnetId"].as_str().unwrap_or("-");
                let vpc = inst["VpcId"].as_str().unwrap_or("-");

                let name = inst["Tags"]
                    .as_array()
                    .and_then(|tags| tags.iter().find(|t| t["Key"].as_str() == Some("Name")))
                    .and_then(|t| t["Value"].as_str())
                    .unwrap_or("-");

                let sgs: Vec<&str> = inst["SecurityGroups"]
                    .as_array()
                    .map(|arr| arr.iter().filter_map(|sg| sg["GroupId"].as_str()).collect())
                    .unwrap_or_default();
                let sg_str = if sgs.is_empty() {
                    "-".to_string()
                } else {
                    sgs.join(",")
                };

                instances.push(format!(
                    "{} {} {} {} pub:{} vpc:{} subnet:{} sg:[{}] ({})",
                    id, state, itype, private_ip, public_ip, vpc, subnet, sg_str, name
                ));
            }
        }
    }

    let total = instances.len();
    let truncated = total > MAX_ITEMS;
    let mut result = format!("EC2: {} instances\n", total);

    for inst in instances.iter().take(MAX_ITEMS) {
        result.push_str(&format!("  {}\n", inst));
    }

    if truncated {
        result.push_str(&format!("  ... +{} more\n", total - MAX_ITEMS));
    }

    let text = result.trim_end().to_string();
    Some(if truncated {
        FilterResult::truncated(text)
    } else {
        FilterResult::new(text)
    })
}

fn filter_ecs_list_services(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let arns = v["serviceArns"].as_array()?;

    let mut result = Vec::new();
    let total = arns.len();

    for arn in arns.iter().take(MAX_ITEMS) {
        let arn_str = arn.as_str().unwrap_or("?");
        result.push(shorten_arn(arn_str).to_string());
    }

    let text = join_with_overflow(&result, total, MAX_ITEMS, "services");
    Some(if total > MAX_ITEMS {
        FilterResult::truncated(text)
    } else {
        FilterResult::new(text)
    })
}

fn filter_ecs_describe_services(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let services = v["services"].as_array()?;

    let mut result = Vec::new();
    let total = services.len();

    for svc in services.iter().take(MAX_ITEMS) {
        let name = svc["serviceName"].as_str().unwrap_or("?");
        let status = svc["status"].as_str().unwrap_or("?");
        let running = svc["runningCount"].as_i64().unwrap_or(0);
        let desired = svc["desiredCount"].as_i64().unwrap_or(0);
        let launch = svc["launchType"].as_str().unwrap_or("?");
        result.push(format!(
            "{} {} {}/{} ({})",
            name, status, running, desired, launch
        ));
    }

    let text = join_with_overflow(&result, total, MAX_ITEMS, "services");
    Some(if total > MAX_ITEMS {
        FilterResult::truncated(text)
    } else {
        FilterResult::new(text)
    })
}

fn filter_rds_instances(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let dbs = v["DBInstances"].as_array()?;

    let mut result = Vec::new();
    let total = dbs.len();

    for db in dbs.iter().take(MAX_ITEMS) {
        let name = db["DBInstanceIdentifier"].as_str().unwrap_or("?");
        let engine = db["Engine"].as_str().unwrap_or("?");
        let version = db["EngineVersion"].as_str().unwrap_or("?");
        let class = db["DBInstanceClass"].as_str().unwrap_or("?");
        let status = db["DBInstanceStatus"].as_str().unwrap_or("?");
        let endpoint = db["Endpoint"]["Address"].as_str().unwrap_or("-");
        let port = db["Endpoint"]["Port"].as_i64().unwrap_or(0);
        result.push(format!(
            "{} {} {} {} {} {}:{}",
            name, engine, version, class, status, endpoint, port
        ));
    }

    let text = join_with_overflow(&result, total, MAX_ITEMS, "instances");
    Some(if total > MAX_ITEMS {
        FilterResult::truncated(text)
    } else {
        FilterResult::new(text)
    })
}

fn filter_cfn_list_stacks(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let stacks = v["StackSummaries"].as_array()?;

    let mut result = Vec::new();
    let total = stacks.len();

    for stack in stacks.iter().take(MAX_ITEMS) {
        let name = stack["StackName"].as_str().unwrap_or("?");
        let status = stack["StackStatus"].as_str().unwrap_or("?");
        let date = stack["LastUpdatedTime"]
            .as_str()
            .or_else(|| stack["CreationTime"].as_str())
            .unwrap_or("?");
        result.push(format!("{} {} {}", name, status, truncate_iso_date(date)));
    }

    let text = join_with_overflow(&result, total, MAX_ITEMS, "stacks");
    Some(if total > MAX_ITEMS {
        FilterResult::truncated(text)
    } else {
        FilterResult::new(text)
    })
}

fn filter_cfn_describe_stacks(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let stacks = v["Stacks"].as_array()?;

    let mut result = Vec::new();
    let total = stacks.len();

    for stack in stacks.iter().take(MAX_ITEMS) {
        let name = stack["StackName"].as_str().unwrap_or("?");
        let status = stack["StackStatus"].as_str().unwrap_or("?");
        let date = stack["LastUpdatedTime"]
            .as_str()
            .or_else(|| stack["CreationTime"].as_str())
            .unwrap_or("?");
        result.push(format!("{} {} {}", name, status, truncate_iso_date(date)));

        if let Some(outputs) = stack["Outputs"].as_array() {
            for out in outputs {
                let key = out["OutputKey"].as_str().unwrap_or("?");
                let val = out["OutputValue"].as_str().unwrap_or("?");
                result.push(format!("  {}={}", key, val));
            }
        }
    }

    let text = join_with_overflow(&result, total, MAX_ITEMS, "stacks");
    Some(if total > MAX_ITEMS {
        FilterResult::truncated(text)
    } else {
        FilterResult::new(text)
    })
}

// --- P0 filters: CloudWatch Logs, CloudFormation Events, Lambda ---

const MAX_LOG_EVENTS: usize = 50;

/// Convert days since Unix epoch to (year, month, day). Civil calendar, UTC.
fn days_to_ymd(days: i64) -> (i64, i64, i64) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn filter_logs_events(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let events = v["events"].as_array()?;

    let total = events.len();
    let truncated = total > MAX_LOG_EVENTS;
    let mut lines = Vec::new();

    for event in events.iter().take(MAX_LOG_EVENTS) {
        // Convert epoch ms to YYYY-MM-DD HH:MM:SS UTC
        let time_str = match event["timestamp"].as_i64() {
            Some(ts) if ts > 0 => {
                let epoch_secs = ts / 1000;
                // Days since Unix epoch
                let days = epoch_secs / 86400;
                let time_of_day = epoch_secs % 86400;
                let h = time_of_day / 3600;
                let m = (time_of_day % 3600) / 60;
                let s = time_of_day % 60;
                // Convert days to Y-M-D (simplified: good through 2099)
                let (y, mo, d) = days_to_ymd(days);
                format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, mo, d, h, m, s)
            }
            _ => "??:??:??".to_string(),
        };

        let msg = event["message"].as_str().unwrap_or("").trim_end();
        // If the message is JSON, compact it to one line
        let compact_msg = if msg.starts_with('{') {
            serde_json::from_str::<Value>(msg)
                .ok()
                .and_then(|v| serde_json::to_string(&v).ok())
                .unwrap_or_else(|| msg.to_string())
        } else {
            msg.to_string()
        };

        lines.push(format!("{} {}", time_str, compact_msg));
    }

    if truncated {
        lines.push(format!("... +{} more events", total - MAX_LOG_EVENTS));
    }

    let text = lines.join("\n");
    Some(if truncated {
        FilterResult::truncated(text)
    } else {
        FilterResult::new(text)
    })
}

fn filter_cfn_events(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let events = v["StackEvents"].as_array()?;

    let mut failed = Vec::new();
    let mut failed_count = 0usize;
    let mut success_count = 0usize;

    for event in events {
        let status = event["ResourceStatus"].as_str().unwrap_or("?");
        let logical_id = event["LogicalResourceId"].as_str().unwrap_or("?");
        let resource_type_raw = event["ResourceType"].as_str().unwrap_or("?");
        let resource_type = resource_type_raw
            .strip_prefix("AWS::")
            .unwrap_or(resource_type_raw);
        let ts = event["Timestamp"]
            .as_str()
            .map(truncate_iso_date)
            .unwrap_or("?");

        if status.contains("FAILED") || status.contains("ROLLBACK") {
            failed_count += 1;
            if failed.len() < MAX_ITEMS {
                let reason = event["ResourceStatusReason"].as_str().unwrap_or("");
                let mut line = format!("{} {} {} {}", ts, logical_id, resource_type, status);
                if !reason.is_empty() {
                    line.push_str(&format!(" REASON: {}", reason));
                }
                failed.push(line);
            }
        } else {
            success_count += 1;
        }
    }

    let total_events = events.len();
    let mut lines = Vec::new();
    lines.push(format!(
        "CloudFormation: {} events ({} failed, {} successful)",
        total_events, failed_count, success_count
    ));

    if !failed.is_empty() {
        lines.push("--- FAILURES ---".to_string());
        for f in &failed {
            lines.push(format!("  {}", f));
        }
    }

    if success_count > 0 {
        lines.push(format!("+ {} successful resources", success_count));
    }

    // Truncate if huge number of events
    let truncated = total_events > MAX_ITEMS * 5; // >100 events
    let text = lines.join("\n");
    Some(if truncated {
        FilterResult::truncated(text)
    } else {
        FilterResult::new(text)
    })
}

fn filter_lambda_list(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let functions = v["Functions"].as_array()?;

    let total = functions.len();
    let truncated = total > MAX_ITEMS;
    let mut result = Vec::new();

    for func in functions.iter().take(MAX_ITEMS) {
        let name = func["FunctionName"].as_str().unwrap_or("?");
        let runtime = func["Runtime"].as_str().unwrap_or("?");
        let memory = func["MemorySize"].as_i64().unwrap_or(0);
        let timeout = func["Timeout"].as_i64().unwrap_or(0);
        let state = func["State"].as_str().unwrap_or("active");
        // SECURITY: Environment is intentionally NOT read (may contain secrets)
        result.push(format!(
            "{} {} {}MB {}s {}",
            name, runtime, memory, timeout, state
        ));
    }

    let text = join_with_overflow(&result, total, MAX_ITEMS, "functions");
    Some(if truncated {
        FilterResult::truncated(text)
    } else {
        FilterResult::new(text)
    })
}

fn filter_lambda_get(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let config = &v["Configuration"];

    let name = config["FunctionName"].as_str().unwrap_or("?");
    let runtime = config["Runtime"].as_str().unwrap_or("?");
    let handler = config["Handler"].as_str().unwrap_or("?");
    let memory = config["MemorySize"].as_i64().unwrap_or(0);
    let timeout = config["Timeout"].as_i64().unwrap_or(0);
    let state = config["State"].as_str().unwrap_or("active");
    let last_modified = config["LastModified"]
        .as_str()
        .map(truncate_iso_date)
        .unwrap_or("?");
    // SECURITY: Environment and Code.Location intentionally NOT read

    let mut text = format!(
        "{} {} {} {}MB {}s {} {}",
        name, runtime, handler, memory, timeout, state, last_modified
    );

    // Show layer names if present
    // Layer ARNs use colons: arn:aws:lambda:region:acct:layer:name:version
    if let Some(layers) = config["Layers"].as_array() {
        if !layers.is_empty() {
            let layer_names: Vec<String> = layers
                .iter()
                .filter_map(|l| {
                    let arn = l["Arn"].as_str()?;
                    let parts: Vec<&str> = arn.rsplitn(3, ':').collect();
                    if parts.len() >= 2 {
                        Some(format!("{}:{}", parts[1], parts[0]))
                    } else {
                        Some(arn.to_string())
                    }
                })
                .collect();
            text.push_str(&format!("\n  layers: {}", layer_names.join(", ")));
        }
    }

    Some(FilterResult::new(text))
}

// --- P1 filters: IAM, DynamoDB, ECS tasks ---

/// Extract principal services/accounts from AssumeRolePolicyDocument.
/// Returns compact list like ["lambda.amazonaws.com", "ecs-tasks.amazonaws.com"]
/// instead of the full 200+ token JSON policy document.
fn extract_assume_principals(role: &Value) -> Vec<String> {
    let mut principals = Vec::new();
    // AssumeRolePolicyDocument can be a JSON string or an object
    let doc = if let Some(s) = role["AssumeRolePolicyDocument"].as_str() {
        serde_json::from_str::<Value>(s).ok()
    } else if role["AssumeRolePolicyDocument"].is_object() {
        Some(role["AssumeRolePolicyDocument"].clone())
    } else {
        None
    };
    if let Some(doc) = doc {
        let statements = doc["Statement"].as_array();
        if let Some(stmts) = statements {
            for stmt in stmts {
                let principal = &stmt["Principal"];
                // Principal can be "*", {"Service": "..."}, {"AWS": "..."}, etc.
                if let Some(s) = principal.as_str() {
                    principals.push(s.to_string());
                } else if let Some(svc) = principal["Service"].as_str() {
                    principals.push(svc.to_string());
                } else if let Some(svcs) = principal["Service"].as_array() {
                    for s in svcs {
                        if let Some(s) = s.as_str() {
                            principals.push(s.to_string());
                        }
                    }
                } else if let Some(aws) = principal["AWS"].as_str() {
                    principals.push(shorten_arn(aws).to_string());
                } else if let Some(awss) = principal["AWS"].as_array() {
                    for a in awss {
                        if let Some(a) = a.as_str() {
                            principals.push(shorten_arn(a).to_string());
                        }
                    }
                }
            }
        }
    }
    principals.dedup();
    principals
}

fn filter_iam_roles(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let roles = v["Roles"].as_array()?;

    let total = roles.len();
    let truncated = total > MAX_ITEMS;
    let mut result = Vec::new();

    for role in roles.iter().take(MAX_ITEMS) {
        let name = role["RoleName"].as_str().unwrap_or("?");
        let date = role["CreateDate"]
            .as_str()
            .map(truncate_iso_date)
            .unwrap_or("?");
        let desc = role["Description"].as_str().unwrap_or("");

        // Extract principals from AssumeRolePolicyDocument (compact, not full JSON)
        let principals = extract_assume_principals(role);
        let principal_str = if principals.is_empty() {
            String::new()
        } else {
            format!(" assume:[{}]", principals.join(","))
        };

        if desc.is_empty() {
            result.push(format!("{} {}{}", name, date, principal_str));
        } else {
            result.push(format!("{} {} [{}]{}", name, date, desc, principal_str));
        }
    }

    let text = join_with_overflow(&result, total, MAX_ITEMS, "roles");
    Some(if truncated {
        FilterResult::truncated(text)
    } else {
        FilterResult::new(text)
    })
}

fn filter_iam_users(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let users = v["Users"].as_array()?;

    let total = users.len();
    let truncated = total > MAX_ITEMS;
    let mut result = Vec::new();

    for user in users.iter().take(MAX_ITEMS) {
        let name = user["UserName"].as_str().unwrap_or("?");
        let date = user["CreateDate"]
            .as_str()
            .map(truncate_iso_date)
            .unwrap_or("?");
        result.push(format!("{} created:{}", name, date));
    }

    let text = join_with_overflow(&result, total, MAX_ITEMS, "users");
    Some(if truncated {
        FilterResult::truncated(text)
    } else {
        FilterResult::new(text)
    })
}

/// Recursively unwrap DynamoDB typed values to plain JSON.
/// `{"S": "foo"}` -> `"foo"`, `{"N": "42"}` -> `42`, `{"M": {...}}` -> unwrapped object, etc.
fn unwrap_dynamodb_value(val: &Value, depth: usize) -> Value {
    if depth > 10 {
        return val.clone();
    }

    if let Some(obj) = val.as_object() {
        if obj.len() == 1 {
            if let Some((key, inner)) = obj.iter().next() {
                match key.as_str() {
                    "S" | "B" => return inner.clone(),
                    "N" => {
                        if let Some(s) = inner.as_str() {
                            // Try i64 first, then f64
                            if let Ok(n) = s.parse::<i64>() {
                                return Value::Number(n.into());
                            }
                            if let Ok(f) = s.parse::<f64>() {
                                if let Some(n) = serde_json::Number::from_f64(f) {
                                    return Value::Number(n);
                                }
                            }
                            return Value::String(s.to_string());
                        }
                        return inner.clone();
                    }
                    "BOOL" => return inner.clone(),
                    "NULL" => return Value::Null,
                    "L" => {
                        if let Some(arr) = inner.as_array() {
                            return Value::Array(
                                arr.iter()
                                    .map(|v| unwrap_dynamodb_value(v, depth + 1))
                                    .collect(),
                            );
                        }
                    }
                    "M" => {
                        if let Some(map) = inner.as_object() {
                            let unwrapped: serde_json::Map<String, Value> = map
                                .iter()
                                .map(|(k, v)| (k.clone(), unwrap_dynamodb_value(v, depth + 1)))
                                .collect();
                            return Value::Object(unwrapped);
                        }
                    }
                    "SS" => return inner.clone(),
                    "NS" => {
                        // Parse NS set: try i64 first, then f64
                        if let Some(arr) = inner.as_array() {
                            let nums: Vec<Value> = arr
                                .iter()
                                .filter_map(|v| {
                                    let s = v.as_str()?;
                                    if let Ok(n) = s.parse::<i64>() {
                                        Some(Value::Number(n.into()))
                                    } else if let Ok(f) = s.parse::<f64>() {
                                        serde_json::Number::from_f64(f).map(Value::Number)
                                    } else {
                                        Some(Value::String(s.to_string()))
                                    }
                                })
                                .collect();
                            return Value::Array(nums);
                        }
                        return inner.clone();
                    }
                    "BS" => return inner.clone(),
                    _ => {}
                }
            }
        }
        // Not a DynamoDB type wrapper — unwrap each field as a potential item
        let unwrapped: serde_json::Map<String, Value> = obj
            .iter()
            .map(|(k, v)| (k.clone(), unwrap_dynamodb_value(v, depth + 1)))
            .collect();
        return Value::Object(unwrapped);
    }

    val.clone()
}

fn filter_dynamodb_items(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let items = v["Items"].as_array()?;

    let count = v["Count"].as_i64().unwrap_or(items.len() as i64);
    let scanned = v["ScannedCount"].as_i64().unwrap_or(count);
    let total = items.len();
    let truncated = total > MAX_ITEMS;

    let mut lines = Vec::new();
    lines.push(format!("Count: {}/{}", count, scanned));

    // Show ConsumedCapacity if present
    if let Some(capacity) = v["ConsumedCapacity"].as_object() {
        if let Some(units) = capacity["CapacityUnits"].as_f64() {
            lines.push(format!("Capacity: {} RCU", units));
        }
    }

    // Show pagination status if LastEvaluatedKey exists
    if v["LastEvaluatedKey"].is_object() {
        lines.push("(paginated — more results available)".to_string());
    }

    for item in items.iter().take(MAX_ITEMS) {
        let unwrapped = unwrap_dynamodb_value(item, 0);
        let compact = serde_json::to_string(&unwrapped).unwrap_or_else(|_| "?".to_string());
        lines.push(compact);
    }

    if truncated {
        lines.push(format!("... +{} more items", total - MAX_ITEMS));
    }

    let text = lines.join("\n");
    Some(if truncated {
        FilterResult::truncated(text)
    } else {
        FilterResult::new(text)
    })
}

fn filter_ecs_tasks(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let tasks = v["tasks"].as_array()?;

    let total = tasks.len();
    let truncated = total > MAX_ITEMS;
    let mut result = Vec::new();

    for task in tasks.iter().take(MAX_ITEMS) {
        let task_arn = task["taskArn"].as_str().unwrap_or("?");
        let task_id = shorten_arn(task_arn);
        let status = task["lastStatus"].as_str().unwrap_or("?");

        let containers: Vec<String> = task["containers"]
            .as_array()
            .map(|cs| {
                cs.iter()
                    .map(|c| {
                        let name = c["name"].as_str().unwrap_or("?");
                        let cstatus = c["lastStatus"].as_str().unwrap_or("?");
                        let exit = c["exitCode"].as_i64();
                        match exit {
                            Some(code) => format!("{}:{}(exit:{})", name, cstatus, code),
                            None => format!("{}:{}", name, cstatus),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        let stopped_reason = task["stoppedReason"].as_str().unwrap_or("");
        let reason_str = if stopped_reason.is_empty() {
            String::new()
        } else {
            format!(" reason:{}", stopped_reason)
        };

        result.push(format!(
            "{} {} containers:[{}]{}",
            task_id,
            status,
            containers.join(", "),
            reason_str
        ));
    }

    let text = join_with_overflow(&result, total, MAX_ITEMS, "tasks");
    Some(if truncated {
        FilterResult::truncated(text)
    } else {
        FilterResult::new(text)
    })
}

// --- P2 filters: Security Groups, S3 objects, EKS, SQS ---

fn format_sg_rule(perm: &Value) -> String {
    let protocol = perm["IpProtocol"].as_str().unwrap_or("?");
    let proto = if protocol == "-1" { "all" } else { protocol };

    let from_port = perm["FromPort"].as_i64();
    let to_port = perm["ToPort"].as_i64();
    let port = match (from_port, to_port) {
        (Some(f), Some(t)) if f == t => format!("{}", f),
        (Some(f), Some(t)) => format!("{}-{}", f, t),
        _ => "*".to_string(),
    };

    let mut sources = Vec::new();
    if let Some(ranges) = perm["IpRanges"].as_array() {
        for r in ranges {
            if let Some(cidr) = r["CidrIp"].as_str() {
                sources.push(cidr.to_string());
            }
        }
    }
    if let Some(ranges) = perm["Ipv6Ranges"].as_array() {
        for r in ranges {
            if let Some(cidr) = r["CidrIpv6"].as_str() {
                sources.push(cidr.to_string());
            }
        }
    }
    if let Some(groups) = perm["UserIdGroupPairs"].as_array() {
        for g in groups {
            let gid = g["GroupId"].as_str().unwrap_or("?");
            sources.push(gid.to_string());
        }
    }

    let src = if sources.is_empty() {
        "?".to_string()
    } else {
        sources.join(",")
    };

    if proto == "all" {
        format!("all<-{}", src)
    } else {
        format!("{}/{}<-{}", proto, port, src)
    }
}

fn filter_security_groups(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let groups = v["SecurityGroups"].as_array()?;

    let total = groups.len();
    let truncated = total > MAX_ITEMS;
    let mut result = Vec::new();

    for sg in groups.iter().take(MAX_ITEMS) {
        let name = sg["GroupName"].as_str().unwrap_or("?");
        let id = sg["GroupId"].as_str().unwrap_or("?");

        let ingress: Vec<String> = sg["IpPermissions"]
            .as_array()
            .map(|perms| perms.iter().map(format_sg_rule).collect())
            .unwrap_or_default();
        let egress: Vec<String> = sg["IpPermissionsEgress"]
            .as_array()
            .map(|perms| perms.iter().map(format_sg_rule).collect())
            .unwrap_or_default();

        let ingress_str = if ingress.is_empty() {
            "none".to_string()
        } else {
            ingress.join(", ")
        };
        let egress_str = if egress.is_empty() {
            "none".to_string()
        } else {
            egress.join(", ")
        };

        result.push(format!(
            "{} ({}) ingress: {} | egress: {}",
            name, id, ingress_str, egress_str
        ));
    }

    let text = join_with_overflow(&result, total, MAX_ITEMS, "groups");
    Some(if truncated {
        FilterResult::truncated(text)
    } else {
        FilterResult::new(text)
    })
}

fn filter_s3_objects(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let empty_vec = vec![];
    let contents = v["Contents"].as_array().unwrap_or(&empty_vec);

    let total = contents.len();
    let truncated = total > MAX_ITEMS;
    let mut result = Vec::new();

    for obj in contents.iter().take(MAX_ITEMS) {
        let key = obj["Key"].as_str().unwrap_or("?");
        let size = obj["Size"].as_u64().unwrap_or(0);
        let modified = obj["LastModified"]
            .as_str()
            .map(truncate_iso_date)
            .unwrap_or("?");
        result.push(format!("{} {} {}", key, human_bytes(size), modified));
    }

    let text = join_with_overflow(&result, total, MAX_ITEMS, "objects");
    Some(if truncated {
        FilterResult::truncated(text)
    } else {
        FilterResult::new(text)
    })
}

fn filter_eks_cluster(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let cluster = &v["cluster"];

    let name = cluster["name"].as_str().unwrap_or("?");
    let status = cluster["status"].as_str().unwrap_or("?");
    let version = cluster["version"].as_str().unwrap_or("?");
    let endpoint = cluster["endpoint"].as_str().unwrap_or("?");
    // certificateAuthority intentionally NOT read (base64 cert, 1000+ chars)

    let text = format!("{} {} k8s/{} {}", name, status, version, endpoint);
    Some(FilterResult::new(text))
}

lazy_static! {
    static ref S3_TRANSFER_RE: Regex = Regex::new(r"^(upload|download|delete|copy|move):").unwrap();
}

fn filter_sqs_messages(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let empty_vec = vec![];
    let messages = v["Messages"].as_array().unwrap_or(&empty_vec);

    let total = messages.len();
    let truncated = total > MAX_ITEMS;
    let mut result = Vec::new();

    for msg in messages.iter().take(MAX_ITEMS) {
        let id = msg["MessageId"].as_str().unwrap_or("?");
        let id_short = &id[..id.len().min(8)]; // UUIDs are ASCII-safe
        let body = msg["Body"].as_str().unwrap_or("?");
        let body_truncated = crate::core::utils::truncate(body, 200);
        // ReceiptHandle intentionally NOT read (200+ chars of opaque garbage)
        result.push(format!("{} {}", id_short, body_truncated));
    }

    let text = join_with_overflow(&result, total, MAX_ITEMS, "messages");
    Some(if truncated {
        FilterResult::truncated(text)
    } else {
        FilterResult::new(text)
    })
}

fn filter_dynamodb_get_item(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;

    let mut lines = Vec::new();

    // Extract and unwrap the Item
    if let Some(item) = v["Item"].as_object() {
        let unwrapped = unwrap_dynamodb_value(&Value::Object(item.clone()), 0);
        let compact = serde_json::to_string(&unwrapped).unwrap_or_else(|_| "?".to_string());
        lines.push(compact);
    }

    // Show ConsumedCapacity if present
    if let Some(capacity) = v["ConsumedCapacity"].as_object() {
        if let Some(units) = capacity["CapacityUnits"].as_f64() {
            lines.push(format!("Capacity: {} RCU", units));
        }
    }

    if lines.is_empty() {
        return None;
    }

    Some(FilterResult::new(lines.join("\n")))
}

fn filter_logs_query_results(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;

    let mut lines = Vec::new();

    // Show status
    if let Some(status) = v["status"].as_str() {
        lines.push(format!("Status: {}", status));
    }

    // Extract results array (array of arrays of {field, value} objects)
    if let Some(results) = v["results"].as_array() {
        let total = results.len();
        let truncated = total > MAX_ITEMS;

        for row in results.iter().take(MAX_ITEMS) {
            if let Some(fields) = row.as_array() {
                let field_pairs: Vec<String> = fields
                    .iter()
                    .filter_map(|field| {
                        let field_name = field["field"].as_str()?;
                        // Skip internal @ptr field
                        if field_name == "@ptr" {
                            return None;
                        }
                        let field_value = match field["value"].as_str() {
                            Some(s) => s.to_string(),
                            None => field["value"].to_string(), // numbers, booleans
                        };
                        Some(format!("{}={}", field_name, field_value))
                    })
                    .collect();
                lines.push(field_pairs.join(" "));
            }
        }

        if truncated {
            lines.push(format!("... +{} more rows", total - MAX_ITEMS));
        }

        let text = lines.join("\n");
        return Some(if truncated {
            FilterResult::truncated(text)
        } else {
            FilterResult::new(text)
        });
    }

    None
}

fn filter_s3_transfer(output: &str) -> FilterResult {
    let lines: Vec<&str> = output.lines().collect();
    let total = lines.len();

    // Pass through short output unchanged
    if total < 10 {
        return FilterResult::new(output.to_string());
    }

    // Count operations
    let mut uploaded = 0;
    let mut downloaded = 0;
    let mut deleted = 0;
    let mut copied = 0;
    let mut moved = 0;
    let mut errors = Vec::new();

    for line in &lines {
        if let Some(captures) = S3_TRANSFER_RE.captures(line) {
            match captures.get(1).map(|m| m.as_str()) {
                Some("upload") => uploaded += 1,
                Some("download") => downloaded += 1,
                Some("delete") => deleted += 1,
                Some("copy") => copied += 1,
                Some("move") => moved += 1,
                _ => {}
            }
        } else if line.contains("error") || line.contains("failed") {
            errors.push(line.to_string());
        }
    }

    let mut summary_parts = Vec::new();
    if uploaded > 0 {
        summary_parts.push(format!("{} uploaded", uploaded));
    }
    if downloaded > 0 {
        summary_parts.push(format!("{} downloaded", downloaded));
    }
    if deleted > 0 {
        summary_parts.push(format!("{} deleted", deleted));
    }
    if copied > 0 {
        summary_parts.push(format!("{} copied", copied));
    }
    if moved > 0 {
        summary_parts.push(format!("{} moved", moved));
    }

    let mut result_lines = Vec::new();

    if !summary_parts.is_empty() {
        result_lines.push(format!(
            "S3 transfer: {}, {} errors",
            summary_parts.join(", "),
            errors.len()
        ));
    }

    // Include error lines verbatim
    for error in errors.iter().take(10) {
        result_lines.push(error.clone());
    }

    if result_lines.is_empty() {
        return FilterResult::new(output.to_string());
    }

    FilterResult::new(result_lines.join("\n"))
}

fn filter_secrets_get(json_str: &str) -> Option<FilterResult> {
    let v: Value = serde_json::from_str(json_str).ok()?;

    let mut lines = Vec::new();

    // Extract Name
    if let Some(name) = v["Name"].as_str() {
        lines.push(format!("Name: {}", name));
    }

    // Extract SecretString
    if let Some(secret_str) = v["SecretString"].as_str() {
        // Try to parse as JSON and compact it
        if let Ok(secret_json) = serde_json::from_str::<Value>(secret_str) {
            let compact =
                serde_json::to_string(&secret_json).unwrap_or_else(|_| secret_str.to_string());
            lines.push(format!("Secret: {}", compact));
        } else {
            lines.push(format!("Secret: {}", secret_str));
        }
    }

    if lines.is_empty() {
        return None;
    }

    Some(FilterResult::new(lines.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::utils::count_tokens;

    #[test]
    fn test_snapshot_sts_identity() {
        let json = r#"{
    "UserId": "AIDAEXAMPLEUSERID1234",
    "Account": "123456789012",
    "Arn": "arn:aws:iam::123456789012:user/dev-user"
}"#;
        let result = filter_sts_identity(json).unwrap();
        assert_eq!(
            result.text,
            "AWS: 123456789012 arn:aws:iam::123456789012:user/dev-user"
        );
        assert!(!result.truncated);
    }

    #[test]
    fn test_snapshot_ec2_instances() {
        let json = r#"{"Reservations":[{"Instances":[{"InstanceId":"i-0a1b2c3d4e5f00001","InstanceType":"t3.micro","PrivateIpAddress":"10.0.1.10","PublicIpAddress":"54.1.2.3","VpcId":"vpc-123","SubnetId":"subnet-a","State":{"Code":16,"Name":"running"},"Tags":[{"Key":"Name","Value":"web-server-1"}],"BlockDeviceMappings":[],"SecurityGroups":[{"GroupId":"sg-001"}]},{"InstanceId":"i-0a1b2c3d4e5f00002","InstanceType":"t3.large","PrivateIpAddress":"10.0.2.20","VpcId":"vpc-123","SubnetId":"subnet-b","State":{"Code":80,"Name":"stopped"},"Tags":[{"Key":"Name","Value":"worker-1"}],"BlockDeviceMappings":[],"SecurityGroups":[{"GroupId":"sg-002"}]}]}]}"#;
        let result = filter_ec2_instances(json).unwrap();
        assert!(result.text.contains("EC2: 2 instances"));
        assert!(result.text.contains("i-0a1b2c3d4e5f00001 running t3.micro 10.0.1.10 pub:54.1.2.3 vpc:vpc-123 subnet:subnet-a sg:[sg-001] (web-server-1)"));
        assert!(result
            .text
            .contains("i-0a1b2c3d4e5f00002 stopped t3.large 10.0.2.20"));
        assert!(!result.truncated);
    }

    #[test]
    fn test_filter_sts_identity() {
        let json = r#"{
            "UserId": "AIDAEXAMPLE",
            "Account": "123456789012",
            "Arn": "arn:aws:iam::123456789012:user/dev"
        }"#;
        let result = filter_sts_identity(json).unwrap();
        assert_eq!(
            result.text,
            "AWS: 123456789012 arn:aws:iam::123456789012:user/dev"
        );
    }

    #[test]
    fn test_filter_sts_identity_missing_fields() {
        let json = r#"{}"#;
        let result = filter_sts_identity(json).unwrap();
        assert_eq!(result.text, "AWS: ? ?");
    }

    #[test]
    fn test_filter_sts_identity_invalid_json() {
        let result = filter_sts_identity("not json");
        assert!(result.is_none());
    }

    #[test]
    fn test_filter_s3_ls_basic() {
        let output = "2024-01-01 bucket1\n2024-01-02 bucket2\n2024-01-03 bucket3\n";
        let result = filter_s3_ls(output);
        assert!(result.text.contains("bucket1"));
        assert!(result.text.contains("bucket3"));
        assert!(!result.truncated);
    }

    #[test]
    fn test_filter_s3_ls_overflow() {
        let mut lines = Vec::new();
        for i in 1..=50 {
            lines.push(format!("2024-01-01 bucket{}", i));
        }
        let input = lines.join("\n");
        let result = filter_s3_ls(&input);
        assert!(result.text.contains("... +20 more items"));
        assert!(result.truncated);
    }

    #[test]
    fn test_filter_ec2_instances() {
        let json = r#"{
            "Reservations": [{
                "Instances": [{
                    "InstanceId": "i-abc123",
                    "State": {"Name": "running"},
                    "InstanceType": "t3.micro",
                    "PrivateIpAddress": "10.0.1.5",
                    "PublicIpAddress": "54.1.2.3",
                    "VpcId": "vpc-001",
                    "SubnetId": "subnet-001",
                    "SecurityGroups": [{"GroupId": "sg-001", "GroupName": "web"}],
                    "Tags": [{"Key": "Name", "Value": "web-server"}]
                }, {
                    "InstanceId": "i-def456",
                    "State": {"Name": "stopped"},
                    "InstanceType": "t3.large",
                    "PrivateIpAddress": "10.0.1.6",
                    "VpcId": "vpc-001",
                    "SubnetId": "subnet-002",
                    "SecurityGroups": [{"GroupId": "sg-002", "GroupName": "worker"}],
                    "Tags": [{"Key": "Name", "Value": "worker"}]
                }]
            }]
        }"#;
        let result = filter_ec2_instances(json).unwrap();
        assert!(result.text.contains("EC2: 2 instances"));
        assert!(result.text.contains("i-abc123 running t3.micro 10.0.1.5 pub:54.1.2.3 vpc:vpc-001 subnet:subnet-001 sg:[sg-001] (web-server)"));
        assert!(result.text.contains("i-def456 stopped t3.large 10.0.1.6"));
        assert!(result.text.contains("sg:[sg-002]"));
    }

    #[test]
    fn test_filter_ec2_no_name_tag() {
        let json = r#"{
            "Reservations": [{
                "Instances": [{
                    "InstanceId": "i-abc123",
                    "State": {"Name": "running"},
                    "InstanceType": "t3.micro",
                    "PrivateIpAddress": "10.0.1.5",
                    "Tags": []
                }]
            }]
        }"#;
        let result = filter_ec2_instances(json).unwrap();
        assert!(result.text.contains("(-)"));
    }

    #[test]
    fn test_filter_ec2_invalid_json() {
        assert!(filter_ec2_instances("not json").is_none());
    }

    #[test]
    fn test_filter_ecs_list_services() {
        let json = r#"{
            "serviceArns": [
                "arn:aws:ecs:us-east-1:123:service/cluster/api-service",
                "arn:aws:ecs:us-east-1:123:service/cluster/worker-service"
            ]
        }"#;
        let result = filter_ecs_list_services(json).unwrap();
        assert!(result.text.contains("api-service"));
        assert!(result.text.contains("worker-service"));
        assert!(!result.text.contains("arn:aws"));
    }

    #[test]
    fn test_filter_ecs_describe_services() {
        let json = r#"{
            "services": [{
                "serviceName": "api",
                "status": "ACTIVE",
                "runningCount": 3,
                "desiredCount": 3,
                "launchType": "FARGATE"
            }]
        }"#;
        let result = filter_ecs_describe_services(json).unwrap();
        assert_eq!(result.text, "api ACTIVE 3/3 (FARGATE)");
    }

    #[test]
    fn test_filter_rds_instances() {
        let json = r#"{
            "DBInstances": [{
                "DBInstanceIdentifier": "mydb",
                "Engine": "postgres",
                "EngineVersion": "15.4",
                "DBInstanceClass": "db.t3.micro",
                "DBInstanceStatus": "available",
                "Endpoint": {"Address": "mydb.cluster-abc.us-east-1.rds.amazonaws.com", "Port": 5432}
            }]
        }"#;
        let result = filter_rds_instances(json).unwrap();
        assert_eq!(result.text, "mydb postgres 15.4 db.t3.micro available mydb.cluster-abc.us-east-1.rds.amazonaws.com:5432");
    }

    #[test]
    fn test_filter_cfn_list_stacks() {
        let json = r#"{
            "StackSummaries": [{
                "StackName": "my-stack",
                "StackStatus": "CREATE_COMPLETE",
                "CreationTime": "2024-01-15T10:30:00Z"
            }, {
                "StackName": "other-stack",
                "StackStatus": "UPDATE_COMPLETE",
                "LastUpdatedTime": "2024-02-20T14:00:00Z",
                "CreationTime": "2024-01-01T00:00:00Z"
            }]
        }"#;
        let result = filter_cfn_list_stacks(json).unwrap();
        assert!(result.text.contains("my-stack CREATE_COMPLETE 2024-01-15"));
        assert!(result
            .text
            .contains("other-stack UPDATE_COMPLETE 2024-02-20"));
    }

    #[test]
    fn test_filter_cfn_describe_stacks_with_outputs() {
        let json = r#"{
            "Stacks": [{
                "StackName": "my-stack",
                "StackStatus": "CREATE_COMPLETE",
                "CreationTime": "2024-01-15T10:30:00Z",
                "Outputs": [
                    {"OutputKey": "ApiUrl", "OutputValue": "https://api.example.com"},
                    {"OutputKey": "BucketName", "OutputValue": "my-bucket"}
                ]
            }]
        }"#;
        let result = filter_cfn_describe_stacks(json).unwrap();
        assert!(result.text.contains("my-stack CREATE_COMPLETE 2024-01-15"));
        assert!(result.text.contains("ApiUrl=https://api.example.com"));
        assert!(result.text.contains("BucketName=my-bucket"));
    }

    #[test]
    fn test_filter_cfn_describe_stacks_no_outputs() {
        let json = r#"{
            "Stacks": [{
                "StackName": "my-stack",
                "StackStatus": "CREATE_COMPLETE",
                "CreationTime": "2024-01-15T10:30:00Z"
            }]
        }"#;
        let result = filter_cfn_describe_stacks(json).unwrap();
        assert!(result.text.contains("my-stack CREATE_COMPLETE 2024-01-15"));
        assert!(!result.text.contains("="));
    }

    #[test]
    fn test_ec2_token_savings() {
        let json = r#"{
    "Reservations": [{
        "ReservationId": "r-001",
        "OwnerId": "123456789012",
        "Groups": [],
        "Instances": [{
            "InstanceId": "i-0a1b2c3d4e5f00001",
            "ImageId": "ami-0abcdef1234567890",
            "InstanceType": "t3.micro",
            "KeyName": "my-key-pair",
            "LaunchTime": "2024-01-15T10:30:00+00:00",
            "Placement": { "AvailabilityZone": "us-east-1a", "GroupName": "", "Tenancy": "default" },
            "PrivateDnsName": "ip-10-0-1-10.ec2.internal",
            "PrivateIpAddress": "10.0.1.10",
            "PublicDnsName": "ec2-54-0-0-10.compute-1.amazonaws.com",
            "PublicIpAddress": "54.0.0.10",
            "State": { "Code": 16, "Name": "running" },
            "SubnetId": "subnet-0abc123def456001",
            "VpcId": "vpc-0abc123def456001",
            "Architecture": "x86_64",
            "BlockDeviceMappings": [{ "DeviceName": "/dev/xvda", "Ebs": { "AttachTime": "2024-01-15T10:30:05+00:00", "DeleteOnTermination": true, "Status": "attached", "VolumeId": "vol-001" } }],
            "EbsOptimized": false,
            "EnaSupport": true,
            "Hypervisor": "xen",
            "NetworkInterfaces": [{ "NetworkInterfaceId": "eni-001", "PrivateIpAddress": "10.0.1.10", "Status": "in-use" }],
            "RootDeviceName": "/dev/xvda",
            "RootDeviceType": "ebs",
            "SecurityGroups": [{ "GroupId": "sg-001", "GroupName": "web-server-sg" }],
            "SourceDestCheck": true,
            "Tags": [{ "Key": "Name", "Value": "web-server-1" }, { "Key": "Environment", "Value": "production" }, { "Key": "Team", "Value": "backend" }],
            "VirtualizationType": "hvm",
            "CpuOptions": { "CoreCount": 1, "ThreadsPerCore": 2 },
            "MetadataOptions": { "State": "applied", "HttpTokens": "required", "HttpEndpoint": "enabled" }
        }]
    }]
}"#;
        let result = filter_ec2_instances(json).unwrap();
        let input_tokens = count_tokens(json);
        let output_tokens = count_tokens(&result.text);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "EC2 filter: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_sts_token_savings() {
        let json = r#"{
    "UserId": "AIDAEXAMPLEUSERID1234",
    "Account": "123456789012",
    "Arn": "arn:aws:iam::123456789012:user/dev-user"
}"#;
        let result = filter_sts_identity(json).unwrap();
        let input_tokens = count_tokens(json);
        let output_tokens = count_tokens(&result.text);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "STS identity filter: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_rds_overflow() {
        let mut dbs = Vec::new();
        for i in 1..=25 {
            dbs.push(format!(
                r#"{{"DBInstanceIdentifier": "db-{}", "Engine": "postgres", "EngineVersion": "15.4", "DBInstanceClass": "db.t3.micro", "DBInstanceStatus": "available"}}"#,
                i
            ));
        }
        let json = format!(r#"{{"DBInstances": [{}]}}"#, dbs.join(","));
        let result = filter_rds_instances(&json).unwrap();
        assert!(result.text.contains("... +5 more instances"));
        assert!(result.truncated);
    }

    // === P0 filter tests ===

    #[test]
    fn test_filter_logs_events() {
        let json = r#"{
            "events": [
                {"timestamp": 1705312200000, "message": "INFO: Starting service\n", "ingestionTime": 1705312201000},
                {"timestamp": 1705312260000, "message": "ERROR: Connection refused\n", "ingestionTime": 1705312261000},
                {"timestamp": 1705312320000, "message": "{\"level\":\"warn\",\"msg\":\"retrying\"}\n", "ingestionTime": 1705312321000}
            ],
            "nextForwardToken": "f/1234567890abcdef1234567890abcdef",
            "nextBackwardToken": "b/1234567890abcdef1234567890abcdef"
        }"#;
        let result = filter_logs_events(json).unwrap();
        assert!(result.text.contains("INFO: Starting service"));
        assert!(result.text.contains("ERROR: Connection refused"));
        // JSON log message should be compacted to single line
        assert!(result.text.contains("retrying"));
        // Pagination tokens should NOT appear
        assert!(!result.text.contains("nextForwardToken"));
        assert!(!result.text.contains("f/1234567890"));
        assert!(!result.truncated);
    }

    #[test]
    fn test_filter_logs_events_truncation() {
        let mut events = Vec::new();
        for i in 0..60 {
            events.push(format!(
                r#"{{"timestamp": {}, "message": "line {}", "ingestionTime": {}}}"#,
                1705312200000i64 + i * 1000,
                i,
                1705312200000i64 + i * 1000 + 100
            ));
        }
        let json = format!(r#"{{"events": [{}]}}"#, events.join(","));
        let result = filter_logs_events(&json).unwrap();
        assert!(result.text.contains("... +10 more events"));
        assert!(result.truncated);
    }

    #[test]
    fn test_filter_logs_events_token_savings() {
        let mut events = Vec::new();
        for i in 0..20 {
            events.push(format!(
                r#"{{"timestamp": {}, "message": "2024-01-15T10:30:{:02}Z INFO [com.example.service.Handler] Processing request id={} user=admin@example.com action=GET /api/v1/items?limit=100&offset=0 duration={}ms", "ingestionTime": {}}}"#,
                1705312200000i64 + i * 1000,
                i,
                1000 + i,
                50 + i * 10,
                1705312200000i64 + i * 1000 + 100
            ));
        }
        let json = format!(
            r#"{{"events": [{}], "nextForwardToken": "f/abcdef1234567890abcdef1234567890abcdef1234567890", "nextBackwardToken": "b/abcdef1234567890abcdef1234567890abcdef1234567890"}}"#,
            events.join(",")
        );
        let result = filter_logs_events(&json).unwrap();
        let input_tokens = count_tokens(&json);
        let output_tokens = count_tokens(&result.text);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        // Logs savings come from stripping ingestionTime, pagination tokens, and JSON keys.
        // With realistic fixtures the savings are modest per-event but the pagination
        // tokens alone save ~20 tokens each.
        assert!(
            savings >= 15.0,
            "Logs filter: expected >=15% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_logs_events_invalid_json() {
        assert!(filter_logs_events("not json").is_none());
    }

    #[test]
    fn test_filter_cfn_events() {
        let json = r#"{
            "StackEvents": [
                {
                    "Timestamp": "2024-01-15T10:30:00Z",
                    "LogicalResourceId": "MyBucket",
                    "ResourceType": "AWS::S3::Bucket",
                    "ResourceStatus": "CREATE_FAILED",
                    "ResourceStatusReason": "Bucket already exists",
                    "ResourceProperties": "{\"BucketName\":\"my-bucket\",\"VersioningConfiguration\":{\"Status\":\"Enabled\"},\"Tags\":[{\"Key\":\"Env\",\"Value\":\"prod\"}]}"
                },
                {
                    "Timestamp": "2024-01-15T10:29:00Z",
                    "LogicalResourceId": "MyVpc",
                    "ResourceType": "AWS::EC2::VPC",
                    "ResourceStatus": "CREATE_COMPLETE",
                    "ResourceProperties": "{\"CidrBlock\":\"10.0.0.0/16\"}"
                },
                {
                    "Timestamp": "2024-01-15T10:28:00Z",
                    "LogicalResourceId": "MyStack",
                    "ResourceType": "AWS::CloudFormation::Stack",
                    "ResourceStatus": "ROLLBACK_IN_PROGRESS",
                    "ResourceStatusReason": "The following resource(s) failed to create: [MyBucket]"
                }
            ]
        }"#;
        let result = filter_cfn_events(json).unwrap();
        assert!(result.text.contains("3 events"));
        assert!(result.text.contains("2 failed"));
        assert!(result.text.contains("1 successful"));
        assert!(result.text.contains("FAILURES"));
        assert!(result.text.contains("MyBucket"));
        assert!(result.text.contains("Bucket already exists"));
        // ResourceProperties should NOT appear
        assert!(!result.text.contains("BucketName"));
        assert!(!result.text.contains("CidrBlock"));
        // AWS:: prefix stripped from resource type
        assert!(result.text.contains("S3::Bucket"));
        assert!(!result.text.contains("AWS::S3"));
    }

    #[test]
    fn test_filter_cfn_events_token_savings() {
        let json = r#"{
            "StackEvents": [
                {"Timestamp": "2024-01-15T10:30:00Z", "LogicalResourceId": "Res1", "ResourceType": "AWS::Lambda::Function", "ResourceStatus": "CREATE_FAILED", "ResourceStatusReason": "Error", "ResourceProperties": "{\"FunctionName\":\"my-fn\",\"Runtime\":\"python3.12\",\"Handler\":\"index.handler\",\"MemorySize\":512,\"Timeout\":30,\"Role\":\"arn:aws:iam::123:role/my-role\",\"Environment\":{\"Variables\":{\"TABLE\":\"my-table\"}}}"},
                {"Timestamp": "2024-01-15T10:29:00Z", "LogicalResourceId": "Res2", "ResourceType": "AWS::EC2::VPC", "ResourceStatus": "CREATE_COMPLETE", "ResourceProperties": "{\"CidrBlock\":\"10.0.0.0/16\",\"EnableDnsSupport\":true,\"EnableDnsHostnames\":true}"},
                {"Timestamp": "2024-01-15T10:28:00Z", "LogicalResourceId": "Res3", "ResourceType": "AWS::S3::Bucket", "ResourceStatus": "CREATE_COMPLETE", "ResourceProperties": "{\"BucketName\":\"my-bucket\",\"VersioningConfiguration\":{\"Status\":\"Enabled\"}}"}
            ]
        }"#;
        let result = filter_cfn_events(json).unwrap();
        let input_tokens = count_tokens(json);
        let output_tokens = count_tokens(&result.text);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        // Real CF deployments have 30+ events with huge ResourceProperties
        // (stringified JSON). Small fixture shows ~46% but real-world is 90%+.
        assert!(
            savings >= 40.0,
            "CFN events filter: expected >=40% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_lambda_list() {
        let json = r#"{
            "Functions": [
                {"FunctionName": "my-api", "Runtime": "python3.12", "MemorySize": 512, "Timeout": 30, "State": "Active", "Environment": {"Variables": {"SECRET_KEY": "s3cr3t", "DB_PASSWORD": "hunter2"}}},
                {"FunctionName": "my-worker", "Runtime": "nodejs20.x", "MemorySize": 256, "Timeout": 60, "State": "Active"}
            ]
        }"#;
        let result = filter_lambda_list(json).unwrap();
        assert!(result.text.contains("my-api python3.12 512MB 30s Active"));
        assert!(result
            .text
            .contains("my-worker nodejs20.x 256MB 60s Active"));
        // SECURITY: secrets must NOT appear
        assert!(!result.text.contains("SECRET_KEY"));
        assert!(!result.text.contains("s3cr3t"));
        assert!(!result.text.contains("DB_PASSWORD"));
        assert!(!result.text.contains("hunter2"));
        assert!(!result.truncated);
    }

    #[test]
    fn test_filter_lambda_list_token_savings() {
        let json = r#"{
            "Functions": [
                {"FunctionName": "fn-1", "FunctionArn": "arn:aws:lambda:us-east-1:123:function:fn-1", "Runtime": "python3.12", "Role": "arn:aws:iam::123:role/role-1", "Handler": "index.handler", "CodeSize": 5242880, "Description": "A function", "Timeout": 30, "MemorySize": 512, "LastModified": "2024-01-15T10:30:00.000+0000", "CodeSha256": "abc123def456", "Version": "$LATEST", "TracingConfig": {"Mode": "Active"}, "RevisionId": "rev-123", "State": "Active", "LastUpdateStatus": "Successful", "PackageType": "Zip", "Architectures": ["x86_64"], "EphemeralStorage": {"Size": 512}, "Environment": {"Variables": {"TABLE_NAME": "my-table", "API_KEY": "secret-api-key-12345"}}}
            ]
        }"#;
        let result = filter_lambda_list(json).unwrap();
        let input_tokens = count_tokens(json);
        let output_tokens = count_tokens(&result.text);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Lambda list filter: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_lambda_get() {
        let json = r#"{
            "Configuration": {
                "FunctionName": "my-api",
                "Runtime": "python3.12",
                "Handler": "app.handler",
                "MemorySize": 512,
                "Timeout": 30,
                "State": "Active",
                "LastModified": "2024-01-15T10:30:00.000+0000",
                "Environment": {"Variables": {"SECRET": "hunter2"}},
                "Layers": [
                    {"Arn": "arn:aws:lambda:us-east-1:123:layer:my-layer:5"},
                    {"Arn": "arn:aws:lambda:us-east-1:123:layer:common-utils:3"}
                ]
            },
            "Code": {"Location": "https://awslambda-us-east-1-tasks.s3.amazonaws.com/snapshots/123/my-func?versionId=abc&X-Amz-Security-Token=very-long-token"},
            "Tags": {"Team": "backend"}
        }"#;
        let result = filter_lambda_get(json).unwrap();
        assert!(result
            .text
            .contains("my-api python3.12 app.handler 512MB 30s Active 2024-01-15"));
        assert!(result.text.contains("layers: my-layer:5, common-utils:3"));
        // SECURITY
        assert!(!result.text.contains("SECRET"));
        assert!(!result.text.contains("hunter2"));
        assert!(!result.text.contains("awslambda"));
        assert!(!result.text.contains("X-Amz-Security-Token"));
    }

    #[test]
    fn test_filter_lambda_get_no_layers() {
        let json = r#"{
            "Configuration": {
                "FunctionName": "simple-fn",
                "Runtime": "nodejs20.x",
                "Handler": "index.handler",
                "MemorySize": 128,
                "Timeout": 10,
                "State": "Active",
                "LastModified": "2024-02-20T14:00:00.000+0000"
            },
            "Code": {"Location": "https://example.com/code"}
        }"#;
        let result = filter_lambda_get(json).unwrap();
        assert!(result.text.contains("simple-fn"));
        assert!(!result.text.contains("layers"));
    }

    #[test]
    fn test_filter_lambda_list_invalid_json() {
        assert!(filter_lambda_list("not json").is_none());
    }

    #[test]
    fn test_filter_cfn_events_invalid_json() {
        assert!(filter_cfn_events("not json").is_none());
    }

    // === P1 filter tests ===

    #[test]
    fn test_filter_iam_roles() {
        let json = r#"{
            "Roles": [
                {"RoleName": "admin-role", "CreateDate": "2024-01-15T10:30:00Z", "Description": "Admin access", "AssumeRolePolicyDocument": "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":{\"Service\":\"lambda.amazonaws.com\"},\"Action\":\"sts:AssumeRole\"}]}"},
                {"RoleName": "lambda-exec", "CreateDate": "2024-02-20T14:00:00Z", "AssumeRolePolicyDocument": "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":{\"Service\":\"lambda.amazonaws.com\"},\"Action\":\"sts:AssumeRole\"}]}"}
            ]
        }"#;
        let result = filter_iam_roles(json).unwrap();
        assert!(result
            .text
            .contains("admin-role 2024-01-15 [Admin access] assume:[lambda.amazonaws.com]"));
        assert!(result
            .text
            .contains("lambda-exec 2024-02-20 assume:[lambda.amazonaws.com]"));
        // Full policy JSON should NOT appear, only extracted principals
        assert!(!result.text.contains("Statement"));
        assert!(!result.text.contains("Version"));
    }

    #[test]
    fn test_filter_iam_roles_token_savings() {
        let json = r#"{
            "Roles": [
                {"RoleName": "role-1", "RoleId": "AROA1234567890", "Arn": "arn:aws:iam::123:role/role-1", "Path": "/", "CreateDate": "2024-01-15T10:30:00Z", "MaxSessionDuration": 3600, "Description": "Test role", "AssumeRolePolicyDocument": "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":{\"Service\":\"lambda.amazonaws.com\"},\"Action\":\"sts:AssumeRole\"}]}", "Tags": [{"Key": "Team", "Value": "backend"}]}
            ]
        }"#;
        let result = filter_iam_roles(json).unwrap();
        let input_tokens = count_tokens(json);
        let output_tokens = count_tokens(&result.text);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "IAM roles filter: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_iam_users() {
        let json = r#"{
            "Users": [
                {"UserName": "alice", "UserId": "AIDA1234", "Arn": "arn:aws:iam::123:user/alice", "Path": "/", "CreateDate": "2024-01-15T10:30:00Z"},
                {"UserName": "bob", "UserId": "AIDA5678", "Arn": "arn:aws:iam::123:user/bob", "Path": "/", "CreateDate": "2024-02-20T14:00:00Z"}
            ]
        }"#;
        let result = filter_iam_users(json).unwrap();
        assert!(result.text.contains("alice created:2024-01-15"));
        assert!(result.text.contains("bob created:2024-02-20"));
        assert!(!result.text.contains("AIDA"));
        assert!(!result.text.contains("arn:aws"));
    }

    #[test]
    fn test_filter_dynamodb_items() {
        let json = r#"{
            "Items": [
                {"id": {"S": "user-1"}, "name": {"S": "Alice"}, "age": {"N": "30"}, "active": {"BOOL": true}},
                {"id": {"S": "user-2"}, "name": {"S": "Bob"}, "scores": {"L": [{"N": "100"}, {"N": "95"}]}, "meta": {"M": {"role": {"S": "admin"}}}}
            ],
            "Count": 2,
            "ScannedCount": 100
        }"#;
        let result = filter_dynamodb_items(json).unwrap();
        assert!(result.text.contains("Count: 2/100"));
        // Type wrappers should be unwrapped
        assert!(result.text.contains("\"Alice\""));
        assert!(result.text.contains("\"Bob\""));
        assert!(!result.text.contains(r#""S""#));
        assert!(!result.text.contains(r#""N""#));
        assert!(!result.text.contains(r#""BOOL""#));
        // Nested types should be unwrapped too
        assert!(result.text.contains("\"admin\""));
    }

    #[test]
    fn test_filter_dynamodb_token_savings() {
        let json = r#"{
            "Items": [
                {"pk": {"S": "USER#1"}, "sk": {"S": "PROFILE"}, "name": {"S": "Alice"}, "email": {"S": "alice@example.com"}, "age": {"N": "30"}, "active": {"BOOL": true}, "tags": {"SS": ["admin", "user"]}, "meta": {"M": {"role": {"S": "admin"}, "team": {"S": "backend"}}}, "scores": {"L": [{"N": "100"}, {"N": "95"}, {"N": "88"}]}},
                {"pk": {"S": "USER#2"}, "sk": {"S": "PROFILE"}, "name": {"S": "Bob"}, "email": {"S": "bob@example.com"}, "age": {"N": "25"}, "active": {"BOOL": false}, "tags": {"SS": ["user"]}, "meta": {"M": {"role": {"S": "viewer"}, "team": {"S": "frontend"}}}, "scores": {"L": [{"N": "80"}, {"N": "75"}]}}
            ],
            "Count": 2,
            "ScannedCount": 2
        }"#;
        let result = filter_dynamodb_items(json).unwrap();
        let input_tokens = count_tokens(json);
        let output_tokens = count_tokens(&result.text);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 30.0,
            "DynamoDB filter: expected >=30% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_dynamodb_null_type() {
        let json = r#"{
            "Items": [{"id": {"S": "1"}, "deleted_at": {"NULL": true}}],
            "Count": 1,
            "ScannedCount": 1
        }"#;
        let result = filter_dynamodb_items(json).unwrap();
        assert!(result.text.contains("null"));
        assert!(!result.text.contains("NULL"));
    }

    #[test]
    fn test_filter_ecs_tasks() {
        let json = r#"{
            "tasks": [
                {
                    "taskArn": "arn:aws:ecs:us-east-1:123:task/my-cluster/abc123def456",
                    "lastStatus": "RUNNING",
                    "desiredStatus": "RUNNING",
                    "containers": [
                        {"name": "web", "lastStatus": "RUNNING"},
                        {"name": "sidecar", "lastStatus": "RUNNING"}
                    ],
                    "attachments": [{"id": "eni-123", "type": "ElasticNetworkInterface", "status": "ATTACHED", "details": []}],
                    "overrides": {"containerOverrides": []}
                },
                {
                    "taskArn": "arn:aws:ecs:us-east-1:123:task/my-cluster/def789ghi012",
                    "lastStatus": "STOPPED",
                    "stoppedReason": "Essential container in task exited",
                    "containers": [
                        {"name": "worker", "lastStatus": "STOPPED", "exitCode": 1}
                    ],
                    "attachments": [],
                    "overrides": {}
                }
            ]
        }"#;
        let result = filter_ecs_tasks(json).unwrap();
        assert!(result
            .text
            .contains("abc123def456 RUNNING containers:[web:RUNNING, sidecar:RUNNING]"));
        assert!(result
            .text
            .contains("def789ghi012 STOPPED containers:[worker:STOPPED(exit:1)]"));
        assert!(result
            .text
            .contains("reason:Essential container in task exited"));
        // Attachments and overrides should NOT appear
        assert!(!result.text.contains("ElasticNetworkInterface"));
        assert!(!result.text.contains("containerOverrides"));
    }

    #[test]
    fn test_filter_iam_roles_invalid_json() {
        assert!(filter_iam_roles("not json").is_none());
    }

    #[test]
    fn test_filter_dynamodb_invalid_json() {
        assert!(filter_dynamodb_items("not json").is_none());
    }

    #[test]
    fn test_filter_ecs_tasks_invalid_json() {
        assert!(filter_ecs_tasks("not json").is_none());
    }

    // === P2 filter tests ===

    #[test]
    fn test_filter_security_groups() {
        let json = r#"{
            "SecurityGroups": [{
                "GroupName": "web-sg",
                "GroupId": "sg-001",
                "IpPermissions": [
                    {"IpProtocol": "tcp", "FromPort": 443, "ToPort": 443, "IpRanges": [{"CidrIp": "0.0.0.0/0"}], "Ipv6Ranges": [], "UserIdGroupPairs": []},
                    {"IpProtocol": "tcp", "FromPort": 22, "ToPort": 22, "IpRanges": [{"CidrIp": "10.0.0.0/8"}], "Ipv6Ranges": [], "UserIdGroupPairs": []}
                ],
                "IpPermissionsEgress": [
                    {"IpProtocol": "-1", "IpRanges": [{"CidrIp": "0.0.0.0/0"}], "Ipv6Ranges": [], "UserIdGroupPairs": []}
                ]
            }]
        }"#;
        let result = filter_security_groups(json).unwrap();
        assert!(result.text.contains("web-sg (sg-001)"));
        assert!(result.text.contains("tcp/443<-0.0.0.0/0"));
        assert!(result.text.contains("tcp/22<-10.0.0.0/8"));
        assert!(result.text.contains("all<-0.0.0.0/0"));
    }

    #[test]
    fn test_filter_security_groups_token_savings() {
        let json = r#"{
            "SecurityGroups": [{
                "GroupName": "web-sg", "GroupId": "sg-001", "Description": "Web server security group", "VpcId": "vpc-001", "OwnerId": "123456789012",
                "IpPermissions": [
                    {"IpProtocol": "tcp", "FromPort": 443, "ToPort": 443, "IpRanges": [{"CidrIp": "0.0.0.0/0", "Description": "HTTPS from anywhere"}], "Ipv6Ranges": [{"CidrIpv6": "::/0", "Description": "HTTPS IPv6"}], "PrefixListIds": [], "UserIdGroupPairs": []},
                    {"IpProtocol": "tcp", "FromPort": 80, "ToPort": 80, "IpRanges": [{"CidrIp": "0.0.0.0/0", "Description": "HTTP from anywhere"}], "Ipv6Ranges": [], "PrefixListIds": [], "UserIdGroupPairs": []}
                ],
                "IpPermissionsEgress": [{"IpProtocol": "-1", "IpRanges": [{"CidrIp": "0.0.0.0/0"}], "Ipv6Ranges": [{"CidrIpv6": "::/0"}], "PrefixListIds": [], "UserIdGroupPairs": []}],
                "Tags": [{"Key": "Name", "Value": "web-sg"}, {"Key": "Environment", "Value": "production"}]
            }]
        }"#;
        let result = filter_security_groups(json).unwrap();
        let input_tokens = count_tokens(json);
        let output_tokens = count_tokens(&result.text);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "SG filter: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_s3_objects() {
        let json = r#"{
            "Contents": [
                {"Key": "data/users.csv", "Size": 5242880, "LastModified": "2024-01-15T10:30:00Z", "ETag": "\"abc123\"", "StorageClass": "STANDARD"},
                {"Key": "logs/app.log", "Size": 1024, "LastModified": "2024-02-20T14:00:00Z", "ETag": "\"def456\"", "StorageClass": "STANDARD"}
            ]
        }"#;
        let result = filter_s3_objects(json).unwrap();
        assert!(result.text.contains("data/users.csv 5.0 MB 2024-01-15"));
        assert!(result.text.contains("logs/app.log 1.0 KB 2024-02-20"));
        // ETag and StorageClass should NOT appear
        assert!(!result.text.contains("abc123"));
        assert!(!result.text.contains("STANDARD"));
    }

    #[test]
    fn test_filter_eks_cluster() {
        let json = r#"{
            "cluster": {
                "name": "my-cluster",
                "status": "ACTIVE",
                "version": "1.28",
                "endpoint": "https://ABC123.gr7.us-east-1.eks.amazonaws.com",
                "certificateAuthority": {"data": "LS0tLS1CRUdJTiBDRVJUSUZJQ0FURS0tLS0tCk1JSUN5RENDQWJDZ0F3SUJBZ0lCQURBTkJna3Foa2lHOXcwQkFRc0ZBREFWTVJNd0VRWURWUVFERXdwcmRXSmwKY21...VERY_LONG_BASE64_CERT_DATA"},
                "logging": {"clusterLogging": [{"types": ["api","audit","authenticator","controllerManager","scheduler"], "enabled": true}]},
                "platformVersion": "eks.5"
            }
        }"#;
        let result = filter_eks_cluster(json).unwrap();
        assert!(result
            .text
            .contains("my-cluster ACTIVE k8s/1.28 https://ABC123.gr7.us-east-1.eks.amazonaws.com"));
        // certificateAuthority should NOT appear
        assert!(!result.text.contains("LS0tLS1CRUdJTi"));
        assert!(!result.text.contains("VERY_LONG"));
    }

    #[test]
    fn test_filter_sqs_messages() {
        let json = r#"{
            "Messages": [
                {
                    "MessageId": "12345678-abcd-efgh-ijkl-1234567890ab",
                    "ReceiptHandle": "AQEBwJnKyrHigUMZj6rYigCgxlaS3SLy0a...VERY_LONG_RECEIPT_HANDLE_200_CHARS_OF_OPAQUE_GARBAGE_THAT_NOBODY_NEEDS",
                    "MD5OfBody": "abc123",
                    "Body": "{\"orderId\": 42, \"status\": \"pending\"}"
                }
            ]
        }"#;
        let result = filter_sqs_messages(json).unwrap();
        assert!(result.text.contains("12345678"));
        assert!(result.text.contains("orderId"));
        // ReceiptHandle should NOT appear
        assert!(!result.text.contains("AQEBwJnK"));
        assert!(!result.text.contains("OPAQUE_GARBAGE"));
        assert!(!result.text.contains("MD5OfBody"));
    }

    #[test]
    fn test_filter_security_groups_invalid_json() {
        assert!(filter_security_groups("not json").is_none());
    }

    #[test]
    fn test_filter_s3_objects_invalid_json() {
        assert!(filter_s3_objects("not json").is_none());
    }

    #[test]
    fn test_filter_eks_cluster_invalid_json() {
        assert!(filter_eks_cluster("not json").is_none());
    }

    #[test]
    fn test_filter_sqs_messages_invalid_json() {
        assert!(filter_sqs_messages("not json").is_none());
    }

    #[test]
    fn test_filter_dynamodb_get_item() {
        let json = r#"{
            "Item": {
                "id": {"N": "123"},
                "name": {"S": "test-item"},
                "price": {"N": "19.99"},
                "tags": {"L": [{"S": "new"}, {"S": "sale"}]},
                "metadata": {"M": {"key": {"S": "value"}}}
            },
            "ConsumedCapacity": {
                "CapacityUnits": 1.0
            }
        }"#;
        let result = filter_dynamodb_get_item(json).unwrap();
        assert!(result.text.contains(r#""id":123"#));
        assert!(result.text.contains(r#""name":"test-item""#));
        assert!(result.text.contains("Capacity: 1 RCU"));
    }

    #[test]
    fn test_filter_dynamodb_get_item_no_item() {
        let json = r#"{}"#;
        assert!(filter_dynamodb_get_item(json).is_none());
    }

    #[test]
    fn test_filter_dynamodb_get_item_invalid_json() {
        assert!(filter_dynamodb_get_item("not json").is_none());
    }

    #[test]
    fn test_filter_logs_query_results() {
        let json = r#"{
            "status": "Complete",
            "results": [
                [
                    {"field": "@timestamp", "value": "2024-01-01 12:00:00"},
                    {"field": "@message", "value": "Error occurred"},
                    {"field": "@ptr", "value": "internal-pointer"}
                ],
                [
                    {"field": "@timestamp", "value": "2024-01-01 12:01:00"},
                    {"field": "@message", "value": "Another error"}
                ]
            ]
        }"#;
        let result = filter_logs_query_results(json).unwrap();
        assert!(result.text.contains("Status: Complete"));
        assert!(result.text.contains("@timestamp=2024-01-01 12:00:00"));
        assert!(result.text.contains("@message=Error occurred"));
        assert!(!result.text.contains("@ptr")); // Should be filtered out
    }

    #[test]
    fn test_filter_logs_query_results_empty() {
        let json = r#"{"status": "Complete", "results": []}"#;
        let result = filter_logs_query_results(json).unwrap();
        assert_eq!(result.text, "Status: Complete");
    }

    #[test]
    fn test_filter_logs_query_results_invalid_json() {
        assert!(filter_logs_query_results("not json").is_none());
    }

    #[test]
    fn test_filter_s3_transfer_short_output() {
        let output = "upload: file1.txt to s3://bucket/file1.txt\n";
        let result = filter_s3_transfer(output);
        // Short output passes through unchanged
        assert_eq!(result.text, output);
    }

    #[test]
    fn test_filter_s3_transfer_with_operations() {
        let output = "\
upload: file1.txt to s3://bucket/file1.txt
upload: file2.txt to s3://bucket/file2.txt
download: s3://bucket/file3.txt to file3.txt
delete: s3://bucket/old.txt
upload: file4.txt to s3://bucket/file4.txt
upload: file5.txt to s3://bucket/file5.txt
download: s3://bucket/file6.txt to file6.txt
copy: s3://bucket/a.txt to s3://bucket/b.txt
error: failed to upload file7.txt
upload: file8.txt to s3://bucket/file8.txt
upload: file9.txt to s3://bucket/file9.txt
upload: file10.txt to s3://bucket/file10.txt
";
        let result = filter_s3_transfer(output);
        assert!(result.text.contains("7 uploaded"));
        assert!(result.text.contains("2 downloaded"));
        assert!(result.text.contains("1 deleted"));
        assert!(result.text.contains("1 copied"));
        assert!(result.text.contains("1 errors"));
        assert!(result.text.contains("error: failed to upload file7.txt"));
    }

    #[test]
    fn test_filter_secrets_get() {
        let json = r#"{
            "Name": "my-secret",
            "SecretString": "{\"username\":\"admin\",\"password\":\"secret123\"}",
            "ARN": "arn:aws:secretsmanager:us-east-1:123456789012:secret:my-secret-AbCdEf",
            "VersionId": "version-uuid",
            "CreatedDate": "2024-01-01T00:00:00Z"
        }"#;
        let result = filter_secrets_get(json).unwrap();
        assert!(result.text.contains("Name: my-secret"));
        assert!(result
            .text
            .contains(r#"{"username":"admin","password":"secret123"}"#));
        assert!(!result.text.contains("ARN"));
        assert!(!result.text.contains("VersionId"));
    }

    #[test]
    fn test_filter_secrets_get_plain_text() {
        let json = r#"{
            "Name": "my-secret",
            "SecretString": "plain-text-password"
        }"#;
        let result = filter_secrets_get(json).unwrap();
        assert!(result.text.contains("Name: my-secret"));
        assert!(result.text.contains("Secret: plain-text-password"));
    }

    #[test]
    fn test_filter_secrets_get_invalid_json() {
        assert!(filter_secrets_get("not json").is_none());
    }

    #[test]
    fn test_dynamodb_n_type_parsing() {
        // Test i64
        let json = r#"{"N": "123"}"#;
        let val: Value = serde_json::from_str(json).unwrap();
        let result = unwrap_dynamodb_value(&val, 0);
        assert_eq!(result, Value::Number(123.into()));

        // Test f64
        let json = r#"{"N": "123.45"}"#;
        let val: Value = serde_json::from_str(json).unwrap();
        let result = unwrap_dynamodb_value(&val, 0);
        assert!(result.is_number());
    }

    #[test]
    fn test_dynamodb_ns_type_parsing() {
        // Test NS with integers and floats
        let json = r#"{"NS": ["123", "456", "78.9"]}"#;
        let val: Value = serde_json::from_str(json).unwrap();
        let result = unwrap_dynamodb_value(&val, 0);
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0], Value::Number(123.into()));
        assert_eq!(arr[1], Value::Number(456.into()));
        assert!(arr[2].is_number());
    }

    #[test]
    fn test_filter_dynamodb_items_with_capacity() {
        let json = r#"{
            "Items": [
                {"id": {"N": "1"}, "name": {"S": "item1"}}
            ],
            "Count": 1,
            "ScannedCount": 1,
            "ConsumedCapacity": {
                "CapacityUnits": 2.5
            }
        }"#;
        let result = filter_dynamodb_items(json).unwrap();
        assert!(result.text.contains("Count: 1/1"));
        assert!(result.text.contains("Capacity: 2.5 RCU"));
    }

    #[test]
    fn test_filter_dynamodb_items_with_pagination() {
        let json = r#"{
            "Items": [
                {"id": {"N": "1"}, "name": {"S": "item1"}}
            ],
            "Count": 1,
            "ScannedCount": 1,
            "LastEvaluatedKey": {
                "id": {"N": "1"}
            }
        }"#;
        let result = filter_dynamodb_items(json).unwrap();
        assert!(result.text.contains("Count: 1/1"));
        assert!(result.text.contains("(paginated — more results available)"));
    }

    // === Snapshot-style tests: verify full output format ===

    #[test]
    fn test_snapshot_logs_events_format() {
        let json = r#"{
            "events": [
                {"timestamp": 1705312200000, "message": "INFO: server started\n", "ingestionTime": 1705312201000},
                {"timestamp": 1705312260000, "message": "ERROR: connection lost\n", "ingestionTime": 1705312261000}
            ],
            "nextForwardToken": "f/token123"
        }"#;
        let result = filter_logs_events(json).unwrap();
        assert_eq!(
            result.text,
            "2024-01-15 09:50:00 INFO: server started\n2024-01-15 09:51:00 ERROR: connection lost"
        );
    }

    #[test]
    fn test_snapshot_lambda_list_format() {
        let json = r#"{"Functions": [
            {"FunctionName": "api", "Runtime": "python3.12", "MemorySize": 512, "Timeout": 30, "State": "Active"}
        ]}"#;
        let result = filter_lambda_list(json).unwrap();
        assert_eq!(result.text, "api python3.12 512MB 30s Active");
    }

    #[test]
    fn test_snapshot_dynamodb_scan_format() {
        let json = r#"{"Items": [{"id": {"N": "1"}, "name": {"S": "Alice"}}], "Count": 1, "ScannedCount": 1}"#;
        let result = filter_dynamodb_items(json).unwrap();
        assert_eq!(result.text, "Count: 1/1\n{\"id\":1,\"name\":\"Alice\"}");
    }

    #[test]
    fn test_snapshot_security_groups_format() {
        let json = r#"{"SecurityGroups": [{
            "GroupName": "web", "GroupId": "sg-1",
            "IpPermissions": [{"IpProtocol": "tcp", "FromPort": 443, "ToPort": 443, "IpRanges": [{"CidrIp": "0.0.0.0/0"}], "Ipv6Ranges": [], "UserIdGroupPairs": []}],
            "IpPermissionsEgress": [{"IpProtocol": "-1", "IpRanges": [{"CidrIp": "0.0.0.0/0"}], "Ipv6Ranges": [], "UserIdGroupPairs": []}]
        }]}"#;
        let result = filter_security_groups(json).unwrap();
        assert_eq!(
            result.text,
            "web (sg-1) ingress: tcp/443<-0.0.0.0/0 | egress: all<-0.0.0.0/0"
        );
    }

    #[test]
    fn test_snapshot_cfn_events_format() {
        let json = r#"{"StackEvents": [
            {"Timestamp": "2024-01-15T10:30:00Z", "LogicalResourceId": "Bucket", "ResourceType": "AWS::S3::Bucket", "ResourceStatus": "CREATE_FAILED", "ResourceStatusReason": "Already exists"},
            {"Timestamp": "2024-01-15T10:29:00Z", "LogicalResourceId": "VPC", "ResourceType": "AWS::EC2::VPC", "ResourceStatus": "CREATE_COMPLETE"}
        ]}"#;
        let result = filter_cfn_events(json).unwrap();
        assert!(result
            .text
            .starts_with("CloudFormation: 2 events (1 failed, 1 successful)"));
        assert!(result.text.contains("--- FAILURES ---"));
        assert!(result
            .text
            .contains("Bucket S3::Bucket CREATE_FAILED REASON: Already exists"));
    }

    // === Empty collection edge cases ===

    #[test]
    fn test_filter_lambda_list_empty() {
        let json = r#"{"Functions": []}"#;
        let result = filter_lambda_list(json).unwrap();
        assert_eq!(result.text, "");
    }

    #[test]
    fn test_filter_iam_roles_empty() {
        let json = r#"{"Roles": []}"#;
        let result = filter_iam_roles(json).unwrap();
        assert_eq!(result.text, "");
    }

    #[test]
    fn test_filter_iam_users_empty() {
        let json = r#"{"Users": []}"#;
        let result = filter_iam_users(json).unwrap();
        assert_eq!(result.text, "");
    }

    #[test]
    fn test_filter_dynamodb_items_empty() {
        let json = r#"{"Items": [], "Count": 0, "ScannedCount": 0}"#;
        let result = filter_dynamodb_items(json).unwrap();
        assert_eq!(result.text, "Count: 0/0");
    }

    #[test]
    fn test_filter_ecs_tasks_empty() {
        let json = r#"{"tasks": []}"#;
        let result = filter_ecs_tasks(json).unwrap();
        assert_eq!(result.text, "");
    }

    #[test]
    fn test_filter_security_groups_empty() {
        let json = r#"{"SecurityGroups": []}"#;
        let result = filter_security_groups(json).unwrap();
        assert_eq!(result.text, "");
    }

    #[test]
    fn test_filter_s3_objects_empty() {
        let json = r#"{}"#;
        let result = filter_s3_objects(json).unwrap();
        assert_eq!(result.text, "");
    }

    #[test]
    fn test_filter_sqs_messages_empty() {
        let json = r#"{}"#;
        let result = filter_sqs_messages(json).unwrap();
        assert_eq!(result.text, "");
    }

    #[test]
    fn test_filter_logs_events_empty() {
        let json = r#"{"events": []}"#;
        let result = filter_logs_events(json).unwrap();
        assert_eq!(result.text, "");
    }

    #[test]
    fn test_filter_ec2_instances_empty() {
        let json = r#"{"Reservations": []}"#;
        let result = filter_ec2_instances(json).unwrap();
        assert_eq!(result.text, "EC2: 0 instances");
    }

    #[test]
    fn test_filter_cfn_events_empty() {
        let json = r#"{"StackEvents": []}"#;
        let result = filter_cfn_events(json).unwrap();
        assert_eq!(
            result.text,
            "CloudFormation: 0 events (0 failed, 0 successful)"
        );
    }

    #[test]
    fn test_filter_cfn_events_failure_count_exceeds_max_items() {
        // Verify that failed_count reports the real count, not the capped collection size
        let mut events = Vec::new();
        for i in 0..30 {
            events.push(format!(
                r#"{{"Timestamp": "2024-01-15T10:30:00Z", "LogicalResourceId": "Res{}", "ResourceType": "AWS::Lambda::Function", "ResourceStatus": "CREATE_FAILED", "ResourceStatusReason": "Error {}", "ResourceProperties": "{{}}"}}"#,
                i, i
            ));
        }
        let json = format!(r#"{{"StackEvents": [{}]}}"#, events.join(","));
        let result = filter_cfn_events(&json).unwrap();
        // Should report all 30 failures, not capped at MAX_ITEMS (20)
        assert!(result.text.contains("30 failed"));
    }
}
