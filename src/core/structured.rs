//! Structured output formatter — converts regex-cleaned command output into
//! LLM-optimized, machine-readable, or human-friendly formats.
//!
//! # Design Principles
//! 1. Regex compresses (strips noise). Structured formats (restructures signal).
//! 2. Prefix characters (`F`, `E`, `W`, `C`) — zero ambiguity for LLMs.
//! 3. One-line summary always first — LLM knows outcome in one glance.
//! 4. Fail open — if parsing fails, return the original cleaned output unchanged.
//!
//! # Supported Command Types
//! - Test runners: jest, vitest, pytest, cargo test, npm/npx test, go test
//! - Build tools: cargo build, npm run build, next build
//! - VCS: git status/log/diff (future)
//! - Package managers: npm install, pip install (future)

use std::collections::HashMap;

/// Output mode for structured formatting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// Human-friendly: 1-2 sentence summary
    Human,
    /// LLM-optimized: prefix-tagged key-value format (lowest tokens, highest clarity)
    Llm,
    /// Machine-readable: JSON for downstream tooling
    Json,
    /// Ultra-compact: single line summary
    Ultra,
}

impl OutputMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "human" => Some(OutputMode::Human),
            "llm" => Some(OutputMode::Llm),
            "json" => Some(OutputMode::Json),
            "ultra" => Some(OutputMode::Ultra),
            _ => None,
        }
    }
}

/// A parsed test result from a regex-cleaned test runner output.
#[derive(Debug, Clone, Default)]
pub struct TestResult {
    pub tool: String,
    pub status: String,
    pub total_suites: Option<usize>,
    pub passed_suites: usize,
    pub failed_suites: usize,
    pub total_tests: Option<usize>,
    pub passed_tests: usize,
    pub failed_tests: usize,
    pub skipped_tests: usize,
    pub todo_tests: usize,
    pub failures: Vec<TestFailure>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
    pub coverage: HashMap<String, f64>,
}

#[derive(Debug, Clone, Default)]
pub struct TestFailure {
    pub file: String,
    pub test: String,
    pub expected: Option<String>,
    pub received: Option<String>,
    pub message: Option<String>,
}

/// Parse regex-cleaned test runner output into a structured `TestResult`.
/// Handles jest, vitest, pytest, cargo test, npm test output formats.
pub fn parse_test_output(tool: &str, cleaned: &str) -> TestResult {
    let mut result = TestResult {
        tool: tool.to_string(),
        ..Default::default()
    };

    let mut in_failures = false;
    let mut current_failure: Option<TestFailure> = None;
    let mut in_coverage = false;

    for line in cleaned.lines() {
        let s = line.trim();
        if s.is_empty() {
            if in_failures && current_failure.is_some() {
                result.failures.push(current_failure.take().unwrap());
            }
            in_failures = false;
            continue;
        }

        // -- Parse test suite / test summary lines --
        if s.starts_with("Test Suites:")
            || s.starts_with("Tests:")
            || s.starts_with("test result:")
        {
            parse_test_counts(s, &mut result);
        }

        // -- Detect failures section --
        if s.starts_with("FAIL") {
            in_failures = true;
            current_failure = Some(TestFailure::default());
            // Extract file and test name: "FAIL src/api/client.test.ts (3 tests)"
            if let Some(stripped) = s.strip_prefix("FAIL ") {
                let parts: Vec<&str> = stripped.split('(').collect();
                if !parts.is_empty() {
                    let file_path = parts[0].trim();
                    result.failures.last_mut().map(|f| f.file = file_path.to_string());
                }
            }
            continue;
        }

        // -- Failure details --
        if in_failures {
            if let Some(ref mut f) = current_failure {
                if s.starts_with("●") {
                    // "● handles timeout correctly"
                    f.test = s.trim_start_matches('●').trim().to_string();
                } else if s.contains("Expected:") {
                    f.expected = Some(s.trim().to_string());
                } else if s.contains("Received:") {
                    f.received = Some(s.trim().to_string());
                } else if s.starts_with("Expected") {
                    f.expected = Some(s.trim().to_string());
                } else if s.starts_with("Received") {
                    f.received = Some(s.trim().to_string());
                } else if s.starts_with(">") || s.contains(".py:") || s.contains(".ts:") {
                    // file:line reference
                    if f.message.is_none() {
                        f.message = Some(s.trim().to_string());
                    }
                }
            }
        }

        // -- Warnings / Errors --
        if s.starts_with("[WARN]") || s.contains("[WARN]") {
            result.warnings.push(s.to_string());
        }
        if s.starts_with("[ERROR]") || s.contains("[ERROR]") {
            result.errors.push(s.to_string());
        }

        // -- Coverage --
        if s.contains("Coverage summary") || s.contains("coverage") {
            in_coverage = true;
            continue;
        }
        if in_coverage && s.starts_with("===") {
            in_coverage = false;
            continue;
        }
        if in_coverage || s.starts_with("Statements") || s.starts_with("Branches")
            || s.starts_with("Functions") || s.starts_with("Lines")
        {
            parse_coverage_line(s, &mut result.coverage);
        }
        if s.starts_with("cov:") {
            // Ultra-compact coverage: "cov: stmt:87% br:74% fn:92% ln:89%"
            parse_compact_coverage(s, &mut result.coverage);
        }
    }

    // Determine status
    if result.failed_tests > 0 || result.failed_suites > 0 {
        result.status = "FAIL".to_string();
    } else if result.passed_tests > 0 || result.passed_suites > 0 {
        result.status = "PASS".to_string();
    } else {
        result.status = "UNKNOWN".to_string();
    }

    result
}

fn parse_test_counts(line: &str, result: &mut TestResult) {
    // "Test Suites: 1 failed, 46 passed, 47 total"
    // "Tests:       2 failed, 307 passed, 314 total"
    // "test result: ok. 142 passed; 0 failed; 0 ignored"
    let lower = line.to_lowercase();

    for part in line.split(',') {
        let words: Vec<&str> = part.split_whitespace().collect();
        for (i, word) in words.iter().enumerate() {
            if i == 0 {
                continue;
            }
            let Ok(n) = words[i - 1].parse::<usize>() else {
                continue;
            };
            if word.contains("failed") {
                result.failed_tests = n;
            } else if word.contains("passed") {
                result.passed_tests = n;
            } else if word.contains("skipped") {
                result.skipped_tests = n;
            } else if word.contains("todo") {
                result.todo_tests = n;
            }
        }
    }

    // Suites
    if lower.contains("suites") {
        for part in line.split(',') {
            let words: Vec<&str> = part.split_whitespace().collect();
            for (i, word) in words.iter().enumerate() {
                if i == 0 {
                    continue;
                }
                let Ok(n) = words[i - 1].parse::<usize>() else {
                    continue;
                };
                if word.contains("failed") {
                    result.failed_suites = n;
                } else if word.contains("passed") {
                    result.passed_suites = n;
                } else if word.contains("total") {
                    result.total_suites = Some(n);
                }
            }
        }
    }
}

fn parse_coverage_line(line: &str, coverage: &mut HashMap<String, f64>) {
    // "Statements   : 87.2% ( 1423/1632 )"
    let parts: Vec<&str> = line.split(':').collect();
    if parts.len() < 2 {
        return;
    }
    let key = parts[0].trim().to_lowercase();
    let pct_str = parts[1].trim();
    if let Some(pct) = pct_str.split('%').next() {
        if let Ok(val) = pct.trim().parse::<f64>() {
            coverage.insert(key, val);
        }
    }
}

fn parse_compact_coverage(line: &str, coverage: &mut HashMap<String, f64>) {
    // "cov: stmt:87% br:74% fn:92% ln:89%"
    for tok in line.split_whitespace().skip(1) {
        let kv: Vec<&str> = tok.split(':').collect();
        if kv.len() == 2 {
            if let Some(pct) = kv[1].strip_suffix('%') {
                if let Ok(val) = pct.parse::<f64>() {
                    coverage.insert(kv[0].to_string(), val);
                }
            }
        }
    }
}

// ────────────────────────────────────────────────────────────
// Formatting functions
// ────────────────────────────────────────────────────────────

/// Format a TestResult into the specified output mode.
pub fn format_test_result(result: &TestResult, mode: OutputMode) -> String {
    match mode {
        OutputMode::Human => format_human(result),
        OutputMode::Llm => format_llm(result),
        OutputMode::Json => format_json(result),
        OutputMode::Ultra => format_ultra(result),
    }
}

fn format_human(result: &TestResult) -> String {
    let mut out = String::new();

    if result.failed_tests > 0 {
        out.push_str(&format!(
            "{} test suites failed. {} of {} tests passed.\n",
            result.failed_suites, result.passed_tests,
            result.total_tests.unwrap_or(result.passed_tests + result.failed_tests)
        ));
        if let Some(f) = result.failures.first() {
            out.push_str(&format!(
                "Main issue: {}::{}",
                f.file, f.test
            ));
            if let Some(ref exp) = f.expected {
                out.push_str(&format!(" — {}", exp));
            }
            if let Some(ref rec) = f.received {
                out.push_str(&format!(", {}", rec));
            }
            out.push('\n');
        }
    } else {
        out.push_str(&format!(
            "All {} tests passed in {} suites.\n",
            result.passed_tests, result.passed_suites
        ));
    }

    if !result.warnings.is_empty() {
        out.push_str(&format!("\n{} warnings (use --verbose for details)\n", result.warnings.len()));
    }

    if let Some(cov) = result.coverage.get("lines") {
        out.push_str(&format!("\nCoverage: {}% lines\n", cov));
    }

    out
}

fn format_llm(result: &TestResult) -> String {
    let mut out = String::new();

    // Summary line: RTK jest 2F/307P 46/47suites cov:89%
    let total = result.total_tests.unwrap_or(result.passed_tests + result.failed_tests);
    let total_suites = result.total_suites.unwrap_or(result.passed_suites + result.failed_suites);
    out.push_str(&format!(
        "RTK {} {}{}F/{}P {}/{}suites",
        result.tool,
        if result.status == "FAIL" { "❌ " } else { "" },
        result.failed_tests,
        result.passed_tests,
        result.passed_suites,
        total_suites,
    ));
    if let Some(cov) = result.coverage.get("lines") {
        out.push_str(&format!(" cov:{}%", cov));
    }
    out.push('\n');

    // Failures
    for f in &result.failures {
        out.push_str(&format!(
            "F  {}::{}",
            f.file, f.test
        ));
        if let Some(ref exp) = f.expected {
            out.push_str(&format!(" → {}", exp));
        }
        if let Some(ref rec) = f.received {
            out.push_str(&format!(" got {}", rec));
        }
        out.push('\n');
    }

    // Errors
    for e in &result.errors {
        out.push_str(&format!("E  {}\n", e.trim_start_matches("[ERROR]").trim()));
    }

    // Warnings
    for w in &result.warnings {
        out.push_str(&format!("W  {}\n", w.trim_start_matches("[WARN]").trim()));
    }

    // Coverage
    if !result.coverage.is_empty() {
        out.push_str("C  ");
        let mut cov_parts: Vec<String> = Vec::new();
        for key in &["statements", "branches", "functions", "lines"] {
            if let Some(val) = result.coverage.get(*key) {
                let short = &key[..2.min(key.len())];
                cov_parts.push(format!("{}:{}%", short, val));
            }
        }
        out.push_str(&cov_parts.join(" "));
        out.push('\n');
    }

    out.trim().to_string()
}

fn format_json(result: &TestResult) -> String {
    let mut map = serde_json::Map::new();
    map.insert("tool".into(), serde_json::Value::String(result.tool.clone()));
    map.insert("status".into(), serde_json::Value::String(result.status.clone()));
    map.insert("total_suites".into(), serde_json::json!(result.total_suites));
    map.insert("passed_suites".into(), serde_json::json!(result.passed_suites));
    map.insert("failed_suites".into(), serde_json::json!(result.failed_suites));
    map.insert("passed_tests".into(), serde_json::json!(result.passed_tests));
    map.insert("failed_tests".into(), serde_json::json!(result.failed_tests));
    map.insert("skipped_tests".into(), serde_json::json!(result.skipped_tests));

    let failures: Vec<serde_json::Value> = result.failures.iter().map(|f| {
        let mut fm = serde_json::Map::new();
        fm.insert("file".into(), serde_json::Value::String(f.file.clone()));
        fm.insert("test".into(), serde_json::Value::String(f.test.clone()));
        if let Some(ref e) = f.expected { fm.insert("expected".into(), serde_json::Value::String(e.clone())); }
        if let Some(ref r) = f.received { fm.insert("received".into(), serde_json::Value::String(r.clone())); }
        if let Some(ref m) = f.message { fm.insert("message".into(), serde_json::Value::String(m.clone())); }
        serde_json::Value::Object(fm)
    }).collect();
    map.insert("failures".into(), serde_json::Value::Array(failures));

    let warnings: Vec<serde_json::Value> = result.warnings.iter()
        .map(|w| serde_json::Value::String(w.clone())).collect();
    map.insert("warnings".into(), serde_json::Value::Array(warnings));

    let errors: Vec<serde_json::Value> = result.errors.iter()
        .map(|e| serde_json::Value::String(e.clone())).collect();
    map.insert("errors".into(), serde_json::Value::Array(errors));

    let coverage: serde_json::Map<String, serde_json::Value> = result.coverage.iter()
        .map(|(k, v)| (k.clone(), serde_json::json!(v)))
        .collect();
    map.insert("coverage".into(), serde_json::Value::Object(coverage));

    serde_json::to_string_pretty(&serde_json::Value::Object(map)).unwrap_or_default()
}

fn format_ultra(result: &TestResult) -> String {
    let mut out = format!("{}:", result.tool);
    if result.failed_tests > 0 {
        out.push_str(&format!("❌ {}F/{}P", result.failed_tests, result.passed_tests));
        if let Some(f) = result.failures.first() {
            out.push_str(&format!(" {}::{}", f.file, f.test));
        }
    } else {
        out.push_str(&format!("✅ {}P", result.passed_tests));
    }
    if let Some(cov) = result.coverage.get("lines") {
        out.push_str(&format!(" cov:{}%", cov));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_jest_failure_output() {
        let cleaned = r#"Test Suites: 1 failed, 46 passed, 47 total
Tests:       2 failed, 307 passed, 314 total
PASS  src/components/Button.test.tsx (24 tests)
PASS  src/utils/format.test.ts (18 tests)
[WARN] Deprecated API: useLegacyAuth() will be removed in v3.0
FAIL  src/api/client.test.ts (3 tests)
● handles timeout correctly
Expected: 200
Received: 408
PASS  src/hooks/useAuth.test.ts (12 tests)
Statements   : 87.2% ( 1423/1632 )
Branches     : 74.1% ( 623/840 )
Functions    : 92.3% ( 241/261 )
Lines        : 88.9% ( 1387/1560 )
"#;

        let result = parse_test_output("jest", cleaned);

        assert_eq!(result.status, "FAIL");
        assert_eq!(result.failed_tests, 2);
        assert_eq!(result.passed_tests, 307);
        assert_eq!(result.failed_suites, 1);
        assert_eq!(result.passed_suites, 46);
        assert_eq!(result.failures.len(), 1);
        assert_eq!(result.failures[0].file, "src/api/client.test.ts");
        assert_eq!(result.failures[0].test, "handles timeout correctly");
        assert_eq!(result.warnings.len(), 1);
        assert!((result.coverage["lines"] - 88.9).abs() < 0.01);
        assert!((result.coverage["statements"] - 87.2).abs() < 0.01);
    }

    #[test]
    fn test_format_llm_jest_failure() {
        let cleaned = r#"Test Suites: 1 failed, 46 passed, 47 total
Tests:       2 failed, 307 passed, 314 total
FAIL  src/api/client.test.ts (3 tests)
● handles timeout correctly
Expected: 200
Received: 408
[WARN] Deprecated API: useLegacyAuth() will be removed in v3.0
Statements   : 87.2% ( 1423/1632 )
Lines        : 88.9% ( 1387/1560 )
"#;

        let result = parse_test_output("jest", cleaned);
        let formatted = format_llm(&result);

        assert!(formatted.starts_with("RTK jest"), "Got: {}", formatted);
        assert!(formatted.contains("2F/307P"), "Got: {}", formatted);
        assert!(formatted.contains("F  src/api/client.test.ts::handles timeout correctly"),
            "Got: {}", formatted);
        assert!(formatted.contains("→ Expected: 200"), "Got: {}", formatted);
        assert!(formatted.contains("got Received: 408"), "Got: {}", formatted);
        assert!(formatted.contains("W  Deprecated API"), "Got: {}", formatted);
        assert!(formatted.contains("C  "), "Got: {}", formatted);
    }

    #[test]
    fn test_format_llm_all_pass() {
        let cleaned = r#"Test Suites: 47 passed, 47 total
Tests:       307 passed, 307 total
PASS  src/App.test.tsx
Statements   : 87.2%
Lines        : 88.9%
"#;
        let result = parse_test_output("jest", cleaned);
        let formatted = format_llm(&result);

        assert_eq!(result.status, "PASS");
        assert!(formatted.contains("0F/307P"), "Got: {}", formatted);
        assert!(!formatted.contains("F  "), "Should have no failures");
    }

    #[test]
    fn test_format_ultra() {
        let cleaned = r#"Tests:       2 failed, 307 passed, 314 total
FAIL  src/api/client.test.ts
● handles timeout correctly
Lines        : 88.9%
"#;
        let result = parse_test_output("jest", cleaned);
        let formatted = format_ultra(&result);

        assert!(formatted.contains("jest:"));
        assert!(formatted.contains("2F/307P"));
        assert!(formatted.contains("cov:89%"));
    }

    #[test]
    fn test_output_modes_parse() {
        assert_eq!(OutputMode::from_str("human"), Some(OutputMode::Human));
        assert_eq!(OutputMode::from_str("LLM"), Some(OutputMode::Llm));
        assert_eq!(OutputMode::from_str("json"), Some(OutputMode::Json));
        assert_eq!(OutputMode::from_str("ultra"), Some(OutputMode::Ultra));
        assert_eq!(OutputMode::from_str("invalid"), None);
    }
}
