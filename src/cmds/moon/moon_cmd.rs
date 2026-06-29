use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;
use anyhow::Result;
use std::ffi::OsString;

#[derive(Debug, PartialEq)]
enum MoonSubcommand {
    Build,
    Test,
    Check,
    Run,
    Other,
}

fn detect_subcommand(args: &[String]) -> MoonSubcommand {
    for a in args.iter().filter(|a| !a.starts_with('-')) {
        match a.as_str() {
            "build" => return MoonSubcommand::Build,
            "test" => return MoonSubcommand::Test,
            "check" => return MoonSubcommand::Check,
            "run" => return MoonSubcommand::Run,
            _ => continue,
        }
    }
    MoonSubcommand::Other
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

fn identity_filter(output: &str) -> String {
    output.to_string()
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
        identity_filter,
        RunOptions::stdout_only().tee("moon"),
    )
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    match detect_subcommand(args) {
        MoonSubcommand::Build | MoonSubcommand::Test | MoonSubcommand::Check => {
            run_with_json(args, verbose)
        }
        MoonSubcommand::Run | MoonSubcommand::Other => {
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
            MoonSubcommand::Build
        );
    }

    #[test]
    fn test_detect_test() {
        assert_eq!(
            detect_subcommand(&["test".to_string()]),
            MoonSubcommand::Test
        );
    }

    #[test]
    fn test_detect_check() {
        assert_eq!(
            detect_subcommand(&["check".to_string()]),
            MoonSubcommand::Check
        );
    }

    #[test]
    fn test_detect_run() {
        assert_eq!(
            detect_subcommand(&["run".to_string(), "main".to_string()]),
            MoonSubcommand::Run
        );
    }

    #[test]
    fn test_detect_other() {
        assert_eq!(
            detect_subcommand(&["fmt".to_string()]),
            MoonSubcommand::Other
        );
        assert_eq!(
            detect_subcommand(&["new".to_string(), "myapp".to_string()]),
            MoonSubcommand::Other
        );
    }

    #[test]
    fn test_detect_empty_is_other() {
        let empty: Vec<String> = vec![];
        assert_eq!(detect_subcommand(&empty), MoonSubcommand::Other);
    }

    #[test]
    fn test_inject_json_not_present() {
        let args = vec!["build".to_string()];
        let result = inject_json(&args);
        assert_eq!(result, vec!["build".to_string(), "--output-json".to_string()]);
    }

    #[test]
    fn test_inject_json_already_present() {
        let args = vec![
            "check".to_string(),
            "--output-json".to_string(),
        ];
        let result = inject_json(&args);
        assert_eq!(result, vec!["check".to_string(), "--output-json".to_string()]);
    }

    #[test]
    fn test_inject_json_skips_non_json_flags() {
        let args = vec!["build".to_string(), "--target".to_string(), "native".to_string()];
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
        let args = vec![
            "--target".to_string(),
            "native".to_string(),
            "build".to_string(),
        ];
        assert_eq!(detect_subcommand(&args), MoonSubcommand::Build);
    }

    #[test]
    fn test_detect_unknown_value_skipped() {
        // "native" is not a subcommand, so should find "check"
        let args = vec![
            "--target".to_string(),
            "native".to_string(),
            "check".to_string(),
        ];
        assert_eq!(detect_subcommand(&args), MoonSubcommand::Check);
    }
}
