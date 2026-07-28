//! Netlify CLI output compression for read-only deploy inspection.
//!
//! Mutable operations, streaming logs, machine-readable modes, and unknown
//! command shapes remain passthrough.

use crate::core::runner::{self, RunMode, RunOptions};
use crate::core::tee::force_tee_hint;
use crate::core::truncate::CAP_LIST;
use crate::core::utils::resolved_command;
use anyhow::Result;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FilterKind {
    DeployLogs,
    SiteDeploys,
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let display = sanitize_args(args);
    let Some(kind) = filter_kind(args) else {
        return run_passthrough(args, &display, verbose);
    };

    let mut cmd = resolved_command("netlify");
    cmd.args(args);
    if verbose > 0 {
        eprintln!("Running: netlify {display}");
    }

    runner::run_filtered(
        cmd,
        "netlify",
        &display,
        move |output| filter_output(kind, output),
        RunOptions::with_tee("netlify").early_exit_on_failure(),
    )
}

fn run_passthrough(args: &[String], display: &str, verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("netlify");
    cmd.args(args);
    if verbose > 0 {
        eprintln!("netlify passthrough: {display}");
    }
    runner::run(
        cmd,
        "netlify",
        display,
        RunMode::Passthrough,
        RunOptions::default(),
    )
}

fn filter_kind(args: &[String]) -> Option<FilterKind> {
    if has_any_flag(args, &["--follow", "--json", "--debug"]) {
        return None;
    }

    match args {
        [command, rest @ ..]
            if command == "logs" && option_value(rest, "--source") == Some("deploy") =>
        {
            Some(FilterKind::DeployLogs)
        }
        [command, operation, ..]
            if command == "api" && operation.eq_ignore_ascii_case("listSiteDeploys") =>
        {
            Some(FilterKind::SiteDeploys)
        }
        _ => None,
    }
}

fn has_any_flag(args: &[String], flags: &[&str]) -> bool {
    args.iter().any(|arg| {
        flags
            .iter()
            .any(|flag| arg == flag || arg.starts_with(&format!("{flag}=")))
    })
}

fn option_value<'a>(args: &'a [String], option: &str) -> Option<&'a str> {
    args.iter().enumerate().find_map(|(index, arg)| {
        if arg == option {
            args.get(index + 1).map(String::as_str)
        } else {
            arg.strip_prefix(&format!("{option}="))
        }
    })
}

fn sanitize_args(args: &[String]) -> String {
    const SENSITIVE: &[&str] = &["--auth", "--auth-token", "--token", "--data"];
    let mut result = Vec::with_capacity(args.len());
    let mut redact_next = false;
    for arg in args {
        if redact_next {
            result.push("[REDACTED]".to_string());
            redact_next = false;
            continue;
        }

        if SENSITIVE.contains(&arg.as_str()) {
            result.push(arg.clone());
            redact_next = true;
        } else if let Some((name, _)) = arg.split_once('=') {
            if SENSITIVE.contains(&name) {
                result.push(format!("{name}=[REDACTED]"));
            } else {
                result.push(arg.clone());
            }
        } else {
            result.push(arg.clone());
        }
    }
    result.join(" ")
}

fn filter_output(kind: FilterKind, output: &str) -> String {
    if output.trim().is_empty() {
        return String::new();
    }
    match kind {
        FilterKind::DeployLogs => filter_deploy_logs(output),
        FilterKind::SiteDeploys => filter_site_deploys(output),
    }
}

fn filter_deploy_logs(output: &str) -> String {
    let lower = output.to_ascii_lowercase();
    let recognized = [
        "finished processing build request",
        "build script success",
        "build failed",
        "site is live",
        "deploy is live",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    if !recognized {
        return output.to_string();
    }

    let retained = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| important_log_line(line))
        .map(strip_log_timestamp)
        .map(str::to_string)
        .collect::<Vec<_>>();
    if retained.is_empty() {
        return output.to_string();
    }
    cap_lines(output, retained, "netlify-deploy-logs")
}

fn strip_log_timestamp(line: &str) -> &str {
    [" AM: ", " PM: "]
        .iter()
        .find_map(|marker| line.split_once(marker).map(|(_, message)| message))
        .unwrap_or(line)
}

fn important_log_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "error",
        "warning",
        "warn:",
        "failed",
        "failure",
        "cancel",
        "starting build",
        "build command",
        "build script",
        "functions bundling",
        "deploy site",
        "starting to deploy",
        "post processing",
        "site is live",
        "deploy is live",
        "finished processing build request",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
        || line.starts_with("❯")
        || line.starts_with("Section completed:")
}

fn filter_site_deploys(output: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(output.trim()) else {
        return output.to_string();
    };
    let Some(deploys) = value.as_array() else {
        return output.to_string();
    };

    let mut lines = Vec::with_capacity(deploys.len() + 1);
    lines.push(format!("Deploys: {}", deploys.len()));
    for deploy in deploys {
        let Some(object) = deploy.as_object() else {
            return output.to_string();
        };
        let Some(id) = string_field(object, "id") else {
            return output.to_string();
        };
        let state = string_field(object, "state").unwrap_or("unknown");
        let context = string_field(object, "context").unwrap_or("-");
        let branch = string_field(object, "branch").unwrap_or("-");
        let commit = string_field(object, "commit_ref")
            .map(short_commit)
            .unwrap_or("-");
        let created = string_field(object, "created_at").unwrap_or("-");
        lines.push(format!(
            "{id} {state} context={context} branch={branch} commit={commit} created={created}"
        ));
    }
    cap_lines(output, lines, "netlify-site-deploys")
}

fn string_field<'a>(
    object: &'a serde_json::Map<String, Value>,
    name: &str,
) -> Option<&'a str> {
    object.get(name).and_then(Value::as_str)
}

fn short_commit(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

fn cap_lines(output: &str, mut lines: Vec<String>, slug: &str) -> String {
    if lines.len() <= CAP_LIST + 1 {
        return lines.join("\n");
    }

    let total = lines.len() - 1;
    lines.truncate(CAP_LIST + 1);
    lines.push(format!("... +{} more entries", total - CAP_LIST));
    if let Some(hint) = force_tee_hint(output, slug) {
        lines.push(hint);
    } else {
        lines.push("[run the original command without rtk for full output]".to_string());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn filters_only_historical_deploy_queries() {
        assert_eq!(
            filter_kind(&args(&["logs", "--source", "deploy"])),
            Some(FilterKind::DeployLogs)
        );
        assert_eq!(
            filter_kind(&args(&["logs", "--source=deploy", "--site", "abc"])),
            Some(FilterKind::DeployLogs)
        );
        assert_eq!(
            filter_kind(&args(&["api", "listSiteDeploys", "--data", "{}"])),
            Some(FilterKind::SiteDeploys)
        );
    }

    #[test]
    fn risky_and_machine_readable_commands_are_passthrough() {
        for command in [
            &["deploy", "--prod"][..],
            &["logs", "--source", "functions"][..],
            &["logs", "--source", "deploy", "--follow"][..],
            &["logs", "--source", "deploy", "--json"][..],
            &["logs", "--source", "deploy", "--debug"][..],
            &["api", "createSiteDeploy"][..],
        ] {
            assert_eq!(filter_kind(&args(command)), None, "command: {command:?}");
        }
    }

    #[test]
    fn sensitive_arguments_are_redacted_from_tracking_display() {
        let display = sanitize_args(&args(&[
            "api",
            "listSiteDeploys",
            "--auth",
            "secret-token",
            "--data={\"site_id\":\"sensitive\"}",
        ]));
        assert!(!display.contains("secret-token"));
        assert!(!display.contains("site_id"));
        assert_eq!(
            display,
            "api listSiteDeploys --auth [REDACTED] --data=[REDACTED]"
        );
    }

    #[test]
    fn invalid_or_unknown_json_falls_back_to_raw() {
        for raw in [
            "not json\n",
            "{\"id\":\"not-an-array\"}\n",
            "[{\"state\":\"ready\"}]\n",
        ] {
            assert_eq!(filter_output(FilterKind::SiteDeploys, raw), raw);
        }
    }

    #[test]
    fn deploy_list_preserves_identity_state_and_commit() {
        let raw = r#"[
          {"id":"deploy-1","state":"ready","context":"production","branch":"main","commit_ref":"abcdef123456","created_at":"2026-07-28T10:00:00Z","summary":{"status":"ready"},"links":{"permalink":"https://example.invalid/1"}},
          {"id":"deploy-2","state":"error","context":"deploy-preview","branch":"feature","commit_ref":"123456789abc","created_at":"2026-07-27T10:00:00Z","summary":{"status":"error"},"links":{"permalink":"https://example.invalid/2"}}
        ]"#;
        let filtered = filter_output(FilterKind::SiteDeploys, raw);
        assert!(filtered.starts_with("Deploys: 2"));
        assert!(filtered.contains("deploy-1 ready context=production"));
        assert!(filtered.contains("commit=abcdef1"));
        assert!(filtered.contains("deploy-2 error"));
        assert!(!filtered.contains("permalink"));
    }

    #[test]
    fn deploy_logs_preserve_phases_warnings_errors_and_result() {
        let raw = "\
10:00:00 AM: build-image version: abc
10:00:01 AM: Fetching cached dependencies
10:00:02 AM: Starting build script
10:00:03 AM: Build command from Netlify app
10:00:04 AM: npm warning deprecated package
10:00:05 AM: Bundling application files
10:00:06 AM: Functions bundling
10:00:07 AM: Starting to deploy site from 'out'
10:00:08 AM: Post processing
10:00:09 AM: Site is live
10:00:10 AM: Finished processing build request in 10s
";
        let filtered = filter_output(FilterKind::DeployLogs, raw);
        assert!(filtered.contains("Starting build script"));
        assert!(filtered.contains("Build command"));
        assert!(filtered.contains("warning"));
        assert!(filtered.contains("Functions bundling"));
        assert!(filtered.contains("Site is live"));
        assert!(filtered.contains("Finished processing"));
        assert!(!filtered.contains("Fetching cached dependencies"));
    }

    #[test]
    fn unknown_log_format_falls_back_to_raw() {
        let raw = "A future Netlify log format\nwith unrelated lines\n";
        assert_eq!(filter_output(FilterKind::DeployLogs, raw), raw);
    }

    #[test]
    fn representative_output_saves_at_least_sixty_percent() {
        let raw = "\
10:00:00 AM: build-image version: abc
10:00:01 AM: buildbot version: def
10:00:02 AM: Fetching cached dependencies
10:00:03 AM: Started restoring cached node modules
10:00:04 AM: Finished restoring cached node modules
10:00:05 AM: Starting build script
10:00:06 AM: Build command from Netlify app
10:00:07 AM: Installing dependencies
10:00:08 AM: Dependencies installed
10:00:09 AM: Build script success
10:00:10 AM: Starting to deploy site from 'out'
10:00:11 AM: Uploading 120 files
10:00:12 AM: Post processing
10:00:13 AM: Site is live
10:00:14 AM: Finished processing build request in 14s
";
        let filtered = filter_output(FilterKind::DeployLogs, raw);
        let savings = 1.0 - filtered.len() as f64 / raw.len() as f64;
        assert!(
            savings >= 0.60,
            "expected at least 60% savings, got {:.1}%",
            savings * 100.0
        );
    }
}
