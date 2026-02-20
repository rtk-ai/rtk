use crate::tracking;
use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashSet;
use std::process::Command;
use std::sync::OnceLock;

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("xcodebuild");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: xcodebuild {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run xcodebuild (is Xcode installed?)")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = filter_xcodebuild(&raw);

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });

    if let Some(hint) = crate::tee::tee_and_hint(&raw, "xcodebuild", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("xcodebuild {}", args.join(" ")),
        &format!("rtk xcodebuild {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

/// Filter xcodebuild output - extract targets, errors, warnings, signing, and build result.
/// Strips verbose build system internals, full command invocations, and package resolution details.
pub fn filter_xcodebuild(output: &str) -> String {
    static TARGET_RE: OnceLock<Regex> = OnceLock::new();
    let target_re = TARGET_RE.get_or_init(|| {
        Regex::new(r"Target '([^']+)' in project '([^']+)'").expect("invalid target regex")
    });

    static COMPILE_FILE_RE: OnceLock<Regex> = OnceLock::new();
    let compile_file_re = COMPILE_FILE_RE.get_or_init(|| {
        Regex::new(r"^SwiftCompile\s+\w+\s+\w+\s+Compiling\\?\s+(\S+\.swift)")
            .expect("invalid compile file regex")
    });

    static ERROR_RE: OnceLock<Regex> = OnceLock::new();
    let error_re = ERROR_RE.get_or_init(|| {
        Regex::new(r"^(.+?):(\d+):(\d+):\s+(error|warning):\s+(.+)$").expect("invalid error regex")
    });

    static SIGNING_RE: OnceLock<Regex> = OnceLock::new();
    let signing_re = SIGNING_RE.get_or_init(|| {
        Regex::new(r#"Signing Identity:\s+"([^"]+)""#).expect("invalid signing regex")
    });

    static RESOLVED_PKG_RE: OnceLock<Regex> = OnceLock::new();
    let resolved_pkg_re = RESOLVED_PKG_RE
        .get_or_init(|| Regex::new(r"^\s{2}\S+:.*@").expect("invalid resolved package regex"));

    // Counters and collectors
    let mut targets: Vec<(String, String)> = Vec::new();
    let mut compiled_files: Vec<String> = Vec::new();
    let mut compiled_files_seen: HashSet<String> = HashSet::new();
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut signing_identity: Option<String> = None;
    let mut packages_resolved = 0;
    let mut packages_fetched = 0;
    let mut pcm_count = 0;
    let mut build_result: Option<String> = None;
    let mut failed_commands: Vec<String> = Vec::new();
    let mut failure_count: Option<String> = None;
    let mut in_resolved_section = false;
    let mut in_failed_section = false;

    for line in output.lines() {
        let trimmed = line.trim();

        // Track build result
        if trimmed.starts_with("** BUILD SUCCEEDED **") {
            build_result = Some("BUILD SUCCEEDED".to_string());
            continue;
        }
        if trimmed.starts_with("** BUILD FAILED **") {
            build_result = Some("BUILD FAILED".to_string());
            continue;
        }

        // Parse failure summary section
        if trimmed == "The following build commands failed:" {
            in_failed_section = true;
            continue;
        }
        if in_failed_section {
            if trimmed.starts_with('(') && trimmed.ends_with("failures)") {
                failure_count = Some(trimmed.to_string());
                in_failed_section = false;
            } else if line.starts_with('\t') || line.starts_with("    ") {
                failed_commands.push(trimmed.to_string());
            }
            continue;
        }

        // Track resolved packages section
        if trimmed == "Resolved source packages:" {
            in_resolved_section = true;
            continue;
        }
        if in_resolved_section {
            if resolved_pkg_re.is_match(line) {
                packages_resolved += 1;
                continue;
            }
            // End of resolved section when we hit a non-matching line
            if !trimmed.is_empty() {
                in_resolved_section = false;
            } else {
                continue;
            }
        }

        // Count fetched packages
        if trimmed.starts_with("Fetching from ") {
            packages_fetched += 1;
            continue;
        }

        // Skip package checkout noise
        if trimmed.starts_with("Creating working copy of package")
            || trimmed.starts_with("Checking out ")
        {
            continue;
        }

        // Extract target info
        if let Some(caps) = target_re.captures(trimmed) {
            let target_name = caps[1].to_string();
            let project_name = caps[2].to_string();
            let entry = (target_name, project_name);
            if !targets.contains(&entry) {
                targets.push(entry);
            }
            continue;
        }

        // Extract compiled Swift files
        if let Some(caps) = compile_file_re.captures(trimmed) {
            let filename = caps[1].replace('\\', "");
            if compiled_files_seen.insert(filename.clone()) {
                compiled_files.push(filename);
            }
            continue;
        }

        // Extract errors and warnings from compiler output
        if let Some(caps) = error_re.captures(trimmed) {
            let file_path = &caps[1];
            let line_num = &caps[2];
            let severity = &caps[4];
            let message = &caps[5];

            // Extract just the filename from full path
            let filename = file_path.rsplit('/').next().unwrap_or(file_path);

            let formatted = format!("  {}:{}: {}", filename, line_num, message);

            if severity == "error" {
                errors.push(formatted);
            } else {
                warnings.push(formatted);
            }
            continue;
        }

        // Extract signing identity
        if let Some(caps) = signing_re.captures(trimmed) {
            signing_identity = Some(caps[1].to_string());
            continue;
        }

        // Count precompiled modules
        if trimmed.starts_with("SwiftExplicitDependencyGeneratePcm ") {
            pcm_count += 1;
            continue;
        }

        // Skip all the noise lines (the rest is build system internals)
        // These are the biggest token wasters:
        // - "cd /path/..." lines
        // - "builtin-*" command lines
        // - Full compiler/linker invocations
        // - ClangStatCache, ProcessProductPackaging, etc.
        // - ExecuteExternalTool lines
        // - Build description signature/path
        // - Internal build steps (Compute*, Create*, Gather*, etc.)
        // - Copy operations
        // - Touch, RegisterExecutionPolicyException
        // - Entitlements blocks
        // - SwiftDriver/SwiftMergeGeneratedHeaders/Ld commands
        // - EmitSwiftModule/SwiftDriverJobDiscovery
        // - AppIntents metadata lines
        // - Timestamp-prefixed log lines
        // We don't explicitly skip them - they're just not captured.
    }

    // Build output
    let mut result = String::new();

    match &build_result {
        Some(status) if status == "BUILD SUCCEEDED" => {
            result.push_str("✓ xcodebuild: BUILD SUCCEEDED\n");
        }
        Some(status) if status == "BUILD FAILED" => {
            result.push_str("✗ xcodebuild: BUILD FAILED\n");
        }
        Some(status) => {
            result.push_str(&format!("xcodebuild: {}\n", status));
        }
        None => {
            result.push_str("xcodebuild: completed\n");
        }
    }

    // Targets
    if !targets.is_empty() {
        let target_strs: Vec<String> = targets
            .iter()
            .map(|(t, p)| format!("{} ({})", t, p))
            .collect();
        if targets.len() == 1 {
            result.push_str(&format!("  Target: {}\n", target_strs[0]));
        } else {
            result.push_str(&format!("  Targets: {}\n", target_strs.join(", ")));
        }
    }

    // Package info
    if packages_resolved > 0 || packages_fetched > 0 {
        let mut pkg_parts = Vec::new();
        if packages_resolved > 0 {
            pkg_parts.push(format!("{} resolved", packages_resolved));
        }
        if packages_fetched > 0 {
            pkg_parts.push(format!("{} fetched", packages_fetched));
        }
        result.push_str(&format!("  Packages: {}\n", pkg_parts.join(", ")));
    }

    // Compiled files
    if !compiled_files.is_empty() {
        if compiled_files.len() <= 5 {
            result.push_str(&format!("  Compiled: {}\n", compiled_files.join(", ")));
        } else {
            let shown: Vec<&str> = compiled_files.iter().take(5).map(|s| s.as_str()).collect();
            result.push_str(&format!(
                "  Compiled: {} (+{} more)\n",
                shown.join(", "),
                compiled_files.len() - 5
            ));
        }
    }

    // Precompiled modules
    if pcm_count > 0 {
        result.push_str(&format!("  Precompiled modules: {}\n", pcm_count));
    }

    // Errors (always show in full - these are what the user needs)
    if !errors.is_empty() {
        result.push_str(&format!(
            "  {} error{}:\n",
            errors.len(),
            if errors.len() == 1 { "" } else { "s" }
        ));
        for err in &errors {
            result.push_str(&format!("{}\n", err));
        }
    }

    // Warnings
    if !warnings.is_empty() {
        result.push_str(&format!(
            "  {} warning{}:\n",
            warnings.len(),
            if warnings.len() == 1 { "" } else { "s" }
        ));
        for warn in &warnings {
            result.push_str(&format!("{}\n", warn));
        }
    }

    // Failed commands summary
    if !failed_commands.is_empty() {
        result.push_str("  Failed:\n");
        for cmd in &failed_commands {
            result.push_str(&format!("    {}\n", cmd));
        }
    }
    if let Some(fc) = &failure_count {
        result.push_str(&format!("  {}\n", fc));
    }

    // Signing
    if let Some(identity) = &signing_identity {
        result.push_str(&format!("  Signed: {}\n", identity));
    }

    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_filter_success_full_build() {
        let input = include_str!("../tests/fixtures/xcodebuild_success_cached.txt");
        let result = filter_xcodebuild(input);

        // Must contain build result
        assert!(
            result.contains("BUILD SUCCEEDED"),
            "missing BUILD SUCCEEDED: {}",
            result
        );
        assert!(result.contains("✓"), "missing success marker: {}", result);

        // Must contain target info
        assert!(
            result.contains("bitchatShareExtension"),
            "missing target: {}",
            result
        );
        assert!(result.contains("bitchat"), "missing project: {}", result);

        // Must contain package count
        assert!(
            result.contains("21 resolved"),
            "missing package count: {}",
            result
        );

        // Must contain compiled files
        assert!(
            result.contains("ShareViewController.swift"),
            "missing compiled file: {}",
            result
        );
        assert!(
            result.contains("TransportConfig.swift"),
            "missing compiled file: {}",
            result
        );

        // Must contain PCM count
        assert!(
            result.contains("Precompiled modules: 50"),
            "missing PCM count: {}",
            result
        );

        // Must contain signing identity
        assert!(
            result.contains("Apple Development: Austin Heap"),
            "missing signing identity: {}",
            result
        );

        // Must NOT contain noise
        assert!(
            !result.contains("builtin-"),
            "contains builtin command: {}",
            result
        );
        assert!(
            !result.contains("/Applications/Xcode.app"),
            "contains Xcode path: {}",
            result
        );
        assert!(
            !result.contains("SwiftExplicitDependencyGeneratePcm"),
            "contains PCM line: {}",
            result
        );
        assert!(
            !result.contains("ClangStatCache"),
            "contains ClangStatCache: {}",
            result
        );
        assert!(
            !result.contains("ProcessProductPackaging"),
            "contains ProcessProductPackaging: {}",
            result
        );
        assert!(
            !result.contains("Entitlements:"),
            "contains Entitlements: {}",
            result
        );
        assert!(
            !result.contains("builtin-copy"),
            "contains builtin-copy: {}",
            result
        );
    }

    #[test]
    fn test_filter_success_minimal_build() {
        let input = include_str!("../tests/fixtures/xcodebuild_success_minimal.txt");
        let result = filter_xcodebuild(input);

        assert!(
            result.contains("BUILD SUCCEEDED"),
            "missing BUILD SUCCEEDED: {}",
            result
        );
        assert!(
            result.contains("bitchatShareExtension"),
            "missing target: {}",
            result
        );
        assert!(
            result.contains("4 resolved"),
            "missing package count: {}",
            result
        );
        assert!(
            result.contains("19 fetched"),
            "missing fetch count: {}",
            result
        );

        // Must NOT contain noise
        assert!(
            !result.contains("Fetching from"),
            "contains Fetching: {}",
            result
        );
        assert!(
            !result.contains("Creating working copy"),
            "contains checkout: {}",
            result
        );
        assert!(
            !result.contains("builtin-infoPlistUtility"),
            "contains builtin: {}",
            result
        );
    }

    #[test]
    fn test_filter_build_failure() {
        let input = include_str!("../tests/fixtures/xcodebuild_failure.txt");
        let result = filter_xcodebuild(input);

        // Must show failure
        assert!(
            result.contains("BUILD FAILED"),
            "missing BUILD FAILED: {}",
            result
        );
        assert!(result.contains("✗"), "missing failure marker: {}", result);

        // Must show errors with filenames (not full paths)
        assert!(
            result.contains("AppDelegate.swift:15"),
            "missing error location: {}",
            result
        );
        assert!(
            result.contains("cannot find 'missingVariable' in scope"),
            "missing error message: {}",
            result
        );
        assert!(
            result.contains("MainView.swift:45"),
            "missing error location: {}",
            result
        );
        assert!(
            result.contains("NetworkManager.swift:67"),
            "missing error location: {}",
            result
        );

        // Must show warning
        assert!(
            result.contains("1 warning"),
            "missing warning count: {}",
            result
        );
        assert!(
            result.contains("result of call to 'print()' is unused"),
            "missing warning message: {}",
            result
        );

        // Must show error count
        assert!(
            result.contains("4 error"),
            "missing error count: {}",
            result
        );

        // Must show failed commands
        assert!(
            result.contains("Failed:"),
            "missing Failed section: {}",
            result
        );

        // Must NOT contain full paths
        assert!(
            !result.contains("/Users/austinheap/Development"),
            "contains full path: {}",
            result
        );
    }

    #[test]
    fn test_token_savings_full_build() {
        let input = include_str!("../tests/fixtures/xcodebuild_success_cached.txt");
        let result = filter_xcodebuild(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 90.0,
            "xcodebuild filter: expected >=90% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_token_savings_minimal_build() {
        let input = include_str!("../tests/fixtures/xcodebuild_success_minimal.txt");
        let result = filter_xcodebuild(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 80.0,
            "xcodebuild filter: expected >=80% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_token_savings_failure() {
        let input = include_str!("../tests/fixtures/xcodebuild_failure.txt");
        let result = filter_xcodebuild(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "xcodebuild filter: expected >=60% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_empty_input() {
        let result = filter_xcodebuild("");
        assert!(
            result.contains("completed"),
            "empty input should show completed: {}",
            result
        );
    }

    #[test]
    fn test_build_succeeded_only() {
        let input = "** BUILD SUCCEEDED **\n";
        let result = filter_xcodebuild(input);
        assert!(
            result.contains("✓ xcodebuild: BUILD SUCCEEDED"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_build_failed_only() {
        let input = "** BUILD FAILED **\n";
        let result = filter_xcodebuild(input);
        assert!(
            result.contains("✗ xcodebuild: BUILD FAILED"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_multiple_targets() {
        let input = "\
note: Target dependency graph (3 targets)
    Target 'MyApp' in project 'MyProject'
    Target 'MyAppTests' in project 'MyProject'
    Target 'MyFramework' in project 'MyFramework'
** BUILD SUCCEEDED **
";
        let result = filter_xcodebuild(input);
        assert!(
            result.contains("Targets:"),
            "multiple targets should use 'Targets:': {}",
            result
        );
        assert!(
            result.contains("MyApp (MyProject)"),
            "missing target: {}",
            result
        );
        assert!(
            result.contains("MyAppTests (MyProject)"),
            "missing target: {}",
            result
        );
        assert!(
            result.contains("MyFramework (MyFramework)"),
            "missing target: {}",
            result
        );
    }

    #[test]
    fn test_single_target() {
        let input = "\
    Target 'MyApp' in project 'MyProject' (no dependencies)
** BUILD SUCCEEDED **
";
        let result = filter_xcodebuild(input);
        assert!(
            result.contains("Target: MyApp (MyProject)"),
            "single target should use 'Target:': {}",
            result
        );
    }

    #[test]
    fn test_no_packages() {
        let input = "\
note: Building targets in dependency order
    Target 'MyApp' in project 'MyProject' (no dependencies)
** BUILD SUCCEEDED **
";
        let result = filter_xcodebuild(input);
        assert!(
            !result.contains("Packages:"),
            "should not show packages when none: {}",
            result
        );
    }

    #[test]
    fn test_many_compiled_files() {
        let input = "\
SwiftCompile normal arm64 Compiling\\ File1.swift /path/File1.swift (in target 'T' from project 'P')
SwiftCompile normal arm64 Compiling\\ File2.swift /path/File2.swift (in target 'T' from project 'P')
SwiftCompile normal arm64 Compiling\\ File3.swift /path/File3.swift (in target 'T' from project 'P')
SwiftCompile normal arm64 Compiling\\ File4.swift /path/File4.swift (in target 'T' from project 'P')
SwiftCompile normal arm64 Compiling\\ File5.swift /path/File5.swift (in target 'T' from project 'P')
SwiftCompile normal arm64 Compiling\\ File6.swift /path/File6.swift (in target 'T' from project 'P')
SwiftCompile normal arm64 Compiling\\ File7.swift /path/File7.swift (in target 'T' from project 'P')
** BUILD SUCCEEDED **
";
        let result = filter_xcodebuild(input);
        assert!(
            result.contains("(+2 more)"),
            "should truncate compiled files: {}",
            result
        );
        assert!(
            result.contains("File1.swift"),
            "should show first files: {}",
            result
        );
        assert!(
            result.contains("File5.swift"),
            "should show up to 5 files: {}",
            result
        );
    }

    #[test]
    fn test_deduplicate_compiled_files() {
        let input = "\
SwiftCompile normal arm64 Compiling\\ File1.swift /path/File1.swift (in target 'T' from project 'P')
SwiftCompile normal arm64 Compiling\\ File1.swift /path/File1.swift (in target 'T' from project 'P')
** BUILD SUCCEEDED **
";
        let result = filter_xcodebuild(input);
        // File1.swift should only appear once in the compiled list
        let count = result.matches("File1.swift").count();
        assert_eq!(
            count, 1,
            "File1.swift should appear once, got {}: {}",
            count, result
        );
    }
}
