use crate::mypy_cmd;
use crate::ruff_cmd;
use crate::tracking;
use crate::utils::{package_manager_exec, truncate};
use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Command;

#[derive(Debug, Deserialize, Serialize)]
struct EslintMessage {
    #[serde(rename = "ruleId")]
    rule_id: Option<String>,
    severity: u8,
    message: String,
    line: usize,
    column: usize,
}

#[derive(Debug, Deserialize, Serialize)]
struct EslintResult {
    #[serde(rename = "filePath")]
    file_path: String,
    messages: Vec<EslintMessage>,
    #[serde(rename = "errorCount")]
    error_count: usize,
    #[serde(rename = "warningCount")]
    warning_count: usize,
}

#[derive(Debug, Deserialize)]
struct PylintDiagnostic {
    #[serde(rename = "type")]
    msg_type: String, // "warning", "error", "convention", "refactor"
    #[allow(dead_code)]
    module: String,
    #[allow(dead_code)]
    obj: String,
    line: usize,
    #[allow(dead_code)]
    column: usize,
    path: String,
    symbol: String, // rule code like "unused-variable"
    message: String,
    #[serde(rename = "message-id")]
    message_id: String, // e.g., "W0612"
}

/// Check if a linter is Python-based (uses pip/pipx, not npm/pnpm)
fn is_python_linter(linter: &str) -> bool {
    matches!(linter, "ruff" | "pylint" | "mypy" | "flake8")
}

/// Strip package manager prefixes (npx, bunx, pnpm, pnpm exec, yarn) from args.
/// Returns the number of args to skip.
fn strip_pm_prefix(args: &[String]) -> usize {
    let pm_names = ["npx", "bunx", "pnpm", "yarn"];
    let mut skip = 0;
    for arg in args {
        if pm_names.contains(&arg.as_str()) || arg == "exec" {
            skip += 1;
        } else {
            break;
        }
    }
    skip
}

/// Detect the linter name from args (after stripping PM prefixes).
/// Returns the linter name and whether it was explicitly specified.
fn detect_linter(args: &[String]) -> (&str, bool) {
    let is_path_or_flag = args.is_empty()
        || args[0].starts_with('-')
        || args[0].contains('/')
        || args[0].contains('.');

    if is_path_or_flag {
        ("eslint", false)
    } else {
        (&args[0], true)
    }
}

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let skip = strip_pm_prefix(args);
    let effective_args = &args[skip..];

    let (linter, explicit) = detect_linter(effective_args);

    // Python linters use Command::new() directly (they're on PATH via pip/pipx)
    // JS linters use package_manager_exec (npx/pnpm exec)
    let mut cmd = if is_python_linter(linter) {
        Command::new(linter)
    } else {
        package_manager_exec(linter)
    };

    // Add format flags based on linter
    match linter {
        "eslint" => {
            cmd.arg("-f").arg("json");
        }
        "ruff" => {
            // Force JSON output for ruff check
            if !effective_args.contains(&"--output-format".to_string()) {
                cmd.arg("check").arg("--output-format=json");
            }
        }
        "pylint" => {
            // Force JSON2 output for pylint
            if !effective_args.contains(&"--output-format".to_string()) {
                cmd.arg("--output-format=json2");
            }
        }
        "mypy" => {
            // mypy uses default text output (no special flags)
        }
        _ => {
            // Other linters: no special formatting
        }
    }

    // Add user arguments (skip first if it was the linter name, and skip "check" for ruff if we added it)
    let start_idx = if !explicit {
        0
    } else if linter == "ruff" && !effective_args.is_empty() && effective_args[0] == "ruff" {
        // Skip "ruff" and "check" if we already added "check"
        if effective_args.len() > 1 && effective_args[1] == "check" {
            2
        } else {
            1
        }
    } else {
        1
    };

    for arg in &effective_args[start_idx..] {
        // Skip --output-format if we already added it
        if linter == "ruff" && arg.starts_with("--output-format") {
            continue;
        }
        if linter == "pylint" && arg.starts_with("--output-format") {
            continue;
        }
        cmd.arg(arg);
    }

    // Default to current directory if no path specified (for ruff/pylint/mypy/eslint)
    if matches!(linter, "ruff" | "pylint" | "mypy" | "eslint") {
        let has_path = effective_args
            .iter()
            .skip(start_idx)
            .any(|a| !a.starts_with('-') && !a.contains('='));
        if !has_path {
            cmd.arg(".");
        }
    }

    if verbose > 0 {
        eprintln!("Running: {} with structured output", linter);
    }

    let output = cmd.output().context(format!(
        "Failed to run {}. Is it installed? Try: pip install {} (or npm/pnpm for JS linters)",
        linter, linter
    ))?;

    // Check if process was killed by signal (SIGABRT, SIGKILL, etc.)
    if !output.status.success() && output.status.code().is_none() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("⚠️  Linter process terminated abnormally (possibly out of memory)");
        if !stderr.is_empty() {
            eprintln!(
                "stderr: {}",
                stderr.lines().take(5).collect::<Vec<_>>().join("\n")
            );
        }
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    // Dispatch to appropriate filter based on linter
    let filtered = match linter {
        "eslint" => filter_eslint_json(&stdout),
        "ruff" => {
            // Reuse ruff_cmd's JSON parser
            if !stdout.trim().is_empty() {
                ruff_cmd::filter_ruff_check_json(&stdout)
            } else {
                "✓ Ruff: No issues found".to_string()
            }
        }
        "pylint" => filter_pylint_json(&stdout),
        "mypy" => mypy_cmd::filter_mypy_output(&raw),
        _ => filter_generic_lint(&raw),
    };

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });
    if let Some(hint) = crate::tee::tee_and_hint(&raw, "lint", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("{} {}", linter, args.join(" ")),
        &format!("rtk lint {} {}", linter, args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

/// Filter ESLint JSON output - group by rule and file
fn filter_eslint_json(output: &str) -> String {
    let results: Result<Vec<EslintResult>, _> = serde_json::from_str(output);

    let results = match results {
        Ok(r) => r,
        Err(e) => {
            // Fallback if JSON parsing fails
            return format!(
                "ESLint output (JSON parse failed: {})\n{}",
                e,
                truncate(output, 500)
            );
        }
    };

    // Count total issues
    let total_errors: usize = results.iter().map(|r| r.error_count).sum();
    let total_warnings: usize = results.iter().map(|r| r.warning_count).sum();
    let total_files = results.iter().filter(|r| !r.messages.is_empty()).count();

    if total_errors == 0 && total_warnings == 0 {
        return "✓ ESLint: No issues found".to_string();
    }

    // Group messages by rule
    let mut by_rule: HashMap<String, usize> = HashMap::new();
    for result in &results {
        for msg in &result.messages {
            if let Some(rule) = &msg.rule_id {
                *by_rule.entry(rule.clone()).or_insert(0) += 1;
            }
        }
    }

    // Group by file
    let mut by_file: Vec<(&EslintResult, usize)> = results
        .iter()
        .filter(|r| !r.messages.is_empty())
        .map(|r| (r, r.messages.len()))
        .collect();
    by_file.sort_by(|a, b| b.1.cmp(&a.1));

    // Build output
    let mut result = String::new();
    result.push_str(&format!(
        "ESLint: {} errors, {} warnings in {} files\n",
        total_errors, total_warnings, total_files
    ));
    result.push_str("═══════════════════════════════════════\n");

    // Show top rules
    let mut rule_counts: Vec<_> = by_rule.iter().collect();
    rule_counts.sort_by(|a, b| b.1.cmp(a.1));

    if !rule_counts.is_empty() {
        result.push_str("Top rules:\n");
        for (rule, count) in rule_counts.iter().take(10) {
            result.push_str(&format!("  {} ({}x)\n", rule, count));
        }
        result.push('\n');
    }

    // Show top files with most issues
    result.push_str("Top files:\n");
    for (file_result, count) in by_file.iter().take(10) {
        let short_path = compact_path(&file_result.file_path);
        result.push_str(&format!("  {} ({} issues)\n", short_path, count));

        // Show top 3 rules in this file
        let mut file_rules: HashMap<String, usize> = HashMap::new();
        for msg in &file_result.messages {
            if let Some(rule) = &msg.rule_id {
                *file_rules.entry(rule.clone()).or_insert(0) += 1;
            }
        }

        let mut file_rule_counts: Vec<_> = file_rules.iter().collect();
        file_rule_counts.sort_by(|a, b| b.1.cmp(a.1));

        for (rule, count) in file_rule_counts.iter().take(3) {
            result.push_str(&format!("    {} ({})\n", rule, count));
        }
    }

    if by_file.len() > 10 {
        result.push_str(&format!("\n... +{} more files\n", by_file.len() - 10));
    }

    result.trim().to_string()
}

/// Filter pylint JSON2 output - group by symbol and file
fn filter_pylint_json(output: &str) -> String {
    let diagnostics: Result<Vec<PylintDiagnostic>, _> = serde_json::from_str(output);

    let diagnostics = match diagnostics {
        Ok(d) => d,
        Err(e) => {
            // Fallback if JSON parsing fails
            return format!(
                "Pylint output (JSON parse failed: {})\n{}",
                e,
                truncate(output, 500)
            );
        }
    };

    if diagnostics.is_empty() {
        return "✓ Pylint: No issues found".to_string();
    }

    // Count by type
    let mut errors = 0;
    let mut warnings = 0;
    let mut conventions = 0;
    let mut refactors = 0;

    for diag in &diagnostics {
        match diag.msg_type.as_str() {
            "error" => errors += 1,
            "warning" => warnings += 1,
            "convention" => conventions += 1,
            "refactor" => refactors += 1,
            _ => {}
        }
    }

    // Count unique files
    let unique_files: std::collections::HashSet<_> = diagnostics.iter().map(|d| &d.path).collect();
    let total_files = unique_files.len();

    // Group by symbol (rule code)
    let mut by_symbol: HashMap<String, usize> = HashMap::new();
    for diag in &diagnostics {
        let key = format!("{} ({})", diag.symbol, diag.message_id);
        *by_symbol.entry(key).or_insert(0) += 1;
    }

    // Group by file
    let mut by_file: HashMap<&str, usize> = HashMap::new();
    for diag in &diagnostics {
        *by_file.entry(&diag.path).or_insert(0) += 1;
    }

    let mut file_counts: Vec<_> = by_file.iter().collect();
    file_counts.sort_by(|a, b| b.1.cmp(a.1));

    // Build output
    let mut result = String::new();
    result.push_str(&format!(
        "Pylint: {} issues in {} files\n",
        diagnostics.len(),
        total_files
    ));

    if errors > 0 || warnings > 0 {
        result.push_str(&format!("  {} errors, {} warnings", errors, warnings));
        if conventions > 0 || refactors > 0 {
            result.push_str(&format!(
                ", {} conventions, {} refactors",
                conventions, refactors
            ));
        }
        result.push('\n');
    }

    result.push_str("═══════════════════════════════════════\n");

    // Show top symbols (rules)
    let mut symbol_counts: Vec<_> = by_symbol.iter().collect();
    symbol_counts.sort_by(|a, b| b.1.cmp(a.1));

    if !symbol_counts.is_empty() {
        result.push_str("Top rules:\n");
        for (symbol, count) in symbol_counts.iter().take(10) {
            result.push_str(&format!("  {} ({}x)\n", symbol, count));
        }
        result.push('\n');
    }

    // Show top files
    result.push_str("Top files:\n");
    for (file, count) in file_counts.iter().take(10) {
        let short_path = compact_path(file);
        result.push_str(&format!("  {} ({} issues)\n", short_path, count));

        // Show top 3 rules in this file
        let mut file_symbols: HashMap<String, usize> = HashMap::new();
        for diag in diagnostics.iter().filter(|d| &d.path == *file) {
            let key = format!("{} ({})", diag.symbol, diag.message_id);
            *file_symbols.entry(key).or_insert(0) += 1;
        }

        let mut file_symbol_counts: Vec<_> = file_symbols.iter().collect();
        file_symbol_counts.sort_by(|a, b| b.1.cmp(a.1));

        for (symbol, count) in file_symbol_counts.iter().take(3) {
            result.push_str(&format!("    {} ({})\n", symbol, count));
        }
    }

    if file_counts.len() > 10 {
        result.push_str(&format!("\n... +{} more files\n", file_counts.len() - 10));
    }

    result.trim().to_string()
}

/// Filter generic linter output (fallback for non-ESLint linters)
fn filter_generic_lint(output: &str) -> String {
    let mut warnings = 0;
    let mut errors = 0;
    let mut issues: Vec<String> = Vec::new();

    for line in output.lines() {
        let line_lower = line.to_lowercase();
        if line_lower.contains("warning") {
            warnings += 1;
            issues.push(line.to_string());
        }
        if line_lower.contains("error") && !line_lower.contains("0 error") {
            errors += 1;
            issues.push(line.to_string());
        }
    }

    if errors == 0 && warnings == 0 {
        return "✓ Lint: No issues found".to_string();
    }

    let mut result = String::new();
    result.push_str(&format!("Lint: {} errors, {} warnings\n", errors, warnings));
    result.push_str("═══════════════════════════════════════\n");

    for issue in issues.iter().take(20) {
        result.push_str(&format!("{}\n", truncate(issue, 100)));
    }

    if issues.len() > 20 {
        result.push_str(&format!("\n... +{} more issues\n", issues.len() - 20));
    }

    result.trim().to_string()
}

/// Compact file path (remove common prefixes)
fn compact_path(path: &str) -> String {
    // Remove common prefixes like /Users/..., /home/..., C:\
    let path = path.replace('\\', "/");

    if let Some(pos) = path.rfind("/src/") {
        format!("src/{}", &path[pos + 5..])
    } else if let Some(pos) = path.rfind("/lib/") {
        format!("lib/{}", &path[pos + 5..])
    } else if let Some(pos) = path.rfind('/') {
        path[pos + 1..].to_string()
    } else {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_eslint_json() {
        let json = r#"[
            {
                "filePath": "/Users/test/project/src/utils.ts",
                "messages": [
                    {
                        "ruleId": "prefer-const",
                        "severity": 1,
                        "message": "Use const instead of let",
                        "line": 10,
                        "column": 5
                    },
                    {
                        "ruleId": "prefer-const",
                        "severity": 1,
                        "message": "Use const instead of let",
                        "line": 15,
                        "column": 5
                    }
                ],
                "errorCount": 0,
                "warningCount": 2
            },
            {
                "filePath": "/Users/test/project/src/api.ts",
                "messages": [
                    {
                        "ruleId": "@typescript-eslint/no-unused-vars",
                        "severity": 2,
                        "message": "Variable x is unused",
                        "line": 20,
                        "column": 10
                    }
                ],
                "errorCount": 1,
                "warningCount": 0
            }
        ]"#;

        let result = filter_eslint_json(json);
        assert!(result.contains("ESLint:"));
        assert!(result.contains("prefer-const"));
        assert!(result.contains("no-unused-vars"));
        assert!(result.contains("src/utils.ts"));
    }

    #[test]
    fn test_compact_path() {
        assert_eq!(
            compact_path("/Users/foo/project/src/utils.ts"),
            "src/utils.ts"
        );
        assert_eq!(
            compact_path("C:\\Users\\project\\src\\api.ts"),
            "src/api.ts"
        );
        assert_eq!(compact_path("simple.ts"), "simple.ts");
    }

    #[test]
    fn test_filter_pylint_json_no_issues() {
        let output = "[]";
        let result = filter_pylint_json(output);
        assert!(result.contains("✓ Pylint"));
        assert!(result.contains("No issues found"));
    }

    #[test]
    fn test_filter_pylint_json_with_issues() {
        let json = r#"[
            {
                "type": "warning",
                "module": "main",
                "obj": "",
                "line": 10,
                "column": 0,
                "path": "src/main.py",
                "symbol": "unused-variable",
                "message": "Unused variable 'x'",
                "message-id": "W0612"
            },
            {
                "type": "warning",
                "module": "main",
                "obj": "foo",
                "line": 15,
                "column": 4,
                "path": "src/main.py",
                "symbol": "unused-variable",
                "message": "Unused variable 'y'",
                "message-id": "W0612"
            },
            {
                "type": "error",
                "module": "utils",
                "obj": "bar",
                "line": 20,
                "column": 0,
                "path": "src/utils.py",
                "symbol": "undefined-variable",
                "message": "Undefined variable 'z'",
                "message-id": "E0602"
            }
        ]"#;

        let result = filter_pylint_json(json);
        assert!(result.contains("3 issues"));
        assert!(result.contains("2 files"));
        assert!(result.contains("1 errors, 2 warnings"));
        assert!(result.contains("unused-variable (W0612)"));
        assert!(result.contains("undefined-variable (E0602)"));
        assert!(result.contains("main.py"));
        assert!(result.contains("utils.py"));
    }

    #[test]
    fn test_strip_pm_prefix_npx() {
        let args: Vec<String> = vec!["npx".into(), "eslint".into(), "src/".into()];
        assert_eq!(strip_pm_prefix(&args), 1);
    }

    #[test]
    fn test_strip_pm_prefix_bunx() {
        let args: Vec<String> = vec!["bunx".into(), "eslint".into(), ".".into()];
        assert_eq!(strip_pm_prefix(&args), 1);
    }

    #[test]
    fn test_strip_pm_prefix_pnpm_exec() {
        let args: Vec<String> = vec!["pnpm".into(), "exec".into(), "eslint".into()];
        assert_eq!(strip_pm_prefix(&args), 2);
    }

    #[test]
    fn test_strip_pm_prefix_none() {
        let args: Vec<String> = vec!["eslint".into(), "src/".into()];
        assert_eq!(strip_pm_prefix(&args), 0);
    }

    #[test]
    fn test_strip_pm_prefix_empty() {
        let args: Vec<String> = vec![];
        assert_eq!(strip_pm_prefix(&args), 0);
    }

    #[test]
    fn test_detect_linter_eslint() {
        let args: Vec<String> = vec!["eslint".into(), "src/".into()];
        let (linter, explicit) = detect_linter(&args);
        assert_eq!(linter, "eslint");
        assert!(explicit);
    }

    #[test]
    fn test_detect_linter_default_on_path() {
        let args: Vec<String> = vec!["src/".into()];
        let (linter, explicit) = detect_linter(&args);
        assert_eq!(linter, "eslint");
        assert!(!explicit);
    }

    #[test]
    fn test_detect_linter_default_on_flag() {
        let args: Vec<String> = vec!["--max-warnings=0".into()];
        let (linter, explicit) = detect_linter(&args);
        assert_eq!(linter, "eslint");
        assert!(!explicit);
    }

    #[test]
    fn test_detect_linter_after_npx_strip() {
        // Simulates: rtk lint npx eslint src/ → after strip_pm_prefix, args = ["eslint", "src/"]
        let full_args: Vec<String> = vec!["npx".into(), "eslint".into(), "src/".into()];
        let skip = strip_pm_prefix(&full_args);
        let effective = &full_args[skip..];
        let (linter, _) = detect_linter(effective);
        assert_eq!(linter, "eslint");
    }

    #[test]
    fn test_detect_linter_after_pnpm_exec_strip() {
        let full_args: Vec<String> =
            vec!["pnpm".into(), "exec".into(), "biome".into(), "check".into()];
        let skip = strip_pm_prefix(&full_args);
        let effective = &full_args[skip..];
        let (linter, _) = detect_linter(effective);
        assert_eq!(linter, "biome");
    }

    #[test]
    fn test_is_python_linter() {
        assert!(is_python_linter("ruff"));
        assert!(is_python_linter("pylint"));
        assert!(is_python_linter("mypy"));
        assert!(is_python_linter("flake8"));
        assert!(!is_python_linter("eslint"));
        assert!(!is_python_linter("biome"));
        assert!(!is_python_linter("unknown"));
    }
}
