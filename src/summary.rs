use crate::tracking;
use crate::utils::truncate;
use anyhow::{Context, Result};
use regex::Regex;
use std::process::{Command, Stdio};

/// Run a command and provide a heuristic summary
pub fn run(command: &str, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Running and summarizing: {}", command);
    }

    let output = if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", command])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
    } else {
        Command::new("sh")
            .args(["-c", command])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
    }
    .context("Failed to execute command")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let summary = summarize_output(&raw, command, output.status.success());
    println!("{}", summary);
    timer.track(command, "rtk summary", &raw, &summary);
    Ok(())
}

fn summarize_output(output: &str, command: &str, success: bool) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let mut result = Vec::new();

    // Status
    let status_icon = if success { "✅" } else { "❌" };
    result.push(format!(
        "{} Command: {}",
        status_icon,
        truncate(command, 60)
    ));
    result.push(format!("   {} lines of output", lines.len()));
    result.push(String::new());

    // Detect type of output and summarize accordingly
    let output_type = detect_output_type(output, command);

    match output_type {
        OutputType::TestResults => summarize_tests(output, &mut result),
        OutputType::BuildOutput => summarize_build(output, &mut result),
        OutputType::LogOutput => summarize_logs_quick(output, &mut result),
        OutputType::ListOutput => summarize_list(output, &mut result),
        OutputType::JsonOutput => summarize_json(output, &mut result),
        OutputType::Generic => summarize_generic(output, &mut result),
    }

    result.join("\n")
}

#[derive(Debug)]
enum OutputType {
    TestResults,
    BuildOutput,
    LogOutput,
    ListOutput,
    JsonOutput,
    Generic,
}

fn detect_output_type(output: &str, command: &str) -> OutputType {
    let cmd_lower = command.to_lowercase();
    let out_lower = output.to_lowercase();

    if cmd_lower.contains("test") || out_lower.contains("passed") && out_lower.contains("failed") {
        OutputType::TestResults
    } else if cmd_lower.contains("build")
        || cmd_lower.contains("compile")
        || out_lower.contains("compiling")
    {
        OutputType::BuildOutput
    } else if out_lower.contains("error:")
        || out_lower.contains("warn:")
        || out_lower.contains("[info]")
    {
        OutputType::LogOutput
    } else if output.trim_start().starts_with('{') || output.trim_start().starts_with('[') {
        OutputType::JsonOutput
    } else if output.lines().all(|l| {
        l.len() < 200
            && !l
                .contains('\t')
                .then_some(true)
                .unwrap_or(l.split_whitespace().count() < 10)
    }) {
        OutputType::ListOutput
    } else {
        OutputType::Generic
    }
}

fn summarize_tests(output: &str, result: &mut Vec<String>) {
    result.push("📋 Test Results:".to_string());

    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;
    let mut failures = Vec::new();

    for line in output.lines() {
        let lower = line.to_lowercase();
        if lower.contains("passed") || lower.contains("✓") || lower.contains("ok") {
            // Try to extract number
            if let Some(n) = extract_number(&lower, "passed") {
                passed = n;
            } else {
                passed += 1;
            }
        }
        if lower.contains("failed") || lower.contains("✗") || lower.contains("fail") {
            if let Some(n) = extract_number(&lower, "failed") {
                failed = n;
            }
            if !line.contains("0 failed") {
                failures.push(line.to_string());
            }
        }
        if lower.contains("skipped") || lower.contains("ignored") {
            if let Some(n) = extract_number(&lower, "skipped").or(extract_number(&lower, "ignored"))
            {
                skipped = n;
            }
        }
    }

    result.push(format!("   ✅ {} passed", passed));
    if failed > 0 {
        result.push(format!("   ❌ {} failed", failed));
    }
    if skipped > 0 {
        result.push(format!("   ⏭️  {} skipped", skipped));
    }

    if !failures.is_empty() {
        result.push(String::new());
        result.push("   Failures:".to_string());
        for f in failures.iter().take(5) {
            result.push(format!("   • {}", truncate(f, 70)));
        }
    }
}

fn summarize_build(output: &str, result: &mut Vec<String>) {
    result.push("🔨 Build Summary:".to_string());

    let mut errors = 0;
    let mut warnings = 0;
    let mut compiled = 0;
    let mut error_msgs = Vec::new();

    for line in output.lines() {
        let lower = line.to_lowercase();
        if lower.contains("error") && !lower.contains("0 error") {
            errors += 1;
            if error_msgs.len() < 5 {
                error_msgs.push(line.to_string());
            }
        }
        if lower.contains("warning") && !lower.contains("0 warning") {
            warnings += 1;
        }
        if lower.contains("compiling") || lower.contains("compiled") {
            compiled += 1;
        }
    }

    if compiled > 0 {
        result.push(format!("   📦 {} crates/files compiled", compiled));
    }
    if errors > 0 {
        result.push(format!("   ❌ {} errors", errors));
    }
    if warnings > 0 {
        result.push(format!("   ⚠️  {} warnings", warnings));
    }
    if errors == 0 && warnings == 0 {
        result.push("   ✅ Build successful".to_string());
    }

    if !error_msgs.is_empty() {
        result.push(String::new());
        result.push("   Errors:".to_string());
        for e in &error_msgs {
            result.push(format!("   • {}", truncate(e, 70)));
        }
    }
}

fn summarize_logs_quick(output: &str, result: &mut Vec<String>) {
    result.push("📝 Log Summary:".to_string());

    let mut errors = 0;
    let mut warnings = 0;
    let mut info = 0;

    for line in output.lines() {
        let lower = line.to_lowercase();
        if lower.contains("error") || lower.contains("fatal") {
            errors += 1;
        } else if lower.contains("warn") {
            warnings += 1;
        } else if lower.contains("info") {
            info += 1;
        }
    }

    result.push(format!("   ❌ {} errors", errors));
    result.push(format!("   ⚠️  {} warnings", warnings));
    result.push(format!("   ℹ️  {} info", info));
}

fn summarize_list(output: &str, result: &mut Vec<String>) {
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    result.push(format!("📋 List ({} items):", lines.len()));

    for line in lines.iter().take(10) {
        result.push(format!("   • {}", truncate(line, 70)));
    }
    if lines.len() > 10 {
        result.push(format!("   ... +{} more", lines.len() - 10));
    }
}

fn summarize_json(output: &str, result: &mut Vec<String>) {
    result.push("📋 JSON Output:".to_string());

    // Try to parse and show structure
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(output) {
        match &value {
            serde_json::Value::Array(arr) => {
                result.push(format!("   Array with {} items", arr.len()));
            }
            serde_json::Value::Object(obj) => {
                result.push(format!("   Object with {} keys:", obj.len()));
                for key in obj.keys().take(10) {
                    result.push(format!("   • {}", key));
                }
                if obj.len() > 10 {
                    result.push(format!("   ... +{} more keys", obj.len() - 10));
                }
            }
            _ => {
                result.push(format!("   {}", truncate(&value.to_string(), 100)));
            }
        }
    } else {
        result.push("   (Invalid JSON)".to_string());
    }
}

fn summarize_generic(output: &str, result: &mut Vec<String>) {
    let lines: Vec<&str> = output.lines().collect();

    result.push("📋 Output:".to_string());

    // First few lines
    for line in lines.iter().take(5) {
        if !line.trim().is_empty() {
            result.push(format!("   {}", truncate(line, 75)));
        }
    }

    if lines.len() > 10 {
        result.push("   ...".to_string());
        // Last few lines
        for line in lines.iter().skip(lines.len() - 3) {
            if !line.trim().is_empty() {
                result.push(format!("   {}", truncate(line, 75)));
            }
        }
    }
}

fn extract_number(text: &str, after: &str) -> Option<usize> {
    let re = Regex::new(&format!(r"(\d+)\s*{}", after)).ok()?;
    re.captures(text)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
}
