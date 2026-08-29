use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;
use anyhow::Result;
use serde::Deserialize;
use std::ffi::OsString;

#[derive(Debug, PartialEq)]
enum MoonbitSubcommand {
    Build,
    Test,
    Check,
    Other,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct MoonbitDiagnostic {
    #[serde(rename = "$message_type")]
    message_type: String,
    level: String,
    error_code: u32,
    path: String,
    loc: String,
    message: String,
}

fn detect_subcommand(args: &[String]) -> MoonbitSubcommand {
    for a in args.iter().filter(|a| !a.starts_with('-')) {
        match a.as_str() {
            "build" => return MoonbitSubcommand::Build,
            "test" => return MoonbitSubcommand::Test,
            "check" => return MoonbitSubcommand::Check,
            _ => continue,
        }
    }
    MoonbitSubcommand::Other
}

fn inject_json(args: &[String]) -> Vec<String> {
    let has_json = args.iter().any(|a| a == "--output-json");
    if has_json {
        args.to_vec()
    } else {
        let mut injected = args.to_vec();
        injected.push("--output-json".to_string());
        injected
    }
}

fn strip_message_prefix(msg: &str) -> &str {
    // Strip "Warning (xxx): " or "Error (xxx): " prefix
    for prefix in &["Warning", "Error"] {
        if let Some(rest) = msg.strip_prefix(prefix) {
            if let Some(after_paren) = rest.trim_start().strip_prefix('(') {
                if let Some(end) = after_paren.find("): ") {
                    return &after_paren[end + 3..];
                }
                if let Some(end) = after_paren.find("):") {
                    return &after_paren[end + 2..];
                }
            }
        }
    }
    msg
}

fn filter_moonbit_output(output: &str) -> String {
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()));
    let mut result = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with("[*]") {
            continue;
        }

        if trimmed.starts_with('{') {
            if let Ok(diag) = serde_json::from_str::<MoonbitDiagnostic>(trimmed) {
                if diag.message_type == "diagnostic" {
                    let rel_path = cwd
                        .as_deref()
                        .and_then(|cwd| diag.path.strip_prefix(cwd))
                        .map(|p| p.trim_start_matches('/'))
                        .unwrap_or(&diag.path);

                    let start_loc = diag.loc.split('-').next().unwrap_or(&diag.loc);

                    let clean_msg = strip_message_prefix(&diag.message);

                    result.push(format!(
                        "{}:{}: {} [{}]: {}",
                        rel_path, start_loc, diag.level, diag.error_code, clean_msg
                    ));
                    continue;
                }
            }
        }

        result.push(line.to_string());
    }

    result.join("\n")
}

fn run_with_json(args: &[String], verbose: u8) -> Result<i32> {
    let injected = inject_json(args);
    let display = injected.join(" ");

    if verbose > 0 {
        eprintln!("Running: moon {}", display);
    }

    let mut cmd = resolved_command("moon");
    for arg in &injected {
        cmd.arg(arg);
    }

    runner::run_filtered(
        cmd,
        "moon",
        &display,
        filter_moonbit_output,
        RunOptions::stdout_only().tee("moon"),
    )
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    match detect_subcommand(args) {
        MoonbitSubcommand::Build | MoonbitSubcommand::Test | MoonbitSubcommand::Check => {
            run_with_json(args, verbose)
        }
        MoonbitSubcommand::Other => {
            let osargs: Vec<OsString> = args.iter().map(OsString::from).collect();
            runner::run_passthrough("moon", &osargs, verbose)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_build() {
        assert_eq!(
            detect_subcommand(&["build".to_string()]),
            MoonbitSubcommand::Build
        );
    }

    #[test]
    fn test_detect_test() {
        assert_eq!(
            detect_subcommand(&["test".to_string()]),
            MoonbitSubcommand::Test
        );
    }

    #[test]
    fn test_detect_check() {
        assert_eq!(
            detect_subcommand(&["check".to_string()]),
            MoonbitSubcommand::Check
        );
    }

    #[test]
    fn test_detect_run_is_other() {
        assert_eq!(
            detect_subcommand(&["run".to_string(), "main".to_string()]),
            MoonbitSubcommand::Other
        );
    }

    #[test]
    fn test_detect_other() {
        assert_eq!(detect_subcommand(&["fmt".to_string()]), MoonbitSubcommand::Other);
        assert_eq!(
            detect_subcommand(&["new".to_string(), "myapp".to_string()]),
            MoonbitSubcommand::Other
        );
    }

    #[test]
    fn test_detect_empty_is_other() {
        assert_eq!(detect_subcommand(&[]), MoonbitSubcommand::Other);
    }

    #[test]
    fn test_inject_json_not_present() {
        let args = vec!["build".to_string()];
        let result = inject_json(&args);
        assert_eq!(result, vec!["build".to_string(), "--output-json".to_string()]);
    }

    #[test]
    fn test_inject_json_already_present() {
        let args = vec!["check".to_string(), "--output-json".to_string()];
        let result = inject_json(&args);
        assert_eq!(result, vec!["check".to_string(), "--output-json".to_string()]);
    }

    #[test]
    fn test_inject_json_skips_non_json_flags() {
        let args = vec![
            "build".to_string(),
            "--target".to_string(),
            "native".to_string(),
        ];
        let result = inject_json(&args);
        assert_eq!(
            result,
            vec![
                "build".to_string(),
                "--target".to_string(),
                "native".to_string(),
                "--output-json".to_string()
            ]
        );
    }

    #[test]
    fn test_detect_after_flags() {
        let args = vec!["--target".to_string(), "native".to_string(), "build".to_string()];
        assert_eq!(detect_subcommand(&args), MoonbitSubcommand::Build);
    }

    #[test]
    fn test_detect_unknown_value_skipped() {
        let args = vec!["--target".to_string(), "native".to_string(), "check".to_string()];
        assert_eq!(detect_subcommand(&args), MoonbitSubcommand::Check);
    }

    #[test]
    fn test_strip_message_prefix_warning() {
        assert_eq!(
            strip_message_prefix("Warning (unused_package): Unused package 'x'"),
            "Unused package 'x'"
        );
    }

    #[test]
    fn test_strip_message_prefix_error() {
        assert_eq!(
            strip_message_prefix("Error (E001): Type mismatch"),
            "Type mismatch"
        );
    }

    #[test]
    fn test_strip_message_prefix_no_match() {
        assert_eq!(
            strip_message_prefix("Just a normal message"),
            "Just a normal message"
        );
    }

    #[test]
    fn test_filter_moonbit_output_ndjson() {
        let input = r#"Warning: Package `x` does not declare `supported_targets`
{"$message_type":"diagnostic","level":"warning","error_code":29,"path":"/project/src/main.mbt","loc":"10:3-10:22","message":"Warning (unused_package): Unused package 'moonbitlang/async'"}
{"$message_type":"diagnostic","level":"warning","error_code":2,"path":"/project/src/utils.mbt","loc":"52:44-52:45","message":"Warning (unused_value): Unused variable 'e'"}
Finished. moon: ran 5 tasks, now up to date (2 warnings, 0 errors)"#;

        let filtered = filter_moonbit_output(input);
        assert!(filtered.contains("Warning: Package `x`"), "plain text warnings pass through");
        assert!(filtered.contains("/project/src/main.mbt:10:3: warning [29]: Unused package 'moonbitlang/async'"),
            "diagnostic reformatted: got:\n{}", filtered);
        assert!(filtered.contains("/project/src/utils.mbt:52:44: warning [2]: Unused variable 'e'"),
            "second diagnostic reformatted");
        assert!(filtered.contains("Finished. moon: ran 5 tasks"), "summary preserved");
    }

    #[test]
    fn test_filter_moonbit_output_test_summary() {
        let input = r#"Warning: Package `x` does not declare `supported_targets`
Total tests: 45, passed: 45, failed: 0."#;
        let filtered = filter_moonbit_output(input);
        assert!(filtered.contains("Warning: Package `x`"));
        assert!(filtered.contains("Total tests: 45, passed: 45, failed: 0."));
    }

    #[test]
    fn test_filter_moonbit_output_strips_empty_lines() {
        let input = "\n\n{\"$message_type\":\"diagnostic\",\"level\":\"warning\",\"error_code\":29,\"path\":\"/p/main.mbt\",\"loc\":\"5:1-5:5\",\"message\":\"test\"}\n\n\n";
        let filtered = filter_moonbit_output(input);
        assert!(!filtered.starts_with('\n'), "Leading blank lines should be stripped");
        assert!(!filtered.ends_with('\n'), "Trailing blank lines should be stripped");
        assert!(filtered.contains("/p/main.mbt:5:1: warning [29]: test"));
        assert_eq!(filtered.lines().count(), 1, "Should be exactly 1 line");
    }

    #[test]
    fn test_filter_moonbit_output_strips_bracket_progress() {
        let input = "[*] 1/5 tasks\n[*] 2/5 tasks\n{\"$message_type\":\"diagnostic\",\"level\":\"error\",\"error_code\":1,\"path\":\"/p/main.mbt\",\"loc\":\"5:1-5:5\",\"message\":\"Error (E001): compile error\"}\n[*] 5/5 tasks\nFinished\n";
        let filtered = filter_moonbit_output(input);
        assert!(!filtered.contains("[*]"), "Progress lines should be stripped");
        assert!(filtered.contains("compile error"));
    }

    #[test]
    fn test_filter_moonbit_output_preserves_non_json_lines() {
        let input = "Some random text\nMore text\n{\"$message_type\":\"other\",\"foo\":\"bar\"}\nend";
        let filtered = filter_moonbit_output(input);
        assert!(filtered.contains("Some random text"));
        assert!(filtered.contains("More text"));
        assert!(filtered.contains("end"));
        // Non-diagnostic JSON should pass through
        assert!(filtered.contains("\"$message_type\":\"other\""));
    }

    #[test]
    fn test_filter_moonbit_output_empty() {
        assert_eq!(filter_moonbit_output(""), "");
    }

    #[test]
    fn test_filter_check_fixture() {
        let input = include_str!("../../../tests/fixtures/moon_check_with_outjson.txt");
        let filtered = filter_moonbit_output(input);

        // NDJSON diagnostics should be reformatted to compact one-liners
        assert!(
            filtered.contains("warning [29]"),
            "warning codes should be preserved"
        );
        assert!(
            filtered.contains("warning [2]"),
            "warning code 2 should be preserved"
        );
        assert!(
            filtered.contains("warning [20]"),
            "warning code 20 should be preserved"
        );
        // Summary preserved
        assert!(
            filtered.contains("Finished. moon:"),
            "summary should be preserved"
        );
        // Plain text warning preserved
        assert!(
            filtered.contains("does not declare `supported_targets`"),
            "plain text warning should pass through"
        );
    }

    #[test]
    fn test_filter_test_fixture() {
        let input = include_str!("../../../tests/fixtures/moon_test_with_outjson.txt");
        let filtered = filter_moonbit_output(input);
        assert!(filtered.contains("Total tests: 45, passed: 45, failed: 0."));
        assert!(filtered.contains("does not declare `supported_targets`"));
    }
}
