//! Filters dotnet CLI output — build, test, and format results.

use crate::binlog;
use crate::core::arg_tokenizer::{self, Dialect, Token, TokenKind, ValueSpec};
use crate::core::args_utils;
use crate::core::guard::never_worse;
use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::truncate::{CAP_ERRORS, CAP_LIST, CAP_WARNINGS};
use crate::core::utils::{resolved_command, truncate};
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
    let args = &args_utils::restore_double_dash(args);
    let tokens = tokenize_dotnet_args(args);
    let timer = tracking::TimedExecution::start();
    let (report_path, cleanup_report_path) = resolve_format_report_path(&tokens);
    let mut cmd = resolved_command("dotnet");
    cmd.env(DOTNET_CLI_UI_LANGUAGE, DOTNET_CLI_UI_LANGUAGE_VALUE);
    cmd.arg("format");

    for arg in build_effective_dotnet_format_args(args, &tokens, report_path.as_deref()) {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: dotnet format {}", args.join(" "));
    }

    let command_started_at = SystemTime::now();
    let result = exec_capture(&mut cmd).context("Failed to run dotnet format")?;
    let raw = format!("{}\n{}", result.stdout, result.stderr);

    let check_mode = !has_write_mode_override(&tokens);
    let filtered =
        format_report_summary_or_raw(report_path.as_deref(), check_mode, &raw, command_started_at);
    let shown = never_worse(&raw, &filtered);
    println!("{}", shown);

    timer.track(
        &format!("dotnet format {}", args.join(" ")),
        &format!("rtk dotnet format {}", args.join(" ")),
        &raw,
        shown,
    );

    if cleanup_report_path {
        if let Some(path) = report_path.as_deref() {
            cleanup_temp_file(path);
        }
    }

    Ok(result.exit_code)
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

    let result =
        exec_capture(&mut cmd).with_context(|| format!("Failed to run dotnet {}", subcommand))?;

    let raw = format!("{}\n{}", result.stdout, result.stderr);

    print!("{}", result.stdout);
    eprint!("{}", result.stderr);

    timer.track(
        &format!("dotnet {}", subcommand),
        &format!("rtk dotnet {}", subcommand),
        &raw,
        &raw,
    );

    Ok(result.exit_code)
}

fn run_dotnet_with_binlog(subcommand: &str, args: &[String], verbose: u8) -> Result<i32> {
    let args = &args_utils::restore_double_dash(args);
    let tokens = tokenize_dotnet_args(args);
    let timer = tracking::TimedExecution::start();
    let binlog_path = build_binlog_path(subcommand);
    let should_expect_binlog = subcommand != "test" || has_binlog_arg(&tokens);

    // Once, not once per consumer: it walks the filesystem, and both the results-directory
    // lookup and the injection below have to agree on the answer.
    let runner_mode = if subcommand == "test" {
        detect_test_runner_mode(&tokens)
    } else {
        TestRunnerMode::Classic
    };

    // For test commands, prefer user-provided results directory; otherwise create isolated one.
    let (trx_results_dir, cleanup_trx_results_dir) =
        resolve_trx_results_dir(subcommand, &tokens, runner_mode);

    let mut cmd = resolved_command("dotnet");
    cmd.env(DOTNET_CLI_UI_LANGUAGE, DOTNET_CLI_UI_LANGUAGE_VALUE);
    cmd.arg(subcommand);

    for arg in build_effective_dotnet_args(
        subcommand,
        args,
        &tokens,
        &binlog_path,
        trx_results_dir.as_deref(),
        runner_mode,
    ) {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: dotnet {} {}", subcommand, args.join(" "));
    }

    let command_started_at = SystemTime::now();
    let result =
        exec_capture(&mut cmd).with_context(|| format!("Failed to run dotnet {}", subcommand))?;

    let raw = format!("{}\n{}", result.stdout, result.stderr);
    let command_success = result.success();

    let (filtered, needs_raw_fallback) = match subcommand {
        "build" => {
            let binlog_summary = if should_expect_binlog && binlog_path.exists() {
                normalize_build_summary(
                    binlog::parse_build(&binlog_path).unwrap_or_default(),
                    command_success,
                )
            } else {
                binlog::BuildSummary::default()
            };
            let raw_summary =
                normalize_build_summary(binlog::parse_build_from_text(&raw), command_success);
            let summary = merge_build_summaries(binlog_summary, raw_summary);
            (format_build_output(&summary, &binlog_path), true)
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

            let summary = normalize_test_summary(summary, command_success);
            let binlog_diagnostics = if should_expect_binlog && binlog_path.exists() {
                normalize_build_summary(
                    binlog::parse_build(&binlog_path).unwrap_or_default(),
                    command_success,
                )
            } else {
                binlog::BuildSummary::default()
            };
            let raw_diagnostics =
                normalize_build_summary(binlog::parse_build_from_text(&raw), command_success);
            let test_build_summary = merge_build_summaries(binlog_diagnostics, raw_diagnostics);
            // The `Failed Tests:` section already carries failure detail parsed from
            // TRX/console; skip the raw-stdout prepend when it would only duplicate it.
            // See issue #2501.
            let needs_raw = test_needs_raw_fallback(&summary);
            (
                format_test_output(
                    &summary,
                    &test_build_summary.errors,
                    &test_build_summary.warnings,
                    &binlog_path,
                ),
                needs_raw,
            )
        }
        "restore" => {
            let binlog_summary = if should_expect_binlog && binlog_path.exists() {
                normalize_restore_summary(
                    binlog::parse_restore(&binlog_path).unwrap_or_default(),
                    command_success,
                )
            } else {
                binlog::RestoreSummary::default()
            };
            let raw_summary =
                normalize_restore_summary(binlog::parse_restore_from_text(&raw), command_success);
            let summary = merge_restore_summaries(binlog_summary, raw_summary);

            let (raw_errors, raw_warnings) = binlog::parse_restore_issues_from_text(&raw);

            (
                format_restore_output(&summary, &raw_errors, &raw_warnings, &binlog_path),
                true,
            )
        }
        _ => (raw.clone(), true),
    };

    let output_to_print = compose_failure_output(
        command_success,
        needs_raw_fallback,
        &result.stdout,
        &result.stderr,
        &filtered,
    );

    let shown = never_worse(&raw, &output_to_print);
    println!("{}", shown);

    timer.track(
        &format!("dotnet {} {}", subcommand, args.join(" ")),
        &format!("rtk dotnet {} {}", subcommand, args.join(" ")),
        &raw,
        shown,
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

    Ok(result.exit_code)
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

fn resolve_trx_results_dir(
    subcommand: &str,
    tokens: &[Token<'_>],
    runner_mode: TestRunnerMode,
) -> (Option<PathBuf>, bool) {
    if subcommand != "test" {
        return (None, false);
    }

    if let Some(user_dir) = extract_results_directory_arg(tokens, runner_mode) {
        return (Some(user_dir), false);
    }

    (Some(build_trx_results_dir()), true)
}

fn build_format_report_path() -> PathBuf {
    std::env::temp_dir().join(format!("rtk_dotnet_format_{}.json", unique_temp_suffix()))
}

fn resolve_format_report_path(tokens: &[Token<'_>]) -> (Option<PathBuf>, bool) {
    if let Some(user_report_path) = extract_report_arg(tokens) {
        return (Some(user_report_path), false);
    }

    (Some(build_format_report_path()), true)
}

fn build_effective_dotnet_format_args(
    args: &[String],
    tokens: &[Token<'_>],
    report_path: Option<&Path>,
) -> Vec<String> {
    let force_write_mode = has_write_mode_override(tokens);
    let mut injected: Vec<String> = Vec::new();
    if !force_write_mode && !has_verify_no_changes_arg(tokens) {
        injected.push("--verify-no-changes".to_string());
    }
    if !has_report_arg(tokens) {
        if let Some(path) = report_path {
            injected.push("--report".to_string());
            injected.push(path.display().to_string());
        }
    }

    // Injected flags go before the user's own `--`: dotnet parks everything past it in
    // UnparsedTokens, so a `--verify-no-changes` after the boundary never applies and format
    // rewrites the tree while RTK still reports check mode.
    let boundary = arg_tokenizer::injection_point(tokens, args.len());
    let write_args: Vec<usize> = write_override_tokens(tokens).map(|t| t.source_index).collect();
    let mut effective: Vec<String> = Vec::with_capacity(args.len() + injected.len());
    for (index, arg) in args.iter().enumerate() {
        if index == boundary {
            effective.append(&mut injected);
        }
        if !write_args.contains(&index) {
            effective.push(arg.clone());
        }
    }
    effective.append(&mut injected);

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

    const MAX_FORMAT_FILES: usize = CAP_LIST;
    for (index, file) in summary
        .files_with_changes
        .iter()
        .take(MAX_FORMAT_FILES)
        .enumerate()
    {
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

    if changed_count > MAX_FORMAT_FILES {
        output.push_str(&format!(
            "\n… +{} more files",
            changed_count - MAX_FORMAT_FILES
        ));
        let all_files = summary
            .files_with_changes
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if let Some(hint) = crate::core::tee::force_tee_tail_hint(
            &all_files,
            "dotnet-format-files",
            MAX_FORMAT_FILES + 1,
        ) {
            output.push_str(&format!(" {}", hint));
        }
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
    tokens: &[Token<'_>],
    binlog_path: &Path,
    trx_results_dir: Option<&Path>,
    runner_mode: TestRunnerMode,
) -> Vec<String> {
    let mut effective = Vec::new();

    if subcommand != "test" && !has_binlog_arg(tokens) {
        effective.push(format!("-bl:{}", binlog_path.display()));
    }

    if subcommand != "test" && !has_verbosity_arg(tokens) {
        effective.push("-v:minimal".to_string());
    }

    // --nologo: skip for MtpNative — args pass directly to the MTP runtime which
    // does not understand MSBuild/VSTest flags.
    if runner_mode != TestRunnerMode::MtpNative && !has_nologo_arg(tokens) {
        effective.push("-nologo".to_string());
    }

    if subcommand == "test" {
        match runner_mode {
            TestRunnerMode::Classic => {
                if !has_trx_logger_arg(tokens) {
                    effective.push("--logger".to_string());
                    effective.push("trx".to_string());
                }
                if !has_results_directory_arg(tokens, runner_mode) {
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
                if !has_report_trx_arg(tokens) {
                    effective.push("--report-trx".to_string());
                }
                effective.extend(args.iter().cloned());
            }
            TestRunnerMode::MtpVsTestBridge => {
                // In VsTestBridge mode (supported on .NET 9 SDK and earlier), --report-trx
                // goes after the -- separator so it reaches the MTP runtime.
                if !has_report_trx_arg(tokens) {
                    effective.extend(inject_report_trx_into_args(args, tokens));
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

fn has_binlog_arg(tokens: &[Token<'_>]) -> bool {
    // Unscoped: wherever the user put `-bl`, that is the binlog that gets written, and RTK
    // adding its own would give MSBuild two binary loggers and parse the wrong one.
    arg_tokenizer::has_flag(tokens, Dialect::Msbuild, "bl")
}

fn has_verbosity_arg(tokens: &[Token<'_>]) -> bool {
    dotnet_has_loose_flag(tokens, "v") || dotnet_has_loose_flag(tokens, "verbosity")
}

/// How the targeted test project(s) run tests — determines which TRX injection strategy to use.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
            Ok(Event::Text(e)) if inside_mtp_element => {
                if let Ok(text) = e.unescape() {
                    if text.trim().eq_ignore_ascii_case("true") {
                        return MtpProjectKind::VsTestBridge;
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
    let Ok(json) = crate::core::utils::from_json_str::<Value>(&content) else {
        return false;
    };
    json.get("test")
        .and_then(|t| t.get("runner"))
        .and_then(|r| r.as_str())
        .is_some_and(|r| r.eq_ignore_ascii_case("Microsoft.Testing.Platform"))
}

/// Checks whether the `global.json` closest to the current directory enables the .NET 10
/// native MTP mode (`"test": { "runner": "Microsoft.Testing.Platform" }`).
fn is_global_json_mtp_mode(start_dir: &Path) -> bool {
    let Ok(mut dir) = start_dir.canonicalize() else {
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

/// Detects which test runner mode the targeted project(s) use. Priority: global.json (MtpNative,
/// overrides project-level properties) > project-file/Directory.Build.props (MtpVsTestBridge) >
/// Classic.
///
/// `explicit_projects` below relies on `dotnet_takes_value`'s allowlist being exhaustive; a
/// missing value-taking flag whose value ends in `.csproj`/`.fsproj`/`.vbproj` would be misread
/// as an explicit project path.
fn detect_test_runner_mode(tokens: &[Token<'_>]) -> TestRunnerMode {
    detect_test_runner_mode_in_dir(tokens, Path::new("."))
}

/// `scan_dir` is where every filesystem probe starts: the project-file scan when no project is
/// named, and the upward walks for `global.json` and `Directory.Build.props`. A parameter
/// rather than a hardcoded "." so tests point it at an isolated tempdir instead of racing on
/// the real process cwd -- all three probes have to honour it, or a developer working beneath
/// a `global.json` decides the result and the tempdir assertions become decorative.
fn detect_test_runner_mode_in_dir(tokens: &[Token<'_>], scan_dir: &Path) -> TestRunnerMode {
    // global.json MTP mode takes overall precedence — when set, dotnet test runs MTP
    // natively regardless of project file properties.
    if is_global_json_mtp_mode(scan_dir) {
        return TestRunnerMode::MtpNative;
    }

    let project_extensions = ["csproj", "fsproj", "vbproj"];

    // Candidate project paths: unconsumed positionals before the user's own `--` (tokens past it
    // are forwarded to the test runner, not to dotnet) ending in one of these extensions. A
    // single-segment absolute path (`/Other.csproj`) tokenizes as a slash *flag* -- structure
    // alone can't tell it from a switch, and this tokenizer does no I/O -- so the extension is
    // what settles it: no MSBuild switch is named `.csproj`.
    let boundary = arg_tokenizer::dashdash_index(tokens).map(|i| tokens[i].source_index);
    let is_project = |name: &str| {
        let lower = name.to_ascii_lowercase();
        project_extensions
            .iter()
            .any(|ext| lower.ends_with(&format!(".{ext}")))
    };
    let explicit_projects: Vec<String> = tokens
        .iter()
        .filter(|t| boundary.is_none_or(|b| t.source_index < b))
        .filter_map(|t| {
            if t.is_free_positional() && is_project(t.text) {
                Some(t.text.to_string())
            } else if t.slash && is_project(t.text) {
                Some(format!("/{}", t.text))
            } else {
                None
            }
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
        // No explicit project — scan scan_dir.
        if let Ok(entries) = std::fs::read_dir(scan_dir) {
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

    // Walk up from the scanned directory looking for Directory.Build.props.
    if let Ok(mut dir) = scan_dir.canonicalize() {
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

/// The value-taking flags that matter for RTK's own decisions, not every flag dotnet accepts:
/// a missing entry leaves the value as a free positional, which `detect_test_runner_mode`'s
/// project-path scan then has to filter by extension for exactly that reason.
fn dotnet_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    (kind == TokenKind::Long
        && matches!(
            name.to_ascii_lowercase().as_str(),
            "a" | "arch"
                | "c"
                | "configuration"
                | "f"
                | "filter"
                | "framework"
                | "l"
                | "logger"
                | "os"
                | "r"
                | "report"
                | "results-directory"
                | "runtime"
        ))
    .then(ValueSpec::value)
}

fn tokenize_dotnet_args(args: &[String]) -> Vec<Token<'_>> {
    arg_tokenizer::tokenize_grammar(args, &dotnet_takes_value, Dialect::Msbuild)
}

/// The tokens dotnet itself parses: everything the user put after `--` is forwarded to the
/// test runner, so a lookup for dotnet's own flags must not see it. The Msbuild dialect keeps
/// classifying past the boundary, which is what makes this slice necessary.
fn dotnet_own_tokens<'t, 'a>(tokens: &'t [Token<'a>]) -> &'t [Token<'a>] {
    arg_tokenizer::before_dashdash(tokens)
}

/// Strict lookup (prefer this): only a literal `--flag` matches. A single-dash or slash spelling
/// of a modern option doesn't get rejected, it gets silently misparsed as an unrelated MSBuild
/// switch (`MSB100x` errors) -- except the legacy passthrough switches in `dotnet_has_loose_flag`.
fn dotnet_double_dash_flag_value<'a>(tokens: &[Token<'a>], name: &str) -> Option<&'a str> {
    arg_tokenizer::double_dash_flag_value(dotnet_own_tokens(tokens), Dialect::Msbuild, name)
}

fn dotnet_has_flag(tokens: &[Token<'_>], name: &str) -> bool {
    arg_tokenizer::has_double_dash_flag(dotnet_own_tokens(tokens), Dialect::Msbuild, name)
}

/// Loose lookup: `-flag`/`--flag`/`/flag` all match. Only correct for genuine legacy MSBuild.exe
/// passthrough switches (`nologo`, `bl`, `v`/`verbosity`) -- see [`dotnet_double_dash_flag_value`].
fn dotnet_has_loose_flag(tokens: &[Token<'_>], name: &str) -> bool {
    arg_tokenizer::has_flag(dotnet_own_tokens(tokens), Dialect::Msbuild, name)
}

/// Loose match, but only when the flag is bare (no attached value) -- a boolean switch's broken
/// attached-value spelling (e.g. `-nologo:true`) would otherwise be misread as "already present".
fn dotnet_has_bare_loose_flag(tokens: &[Token<'_>], name: &str) -> bool {
    dotnet_own_tokens(tokens).iter().any(|t| {
        t.kind == TokenKind::Long && t.attached.is_none() && t.text.eq_ignore_ascii_case(name)
    })
}


fn dotnet_has_bare_double_dash_flag(tokens: &[Token<'_>], name: &str) -> bool {
    tokens.iter().any(|t| {
        t.kind == TokenKind::Long
            && t.double_dash
            && t.attached.is_none()
            && t.text.eq_ignore_ascii_case(name)
    })
}

fn has_nologo_arg(tokens: &[Token<'_>]) -> bool {
    // -nologo is a pure boolean switch; an attached-value spelling must not count as present.
    dotnet_has_bare_loose_flag(tokens, "nologo")
}

fn has_trx_logger_arg(tokens: &[Token<'_>]) -> bool {
    // --logger can legitimately repeat (e.g. `--logger "console;verbosity=normal" --logger
    // trx`), so every occurrence must be checked, not just the first. `-l` is dotnet's own
    // alias for it, single-dash only: MSBuild's `/l:` is an unrelated logger-assembly switch,
    // and `--l` is not a dotnet spelling at all (System.CommandLine does no abbreviation).
    //
    // Scoped to dotnet's own region, unlike the results-directory lookups below: `dotnet test
    // --help` says the arguments after `--` go "to the application that is being run", so a
    // `--logger` there is the test app's, not VSTest's, and RTK still owes it a trx logger.
    let own = dotnet_own_tokens(tokens);
    arg_tokenizer::double_dash_flag_values(own, Dialect::Msbuild, "logger")
        .chain(
            own.iter()
                .filter(|t| {
                    t.kind == TokenKind::Long
                        && !t.slash
                        && !t.double_dash
                        && t.text.eq_ignore_ascii_case("l")
                })
                // `own`, not `tokens`: Token::value resolves `linked` as an index into the
                // slice it is handed, so mixing the two reads a different token's text.
                .filter_map(|t| t.value(own)),
        )
        .any(|value| {
            let lower = value.to_ascii_lowercase();
            lower == "trx" || lower.starts_with("trx;")
        })
}

/// Where a `--results-directory` counts, which is not the same region in both runner modes.
///
/// In MTP bridge mode the runner reads its own copy from past the `--`, and that is where the
/// TRX lands. In Classic/VSTest the flag is dotnet's own and must precede the `--`; one past it
/// belongs to the test app, and dotnet writes to ./TestResults regardless -- verified against
/// the real SDK 9. Both lookups below share this so they cannot disagree: reading the app's
/// path as dotnet's would splice it into dotnet's own arguments.
fn results_directory_scope<'t, 'a>(
    tokens: &'t [Token<'a>],
    runner_mode: TestRunnerMode,
) -> &'t [Token<'a>] {
    match runner_mode {
        TestRunnerMode::MtpVsTestBridge => tokens,
        TestRunnerMode::Classic | TestRunnerMode::MtpNative => dotnet_own_tokens(tokens),
    }
}

fn has_results_directory_arg(tokens: &[Token<'_>], runner_mode: TestRunnerMode) -> bool {
    arg_tokenizer::has_double_dash_flag(
        results_directory_scope(tokens, runner_mode),
        Dialect::Msbuild,
        "results-directory",
    )
}

fn has_report_arg(tokens: &[Token<'_>]) -> bool {
    dotnet_has_flag(tokens, "report")
}

fn has_report_trx_arg(tokens: &[Token<'_>]) -> bool {
    // Deliberately unscoped, unlike dotnet's own flags: --report-trx is a direct dotnet flag
    // in MTP-native mode and the runner's flag past `--` in the VSTest bridge, so either
    // region is a legitimate place for the user to have written it. Bare-only, so an attached
    // spelling like "--report-trx:true" doesn't count as present.
    dotnet_has_bare_double_dash_flag(tokens, "report-trx")
}

/// Injects `--report-trx` after the `--` separator in `args`.
/// If no `--` separator exists, appends `-- --report-trx` at the end.
fn inject_report_trx_into_args(args: &[String], tokens: &[Token<'_>]) -> Vec<String> {
    let sep = arg_tokenizer::dashdash_index(tokens).map(|i| tokens[i].source_index);
    if let Some(sep) = sep {
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

fn extract_report_arg(tokens: &[Token<'_>]) -> Option<PathBuf> {
    dotnet_double_dash_flag_value(tokens, "report").map(PathBuf::from)
}

fn has_verify_no_changes_arg(tokens: &[Token<'_>]) -> bool {
    dotnet_has_flag(tokens, "verify-no-changes")
}

/// The `--write` tokens RTK owns: its own pseudo-flag, stripped before forwarding to real
/// dotnet (which has no `--write`). Detection and stripping both go through here so they
/// cannot drift -- an attached `--write=true` is not RTK's flag and must pass through, while
/// one past `--` is still RTK's, since no dotnet option of that name exists for the boundary
/// to be forwarding it to.
fn write_override_tokens<'t, 'a>(
    tokens: &'t [Token<'a>],
) -> impl Iterator<Item = &'t Token<'a>> + 't {
    // Unscoped: `--write` is RTK's own pseudo-flag, so it has no dotnet counterpart that the
    // `--` boundary could be forwarding it to -- past the boundary it would be neither
    // honored nor stripped, and real dotnet would choke on it.
    tokens.iter().filter(|t| {
        t.kind == TokenKind::Long
            && t.double_dash
            && t.attached.is_none()
            && t.text.eq_ignore_ascii_case("write")
    })
}

fn has_write_mode_override(tokens: &[Token<'_>]) -> bool {
    write_override_tokens(tokens).next().is_some()
}

fn extract_results_directory_arg(
    tokens: &[Token<'_>],
    runner_mode: TestRunnerMode,
) -> Option<PathBuf> {
    arg_tokenizer::double_dash_flag_value(
        results_directory_scope(tokens, runner_mode),
        Dialect::Msbuild,
        "results-directory",
    )
    .map(PathBuf::from)
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

/// Format the build summary for stdout.
///
/// `_binlog_path` is intentionally unused — the binlog is a temporary file
/// that has already been cleaned up by the time this runs.
fn format_build_output(summary: &binlog::BuildSummary, _binlog_path: &Path) -> String {
    let status_icon = if summary.succeeded { "ok" } else { "fail" };
    let duration = summary.duration_text.as_deref().unwrap_or("unknown");

    const MAX_BUILD_ERRORS: usize = CAP_ERRORS;
    const MAX_BUILD_WARNINGS: usize = CAP_WARNINGS;

    let mut errors = String::new();
    if !summary.errors.is_empty() {
        errors.push_str("Errors:\n");
        for issue in summary.errors.iter().take(MAX_BUILD_ERRORS) {
            errors.push_str(&format!("{}\n", format_issue(issue, "error")));
        }
        if summary.errors.len() > MAX_BUILD_ERRORS {
            errors.push_str(&format!(
                "  … +{} more errors\n",
                summary.errors.len() - MAX_BUILD_ERRORS
            ));
            let all_errors = summary
                .errors
                .iter()
                .map(|e| format_issue(e, "error"))
                .collect::<Vec<_>>()
                .join("\n");
            if let Some(hint) = crate::core::tee::force_tee_tail_hint(
                &all_errors,
                "dotnet-build-errors",
                MAX_BUILD_ERRORS + 1,
            ) {
                errors.push_str(&format!("  {}\n", hint));
            }
        }
    }

    let mut warnings = String::new();
    if !summary.warnings.is_empty() {
        warnings.push_str("Warnings:\n");
        for issue in summary.warnings.iter().take(MAX_BUILD_WARNINGS) {
            warnings.push_str(&format!("{}\n", format_issue(issue, "warning")));
        }
        if summary.warnings.len() > MAX_BUILD_WARNINGS {
            warnings.push_str(&format!(
                "  … +{} more warnings\n",
                summary.warnings.len() - MAX_BUILD_WARNINGS
            ));
            let all_warnings = summary
                .warnings
                .iter()
                .map(|w| format_issue(w, "warning"))
                .collect::<Vec<_>>()
                .join("\n");
            if let Some(hint) = crate::core::tee::force_tee_tail_hint(
                &all_warnings,
                "dotnet-build-warnings",
                MAX_BUILD_WARNINGS + 1,
            ) {
                warnings.push_str(&format!("  {}\n", hint));
            }
        }
    }

    let verdict = format!(
        "{} dotnet build: {} projects, {} errors, {} warnings ({})",
        status_icon,
        summary.project_count,
        summary.errors.len(),
        summary.warnings.len(),
        duration
    );

    // Status line is emitted last so consumers that read the tail of the stream
    // (`| tail -N`, agent watch/monitor modes, bounded context windows) get a
    // definitive verdict. Mirrors native `dotnet build`, which ends with
    // `Build succeeded.` / `Build FAILED.`. See issue #1574.
    // Warnings before errors: errors survive `| tail -N` immediately above the verdict.
    [warnings, errors, verdict]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// True only when the filtered `Failed Tests:` section can't stand on its own: no failures were
/// parsed, fewer were parsed than `summary.failed`, or some parsed failure has no detail. Prepend
/// the raw output otherwise duplicates every failure block already in the filtered section.
fn test_needs_raw_fallback(summary: &binlog::TestSummary) -> bool {
    summary.failed_tests.is_empty()
        || summary.failed_tests.len() < summary.failed
        || summary.failed_tests.iter().any(|t| t.details.is_empty())
}

/// On success, or when `needs_raw_fallback` is false, only the filtered summary is emitted;
/// otherwise raw stdout (or stderr if stdout is empty) is prepended.
fn compose_failure_output(
    command_success: bool,
    needs_raw_fallback: bool,
    stdout: &str,
    stderr: &str,
    filtered: &str,
) -> String {
    if command_success || !needs_raw_fallback {
        return filtered.to_string();
    }

    let stdout_trimmed = stdout.trim();
    let stderr_trimmed = stderr.trim();
    if !stdout_trimmed.is_empty() {
        format!("{}\n\n{}", stdout_trimmed, filtered)
    } else if !stderr_trimmed.is_empty() {
        format!("{}\n\n{}", stderr_trimmed, filtered)
    } else {
        filtered.to_string()
    }
}

/// Format the test summary for stdout.
///
/// `_binlog_path` is intentionally unused — the binlog is a temporary file
/// that has already been cleaned up by the time this runs.
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

    let header = if counts_unavailable {
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

    const MAX_DOTNET_FAILURES: usize = CAP_WARNINGS;
    let mut failed_tests_section = String::new();
    if has_failures && !summary.failed_tests.is_empty() {
        failed_tests_section.push_str("Failed Tests:\n");
        for failed in summary.failed_tests.iter().take(MAX_DOTNET_FAILURES) {
            failed_tests_section.push_str(&format!("  {}\n", failed.name));
            for detail in &failed.details {
                failed_tests_section.push_str(&format!("    {}\n", truncate(detail, 320)));
            }
            failed_tests_section.push('\n');
        }
        if summary.failed_tests.len() > MAX_DOTNET_FAILURES {
            failed_tests_section.push_str(&format!(
                "… +{} more failed tests\n",
                summary.failed_tests.len() - MAX_DOTNET_FAILURES
            ));
            let all_failed = summary
                .failed_tests
                .iter()
                .skip(MAX_DOTNET_FAILURES)
                .map(|t| {
                    let mut s = t.name.clone();
                    for detail in &t.details {
                        s.push_str(&format!("\n  {}", truncate(detail, 320)));
                    }
                    s
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            if let Some(hint) =
                crate::core::tee::force_tee_hint(&all_failed, "dotnet-test-failures")
            {
                failed_tests_section.push_str(&format!("  {}\n", hint));
            }
        }
    }

    const MAX_TEST_ERRORS: usize = CAP_WARNINGS;
    const MAX_TEST_WARNINGS: usize = CAP_WARNINGS;

    let mut errors_section = String::new();
    if !errors.is_empty() {
        errors_section.push_str("Errors:\n");
        for issue in errors.iter().take(MAX_TEST_ERRORS) {
            errors_section.push_str(&format!("{}\n", format_issue(issue, "error")));
        }
        if errors.len() > MAX_TEST_ERRORS {
            errors_section.push_str(&format!(
                "  … +{} more errors\n",
                errors.len() - MAX_TEST_ERRORS
            ));
            let all_errors = errors
                .iter()
                .map(|e| format_issue(e, "error"))
                .collect::<Vec<_>>()
                .join("\n");
            if let Some(hint) = crate::core::tee::force_tee_tail_hint(
                &all_errors,
                "dotnet-test-errors",
                MAX_TEST_ERRORS + 1,
            ) {
                errors_section.push_str(&format!("  {}\n", hint));
            }
        }
    }

    let mut warnings_section = String::new();
    if !warnings.is_empty() {
        warnings_section.push_str("Warnings:\n");
        for issue in warnings.iter().take(MAX_TEST_WARNINGS) {
            warnings_section.push_str(&format!("{}\n", format_issue(issue, "warning")));
        }
        if warnings.len() > MAX_TEST_WARNINGS {
            warnings_section.push_str(&format!(
                "  … +{} more warnings\n",
                warnings.len() - MAX_TEST_WARNINGS
            ));
            let all_warnings = warnings
                .iter()
                .map(|w| format_issue(w, "warning"))
                .collect::<Vec<_>>()
                .join("\n");
            if let Some(hint) = crate::core::tee::force_tee_tail_hint(
                &all_warnings,
                "dotnet-test-warnings",
                MAX_TEST_WARNINGS + 1,
            ) {
                warnings_section.push_str(&format!("  {}\n", hint));
            }
        }
    }

    // Status line emitted last; see format_build_output (issue #1574).
    // Warnings before errors: errors survive `| tail -N` immediately above the verdict.
    [
        failed_tests_section,
        warnings_section,
        errors_section,
        header,
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join("\n")
}

/// Format the restore summary for stdout.
///
/// `_binlog_path` is intentionally unused — the binlog is a temporary file
/// that has already been cleaned up by the time this runs.
fn format_restore_output(
    summary: &binlog::RestoreSummary,
    errors: &[binlog::BinlogIssue],
    warnings: &[binlog::BinlogIssue],
    _binlog_path: &Path,
) -> String {
    let has_errors = summary.errors > 0;
    let status_icon = if has_errors { "fail" } else { "ok" };
    let duration = summary.duration_text.as_deref().unwrap_or("unknown");

    const MAX_FORMAT_ERRORS: usize = CAP_ERRORS;
    const MAX_FORMAT_WARNINGS: usize = CAP_WARNINGS;

    let mut errors_section = String::new();
    if !errors.is_empty() {
        errors_section.push_str("Errors:\n");
        for issue in errors.iter().take(MAX_FORMAT_ERRORS) {
            errors_section.push_str(&format!("{}\n", format_issue(issue, "error")));
        }
        if errors.len() > MAX_FORMAT_ERRORS {
            errors_section.push_str(&format!(
                "  … +{} more errors\n",
                errors.len() - MAX_FORMAT_ERRORS
            ));
            let all_errors = errors
                .iter()
                .map(|e| format_issue(e, "error"))
                .collect::<Vec<_>>()
                .join("\n");
            if let Some(hint) = crate::core::tee::force_tee_tail_hint(
                &all_errors,
                "dotnet-format-errors",
                MAX_FORMAT_ERRORS + 1,
            ) {
                errors_section.push_str(&format!("  {}\n", hint));
            }
        }
    }

    let mut warnings_section = String::new();
    if !warnings.is_empty() {
        warnings_section.push_str("Warnings:\n");
        for issue in warnings.iter().take(MAX_FORMAT_WARNINGS) {
            warnings_section.push_str(&format!("{}\n", format_issue(issue, "warning")));
        }
        if warnings.len() > MAX_FORMAT_WARNINGS {
            warnings_section.push_str(&format!(
                "  … +{} more warnings\n",
                warnings.len() - MAX_FORMAT_WARNINGS
            ));
            let all_warnings = warnings
                .iter()
                .map(|w| format_issue(w, "warning"))
                .collect::<Vec<_>>()
                .join("\n");
            if let Some(hint) = crate::core::tee::force_tee_tail_hint(
                &all_warnings,
                "dotnet-format-warnings",
                MAX_FORMAT_WARNINGS + 1,
            ) {
                warnings_section.push_str(&format!("  {}\n", hint));
            }
        }
    }

    let verdict = format!(
        "{} dotnet restore: {} projects, {} errors, {} warnings ({})",
        status_icon, summary.restored_projects, summary.errors, summary.warnings, duration
    );

    // Status line emitted last; see format_build_output (issue #1574).
    // Warnings before errors: errors survive `| tail -N` immediately above the verdict.
    [warnings_section, errors_section, verdict]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
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

        let tokens = tokenize_dotnet_args(args);
        let runner_mode = if subcommand == "test" {
            detect_test_runner_mode(&tokens)
        } else {
            TestRunnerMode::Classic
        };
        build_effective_dotnet_args(
            subcommand,
            args,
            &tokens,
            binlog_path,
            trx_results_dir,
            runner_mode,
        )
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
        assert!(has_binlog_arg(&tokenize_dotnet_args(&args)));

        let args = vec!["/bl".to_string()];
        assert!(has_binlog_arg(&tokenize_dotnet_args(&args)));

        let args = vec!["--configuration".to_string(), "Release".to_string()];
        assert!(!has_binlog_arg(&tokenize_dotnet_args(&args)));

        // `/r` is MSBuild's boolean restore, not dotnet's `-r <rid>`: if it swallowed the next
        // arg, RTK would inject a second -bl on top of the user's own.
        let args = vec!["/r".to_string(), "-bl:my.binlog".to_string()];
        assert!(has_binlog_arg(&tokenize_dotnet_args(&args)));
    }

    #[test]
    fn test_double_dash_only_flags_reject_single_dash_and_slash_spellings() {
        let args = vec!["-results-directory".to_string(), "/tmp/out".to_string()];
        assert!(!has_results_directory_arg(&tokenize_dotnet_args(&args), TestRunnerMode::Classic));
        assert_eq!(extract_results_directory_arg(&tokenize_dotnet_args(&args), TestRunnerMode::Classic), None);

        let args = vec!["/results-directory".to_string(), "/tmp/out".to_string()];
        assert!(!has_results_directory_arg(&tokenize_dotnet_args(&args), TestRunnerMode::Classic));

        let args = vec!["-report".to_string(), "/tmp/r.json".to_string()];
        assert!(!has_report_arg(&tokenize_dotnet_args(&args)));
        assert_eq!(extract_report_arg(&tokenize_dotnet_args(&args)), None);

        let args = vec!["-report-trx".to_string()];
        assert!(!has_report_trx_arg(&tokenize_dotnet_args(&args)));

        let args = vec!["-verify-no-changes".to_string()];
        assert!(!has_verify_no_changes_arg(&tokenize_dotnet_args(&args)));

        let args = vec!["-write".to_string()];
        assert!(!has_write_mode_override(&tokenize_dotnet_args(&args)));

        // The canonical "--" forms still work.
        let args = vec!["--results-directory".to_string(), "/tmp/out".to_string()];
        assert!(has_results_directory_arg(&tokenize_dotnet_args(&args), TestRunnerMode::Classic));
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

    // Regression tests for issue #2501: on failing test runs the raw stdout was
    // prepended in front of the filtered `Failed Tests:` section, duplicating every
    // failure block (+65% vs raw). `test_needs_raw_fallback` must suppress the raw
    // prepend when the structured section already carries failure detail, while
    // keeping it when the filter couldn't capture the failures.

    #[test]
    fn test_needs_raw_fallback_false_when_failures_have_detail() {
        // Every reported failure was parsed and carries detail: the structured
        // section stands alone, so the raw prepend is dropped (issue #2501).
        let failed_tests: Vec<binlog::FailedTest> = (0..5)
            .map(|i| binlog::FailedTest {
                name: format!("MyTests.Case{i}"),
                details: vec!["Assert.True() Failure".to_string()],
            })
            .collect();
        let summary = binlog::TestSummary {
            passed: 717,
            failed: 5,
            skipped: 0,
            total: 722,
            project_count: 1,
            failed_tests,
            duration_text: Some("2 s".to_string()),
        };
        assert!(!test_needs_raw_fallback(&summary));
    }

    #[test]
    fn test_needs_raw_fallback_true_when_parsed_list_incomplete() {
        // summary.failed reports 5, but only 3 blocks were parsed (each with
        // detail). The 2 missing failures live only in raw stdout — keep the
        // fallback so they aren't silently dropped.
        let summary = binlog::TestSummary {
            passed: 717,
            failed: 5,
            skipped: 0,
            total: 722,
            project_count: 1,
            failed_tests: vec![
                binlog::FailedTest {
                    name: "MyTests.One".to_string(),
                    details: vec!["Assert.True() Failure".to_string()],
                },
                binlog::FailedTest {
                    name: "MyTests.Two".to_string(),
                    details: vec!["Assert.Equal() Failure".to_string()],
                },
                binlog::FailedTest {
                    name: "MyTests.Three".to_string(),
                    details: vec!["Assert.Null() Failure".to_string()],
                },
            ],
            duration_text: Some("2 s".to_string()),
        };
        assert!(test_needs_raw_fallback(&summary));
    }

    #[test]
    fn test_needs_raw_fallback_true_when_no_failures_parsed() {
        // Build failure / crash: command failed but nothing landed in failed_tests.
        let summary = binlog::TestSummary {
            failed: 1,
            total: 1,
            ..Default::default()
        };
        assert!(test_needs_raw_fallback(&summary));
    }

    #[test]
    fn test_needs_raw_fallback_true_when_a_failure_lacks_detail() {
        // Self-closing <UnitTestResult> with no <ErrorInfo>: name only, no detail.
        let summary = binlog::TestSummary {
            failed: 1,
            total: 1,
            failed_tests: vec![binlog::FailedTest {
                name: "MyTests.NoDetail".to_string(),
                details: Vec::new(),
            }],
            ..Default::default()
        };
        assert!(test_needs_raw_fallback(&summary));
    }

    #[test]
    fn test_compose_failure_output_drops_raw_when_no_fallback_needed() {
        // The raw stdout contains the inline failure; the filtered section also
        // contains it. With needs_raw_fallback=false, the failure must appear once.
        let raw_stdout = "  failed MyTests.HasRestriction\n    Assert.True() Failure";
        let filtered =
            "Failed Tests:\n  MyTests.HasRestriction\n    Assert.True() Failure\n\nfail dotnet test: 717 passed, 5 failed";
        let output = compose_failure_output(false, false, raw_stdout, "", filtered);

        assert_eq!(output, filtered);
        assert_eq!(output.matches("HasRestriction").count(), 1);
    }

    #[test]
    fn test_compose_failure_output_prepends_raw_when_fallback_needed() {
        let raw_stdout = "Build FAILED.\n  Program.cs(1,1): error CS1002";
        let filtered = "fail dotnet test: 0 passed, 1 failed";
        // command_success=false, needs_raw_fallback=true → raw is prepended.
        let output = compose_failure_output(false, true, raw_stdout, "", filtered);

        assert!(output.starts_with("Build FAILED."));
        assert!(output.ends_with(filtered));
    }

    #[test]
    fn test_compose_failure_output_uses_stderr_when_stdout_empty() {
        let filtered = "fail dotnet test: 0 passed, 1 failed";
        let output = compose_failure_output(false, true, "   ", "boom on stderr", filtered);

        assert!(output.starts_with("boom on stderr"));
        assert!(output.ends_with(filtered));
    }

    #[test]
    fn test_compose_failure_output_returns_filtered_on_success() {
        let filtered = "ok dotnet test: 722 tests passed";
        let output = compose_failure_output(true, true, "ignored raw", "ignored", filtered);
        assert_eq!(output, filtered);
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

    // Regression tests for issue #1574: status line must be the final line so that
    // consumers reading the tail of the stream (`| tail -N`, agent watch/monitor
    // modes, bounded context windows) get a definitive `ok` / `fail` verdict.
    // Mirrors native `dotnet`, which ends with `Build succeeded.` / `Build FAILED.`.

    #[test]
    fn test_format_build_output_status_line_is_last_for_tail_consumers() {
        let summary = binlog::BuildSummary {
            succeeded: true,
            project_count: 1,
            errors: Vec::new(),
            warnings: vec![binlog::BinlogIssue {
                code: "CS0219".to_string(),
                file: "src/Program.cs".to_string(),
                line: 25,
                column: 10,
                message: "Variable assigned but never used".to_string(),
            }],
            duration_text: Some("00:00:01.23".to_string()),
        };
        let output = format_build_output(&summary, Path::new("/tmp/build.binlog"));
        let last_line = output.lines().last().expect("output must not be empty");
        assert!(
            last_line.starts_with("ok dotnet build:"),
            "status line must be the last line for `| tail -N` consumers, got: {:?}",
            last_line
        );

        let last_5: Vec<&str> = output.lines().rev().take(5).collect();
        assert!(
            last_5.iter().any(|l| l.starts_with("ok dotnet build:")),
            "`tail -5` must include the status line, got tail: {:?}",
            last_5
        );
    }

    #[test]
    fn test_format_test_output_status_line_is_last_for_tail_consumers() {
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
        let last_line = output.lines().last().expect("output must not be empty");
        assert!(
            last_line.starts_with("ok dotnet test:"),
            "status line must be the last line, got: {:?}",
            last_line
        );
    }

    #[test]
    fn test_format_restore_output_status_line_is_last_for_tail_consumers() {
        let summary = binlog::RestoreSummary {
            restored_projects: 1,
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
        let last_line = output.lines().last().expect("output must not be empty");
        assert!(
            last_line.starts_with("fail dotnet restore:"),
            "status line must be the last line, got: {:?}",
            last_line
        );
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
    fn test_runner_mode_probes_follow_scan_dir_not_the_process_cwd() {
        // All three probes have to start from scan_dir: while only the project scan did, a
        // developer working beneath a global.json decided the result and every tempdir
        // assertion in these tests was decorative.
        let mtp_root = tempfile::tempdir().expect("create temp dir");
        fs::write(
            mtp_root.path().join("global.json"),
            r#"{"test":{"runner":"Microsoft.Testing.Platform"}}"#,
        )
        .expect("write global.json");
        let nested = mtp_root.path().join("nested");
        fs::create_dir(&nested).expect("create nested");

        let tokens = tokenize_dotnet_args(&[]);
        assert_eq!(
            detect_test_runner_mode_in_dir(&tokens, &nested),
            TestRunnerMode::MtpNative,
            "a global.json above the scanned directory still counts"
        );

        // A directory with no global.json above it is unaffected by wherever the process is.
        let plain = tempfile::tempdir().expect("create temp dir");
        fs::write(
            plain.path().join("Classic.Tests.csproj"),
            r#"<Project Sdk="Microsoft.NET.Sdk"></Project>"#,
        )
        .expect("write csproj");
        assert_eq!(
            detect_test_runner_mode_in_dir(&tokens, plain.path()),
            TestRunnerMode::Classic
        );
    }

    #[test]
    fn test_single_segment_absolute_project_is_a_path_not_a_switch() {
        // `/Other.csproj` tokenizes as a slash flag (structure alone can't tell it from an
        // MSBuild switch), and being missed from the explicit projects made RTK scan the cwd
        // and adopt an unrelated project's runner mode.
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        fs::write(
            temp_dir.path().join("Root.csproj"),
            r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <UseMicrosoftTestingPlatformRunner>true</UseMicrosoftTestingPlatformRunner>
  </PropertyGroup>
</Project>"#,
        )
        .expect("write csproj");

        let args = vec!["/Other.csproj".to_string()];
        let tokens = tokenize_dotnet_args(&args);
        assert_eq!(
            detect_test_runner_mode_in_dir(&tokens, temp_dir.path()),
            TestRunnerMode::Classic,
            "an explicit project must not fall back to scanning the cwd"
        );

        // With no project named at all, scanning the directory is still right.
        let tokens = tokenize_dotnet_args(&[]);
        assert_eq!(
            detect_test_runner_mode_in_dir(&tokens, temp_dir.path()),
            TestRunnerMode::MtpVsTestBridge
        );
    }

    #[test]
    fn test_forwarded_args_do_not_suppress_injection() {
        // Everything past `--` goes to the test runner, not to dotnet, so a `--logger` there
        // is not the user asking dotnet for a logger -- RTK still has to inject its own.
        let args = vec![
            "--".to_string(),
            "--logger".to_string(),
            "trx".to_string(),
        ];
        let injected = build_dotnet_args_for_test("test", &args, true);
        let boundary = injected.iter().position(|a| a == "--").expect("boundary");
        let logger = injected
            .windows(2)
            .position(|w| w == ["--logger", "trx"])
            .expect("RTK's own logger");
        assert!(logger < boundary, "{injected:?}");

        let args = vec!["--".to_string(), "--nologo".to_string()];
        let injected = build_dotnet_args_for_test("test", &args, true);
        let boundary = injected.iter().position(|a| a == "--").expect("boundary");
        let nologo = injected.iter().position(|a| a == "-nologo").expect("nologo");
        assert!(nologo < boundary, "{injected:?}");
    }

    #[test]
    fn test_short_logger_alias_does_not_duplicate() {
        // `-l trx` is dotnet's documented short form of `--logger trx`; missing it injected a
        // second trx logger on top of the user's own.
        for args in [vec!["-l", "trx"], vec!["-l:trx"]] {
            let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            let injected = build_dotnet_args_for_test("test", &args, true);
            assert!(
                !injected.windows(2).any(|w| w == ["--logger", "trx"]),
                "must not inject a duplicate trx logger: {injected:?}"
            );
        }

        // MSBuild's `/l:` attaches a logger *assembly*, not VSTest's trx logger, so RTK's own
        // injection still has to happen.
        let args = vec!["/l:trx".to_string()];
        let injected = build_dotnet_args_for_test("test", &args, true);
        assert!(
            injected.windows(2).any(|w| w == ["--logger", "trx"]),
            "/l: is MSBuild's logger switch, not --logger: {injected:?}"
        );
    }

    #[test]
    fn test_second_of_multiple_loggers_being_trx_does_not_duplicate() {
        // Regression: --logger can legitimately repeat (a documented VSTest pattern); a trx
        // logger that isn't the FIRST --logger occurrence must still be detected.
        let args = vec![
            "--logger".to_string(),
            "console;verbosity=normal".to_string(),
            "--logger".to_string(),
            "trx".to_string(),
        ];

        let injected = build_dotnet_args_for_test("test", &args, true);
        let trx_logger_count = injected.iter().filter(|a| *a == "trx").count();
        assert_eq!(
            trx_logger_count, 1,
            "must not inject a duplicate trx logger: {injected:?}"
        );
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
        let tokens = tokenize_dotnet_args(&args);
        assert_eq!(
            detect_test_runner_mode(&tokens),
            TestRunnerMode::MtpVsTestBridge
        );

        let binlog_path = Path::new("/tmp/test.binlog");
        let injected = build_effective_dotnet_args(
            "test",
            &args,
            &tokens,
            binlog_path,
            None,
            detect_test_runner_mode(&tokens),
        );

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
        let tokens = tokenize_dotnet_args(&args);
        assert_eq!(
            detect_test_runner_mode(&tokens),
            TestRunnerMode::MtpVsTestBridge
        );

        let binlog_path = Path::new("/tmp/test.binlog");
        let injected = build_effective_dotnet_args(
            "test",
            &args,
            &tokens,
            binlog_path,
            None,
            detect_test_runner_mode(&tokens),
        );

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
        let tokens = tokenize_dotnet_args(&args);
        assert_eq!(
            detect_test_runner_mode(&tokens),
            TestRunnerMode::MtpVsTestBridge
        );

        let binlog_path = Path::new("/tmp/test.binlog");
        let injected = build_effective_dotnet_args(
            "test",
            &args,
            &tokens,
            binlog_path,
            None,
            detect_test_runner_mode(&tokens),
        );

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
        let tokens = tokenize_dotnet_args(&args);
        let injected = build_effective_dotnet_args(
            "test",
            &args,
            &tokens,
            binlog_path,
            None,
            detect_test_runner_mode(&tokens),
        );

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
        let tokens = tokenize_dotnet_args(&args);
        let injected = build_effective_dotnet_args(
            "test",
            &args,
            &tokens,
            binlog_path,
            None,
            detect_test_runner_mode(&tokens),
        );

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
        let tokens = tokenize_dotnet_args(&args);
        assert_eq!(detect_test_runner_mode(&tokens), TestRunnerMode::Classic);

        let binlog_path = Path::new("/tmp/test.binlog");
        let trx_dir = Path::new("/tmp/test_results");
        let injected =
            build_effective_dotnet_args(
                "test",
                &args,
                &tokens,
                binlog_path,
                Some(trx_dir),
                TestRunnerMode::Classic,
            );
        assert!(injected.contains(&"--logger".to_string()));
        assert!(injected.contains(&"trx".to_string()));
    }

    #[test]
    fn test_detect_mode_ignores_flag_value_ending_in_project_extension() {
        // A value-taking flag's own value (e.g. --results-directory's path) must not be misread
        // as an explicit project reference just because it ends in .csproj.
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let csproj = temp_dir.path().join("Real.Tests.csproj");
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
            "--results-directory".to_string(),
            "/tmp/MyResults.csproj".to_string(),
        ];
        let tokens = tokenize_dotnet_args(&args);
        assert_eq!(
            detect_test_runner_mode_in_dir(&tokens, temp_dir.path()),
            TestRunnerMode::MtpVsTestBridge,
            "the flag value's fake .csproj suffix must not stop scan_dir from being scanned"
        );
    }

    #[test]
    fn test_detect_mode_ignores_filter_and_configuration_flag_values() {
        // --filter and -c/--configuration's own values must not be misread as an explicit
        // project reference either.
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let csproj = temp_dir.path().join("Real.Tests.csproj");
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
            "--filter".to_string(),
            "FullyQualifiedName~Foo.csproj".to_string(),
        ];
        let tokens = tokenize_dotnet_args(&args);
        assert_eq!(
            detect_test_runner_mode_in_dir(&tokens, temp_dir.path()),
            TestRunnerMode::MtpVsTestBridge,
            "--filter's value must not stop scan_dir from being scanned"
        );

        let args = vec!["-c".to_string(), "Debug.csproj".to_string()];
        let tokens = tokenize_dotnet_args(&args);
        assert_eq!(
            detect_test_runner_mode_in_dir(&tokens, temp_dir.path()),
            TestRunnerMode::MtpVsTestBridge,
            "-c's value must not stop scan_dir from being scanned"
        );
    }

    #[test]
    fn test_detect_mode_ignores_positional_after_double_dash() {
        // A VSTest/MTP filter expression after -- (a forwarding boundary, not end-of-options)
        // must not be treated as an explicit project reference just because it looks like one.
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let csproj = temp_dir.path().join("Real.Tests.csproj");
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
            "--".to_string(),
            "FullyQualifiedName~Foo.csproj".to_string(),
        ];
        let tokens = tokenize_dotnet_args(&args);
        assert_eq!(
            detect_test_runner_mode_in_dir(&tokens, temp_dir.path()),
            TestRunnerMode::MtpVsTestBridge,
            "a post-- filter expression must not stop scan_dir from being scanned"
        );
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
        assert!(has_results_directory_arg(&tokenize_dotnet_args(&args), TestRunnerMode::Classic));

        let args = vec!["--results-directory=/tmp/trx".to_string()];
        assert!(has_results_directory_arg(&tokenize_dotnet_args(&args), TestRunnerMode::Classic));

        let args = vec!["--logger".to_string(), "trx".to_string()];
        assert!(!has_results_directory_arg(&tokenize_dotnet_args(&args), TestRunnerMode::Classic));
    }

    #[test]
    fn test_extract_results_directory_arg_detects_variants() {
        let args = vec!["--results-directory".to_string(), "/tmp/r1".to_string()];
        assert_eq!(
            extract_results_directory_arg(&tokenize_dotnet_args(&args), TestRunnerMode::Classic),
            Some(PathBuf::from("/tmp/r1"))
        );

        let args = vec!["--results-directory=/tmp/r2".to_string()];
        assert_eq!(
            extract_results_directory_arg(&tokenize_dotnet_args(&args), TestRunnerMode::Classic),
            Some(PathBuf::from("/tmp/r2"))
        );
    }

    #[test]
    fn test_resolve_trx_results_dir_user_directory_is_not_marked_for_cleanup() {
        let args = vec![
            "--results-directory".to_string(),
            "/custom/results".to_string(),
        ];

        let (dir, cleanup) = resolve_trx_results_dir("test", &tokenize_dotnet_args(&args), TestRunnerMode::Classic);
        assert_eq!(dir, Some(PathBuf::from("/custom/results")));
        assert!(!cleanup);
    }

    #[test]
    fn test_results_directory_past_the_boundary_is_the_apps_in_classic_mode() {
        // Verified against the real SDK 9: with `dotnet test -- --results-directory X`, a
        // Classic/VSTest run writes its TRX to ./TestResults and the flag reaches the test app
        // instead. Reading it as dotnet's own suppressed RTK's injection and left that TRX
        // behind in the user's project, unclaimed and never cleaned up.
        let args = vec![
            "--".to_string(),
            "--results-directory".to_string(),
            "/tmp/rd2".to_string(),
        ];
        let tokens = tokenize_dotnet_args(&args);

        assert!(!has_results_directory_arg(&tokens, TestRunnerMode::Classic));
        assert_eq!(
            extract_results_directory_arg(&tokens, TestRunnerMode::Classic),
            None
        );
        let (dir, cleanup) = resolve_trx_results_dir("test", &tokens, TestRunnerMode::Classic);
        assert!(cleanup, "RTK's own temp dir, so RTK cleans it up");
        assert_ne!(dir, Some(PathBuf::from("/tmp/rd2")));

        // The bridge runner really does read its own copy from past the `--`.
        assert!(has_results_directory_arg(
            &tokens,
            TestRunnerMode::MtpVsTestBridge
        ));
        assert_eq!(
            extract_results_directory_arg(&tokens, TestRunnerMode::MtpVsTestBridge),
            Some(PathBuf::from("/tmp/rd2"))
        );

        // Before the boundary it is dotnet's own flag in every mode.
        let own = vec!["--results-directory".to_string(), "/tmp/rd1".to_string()];
        let own = tokenize_dotnet_args(&own);
        for mode in [TestRunnerMode::Classic, TestRunnerMode::MtpVsTestBridge] {
            assert!(has_results_directory_arg(&own, mode), "{mode:?}");
        }
    }

    #[test]
    fn test_resolve_trx_results_dir_generated_directory_is_marked_for_cleanup() {
        let args = Vec::<String>::new();

        let (dir, cleanup) = resolve_trx_results_dir("test", &tokenize_dotnet_args(&args), TestRunnerMode::Classic);
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
        let (report_path, cleanup) = resolve_format_report_path(&tokenize_dotnet_args(&args));
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

        let (report_path, cleanup) = resolve_format_report_path(&tokenize_dotnet_args(&args));
        assert_eq!(
            report_path,
            Some(PathBuf::from("/tmp/user-format-report.json"))
        );
        assert!(!cleanup);
    }

    #[test]
    fn test_format_preserves_positional_project_argument_order() {
        let args = vec!["src/App/App.csproj".to_string()];

        let tokens = tokenize_dotnet_args(&args);
        let effective =
            build_effective_dotnet_format_args(&args, &tokens, Some(Path::new("/tmp/report.json")));
        assert_eq!(
            effective.first().map(String::as_str),
            Some("src/App/App.csproj")
        );
    }

    #[test]
    fn test_format_injects_before_the_users_double_dash() {
        // dotnet parks everything past `--` in UnparsedTokens, so an injected
        // `--verify-no-changes` after the boundary never applies -- format would rewrite the
        // tree while RTK reported check mode.
        let args = vec!["--".to_string(), "./src".to_string()];

        let tokens = tokenize_dotnet_args(&args);
        let effective =
            build_effective_dotnet_format_args(&args, &tokens, Some(Path::new("/tmp/report.json")));

        let boundary = effective.iter().position(|a| a == "--").expect("boundary");
        let verify = effective
            .iter()
            .position(|a| a == "--verify-no-changes")
            .expect("check mode");
        let report = effective.iter().position(|a| a == "--report").expect("report");
        assert!(verify < boundary, "{effective:?}");
        assert!(report < boundary, "{effective:?}");
    }

    #[test]
    fn test_write_override_detection_and_stripping_agree_on_attached_values() {
        // Detection must agree with stripping: an attached-value spelling ("--write=true") isn't
        // recognized as the --write pseudo-flag at all, so it passes through untouched instead
        // of being silently swallowed while RTK's own injection still fires.
        let args = vec!["--write=true".to_string()];
        let tokens = tokenize_dotnet_args(&args);
        assert!(!has_write_mode_override(&tokens));

        let effective = build_effective_dotnet_format_args(&args, &tokens, None);
        assert!(
            effective.contains(&"--write=true".to_string()),
            "an unrecognized flag must pass through untouched, not be silently dropped: {effective:?}"
        );
        // Since it wasn't recognized as an override, RTK still defaults to its own safe
        // check-only mode.
        assert!(effective.contains(&"--verify-no-changes".to_string()));

        // The bare boolean form still works as documented.
        let args = vec!["--write".to_string()];
        let tokens = tokenize_dotnet_args(&args);
        assert!(has_write_mode_override(&tokens));
        let effective = build_effective_dotnet_format_args(&args, &tokens, None);
        assert!(!effective.iter().any(|a| a == "--write"));
        assert!(!effective.iter().any(|a| a == "--verify-no-changes"));
    }

    #[test]
    fn test_has_nologo_arg_rejects_attached_value() {
        // -nologo is a pure boolean switch; a broken "-nologo:true" spelling must not be read as
        // "already present" (real dotnet rejects it with "MSB1002: This switch does not take any
        // parameters").
        let args = vec!["-nologo:true".to_string()];
        let tokens = tokenize_dotnet_args(&args);
        assert!(!has_nologo_arg(&tokens));

        // The bare boolean form (in any of its interchangeable legacy MSBuild spellings) still
        // matches as documented.
        for spelling in ["-nologo", "--nologo", "/nologo"] {
            let args = vec![spelling.to_string()];
            let tokens = tokenize_dotnet_args(&args);
            assert!(has_nologo_arg(&tokens), "{spelling} should be recognized");
        }
    }

    #[test]
    fn test_has_report_trx_arg_rejects_attached_value() {
        // Same class as --write/--nologo above: a broken attached-value spelling must not be
        // read as "already present".
        let args = vec!["--report-trx:true".to_string()];
        let tokens = tokenize_dotnet_args(&args);
        assert!(!has_report_trx_arg(&tokens));

        let args = vec!["--report-trx=true".to_string()];
        let tokens = tokenize_dotnet_args(&args);
        assert!(!has_report_trx_arg(&tokens));

        // The bare boolean form still matches as documented.
        let args = vec!["--report-trx".to_string()];
        let tokens = tokenize_dotnet_args(&args);
        assert!(has_report_trx_arg(&tokens));
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
