//! AWS CLI output compression.
//!
//! Replaces verbose `--output table`/`text` with JSON, then compresses.
//! Specialized filters for high-frequency commands (STS, S3, EC2, ECS, RDS, CloudFormation).

use crate::json_cmd;
use crate::tracking;
use crate::utils::{join_with_overflow, resolved_command, truncate_iso_date};
use anyhow::{Context, Result};
use serde_json::Value;

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
}
