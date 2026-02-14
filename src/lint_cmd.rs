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

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    // Detect linter name (first arg if not a path/flag, else default to eslint)
    let is_path_or_flag = args.is_empty()
        || args[0].starts_with('-')
        || args[0].contains('/')
        || args[0].contains('.');

    let linter = if is_path_or_flag { "eslint" } else { &args[0] };

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
            if !args.contains(&"--output-format".to_string()) {
                cmd.arg("check").arg("--output-format=json");
            }
        }
        "pylint" => {
            // Force JSON2 output for pylint
            if !args.contains(&"--output-format".to_string()) {
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
    let start_idx = if is_path_or_flag {
        0
    } else if linter == "ruff" && !args.is_empty() && args[0] == "ruff" {
        // Skip "ruff" and "check" if we already added "check"
        if args.len() > 1 && args[1] == "check" {
            2
        } else {
            1
        }
    } else {
        1
    };

    for arg in &args[start_idx..] {
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
        let has_path = args
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
        "mypy" => filter_mypy_output(&raw),
        _ => filter_generic_lint(&raw),
    };

    println!("{}", filtered);

    timer.track(
        &format!("{} {}", linter, args.join(" ")),
        &format!("rtk lint {} {}", linter, args.join(" ")),
        &raw,
        &filtered,
    );

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

/// Filter mypy text output - parse and group by error code and file
fn filter_mypy_output(output: &str) -> String {
    // Regex pattern: path/to/file.py:line: error: message [error-code]
    let re = Regex::new(r"^(.+\.py):(\d+): (error|warning|note): (.+?) \[(.+?)\]").unwrap();

    let mut issues: Vec<(String, String, String, String)> = Vec::new(); // (file, line, level, code)
    let mut errors = 0;
    let mut warnings = 0;
    let mut notes = 0;

    for line in output.lines() {
        if let Some(caps) = re.captures(line) {
            let file = caps.get(1).map_or("", |m| m.as_str());
            let line_num = caps.get(2).map_or("", |m| m.as_str());
            let level = caps.get(3).map_or("", |m| m.as_str());
            let code = caps.get(5).map_or("", |m| m.as_str());

            match level {
                "error" => errors += 1,
                "warning" => warnings += 1,
                "note" => notes += 1,
                _ => {}
            }

            issues.push((
                file.to_string(),
                line_num.to_string(),
                level.to_string(),
                code.to_string(),
            ));
        }
    }

    if issues.is_empty() {
        // Check if mypy output contains "Success" or similar
        if output.contains("Success") || output.trim().is_empty() {
            return "✓ Mypy: No issues found".to_string();
        }
        // Fallback to generic output if no regex matches
        return format!("Mypy output:\n{}", truncate(output, 500));
    }

    // Count unique files
    let unique_files: std::collections::HashSet<_> = issues.iter().map(|(f, _, _, _)| f).collect();
    let total_files = unique_files.len();

    // Group by error code
    let mut by_code: HashMap<String, usize> = HashMap::new();
    for (_, _, _, code) in &issues {
        *by_code.entry(code.clone()).or_insert(0) += 1;
    }

    // Group by file
    let mut by_file: HashMap<&str, usize> = HashMap::new();
    for (file, _, _, _) in &issues {
        *by_file.entry(file.as_str()).or_insert(0) += 1;
    }

    let mut file_counts: Vec<_> = by_file.iter().collect();
    file_counts.sort_by(|a, b| b.1.cmp(a.1));

    // Build output
    let mut result = String::new();
    result.push_str(&format!(
        "Mypy: {} issues in {} files\n",
        issues.len(),
        total_files
    ));

    if errors > 0 || warnings > 0 {
        result.push_str(&format!("  {} errors, {} warnings", errors, warnings));
        if notes > 0 {
            result.push_str(&format!(", {} notes", notes));
        }
        result.push('\n');
    }

    result.push_str("═══════════════════════════════════════\n");

    // Show top error codes
    let mut code_counts: Vec<_> = by_code.iter().collect();
    code_counts.sort_by(|a, b| b.1.cmp(a.1));

    if !code_counts.is_empty() {
        result.push_str("Top error codes:\n");
        for (code, count) in code_counts.iter().take(10) {
            result.push_str(&format!("  {} ({}x)\n", code, count));
        }
        result.push('\n');
    }

    // Show top files
    result.push_str("Top files:\n");
    for (file, count) in file_counts.iter().take(10) {
        let short_path = compact_path(file);
        result.push_str(&format!("  {} ({} issues)\n", short_path, count));

        // Show top 3 error codes in this file
        let mut file_codes: HashMap<String, usize> = HashMap::new();
        for (_f, _, _, code) in issues.iter().filter(|(f, _, _, _)| f == *file) {
            *file_codes.entry(code.clone()).or_insert(0) += 1;
        }

        let mut file_code_counts: Vec<_> = file_codes.iter().collect();
        file_code_counts.sort_by(|a, b| b.1.cmp(a.1));

        for (code, count) in file_code_counts.iter().take(3) {
            result.push_str(&format!("    {} ({})\n", code, count));
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
    fn test_filter_mypy_no_issues() {
        let output = "Success: no issues found in 5 source files";
        let result = filter_mypy_output(output);
        assert!(result.contains("✓ Mypy"));
        assert!(result.contains("No issues found"));
    }

    #[test]
    fn test_filter_mypy_with_errors() {
        let output = r#"src/main.py:10: error: Incompatible return value type [return-value]
src/main.py:15: error: Argument 1 has incompatible type "str"; expected "int" [arg-type]
src/utils.py:20: error: Name "foo" is not defined [name-defined]
src/utils.py:25: warning: Unused "type: ignore" comment [unused-ignore]
Found 4 errors in 2 files (checked 5 source files)"#;

        let result = filter_mypy_output(output);
        assert!(result.contains("4 issues"));
        assert!(result.contains("2 files"));
        assert!(result.contains("3 errors, 1 warnings"));
        assert!(result.contains("return-value"));
        assert!(result.contains("arg-type"));
        assert!(result.contains("name-defined"));
        assert!(result.contains("main.py"));
        assert!(result.contains("utils.py"));
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
