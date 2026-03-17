use crate::tracking;
use crate::utils::{resolved_command, strip_ansi};
use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub enum FlutterCommand {
    Test,
    Analyze,
    Build,
    Pub,
}

pub fn run(cmd: FlutterCommand, args: &[String], verbose: u8) -> Result<()> {
    match cmd {
        FlutterCommand::Test => run_flutter_filtered("test", args, verbose, filter_flutter_test),
        FlutterCommand::Analyze => {
            run_flutter_filtered("analyze", args, verbose, filter_flutter_analyze)
        }
        FlutterCommand::Build => run_flutter_filtered("build", args, verbose, filter_flutter_build),
        FlutterCommand::Pub => run_flutter_pub(args, verbose),
    }
}

fn run_flutter_filtered<F>(
    subcommand: &str,
    args: &[String],
    verbose: u8,
    filter_fn: F,
) -> Result<()>
where
    F: Fn(&str) -> String,
{
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("flutter");
    cmd.arg(subcommand);

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: flutter {} {}", subcommand, args.join(" "));
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run flutter {}", subcommand))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}\n{}", stdout, stderr);

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });

    let filtered = filter_fn(&combined);

    if let Some(hint) =
        crate::tee::tee_and_hint(&combined, &format!("flutter_{}", subcommand), exit_code)
    {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("flutter {} {}", subcommand, args.join(" ")),
        &format!("rtk flutter {} {}", subcommand, args.join(" ")),
        &combined,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

fn run_flutter_pub(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("flutter");
    cmd.arg("pub");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: flutter pub {}", args.join(" "));
    }

    let output = cmd.output().context("Failed to run flutter pub")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}\n{}", stdout, stderr);

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });

    let filtered = filter_flutter_pub(&combined);

    if let Some(hint) = crate::tee::tee_and_hint(&combined, "flutter_pub", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("flutter pub {}", args.join(" ")),
        &format!("rtk flutter pub {}", args.join(" ")),
        &combined,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

/// Filter `flutter test` output — keep only failures; short-circuit to success line when clean.
///
/// Flutter's compact reporter emits lines like:
///   `HH:MM +N: test name`          — progress (suppress)
///   `HH:MM +N -M: test name [E]`   — failure header (keep)
///   indented detail lines           — failure body (keep)
///   `HH:MM +N: All tests passed!`  — clean summary (keep)
///   `HH:MM +N -M: Some tests failed.` — failure summary (keep)
///   `To run this test again: ...`  — rerun hint (keep)
pub fn filter_flutter_test(input: &str) -> String {
    let clean = strip_ansi(input);
    let mut result = Vec::new();
    let mut in_failure = false;

    for line in clean.lines() {
        let body = strip_test_prefix(line);

        if body.ends_with("All tests passed!") {
            return "\u{2713} All tests passed".to_string();
        }

        if body.ends_with("Some tests failed.") {
            in_failure = false;
            result.push(line.to_string());
            continue;
        }

        if is_failure_header(line) {
            in_failure = true;
            result.push(line.to_string());
            continue;
        }

        if in_failure {
            if line.starts_with("  ") || line.starts_with('\t') {
                result.push(line.to_string());
            } else if line.trim_start().starts_with("To run this test again:") {
                result.push(line.to_string());
                in_failure = false;
            } else {
                in_failure = false;
            }
            continue;
        }

        if has_test_prefix(line) {
            continue;
        }

        if !line.trim().is_empty() {
            result.push(line.to_string());
        }
    }

    if result.is_empty() {
        return "\u{2713} All tests passed".to_string();
    }

    result.join("\n")
}

/// Filter `flutter analyze` output — keep only `warning -` and `error -` lines.
///
/// Flutter analyze uses ` - ` (with spaces) as separator, unlike Dart's ` \u{2022} `.
/// The summary line ("N issues found") is written to stderr; we receive it in the
/// combined stdout+stderr feed and keep it.
pub fn filter_flutter_analyze(input: &str) -> String {
    let clean = strip_ansi(input);
    let mut result = Vec::new();

    for line in clean.lines() {
        let trimmed = line.trim_start();

        if trimmed.starts_with("error - ") || trimmed.starts_with("warning - ") {
            result.push(line.to_string());
            continue;
        }

        if trimmed.starts_with("No issues found!")
            || (trimmed.contains("issue")
                && (trimmed.ends_with('.') || trimmed.contains("ran in")))
        {
            result.push(trimmed.to_string());
        }
    }

    if result.is_empty() {
        return "\u{2713} No issues found".to_string();
    }

    result.join("\n")
}

/// Filter `flutter build` output — keep only the final result line and errors.
///
/// A successful build ends with a line like `\u{2713} Built build/app/...`
/// Errors begin with `Error:` or appear as compiler diagnostics.
pub fn filter_flutter_build(input: &str) -> String {
    let clean = strip_ansi(input);
    let mut result = Vec::new();
    let mut built_line: Option<String> = None;

    for line in clean.lines() {
        let trimmed = line.trim_start();

        if trimmed.starts_with("\u{2713} Built") || trimmed.starts_with("\u{221a} Built") {
            built_line = Some(trimmed.to_string());
            continue;
        }

        if trimmed.starts_with("Error:") || trimmed.starts_with("error:") {
            result.push(line.to_string());
            continue;
        }

        if trimmed.contains(": Error:") || trimmed.contains(": error:") {
            result.push(line.to_string());
            continue;
        }

        if trimmed.starts_with("FAILED") || trimmed.contains("Build failed") {
            result.push(trimmed.to_string());
        }
    }

    if let Some(built) = built_line {
        if result.is_empty() {
            return built;
        }
        result.push(built);
    }

    if result.is_empty() {
        return clean.trim().to_string();
    }

    result.join("\n")
}

/// Filter `flutter pub get/upgrade` output — strip download noise, keep summary.
///
/// Suppresses "Resolving dependencies..." and "Downloading packages..." headers
/// plus individual package download lines; keeps the "Got dependencies!" summary
/// and any compatibility warnings.
pub fn filter_flutter_pub(input: &str) -> String {
    let clean = strip_ansi(input);
    let mut result = Vec::new();

    for line in clean.lines() {
        let trimmed = line.trim_start();

        if trimmed.starts_with("Resolving dependencies")
            || trimmed.starts_with("Downloading packages")
        {
            continue;
        }

        if line.starts_with("  ") && !trimmed.is_empty() && looks_like_package_line(trimmed) {
            continue;
        }

        if !trimmed.is_empty() {
            result.push(trimmed.to_string());
        }
    }

    if result.is_empty() {
        return clean.trim().to_string();
    }

    result.join("\n")
}

/// Return `true` when a line looks like a package-version download entry.
///
/// These lines match the pattern `<package_name> <version> [(<newer> available)]`
/// where the package name contains only word chars, hyphens, and underscores.
fn looks_like_package_line(trimmed: &str) -> bool {
    let mut parts = trimmed.splitn(2, ' ');
    if let Some(name) = parts.next() {
        let name_ok = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if name_ok {
            if let Some(rest) = parts.next() {
                let first_char = rest.chars().next().unwrap_or(' ');
                return first_char.is_ascii_digit();
            }
        }
    }
    false
}

/// Strip the compact-reporter timestamp + counter prefix from a test line.
///
/// Format: `HH:MM +N: ` or `HH:MM +N -M: `
fn strip_test_prefix(line: &str) -> &str {
    let mut rest = line.trim_start();

    if rest.len() >= 6 && rest.as_bytes()[2] == b':' && rest.as_bytes()[5] == b' ' {
        rest = &rest[6..];
    } else {
        return line;
    }

    if let Some(colon_pos) = rest.find(": ") {
        &rest[colon_pos + 2..]
    } else {
        rest
    }
}

/// Return `true` when the line has the compact reporter timestamp prefix.
fn has_test_prefix(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.len() >= 7
        && trimmed.as_bytes()[2] == b':'
        && trimmed.as_bytes()[5] == b' '
        && trimmed.as_bytes()[6] == b'+'
}

/// Return `true` when the line is a failure header line (contains " -N:" counter).
fn is_failure_header(line: &str) -> bool {
    has_test_prefix(line) && {
        let trimmed = line.trim_start();
        if let Some(colon_pos) = trimmed[6..].find(": ") {
            let counter = &trimmed[6..6 + colon_pos];
            counter.contains(" -")
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("flutter")
            .join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("Cannot read fixture {}: {}", path.display(), e))
    }

    // --- flutter test ---

    #[test]
    fn test_flutter_test_passing_emits_success_line() {
        let raw = fixture("flutter_test_passing_raw.txt");
        let result = filter_flutter_test(&raw);
        assert_eq!(result, "\u{2713} All tests passed");
    }

    #[test]
    fn test_flutter_test_failing_shows_failure_block() {
        let raw = fixture("flutter_test_failing_raw.txt");
        let result = filter_flutter_test(&raw);
        assert!(
            result.contains("failing test name"),
            "missing failure header: {result}"
        );
        assert!(result.contains("Expected:"), "missing assertion detail: {result}");
        assert!(!result.contains("loading"), "progress line leaked: {result}");
    }

    #[test]
    fn test_flutter_test_failing_suppresses_progress() {
        let raw = fixture("flutter_test_failing_raw.txt");
        let result = filter_flutter_test(&raw);
        assert!(!result.contains("TsIconSize"), "progress line leaked: {result}");
    }

    #[test]
    fn test_flutter_test_empty_is_success() {
        let result = filter_flutter_test("");
        assert_eq!(result, "\u{2713} All tests passed");
    }

    #[test]
    fn test_flutter_test_token_savings() {
        let raw = fixture("flutter_test_passing_raw.txt");
        let result = filter_flutter_test(&raw);
        assert!(
            result.len() < raw.len() / 2,
            "expected >50% reduction, got raw={} filtered={}",
            raw.len(),
            result.len()
        );
    }

    // --- flutter analyze ---

    #[test]
    fn test_flutter_analyze_keeps_warnings_and_errors() {
        let raw = fixture("flutter_analyze_issues_raw.txt");
        let result = filter_flutter_analyze(&raw);
        assert!(
            result.contains("warning - Dead code"),
            "missing warning: {result}"
        );
        assert!(
            result.contains("warning - The left operand"),
            "missing warning: {result}"
        );
    }

    #[test]
    fn test_flutter_analyze_suppresses_info() {
        let raw = fixture("flutter_analyze_issues_raw.txt");
        let result = filter_flutter_analyze(&raw);
        assert!(!result.contains("info -"), "info line leaked: {result}");
    }

    #[test]
    fn test_flutter_analyze_suppresses_header() {
        let raw = fixture("flutter_analyze_issues_raw.txt");
        let result = filter_flutter_analyze(&raw);
        assert!(!result.contains("Analyzing"), "header line leaked: {result}");
    }

    #[test]
    fn test_flutter_analyze_clean_returns_success() {
        let clean = "Analyzing project...\n\nNo issues found!\n";
        let result = filter_flutter_analyze(clean);
        assert_eq!(result, "\u{2713} No issues found");
    }

    #[test]
    fn test_flutter_analyze_token_savings() {
        let raw = fixture("flutter_analyze_issues_raw.txt");
        let result = filter_flutter_analyze(&raw);
        assert!(
            result.len() < raw.len(),
            "expected reduction, got raw={} filtered={}",
            raw.len(),
            result.len()
        );
    }

    // --- flutter build ---

    #[test]
    fn test_flutter_build_success_shows_built_line() {
        let output = "Compiling...\nBuilding APK...\n\u{2713} Built build/app/outputs/flutter-apk/app-release.apk (7.2MB).\n";
        let result = filter_flutter_build(output);
        assert!(
            result.contains("Built build/app"),
            "missing built line: {result}"
        );
        assert!(!result.contains("Compiling"), "noise leaked: {result}");
    }

    #[test]
    fn test_flutter_build_error_shown() {
        let output = "Compiling...\nlib/main.dart:10:5: Error: Undefined name 'Foo'.\nBuild failed.\n";
        let result = filter_flutter_build(output);
        assert!(
            result.contains("Error:") || result.contains("error:"),
            "missing error: {result}"
        );
    }

    #[test]
    fn test_flutter_build_suppresses_progress() {
        let output = "Running Gradle task 'assembleRelease'...\n\u{2713} Built build/app/outputs/flutter-apk/app-release.apk (7.2MB).\n";
        let result = filter_flutter_build(output);
        assert!(!result.contains("Running Gradle"), "progress leaked: {result}");
    }

    // --- flutter pub ---

    #[test]
    fn test_flutter_pub_get_keeps_summary() {
        let raw = fixture("flutter_pub_get_raw.txt");
        let result = filter_flutter_pub(&raw);
        assert!(result.contains("Got dependencies!"), "missing summary: {result}");
    }

    #[test]
    fn test_flutter_pub_get_strips_download_header() {
        let raw = fixture("flutter_pub_get_raw.txt");
        let result = filter_flutter_pub(&raw);
        assert!(
            !result.contains("Downloading packages"),
            "header leaked: {result}"
        );
        assert!(
            !result.contains("Resolving dependencies"),
            "header leaked: {result}"
        );
    }

    #[test]
    fn test_flutter_pub_get_strips_package_lines() {
        let raw = fixture("flutter_pub_get_raw.txt");
        let result = filter_flutter_pub(&raw);
        assert!(
            !result.contains("_fe_analyzer_shared"),
            "package line leaked: {result}"
        );
        assert!(
            !result.contains("flutter_riverpod 2.6.1"),
            "package line leaked: {result}"
        );
    }

    #[test]
    fn test_flutter_pub_get_keeps_compat_warning() {
        let raw = fixture("flutter_pub_get_raw.txt");
        let result = filter_flutter_pub(&raw);
        assert!(
            result.contains("36 packages have newer versions"),
            "compat warning missing: {result}"
        );
    }

    #[test]
    fn test_flutter_pub_token_savings() {
        let raw = fixture("flutter_pub_get_raw.txt");
        let result = filter_flutter_pub(&raw);
        assert!(
            result.len() < raw.len() / 2,
            "expected >50% reduction, got raw={} filtered={}",
            raw.len(),
            result.len()
        );
    }

    // --- helpers ---

    #[test]
    fn test_has_test_prefix_detects_timestamp() {
        assert!(has_test_prefix("00:02 +20: All tests passed!"));
        assert!(has_test_prefix("00:01 +1: some test name"));
        assert!(!has_test_prefix("  Expected: 'foo'"));
        assert!(!has_test_prefix("To run this test again:"));
    }

    #[test]
    fn test_is_failure_header_detects_minus_counter() {
        assert!(is_failure_header("00:02 +2 -1: failing test name [E]"));
        assert!(!is_failure_header("00:01 +1: passing test name"));
    }

    #[test]
    fn test_looks_like_package_line_valid() {
        assert!(looks_like_package_line("flutter_riverpod 2.6.1 (3.3.1 available)"));
        assert!(looks_like_package_line("_fe_analyzer_shared 85.0.0"));
        assert!(!looks_like_package_line("Got dependencies!"));
        assert!(!looks_like_package_line("36 packages have newer versions"));
    }
}
