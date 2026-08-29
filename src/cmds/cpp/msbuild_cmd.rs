//! Filters MSBuild build output — noise lines dropped via streaming, errors kept verbatim.
//!
//! Uses `BlockStreamFilter` + `BlockHandler` (streaming mode) because MSBuild logs can be
//! enormous (hundreds of projects × many target lines). Streaming avoids buffering the
//! entire log and allows real-time noise suppression.
//!
//! Locale-agnostic design: all pattern matching uses structural rules independent of language.
//!
//! Design: summary-only output. All lines are captured silently in `should_skip` and the
//! final output is composed entirely in `format_summary`. No lines are emitted during streaming.

#![allow(dead_code)]

use super::diag;
use crate::core::runner;
use crate::core::stream::{BlockHandler, BlockStreamFilter};
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::Result;
use std::collections::HashSet;

// ─── Line Classification Helpers ───

/// Extract meaningful content after stripping ANSI and timestamp prefix.
fn extract_content(line: &str) -> String {
    let cleaned = strip_ansi(line);
    // Remove timestamp prefix like "10:45:41.690     0>" or "10:45:41.690     0]"
    let re = diag::lazy_re!(r"^\d{2}:\d{2}:\d{2}\.\d{3}\s+\d+[>\]]\s*");
    let after_ts = re.replace(&cleaned, "");
    after_ts.trim().to_string()
}

/// Check if a line (after stripping ANSI) is just a timestamp prefix with optional node id.
fn is_timestamp_only(line: &str) -> bool {
    diag::lazy_re!(r"^\d{2}:\d{2}:\d{2}\.\d{3}\s+\d+>$").is_match(line.trim())
}

/// Check if a line has meaningful content (not just ANSI noise / timestamp).
fn has_meaningful_content(line: &str) -> bool {
    let binding = strip_ansi(line);
    let stripped = binding.trim();
    if stripped.is_empty() {
        return false;
    }
    !is_timestamp_only(stripped)
}

/// Check if a line is a project start: contains a `.csproj` / `.sln` / `.vcxproj` path
/// with parenthesized node number like `(1)` → `(28:20)`.
fn is_project_start_line(cleaned: &str) -> bool {
    let has_project_ext =
        cleaned.contains(".csproj") || cleaned.contains(".vcxproj") || cleaned.contains(".sln");
    if !has_project_ext {
        return false;
    }
    diag::lazy_re!(r"\(\d+(:\d+)?\)").is_match(cleaned)
}

/// Check if a line is a project completion line: contains only a `.csproj` / `.vcxproj` path
/// and NO solution path (distinguishes from project-start which has both).
fn is_project_done_line(cleaned: &str) -> bool {
    let has_csproj = cleaned.contains(".csproj");
    let has_vcxproj = cleaned.contains(".vcxproj");
    if !has_csproj && !has_vcxproj {
        return false;
    }
    // If there's a .sln path, this is a project start with both solution and project paths
    if cleaned.contains(".sln") {
        return false;
    }
    // Count project extension occurrences — should be exactly 1
    cleaned.matches(".csproj").count() + cleaned.matches(".vcxproj").count() == 1
}

/// Check if a line matches build output: `ProjectName -> output.dll`.
/// Strict: left side must be a simple project name (no path separators),
/// right side must be a file path with a recognizable build artifact extension.
fn is_build_output_line(cleaned: &str) -> Option<String> {
    if let Some(pos) = cleaned.find(" -> ") {
        let project = cleaned[..pos].trim();
        let output = cleaned[pos + 4..].trim();
        // Left side must be a simple name (no path separators like \ or /)
        if project.contains('\\') || project.contains('/') {
            return None;
        }
        // Right side must be a file path with a build artifact extension
        if output.contains('.')
            && diag::lazy_re!(r"\.(dll|exe|lib|pyd|so|dylib|app|target|winmd)$").is_match(output)
        {
            return Some(format!("{} -> {}", project, output));
        }
    }
    None
}

/// Check if a line is a target execution line (ends with `:`), not an error/warning/header.
fn is_target_execution_line(cleaned: &str) -> bool {
    if !cleaned.ends_with(':') {
        return false;
    }
    // Known important line types that end with colon
    if cleaned.starts_with("error ")
        || cleaned.starts_with("warning ")
        || cleaned.starts_with("error:")
        || cleaned.starts_with("warning:")
        || cleaned.starts_with("=== ")
        || cleaned.starts_with("Build:")
    {
        return false;
    }
    // Skip if it looks like a Windows drive path with colon (e.g., "D:\path:")
    if cleaned.len() >= 3
        && cleaned.chars().nth(1) == Some(':')
        && cleaned.chars().nth(2) == Some('\\')
    {
        return false;
    }
    true
}

/// Check if a line is an MSBuild error line.
/// Matches both standalone (`error MSBXXXX:`) and embedded (`file(line): error CSXXXX:`) formats.
fn is_error_line(cleaned: &str) -> bool {
    // Standalone: "error MSB3202: ..." or "error CS0103: ..."
    if diag::lazy_re!(r"^error (MSB\d+|CS\d+)").is_match(cleaned) {
        return true;
    }
    // Embedded in file path: "file(line): error CSXXXX: ..."
    if diag::lazy_re!(r":\s*error (MSB\d+|CS\d+):").is_match(cleaned) {
        return true;
    }
    // Embedded without line number: "file: error CSXXXX: ..."
    if diag::lazy_re!(r"error (MSB\d+|CS\d+):").is_match(cleaned) {
        return true;
    }
    false
}

/// Check if a line is a warning line.
fn is_warning_line(cleaned: &str) -> bool {
    diag::lazy_re!(r"^warning (MSB\d+|CS\d+)").is_match(cleaned)
}

/// Check if a line starts a nested UBT build block.
fn is_nested_ubt_build_start(cleaned: &str) -> bool {
    cleaned.starts_with("Build:") && cleaned.contains("Build.bat")
}

/// Check if a line is an MSBuild version banner.
fn is_msbuild_version_line(cleaned: &str) -> bool {
    cleaned.contains("MSBuild")
        && diag::lazy_re!(r"\d+\.\d+\.\d+").is_match(cleaned)
        && !cleaned.starts_with("error")
        && !cleaned.starts_with("warning")
}

/// Check if a line is a build header like `=== REBUILD (incremental, 2nd attempt) ===`.
fn is_build_header(cleaned: &str) -> bool {
    cleaned.starts_with("=== ") && cleaned.ends_with(" ===")
}

/// Known MSBuild target execution lines that are always boilerplate noise.
fn is_known_target_line(cleaned: &str) -> bool {
    let known_targets: HashSet<&str> = [
        "ValidateSolutionConfiguration:",
        "ValidateProjects:",
        "CoreCompile:",
        "_GenerateSourceLinkFile:",
        "_TouchLastBuildWithSkipAnalyzers:",
        "GenerateBuildDependencyFile:",
        "_CopyOutOfDateSourceItemsToOutputDirectory:",
        "CopyFilesToOutputDirectory:",
        "_CreateAppHost:",
        "_ProcessScopedCssFiles:",
        "_BuildCopyStaticWebAssetsPreserveNewest:",
        "CleanupEmptyRefsFolder:",
        "CoreResGen:",
        "GenerateTargetFrameworkMonikerAttribute:",
        "CoreGenerateAssemblyInfo:",
        "GenerateBuildRuntimeConfigurationFiles:",
        "ResolveAssemblyReferences:",
        "ResolveProjectReferences:",
        "_CheckForUnsupportedTargetFramework:",
        "GenerateBindingRedirects:",
        "_CheckForCompileOutputs:",
        "AddBuiltProjectOutputToFastUpToDateCheck:",
        "BuiltProjectOutputGroup:",
        "DocumentationProjectOutputGroup:",
        "DebugSymbolsProjectOutputGroup:",
        "SatelliteDllsProjectOutputGroup:",
        "SourceFilesProjectOutputGroup:",
        "GetCopyToOutputDirectoryItems:",
        "_CopySourceItemsToOutputDirectory:",
        "_CopyAppConfigToOutputDirectory:",
        "_CopyManifestFilesToOutputDirectory:",
        "_GetChildProjectCopyToOutputDirectoryItems:",
        "PrepareForBuild:",
        "GetNativeManifest:",
        "_BeforeVCCLCompilerTool:",
        "_GenerateClStdDependencies:",
        "ClCompile:",
        "Link:",
        "Manifest:",
        "Midl:",
        "ResourceCompile:",
        "_GenerateRestoreSolutionProjectPathMap:",
        "_GetAllRestoreProjectPathMap:",
    ]
    .iter()
    .copied()
    .collect();
    known_targets.contains(cleaned)
}

// ─── Handler (Summary-Only) ───

/// BlockHandler for MSBuild build output — summary-only mode.
///
/// All lines are captured silently in `should_skip` (returns true for everything).
/// The final output is composed entirely in `format_summary`.
pub struct MsBuildHandler {
    // Captured metadata
    command: Option<String>,
    msbuild_version: Option<String>,
    build_mode: Option<String>,

    // Counters
    projects_built: usize,

    // Error tracking
    errors: Vec<String>,
    in_error_block: bool,

    // Warning tracking
    warnings: Vec<String>,

    // Build output lines
    build_output_lines: Vec<String>,

    // Nested UBT tracking
    nested_ubt_output: Vec<String>,
    in_nested_ubt: bool,

    // Raw line counter (for debugging)
    raw_count: usize,
}

impl MsBuildHandler {
    pub fn new() -> Self {
        Self {
            command: None,
            msbuild_version: None,
            build_mode: None,
            projects_built: 0,
            errors: Vec::new(),
            in_error_block: false,
            warnings: Vec::new(),
            build_output_lines: Vec::new(),
            nested_ubt_output: Vec::new(),
            in_nested_ubt: false,
            raw_count: 0,
        }
    }
}

impl BlockHandler for MsBuildHandler {
    /// should_skip captures ALL information and returns true (skip everything).
    /// No lines are emitted during streaming — only format_summary produces output.
    fn should_skip(&mut self, line: &str) -> bool {
        self.raw_count += 1;

        // Skip lines without meaningful content
        if !has_meaningful_content(line) {
            return true;
        }

        let content = extract_content(line);

        // If after stripping timestamp nothing meaningful, skip
        if content.is_empty() {
            return true;
        }

        // [CMD] line — capture command
        if let Some(cmd) = content.strip_prefix("[CMD] ") {
            self.command = Some(cmd.to_string());
            return true;
        }

        // MSBuild version banner — capture
        if is_msbuild_version_line(&content) {
            // Extract version number
            if let Some(caps) = diag::lazy_re!(r"(\d+\.\d+\.\d+)").captures(&content) {
                self.msbuild_version = Some(caps.get(1).unwrap().as_str().to_string());
            }
            return true;
        }

        // Build header — capture mode
        if is_build_header(&content) {
            let mode = content
                .trim_start_matches("=== ")
                .trim_end_matches(" ===")
                .trim()
                .to_string();
            self.build_mode = Some(mode);
            return true;
        }

        // Build output lines — capture project name
        if let Some(build_line) = is_build_output_line(&content) {
            self.build_output_lines.push(build_line);
            self.projects_built = self.build_output_lines.len();
            return true;
        }

        // Error lines — capture
        if is_error_line(&content) {
            self.in_error_block = true;
            self.errors.push(content.clone());
            return true;
        }

        // Error block continuation (indented context lines)
        if self.in_error_block && (content.starts_with(' ') || content.starts_with('\t')) {
            if let Some(last) = self.errors.last_mut() {
                last.push('\n');
                last.push_str(&content);
            }
            return true;
        }

        // End of error block (non-indented, non-empty line that's not a new error)
        if self.in_error_block && !content.is_empty() {
            self.in_error_block = false;
            // Don't return yet — check if this line has other meaning
        }

        // Warning lines — capture
        if is_warning_line(&content) {
            self.warnings.push(content.clone());
            return true;
        }

        // Nested UBT
        if is_nested_ubt_build_start(&content) {
            self.in_nested_ubt = true;
            self.nested_ubt_output.push(content.clone());
            return true;
        }

        // Nested UBT continuation
        if self.in_nested_ubt {
            if content.starts_with("Using bundled DotNet")
                || content.starts_with("Running UnrealBuildTool")
                || content.contains("Target is up to date")
                || content.starts_with("Result: ")
                || content.starts_with("Total execution time:")
            {
                self.nested_ubt_output.push(content.clone());
                return true;
            }
            self.in_nested_ubt = false;
        }

        // Project lines — we track them via build output lines, no need to count separately

        // Known target execution lines — skip
        if is_known_target_line(&content) || is_target_execution_line(&content) {
            return true;
        }

        // Everything else — skip
        true
    }

    fn is_block_start(&mut self, _line: &str) -> bool {
        // Summary-only: no blocks emitted during streaming
        false
    }

    fn is_block_continuation(&mut self, _line: &str, _block: &[String]) -> bool {
        // Summary-only: no blocks emitted during streaming
        false
    }

    fn format_summary(&self, _exit_code: i32, _raw: &str) -> Option<String> {
        let mut lines = Vec::new();

        // Mode display
        let mode_display = if let Some(ref mode) = self.build_mode {
            format!(" ({})", mode)
        } else {
            String::new()
        };

        // Summary line
        let built = self.build_output_lines.len();
        if self.errors.is_empty() {
            lines.push(format!(
                "ok msbuild: {} projects built{}",
                built, mode_display
            ));
        } else {
            lines.push(format!(
                "msbuild: {} built, {} errors{}",
                built,
                self.errors.len(),
                mode_display
            ));
        }

        // MSBuild version
        if let Some(ref ver) = self.msbuild_version {
            lines.push(format!("  MSBuild {}", ver));
        }

        // Build output lines
        for out in &self.build_output_lines {
            lines.push(format!("  built: {}", out));
        }

        // Warnings
        for w in &self.warnings {
            lines.push(format!("  warning: {}", w));
        }

        // Errors
        if !self.errors.is_empty() {
            lines.push("Errors:".to_string());
            for e in &self.errors {
                let first_line = e.lines().next().unwrap_or(e);
                lines.push(format!("  {}", first_line));
                for rest in e.lines().skip(1) {
                    lines.push(format!("    {}", rest));
                }
            }
        }

        // Nested UBT output
        for ubt_line in &self.nested_ubt_output {
            lines.push(format!("  nested UBT: {}", ubt_line));
        }

        Some(lines.join("\n") + "\n")
    }
}

// ─── Public API ───

/// Run MSBuild with output filtering.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("msbuild: running msbuild {}", args.join(" "));
    }

    let mut cmd = resolved_command("msbuild");
    for arg in args {
        cmd.arg(arg);
    }
    let args_str = args.join(" ");

    runner::run_streamed(
        cmd,
        "msbuild",
        &args_str,
        Box::new(BlockStreamFilter::new(MsBuildHandler::new())),
        runner::RunOptions::with_tee("msbuild"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stream::StreamFilter;
    use crate::core::tracking::estimate_tokens;

    fn run_block_filter(filter: &mut dyn StreamFilter, input: &str, exit_code: i32) -> String {
        let mut output = String::new();
        for line in input.lines() {
            if let Some(s) = filter.feed_line(line) {
                output.push_str(&s);
            }
        }
        output.push_str(&filter.flush());
        if let Some(post) = filter.on_exit(exit_code, input) {
            output.push_str(&post);
        }
        output
    }

    fn filter_msbuild(input: &str, exit_code: i32) -> String {
        let handler = MsBuildHandler::new();
        let mut filter = BlockStreamFilter::new(handler);
        run_block_filter(&mut filter, input, exit_code)
    }

    // ── Helper tests ──

    #[test]
    fn test_is_project_start_line_with_csproj() {
        assert!(is_project_start_line(
            "D:\\TestUE\\TestUE.sln(1) -> D:\\TestUE\\Source\\TestUEModule\\TestUEModule.csproj"
        ));
        assert!(is_project_start_line(
            "D:\\TestUE\\TestUE.sln(28:20) -> D:\\TestUE\\Source\\TestUEModule\\TestUEModule.csproj"
        ));
    }

    #[test]
    fn test_is_project_start_line_with_vcxproj() {
        assert!(is_project_start_line(
            "D:\\TestUE\\TestUE.sln(1) -> D:\\TestUE\\Source\\TestUEModule\\TestUEModule.vcxproj"
        ));
    }

    #[test]
    fn test_is_project_start_line_not() {
        assert!(!is_project_start_line("some random text"));
        assert!(!is_project_start_line(
            "Build: D:\\UE_5.6\\Engine\\Build\\BatchFiles\\Build.bat"
        ));
    }

    #[test]
    fn test_is_project_done_line() {
        assert!(is_project_done_line(
            "D:\\TestUE\\Source\\TestUEModule\\TestUEModule.csproj"
        ));
        assert!(is_project_done_line(
            "D:\\TestUE\\Source\\TestUEModule\\TestUEModule.vcxproj"
        ));
    }

    #[test]
    fn test_is_project_done_line_not_when_two_projects() {
        // This has both .sln and .csproj → it's a project start, not done
        assert!(!is_project_done_line(
            "D:\\TestUE\\TestUE.sln(1) -> D:\\TestUE\\Source\\TestUEModule\\TestUEModule.csproj"
        ));
    }

    #[test]
    fn test_is_build_output_line() {
        assert!(is_build_output_line("TestUEModule -> D:\\Output\\TestUEModule.dll").is_some());
        assert!(is_build_output_line("ModuleB -> D:\\Output\\ModuleB.exe").is_some());
        assert!(is_build_output_line("ModuleC -> D:\\Output\\ModuleC.lib").is_some());
    }

    #[test]
    fn test_is_build_output_line_no_match() {
        assert!(is_build_output_line("some text without arrow").is_none());
        assert!(is_build_output_line("error CS0103: the name 'foo' does not exist").is_none());
    }

    #[test]
    fn test_is_error_line() {
        assert!(is_error_line(
            "error MSB3202: The source file 'x' could not be found"
        ));
        assert!(is_error_line(
            "error CS0103: The name 'foo' does not exist in the current context"
        ));
    }

    #[test]
    fn test_is_error_line_not() {
        assert!(!is_error_line("warning MSB3202: something"));
        assert!(!is_error_line("Build: D:\\Build.bat"));
    }

    #[test]
    fn test_is_warning_line() {
        assert!(is_warning_line("warning MSB3202: something"));
        assert!(!is_warning_line("error MSB3202: something"));
    }

    #[test]
    fn test_is_target_execution_line() {
        assert!(is_target_execution_line("CoreCompile:"));
        assert!(is_target_execution_line("_GenerateSourceLinkFile:"));
        assert!(is_target_execution_line("CopyFilesToOutputDirectory:"));
    }

    #[test]
    fn test_is_target_execution_line_not() {
        assert!(!is_target_execution_line("error MSB3202:"));
        assert!(!is_target_execution_line("warning MSB3202:"));
        assert!(!is_target_execution_line("=== REBUILD ==="));
    }

    #[test]
    fn test_is_nested_ubt_build_start() {
        assert!(is_nested_ubt_build_start(
            "Build: D:\\UE_5.6\\Engine\\Build\\BatchFiles\\Build.bat TestUEEditor Win64 Development"
        ));
    }

    #[test]
    fn test_is_nested_ubt_build_start_not() {
        assert!(!is_nested_ubt_build_start("Build: D:\\some\\other.exe"));
    }

    #[test]
    fn test_has_meaningful_content() {
        assert!(has_meaningful_content("Hello world"));
        assert!(!has_meaningful_content(""));
        assert!(!has_meaningful_content("\x1b[36;1m\x1b[m"));
    }

    // ── Success cases ──

    #[test]
    fn test_msbuild_success_all_up_to_date() {
        let input = "\
[CMD] C:\\Program Files\\Microsoft Visual Studio\\2022\\Community\\MSBuild\\Current\\Bin\\MSBuild.exe D:\\TestUE\\TestUE.sln /p:Configuration=Development
\x1b[36;1m10:45:41.690     0>\x1b[mMicrosoft (R) Build Engine version 17.14.23 for MSBuild
\x1b[36;1m10:45:41.690     0>\x1b[mCopyright (C) Microsoft Corporation. All rights reserved.
\x1b[36;1m10:45:41.690     0>\x1b[m
\x1b[36;1m10:45:41.690     0>\x1b[m=== REBUILD (incremental, 2nd attempt) ===
\x1b[36;1m10:45:41.690     0>\x1b[mValidateSolutionConfiguration:
\x1b[36;1m10:45:41.690     0>\x1b[mTestUEModule -> D:\\Output\\TestUEModule.dll
\x1b[36;1m10:45:41.690     0>\x1b[mBuild succeeded.
\x1b[36;1m10:45:41.690     0>\x1b[m    0 Warning(s)
\x1b[36;1m10:45:41.690     0>\x1b[m    0 Error(s)
";
        let result = filter_msbuild(input, 0);
        assert!(
            result.contains("ok msbuild:"),
            "should have ok prefix, got: {}",
            result
        );
        assert!(
            !result.contains("ValidateSolutionConfiguration:"),
            "should skip solution targets, got: {}",
            result
        );
        assert!(
            !result.contains("\x1b["),
            "ANSI codes should be stripped, got: {}",
            result
        );
        assert!(
            result.contains("MSBuild 17.14.23"),
            "version should be captured, got: {}",
            result
        );
        assert!(
            result.contains("REBUILD (incremental, 2nd attempt)"),
            "build mode should be captured, got: {}",
            result
        );
    }

    #[test]
    fn test_msbuild_single_error() {
        let input = "\
\x1b[36;1m10:45:41.690     0>\x1b[m=== REBUILD ===
\x1b[36;1m10:45:41.690     0>\x1b[mD:\\TestUE\\TestUE.sln(1) -> D:\\TestUE\\Source\\TestUEModule\\TestUEModule.csproj
\x1b[36;1m10:45:41.690     0>\x1b[mCoreCompile:
\x1b[36;1m10:45:41.690     0>\x1b[mD:\\TestUE\\Source\\TestUEModule\\main.cpp(42): error CS0103: The name 'foo' does not exist
\x1b[36;1m10:45:41.690     0>\x1b[m  D:\\TestUE\\Source\\TestUEModule\\main.cpp(42): context line
\x1b[36;1m10:45:41.690     0>\x1b[mBuild FAILED.
";
        let result = filter_msbuild(input, 1);
        assert!(
            result.contains("error CS0103"),
            "error should be kept, got: {}",
            result
        );
        assert!(
            !result.contains("CoreCompile:"),
            "target lines should be skipped, got: {}",
            result
        );
    }

    #[test]
    fn test_msbuild_multiple_errors() {
        let input = "\
\x1b[36;1m10:45:41.690     0>\x1b[m=== REBUILD ===
\x1b[36;1m10:45:41.690     0>\x1b[mD:\\TestUE\\TestUE.sln(1) -> D:\\TestUE\\Source\\ModuleA\\ModuleA.csproj
\x1b[36;1m10:45:41.690     0>\x1b[merror MSB3202: The source file 'x' could not be found
\x1b[36;1m10:45:41.690     0>\x1b[mD:\\TestUE\\TestUE.sln(2) -> D:\\TestUE\\Source\\ModuleB\\ModuleB.csproj
\x1b[36;1m10:45:41.690     0>\x1b[merror CS0103: The name 'bar' does not exist
\x1b[36;1m10:45:41.690     0>\x1b[mBuild FAILED.
";
        let result = filter_msbuild(input, 1);
        assert!(result.contains("error MSB3202"), "got: {}", result);
        assert!(result.contains("error CS0103"), "got: {}", result);
    }

    #[test]
    fn test_msbuild_nested_ubt_output() {
        let input = "\
\x1b[36;1m10:45:41.690     0>\x1b[mBuild: D:\\UE_5.6\\Engine\\Build\\BatchFiles\\Build.bat TestUEEditor Win64 Development
Using bundled DotNet SDK version: 8.0.300
Running UnrealBuildTool: dotnet D:\\UE_5.6\\Engine\\Binaries\\DotNET\\UnrealBuildTool\\UnrealBuildTool.dll
Target is up to date
Result: Succeeded
Total execution time: 0.57 seconds
";
        let result = filter_msbuild(input, 0);
        assert!(
            result.contains("nested UBT"),
            "should contain nested UBT summary, got: {}",
            result
        );
        assert!(result.contains("Target is up to date"), "got: {}", result);
        assert!(result.contains("Result: Succeeded"), "got: {}", result);
    }

    #[test]
    fn test_msbuild_project_counting() {
        let input = "\
\x1b[36;1m10:45:41.690     0>\x1b[m=== REBUILD ===
\x1b[36;1m10:45:41.690     0>\x1b[mD:\\TestUE\\TestUE.sln(1) -> D:\\TestUE\\Source\\ModuleA\\ModuleA.csproj
\x1b[36;1m10:45:41.690     0>\x1b[mModuleA -> D:\\Output\\ModuleA.dll
\x1b[36;1m10:45:41.690     0>\x1b[mD:\\TestUE\\TestUE.sln(2) -> D:\\TestUE\\Source\\ModuleB\\ModuleB.csproj
\x1b[36;1m10:45:41.690     0>\x1b[mModuleB -> D:\\Output\\ModuleB.exe
\x1b[36;1m10:45:41.690     0>\x1b[mD:\\TestUE\\TestUE.sln(3) -> D:\\TestUE\\Source\\ModuleC\\ModuleC.vcxproj
\x1b[36;1m10:45:41.690     0>\x1b[mModuleC -> D:\\Output\\ModuleC.dll
";
        let result = filter_msbuild(input, 0);
        assert!(
            result.contains("3 projects built"),
            "should have 3 projects, got: {}",
            result
        );
        assert!(result.contains("built: ModuleA ->"), "got: {}", result);
        assert!(result.contains("built: ModuleB ->"), "got: {}", result);
        assert!(result.contains("built: ModuleC ->"), "got: {}", result);
    }

    #[test]
    fn test_msbuild_ansi_stripped() {
        let input = "\x1b[36;1m10:45:41.690     0>\x1b[m\x1b[31merror MSB3202: something\x1b[0m\n";
        let result = filter_msbuild(input, 1);
        assert!(
            !result.contains("\x1b["),
            "ANSI codes should be stripped, got: {}",
            result
        );
        assert!(result.contains("error MSB3202"), "got: {}", result);
    }

    #[test]
    fn test_msbuild_empty_input() {
        let result = filter_msbuild("", 0);
        assert!(
            result.contains("msbuild"),
            "should have a summary, got: '{}'",
            result
        );
    }

    #[test]
    fn test_msbuild_token_savings_above_90pct() {
        let mut input = String::new();
        for i in 1..=50 {
            input.push_str(&format!(
                "\x1b[36;1m10:45:41.690     {}>\x1b[mD:\\Test.sln({}) -> D:\\Test\\Module{}\\Module{}.csproj\n",
                i, i, i, i
            ));
            input.push_str("\x1b[36;1m10:45:41.690     0>\x1b[mCoreCompile:\n");
            input.push_str("\x1b[36;1m10:45:41.690     0>\x1b[m_GenerateSourceLinkFile:\n");
            input.push_str("\x1b[36;1m10:45:41.690     0>\x1b[mCopyFilesToOutputDirectory:\n");
            input.push_str(&format!(
                "\x1b[36;1m10:45:41.690     {}>\x1b[mModule{} -> D:\\Output\\Module{}.dll\n",
                i, i, i
            ));
            // More target noise
            input.push_str("\x1b[36;1m10:45:41.690     0>\x1b[mClCompile:\n");
            input.push_str("\x1b[36;1m10:45:41.690     0>\x1b[mLink:\n");
            input.push_str("\x1b[36;1m10:45:41.690     0>\x1b[mManifest:\n");
            input.push_str("\x1b[36;1m10:45:41.690     0>\x1b[mResolveAssemblyReferences:\n");
            input.push_str("\x1b[36;1m10:45:41.690     0>\x1b[mPrepareForBuild:\n");
            // Locale-specific noise
            input.push_str(&format!(
                "\x1b[36;1m10:45:41.690     {}>\x1b[m  Build completed for module {} (skipped)\n",
                i, i
            ));
        }
        input.push_str("Build succeeded.\n");

        let result = filter_msbuild(&input, 0);
        let raw_tokens = estimate_tokens(&input);
        let filtered_tokens = estimate_tokens(&result);
        let savings = if raw_tokens > 0 {
            ((raw_tokens - filtered_tokens) as f64 / raw_tokens as f64 * 100.0) as usize
        } else {
            0
        };
        assert!(
            savings >= 90,
            "token savings: {}% (expected >=90%)",
            savings
        );
    }

    #[test]
    fn test_msbuild_target_lines_dropped() {
        let input = "\
=== BUILD ===
CoreCompile:
_GenerateSourceLinkFile:
CopyFilesToOutputDirectory:
ValidateSolutionConfiguration:
Result: Succeeded
";
        let result = filter_msbuild(input, 0);
        assert!(
            !result.contains("CoreCompile:"),
            "target lines should be dropped, got: {}",
            result
        );
        assert!(
            !result.contains("_GenerateSourceLinkFile:"),
            "got: {}",
            result
        );
        assert!(
            !result.contains("CopyFilesToOutputDirectory:"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_msbuild_build_output_lines_preserved() {
        let input = "\
=== BUILD ===
ModuleA -> D:\\Output\\ModuleA.dll
ModuleB -> D:\\Output\\ModuleB.exe
ModuleC -> D:\\Output\\ModuleC.lib
Result: Succeeded
";
        let result = filter_msbuild(input, 0);
        assert!(
            result.contains("built: ModuleA ->"),
            "build output should be preserved, got: {}",
            result
        );
        assert!(result.contains("built: ModuleB ->"), "got: {}", result);
        assert!(result.contains("built: ModuleC ->"), "got: {}", result);
    }

    #[test]
    fn test_msbuild_version_captured() {
        let input = "\
Microsoft (R) Build Engine version 17.14.23 for MSBuild
Result: Succeeded
";
        let result = filter_msbuild(input, 0);
        assert!(
            result.contains("MSBuild 17.14.23"),
            "MSBuild version should be captured, got: {}",
            result
        );
    }
}
