//! CDK CLI output compression.
//!
//! Filters `cdk synth`, `cdk deploy`, and `cdk diff` output to reduce tokens.
//! - synth: Extract resource counts and logical IDs from CloudFormation template
//! - deploy: Strip IN_PROGRESS event spam, preserve errors and outputs
//! - diff: Summarize changes, preserve security-relevant diffs

use crate::tracking;
use crate::utils::{resolved_command, strip_ansi};
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref CFN_RESOURCE_TYPE_RE: Regex =
        Regex::new(r#"(?m)^\s+Type:\s+(AWS::\S+)"#).unwrap();
    static ref CFN_LOGICAL_ID_RE: Regex =
        Regex::new(r#"(?m)^  (\w[\w-]*):\s*$"#).unwrap();
    static ref CFN_PARAMETER_RE: Regex =
        Regex::new(r#"(?m)^  (\w[\w-]*):\s*\n\s+Type:"#).unwrap();
    static ref CFN_OUTPUT_KEY_RE: Regex =
        Regex::new(r#"(?m)^  (\w[\w-]*):\s*\n\s+Value:"#).unwrap();
    static ref DEPLOY_STACK_RE: Regex =
        Regex::new(r"(?i)(\S+)\s*[:|]\s*(CREATE|UPDATE|DELETE|ROLLBACK|IMPORT)_(COMPLETE|FAILED|IN_PROGRESS|ROLLBACK_COMPLETE|ROLLBACK_IN_PROGRESS|CLEANUP_IN_PROGRESS)")
            .unwrap();
    static ref DEPLOY_OUTPUT_RE: Regex =
        Regex::new(r"(?m)^Outputs:").unwrap();
    static ref DIFF_CHANGE_RE: Regex =
        Regex::new(r"(?m)^\[([+\-~])\]\s+(.+)$").unwrap();
}

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    if args.is_empty() {
        return run_passthrough(args, verbose);
    }

    match args[0].as_str() {
        "synth" | "synthesize" => run_synth(&args[1..], verbose),
        "deploy" => run_deploy(&args[1..], verbose),
        "diff" => run_diff(&args[1..], verbose),
        _ => run_passthrough(args, verbose),
    }
}

fn run_cdk_command(
    subcommand: &str,
    extra_args: &[String],
    verbose: u8,
) -> Result<(String, String, std::process::ExitStatus)> {
    let mut cmd = resolved_command("npx");
    cmd.arg("cdk");
    cmd.arg(subcommand);
    for arg in extra_args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: npx cdk {} {}", subcommand, extra_args.join(" "));
    }

    let output = cmd
        .output()
        .context(format!("Failed to run npx cdk {}", subcommand))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok((stdout, stderr, output.status))
}

fn run_synth(extra_args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let (raw, stderr, status) = run_cdk_command("synth", extra_args, verbose)?;

    if !status.success() {
        timer.track("cdk synth", "rtk cdk synth", &stderr, &stderr);
        eprint!("{}", stderr);
        std::process::exit(status.code().unwrap_or(1));
    }

    let combined = if stderr.is_empty() {
        raw.clone()
    } else {
        format!("{}\n{}", raw, stderr)
    };

    let filtered = filter_synth(&combined);
    println!("{}", filtered);

    timer.track("cdk synth", "rtk cdk synth", &combined, &filtered);
    Ok(())
}

fn filter_synth(output: &str) -> String {
    let clean = strip_ansi(output);

    // Count resources by Type
    let mut type_counts: Vec<(String, usize)> = Vec::new();
    for cap in CFN_RESOURCE_TYPE_RE.captures_iter(&clean) {
        let rtype = cap[1].to_string();
        if let Some(entry) = type_counts.iter_mut().find(|(t, _)| t == &rtype) {
            entry.1 += 1;
        } else {
            type_counts.push((rtype, 1));
        }
    }

    if type_counts.is_empty() {
        // Not a CloudFormation template — might be JSON or empty
        // Try JSON synth output
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&clean) {
            return filter_synth_json(&v);
        }
        return clean;
    }

    type_counts.sort_by(|a, b| b.1.cmp(&a.1));
    let total_resources: usize = type_counts.iter().map(|(_, c)| c).sum();

    let mut result = format!("CDK synth: {} resources\n", total_resources);
    for (rtype, count) in &type_counts {
        result.push_str(&format!("  {}: {}\n", rtype, count));
    }

    // Extract Parameters section
    let params: Vec<&str> = CFN_PARAMETER_RE
        .captures_iter(&clean)
        .filter_map(|c| c.get(1).map(|m| m.as_str()))
        .collect();
    if !params.is_empty() {
        result.push_str(&format!("Parameters: {}\n", params.join(", ")));
    }

    // Extract Outputs section
    let outputs: Vec<&str> = CFN_OUTPUT_KEY_RE
        .captures_iter(&clean)
        .filter_map(|c| c.get(1).map(|m| m.as_str()))
        .collect();
    if !outputs.is_empty() {
        result.push_str(&format!("Outputs: {}\n", outputs.join(", ")));
    }

    result.trim_end().to_string()
}

fn filter_synth_json(v: &serde_json::Value) -> String {
    // JSON CloudFormation template
    let resources = v.get("Resources").and_then(|r| r.as_object());
    let parameters = v.get("Parameters").and_then(|p| p.as_object());
    let outputs = v.get("Outputs").and_then(|o| o.as_object());

    let mut type_counts: Vec<(String, usize)> = Vec::new();
    if let Some(res) = resources {
        for (_logical_id, resource) in res {
            let rtype = resource
                .get("Type")
                .and_then(|t| t.as_str())
                .unwrap_or("Unknown")
                .to_string();
            if let Some(entry) = type_counts.iter_mut().find(|(t, _)| t == &rtype) {
                entry.1 += 1;
            } else {
                type_counts.push((rtype, 1));
            }
        }
    }

    type_counts.sort_by(|a, b| b.1.cmp(&a.1));
    let total: usize = type_counts.iter().map(|(_, c)| c).sum();

    let mut result = format!("CDK synth: {} resources\n", total);
    for (rtype, count) in &type_counts {
        result.push_str(&format!("  {}: {}\n", rtype, count));
    }

    if let Some(params) = parameters {
        let keys: Vec<&str> = params.keys().map(|k| k.as_str()).collect();
        if !keys.is_empty() {
            result.push_str(&format!("Parameters: {}\n", keys.join(", ")));
        }
    }

    if let Some(outs) = outputs {
        let keys: Vec<&str> = outs.keys().map(|k| k.as_str()).collect();
        if !keys.is_empty() {
            result.push_str(&format!("Outputs: {}\n", keys.join(", ")));
        }
    }

    result.trim_end().to_string()
}

fn run_deploy(extra_args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let (raw, stderr, status) = run_cdk_command("deploy", extra_args, verbose)?;

    let combined = format!("{}\n{}", raw, stderr);

    if !status.success() {
        let filtered = filter_deploy(&combined);
        timer.track("cdk deploy", "rtk cdk deploy", &combined, &filtered);
        println!("{}", filtered);
        std::process::exit(status.code().unwrap_or(1));
    }

    let filtered = filter_deploy(&combined);
    println!("{}", filtered);

    timer.track("cdk deploy", "rtk cdk deploy", &combined, &filtered);
    Ok(())
}

fn filter_deploy(output: &str) -> String {
    let clean = strip_ansi(output);
    let lines: Vec<&str> = clean.lines().collect();

    let mut stack_name: Option<&str> = None;
    let mut creates = 0usize;
    let mut updates = 0usize;
    let mut deletes = 0usize;
    let mut failed_lines: Vec<&str> = Vec::new();
    let mut output_lines: Vec<&str> = Vec::new();
    let mut in_outputs = false;

    for line in &lines {
        let trimmed = line.trim();

        // Detect Outputs section
        if DEPLOY_OUTPUT_RE.is_match(trimmed) {
            in_outputs = true;
            continue;
        }

        if in_outputs {
            if trimmed.is_empty() {
                in_outputs = false;
            } else {
                output_lines.push(trimmed);
            }
            continue;
        }

        // Count events by action
        if let Some(cap) = DEPLOY_STACK_RE.captures(trimmed) {
            if stack_name.is_none() {
                // Extract stack name from first resource line
                // Look for the stack name in the line context
                if let Some(name) = extract_stack_name(trimmed) {
                    stack_name = Some(name);
                }
            }

            let action = cap.get(2).map(|m| m.as_str()).unwrap_or("");
            let result_str = cap.get(3).map(|m| m.as_str()).unwrap_or("");

            if result_str == "COMPLETE" {
                match action {
                    "CREATE" => creates += 1,
                    "UPDATE" => updates += 1,
                    "DELETE" => deletes += 1,
                    _ => {}
                }
            }

            // Preserve FAILED/ROLLBACK lines
            if result_str == "FAILED" || action == "ROLLBACK" || trimmed.contains("ROLLBACK") {
                failed_lines.push(line);
            }
        }

        // Preserve error lines
        if trimmed.starts_with("Error:")
            || trimmed.starts_with("error:")
            || trimmed.contains("FAILED")
            || trimmed.contains("fail") && !trimmed.contains("IN_PROGRESS")
        {
            if !failed_lines.contains(line) {
                failed_lines.push(line);
            }
        }
    }

    let name = stack_name.unwrap_or("stack");
    let mut result = format!("CDK deploy: {}\n", name);

    let mut parts: Vec<String> = Vec::new();
    if creates > 0 {
        parts.push(format!("{} created", creates));
    }
    if updates > 0 {
        parts.push(format!("{} updated", updates));
    }
    if deletes > 0 {
        parts.push(format!("{} deleted", deletes));
    }

    if parts.is_empty() {
        result.push_str("  No resource changes\n");
    } else {
        result.push_str(&format!("  Resources: {}\n", parts.join(", ")));
    }

    if !failed_lines.is_empty() {
        result.push_str("  Errors:\n");
        for line in &failed_lines {
            result.push_str(&format!("    {}\n", line.trim()));
        }
    }

    if !output_lines.is_empty() {
        result.push_str("  Outputs:\n");
        for line in &output_lines {
            result.push_str(&format!("    {}\n", line));
        }
    }

    result.trim_end().to_string()
}

fn extract_stack_name(line: &str) -> Option<&str> {
    // CDK deploy lines often contain the stack name
    // Common patterns: "MyStack | CREATE_COMPLETE", "MyStack: deploying..."
    let trimmed = line.trim();
    // Split on common separators
    for sep in [" | ", ": ", " - "] {
        if let Some(idx) = trimmed.find(sep) {
            let candidate = trimmed[..idx].trim();
            if !candidate.is_empty() && !candidate.contains(' ') {
                return Some(candidate);
            }
        }
    }
    None
}

fn run_diff(extra_args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let (raw, stderr, status) = run_cdk_command("diff", extra_args, verbose)?;

    let combined = format!("{}\n{}", raw, stderr);

    // cdk diff exits non-zero when there ARE differences (expected behavior)
    let filtered = filter_diff(&combined);
    println!("{}", filtered);

    timer.track("cdk diff", "rtk cdk diff", &combined, &filtered);

    if !status.success() {
        // Exit code 1 means "there are differences" — not an error
        // Only propagate if it's a real error (exit code > 1)
        let code = status.code().unwrap_or(1);
        if code > 1 {
            std::process::exit(code);
        }
    }
    Ok(())
}

fn filter_diff(output: &str) -> String {
    let clean = strip_ansi(output);
    let lines: Vec<&str> = clean.lines().collect();

    let mut additions = 0usize;
    let mut removals = 0usize;
    let mut modifications = 0usize;
    let mut security_lines: Vec<&str> = Vec::new();

    for line in &lines {
        let trimmed = line.trim();

        if let Some(cap) = DIFF_CHANGE_RE.captures(trimmed) {
            let change_type = &cap[1];
            let content = &cap[2];

            match change_type {
                "+" => additions += 1,
                "-" => removals += 1,
                "~" => modifications += 1,
                _ => {}
            }

            // Preserve IAM and SecurityGroup changes in full
            if content.contains("AWS::IAM")
                || content.contains("AWS::EC2::SecurityGroup")
                || content.contains("PolicyDocument")
                || content.contains("SecurityGroup")
            {
                security_lines.push(line);
            }
        }
    }

    if additions == 0 && removals == 0 && modifications == 0 {
        return if clean.trim().is_empty() {
            "CDK diff: no changes".to_string()
        } else {
            clean
        };
    }

    let mut result = format!(
        "CDK diff: {} additions, {} removals, {} modifications\n",
        additions, removals, modifications
    );

    if !security_lines.is_empty() {
        result.push_str("Security-relevant changes:\n");
        for line in &security_lines {
            result.push_str(&format!("  {}\n", line.trim()));
        }
    }

    result.trim_end().to_string()
}

fn run_passthrough(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let mut cmd = resolved_command("npx");
    cmd.arg("cdk");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: npx cdk {}", args.join(" "));
    }

    let status = cmd.status().context("Failed to run npx cdk")?;
    let args_str = args.join(" ");
    timer.track_passthrough(
        &format!("cdk {}", args_str),
        &format!("rtk cdk {} (passthrough)", args_str),
    );

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_filter_synth_yaml() {
        let output = r#"Resources:
  MyFunction:
    Type: AWS::Lambda::Function
    Properties:
      Runtime: python3.11
      Handler: index.handler
      Code:
        S3Bucket: my-bucket
        S3Key: code.zip
      MemorySize: 256
      Timeout: 30
  MyTable:
    Type: AWS::DynamoDB::Table
    Properties:
      TableName: my-table
      AttributeDefinitions:
        - AttributeName: id
          AttributeType: S
      KeySchema:
        - AttributeName: id
          KeyType: HASH
  MyApi:
    Type: AWS::ApiGateway::RestApi
    Properties:
      Name: my-api
  MyApi2:
    Type: AWS::ApiGateway::RestApi
    Properties:
      Name: my-api-2
  MyBucket:
    Type: AWS::S3::Bucket
    Properties:
      BucketName: my-bucket
"#;
        let result = filter_synth(output);
        assert!(result.contains("CDK synth: 5 resources"));
        assert!(result.contains("AWS::ApiGateway::RestApi: 2"));
        assert!(result.contains("AWS::Lambda::Function: 1"));
        assert!(result.contains("AWS::DynamoDB::Table: 1"));
    }

    #[test]
    fn test_filter_synth_json() {
        let output = r#"{
            "Resources": {
                "MyFunc": {"Type": "AWS::Lambda::Function", "Properties": {"Runtime": "python3.11"}},
                "MyTable": {"Type": "AWS::DynamoDB::Table", "Properties": {"TableName": "t"}},
                "MyBucket": {"Type": "AWS::S3::Bucket", "Properties": {"BucketName": "b"}}
            },
            "Parameters": {
                "Env": {"Type": "String", "Default": "prod"},
                "Region": {"Type": "String"}
            },
            "Outputs": {
                "ApiUrl": {"Value": "https://api.example.com"},
                "BucketArn": {"Value": "arn:aws:s3:::b"}
            }
        }"#;
        let result = filter_synth(output);
        assert!(result.contains("CDK synth: 3 resources"));
        assert!(result.contains("Parameters: Env, Region"));
        assert!(result.contains("Outputs: ApiUrl, BucketArn"));
    }

    #[test]
    fn test_filter_deploy_success() {
        let output = "MyStack | CREATE_IN_PROGRESS | AWS::CloudFormation::Stack | MyStack
MyStack | CREATE_IN_PROGRESS | AWS::Lambda::Function | MyFunc
MyStack | CREATE_COMPLETE | AWS::Lambda::Function | MyFunc
MyStack | CREATE_IN_PROGRESS | AWS::DynamoDB::Table | MyTable
MyStack | CREATE_COMPLETE | AWS::DynamoDB::Table | MyTable
MyStack | CREATE_IN_PROGRESS | AWS::S3::Bucket | MyBucket
MyStack | CREATE_COMPLETE | AWS::S3::Bucket | MyBucket
MyStack | CREATE_COMPLETE | AWS::CloudFormation::Stack | MyStack

Outputs:
MyStack.ApiUrl = https://api.example.com
MyStack.BucketArn = arn:aws:s3:::my-bucket
";
        let result = filter_deploy(output);
        assert!(result.contains("CDK deploy: MyStack"));
        assert!(result.contains("4 created"));
        assert!(result.contains("Outputs:"));
        assert!(result.contains("ApiUrl"));
        // Should not contain IN_PROGRESS spam
        assert!(!result.contains("IN_PROGRESS"));
    }

    #[test]
    fn test_filter_deploy_rollback() {
        let output = "MyStack | CREATE_IN_PROGRESS | AWS::Lambda::Function | MyFunc
MyStack | CREATE_FAILED | AWS::Lambda::Function | MyFunc
MyStack | ROLLBACK_IN_PROGRESS | AWS::CloudFormation::Stack | MyStack
MyStack | ROLLBACK_COMPLETE | AWS::CloudFormation::Stack | MyStack
Error: The stack named MyStack failed creation";
        let result = filter_deploy(output);
        assert!(result.contains("Errors:"));
        assert!(result.contains("FAILED") || result.contains("ROLLBACK"));
    }

    #[test]
    fn test_filter_deploy_with_outputs() {
        let output = "MyStack | UPDATE_COMPLETE | AWS::Lambda::Function | MyFunc

Outputs:
MyStack.Endpoint = https://api.example.com
MyStack.TableName = my-table
";
        let result = filter_deploy(output);
        assert!(result.contains("Outputs:"));
        assert!(result.contains("Endpoint"));
        assert!(result.contains("TableName"));
    }

    #[test]
    fn test_filter_diff_additions() {
        let output = "Stack MyStack
[+] AWS::Lambda::Function MyFunc
[+] AWS::DynamoDB::Table MyTable
[~] AWS::S3::Bucket MyBucket
[-] AWS::SNS::Topic OldTopic
";
        let result = filter_diff(output);
        assert!(result.contains("2 additions"));
        assert!(result.contains("1 removals"));
        assert!(result.contains("1 modifications"));
    }

    #[test]
    fn test_filter_diff_security() {
        let output = "Stack MyStack
[+] AWS::IAM::Role MyRole
[~] AWS::EC2::SecurityGroup MySG
[+] AWS::Lambda::Function MyFunc
";
        let result = filter_diff(output);
        assert!(result.contains("Security-relevant changes:"));
        assert!(result.contains("AWS::IAM::Role"));
        assert!(result.contains("AWS::EC2::SecurityGroup"));
    }

    #[test]
    fn test_filter_diff_empty() {
        let result = filter_diff("");
        assert_eq!(result, "CDK diff: no changes");
    }

    #[test]
    fn test_synth_token_savings() {
        let output = r#"{
            "Resources": {
                "MyFunc": {"Type": "AWS::Lambda::Function", "Properties": {"Runtime": "python3.11", "Handler": "index.handler", "Code": {"S3Bucket": "bucket", "S3Key": "code.zip"}, "MemorySize": 256, "Timeout": 30, "Environment": {"Variables": {"TABLE_NAME": "my-table", "BUCKET_NAME": "my-bucket"}}}},
                "MyTable": {"Type": "AWS::DynamoDB::Table", "Properties": {"TableName": "my-table", "AttributeDefinitions": [{"AttributeName": "id", "AttributeType": "S"}], "KeySchema": [{"AttributeName": "id", "KeyType": "HASH"}], "BillingMode": "PAY_PER_REQUEST"}},
                "MyBucket": {"Type": "AWS::S3::Bucket", "Properties": {"BucketName": "my-bucket", "VersioningConfiguration": {"Status": "Enabled"}, "BucketEncryption": {"ServerSideEncryptionConfiguration": [{"ServerSideEncryptionByDefault": {"SSEAlgorithm": "aws:kms"}}]}}},
                "MyApi": {"Type": "AWS::ApiGateway::RestApi", "Properties": {"Name": "my-api", "Description": "My REST API endpoint"}},
                "MyRole": {"Type": "AWS::IAM::Role", "Properties": {"AssumeRolePolicyDocument": {"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": {"Service": "lambda.amazonaws.com"}, "Action": "sts:AssumeRole"}]}, "Policies": [{"PolicyName": "DynamoDBAccess", "PolicyDocument": {"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Action": ["dynamodb:*"], "Resource": "*"}]}}]}}
            },
            "Parameters": {"Env": {"Type": "String"}, "Region": {"Type": "String"}},
            "Outputs": {"ApiUrl": {"Value": "https://api.example.com"}, "BucketArn": {"Value": "arn:aws:s3:::my-bucket"}}
        }"#;
        let result = filter_synth(output);
        let input_tokens = count_tokens(output);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "CDK synth filter: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_deploy_token_savings() {
        let mut lines: Vec<String> = Vec::new();
        for i in 0..20 {
            lines.push(format!(
                "MyStack | CREATE_IN_PROGRESS | AWS::Lambda::Function | Func{}",
                i
            ));
            lines.push(format!(
                "MyStack | CREATE_COMPLETE | AWS::Lambda::Function | Func{}",
                i
            ));
        }
        lines.push("MyStack | CREATE_COMPLETE | AWS::CloudFormation::Stack | MyStack".to_string());
        lines.push(String::new());
        lines.push("Outputs:".to_string());
        lines.push("MyStack.ApiUrl = https://api.example.com".to_string());

        let output = lines.join("\n");
        let result = filter_deploy(&output);
        let input_tokens = count_tokens(&output);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "CDK deploy filter: expected >=60% savings, got {:.1}%",
            savings
        );
    }
}
