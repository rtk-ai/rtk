//! AWS CLI output compression.
//!
//! Replaces verbose `--output table`/`text` with JSON, then compresses.
//! Specialized filters for high-frequency commands (STS, S3, EC2, ECS, RDS, CloudFormation).

use crate::json_cmd;
use crate::tracking;
use crate::utils::{join_with_overflow, resolved_command, truncate_iso_date};
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use serde_json::Value;

lazy_static! {
    static ref TIMESTAMP_RE: Regex = Regex::new(r"\d{4}-\d{2}-\d{2}T(\d{2}:\d{2}:\d{2})").unwrap();
    static ref S3_OP_RE: Regex = Regex::new(r"^(upload|download|delete|copy|move):").unwrap();
}

const MAX_ITEMS: usize = 20;
const JSON_COMPRESS_DEPTH: usize = 4;

/// Run an AWS CLI command with token-optimized output
pub fn run(subcommand: &str, args: &[String], verbose: u8) -> Result<()> {
    // Build the full sub-path: e.g. "sts" + ["get-caller-identity"] -> "sts get-caller-identity"
    let full_sub = if args.is_empty() {
        subcommand.to_string()
    } else {
        format!("{} {}", subcommand, args.join(" "))
    };

    // Route to specialized handlers
    match subcommand {
        "sts" if !args.is_empty() && args[0] == "get-caller-identity" => {
            run_sts_identity(&args[1..], verbose)
        }
        "s3" if !args.is_empty() && args[0] == "ls" => run_s3_ls(&args[1..], verbose),
        "ec2" if !args.is_empty() && args[0] == "describe-instances" => {
            run_ec2_describe(&args[1..], verbose)
        }
        "ecs" if !args.is_empty() && args[0] == "list-services" => {
            run_ecs_list_services(&args[1..], verbose)
        }
        "ecs" if !args.is_empty() && args[0] == "describe-services" => {
            run_ecs_describe_services(&args[1..], verbose)
        }
        "rds" if !args.is_empty() && args[0] == "describe-db-instances" => {
            run_rds_describe(&args[1..], verbose)
        }
        "cloudformation" if !args.is_empty() && args[0] == "list-stacks" => {
            run_cfn_list_stacks(&args[1..], verbose)
        }
        "cloudformation" if !args.is_empty() && args[0] == "describe-stacks" => {
            run_cfn_describe_stacks(&args[1..], verbose)
        }
        // DynamoDB
        "dynamodb" if !args.is_empty() && args[0] == "scan" => {
            run_dynamodb_read("scan", &args[1..], verbose)
        }
        "dynamodb" if !args.is_empty() && args[0] == "query" => {
            run_dynamodb_read("query", &args[1..], verbose)
        }
        "dynamodb" if !args.is_empty() && args[0] == "get-item" => {
            run_dynamodb_get_item(&args[1..], verbose)
        }
        // CloudWatch Logs
        "logs" if !args.is_empty() && args[0] == "filter-log-events" => {
            run_logs_filter_events(&args[1..], verbose)
        }
        "logs" if !args.is_empty() && args[0] == "get-query-results" => {
            run_logs_query_results(&args[1..], verbose)
        }
        // S3 transfer operations
        "s3" if !args.is_empty() && (args[0] == "sync" || args[0] == "cp") => {
            run_s3_transfer(&args[0], &args[1..], verbose)
        }
        // Secrets Manager
        "secretsmanager" if !args.is_empty() && args[0] == "get-secret-value" => {
            run_secrets_get(&args[1..], verbose)
        }
        _ => run_generic(subcommand, args, verbose, &full_sub),
    }
}

/// Returns true for operations that return structured JSON (describe-*, list-*, get-*).
/// Mutating/transfer operations (s3 cp, s3 sync, s3 mb, etc.) emit plain text progress
/// and do not accept --output json, so we must not inject it for them.
fn is_structured_operation(args: &[String]) -> bool {
    let op = args.first().map(|s| s.as_str()).unwrap_or("");
    op.starts_with("describe-") || op.starts_with("list-") || op.starts_with("get-")
}

/// Generic strategy: force --output json for structured ops, compress via json_cmd schema
fn run_generic(subcommand: &str, args: &[String], verbose: u8, full_sub: &str) -> Result<()> {
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
        std::process::exit(output.status.code().unwrap_or(1));
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

    Ok(())
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

fn run_sts_identity(extra_args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let (raw, stderr, status) = run_aws_json(&["sts", "get-caller-identity"], extra_args, verbose)?;

    if !status.success() {
        timer.track(
            "aws sts get-caller-identity",
            "rtk aws sts get-caller-identity",
            &stderr,
            &stderr,
        );
        std::process::exit(status.code().unwrap_or(1));
    }

    let filtered = match filter_sts_identity(&raw) {
        Some(f) => f,
        None => raw.clone(),
    };
    println!("{}", filtered);

    timer.track(
        "aws sts get-caller-identity",
        "rtk aws sts get-caller-identity",
        &raw,
        &filtered,
    );
    Ok(())
}

fn run_s3_ls(extra_args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    // s3 ls doesn't support --output json, run as-is and filter text
    let mut cmd = resolved_command("aws");
    cmd.args(["s3", "ls"]);
    for arg in extra_args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: aws s3 ls {}", extra_args.join(" "));
    }

    let output = cmd.output().context("Failed to run aws s3 ls")?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        timer.track("aws s3 ls", "rtk aws s3 ls", &stderr, &stderr);
        eprintln!("{}", stderr.trim());
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let filtered = filter_s3_ls(&raw);
    println!("{}", filtered);

    timer.track("aws s3 ls", "rtk aws s3 ls", &raw, &filtered);
    Ok(())
}

fn run_ec2_describe(extra_args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let (raw, stderr, status) = run_aws_json(&["ec2", "describe-instances"], extra_args, verbose)?;

    if !status.success() {
        timer.track(
            "aws ec2 describe-instances",
            "rtk aws ec2 describe-instances",
            &stderr,
            &stderr,
        );
        std::process::exit(status.code().unwrap_or(1));
    }

    let filtered = match filter_ec2_instances(&raw) {
        Some(f) => f,
        None => raw.clone(),
    };
    println!("{}", filtered);

    timer.track(
        "aws ec2 describe-instances",
        "rtk aws ec2 describe-instances",
        &raw,
        &filtered,
    );
    Ok(())
}

fn run_ecs_list_services(extra_args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let (raw, stderr, status) = run_aws_json(&["ecs", "list-services"], extra_args, verbose)?;

    if !status.success() {
        timer.track(
            "aws ecs list-services",
            "rtk aws ecs list-services",
            &stderr,
            &stderr,
        );
        std::process::exit(status.code().unwrap_or(1));
    }

    let filtered = match filter_ecs_list_services(&raw) {
        Some(f) => f,
        None => raw.clone(),
    };
    println!("{}", filtered);

    timer.track(
        "aws ecs list-services",
        "rtk aws ecs list-services",
        &raw,
        &filtered,
    );
    Ok(())
}

fn run_ecs_describe_services(extra_args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let (raw, stderr, status) = run_aws_json(&["ecs", "describe-services"], extra_args, verbose)?;

    if !status.success() {
        timer.track(
            "aws ecs describe-services",
            "rtk aws ecs describe-services",
            &stderr,
            &stderr,
        );
        std::process::exit(status.code().unwrap_or(1));
    }

    let filtered = match filter_ecs_describe_services(&raw) {
        Some(f) => f,
        None => raw.clone(),
    };
    println!("{}", filtered);

    timer.track(
        "aws ecs describe-services",
        "rtk aws ecs describe-services",
        &raw,
        &filtered,
    );
    Ok(())
}

fn run_rds_describe(extra_args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let (raw, stderr, status) =
        run_aws_json(&["rds", "describe-db-instances"], extra_args, verbose)?;

    if !status.success() {
        timer.track(
            "aws rds describe-db-instances",
            "rtk aws rds describe-db-instances",
            &stderr,
            &stderr,
        );
        std::process::exit(status.code().unwrap_or(1));
    }

    let filtered = match filter_rds_instances(&raw) {
        Some(f) => f,
        None => raw.clone(),
    };
    println!("{}", filtered);

    timer.track(
        "aws rds describe-db-instances",
        "rtk aws rds describe-db-instances",
        &raw,
        &filtered,
    );
    Ok(())
}

fn run_cfn_list_stacks(extra_args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let (raw, stderr, status) =
        run_aws_json(&["cloudformation", "list-stacks"], extra_args, verbose)?;

    if !status.success() {
        timer.track(
            "aws cloudformation list-stacks",
            "rtk aws cloudformation list-stacks",
            &stderr,
            &stderr,
        );
        std::process::exit(status.code().unwrap_or(1));
    }

    let filtered = match filter_cfn_list_stacks(&raw) {
        Some(f) => f,
        None => raw.clone(),
    };
    println!("{}", filtered);

    timer.track(
        "aws cloudformation list-stacks",
        "rtk aws cloudformation list-stacks",
        &raw,
        &filtered,
    );
    Ok(())
}

fn run_cfn_describe_stacks(extra_args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let (raw, stderr, status) =
        run_aws_json(&["cloudformation", "describe-stacks"], extra_args, verbose)?;

    if !status.success() {
        timer.track(
            "aws cloudformation describe-stacks",
            "rtk aws cloudformation describe-stacks",
            &stderr,
            &stderr,
        );
        std::process::exit(status.code().unwrap_or(1));
    }

    let filtered = match filter_cfn_describe_stacks(&raw) {
        Some(f) => f,
        None => raw.clone(),
    };
    println!("{}", filtered);

    timer.track(
        "aws cloudformation describe-stacks",
        "rtk aws cloudformation describe-stacks",
        &raw,
        &filtered,
    );
    Ok(())
}

// --- Filter functions (all use serde_json::Value for resilience) ---

fn filter_sts_identity(json_str: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let account = v["Account"].as_str().unwrap_or("?");
    let arn = v["Arn"].as_str().unwrap_or("?");
    Some(format!("AWS: {} {}", account, arn))
}

fn filter_s3_ls(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let total = lines.len();
    let mut result: Vec<&str> = lines.iter().take(MAX_ITEMS + 10).copied().collect();

    if total > MAX_ITEMS + 10 {
        result.truncate(MAX_ITEMS + 10);
        result.push(""); // will be replaced
        return format!(
            "{}\n... +{} more items",
            result[..result.len() - 1].join("\n"),
            total - MAX_ITEMS - 10
        );
    }

    result.join("\n")
}

fn filter_ec2_instances(json_str: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let reservations = v["Reservations"].as_array()?;

    let mut instances: Vec<String> = Vec::new();
    for res in reservations {
        if let Some(insts) = res["Instances"].as_array() {
            for inst in insts {
                let id = inst["InstanceId"].as_str().unwrap_or("?");
                let state = inst["State"]["Name"].as_str().unwrap_or("?");
                let itype = inst["InstanceType"].as_str().unwrap_or("?");
                let ip = inst["PrivateIpAddress"].as_str().unwrap_or("-");

                // Extract Name tag
                let name = inst["Tags"]
                    .as_array()
                    .and_then(|tags| tags.iter().find(|t| t["Key"].as_str() == Some("Name")))
                    .and_then(|t| t["Value"].as_str())
                    .unwrap_or("-");

                instances.push(format!("{} {} {} {} ({})", id, state, itype, ip, name));
            }
        }
    }

    let total = instances.len();
    let mut result = format!("EC2: {} instances\n", total);

    for inst in instances.iter().take(MAX_ITEMS) {
        result.push_str(&format!("  {}\n", inst));
    }

    if total > MAX_ITEMS {
        result.push_str(&format!("  ... +{} more\n", total - MAX_ITEMS));
    }

    Some(result.trim_end().to_string())
}

fn filter_ecs_list_services(json_str: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let arns = v["serviceArns"].as_array()?;

    let mut result = Vec::new();
    let total = arns.len();

    for arn in arns.iter().take(MAX_ITEMS) {
        let arn_str = arn.as_str().unwrap_or("?");
        // Extract short name from ARN: arn:aws:ecs:...:service/cluster/name -> name
        let short = arn_str.rsplit('/').next().unwrap_or(arn_str);
        result.push(short.to_string());
    }

    Some(join_with_overflow(&result, total, MAX_ITEMS, "services"))
}

fn filter_ecs_describe_services(json_str: &str) -> Option<String> {
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

    Some(join_with_overflow(&result, total, MAX_ITEMS, "services"))
}

fn filter_rds_instances(json_str: &str) -> Option<String> {
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
        result.push(format!(
            "{} {} {} {} {}",
            name, engine, version, class, status
        ));
    }

    Some(join_with_overflow(&result, total, MAX_ITEMS, "instances"))
}

fn filter_cfn_list_stacks(json_str: &str) -> Option<String> {
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

    Some(join_with_overflow(&result, total, MAX_ITEMS, "stacks"))
}

fn filter_cfn_describe_stacks(json_str: &str) -> Option<String> {
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

        // Show outputs if present
        if let Some(outputs) = stack["Outputs"].as_array() {
            for out in outputs {
                let key = out["OutputKey"].as_str().unwrap_or("?");
                let val = out["OutputValue"].as_str().unwrap_or("?");
                result.push(format!("  {}={}", key, val));
            }
        }
    }

    Some(join_with_overflow(&result, total, MAX_ITEMS, "stacks"))
}

// --- DynamoDB type flattening ---

/// Recursively flatten DynamoDB typed wrappers into plain JSON values.
/// `{"S": "hello"}` → `"hello"`, `{"N": "42"}` → `42`, `{"BOOL": true}` → `true`,
/// `{"NULL": true}` → `null`, `{"L": [...]}` → `[...]`, `{"M": {...}}` → `{...}`,
/// `{"SS": [...]}` / `{"NS": [...]}` / `{"BS": [...]}` → `[...]`.
fn flatten_dynamodb_value(value: &Value) -> Value {
    match value {
        Value::Object(map) if map.len() == 1 => {
            let (key, inner) = map.iter().next().unwrap();
            match key.as_str() {
                "S" | "B" => inner.clone(),
                "N" => {
                    // Convert numeric string to JSON number
                    if let Some(s) = inner.as_str() {
                        if let Ok(n) = s.parse::<i64>() {
                            return Value::Number(n.into());
                        }
                        if let Ok(f) = s.parse::<f64>() {
                            if let Some(n) = serde_json::Number::from_f64(f) {
                                return Value::Number(n);
                            }
                        }
                        // Fallback: keep as string if unparseable
                        Value::String(s.to_string())
                    } else {
                        inner.clone()
                    }
                }
                "BOOL" => inner.clone(),
                "NULL" => Value::Null,
                "L" => {
                    if let Some(arr) = inner.as_array() {
                        Value::Array(arr.iter().map(flatten_dynamodb_value).collect())
                    } else {
                        inner.clone()
                    }
                }
                "M" => {
                    if let Some(obj) = inner.as_object() {
                        Value::Object(
                            obj.iter()
                                .map(|(k, v)| (k.clone(), flatten_dynamodb_value(v)))
                                .collect(),
                        )
                    } else {
                        inner.clone()
                    }
                }
                "SS" | "NS" | "BS" => {
                    if key == "NS" {
                        if let Some(arr) = inner.as_array() {
                            return Value::Array(
                                arr.iter()
                                    .map(|v| {
                                        if let Some(s) = v.as_str() {
                                            if let Ok(n) = s.parse::<i64>() {
                                                return Value::Number(n.into());
                                            }
                                            if let Ok(f) = s.parse::<f64>() {
                                                if let Some(n) = serde_json::Number::from_f64(f) {
                                                    return Value::Number(n);
                                                }
                                            }
                                        }
                                        v.clone()
                                    })
                                    .collect(),
                            );
                        }
                    }
                    inner.clone()
                }
                _ => {
                    // Not a DynamoDB type descriptor — recurse into the object
                    Value::Object(
                        map.iter()
                            .map(|(k, v)| (k.clone(), flatten_dynamodb_value(v)))
                            .collect(),
                    )
                }
            }
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), flatten_dynamodb_value(v)))
                .collect(),
        ),
        Value::Array(arr) => Value::Array(arr.iter().map(flatten_dynamodb_value).collect()),
        _ => value.clone(),
    }
}

fn run_dynamodb_read(op: &str, extra_args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let cmd_name = format!("dynamodb {}", op);
    let (raw, stderr, status) = run_aws_json(&["dynamodb", op], extra_args, verbose)?;

    if !status.success() {
        timer.track(
            &format!("aws {}", cmd_name),
            &format!("rtk aws {}", cmd_name),
            &stderr,
            &stderr,
        );
        std::process::exit(status.code().unwrap_or(1));
    }

    let filtered = match filter_dynamodb_scan_query(&raw, op) {
        Some(f) => f,
        None => raw.clone(),
    };
    println!("{}", filtered);

    timer.track(
        &format!("aws {}", cmd_name),
        &format!("rtk aws {}", cmd_name),
        &raw,
        &filtered,
    );
    Ok(())
}

fn filter_dynamodb_scan_query(json_str: &str, op: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let items = v.get("Items").and_then(|i| i.as_array())?;
    let count = v
        .get("Count")
        .and_then(|c| c.as_u64())
        .unwrap_or(items.len() as u64);
    let scanned = v
        .get("ScannedCount")
        .and_then(|c| c.as_u64())
        .unwrap_or(count);

    let flattened: Vec<Value> = items.iter().map(flatten_dynamodb_value).collect();

    let mut result = format!("DynamoDB {}: {} items, scanned: {}", op, count, scanned);

    // Preserve pagination token so LLMs know results are truncated
    if let Some(last_key) = v.get("LastEvaluatedKey") {
        let flat_key = flatten_dynamodb_value(last_key);
        result.push_str(&format!(
            ", LastEvaluatedKey: {}",
            serde_json::to_string(&flat_key).unwrap_or_default()
        ));
    }

    // Preserve capacity info for cost debugging
    if let Some(cap) = v.get("ConsumedCapacity") {
        if let Some(units) = cap.get("CapacityUnits").and_then(|u| u.as_f64()) {
            result.push_str(&format!(", capacity: {} RCU", units));
        }
    }

    result.push('\n');

    const MAX_DISPLAY: usize = 50;
    let display_items: Vec<&Value> = flattened.iter().take(MAX_DISPLAY).collect();
    let display_arr = Value::Array(display_items.into_iter().cloned().collect());
    result.push_str(&serde_json::to_string(&display_arr).unwrap_or_default());

    if flattened.len() > MAX_DISPLAY {
        result.push_str(&format!(
            "\n... +{} more items",
            flattened.len() - MAX_DISPLAY
        ));
    }

    Some(result)
}

fn run_dynamodb_get_item(extra_args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let (raw, stderr, status) = run_aws_json(&["dynamodb", "get-item"], extra_args, verbose)?;

    if !status.success() {
        timer.track(
            "aws dynamodb get-item",
            "rtk aws dynamodb get-item",
            &stderr,
            &stderr,
        );
        std::process::exit(status.code().unwrap_or(1));
    }

    let filtered = match filter_dynamodb_get_item(&raw) {
        Some(f) => f,
        None => raw.clone(),
    };
    println!("{}", filtered);

    timer.track(
        "aws dynamodb get-item",
        "rtk aws dynamodb get-item",
        &raw,
        &filtered,
    );
    Ok(())
}

fn filter_dynamodb_get_item(json_str: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let item = v.get("Item")?;
    let flattened = flatten_dynamodb_value(item);
    let mut result = String::from("DynamoDB item:\n");
    result.push_str(&serde_json::to_string(&flattened).unwrap_or_default());
    Some(result)
}

// --- CloudWatch Logs ---

fn run_logs_filter_events(extra_args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let (raw, stderr, status) = run_aws_json(&["logs", "filter-log-events"], extra_args, verbose)?;

    if !status.success() {
        timer.track(
            "aws logs filter-log-events",
            "rtk aws logs filter-log-events",
            &stderr,
            &stderr,
        );
        std::process::exit(status.code().unwrap_or(1));
    }

    let filtered = match filter_log_events(&raw) {
        Some(f) => f,
        None => raw.clone(),
    };
    println!("{}", filtered);

    timer.track(
        "aws logs filter-log-events",
        "rtk aws logs filter-log-events",
        &raw,
        &filtered,
    );
    Ok(())
}

fn filter_log_events(json_str: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let events = v.get("events").and_then(|e| e.as_array())?;

    if events.is_empty() {
        return Some("CloudWatch: 0 events".to_string());
    }

    let mut lines: Vec<String> = Vec::new();
    let mut last_msg: Option<String> = None;
    let mut dup_count: usize = 0;

    for event in events {
        let ts = event
            .get("timestamp")
            .and_then(|t| t.as_i64())
            .map(format_epoch_ms)
            .or_else(|| {
                event
                    .get("ingestionTime")
                    .and_then(|t| t.as_i64())
                    .map(format_epoch_ms)
            })
            .unwrap_or_else(|| "?".to_string());

        let msg = event
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .trim_end();

        let stream = event
            .get("logStreamName")
            .and_then(|s| s.as_str())
            .unwrap_or("");

        if let Some(ref last) = last_msg {
            if last == msg {
                dup_count += 1;
                continue;
            }
            if dup_count > 0 {
                lines.push(format!("  [x{}]", dup_count + 1));
                dup_count = 0;
            }
        }
        last_msg = Some(msg.to_string());
        if stream.is_empty() {
            lines.push(format!("{} {}", ts, msg));
        } else {
            lines.push(format!("{} [{}] {}", ts, stream, msg));
        }
    }

    // Flush trailing dups
    if dup_count > 0 {
        lines.push(format!("  [x{}]", dup_count + 1));
    }

    let header = format!("CloudWatch: {} events", events.len());
    Some(format!("{}\n{}", header, lines.join("\n")))
}

/// Format epoch milliseconds to MM-DD HH:MM:SS (short ISO without year)
fn format_epoch_ms(ms: i64) -> String {
    let secs = ms / 1000;
    // Days since epoch
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let h = time_of_day / 3600;
    let m = (time_of_day % 3600) / 60;
    let s = time_of_day % 60;

    // Convert days since epoch to month-day (simplified: use a standard algorithm)
    let (_, month, day) = days_to_date(days);
    format!("{:02}-{:02} {:02}:{:02}:{:02}", month, day, h, m, s)
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_date(days: i64) -> (i64, u32, u32) {
    // Civil calendar algorithm from Howard Hinnant
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
}

fn run_logs_query_results(extra_args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let (raw, stderr, status) = run_aws_json(&["logs", "get-query-results"], extra_args, verbose)?;

    if !status.success() {
        timer.track(
            "aws logs get-query-results",
            "rtk aws logs get-query-results",
            &stderr,
            &stderr,
        );
        std::process::exit(status.code().unwrap_or(1));
    }

    let filtered = match filter_logs_query_results(&raw) {
        Some(f) => f,
        None => raw.clone(),
    };
    println!("{}", filtered);

    timer.track(
        "aws logs get-query-results",
        "rtk aws logs get-query-results",
        &raw,
        &filtered,
    );
    Ok(())
}

fn filter_logs_query_results(json_str: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let results = v.get("results").and_then(|r| r.as_array())?;
    let status = v
        .get("status")
        .and_then(|s| s.as_str())
        .unwrap_or("Unknown");

    if results.is_empty() {
        return Some(format!("CloudWatch query ({}): 0 rows", status));
    }

    let mut lines: Vec<String> = Vec::new();
    for row in results {
        if let Some(fields) = row.as_array() {
            let pairs: Vec<String> = fields
                .iter()
                .filter_map(|f| {
                    let field = f.get("field").and_then(|v| v.as_str())?;
                    let value = f.get("value").and_then(|v| v.as_str())?;
                    // Skip the @ptr field (internal pointer, no user value)
                    if field == "@ptr" {
                        return None;
                    }
                    Some(format!("{}={}", field, value))
                })
                .collect();
            lines.push(pairs.join(" "));
        }
    }

    let header = format!("CloudWatch query ({}): {} rows", status, results.len());
    Some(format!("{}\n{}", header, lines.join("\n")))
}

// --- S3 transfer (sync/cp) ---

fn run_s3_transfer(op: &str, extra_args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let cmd_name = format!("s3 {}", op);

    // s3 sync/cp produce text output, not JSON
    let mut cmd = resolved_command("aws");
    cmd.args(["s3", op]);
    for arg in extra_args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: aws {} {}", cmd_name, extra_args.join(" "));
    }

    let output = cmd
        .output()
        .context(format!("Failed to run aws {}", cmd_name))?;
    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        timer.track(
            &format!("aws {}", cmd_name),
            &format!("rtk aws {}", cmd_name),
            &stderr,
            &stderr,
        );
        eprint!("{}", stderr);
        print!("{}", raw);
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let combined = if stderr.is_empty() {
        raw.clone()
    } else {
        format!("{}\n{}", raw, stderr)
    };

    let filtered = filter_s3_transfer(&combined, op);
    println!("{}", filtered);

    timer.track(
        &format!("aws {}", cmd_name),
        &format!("rtk aws {}", cmd_name),
        &combined,
        &filtered,
    );
    Ok(())
}

fn filter_s3_transfer(output: &str, op: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();

    // Pass through unchanged if short output
    if lines.len() < 10 {
        return output.to_string();
    }

    let mut uploads = 0usize;
    let mut downloads = 0usize;
    let mut deletes = 0usize;
    let mut copies = 0usize;
    let mut errors: Vec<&str> = Vec::new();
    let mut warnings: Vec<&str> = Vec::new();

    for line in &lines {
        let trimmed = line.trim();
        if trimmed.starts_with("upload:")
            || trimmed.starts_with("Completed") && trimmed.contains("upload")
        {
            uploads += 1;
        } else if trimmed.starts_with("download:") {
            downloads += 1;
        } else if trimmed.starts_with("delete:") {
            deletes += 1;
        } else if trimmed.starts_with("copy:") || trimmed.starts_with("move:") {
            copies += 1;
        } else if trimmed.starts_with("warning:") || trimmed.starts_with("WARN") {
            warnings.push(line);
        } else if trimmed.starts_with("error:")
            || trimmed.starts_with("ERROR")
            || trimmed.starts_with("fatal")
        {
            errors.push(line);
        }
    }

    let mut parts: Vec<String> = Vec::new();
    if uploads > 0 {
        parts.push(format!("{} uploaded", uploads));
    }
    if downloads > 0 {
        parts.push(format!("{} downloaded", downloads));
    }
    if deletes > 0 {
        parts.push(format!("{} deleted", deletes));
    }
    if copies > 0 {
        parts.push(format!("{} copied", copies));
    }

    let mut result = format!("S3 {}: {}", op, parts.join(", "));

    if !errors.is_empty() {
        result.push_str(&format!(", {} errors", errors.len()));
    } else {
        result.push_str(", 0 errors");
    }

    for warn in &warnings {
        result.push_str(&format!("\n{}", warn));
    }
    for err in &errors {
        result.push_str(&format!("\n{}", err));
    }

    result
}

// --- Secrets Manager ---

fn run_secrets_get(extra_args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let (raw, stderr, status) =
        run_aws_json(&["secretsmanager", "get-secret-value"], extra_args, verbose)?;

    if !status.success() {
        timer.track(
            "aws secretsmanager get-secret-value",
            "rtk aws secretsmanager get-secret-value",
            &stderr,
            &stderr,
        );
        std::process::exit(status.code().unwrap_or(1));
    }

    let filtered = match filter_secret_value(&raw) {
        Some(f) => f,
        None => raw.clone(),
    };
    println!("{}", filtered);

    timer.track(
        "aws secretsmanager get-secret-value",
        "rtk aws secretsmanager get-secret-value",
        &raw,
        &filtered,
    );
    Ok(())
}

fn filter_secret_value(json_str: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let name = v.get("Name").and_then(|n| n.as_str()).unwrap_or("?");
    let secret_string = v.get("SecretString").and_then(|s| s.as_str())?;

    // Try to parse SecretString as JSON for compact display
    let display = if let Ok(parsed) = serde_json::from_str::<Value>(secret_string) {
        serde_json::to_string(&parsed).unwrap_or_else(|_| secret_string.to_string())
    } else {
        secret_string.to_string()
    };

    Some(format!("{}: {}", name, display))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_sts_identity() {
        let json = r#"{
    "UserId": "AIDAEXAMPLEUSERID1234",
    "Account": "123456789012",
    "Arn": "arn:aws:iam::123456789012:user/dev-user"
}"#;
        let result = filter_sts_identity(json).unwrap();
        assert_eq!(
            result,
            "AWS: 123456789012 arn:aws:iam::123456789012:user/dev-user"
        );
    }

    #[test]
    fn test_snapshot_ec2_instances() {
        let json = r#"{"Reservations":[{"Instances":[{"InstanceId":"i-0a1b2c3d4e5f00001","InstanceType":"t3.micro","PrivateIpAddress":"10.0.1.10","State":{"Code":16,"Name":"running"},"Tags":[{"Key":"Name","Value":"web-server-1"}],"BlockDeviceMappings":[],"SecurityGroups":[]},{"InstanceId":"i-0a1b2c3d4e5f00002","InstanceType":"t3.large","PrivateIpAddress":"10.0.2.20","State":{"Code":80,"Name":"stopped"},"Tags":[{"Key":"Name","Value":"worker-1"}],"BlockDeviceMappings":[],"SecurityGroups":[]}]}]}"#;
        let result = filter_ec2_instances(json).unwrap();
        assert!(result.contains("EC2: 2 instances"));
        assert!(result.contains("i-0a1b2c3d4e5f00001 running t3.micro 10.0.1.10 (web-server-1)"));
        assert!(result.contains("i-0a1b2c3d4e5f00002 stopped t3.large 10.0.2.20 (worker-1)"));
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
            result,
            "AWS: 123456789012 arn:aws:iam::123456789012:user/dev"
        );
    }

    #[test]
    fn test_filter_sts_identity_missing_fields() {
        let json = r#"{}"#;
        let result = filter_sts_identity(json).unwrap();
        assert_eq!(result, "AWS: ? ?");
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
        assert!(result.contains("bucket1"));
        assert!(result.contains("bucket3"));
    }

    #[test]
    fn test_filter_s3_ls_overflow() {
        let mut lines = Vec::new();
        for i in 1..=50 {
            lines.push(format!("2024-01-01 bucket{}", i));
        }
        let input = lines.join("\n");
        let result = filter_s3_ls(&input);
        assert!(result.contains("... +20 more items"));
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
                    "Tags": [{"Key": "Name", "Value": "web-server"}]
                }, {
                    "InstanceId": "i-def456",
                    "State": {"Name": "stopped"},
                    "InstanceType": "t3.large",
                    "PrivateIpAddress": "10.0.1.6",
                    "Tags": [{"Key": "Name", "Value": "worker"}]
                }]
            }]
        }"#;
        let result = filter_ec2_instances(json).unwrap();
        assert!(result.contains("EC2: 2 instances"));
        assert!(result.contains("i-abc123 running t3.micro 10.0.1.5 (web-server)"));
        assert!(result.contains("i-def456 stopped t3.large 10.0.1.6 (worker)"));
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
        assert!(result.contains("(-)"));
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
        assert!(result.contains("api-service"));
        assert!(result.contains("worker-service"));
        assert!(!result.contains("arn:aws"));
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
        assert_eq!(result, "api ACTIVE 3/3 (FARGATE)");
    }

    #[test]
    fn test_filter_rds_instances() {
        let json = r#"{
            "DBInstances": [{
                "DBInstanceIdentifier": "mydb",
                "Engine": "postgres",
                "EngineVersion": "15.4",
                "DBInstanceClass": "db.t3.micro",
                "DBInstanceStatus": "available"
            }]
        }"#;
        let result = filter_rds_instances(json).unwrap();
        assert_eq!(result, "mydb postgres 15.4 db.t3.micro available");
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
        assert!(result.contains("my-stack CREATE_COMPLETE 2024-01-15"));
        assert!(result.contains("other-stack UPDATE_COMPLETE 2024-02-20"));
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
        assert!(result.contains("my-stack CREATE_COMPLETE 2024-01-15"));
        assert!(result.contains("ApiUrl=https://api.example.com"));
        assert!(result.contains("BucketName=my-bucket"));
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
        assert!(result.contains("my-stack CREATE_COMPLETE 2024-01-15"));
        assert!(!result.contains("="));
    }

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
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
        let output_tokens = count_tokens(&result);
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
        let output_tokens = count_tokens(&result);
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
        assert!(result.contains("... +5 more instances"));
    }

    // --- DynamoDB tests ---

    #[test]
    fn test_flatten_dynamodb_string() {
        let v: Value = serde_json::from_str(r#"{"S": "hello"}"#).unwrap();
        assert_eq!(flatten_dynamodb_value(&v), Value::String("hello".into()));
    }

    #[test]
    fn test_flatten_dynamodb_number() {
        let v: Value = serde_json::from_str(r#"{"N": "42"}"#).unwrap();
        assert_eq!(flatten_dynamodb_value(&v), serde_json::json!(42));
    }

    #[test]
    fn test_flatten_dynamodb_bool() {
        let v: Value = serde_json::from_str(r#"{"BOOL": true}"#).unwrap();
        assert_eq!(flatten_dynamodb_value(&v), Value::Bool(true));
    }

    #[test]
    fn test_flatten_dynamodb_null() {
        let v: Value = serde_json::from_str(r#"{"NULL": true}"#).unwrap();
        assert_eq!(flatten_dynamodb_value(&v), Value::Null);
    }

    #[test]
    fn test_flatten_dynamodb_list() {
        let v: Value = serde_json::from_str(r#"{"L": [{"S": "a"}, {"N": "1"}]}"#).unwrap();
        let expected = serde_json::json!(["a", 1]);
        assert_eq!(flatten_dynamodb_value(&v), expected);
    }

    #[test]
    fn test_flatten_dynamodb_map() {
        let v: Value =
            serde_json::from_str(r#"{"M": {"name": {"S": "Alice"}, "age": {"N": "30"}}}"#).unwrap();
        let expected = serde_json::json!({"name": "Alice", "age": 30});
        assert_eq!(flatten_dynamodb_value(&v), expected);
    }

    #[test]
    fn test_flatten_dynamodb_string_set() {
        let v: Value = serde_json::from_str(r#"{"SS": ["a", "b", "c"]}"#).unwrap();
        let expected = serde_json::json!(["a", "b", "c"]);
        assert_eq!(flatten_dynamodb_value(&v), expected);
    }

    #[test]
    fn test_flatten_dynamodb_number_set() {
        let v: Value = serde_json::from_str(r#"{"NS": ["1", "2", "3"]}"#).unwrap();
        let expected = serde_json::json!([1, 2, 3]);
        assert_eq!(flatten_dynamodb_value(&v), expected);
    }

    #[test]
    fn test_flatten_dynamodb_nested() {
        let v: Value = serde_json::from_str(
            r#"{"M": {"items": {"L": [{"M": {"id": {"N": "1"}, "name": {"S": "foo"}}}]}}}"#,
        )
        .unwrap();
        let expected = serde_json::json!({"items": [{"id": 1, "name": "foo"}]});
        assert_eq!(flatten_dynamodb_value(&v), expected);
    }

    #[test]
    fn test_flatten_non_dynamodb_passthrough() {
        let v: Value = serde_json::from_str(r#"{"name": "Alice", "age": 30}"#).unwrap();
        let expected = serde_json::json!({"name": "Alice", "age": 30});
        assert_eq!(flatten_dynamodb_value(&v), expected);
    }

    #[test]
    fn test_dynamodb_scan_filter() {
        let json = r#"{
            "Items": [
                {"id": {"N": "1"}, "name": {"S": "Alice"}, "active": {"BOOL": true}},
                {"id": {"N": "2"}, "name": {"S": "Bob"}, "active": {"BOOL": false}}
            ],
            "Count": 2,
            "ScannedCount": 100,
            "ConsumedCapacity": {"TableName": "users", "CapacityUnits": 5.0}
        }"#;
        let result = filter_dynamodb_scan_query(json, "scan").unwrap();
        assert!(result.contains("DynamoDB scan: 2 items, scanned: 100"));
        assert!(result.contains("\"Alice\""));
        assert!(result.contains("\"Bob\""));
        // Type wrappers should be stripped
        assert!(!result.contains(r#""S""#));
        assert!(!result.contains(r#""N""#));
        assert!(!result.contains(r#""BOOL""#));
        // ConsumedCapacity should be preserved
        assert!(result.contains("capacity: 5 RCU"));
    }

    #[test]
    fn test_dynamodb_scan_empty() {
        let json = r#"{"Items": [], "Count": 0, "ScannedCount": 0}"#;
        let result = filter_dynamodb_scan_query(json, "scan").unwrap();
        assert!(result.contains("DynamoDB scan: 0 items"));
    }

    #[test]
    fn test_dynamodb_get_item_filter() {
        let json = r#"{
            "Item": {
                "userId": {"S": "user-123"},
                "email": {"S": "alice@example.com"},
                "loginCount": {"N": "42"},
                "tags": {"SS": ["admin", "user"]}
            }
        }"#;
        let result = filter_dynamodb_get_item(json).unwrap();
        assert!(result.contains("DynamoDB item:"));
        assert!(result.contains("\"user-123\""));
        assert!(result.contains("42"));
        assert!(!result.contains(r#""S""#));
    }

    #[test]
    fn test_dynamodb_token_savings() {
        let json = r#"{
            "Items": [
                {"id": {"N": "1"}, "name": {"S": "Alice"}, "email": {"S": "alice@example.com"}, "active": {"BOOL": true}, "metadata": {"M": {"role": {"S": "admin"}, "loginCount": {"N": "42"}}}},
                {"id": {"N": "2"}, "name": {"S": "Bob"}, "email": {"S": "bob@example.com"}, "active": {"BOOL": false}, "metadata": {"M": {"role": {"S": "user"}, "loginCount": {"N": "7"}}}},
                {"id": {"N": "3"}, "name": {"S": "Charlie"}, "email": {"S": "charlie@example.com"}, "active": {"BOOL": true}, "metadata": {"M": {"role": {"S": "viewer"}, "loginCount": {"N": "0"}}}}
            ],
            "Count": 3,
            "ScannedCount": 3,
            "ConsumedCapacity": {"TableName": "users", "CapacityUnits": 15.0}
        }"#;
        let result = filter_dynamodb_scan_query(json, "scan").unwrap();
        let input_tokens = count_tokens(json);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 40.0,
            "DynamoDB scan filter: expected >=40% savings, got {:.1}%",
            savings
        );
    }

    // --- CloudWatch Logs tests ---

    #[test]
    fn test_filter_log_events_basic() {
        let json = r#"{
            "events": [
                {"timestamp": 1700000000000, "message": "START RequestId: abc-123\n", "logStreamName": "2024/01/15/[$LATEST]abc123"},
                {"timestamp": 1700000001000, "message": "Processing item 1\n", "logStreamName": "2024/01/15/[$LATEST]abc123"},
                {"timestamp": 1700000002000, "message": "END RequestId: abc-123\n", "logStreamName": "2024/01/15/[$LATEST]abc123"}
            ],
            "searchedLogStreams": [{"logStreamName": "stream-1", "searchedCompletely": true}]
        }"#;
        let result = filter_log_events(json).unwrap();
        assert!(result.contains("CloudWatch: 3 events"));
        assert!(result.contains("START RequestId"));
        assert!(result.contains("Processing item"));
        // logStreamName should be included in output
        assert!(result.contains("[$LATEST]abc123"));
        // Timestamps should include date
        assert!(result.contains("11-14"));
        // Should not contain searchedLogStreams metadata
        assert!(!result.contains("searchedCompletely"));
    }

    #[test]
    fn test_filter_log_events_dedup() {
        let json = r#"{
            "events": [
                {"timestamp": 1700000000000, "message": "heartbeat"},
                {"timestamp": 1700000001000, "message": "heartbeat"},
                {"timestamp": 1700000002000, "message": "heartbeat"},
                {"timestamp": 1700000003000, "message": "done"}
            ]
        }"#;
        let result = filter_log_events(json).unwrap();
        assert!(result.contains("[x3]"));
        assert!(result.contains("done"));
    }

    #[test]
    fn test_filter_log_events_empty() {
        let json = r#"{"events": []}"#;
        let result = filter_log_events(json).unwrap();
        assert_eq!(result, "CloudWatch: 0 events");
    }

    #[test]
    fn test_filter_logs_query_results() {
        let json = r#"{
            "results": [
                [{"field": "@timestamp", "value": "2024-01-15 10:30:00"}, {"field": "@message", "value": "Error occurred"}, {"field": "@ptr", "value": "abc123"}],
                [{"field": "@timestamp", "value": "2024-01-15 10:31:00"}, {"field": "@message", "value": "Retry succeeded"}, {"field": "@ptr", "value": "def456"}]
            ],
            "status": "Complete",
            "statistics": {"recordsMatched": 2, "recordsScanned": 1000, "bytesScanned": 50000}
        }"#;
        let result = filter_logs_query_results(json).unwrap();
        assert!(result.contains("CloudWatch query (Complete): 2 rows"));
        assert!(result.contains("@timestamp=2024-01-15 10:30:00"));
        assert!(result.contains("@message=Error occurred"));
        // @ptr should be filtered out
        assert!(!result.contains("@ptr"));
        // statistics should be stripped
        assert!(!result.contains("recordsScanned"));
    }

    #[test]
    fn test_filter_log_events_token_savings() {
        let json = r#"{
            "events": [
                {"timestamp": 1700000000000, "message": "START RequestId: abc-123 Version: $LATEST\n", "ingestionTime": 1700000000500, "eventId": "37134513644831132873138850000000000000000000000001", "logStreamName": "2024/01/15/[$LATEST]abc123"},
                {"timestamp": 1700000001000, "message": "INFO Processing request\n", "ingestionTime": 1700000001500, "eventId": "37134513644831132873138850000000000000000000000002", "logStreamName": "2024/01/15/[$LATEST]abc123"},
                {"timestamp": 1700000002000, "message": "INFO Processing request\n", "ingestionTime": 1700000002500, "eventId": "37134513644831132873138850000000000000000000000003", "logStreamName": "2024/01/15/[$LATEST]abc123"},
                {"timestamp": 1700000003000, "message": "END RequestId: abc-123\n", "ingestionTime": 1700000003500, "eventId": "37134513644831132873138850000000000000000000000004", "logStreamName": "2024/01/15/[$LATEST]abc123"},
                {"timestamp": 1700000004000, "message": "REPORT RequestId: abc-123 Duration: 150.50 ms Billed Duration: 200 ms Memory Size: 128 MB Max Memory Used: 64 MB\n", "ingestionTime": 1700000004500, "eventId": "37134513644831132873138850000000000000000000000005", "logStreamName": "2024/01/15/[$LATEST]abc123"}
            ],
            "searchedLogStreams": [{"logStreamName": "2024/01/15/[$LATEST]abc123", "searchedCompletely": true}]
        }"#;
        let result = filter_log_events(json).unwrap();
        let input_tokens = count_tokens(json);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 40.0,
            "CloudWatch filter: expected >=40% savings, got {:.1}%",
            savings
        );
    }

    // --- S3 transfer tests ---

    #[test]
    fn test_filter_s3_sync_summary() {
        let output = (0..15)
            .map(|i| format!("upload: ./file{}.txt to s3://bucket/file{}.txt", i, i))
            .chain(std::iter::once("delete: s3://bucket/old1.txt".to_string()))
            .chain(std::iter::once("delete: s3://bucket/old2.txt".to_string()))
            .collect::<Vec<_>>()
            .join("\n");
        let result = filter_s3_transfer(&output, "sync");
        assert!(result.contains("S3 sync: 15 uploaded, 2 deleted, 0 errors"));
    }

    #[test]
    fn test_filter_s3_cp_short_passthrough() {
        let output = "upload: ./file.txt to s3://bucket/file.txt";
        let result = filter_s3_transfer(output, "cp");
        assert_eq!(result, output);
    }

    #[test]
    fn test_filter_s3_transfer_errors_preserved() {
        let mut lines: Vec<String> = (0..12)
            .map(|i| format!("upload: ./file{}.txt to s3://bucket/file{}.txt", i, i))
            .collect();
        lines.push("error: Unable to upload ./bad.txt: Access Denied".to_string());
        lines.push("warning: Skipping symlink ./link.txt".to_string());
        let output = lines.join("\n");
        let result = filter_s3_transfer(&output, "sync");
        assert!(result.contains("error: Unable to upload"));
        assert!(result.contains("warning: Skipping symlink"));
        assert!(result.contains("1 errors"));
    }

    #[test]
    fn test_s3_transfer_token_savings() {
        let output = (0..50)
            .map(|i| format!("upload: ./path/to/some/long/directory/structure/file{}.txt to s3://my-very-long-bucket-name/prefix/path/to/file{}.txt", i, i))
            .collect::<Vec<_>>()
            .join("\n");
        let result = filter_s3_transfer(&output, "sync");
        let input_tokens = count_tokens(&output);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "S3 sync filter: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    // --- Secrets Manager tests ---

    #[test]
    fn test_filter_secret_value_string() {
        let json = r#"{
            "ARN": "arn:aws:secretsmanager:us-east-1:123456789012:secret:MySecret-abc123",
            "Name": "MySecret",
            "VersionId": "a1b2c3d4-5678-90ab-cdef-EXAMPLE11111",
            "SecretString": "my-plain-secret-value",
            "VersionStages": ["AWSCURRENT"],
            "CreatedDate": "2024-01-15T10:30:00Z"
        }"#;
        let result = filter_secret_value(json).unwrap();
        assert_eq!(result, "MySecret: my-plain-secret-value");
        assert!(!result.contains("ARN"));
        assert!(!result.contains("VersionId"));
    }

    #[test]
    fn test_filter_secret_value_json() {
        let json = r#"{
            "ARN": "arn:aws:secretsmanager:us-east-1:123456789012:secret:DbCreds-xyz",
            "Name": "DbCreds",
            "VersionId": "a1b2c3d4",
            "SecretString": "{\"username\":\"admin\",\"password\":\"hunter2\",\"host\":\"db.example.com\",\"port\":5432}",
            "VersionStages": ["AWSCURRENT"],
            "CreatedDate": "2024-01-15T10:30:00Z"
        }"#;
        let result = filter_secret_value(json).unwrap();
        assert!(result.starts_with("DbCreds: "));
        assert!(result.contains("\"username\":\"admin\""));
        assert!(!result.contains("VersionStages"));
    }

    #[test]
    fn test_secret_value_token_savings() {
        let json = r#"{
            "ARN": "arn:aws:secretsmanager:us-east-1:123456789012:secret:MySecret-abc123def456",
            "Name": "MyDatabaseCredentials",
            "VersionId": "a1b2c3d4-5678-90ab-cdef-EXAMPLE11111",
            "SecretString": "{\"username\":\"admin\",\"password\":\"s3cret!\"}",
            "VersionStages": ["AWSCURRENT"],
            "CreatedDate": "2024-01-15T10:30:00.000Z",
            "ResponseMetadata": {"RequestId": "abc-123", "HTTPStatusCode": 200}
        }"#;
        let result = filter_secret_value(json).unwrap();
        let input_tokens = count_tokens(json);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Secrets Manager filter: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    // --- format_epoch_ms test ---

    #[test]
    fn test_format_epoch_ms() {
        // 1700000000000 ms = 2023-11-14T22:13:20Z
        assert_eq!(format_epoch_ms(1700000000000), "11-14 22:13:20");
        assert_eq!(format_epoch_ms(0), "01-01 00:00:00");
    }
}
