use crate::core::tracking;
use crate::core::utils::{resolved_command, strip_ansi, tool_exists, truncate};
use anyhow::{Context, Result};
use regex::Regex;
use std::ffi::OsString;
use std::io::IsTerminal;
use std::process::Command;

lazy_static::lazy_static! {
    /// CloudFormation resource progress lines (both standard and stack-prefixed formats):
    ///   " 0/12 | 10:30:01 AM | CREATE_IN_PROGRESS ..."
    ///   "StackName | 0/6 | 17:20:49 | UPDATE_IN_PROGRESS ..."
    static ref PROGRESS_RE: Regex = Regex::new(
        r"(\d+/\d+\s*\||\|\s*\d+/\d+\s*\|).*\|\s*(CREATE|UPDATE|DELETE|IMPORT)_(IN_PROGRESS|COMPLETE|FAILED|SKIPPED|COMPLETE_CLEA)"
    ).unwrap();

    /// Resource type line in synth YAML output (Type: AWS::...)
    static ref RESOURCE_TYPE_RE: Regex = Regex::new(r"^\s+Type:\s+(AWS::\S+)").unwrap();

    /// Stack name from diff/deploy output header
    static ref STACK_NAME_RE: Regex = Regex::new(r"^Stack\s+(\S+)").unwrap();
}

/// Build a Command for the CDK CLI, with npx fallback.
fn cdk_command() -> Command {
    if tool_exists("cdk") {
        resolved_command("cdk")
    } else {
        let mut cmd = resolved_command("npx");
        cmd.arg("cdk");
        cmd
    }
}

pub fn run_diff(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = cdk_command();
    cmd.arg("diff");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: cdk diff {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run cdk diff. Is AWS CDK installed?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });

    let filtered = filter_cdk_diff(&strip_ansi(&raw));

    if let Some(hint) = crate::core::tee::tee_and_hint(&raw, "cdk_diff", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("cdk diff {}", args.join(" ")),
        &format!("rtk cdk diff {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

pub fn run_synth(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = cdk_command();
    cmd.arg("synth");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: cdk synth {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run cdk synth. Is AWS CDK installed?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });

    let filtered = filter_cdk_synth(&strip_ansi(&raw));

    if let Some(hint) = crate::core::tee::tee_and_hint(&raw, "cdk_synth", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("cdk synth {}", args.join(" ")),
        &format!("rtk cdk synth {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

pub fn run_deploy(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = cdk_command();
    cmd.arg("deploy");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: cdk deploy {}", args.join(" "));
    }

    // Use .status() for interactive deploy (approval prompts)
    // unless we detect non-interactive flags
    let is_non_interactive = args.iter().enumerate().any(|(i, a)| {
        a == "--require-approval=never"
            || (a == "--require-approval" && args.get(i + 1).map(|s| s.as_str()) == Some("never"))
            || a == "--ci"
            || a == "-y"
            || a == "--yes"
    });

    if !is_non_interactive && std::io::stdout().is_terminal() {
        // Interactive mode: pass through stdin/stdout for approval prompts
        let status = cmd
            .status()
            .context("Failed to run cdk deploy. Is AWS CDK installed?")?;

        let exit_code = status
            .code()
            .unwrap_or(if status.success() { 0 } else { 1 });
        timer.track_passthrough(
            &format!("cdk deploy {}", args.join(" ")),
            &format!("rtk cdk deploy {} (interactive)", args.join(" ")),
        );

        if !status.success() {
            std::process::exit(exit_code);
        }

        return Ok(());
    }

    // Non-interactive: capture and filter output
    let output = cmd
        .output()
        .context("Failed to run cdk deploy. Is AWS CDK installed?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });

    let filtered = filter_cdk_deploy(&strip_ansi(&raw));

    if let Some(hint) = crate::core::tee::tee_and_hint(&raw, "cdk_deploy", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("cdk deploy {}", args.join(" ")),
        &format!("rtk cdk deploy {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

pub fn run_other(args: &[OsString], verbose: u8) -> Result<()> {
    if args.is_empty() {
        anyhow::bail!("cdk: no subcommand specified");
    }

    let timer = tracking::TimedExecution::start();

    let subcommand = args[0].to_string_lossy();
    let mut cmd = cdk_command();
    cmd.arg(&*subcommand);

    for arg in &args[1..] {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: cdk {} ...", subcommand);
    }

    let status = cmd
        .status()
        .with_context(|| format!("Failed to run cdk {}", subcommand))?;

    timer.track_passthrough(
        &format!("cdk {}", subcommand),
        &format!("rtk cdk {}", subcommand),
    );

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

// ──────────────────── Filter functions ────────────────────

/// Whitelist check: is this line meaningful diff content?
fn is_diff_content(trimmed: &str) -> bool {
    // Stack headers
    STACK_NAME_RE.is_match(trimmed)
    // Diff markers — catches ALL diff content including nested subtree
    // (both proper Unicode and mojibake box-drawing, because [~]/[+]/[-] is always ASCII)
    || trimmed.contains("[~]") || trimmed.contains("[+]") || trimmed.contains("[-]")
    || trimmed.contains("[ ]")
    // Unified diff hunk headers (inside inline diffs)
    || trimmed.contains("@@ ")
    // Section headers
    || trimmed == "Resources"
    // Status lines
    || trimmed.contains("no differences") || trimmed.contains("No differences")
    || trimmed.contains("Number of stacks with differences")
    // IAM changes (keep if present)
    || trimmed.starts_with("IAM Statement Changes") || trimmed.starts_with("IAM Policy Changes")
    || trimmed.starts_with('\u{250c}') || trimmed.starts_with('\u{251c}') || trimmed.starts_with('\u{2514}')
    || trimmed.starts_with('\u{2502}')
}

/// Filter cdk diff output: whitelist approach — keep only meaningful lines.
pub fn filter_cdk_diff(output: &str) -> String {
    let mut added = 0usize;
    let mut removed = 0usize;
    let mut changed = 0usize;
    let mut lines: Vec<String> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if is_diff_content(trimmed) {
            // Count diff markers at the start of trimmed line
            if trimmed.starts_with("[+]") {
                added += 1;
            } else if trimmed.starts_with("[-]") {
                removed += 1;
            } else if trimmed.starts_with("[~]") {
                changed += 1;
            }
            lines.push(truncate(line, 120).to_string());
        }
    }

    let mut result = String::new();
    for line in &lines {
        result.push_str(line);
        result.push('\n');
    }

    result.push_str(&format!(
        "\nSummary: {} added, {} changed, {} removed",
        added, changed, removed
    ));

    result.trim().to_string()
}

/// Filter cdk synth output: dual-path whitelist.
/// - No-template path: "Successfully synthesized" message + stack list
/// - Template path: extract resource type counts from YAML
pub fn filter_cdk_synth(output: &str) -> String {
    let mut has_synth_message = false;
    let mut synth_line = String::new();
    let mut stack_list_line = String::new();
    let mut resource_types: Vec<String> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // Detect "Successfully synthesized" line
        if trimmed.contains("Successfully synthesized") {
            has_synth_message = true;
            synth_line = trimmed.to_string();
            continue;
        }

        // Detect "Supply a stack id" line (contains stack list)
        if trimmed.starts_with("Supply a stack id") {
            stack_list_line = trimmed.to_string();
            continue;
        }

        // Collect resource types from YAML template
        if let Some(caps) = RESOURCE_TYPE_RE.captures(line) {
            resource_types.push(caps[1].to_string());
        }
    }

    let mut result = String::new();

    // No-template path: show synth message + stack list
    if has_synth_message {
        result.push_str(&synth_line);
        result.push('\n');
        if !stack_list_line.is_empty() {
            // Extract just the stack names from "Supply a stack id (A, B, C) to display its template."
            if let Some(start) = stack_list_line.find('(') {
                if let Some(end) = stack_list_line.rfind(')') {
                    let stacks = &stack_list_line[start + 1..end];
                    result.push_str(&format!("Stacks: {}\n", stacks));
                }
            }
        }
    }

    // Template path: show resource type summary
    if !resource_types.is_empty() {
        // Group by resource type
        let mut type_counts: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for rt in &resource_types {
            *type_counts.entry(rt.as_str()).or_insert(0) += 1;
        }

        let mut sorted_types: Vec<_> = type_counts.into_iter().collect();
        sorted_types.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));

        result.push_str(&format!("Resources: {} total\n", resource_types.len()));
        for (rtype, count) in &sorted_types {
            let short = rtype.strip_prefix("AWS::").unwrap_or(rtype);
            if *count > 1 {
                result.push_str(&format!("  {} x{}\n", short, count));
            } else {
                result.push_str(&format!("  {}\n", short));
            }
        }
    }

    // Handle empty output
    if result.is_empty() {
        result.push_str("Resources: 0 total");
    }

    result.trim().to_string()
}

/// Filter cdk deploy output: whitelist approach — keep only meaningful lines.
pub fn filter_cdk_deploy(output: &str) -> String {
    let mut stack_name = String::new();
    let mut outputs: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut success_line = String::new();
    let mut deploy_time = String::new();
    let mut total_time = String::new();
    let mut in_outputs = false;
    let mut resource_count = 0usize;
    let mut failed_count = 0usize;
    let mut complete_count = 0usize;

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            continue;
        }

        // Track progress lines for counting (both standard and stack-prefixed)
        if PROGRESS_RE.is_match(trimmed) {
            resource_count += 1;
            if trimmed.contains("_FAILED") {
                failed_count += 1;
                errors.push(truncate(trimmed, 150).to_string());
            } else if trimmed.contains("_COMPLETE") {
                complete_count += 1;
            }
            continue;
        }

        // Success line: proper Unicode checkmark or mojibake variant
        if trimmed.contains('\u{2705}') || trimmed.contains("Ô£à") {
            success_line = trimmed.to_string();
            // Extract stack name after the emoji
            let name = trimmed
                .trim_start_matches('\u{2705}')
                .trim_start_matches("Ô£à")
                .trim();
            if !name.is_empty() {
                stack_name = name.to_string();
            }
            continue;
        }

        // "deploying..." line
        if trimmed.contains("deploying...") {
            stack_name = trimmed.split(':').next().unwrap_or("").trim().to_string();
            continue;
        }

        // Deployment time
        if trimmed.contains("Deployment time:") {
            deploy_time = trimmed.to_string();
            continue;
        }
        if trimmed.contains("Total time:") {
            total_time = trimmed.to_string();
            continue;
        }

        // Track Outputs section
        if trimmed == "Outputs:" || trimmed.starts_with("Outputs:") {
            in_outputs = true;
            continue;
        }
        if trimmed.starts_with("Stack ARN:") {
            in_outputs = false;
            continue;
        }

        if in_outputs {
            outputs.push(truncate(trimmed, 150).to_string());
            continue;
        }

        // Capture deployment error/failure messages (CDK-specific patterns only)
        if (trimmed.contains("FAILED")
            || trimmed.contains("ROLLBACK")
            || trimmed.starts_with("❌")
            || trimmed.contains("Deployment failed"))
            && !errors.iter().any(|e| e == trimmed)
        {
            errors.push(truncate(trimmed, 150).to_string());
        }
    }

    let mut result = String::new();

    if !success_line.is_empty() {
        result.push_str(&format!("{}\n", success_line));
    } else if !stack_name.is_empty() {
        result.push_str(&format!("Stack: {}\n", stack_name));
    }

    // Resource summary
    if resource_count > 0 {
        result.push_str(&format!(
            "Resources: {}/{} complete",
            complete_count, resource_count
        ));
        if failed_count > 0 {
            result.push_str(&format!(", {} failed", failed_count));
        }
        result.push('\n');
    }

    // Errors
    if !errors.is_empty() {
        result.push_str("\nErrors:\n");
        for err in &errors {
            result.push_str(&format!("  {}\n", err));
        }
    }

    // Outputs
    if !outputs.is_empty() {
        result.push_str("\nOutputs:\n");
        for out in &outputs {
            result.push_str(&format!("  {}\n", out));
        }
    }

    // Timing
    if !deploy_time.is_empty() {
        result.push_str(&format!("\n{}", deploy_time));
    }
    if !total_time.is_empty() {
        result.push_str(&format!("\n{}", total_time));
    }

    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    // ──────────────── cdk diff — main fixture ────────────────

    #[test]
    fn test_filter_cdk_diff_basic() {
        let input = include_str!("../../../tests/fixtures/cdk_diff_raw.txt");
        let output = filter_cdk_diff(input);

        // Diff content preserved
        assert!(output.contains("[~]"));
        assert!(output.contains("Summary:"));
        // Bundling noise stripped
        assert!(!output.contains("Bundling asset"));
        assert!(!output.contains("cdk.out/bundling"));
        assert!(!output.contains("esbuild"));
        // NOTICES stripped
        assert!(!output.contains("NOTICES"));
        assert!(!output.contains("cdk watch"));
    }

    #[test]
    fn test_filter_cdk_diff_multi_stack() {
        let input = include_str!("../../../tests/fixtures/cdk_diff_raw.txt");
        let output = filter_cdk_diff(input);

        // Multiple stacks preserved
        assert!(output.contains("Stack CdkBackendStack/PushNotificationsStack"));
        assert!(output.contains("Stack CdkBackendStack/WorkflowStack"));
        assert!(output.contains("Stack CdkBackendStack/BatchWorkflowStack"));
        assert!(
            output.contains("Stack CdkBackendStack\n")
                || output.contains("Stack CdkBackendStack\r")
        );
    }

    #[test]
    fn test_filter_cdk_diff_counts() {
        let input = include_str!("../../../tests/fixtures/cdk_diff_raw.txt");
        let output = filter_cdk_diff(input);

        // All changes are [~] in this fixture (10 lambdas with code changes)
        assert!(output.contains("0 added"));
        assert!(output.contains("10 changed"));
        assert!(output.contains("0 removed"));
    }

    #[test]
    fn test_filter_cdk_diff_savings() {
        let input = include_str!("../../../tests/fixtures/cdk_diff_raw.txt");
        let output = filter_cdk_diff(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "cdk diff filter: expected >=60% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_filter_cdk_diff_snapshot() {
        let input = include_str!("../../../tests/fixtures/cdk_diff_raw.txt");
        let output = filter_cdk_diff(input);
        assert_snapshot!(output);
    }

    // ──────────────── cdk diff — _1 fixture (SampleApp) ────────────────

    #[test]
    fn test_filter_cdk_diff_1_docker_pip_stripped() {
        let input = include_str!("../../../tests/fixtures/cdk_diff_raw_1.txt");
        let output = filter_cdk_diff(input);

        // Docker pull noise stripped
        assert!(!output.contains("Pulling fs layer"));
        assert!(!output.contains("Download complete"));
        assert!(!output.contains("Pull complete"));
        // pip install noise stripped
        assert!(!output.contains("Collecting feedparser"));
        assert!(!output.contains("Downloading"));
        assert!(!output.contains("Installing collected packages"));
        // esbuild noise stripped
        assert!(!output.contains("esbuild"));
    }

    #[test]
    fn test_filter_cdk_diff_1_mojibake_kept() {
        let input = include_str!("../../../tests/fixtures/cdk_diff_raw_1.txt");
        let output = filter_cdk_diff(input);

        // Mojibake diff content with [~]/[+]/[-] preserved
        assert!(output.contains("[~]"));
        assert!(output.contains("[+]"));
        assert!(output.contains("[-]"));
        // Stack name preserved
        assert!(output.contains("Stack SampleApp-RssBatch"));
    }

    #[test]
    fn test_filter_cdk_diff_1_savings() {
        let input = include_str!("../../../tests/fixtures/cdk_diff_raw_1.txt");
        let output = filter_cdk_diff(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "cdk diff _1 filter: expected >=60% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_filter_cdk_diff_1_snapshot() {
        let input = include_str!("../../../tests/fixtures/cdk_diff_raw_1.txt");
        let output = filter_cdk_diff(input);
        assert_snapshot!(output);
    }

    // ──────────────── cdk diff — edge cases ────────────────

    #[test]
    fn test_filter_cdk_diff_no_changes() {
        let input = "Stack MyStack\nThere were no differences";
        let output = filter_cdk_diff(input);

        assert!(output.contains("no differences"));
        assert!(output.contains("0 added"));
    }

    #[test]
    fn test_filter_cdk_diff_empty() {
        let output = filter_cdk_diff("");
        assert!(output.contains("0 added"));
    }

    #[test]
    fn test_filter_cdk_diff_iam_synthetic() {
        let input = r#"Stack MyStack
Resources
[~] AWS::IAM::Role MyRole
IAM Statement Changes
┌───┬─────────┬────────┬───────────┐
│   │ Res     │ Effect │ Action    │
├───┼─────────┼────────┼───────────┤
│ + │ MyRole  │ Allow  │ s3:Get*   │
└───┴─────────┴────────┴───────────┘
[+] AWS::Lambda::Function NewFunc"#;

        let output = filter_cdk_diff(input);

        assert!(output.contains("IAM Statement Changes"));
        assert!(output.contains("\u{250c}"));
        assert!(output.contains("\u{2502}"));
        assert!(output.contains("\u{2514}"));
        assert!(output.contains("1 added, 1 changed, 0 removed"));
    }

    // ──────────────── cdk synth — main fixture ────────────────

    #[test]
    fn test_filter_cdk_synth_basic() {
        let input = include_str!("../../../tests/fixtures/cdk_synth_raw.txt");
        let output = filter_cdk_synth(input);

        // No-template path: shows synth success + stack list
        assert!(output.contains("Successfully synthesized"));
        assert!(output.contains("Stacks:"));
        // Bundling stripped
        assert!(!output.contains("Bundling asset"));
        assert!(!output.contains("esbuild"));
        // NOTICES stripped
        assert!(!output.contains("NOTICES"));
    }

    #[test]
    fn test_filter_cdk_synth_stack_list() {
        let input = include_str!("../../../tests/fixtures/cdk_synth_raw.txt");
        let output = filter_cdk_synth(input);

        // Stack names extracted
        assert!(output.contains("CdkBackendStack/PushNotificationsStack"));
        assert!(output.contains("CdkBackendStack/BatchWorkflowStack"));
    }

    #[test]
    fn test_filter_cdk_synth_savings() {
        let input = include_str!("../../../tests/fixtures/cdk_synth_raw.txt");
        let output = filter_cdk_synth(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "cdk synth filter: expected >=60% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_filter_cdk_synth_snapshot() {
        let input = include_str!("../../../tests/fixtures/cdk_synth_raw.txt");
        let output = filter_cdk_synth(input);
        assert_snapshot!(output);
    }

    // ──────────────── cdk synth — _1 fixture (template path) ────────────────

    #[test]
    fn test_filter_cdk_synth_1_yaml_template() {
        let input = include_str!("../../../tests/fixtures/cdk_synth_raw_1.txt");
        let output = filter_cdk_synth(input);

        // Template path: resource types extracted
        assert!(output.contains("Resources:"));
        assert!(output.contains("IAM::Role"));
        assert!(output.contains("IAM::Policy"));
        assert!(output.contains("Lambda::Function"));
        // Full YAML not present
        assert!(!output.contains("Properties:"));
        assert!(!output.contains("BucketName:"));
    }

    #[test]
    fn test_filter_cdk_synth_1_savings() {
        let input = include_str!("../../../tests/fixtures/cdk_synth_raw_1.txt");
        let output = filter_cdk_synth(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "cdk synth _1 filter: expected >=60% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_filter_cdk_synth_1_snapshot() {
        let input = include_str!("../../../tests/fixtures/cdk_synth_raw_1.txt");
        let output = filter_cdk_synth(input);
        assert_snapshot!(output);
    }

    // ──────────────── cdk synth — edge cases ────────────────

    #[test]
    fn test_filter_cdk_synth_empty() {
        let output = filter_cdk_synth("");
        assert!(output.contains("Resources: 0"));
    }

    #[test]
    fn test_filter_cdk_synth_yaml_synthetic() {
        let input = r#"Resources:
  MyBucket:
    Type: AWS::S3::Bucket
    Properties:
      BucketName: my-bucket
  MyFunc:
    Type: AWS::Lambda::Function
    Properties:
      Runtime: nodejs22.x
  MyTable:
    Type: AWS::DynamoDB::Table
    Properties:
      TableName: my-table
  MyFunc2:
    Type: AWS::Lambda::Function
    Properties:
      Runtime: python3.14"#;

        let output = filter_cdk_synth(input);

        assert!(output.contains("Resources: 4 total"));
        assert!(output.contains("Lambda::Function x2"));
        assert!(output.contains("S3::Bucket"));
        assert!(output.contains("DynamoDB::Table"));
        assert!(!output.contains("Properties:"));
    }

    // ──────────────── cdk deploy — main fixture ────────────────

    #[test]
    fn test_filter_cdk_deploy_basic() {
        let input = include_str!("../../../tests/fixtures/cdk_deploy_raw.txt");
        let output = filter_cdk_deploy(input);

        assert!(output.contains("\u{2705}"));
        // Per-resource progress stripped
        assert!(!output.contains("CREATE_IN_PROGRESS"));
        assert!(!output.contains("10:30:"));
        // Bundling stripped
        assert!(!output.contains("Bundling asset"));
    }

    #[test]
    fn test_filter_cdk_deploy_outputs_preserved() {
        let input = include_str!("../../../tests/fixtures/cdk_deploy_raw.txt");
        let output = filter_cdk_deploy(input);

        assert!(output.contains("Outputs:"));
        assert!(output.contains("ApiEndpoint"));
    }

    #[test]
    fn test_filter_cdk_deploy_resource_summary() {
        let input = include_str!("../../../tests/fixtures/cdk_deploy_raw.txt");
        let output = filter_cdk_deploy(input);

        assert!(output.contains("Resources:"));
        assert!(output.contains("complete"));
    }

    #[test]
    fn test_filter_cdk_deploy_savings() {
        let input = include_str!("../../../tests/fixtures/cdk_deploy_raw.txt");
        let output = filter_cdk_deploy(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "cdk deploy filter: expected >=60% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_filter_cdk_deploy_snapshot() {
        let input = include_str!("../../../tests/fixtures/cdk_deploy_raw.txt");
        let output = filter_cdk_deploy(input);
        assert_snapshot!(output);
    }

    // ──────────────── cdk deploy — _1 fixture (SampleApp) ────────────────

    #[test]
    fn test_filter_cdk_deploy_1_basic() {
        let input = include_str!("../../../tests/fixtures/cdk_deploy_raw_1.txt");
        let output = filter_cdk_deploy(input);

        // Success detected (mojibake checkmark)
        assert!(
            output.contains("SampleApp-RssBatch"),
            "Should contain stack name. Output:\n{}",
            output
        );
        // Progress stripped
        assert!(!output.contains("UPDATE_IN_PROGRESS"));
        assert!(!output.contains("17:20:"));
    }

    #[test]
    fn test_filter_cdk_deploy_1_notices_stripped() {
        let input = include_str!("../../../tests/fixtures/cdk_deploy_raw_1.txt");
        let output = filter_cdk_deploy(input);

        assert!(!output.contains("NOTICES"));
        assert!(!output.contains("cdk watch"));
        assert!(!output.contains("cdk acknowledge"));
    }

    #[test]
    fn test_filter_cdk_deploy_1_savings() {
        let input = include_str!("../../../tests/fixtures/cdk_deploy_raw_1.txt");
        let output = filter_cdk_deploy(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "cdk deploy _1 filter: expected >=60% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_filter_cdk_deploy_1_snapshot() {
        let input = include_str!("../../../tests/fixtures/cdk_deploy_raw_1.txt");
        let output = filter_cdk_deploy(input);
        assert_snapshot!(output);
    }

    // ──────────────── cdk deploy — edge cases ────────────────

    #[test]
    fn test_filter_cdk_deploy_empty() {
        let output = filter_cdk_deploy("");
        // Should not panic on empty input and produce empty result
        assert!(
            output.is_empty(),
            "Expected empty output, got: {:?}",
            output
        );
    }

    #[test]
    fn test_filter_cdk_deploy_failure() {
        let input = r#"InfraStack: deploying... [1/1]
InfraStack: creating CloudFormation changeset...
 0/5 | 10:30:01 AM | CREATE_IN_PROGRESS   | AWS::Lambda::Function | ApiLambda
 1/5 | 10:30:10 AM | CREATE_COMPLETE      | AWS::Lambda::Function | ApiLambda
 1/5 | 10:30:15 AM | CREATE_FAILED        | AWS::IAM::Policy      | BadPolicy (Resource limit exceeded)
 1/5 | 10:30:20 AM | ROLLBACK_IN_PROGRESS | AWS::CloudFormation::Stack | InfraStack

❌ Deployment failed: Error: The stack named InfraStack failed creation"#;

        let output = filter_cdk_deploy(input);

        assert!(
            output.contains("FAILED")
                || output.contains("Deployment failed")
                || output.contains("❌")
        );
    }
}
