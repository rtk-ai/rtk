use crate::tracking;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::BTreeMap;
use std::process::Command;

/// Maximum number of files shown per rule before truncation.
const MAX_FILES_PER_RULE: usize = 5;

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("swiftlint");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: swiftlint {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run swiftlint (is it installed? Try: brew install swiftlint)")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = if verbose > 0 {
        filter_swiftlint_verbose(&raw)
    } else {
        let result = filter_swiftlint(&raw);
        // Fallback to raw output if filter produces empty result
        if result.is_empty() && !raw.trim().is_empty() {
            eprintln!("rtk: swiftlint filter produced empty output, showing raw");
            raw.trim().to_string()
        } else {
            result
        }
    };

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });

    if let Some(hint) = crate::tee::tee_and_hint(&raw, "swiftlint", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("swiftlint {}", args.join(" ")),
        &format!("rtk swiftlint {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

// === Regex helpers (lazy_static for repo consistency) ===

lazy_static! {
    /// Regex matching "Linting 'Foo.swift' (N/M)" or "Correcting 'Foo.swift' (N/M)" progress lines.
    static ref PROGRESS_RE: Regex =
        Regex::new(r"^(?:Linting|Correcting) '.+\.swift' \(\d+/\d+\)$").unwrap();

    /// Regex matching SwiftLint violation lines:
    /// /path/to/File.swift:LINE:COL: warning|error: Message (rule_id)
    static ref VIOLATION_RE: Regex =
        Regex::new(r"^(.+?):(\d+):(\d+): (warning|error): (.+)$").unwrap();

    /// Regex to extract file count from summary line.
    static ref FILE_COUNT_RE: Regex =
        Regex::new(r"in (\d+) files").unwrap();
}

// === Utility functions ===

/// Extract rule_id from the end of a SwiftLint violation message.
/// e.g., "Line Length Violation: ... (line_length)" -> "line_length"
fn extract_rule_id(message: &str) -> &str {
    if let Some(start) = message.rfind('(') {
        if message.ends_with(')') {
            return &message[start + 1..message.len() - 1];
        }
    }
    message
}

/// Format compact file:line locations for a single rule.
/// Files beyond MAX_FILES_PER_RULE are truncated with "+N more".
fn format_rule_locations(files: &BTreeMap<String, Vec<String>>) -> String {
    let total = files.len();
    let mut parts = Vec::new();
    for (i, (file, lines)) in files.iter().enumerate() {
        if i >= MAX_FILES_PER_RULE {
            parts.push(format!("+{} more", total - MAX_FILES_PER_RULE));
            break;
        }
        parts.push(format!("{}:{}", file, lines.join(",")));
    }
    parts.join(" ")
}

/// Sort rules by violation count (descending), then name (ascending) for determinism.
fn sorted_rules(
    rules: &BTreeMap<String, BTreeMap<String, Vec<String>>>,
) -> Vec<(&String, &BTreeMap<String, Vec<String>>)> {
    let mut sorted: Vec<_> = rules.iter().collect();
    sorted.sort_by(|a, b| {
        let count_a: usize = a.1.values().map(|v| v.len()).sum();
        let count_b: usize = b.1.values().map(|v| v.len()).sum();
        count_b.cmp(&count_a).then_with(|| a.0.cmp(b.0))
    });
    sorted
}

// === Main filter ===

/// Aggressively compress swiftlint output: group by rule, strip verbose messages,
/// compact file:line references. Matches git-status style compression for maximum
/// token savings.
pub fn filter_swiftlint(output: &str) -> String {
    let mut summary: Option<String> = None;
    // rule_id -> BTreeMap<filename, Vec<line_num>>
    let mut errors_by_rule: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    let mut warnings_by_rule: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    let mut total_errors: usize = 0;
    let mut total_warnings: usize = 0;

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            continue;
        }

        // Skip header and config lines
        if trimmed.starts_with("Linting Swift files")
            || trimmed.starts_with("Correcting Swift files")
            || trimmed.starts_with("Loading configuration from")
        {
            continue;
        }

        // Strip progress lines
        if PROGRESS_RE.is_match(trimmed) {
            continue;
        }

        // Capture summary (used only for file count extraction)
        if trimmed.starts_with("Done linting!") || trimmed.starts_with("Done correcting!") {
            summary = Some(trimmed.to_string());
            continue;
        }

        // Parse violation lines
        if let Some(caps) = VIOLATION_RE.captures(trimmed) {
            let full_path = &caps[1];
            let line_num = &caps[2];
            let severity = &caps[4];
            let message = &caps[5];

            let filename = full_path.rsplit('/').next().unwrap_or(full_path);
            let rule_id = extract_rule_id(message);

            let target = match severity {
                "error" => {
                    total_errors += 1;
                    &mut errors_by_rule
                }
                "warning" => {
                    total_warnings += 1;
                    &mut warnings_by_rule
                }
                _ => continue,
            };

            target
                .entry(rule_id.to_string())
                .or_default()
                .entry(filename.to_string())
                .or_default()
                .push(line_num.to_string());
            continue;
        }

        // All other lines (config errors, rule identifiers, etc.) silently skipped
    }

    // Empty input
    if total_errors == 0 && total_warnings == 0 && summary.is_none() {
        return String::new();
    }

    // Extract file count from summary
    let file_count = summary
        .as_ref()
        .and_then(|s| FILE_COUNT_RE.captures(s))
        .and_then(|caps| caps[1].parse::<usize>().ok());
    let files_str = file_count
        .map(|n| format!(" ({} files)", n))
        .unwrap_or_default();

    // Clean run (summary present but no violations matched)
    if total_errors == 0 && total_warnings == 0 {
        return format!("✓ SwiftLint: clean{}", files_str);
    }

    let mut result = String::new();

    // Header
    result.push_str(&format!(
        "⚠️ SwiftLint: {} warnings, {} errors{}\n",
        total_warnings, total_errors, files_str
    ));

    // Errors section
    if !errors_by_rule.is_empty() {
        result.push_str(&format!("\n❌ Errors ({}):\n", total_errors));
        for (rule, files) in sorted_rules(&errors_by_rule) {
            let count: usize = files.values().map(|v| v.len()).sum();
            result.push_str(&format!(
                "  {} ({}): {}\n",
                rule,
                count,
                format_rule_locations(files)
            ));
        }
    }

    // Warnings section
    if !warnings_by_rule.is_empty() {
        result.push_str(&format!("\n⚠️ Warnings ({}):\n", total_warnings));
        for (rule, files) in sorted_rules(&warnings_by_rule) {
            let count: usize = files.values().map(|v| v.len()).sum();
            result.push_str(&format!(
                "  {} ({}): {}\n",
                rule,
                count,
                format_rule_locations(files)
            ));
        }
    }

    result.trim().to_string()
}

/// Verbose mode: returns raw swiftlint output unchanged for debugging.
fn filter_swiftlint_verbose(output: &str) -> String {
    output.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    // === Format tests ===

    #[test]
    fn test_filter_empty_input() {
        let result = filter_swiftlint("");
        assert!(result.is_empty(), "expected empty, got: {}", result);
    }

    #[test]
    fn test_filter_vapor_warnings_only() {
        let input = include_str!("../tests/fixtures/swiftlint_gh_vapor_warnings_only.txt");
        let result = filter_swiftlint(input);

        // Header with counts and file count
        assert!(
            result.contains("22 warnings, 0 errors (342 files)"),
            "missing header: {}",
            result
        );

        // No progress lines
        assert!(
            !result.contains("Linting 'Application.swift'"),
            "contains progress line: {}",
            result
        );
        assert!(
            !result.contains("(1/342)"),
            "contains progress counter: {}",
            result
        );

        // Rules grouped
        assert!(
            result.contains("line_length"),
            "missing line_length rule: {}",
            result
        );
        assert!(
            result.contains("trailing_whitespace"),
            "missing trailing_whitespace rule: {}",
            result
        );
        assert!(
            result.contains("vertical_whitespace"),
            "missing vertical_whitespace rule: {}",
            result
        );

        // Warnings section present, errors section absent
        assert!(
            result.contains("⚠️ Warnings"),
            "missing warnings section: {}",
            result
        );
        assert!(
            !result.contains("❌ Errors"),
            "should not have errors section: {}",
            result
        );
    }

    #[test]
    fn test_filter_alamofire_many_violations() {
        let input = include_str!("../tests/fixtures/swiftlint_gh_alamofire_many_violations.txt");
        let result = filter_swiftlint(input);

        // Header
        assert!(
            result.contains("98 warnings, 12 errors (85 files)"),
            "missing header: {}",
            result
        );

        // Errors section with key rules
        assert!(
            result.contains("❌ Errors (12):"),
            "missing errors section: {}",
            result
        );
        assert!(
            result.contains("file_length"),
            "missing file_length rule: {}",
            result
        );
        assert!(
            result.contains("cyclomatic_complexity"),
            "missing cyclomatic_complexity: {}",
            result
        );
        assert!(
            result.contains("function_body_length"),
            "missing function_body_length: {}",
            result
        );
        assert!(
            result.contains("type_body_length"),
            "missing type_body_length: {}",
            result
        );

        // Warnings section with key rules
        assert!(
            result.contains("⚠️ Warnings (98):"),
            "missing warnings section: {}",
            result
        );
        assert!(
            result.contains("force_cast"),
            "missing force_cast rule: {}",
            result
        );
        assert!(
            result.contains("force_try"),
            "missing force_try rule: {}",
            result
        );

        // No full paths in output
        assert!(
            !result.contains("/Users/ci/"),
            "contains full path: {}",
            result
        );

        // No progress lines
        assert!(
            !result.contains("Linting '"),
            "contains progress line: {}",
            result
        );

        // No verbose violation messages
        assert!(
            !result.contains("Line should be 120 characters"),
            "contains verbose message: {}",
            result
        );
    }

    #[test]
    fn test_filter_strips_progress_interleaved() {
        let input = include_str!("../tests/fixtures/swiftlint_gh_rxswift_interleaved.txt");
        let result = filter_swiftlint(input);

        // Progress lines stripped
        assert!(
            !result.contains("Linting 'Observable.swift'"),
            "contains progress: {}",
            result
        );
        assert!(
            !result.contains("(1/456)"),
            "contains progress counter: {}",
            result
        );
        assert!(
            !result.contains("(100/456)"),
            "contains progress counter: {}",
            result
        );

        // Rules present
        assert!(
            result.contains("line_length"),
            "missing line_length: {}",
            result
        );
        assert!(
            result.contains("identifier_name"),
            "missing identifier_name: {}",
            result
        );

        // No full paths
        assert!(
            !result.contains("/Users/runner/work/RxSwift/"),
            "contains full path: {}",
            result
        );

        // File count from summary
        assert!(
            result.contains("456 files"),
            "missing file count: {}",
            result
        );
    }

    #[test]
    fn test_filter_preserves_all_errors() {
        let input = include_str!("../tests/fixtures/swiftlint_gh_kingfisher_strict_mode.txt");
        let result = filter_swiftlint(input);

        // All violations are errors in strict mode
        assert!(
            result.contains("49 errors"),
            "missing error count: {}",
            result
        );
        assert!(
            result.contains("0 warnings"),
            "missing warning count: {}",
            result
        );

        // Key rules preserved
        assert!(
            result.contains("force_unwrapping"),
            "missing force_unwrapping: {}",
            result
        );
        assert!(
            result.contains("force_cast"),
            "missing force_cast: {}",
            result
        );
        assert!(
            result.contains("file_length"),
            "missing file_length: {}",
            result
        );
        assert!(
            result.contains("cyclomatic_complexity"),
            "missing cyclomatic_complexity: {}",
            result
        );
        assert!(
            result.contains("line_length"),
            "missing line_length: {}",
            result
        );
        assert!(
            result.contains("function_body_length"),
            "missing function_body_length: {}",
            result
        );

        // Errors section only, no warnings section
        assert!(
            result.contains("❌ Errors (49):"),
            "missing errors section: {}",
            result
        );
        assert!(
            !result.contains("⚠️ Warnings"),
            "should not have warnings section: {}",
            result
        );
    }

    #[test]
    fn test_filter_strips_paths_alamofire() {
        let input = include_str!("../tests/fixtures/swiftlint_gh_alamofire_violations.txt");
        let result = filter_swiftlint(input);

        // No full paths
        assert!(
            !result.contains("/Users/runner/work/Alamofire/"),
            "contains full path: {}",
            result
        );

        // Filenames present in rule locations
        assert!(
            result.contains("Session.swift"),
            "missing filename: {}",
            result
        );
        assert!(
            result.contains("Request.swift"),
            "missing filename: {}",
            result
        );

        // Config loading line stripped
        assert!(
            !result.contains("Loading configuration from"),
            "config loading line should be stripped: {}",
            result
        );
    }

    #[test]
    fn test_filter_groups_by_rule() {
        let input = include_str!("../tests/fixtures/swiftlint_gh_alamofire_many_violations.txt");
        let result = filter_swiftlint(input);

        // file_length: 7 errors across 7 files (truncated to 5 + "+2 more")
        assert!(
            result.contains("file_length (7):"),
            "missing file_length count: {}",
            result
        );
        let file_length_line = result
            .lines()
            .find(|l| l.contains("file_length (7)"))
            .expect("file_length line not found");
        assert!(
            file_length_line.contains("AFError.swift:350"),
            "missing AFError in file_length: {}",
            file_length_line
        );
        assert!(
            file_length_line.contains("+2 more"),
            "missing truncation: {}",
            file_length_line
        );

        // cyclomatic_complexity: 3 errors, all shown (under MAX_FILES_PER_RULE)
        assert!(
            result.contains("cyclomatic_complexity (3):"),
            "missing cyclomatic_complexity count: {}",
            result
        );
        let cc_line = result
            .lines()
            .find(|l| l.contains("cyclomatic_complexity"))
            .expect("cyclomatic_complexity line not found");
        assert!(
            cc_line.contains("DownloadRequest.swift:78"),
            "missing DownloadRequest: {}",
            cc_line
        );
        assert!(
            cc_line.contains("ResponseSerialization.swift:278"),
            "missing ResponseSerialization: {}",
            cc_line
        );
        assert!(
            cc_line.contains("Validation.swift:100"),
            "missing Validation: {}",
            cc_line
        );

        // Same-file line numbers are comma-separated
        // AFError.swift has many line_length warnings: 23,45,78,112,145,178,212,256,289,312
        let ll_line = result
            .lines()
            .find(|l| l.contains("line_length") && l.contains("AFError.swift"))
            .expect("AFError.swift missing from line_length");
        // Multiple lines from same file should be comma-separated
        assert!(
            ll_line.contains("AFError.swift:23,45,78,112,145,178,212,256,289,312"),
            "AFError.swift lines not comma-separated: {}",
            ll_line
        );
        // Truncation indicator for rules with many files
        assert!(
            ll_line.contains("+"),
            "line_length should show truncation: {}",
            ll_line
        );
    }

    #[test]
    fn test_filter_config_error() {
        let input = include_str!("../tests/fixtures/swiftlint_gh_realm_configerror.txt");
        let result = filter_swiftlint(input);

        // Config error lines stripped
        assert!(
            !result.contains("configuration error:"),
            "config error in output: {}",
            result
        );
        assert!(
            !result.contains("Valid rule identifiers:"),
            "rule identifiers in output: {}",
            result
        );

        // Rules from violations present
        assert!(
            result.contains("line_length"),
            "missing line_length: {}",
            result
        );
        assert!(
            result.contains("force_cast"),
            "missing force_cast: {}",
            result
        );
        assert!(
            result.contains("identifier_name"),
            "missing identifier_name: {}",
            result
        );

        // Correct error count (4 errors)
        assert!(
            result.contains("4 errors"),
            "missing error count: {}",
            result
        );

        // File count from summary
        assert!(
            result.contains("370 files"),
            "missing file count: {}",
            result
        );
    }

    #[test]
    fn test_filter_minimal_project() {
        let input = include_str!("../tests/fixtures/swiftlint_gh_snapkit_minimal.txt");
        let result = filter_swiftlint(input);

        // 3 warnings, 0 errors
        assert!(
            result.contains("3 warnings, 0 errors (23 files)"),
            "missing header: {}",
            result
        );

        // All 3 are line_length
        assert!(
            result.contains("line_length (3):"),
            "missing line_length count: {}",
            result
        );

        // Two files present
        assert!(
            result.contains("ConstraintMaker.swift"),
            "missing file: {}",
            result
        );
        assert!(
            result.contains("ConstraintMakerRelatable.swift"),
            "missing file: {}",
            result
        );

        // No errors section
        assert!(
            !result.contains("❌ Errors"),
            "should not have errors section: {}",
            result
        );
    }

    #[test]
    fn test_verbose_mode() {
        let input = include_str!("../tests/fixtures/swiftlint_gh_rxswift_interleaved.txt");
        let result = filter_swiftlint_verbose(input);

        // Verbose mode preserves progress lines
        assert!(
            result.contains("Linting 'Observable.swift' (1/456)"),
            "verbose mode should include progress lines: {}",
            result
        );
    }

    // === Utility function tests ===

    #[test]
    fn test_extract_rule_id() {
        assert_eq!(
            extract_rule_id("Line Length Violation: Line should be 120 characters or less; currently it has 167 characters (line_length)"),
            "line_length"
        );
        assert_eq!(
            extract_rule_id("Force Cast Violation: Force casts should be avoided (force_cast)"),
            "force_cast"
        );
        assert_eq!(
            extract_rule_id(
                "Variable name 'e' should be between 3 and 40 characters long (identifier_name)"
            ),
            "identifier_name"
        );
        // No rule_id in parens - returns full message
        assert_eq!(
            extract_rule_id("Some violation without rule id"),
            "Some violation without rule id"
        );
    }

    #[test]
    fn test_format_rule_locations_under_max() {
        let mut files = BTreeMap::new();
        files
            .entry("Foo.swift".to_string())
            .or_insert_with(Vec::new)
            .push("23".to_string());
        files
            .entry("Bar.swift".to_string())
            .or_insert_with(Vec::new)
            .extend(vec!["34".to_string(), "56".to_string()]);

        let result = format_rule_locations(&files);
        assert_eq!(result, "Bar.swift:34,56 Foo.swift:23");
    }

    #[test]
    fn test_format_rule_locations_over_max() {
        let mut files = BTreeMap::new();
        for i in 0..7 {
            files
                .entry(format!("File{}.swift", i))
                .or_insert_with(Vec::new)
                .push(format!("{}", i * 10));
        }

        let result = format_rule_locations(&files);
        assert!(result.contains("+2 more"), "missing truncation: {}", result);
        // First 5 files shown (alphabetical: File0..File4)
        assert!(
            result.contains("File0.swift:0"),
            "missing File0: {}",
            result
        );
        assert!(
            result.contains("File4.swift:40"),
            "missing File4: {}",
            result
        );
        // File5, File6 truncated
        assert!(
            !result.contains("File5.swift"),
            "File5 should be truncated: {}",
            result
        );
    }

    // === Token savings tests ===

    #[test]
    fn test_token_savings_vapor() {
        let input = include_str!("../tests/fixtures/swiftlint_gh_vapor_warnings_only.txt");
        let result = filter_swiftlint(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 80.0,
            "swiftlint vapor: expected >=80% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_token_savings_alamofire() {
        let input = include_str!("../tests/fixtures/swiftlint_gh_alamofire_many_violations.txt");
        let result = filter_swiftlint(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 75.0,
            "swiftlint alamofire: expected >=75% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_token_savings_all_fixtures() {
        let fixtures: Vec<(&str, &str)> = vec![
            (
                "alamofire_many",
                include_str!("../tests/fixtures/swiftlint_gh_alamofire_many_violations.txt"),
            ),
            (
                "alamofire_violations",
                include_str!("../tests/fixtures/swiftlint_gh_alamofire_violations.txt"),
            ),
            (
                "kingfisher_strict",
                include_str!("../tests/fixtures/swiftlint_gh_kingfisher_strict_mode.txt"),
            ),
            (
                "realm_configerror",
                include_str!("../tests/fixtures/swiftlint_gh_realm_configerror.txt"),
            ),
            (
                "rxswift_interleaved",
                include_str!("../tests/fixtures/swiftlint_gh_rxswift_interleaved.txt"),
            ),
            (
                "snapkit_minimal",
                include_str!("../tests/fixtures/swiftlint_gh_snapkit_minimal.txt"),
            ),
            (
                "vapor_warnings",
                include_str!("../tests/fixtures/swiftlint_gh_vapor_warnings_only.txt"),
            ),
        ];

        for (name, input) in &fixtures {
            let result = filter_swiftlint(input);

            let input_tokens = count_tokens(input);
            let output_tokens = count_tokens(&result);

            assert!(input_tokens > 0, "fixture {} has no tokens in input", name);
            assert!(!result.is_empty(), "fixture {} produced empty output", name);

            let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

            // All fixtures should achieve 60%+ savings with the new aggressive filter
            assert!(
                savings >= 60.0,
                "swiftlint {} filter: expected >=60% savings, got {:.1}% ({} -> {} tokens)",
                name,
                savings,
                input_tokens,
                output_tokens
            );
        }
    }
}
