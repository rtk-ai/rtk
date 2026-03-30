//! Filters dotnet CLI output — build, test, and format results.

use crate::binlog;
use crate::core::tracking;
use crate::core::utils::{exit_code_from_output, resolved_command, truncate};
use crate::dotnet_format_report;
use crate::dotnet_trx;
use anyhow::{Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use serde_json::Value;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const DOTNET_CLI_UI_LANGUAGE: &str = "DOTNET_CLI_UI_LANGUAGE";
const DOTNET_CLI_UI_LANGUAGE_VALUE: &str = "en-US";
static TEMP_PATH_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn run_build(args: &[String], verbose: u8) -> Result<i32> {
    run_dotnet_with_binlog("build", args, verbose)
}

pub fn run_test(args: &[String], verbose: u8) -> Result<i32> {
    run_dotnet_with_binlog("test", args, verbose)
}

pub fn run_restore(args: &[String], verbose: u8) -> Result<i32> {
    run_dotnet_with_binlog("restore", args, verbose)
}

pub fn run_format(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let (report_path, cleanup_report_path) = resolve_format_report_path(args);
    let mut cmd = resolved_command("dotnet");
    cmd.env(DOTNET_CLI_UI_LANGUAGE, DOTNET_CLI_UI_LANGUAGE_VALUE);
    cmd.arg("format");

    for arg in build_effective_dotnet_format_args(args, report_path.as_deref()) {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: dotnet format {}", args.join(" "));
    }

    let command_started_at = SystemTime::now();
    let output = cmd.output().context("Failed to run dotnet format")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let check_mode = !has_write_mode_override(args);
    let filtered =
        format_report_summary_or_raw(report_path.as_deref(), check_mode, &raw, command_started_at);
    println!("{}", filtered);

    timer.track(
        &format!("dotnet format {}", args.join(" ")),
        &format!("rtk dotnet format {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if cleanup_report_path {
        if let Some(path) = report_path.as_deref() {
            cleanup_temp_file(path);
        }
    }

    Ok(exit_code_from_output(&output, "dotnet"))
}

pub fn run_passthrough(args: &[OsString], verbose: u8) -> Result<i32> {
    if args.is_empty() {
        anyhow::bail!("dotnet: no subcommand specified");
    }

    let timer = tracking::TimedExecution::start();
    let subcommand = args[0].to_string_lossy().to_string();

    let mut cmd = resolved_command("dotnet");
    cmd.env(DOTNET_CLI_UI_LANGUAGE, DOTNET_CLI_UI_LANGUAGE_VALUE);
    cmd.arg(&subcommand);
    for arg in &args[1..] {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: dotnet {} ...", subcommand);
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run dotnet {}", subcommand))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    print!("{}", stdout);
    eprint!("{}", stderr);

    timer.track(
        &format!("dotnet {}", subcommand),
        &format!("rtk dotnet {}", subcommand),
        &raw,
        &raw,
    );

    Ok(exit_code_from_output(&output, "dotnet"))
}

fn run_dotnet_with_binlog(subcommand: &str, args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let binlog_path = build_binlog_path(subcommand);
    let should_expect_binlog = subcommand != "test" || has_binlog_arg(args);

    // For test commands, prefer user-provided results directory; otherwise create isolated one.
    let (trx_results_dir, cleanup_trx_results_dir) = resolve_trx_results_dir(subcommand, args);

    let mut cmd = resolved_command("dotnet");
    cmd.env(DOTNET_CLI_UI_LANGUAGE, DOTNET_CLI_UI_LANGUAGE_VALUE);
    cmd.arg(subcommand);

    for arg in
        build_effective_dotnet_args(subcommand, args, &binlog_path, trx_results_dir.as_deref())
    {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: dotnet {} {}", subcommand, args.join(" "));
    }

    let command_started_at = SystemTime::now();
    let output = cmd
        .output()
        .with_context(|| format!("Failed to run dotnet {}", subcommand))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = match subcommand {
        "build" => {
            let binlog_summary = if should_expect_binlog && binlog_path.exists() {
                normalize_build_summary(
                    binlog::parse_build(&binlog_path).unwrap_or_default(),
                    output.status.success(),
                )
            } else {
                binlog::BuildSummary::default()
            };
            let raw_summary = normalize_build_summary(
                binlog::parse_build_from_text(&raw),
                output.status.success(),
            );
            let summary = merge_build_summaries(binlog_summary, raw_summary);
            format_build_output(&summary, &binlog_path)
        }
        "test" => {
            // First try to parse from binlog/console output
            let parsed_summary = if should_expect_binlog && binlog_path.exists() {
                binlog::parse_test(&binlog_path).unwrap_or_default()
            } else {
                binlog::TestSummary::default()
            };
            let raw_summary = binlog::parse_test_from_text(&raw);
            let merged_summary = merge_test_summaries(parsed_summary, raw_summary);
            let summary = merge_test_summary_from_trx(
                merged_summary,
                trx_results_dir.as_deref(),
                dotnet_trx::find_recent_trx_in_testresults(),
                command_started_at,
            );

            let summary = normalize_test_summary(summary, output.status.success());
            let binlog_diagnostics = if should_expect_binlog && binlog_path.exists() {
                normalize_build_summary(
                    binlog::parse_build(&binlog_path).unwrap_or_default(),
                    output.status.success(),
                )
            } else {
                binlog::BuildSummary::default()
            };
            let raw_diagnostics = normalize_build_summary(
                binlog::parse_build_from_text(&raw),
                output.status.success(),
            );
            let test_build_summary = merge_build_summaries(binlog_diagnostics, raw_diagnostics);
            format_test_output(
                &summary,
                &test_build_summary.errors,
                &test_build_summary.warnings,
                &binlog_path,
            )
        }
        "restore" => {
            let binlog_summary = if should_expect_binlog && binlog_path.exists() {
                normalize_restore_summary(
                    binlog::parse_restore(&binlog_path).unwrap_or_default(),
                    output.status.success(),
                )
            } else {
                binlog::RestoreSummary::default()
            };
            let raw_summary = normalize_restore_summary(
                binlog::parse_restore_from_text(&raw),
                output.status.success(),
            );
            let summary = merge_restore_summaries(binlog_summary, raw_summary);

            let (raw_errors, raw_warnings) = binlog::parse_restore_issues_from_text(&raw);

            format_restore_output(&summary, &raw_errors, &raw_warnings, &binlog_path)
        }
        _ => raw.clone(),
    };

    let output_to_print = if !output.status.success() {
        let stdout_trimmed = stdout.trim();
        let stderr_trimmed = stderr.trim();
        if !stdout_trimmed.is_empty() {
            format!("{}\n\n{}", stdout_trimmed, filtered)
        } else if !stderr_trimmed.is_empty() {
            format!("{}\n\n{}", stderr_trimmed, filtered)
        } else {
            filtered
        }
    } else {
        filtered
    };

    println!("{}", output_to_print);

    timer.track(
        &format!("dotnet {} {}", subcommand, args.join(" ")),
        &format!("rtk dotnet {} {}", subcommand, args.join(" ")),
        &raw,
        &output_to_print,
    );

    cleanup_temp_file(&binlog_path);
    if cleanup_trx_results_dir {
        if let Some(dir) = trx_results_dir.as_deref() {
            cleanup_temp_dir(dir);
        }
    }

    if verbose > 0 {
        eprintln!("Binlog cleaned up: {}", binlog_path.display());
    }

    Ok(exit_code_from_output(&output, "dotnet"))
}

fn build_binlog_path(subcommand: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "rtk_dotnet_{}_{}.binlog",
        subcommand,
        unique_temp_suffix()
    ))
}

fn build_trx_results_dir() -> PathBuf {
    std::env::temp_dir().join(format!("rtk_dotnet_testresults_{}", unique_temp_suffix()))
}

fn unique_temp_suffix() -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let pid = std::process::id();
    let seq = TEMP_PATH_COUNTER.fetch_add(1, Ordering::Relaxed);

    // Keep suffix compact to avoid long temp paths while preserving practical uniqueness.
    format!("{:x}{:x}{:x}", ts, pid, seq)
}

fn resolve_trx_results_dir(subcommand: &str, args: &[String]) -> (Option<PathBuf>, bool) {
    if subcommand != "test" {
        return (None, false);
    }

    if let Some(user_dir) = extract_results_directory_arg(args) {
        return (Some(user_dir), false);
    }

    (Some(build_trx_results_dir()), true)
}

fn build_format_report_path() -> PathBuf {
    std::env::temp_dir().join(format!("rtk_dotnet_format_{}.json", unique_temp_suffix()))
}

fn resolve_format_report_path(args: &[String]) -> (Option<PathBuf>, bool) {
    if let Some(user_report_path) = extract_report_arg(args) {
        return (Some(user_report_path), false);
    }

    (Some(build_format_report_path()), true)
}

fn build_effective_dotnet_format_args(args: &[String], report_path: Option<&Path>) -> Vec<String> {
    let mut effective: Vec<String> = args
        .iter()
        .filter(|arg| !arg.eq_ignore_ascii_case("--write"))
        .cloned()
        .collect();
    let force_write_mode = has_write_mode_override(args);

    if !force_write_mode && !has_verify_no_changes_arg(args) {
        effective.push("--verify-no-changes".to_string());
    }

    if !has_report_arg(args) {
        if let Some(path) = report_path {
            effective.push("--report".to_string());
            effective.push(path.display().to_string());
        }
    }

    effective
}

fn format_report_summary_or_raw(
    report_path: Option<&Path>,
    check_mode: bool,
    raw: &str,
    command_started_at: SystemTime,
) -> String {
    let Some(report_path) = report_path else {
        return raw.to_string();
    };

    if !is_fresh_report(report_path, command_started_at) {
        return raw.to_string();
    }

    match dotnet_format_report::parse_format_report(report_path) {
        Ok(summary) => format_dotnet_format_output(&summary, check_mode),
        Err(_) => raw.to_string(),
    }
}

fn is_fresh_report(path: &Path, command_started_at: SystemTime) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };

    let Ok(modified_at) = metadata.modified() else {
        return false;
    };

    modified_at.duration_since(command_started_at).is_ok()
}

fn format_dotnet_format_output(
    summary: &dotnet_format_report::FormatSummary,
    check_mode: bool,
) -> String {
    let changed_count = summary.files_with_changes.len();

    if changed_count == 0 {
        return format!(
            "ok dotnet format: {} files formatted correctly",
            summary.total_files
        );
    }

    if !check_mode {
        return format!(
            "ok dotnet format: formatted {} files ({} already formatted)",
            changed_count, summary.files_unchanged
        );
    }

    let mut output = format!("Format: {} files need formatting", changed_count);
    output.push_str("\n---------------------------------------");

    for (index, file) in summary.files_with_changes.iter().take(20).enumerate() {
        let first_change = &file.changes[0];
        let rule = if first_change.diagnostic_id.is_empty() {
            first_change.format_description.as_str()
        } else {
            first_change.diagnostic_id.as_str()
        };
        output.push_str(&format!(
            "\n{}. {} (line {}, col {}, {})",
            index + 1,
            file.path,
            first_change.line_number,
            first_change.char_number,
            rule
        ));
    }

    if changed_count > 20 {
        output.push_str(&format!("\n... +{} more files", changed_count - 20));
    }

    output.push_str(&format!(
        "\n\nok {} files already formatted\nRun `dotnet format` to apply fixes",
        summary.files_unchanged
    ));
    output
}

fn cleanup_temp_file(path: &Path) {
    if path.exists() {
        std::fs::remove_file(path).ok();
    }
}

fn cleanup_temp_dir(path: &Path) {
    if path.exists() {
        std::fs::remove_dir_all(path).ok();
    }
}

fn merge_test_summary_from_trx(
    mut summary: binlog::TestSummary,
    trx_results_dir: Option<&Path>,
    fallback_trx_path: Option<PathBuf>,
    command_started_at: SystemTime,
) -> binlog::TestSummary {
    let mut trx_summary = None;

    if let Some(dir) = trx_results_dir.filter(|path| path.exists()) {
        trx_summary = dotnet_trx::parse_trx_files_in_dir_since(dir, Some(command_started_at));

        if trx_summary.is_none() {
            trx_summary = dotnet_trx::parse_trx_files_in_dir(dir);
        }
    }

    if trx_summary.is_none() {
        if let Some(trx) = fallback_trx_path {
            trx_summary = dotnet_trx::parse_trx_file_since(&trx, command_started_at);
        }
    }

    let Some(trx_summary) = trx_summary else {
        return summary;
    };

    if trx_summary.total > 0 && (summary.total == 0 || trx_summary.total >= summary.total) {
        summary.passed = trx_summary.passed;
        summary.failed = trx_summary.failed;
        summary.skipped = trx_summary.skipped;
        summary.total = trx_summary.total;
    }

    if summary.failed_tests.is_empty() && !trx_summary.failed_tests.is_empty() {
        summary.failed_tests = trx_summary.failed_tests;
    }

    if let Some(duration) = trx_summary.duration_text {
        summary.duration_text = Some(duration);
    }

    if trx_summary.project_count > summary.project_count {
        summary.project_count = trx_summary.project_count;
    }

    summary
}

fn build_effective_dotnet_args(
    subcommand: &str,
    args: &[String],
    binlog_path: &Path,
    trx_results_dir: Option<&Path>,
) -> Vec<String> {
    let mut effective = Vec::new();

    if subcommand != "test" && !has_binlog_arg(args) {
        effective.push(format!("-bl:{}", binlog_path.display()));
    }

    if subcommand != "test" && !has_verbosity_arg(args) {
        effective.push("-v:minimal".to_string());
    }

    let runner_mode = if subcommand == "test" {
        detect_test_runner_mode(args)
    } else {
        TestRunnerMode::Classic
    };

    // --nologo: skip for MtpNative — args pass directly to the MTP runtime which
    // does not understand MSBuild/VSTest flags.
    if runner_mode != TestRunnerMode::MtpNative && !has_nologo_arg(args) {
        effective.push("-nologo".to_string());
    }

    if subcommand == "test" {
        match runner_mode {
            TestRunnerMode::Classic => {
                if !has_trx_logger_arg(args) {
                    effective.push("--logger".to_string());
                    effective.push("trx".to_string());
                }
                if !has_results_directory_arg(args) {
                    if let Some(results_dir) = trx_results_dir {
                        effective.push("--results-directory".to_string());
                        effective.push(results_dir.display().to_string());
                    }
                }
                effective.extend(args.iter().cloned());
            }
            TestRunnerMode::MtpNative => {
                // In .NET 10 native MTP mode, --report-trx is a direct dotnet test flag.
                // Modern MTP frameworks (TUnit 1.19.74+, MSTest, xUnit with MTP runner)
                // include Microsoft.Testing.Extensions.TrxReport natively.
                if !has_report_trx_arg(args) {
                    effective.push("--report-trx".to_string());
                }
                effective.extend(args.iter().cloned());
            }
            TestRunnerMode::MtpVsTestBridge => {
                // In VsTestBridge mode (supported on .NET 9 SDK and earlier), --report-trx
                // goes after the -- separator so it reaches the MTP runtime.
                if !has_report_trx_arg(args) {
                    effective.extend(inject_report_trx_into_args(args));
                } else {
                    effective.extend(args.iter().cloned());
                }
            }
        }
    } else {
        effective.extend(args.iter().cloned());
    }

    effective
}

fn has_binlog_arg(args: &[String]) -> bool {
    args.iter().any(|arg| {
        let lower = arg.to_ascii_lowercase();
        lower.starts_with("-bl") || lower.starts_with("/bl")
    })
}

fn has_verbosity_arg(args: &[String]) -> bool {
    args.iter().any(|arg| {
        let lower = arg.to_ascii_lowercase();
        lower.starts_with("-v:")
            || lower.starts_with("/v:")
            || lower == "-v"
            || lower == "/v"
            || lower == "--verbosity"
            || lower.starts_with("--verbosity=")
    })
}

/// How the targeted test project(s) run tests — determines which TRX injection strategy to use.
#[derive(Debug, PartialEq)]
enum TestRunnerMode {
    /// Classic VSTest runner. Inject `--logger trx --results-directory`.
    Classic,
    /// Native MTP runner (`UseMicrosoftTestingPlatformRunner`, `UseTestingPlatformRunner`, or
    /// global.json MTP mode). `--logger trx` breaks the run; inject `--report-trx` directly.
    MtpNative,
    /// VSTest bridge for MTP (`TestingPlatformDotnetTestSupport=true`). `--logger trx` is
    /// silently ignored; MTP args must come after `--`. Inject `-- --report-trx`.
    MtpVsTestBridge,
}

/// Which MTP-related property a single MSBuild file declares.
#[derive(Debug, PartialEq)]
enum MtpProjectKind {
    None,
    VsTestBridge, // UseMicrosoftTestingPlatformRunner | UseTestingPlatformRunner | TestingPlatformDotnetTestSupport
}

/// Scans a single MSBuild file (.csproj / .fsproj / .vbproj / Directory.Build.props) for
/// MTP-related properties and returns which kind it is.
fn scan_mtp_kind_in_file(path: &Path) -> MtpProjectKind {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return MtpProjectKind::None,
    };

    let mut reader = Reader::from_str(&content);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut inside_mtp_element = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name_lower = e.local_name().as_ref().to_ascii_lowercase();
                // All project-file MTP properties run in VSTest bridge mode and require
                // MTP-specific args to come after `--`. Only global.json MTP mode is native.
                inside_mtp_element = matches!(
                    name_lower.as_slice(),
                    b"usemicrosofttestingplatformrunner"
                        | b"usetestingplatformrunner"
                        | b"testingplatformdotnettestsupport"
                );
            }
            Ok(Event::Text(e)) => {
                if inside_mtp_element {
                    if let Ok(text) = e.unescape() {
                        if text.trim().eq_ignore_ascii_case("true") {
                            return MtpProjectKind::VsTestBridge;
                        }
                    }
                }
            }
            Ok(Event::End(_)) => inside_mtp_element = false,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    MtpProjectKind::None
}

fn parse_global_json_mtp_mode(path: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<Value>(&content) else {
        return false;
    };
    json.get("test")
        .and_then(|t| t.get("runner"))
        .and_then(|r| r.as_str())
        .is_some_and(|r| r.eq_ignore_ascii_case("Microsoft.Testing.Platform"))
}

/// Checks whether the `global.json` closest to the current directory enables the .NET 10
/// native MTP mode (`"test": { "runner": "Microsoft.Testing.Platform" }`).
fn is_global_json_mtp_mode() -> bool {
    let Ok(mut dir) = std::env::current_dir() else {
        return false;
    };
    loop {
        let path = dir.join("global.json");
        if path.exists() {
            let is_mtp = parse_global_json_mtp_mode(&path);
            return is_mtp; // stop at first global.json found, regardless of result
        }
        if !dir.pop() {
            break;
        }
    }
    false
}

/// Detects which test runner mode the targeted project(s) use.
///
/// Priority order: global.json (MtpNative) > project-file/Directory.Build.props (MtpVsTestBridge) > Classic.
/// `global.json` MTP mode is checked first because it overrides all project-level properties.
fn detect_test_runner_mode(args: &[String]) -> TestRunnerMode {
    // global.json MTP mode takes overall precedence — when set, dotnet test runs MTP
    // natively regardless of project file properties.
    if is_global_json_mtp_mode() {
        return TestRunnerMode::MtpNative;
    }

    let project_extensions = ["csproj", "fsproj", "vbproj"];

    let explicit_projects: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| {
            let lower = a.to_ascii_lowercase();
            project_extensions
                .iter()
                .any(|ext| lower.ends_with(&format!(".{ext}")))
        })
        .collect();

    let mut found = MtpProjectKind::None;

    if !explicit_projects.is_empty() {
        for p in &explicit_projects {
            if scan_mtp_kind_in_file(Path::new(p)) == MtpProjectKind::VsTestBridge {
                found = MtpProjectKind::VsTestBridge;
            }
        }
    } else {
        // No explicit project — scan current directory.
        if let Ok(entries) = std::fs::read_dir(".") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy().to_ascii_lowercase();
                if project_extensions
                    .iter()
                    .any(|ext| name_str.ends_with(&format!(".{ext}")))
                    && scan_mtp_kind_in_file(&entry.path()) == MtpProjectKind::VsTestBridge
                {
                    found = MtpProjectKind::VsTestBridge;
                }
            }
        }
    }

    if found == MtpProjectKind::VsTestBridge {
        return TestRunnerMode::MtpVsTestBridge;
    }

    // Walk up from current directory looking for Directory.Build.props.
    if let Ok(mut dir) = std::env::current_dir() {
        loop {
            let props = dir.join("Directory.Build.props");
            if props.exists() {
                if scan_mtp_kind_in_file(&props) == MtpProjectKind::VsTestBridge {
                    return TestRunnerMode::MtpVsTestBridge;
                }
                break; // only read the first (closest) Directory.Build.props
            }
            if !dir.pop() {
                break;
            }
        }
    }

    TestRunnerMode::Classic
}

fn has_nologo_arg(args: &[String]) -> bool {
    args.iter()
        .any(|arg| matches!(arg.to_ascii_lowercase().as_str(), "-nologo" | "/nologo"))
}

fn has_trx_logger_arg(args: &[String]) -> bool {
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        let lower = arg.to_ascii_lowercase();
        if lower == "--logger" {
            if let Some(next) = iter.peek() {
                let next_lower = next.to_ascii_lowercase();
                if next_lower == "trx" || next_lower.starts_with("trx;") {
                    return true;
                }
            }
            continue;
        }

        for prefix in ["--logger:", "--logger="] {
            if let Some(value) = lower.strip_prefix(prefix) {
                if value == "trx" || value.starts_with("trx;") {
                    return true;
                }
            }
        }
    }

    false
}

fn has_results_directory_arg(args: &[String]) -> bool {
    args.iter().any(|arg| {
        let lower = arg.to_ascii_lowercase();
        lower == "--results-directory" || lower.starts_with("--results-directory=")
    })
}

fn has_report_arg(args: &[String]) -> bool {
    args.iter().any(|arg| {
        let lower = arg.to_ascii_lowercase();
        lower == "--report" || lower.starts_with("--report=")
    })
}

fn has_report_trx_arg(args: &[String]) -> bool {
    args.iter().any(|a| a.eq_ignore_ascii_case("--report-trx"))
}

/// Injects `--report-trx` after the `--` separator in `args`.
/// If no `--` separator exists, appends `-- --report-trx` at the end.
fn inject_report_trx_into_args(args: &[String]) -> Vec<String> {
    if let Some(sep) = args.iter().position(|a| a == "--") {
        let mut result = args.to_vec();
        result.insert(sep + 1, "--report-trx".to_string());
        result
    } else {
        let mut result = args.to_vec();
        result.push("--".to_string());
        result.push("--report-trx".to_string());
        result
    }
}

fn extract_report_arg(args: &[String]) -> Option<PathBuf> {
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if arg.eq_ignore_ascii_case("--report") {
            if let Some(next) = iter.peek() {
                return Some(PathBuf::from(next.as_str()));
            }
            continue;
        }

        if let Some((_, value)) = arg.split_once('=') {
            if arg
                .split('=')
                .next()
                .is_some_and(|key| key.eq_ignore_ascii_case("--report"))
            {
                return Some(PathBuf::from(value));
            }
        }
    }

    None
}

fn has_verify_no_changes_arg(args: &[String]) -> bool {
    args.iter().any(|arg| {
        let lower = arg.to_ascii_lowercase();
        lower == "--verify-no-changes" || lower.starts_with("--verify-no-changes=")
    })
}

fn has_write_mode_override(args: &[String]) -> bool {
    args.iter().any(|arg| arg.eq_ignore_ascii_case("--write"))
}

fn extract_results_directory_arg(args: &[String]) -> Option<PathBuf> {
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if arg.eq_ignore_ascii_case("--results-directory") {
            if let Some(next) = iter.peek() {
                return Some(PathBuf::from(next.as_str()));
            }
            continue;
        }

        if let Some((_, value)) = arg.split_once('=') {
            if arg
                .split('=')
                .next()
                .is_some_and(|key| key.eq_ignore_ascii_case("--results-directory"))
            {
                return Some(PathBuf::from(value));
            }
        }
    }

    None
}

fn normalize_build_summary(
    mut summary: binlog::BuildSummary,
    command_success: bool,
) -> binlog::BuildSummary {
    if command_success {
        summary.succeeded = true;
        if summary.project_count == 0 {
            summary.project_count = 1;
        }
    }

    summary
}

fn merge_build_summaries(
    mut binlog_summary: binlog::BuildSummary,
    raw_summary: binlog::BuildSummary,
) -> binlog::BuildSummary {
    if binlog_summary.errors.is_empty() {
        binlog_summary.errors = raw_summary.errors;
    }
    if binlog_summary.warnings.is_empty() {
        binlog_summary.warnings = raw_summary.warnings;
    }

    if binlog_summary.project_count == 0 {
        binlog_summary.project_count = raw_summary.project_count;
    }
    if binlog_summary.duration_text.is_none() {
        binlog_summary.duration_text = raw_summary.duration_text;
    }

    binlog_summary
}

fn normalize_test_summary(
    mut summary: binlog::TestSummary,
    command_success: bool,
) -> binlog::TestSummary {
    if !command_success && summary.failed == 0 && summary.failed_tests.is_empty() {
        summary.failed = 1;
        if summary.total == 0 {
            summary.total = 1;
        }
    }

    if command_success && summary.total == 0 && summary.passed == 0 {
        summary.project_count = summary.project_count.max(1);
    }

    summary
}

fn merge_test_summaries(
    mut binlog_summary: binlog::TestSummary,
    raw_summary: binlog::TestSummary,
) -> binlog::TestSummary {
    if binlog_summary.total == 0 && raw_summary.total > 0 {
        binlog_summary.passed = raw_summary.passed;
        binlog_summary.failed = raw_summary.failed;
        binlog_summary.skipped = raw_summary.skipped;
        binlog_summary.total = raw_summary.total;
    }

    if !raw_summary.failed_tests.is_empty() {
        binlog_summary.failed_tests = raw_summary.failed_tests;
    }

    if binlog_summary.project_count == 0 {
        binlog_summary.project_count = raw_summary.project_count;
    }

    if binlog_summary.duration_text.is_none() {
        binlog_summary.duration_text = raw_summary.duration_text;
    }

    binlog_summary
}

fn normalize_restore_summary(
    mut summary: binlog::RestoreSummary,
    command_success: bool,
) -> binlog::RestoreSummary {
    if !command_success && summary.errors == 0 {
        summary.errors = 1;
    }

    summary
}

fn merge_restore_summaries(
    mut binlog_summary: binlog::RestoreSummary,
    raw_summary: binlog::RestoreSummary,
) -> binlog::RestoreSummary {
    if binlog_summary.restored_projects == 0 {
        binlog_summary.restored_projects = raw_summary.restored_projects;
    }
    if binlog_summary.errors == 0 {
        binlog_summary.errors = raw_summary.errors;
    }
    if binlog_summary.warnings == 0 {
        binlog_summary.warnings = raw_summary.warnings;
    }
    if binlog_summary.duration_text.is_none() {
        binlog_summary.duration_text = raw_summary.duration_text;
    }

    binlog_summary
}

fn format_issue(issue: &binlog::BinlogIssue, kind: &str) -> String {
    if issue.file.is_empty() {
        return format!("  {} {}", kind, truncate(&issue.message, 180));
    }
    if issue.code.is_empty() {
        return format!(
            "  {}({},{}) {}: {}",
            issue.file,
            issue.line,
            issue.column,
            kind,
            truncate(&issue.message, 180)
        );
    }
    format!(
        "  {}({},{}) {} {}: {}",
        issue.file,
        issue.line,
        issue.column,
        kind,
        issue.code,
        truncate(&issue.message, 180)
    )
}

fn format_build_output(summary: &binlog::BuildSummary, _binlog_path: &Path) -> String {
    let status_icon = if summary.succeeded { "ok" } else { "fail" };
    let duration = summary.duration_text.as_deref().unwrap_or("unknown");

    let mut out = format!(
        "{} dotnet build: {} projects, {} errors, {} warnings ({})",
        status_icon,
        summary.project_count,
        summary.errors.len(),
        summary.warnings.len(),
        duration
    );

    if !summary.errors.is_empty() {
        out.push_str("\n---------------------------------------\n\nErrors:\n");
        for issue in summary.errors.iter().take(20) {
            out.push_str(&format!("{}\n", format_issue(issue, "error")));
        }
        if summary.errors.len() > 20 {
            out.push_str(&format!(
                "  ... +{} more errors\n",
                summary.errors.len() - 20
            ));
        }
    }

    if !summary.warnings.is_empty() {
        out.push_str("\nWarnings:\n");
        for issue in summary.warnings.iter().take(10) {
            out.push_str(&format!("{}\n", format_issue(issue, "warning")));
        }
        if summary.warnings.len() > 10 {
            out.push_str(&format!(
                "  ... +{} more warnings\n",
                summary.warnings.len() - 10
            ));
        }
    }

    // Binlog path omitted from output (temp file, already cleaned up)
    out
}

fn format_test_output(
    summary: &binlog::TestSummary,
    errors: &[binlog::BinlogIssue],
    warnings: &[binlog::BinlogIssue],
    _binlog_path: &Path,
) -> String {
    let has_failures = summary.failed > 0 || !summary.failed_tests.is_empty();
    let status_icon = if has_failures { "fail" } else { "ok" };
    let duration = summary.duration_text.as_deref().unwrap_or("unknown");
    let warning_count = warnings.len();
    let counts_unavailable = summary.passed == 0
        && summary.failed == 0
        && summary.skipped == 0
        && summary.total == 0
        && summary.failed_tests.is_empty();

    let mut out = if counts_unavailable {
        format!(
            "{} dotnet test: completed (binlog-only mode, counts unavailable, {} warnings) ({})",
            status_icon, warning_count, duration
        )
    } else if has_failures {
        format!(
            "{} dotnet test: {} passed, {} failed, {} skipped, {} warnings in {} projects ({})",
            status_icon,
            summary.passed,
            summary.failed,
            summary.skipped,
            warning_count,
            summary.project_count,
            duration
        )
    } else {
        format!(
            "{} dotnet test: {} tests passed, {} warnings in {} projects ({})",
            status_icon, summary.passed, warning_count, summary.project_count, duration
        )
    };

    if has_failures && !summary.failed_tests.is_empty() {
        out.push_str("\n---------------------------------------\n\nFailed Tests:\n");
        for failed in summary.failed_tests.iter().take(15) {
            out.push_str(&format!("  {}\n", failed.name));
            for detail in &failed.details {
                out.push_str(&format!("    {}\n", truncate(detail, 320)));
            }
            out.push('\n');
        }
        if summary.failed_tests.len() > 15 {
            out.push_str(&format!(
                "... +{} more failed tests\n",
                summary.failed_tests.len() - 15
            ));
        }
    }

    if !errors.is_empty() {
        out.push_str("\nErrors:\n");
        for issue in errors.iter().take(10) {
            out.push_str(&format!("{}\n", format_issue(issue, "error")));
        }
        if errors.len() > 10 {
            out.push_str(&format!("  ... +{} more errors\n", errors.len() - 10));
        }
    }

    if !warnings.is_empty() {
        out.push_str("\nWarnings:\n");
        for issue in warnings.iter().take(10) {
            out.push_str(&format!("{}\n", format_issue(issue, "warning")));
        }
        if warnings.len() > 10 {
            out.push_str(&format!("  ... +{} more warnings\n", warnings.len() - 10));
        }
    }

    // Binlog path omitted from output (temp file, already cleaned up)
    out
}

fn format_restore_output(
    summary: &binlog::RestoreSummary,
    errors: &[binlog::BinlogIssue],
    warnings: &[binlog::BinlogIssue],
    _binlog_path: &Path,
) -> String {
    let has_errors = summary.errors > 0;
    let status_icon = if has_errors { "fail" } else { "ok" };
    let duration = summary.duration_text.as_deref().unwrap_or("unknown");

    let mut out = format!(
        "{} dotnet restore: {} projects, {} errors, {} warnings ({})",
        status_icon, summary.restored_projects, summary.errors, summary.warnings, duration
    );

    if !errors.is_empty() {
        out.push_str("\n---------------------------------------\n\nErrors:\n");
        for issue in errors.iter().take(20) {
            out.push_str(&format!("{}\n", format_issue(issue, "error")));
        }
        if errors.len() > 20 {
            out.push_str(&format!("  ... +{} more errors\n", errors.len() - 20));
        }
    }

    if !warnings.is_empty() {
        out.push_str("\nWarnings:\n");
        for issue in warnings.iter().take(10) {
            out.push_str(&format!("{}\n", format_issue(issue, "warning")));
        }
        if warnings.len() > 10 {
            out.push_str(&format!("  ... +{} more warnings\n", warnings.len() - 10));
        }
    }

    // Binlog path omitted from output (temp file, already cleaned up)
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dotnet_format_report;
    use std::fs;
    use std::time::Duration;

    fn build_dotnet_args_for_test(
        subcommand: &str,
        args: &[String],
        with_trx: bool,
    ) -> Vec<String> {
        let binlog_path = Path::new("/tmp/test.binlog");
        let trx_results_dir = if with_trx {
            Some(Path::new("/tmp/test results"))
        } else {
            None
        };

        build_effective_dotnet_args(subcommand, args, binlog_path, trx_results_dir)
    }

    fn trx_with_counts(total: usize, passed: usize, failed: usize) -> String {
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<TestRun xmlns="http://microsoft.com/schemas/VisualStudio/TeamTest/2010">
  <ResultSummary outcome="Completed">
    <Counters total="{}" executed="{}" passed="{}" failed="{}" error="0" />
  </ResultSummary>
</TestRun>"#,
            total, total, passed, failed
        )
    }

    fn format_fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("dotnet")
            .join(name)
    }

    #[test]
    fn test_has_binlog_arg_detects_variants() {
        let args = vec!["-bl:my.binlog".to_string()];
        assert!(has_binlog_arg(&args));

        let args = vec!["/bl".to_string()];
        assert!(has_binlog_arg(&args));

        let args = vec!["--configuration".to_string(), "Release".to_string()];
        assert!(!has_binlog_arg(&args));
    }

    #[test]
    fn test_format_build_output_includes_errors_and_warnings() {
        let summary = binlog::BuildSummary {
            succeeded: false,
            project_count: 2,
            errors: vec![binlog::BinlogIssue {
                code: "CS0103".to_string(),
                file: "src/Program.cs".to_string(),
                line: 42,
                column: 15,
                message: "The name 'foo' does not exist".to_string(),
            }],
            warnings: vec![binlog::BinlogIssue {
                code: "CS0219".to_string(),
                file: "src/Program.cs".to_string(),
                line: 25,
                column: 10,
                message: "Variable 'x' is assigned but never used".to_string(),
            }],
            duration_text: Some("00:00:04.20".to_string()),
        };

        let output = format_build_output(&summary, Path::new("/tmp/build.binlog"));
        assert!(output.contains("dotnet build: 2 projects, 1 errors, 1 warnings"));
        assert!(output.contains("error CS0103"));
        assert!(output.contains("warning CS0219"));
    }

    #[test]
    fn test_format_test_output_shows_failures() {
        let summary = binlog::TestSummary {
            passed: 10,
            failed: 1,
            skipped: 0,
            total: 11,
            project_count: 1,
            failed_tests: vec![binlog::FailedTest {
                name: "MyTests.ShouldFail".to_string(),
                details: vec!["Assert.Equal failure".to_string()],
            }],
            duration_text: Some("1 s".to_string()),
        };

        let output = format_test_output(&summary, &[], &[], Path::new("/tmp/test.binlog"));
        assert!(output.contains("10 passed, 1 failed"));
        assert!(output.contains("MyTests.ShouldFail"));
    }

    #[test]
    fn test_format_test_output_surfaces_warnings() {
        let summary = binlog::TestSummary {
            passed: 940,
            failed: 0,
            skipped: 7,
            total: 947,
            project_count: 1,
            failed_tests: Vec::new(),
            duration_text: Some("1 s".to_string()),
        };

        let warnings = vec![binlog::BinlogIssue {
            code: String::new(),
            file: "/sdk/Microsoft.TestPlatform.targets".to_string(),
            line: 48,
            column: 5,
            message: "Violators:".to_string(),
        }];

        let output = format_test_output(&summary, &[], &warnings, Path::new("/tmp/test.binlog"));
        assert!(output.contains("940 tests passed, 1 warnings"));
        assert!(output.contains("Warnings:"));
        assert!(output.contains("Microsoft.TestPlatform.targets"));
    }

    #[test]
    fn test_format_test_output_surfaces_errors() {
        let summary = binlog::TestSummary {
            passed: 939,
            failed: 1,
            skipped: 7,
            total: 947,
            project_count: 1,
            failed_tests: Vec::new(),
            duration_text: Some("1 s".to_string()),
        };

        let errors = vec![binlog::BinlogIssue {
            code: "TESTERROR".to_string(),
            file: "/repo/MessageMapperTests.cs".to_string(),
            line: 135,
            column: 0,
            message: "CreateInstance_should_initialize_interface_message_type_on_demand"
                .to_string(),
        }];

        let output = format_test_output(&summary, &errors, &[], Path::new("/tmp/test.binlog"));
        assert!(output.contains("Errors:"));
        assert!(output.contains("error TESTERROR"));
        assert!(
            output.contains("CreateInstance_should_initialize_interface_message_type_on_demand")
        );
    }

    #[test]
    fn test_format_restore_output_success() {
        let summary = binlog::RestoreSummary {
            restored_projects: 3,
            warnings: 1,
            errors: 0,
            duration_text: Some("00:00:01.10".to_string()),
        };

        let output = format_restore_output(&summary, &[], &[], Path::new("/tmp/restore.binlog"));
        assert!(output.starts_with("ok dotnet restore"));
        assert!(output.contains("3 projects"));
        assert!(output.contains("1 warnings"));
    }

    #[test]
    fn test_format_restore_output_failure() {
        let summary = binlog::RestoreSummary {
            restored_projects: 2,
            warnings: 0,
            errors: 1,
            duration_text: Some("00:00:01.00".to_string()),
        };

        let output = format_restore_output(&summary, &[], &[], Path::new("/tmp/restore.binlog"));
        assert!(output.starts_with("fail dotnet restore"));
        assert!(output.contains("1 errors"));
    }

    #[test]
    fn test_format_restore_output_includes_error_details() {
        let summary = binlog::RestoreSummary {
            restored_projects: 2,
            warnings: 0,
            errors: 1,
            duration_text: Some("00:00:01.00".to_string()),
        };

        let issues = vec![binlog::BinlogIssue {
            code: "NU1101".to_string(),
            file: "/repo/src/App/App.csproj".to_string(),
            line: 0,
            column: 0,
            message: "Unable to find package Foo.Bar".to_string(),
        }];

        let output =
            format_restore_output(&summary, &issues, &[], Path::new("/tmp/restore.binlog"));
        assert!(output.contains("Errors:"));
        assert!(output.contains("error NU1101"));
        assert!(output.contains("Unable to find package Foo.Bar"));
    }

    #[test]
    fn test_format_test_output_handles_binlog_only_without_counts() {
        let summary = binlog::TestSummary {
            passed: 0,
            failed: 0,
            skipped: 0,
            total: 0,
            project_count: 0,
            failed_tests: Vec::new(),
            duration_text: Some("unknown".to_string()),
        };

        let output = format_test_output(&summary, &[], &[], Path::new("/tmp/test.binlog"));
        assert!(output.contains("counts unavailable"));
    }

    #[test]
    fn test_normalize_build_summary_sets_success_floor() {
        let summary = binlog::BuildSummary {
            succeeded: false,
            project_count: 0,
            errors: Vec::new(),
            warnings: Vec::new(),
            duration_text: None,
        };

        let normalized = normalize_build_summary(summary, true);
        assert!(normalized.succeeded);
        assert_eq!(normalized.project_count, 1);
    }

    #[test]
    fn test_merge_build_summaries_keeps_structured_issues_when_present() {
        let binlog_summary = binlog::BuildSummary {
            succeeded: false,
            project_count: 11,
            errors: vec![binlog::BinlogIssue {
                code: String::new(),
                file: "IDE0055".to_string(),
                line: 0,
                column: 0,
                message: "Fix formatting".to_string(),
            }],
            warnings: Vec::new(),
            duration_text: Some("00:00:03.54".to_string()),
        };

        let raw_summary = binlog::BuildSummary {
            succeeded: false,
            project_count: 2,
            errors: vec![
                binlog::BinlogIssue {
                    code: "IDE0055".to_string(),
                    file: "/repo/src/Behavior.cs".to_string(),
                    line: 13,
                    column: 32,
                    message: "Fix formatting".to_string(),
                },
                binlog::BinlogIssue {
                    code: "IDE0055".to_string(),
                    file: "/repo/src/Behavior.cs".to_string(),
                    line: 13,
                    column: 41,
                    message: "Fix formatting".to_string(),
                },
            ],
            warnings: Vec::new(),
            duration_text: Some("00:00:03.54".to_string()),
        };

        let merged = merge_build_summaries(binlog_summary, raw_summary);
        assert_eq!(merged.project_count, 11);
        assert_eq!(merged.errors.len(), 1);
        assert_eq!(merged.errors[0].file, "IDE0055");
        assert_eq!(merged.errors[0].line, 0);
        assert_eq!(merged.errors[0].column, 0);
    }

    #[test]
    fn test_merge_build_summaries_keeps_binlog_when_context_is_good() {
        let binlog_summary = binlog::BuildSummary {
            succeeded: false,
            project_count: 2,
            errors: vec![binlog::BinlogIssue {
                code: "CS0103".to_string(),
                file: "src/Program.cs".to_string(),
                line: 42,
                column: 15,
                message: "The name 'foo' does not exist".to_string(),
            }],
            warnings: Vec::new(),
            duration_text: Some("00:00:01.00".to_string()),
        };

        let raw_summary = binlog::BuildSummary {
            succeeded: false,
            project_count: 2,
            errors: vec![binlog::BinlogIssue {
                code: "CS0103".to_string(),
                file: String::new(),
                line: 0,
                column: 0,
                message: "Build error #1 (details omitted)".to_string(),
            }],
            warnings: Vec::new(),
            duration_text: None,
        };

        let merged = merge_build_summaries(binlog_summary.clone(), raw_summary);
        assert_eq!(merged.errors, binlog_summary.errors);
    }

    #[test]
    fn test_normalize_test_summary_sets_failure_floor() {
        let summary = binlog::TestSummary {
            passed: 0,
            failed: 0,
            skipped: 0,
            total: 0,
            project_count: 0,
            failed_tests: Vec::new(),
            duration_text: None,
        };

        let normalized = normalize_test_summary(summary, false);
        assert_eq!(normalized.failed, 1);
        assert_eq!(normalized.total, 1);
    }

    #[test]
    fn test_merge_test_summaries_keeps_structured_counts_and_fills_failed_tests() {
        let binlog_summary = binlog::TestSummary {
            passed: 939,
            failed: 1,
            skipped: 8,
            total: 948,
            project_count: 1,
            failed_tests: Vec::new(),
            duration_text: Some("unknown".to_string()),
        };

        let raw_summary = binlog::TestSummary {
            passed: 939,
            failed: 1,
            skipped: 7,
            total: 947,
            project_count: 0,
            failed_tests: vec![binlog::FailedTest {
                name: "MessageMapperTests.CreateInstance_should_initialize_interface_message_type_on_demand"
                    .to_string(),
                details: vec!["Assert.That(messageInstance, Is.Null)".to_string()],
            }],
            duration_text: Some("1 s".to_string()),
        };

        let merged = merge_test_summaries(binlog_summary, raw_summary);
        assert_eq!(merged.skipped, 8);
        assert_eq!(merged.total, 948);
        assert_eq!(merged.failed_tests.len(), 1);
        assert!(merged.failed_tests[0]
            .name
            .contains("CreateInstance_should_initialize"));
    }

    #[test]
    fn test_normalize_restore_summary_sets_error_floor_on_failed_command() {
        let summary = binlog::RestoreSummary {
            restored_projects: 2,
            warnings: 0,
            errors: 0,
            duration_text: None,
        };

        let normalized = normalize_restore_summary(summary, false);
        assert_eq!(normalized.errors, 1);
    }

    #[test]
    fn test_merge_restore_summaries_prefers_raw_error_count() {
        let binlog_summary = binlog::RestoreSummary {
            restored_projects: 2,
            warnings: 0,
            errors: 0,
            duration_text: Some("unknown".to_string()),
        };

        let raw_summary = binlog::RestoreSummary {
            restored_projects: 0,
            warnings: 0,
            errors: 1,
            duration_text: Some("unknown".to_string()),
        };

        let merged = merge_restore_summaries(binlog_summary, raw_summary);
        assert_eq!(merged.errors, 1);
        assert_eq!(merged.restored_projects, 2);
    }

    #[test]
    fn test_forwarding_args_with_spaces() {
        let args = vec![
            "--filter".to_string(),
            "FullyQualifiedName~MyTests.Calculator*".to_string(),
            "-c".to_string(),
            "Release".to_string(),
        ];

        let injected = build_dotnet_args_for_test("test", &args, true);
        assert!(injected.contains(&"--filter".to_string()));
        assert!(injected.contains(&"FullyQualifiedName~MyTests.Calculator*".to_string()));
        assert!(injected.contains(&"-c".to_string()));
        assert!(injected.contains(&"Release".to_string()));
    }

    #[test]
    fn test_forwarding_config_and_framework() {
        let args = vec![
            "--configuration".to_string(),
            "Release".to_string(),
            "--framework".to_string(),
            "net8.0".to_string(),
        ];

        let injected = build_dotnet_args_for_test("test", &args, true);
        assert!(injected.contains(&"--configuration".to_string()));
        assert!(injected.contains(&"Release".to_string()));
        assert!(injected.contains(&"--framework".to_string()));
        assert!(injected.contains(&"net8.0".to_string()));
    }

    #[test]
    fn test_forwarding_project_file() {
        let args = vec![
            "--project".to_string(),
            "src/My App.Tests/My App.Tests.csproj".to_string(),
        ];

        let injected = build_dotnet_args_for_test("test", &args, true);
        assert!(injected.contains(&"--project".to_string()));
        assert!(injected.contains(&"src/My App.Tests/My App.Tests.csproj".to_string()));
    }

    #[test]
    fn test_forwarding_no_build_and_no_restore() {
        let args = vec!["--no-build".to_string(), "--no-restore".to_string()];

        let injected = build_dotnet_args_for_test("test", &args, true);
        assert!(injected.contains(&"--no-build".to_string()));
        assert!(injected.contains(&"--no-restore".to_string()));
    }

    #[test]
    fn test_user_verbose_override() {
        let args = vec!["-v:detailed".to_string()];

        let injected = build_dotnet_args_for_test("test", &args, true);
        let verbose_count = injected.iter().filter(|a| a.starts_with("-v:")).count();
        assert_eq!(verbose_count, 1);
        assert!(injected.contains(&"-v:detailed".to_string()));
        assert!(!injected.contains(&"-v:minimal".to_string()));
    }

    #[test]
    fn test_user_long_verbosity_override() {
        let args = vec!["--verbosity".to_string(), "detailed".to_string()];

        let injected = build_dotnet_args_for_test("build", &args, false);
        assert!(injected.contains(&"--verbosity".to_string()));
        assert!(injected.contains(&"detailed".to_string()));
        assert!(!injected.contains(&"-v:minimal".to_string()));
    }

    #[test]
    fn test_test_subcommand_does_not_inject_minimal_verbosity_by_default() {
        let args = Vec::<String>::new();

        let injected = build_dotnet_args_for_test("test", &args, true);
        assert!(!injected.contains(&"-v:minimal".to_string()));
    }

    #[test]
    fn test_user_logger_override() {
        let args = vec![
            "--logger".to_string(),
            "console;verbosity=detailed".to_string(),
        ];

        let injected = build_dotnet_args_for_test("test", &args, true);
        assert!(injected.contains(&"--logger".to_string()));
        assert!(injected.contains(&"console;verbosity=detailed".to_string()));
        assert!(injected.iter().any(|a| a == "trx"));
        assert!(injected.iter().any(|a| a == "--results-directory"));
    }

    #[test]
    fn test_trx_logger_and_results_directory_injected() {
        let args = Vec::<String>::new();

        let injected = build_dotnet_args_for_test("test", &args, true);
        assert!(injected.contains(&"--logger".to_string()));
        assert!(injected.contains(&"trx".to_string()));
        assert!(injected.contains(&"--results-directory".to_string()));
        assert!(injected.contains(&"/tmp/test results".to_string()));
    }

    #[test]
    fn test_user_trx_logger_does_not_duplicate() {
        let args = vec!["--logger".to_string(), "trx".to_string()];

        let injected = build_dotnet_args_for_test("test", &args, true);
        let trx_logger_count = injected.iter().filter(|a| *a == "trx").count();
        assert_eq!(trx_logger_count, 1);
    }

    #[test]
    fn test_user_results_directory_prevents_extra_injection() {
        let args = vec![
            "--results-directory".to_string(),
            "/custom/results".to_string(),
        ];

        let injected = build_dotnet_args_for_test("test", &args, true);
        assert!(!injected
            .windows(2)
            .any(|w| w[0] == "--results-directory" && w[1] == "/tmp/test results"));
        assert!(injected
            .windows(2)
            .any(|w| w[0] == "--results-directory" && w[1] == "/custom/results"));
    }

    #[test]
    fn test_scan_mtp_kind_detects_use_microsoft_testing_platform_runner() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let csproj = temp_dir.path().join("MyProject.csproj");
        fs::write(
            &csproj,
            r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <UseMicrosoftTestingPlatformRunner>true</UseMicrosoftTestingPlatformRunner>
  </PropertyGroup>
</Project>"#,
        )
        .expect("write csproj");

        assert_eq!(scan_mtp_kind_in_file(&csproj), MtpProjectKind::VsTestBridge);
    }

    #[test]
    fn test_scan_mtp_kind_detects_use_testing_platform_runner() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let csproj = temp_dir.path().join("MyProject.csproj");
        fs::write(
            &csproj,
            r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <UseTestingPlatformRunner>true</UseTestingPlatformRunner>
  </PropertyGroup>
</Project>"#,
        )
        .expect("write csproj");

        assert_eq!(scan_mtp_kind_in_file(&csproj), MtpProjectKind::VsTestBridge);
    }

    #[test]
    fn test_is_mtp_project_file_returns_false_for_classic_vstest() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let csproj = temp_dir.path().join("MyProject.csproj");
        fs::write(
            &csproj,
            r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>net9.0</TargetFramework>
  </PropertyGroup>
  <ItemGroup>
    <PackageReference Include="xunit" Version="2.9.0" />
  </ItemGroup>
</Project>"#,
        )
        .expect("write csproj");

        assert_eq!(scan_mtp_kind_in_file(&csproj), MtpProjectKind::None);
    }

    #[test]
    fn test_scan_mtp_kind_returns_none_when_value_is_false() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let csproj = temp_dir.path().join("MyProject.csproj");
        fs::write(
            &csproj,
            r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <UseMicrosoftTestingPlatformRunner>false</UseMicrosoftTestingPlatformRunner>
  </PropertyGroup>
</Project>"#,
        )
        .expect("write csproj");

        assert_eq!(scan_mtp_kind_in_file(&csproj), MtpProjectKind::None);
    }

    #[test]
    fn test_scan_mtp_kind_detects_vstest_bridge() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let csproj = temp_dir.path().join("MSTest.Tests.csproj");
        fs::write(
            &csproj,
            r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TestingPlatformDotnetTestSupport>true</TestingPlatformDotnetTestSupport>
  </PropertyGroup>
</Project>"#,
        )
        .expect("write csproj");

        assert_eq!(scan_mtp_kind_in_file(&csproj), MtpProjectKind::VsTestBridge);
    }

    #[test]
    fn test_both_mtp_properties_in_same_file_still_vstest_bridge() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let csproj = temp_dir.path().join("Hybrid.Tests.csproj");
        fs::write(
            &csproj,
            r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TestingPlatformDotnetTestSupport>true</TestingPlatformDotnetTestSupport>
    <UseMicrosoftTestingPlatformRunner>true</UseMicrosoftTestingPlatformRunner>
  </PropertyGroup>
</Project>"#,
        )
        .expect("write csproj");

        // All project-file properties → VsTestBridge; only global.json gives MtpNative
        assert_eq!(scan_mtp_kind_in_file(&csproj), MtpProjectKind::VsTestBridge);
    }

    #[test]
    fn test_detect_mode_mtp_csproj_is_vstest_bridge_injects_report_trx() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let csproj = temp_dir.path().join("MTP.Tests.csproj");
        fs::write(
            &csproj,
            r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <UseMicrosoftTestingPlatformRunner>true</UseMicrosoftTestingPlatformRunner>
  </PropertyGroup>
</Project>"#,
        )
        .expect("write csproj");

        let args = vec![csproj.display().to_string()];
        assert_eq!(
            detect_test_runner_mode(&args),
            TestRunnerMode::MtpVsTestBridge
        );

        let binlog_path = Path::new("/tmp/test.binlog");
        let injected = build_effective_dotnet_args("test", &args, binlog_path, None);

        // MTP VsTestBridge → --report-trx injected after --, no VSTest --logger trx
        assert!(!injected.contains(&"--logger".to_string()));
        assert!(injected.contains(&"--report-trx".to_string()));
        assert!(injected.contains(&"--".to_string()));
    }

    #[test]
    fn test_detect_mode_vstest_bridge_injects_report_trx() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let csproj = temp_dir.path().join("MSTest.Tests.csproj");
        fs::write(
            &csproj,
            r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TestingPlatformDotnetTestSupport>true</TestingPlatformDotnetTestSupport>
  </PropertyGroup>
</Project>"#,
        )
        .expect("write csproj");

        let args = vec![csproj.display().to_string()];
        assert_eq!(
            detect_test_runner_mode(&args),
            TestRunnerMode::MtpVsTestBridge
        );

        let binlog_path = Path::new("/tmp/test.binlog");
        let injected = build_effective_dotnet_args("test", &args, binlog_path, None);

        // --report-trx injected after --, --nologo supported in bridge mode
        assert!(!injected.contains(&"--logger".to_string()));
        assert!(injected.contains(&"--report-trx".to_string()));
        assert!(injected.contains(&"--".to_string()));
        assert!(injected.contains(&"-nologo".to_string()));
    }

    #[test]
    fn test_parse_global_json_mtp_mode_detects_mtp_native() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let global_json = temp_dir.path().join("global.json");
        fs::write(
            &global_json,
            r#"{"sdk":{"version":"10.0.100"},"test":{"runner":"Microsoft.Testing.Platform"}}"#,
        )
        .expect("write global.json");

        assert!(parse_global_json_mtp_mode(&global_json));
    }

    #[test]
    fn test_vstest_bridge_injects_report_trx_after_separator() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let csproj = temp_dir.path().join("MTP.Tests.csproj");
        fs::write(
            &csproj,
            r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <UseMicrosoftTestingPlatformRunner>true</UseMicrosoftTestingPlatformRunner>
  </PropertyGroup>
</Project>"#,
        )
        .expect("write csproj");

        let args = vec![csproj.display().to_string()];
        assert_eq!(
            detect_test_runner_mode(&args),
            TestRunnerMode::MtpVsTestBridge
        );

        let binlog_path = Path::new("/tmp/test.binlog");
        let injected = build_effective_dotnet_args("test", &args, binlog_path, None);

        // VsTestBridge → inject -- --report-trx after user args
        assert!(injected.contains(&"--".to_string()));
        assert!(injected.contains(&"--report-trx".to_string()));
        let sep_pos = injected.iter().position(|a| a == "--").unwrap();
        let trx_pos = injected.iter().position(|a| a == "--report-trx").unwrap();
        assert!(sep_pos < trx_pos);
        // No VSTest logger
        assert!(!injected.contains(&"--logger".to_string()));
    }

    #[test]
    fn test_vstest_bridge_existing_separator_inserts_report_trx_after_it() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let csproj = temp_dir.path().join("MTP.Tests.csproj");
        fs::write(
            &csproj,
            r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <UseMicrosoftTestingPlatformRunner>true</UseMicrosoftTestingPlatformRunner>
  </PropertyGroup>
</Project>"#,
        )
        .expect("write csproj");

        let args = vec![
            csproj.display().to_string(),
            "--".to_string(),
            "--parallel".to_string(),
        ];
        let binlog_path = Path::new("/tmp/test.binlog");
        let injected = build_effective_dotnet_args("test", &args, binlog_path, None);

        // --report-trx inserted right after existing --
        let sep_pos = injected.iter().position(|a| a == "--").unwrap();
        assert_eq!(injected[sep_pos + 1], "--report-trx");
        assert!(injected.contains(&"--parallel".to_string()));
    }

    #[test]
    fn test_vstest_bridge_respects_existing_report_trx() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let csproj = temp_dir.path().join("MTP.Tests.csproj");
        fs::write(
            &csproj,
            r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <UseMicrosoftTestingPlatformRunner>true</UseMicrosoftTestingPlatformRunner>
  </PropertyGroup>
</Project>"#,
        )
        .expect("write csproj");

        let args = vec![
            csproj.display().to_string(),
            "--".to_string(),
            "--report-trx".to_string(),
        ];
        let binlog_path = Path::new("/tmp/test.binlog");
        let injected = build_effective_dotnet_args("test", &args, binlog_path, None);

        // Should not double-inject
        assert_eq!(injected.iter().filter(|a| *a == "--report-trx").count(), 1);
    }

    #[test]
    fn test_detect_mode_classic_csproj_injects_trx() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let csproj = temp_dir.path().join("Classic.Tests.csproj");
        fs::write(
            &csproj,
            r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>net9.0</TargetFramework>
  </PropertyGroup>
</Project>"#,
        )
        .expect("write csproj");

        let args = vec![csproj.display().to_string()];
        assert_eq!(detect_test_runner_mode(&args), TestRunnerMode::Classic);

        let binlog_path = Path::new("/tmp/test.binlog");
        let trx_dir = Path::new("/tmp/test_results");
        let injected = build_effective_dotnet_args("test", &args, binlog_path, Some(trx_dir));
        assert!(injected.contains(&"--logger".to_string()));
        assert!(injected.contains(&"trx".to_string()));
    }

    #[test]
    fn test_detect_mode_directory_build_props_vstest_bridge() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let props = temp_dir.path().join("Directory.Build.props");
        fs::write(
            &props,
            r#"<Project>
  <PropertyGroup>
    <TestingPlatformDotnetTestSupport>true</TestingPlatformDotnetTestSupport>
  </PropertyGroup>
</Project>"#,
        )
        .expect("write Directory.Build.props");

        assert_eq!(scan_mtp_kind_in_file(&props), MtpProjectKind::VsTestBridge);
    }

    #[test]
    fn test_is_global_json_mtp_mode_detects_mtp_runner() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let global_json = temp_dir.path().join("global.json");
        fs::write(
            &global_json,
            r#"{ "sdk": { "version": "10.0.100" }, "test": { "runner": "Microsoft.Testing.Platform" } }"#,
        )
        .expect("write global.json");

        assert!(parse_global_json_mtp_mode(&global_json));
    }

    #[test]
    fn test_is_global_json_mtp_mode_returns_false_for_vstest_runner() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let global_json = temp_dir.path().join("global.json");
        fs::write(&global_json, r#"{ "sdk": { "version": "9.0.100" } }"#)
            .expect("write global.json");

        assert!(!parse_global_json_mtp_mode(&global_json));
    }

    #[test]
    fn test_merge_test_summary_from_trx_uses_primary_and_cleans_file() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let primary = temp_dir.path().join("primary.trx");
        fs::write(&primary, trx_with_counts(3, 3, 0)).expect("write primary trx");

        let filled = merge_test_summary_from_trx(
            binlog::TestSummary::default(),
            Some(temp_dir.path()),
            None,
            SystemTime::now(),
        );

        assert_eq!(filled.total, 3);
        assert_eq!(filled.passed, 3);
        assert!(primary.exists());
    }

    #[test]
    fn test_merge_test_summary_from_trx_falls_back_to_testresults() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let fallback = temp_dir.path().join("fallback.trx");
        fs::write(&fallback, trx_with_counts(2, 1, 1)).expect("write fallback trx");
        let missing_primary = temp_dir.path().join("missing.trx");

        let filled = merge_test_summary_from_trx(
            binlog::TestSummary::default(),
            Some(&missing_primary),
            Some(fallback.clone()),
            UNIX_EPOCH,
        );

        assert_eq!(filled.total, 2);
        assert_eq!(filled.failed, 1);
        assert!(fallback.exists());
    }

    #[test]
    fn test_merge_test_summary_from_trx_returns_default_when_no_trx() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let missing = temp_dir.path().join("missing.trx");

        let filled = merge_test_summary_from_trx(
            binlog::TestSummary::default(),
            Some(&missing),
            None,
            SystemTime::now(),
        );
        assert_eq!(filled.total, 0);
    }

    #[test]
    fn test_merge_test_summary_from_trx_ignores_stale_fallback_file() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let fallback = temp_dir.path().join("fallback.trx");
        fs::write(&fallback, trx_with_counts(2, 1, 1)).expect("write fallback trx");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let command_started_at = SystemTime::now();
        let missing_primary = temp_dir.path().join("missing.trx");

        let filled = merge_test_summary_from_trx(
            binlog::TestSummary::default(),
            Some(&missing_primary),
            Some(fallback.clone()),
            command_started_at,
        );

        assert_eq!(filled.total, 0);
        assert!(fallback.exists());
    }

    #[test]
    fn test_merge_test_summary_from_trx_keeps_larger_existing_counts() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let primary = temp_dir.path().join("primary.trx");
        fs::write(&primary, trx_with_counts(5, 4, 1)).expect("write primary trx");

        let existing = binlog::TestSummary {
            passed: 10,
            failed: 2,
            skipped: 0,
            total: 12,
            project_count: 1,
            failed_tests: Vec::new(),
            duration_text: Some("1 s".to_string()),
        };

        let merged =
            merge_test_summary_from_trx(existing, Some(temp_dir.path()), None, SystemTime::now());
        assert_eq!(merged.total, 12);
        assert_eq!(merged.passed, 10);
        assert_eq!(merged.failed, 2);
    }

    #[test]
    fn test_merge_test_summary_from_trx_overrides_smaller_existing_counts() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let primary = temp_dir.path().join("primary.trx");
        fs::write(&primary, trx_with_counts(12, 10, 2)).expect("write primary trx");

        let existing = binlog::TestSummary {
            passed: 4,
            failed: 1,
            skipped: 0,
            total: 5,
            project_count: 1,
            failed_tests: Vec::new(),
            duration_text: Some("1 s".to_string()),
        };

        let merged =
            merge_test_summary_from_trx(existing, Some(temp_dir.path()), None, SystemTime::now());
        assert_eq!(merged.total, 12);
        assert_eq!(merged.passed, 10);
        assert_eq!(merged.failed, 2);
    }

    #[test]
    fn test_merge_test_summary_from_trx_uses_larger_project_count() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let trx_a = temp_dir.path().join("a.trx");
        let trx_b = temp_dir.path().join("b.trx");
        fs::write(&trx_a, trx_with_counts(2, 2, 0)).expect("write first trx");
        fs::write(&trx_b, trx_with_counts(3, 3, 0)).expect("write second trx");

        let existing = binlog::TestSummary {
            passed: 5,
            failed: 0,
            skipped: 0,
            total: 5,
            project_count: 1,
            failed_tests: Vec::new(),
            duration_text: Some("1 s".to_string()),
        };

        let merged =
            merge_test_summary_from_trx(existing, Some(temp_dir.path()), None, SystemTime::now());
        assert_eq!(merged.project_count, 2);
    }

    #[test]
    fn test_has_results_directory_arg_detects_variants() {
        let args = vec!["--results-directory".to_string(), "/tmp/trx".to_string()];
        assert!(has_results_directory_arg(&args));

        let args = vec!["--results-directory=/tmp/trx".to_string()];
        assert!(has_results_directory_arg(&args));

        let args = vec!["--logger".to_string(), "trx".to_string()];
        assert!(!has_results_directory_arg(&args));
    }

    #[test]
    fn test_extract_results_directory_arg_detects_variants() {
        let args = vec!["--results-directory".to_string(), "/tmp/r1".to_string()];
        assert_eq!(
            extract_results_directory_arg(&args),
            Some(PathBuf::from("/tmp/r1"))
        );

        let args = vec!["--results-directory=/tmp/r2".to_string()];
        assert_eq!(
            extract_results_directory_arg(&args),
            Some(PathBuf::from("/tmp/r2"))
        );
    }

    #[test]
    fn test_resolve_trx_results_dir_user_directory_is_not_marked_for_cleanup() {
        let args = vec![
            "--results-directory".to_string(),
            "/custom/results".to_string(),
        ];

        let (dir, cleanup) = resolve_trx_results_dir("test", &args);
        assert_eq!(dir, Some(PathBuf::from("/custom/results")));
        assert!(!cleanup);
    }

    #[test]
    fn test_resolve_trx_results_dir_generated_directory_is_marked_for_cleanup() {
        let args = Vec::<String>::new();

        let (dir, cleanup) = resolve_trx_results_dir("test", &args);
        assert!(dir.is_some());
        assert!(cleanup);
    }

    #[test]
    fn test_format_all_formatted() {
        let summary =
            dotnet_format_report::parse_format_report(&format_fixture("format_success.json"))
                .expect("parse format report");

        let output = format_dotnet_format_output(&summary, true);
        assert!(output.contains("ok dotnet format: 2 files formatted correctly"));
    }

    #[test]
    fn test_format_needs_formatting() {
        let summary =
            dotnet_format_report::parse_format_report(&format_fixture("format_changes.json"))
                .expect("parse format report");

        let output = format_dotnet_format_output(&summary, true);
        assert!(output.contains("Format: 2 files need formatting"));
        assert!(output.contains("src/Program.cs (line 42, col 17, WHITESPACE)"));
        assert!(output.contains("Run `dotnet format` to apply fixes"));
    }

    #[test]
    fn test_format_temp_file_cleanup() {
        let args = Vec::<String>::new();
        let (report_path, cleanup) = resolve_format_report_path(&args);
        let report_path = report_path.expect("report path");

        assert!(cleanup);
        fs::write(&report_path, "[]").expect("write temp report");
        cleanup_temp_file(&report_path);
        assert!(!report_path.exists());
    }

    #[test]
    fn test_format_user_report_arg_no_cleanup() {
        let args = vec![
            "--report".to_string(),
            "/tmp/user-format-report.json".to_string(),
        ];

        let (report_path, cleanup) = resolve_format_report_path(&args);
        assert_eq!(
            report_path,
            Some(PathBuf::from("/tmp/user-format-report.json"))
        );
        assert!(!cleanup);
    }

    #[test]
    fn test_format_preserves_positional_project_argument_order() {
        let args = vec!["src/App/App.csproj".to_string()];

        let effective =
            build_effective_dotnet_format_args(&args, Some(Path::new("/tmp/report.json")));
        assert_eq!(
            effective.first().map(String::as_str),
            Some("src/App/App.csproj")
        );
    }

    #[test]
    fn test_format_report_summary_ignores_stale_report_file() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let report = temp_dir.path().join("report.json");
        fs::write(&report, "[]").expect("write report");

        let command_started_at = SystemTime::now()
            .checked_add(Duration::from_secs(2))
            .expect("future timestamp");
        let raw = "RAW OUTPUT";

        let output = format_report_summary_or_raw(Some(&report), true, raw, command_started_at);
        assert_eq!(output, raw);
    }

    #[test]
    fn test_format_report_summary_uses_fresh_report_file() {
        let report = format_fixture("format_success.json");
        let raw = "RAW OUTPUT";

        let output = format_report_summary_or_raw(Some(&report), true, raw, UNIX_EPOCH);
        assert!(output.contains("ok dotnet format: 2 files formatted correctly"));
    }

    #[test]
    fn test_cleanup_temp_file_removes_existing_file() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let temp_file = temp_dir.path().join("temp.binlog");
        fs::write(&temp_file, "content").expect("write temp file");

        cleanup_temp_file(&temp_file);

        assert!(!temp_file.exists());
    }

    #[test]
    fn test_cleanup_temp_file_ignores_missing_file() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let missing_file = temp_dir.path().join("missing.binlog");

        cleanup_temp_file(&missing_file);

        assert!(!missing_file.exists());
    }
}
