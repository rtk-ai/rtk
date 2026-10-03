//! Salesforce CLI (`sf`) command filter router.

use super::apex_test::filter_apex_test;
use super::common::{has_flag, FilterOptions, FilterOutcome};
use super::deploy::filter_deploy;
use super::retrieve::filter_retrieve;
use crate::core::runner;
use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::utils::{exit_code_from_status, resolved_command};
use anyhow::{Context, Result};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SfCommandKind {
    Deploy,
    Retrieve,
    ApexTest,
    Passthrough,
}

pub fn run(args: &[String], verbose: u8, ultra_compact: bool) -> Result<i32> {
    match detect_kind(args) {
        SfCommandKind::Passthrough => run_passthrough(args, verbose),
        kind => run_filtered(args, kind, verbose, ultra_compact),
    }
}

fn detect_kind(args: &[String]) -> SfCommandKind {
    if args.len() >= 3
        && args[0] == "project"
        && args[1] == "deploy"
        && args[2] == "start"
    {
        SfCommandKind::Deploy
    } else if args.len() >= 3
        && args[0] == "project"
        && args[1] == "retrieve"
        && args[2] == "start"
    {
        SfCommandKind::Retrieve
    } else if args.len() >= 3 && args[0] == "apex" && args[1] == "run" && args[2] == "test" {
        SfCommandKind::ApexTest
    } else {
        SfCommandKind::Passthrough
    }
}

fn build_sf_command(args: &[String], kind: SfCommandKind) -> Command {
    let mut cmd = resolved_command("sf");
    for arg in args {
        cmd.arg(arg);
    }
    if !has_flag(args, "--json") {
        cmd.arg("--json");
    }
    if kind == SfCommandKind::Deploy && !has_flag(args, "--concise") {
        cmd.arg("--concise");
    }
    cmd
}

fn run_filtered(
    args: &[String],
    kind: SfCommandKind,
    verbose: u8,
    ultra_compact: bool,
) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let cmd_label = format!("sf {}", args.join(" "));
    let rtk_label = format!("rtk {cmd_label}");
    let slug = cmd_label.replace(' ', "_");
    let opts = FilterOptions { ultra_compact };

    let mut cmd = build_sf_command(args, kind);
    if verbose > 0 {
        eprintln!("Running: {cmd_label}");
    }

    let result = exec_capture(&mut cmd).context("Failed to run sf")?;
    let raw = format!("{}\n{}", result.stdout, result.stderr);

    if !result.success() {
        let hint = crate::core::tee::tee_and_hint(&raw, &slug, result.exit_code);
        if let Some(h) = hint {
            eprintln!("{}\n{}", result.stderr.trim(), h);
        } else if !result.stderr.is_empty() {
            eprintln!("{}", result.stderr.trim());
        }
        timer.track(&cmd_label, &rtk_label, &raw, &raw);
        return Ok(result.exit_code);
    }

    let outcome = apply_filter(kind, &result.stdout, opts);

    if outcome.passthrough {
        print!("{}", result.stdout);
        if !result.stderr.is_empty() {
            eprint!("{}", result.stderr);
        }
        timer.track(&cmd_label, &rtk_label, &raw, &raw);
        return Ok(result.exit_code);
    }

    let hint = if outcome.truncated {
        crate::core::tee::force_tee_hint(&raw, &slug)
    } else {
        crate::core::tee::tee_and_hint(&raw, &slug, 0)
    };
    let shown = runner::emit_guarded(&outcome.text, hint.as_deref(), &result.stdout);
    timer.track(&cmd_label, &rtk_label, &raw, &shown);
    Ok(result.exit_code)
}

fn apply_filter(kind: SfCommandKind, stdout: &str, opts: FilterOptions) -> FilterOutcome {
    match kind {
        SfCommandKind::Deploy => filter_deploy(stdout, opts),
        SfCommandKind::Retrieve => filter_retrieve(stdout, opts),
        SfCommandKind::ApexTest => filter_apex_test(stdout, opts),
        SfCommandKind::Passthrough => FilterOutcome::passthrough(stdout.to_string()),
    }
}

pub fn run_passthrough(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let cmd_label = format!("sf {}", args.join(" "));

    let mut cmd = resolved_command("sf");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: {cmd_label}");
    }

    let status = cmd.status().context("Failed to run sf")?;
    let exit_code = exit_code_from_status(&status, "sf");

    timer.track(
        &cmd_label,
        &format!("rtk {cmd_label}"),
        &cmd_label,
        &cmd_label,
    );

    Ok(exit_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_kind_routes_three_commands() {
        assert_eq!(
            detect_kind(&[
                "project".into(),
                "deploy".into(),
                "start".into(),
            ]),
            SfCommandKind::Deploy
        );
        assert_eq!(
            detect_kind(&[
                "project".into(),
                "retrieve".into(),
                "start".into(),
            ]),
            SfCommandKind::Retrieve
        );
        assert_eq!(
            detect_kind(&["apex".into(), "run".into(), "test".into()]),
            SfCommandKind::ApexTest
        );
        assert_eq!(detect_kind(&["org".into(), "list".into()]), SfCommandKind::Passthrough);
    }

    #[test]
    fn build_sf_command_injects_json_and_concise_for_deploy() {
        let args = vec![
            "project".into(),
            "deploy".into(),
            "start".into(),
            "--source-dir".into(),
            "force-app".into(),
        ];
        let cmd = build_sf_command(&args, SfCommandKind::Deploy);
        let rendered: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(rendered.contains(&"--json".to_string()));
        assert!(rendered.contains(&"--concise".to_string()));
    }

    #[test]
    fn build_sf_command_does_not_duplicate_flags() {
        let args = vec![
            "project".into(),
            "deploy".into(),
            "start".into(),
            "--json".into(),
            "--concise".into(),
        ];
        let cmd = build_sf_command(&args, SfCommandKind::Deploy);
        let rendered: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(rendered.iter().filter(|a| *a == "--json").count(), 1);
        assert_eq!(rendered.iter().filter(|a| *a == "--concise").count(), 1);
    }

    /// Documented savings in `README.md` — update both when filters or fixtures change.
    #[test]
    fn fixture_savings_table() {
        use crate::cmds::salesforce::apex_test::filter_apex_test;
        use crate::cmds::salesforce::common::FilterOptions;
        use crate::cmds::salesforce::deploy::filter_deploy;
        use crate::cmds::salesforce::retrieve::filter_retrieve;
        use crate::core::utils::count_tokens;

        fn savings_pct(raw: &str, filtered: &str) -> f64 {
            let raw_n = count_tokens(raw) as f64;
            let filtered_n = count_tokens(filtered) as f64;
            100.0 - (filtered_n / raw_n * 100.0)
        }

        let opts = FilterOptions { ultra_compact: false };

        let deploy_verbose = include_str!("../../../tests/fixtures/salesforce/deploy_success_verbose.json");
        let deploy_verbose_out = filter_deploy(deploy_verbose, opts);

        let deploy_concise = include_str!("../../../tests/fixtures/salesforce/deploy_success_concise.json");
        let deploy_concise_out = filter_deploy(deploy_concise, opts);

        let deploy_failed = include_str!("../../../tests/fixtures/salesforce/deploy_failed.json");
        let deploy_failed_out = filter_deploy(deploy_failed, opts);

        let retrieve = include_str!("../../../tests/fixtures/salesforce/retrieve_success.json");
        let retrieve_out = filter_retrieve(retrieve, opts);

        let apex_pass = include_str!("../../../tests/fixtures/salesforce/apex_test_pass_with_coverage.json");
        let apex_pass_out = filter_apex_test(apex_pass, opts);

        let apex_failed = include_str!("../../../tests/fixtures/salesforce/apex_test_failed.json");
        let apex_failed_out = filter_apex_test(apex_failed, opts);

        let apex_async = include_str!("../../../tests/fixtures/salesforce/apex_test_async.json");
        let apex_async_out = filter_apex_test(apex_async, opts);

        let cases: [(&str, usize, usize, f64, bool); 7] = [
            (
                "deploy_success_verbose.json",
                count_tokens(deploy_verbose),
                count_tokens(&deploy_verbose_out.text),
                savings_pct(deploy_verbose, &deploy_verbose_out.text),
                false,
            ),
            (
                "deploy_success_concise.json",
                count_tokens(deploy_concise),
                count_tokens(&deploy_concise_out.text),
                savings_pct(deploy_concise, &deploy_concise_out.text),
                false,
            ),
            (
                "deploy_failed.json",
                count_tokens(deploy_failed),
                count_tokens(&deploy_failed_out.text),
                savings_pct(deploy_failed, &deploy_failed_out.text),
                false,
            ),
            (
                "retrieve_success.json",
                count_tokens(retrieve),
                count_tokens(&retrieve_out.text),
                savings_pct(retrieve, &retrieve_out.text),
                false,
            ),
            (
                "apex_test_pass_with_coverage.json",
                count_tokens(apex_pass),
                count_tokens(&apex_pass_out.text),
                savings_pct(apex_pass, &apex_pass_out.text),
                false,
            ),
            (
                "apex_test_failed.json",
                count_tokens(apex_failed),
                count_tokens(&apex_failed_out.text),
                savings_pct(apex_failed, &apex_failed_out.text),
                false,
            ),
            (
                "apex_test_async.json",
                count_tokens(apex_async),
                count_tokens(&apex_async_out.text),
                savings_pct(apex_async, &apex_async_out.text),
                apex_async_out.passthrough,
            ),
        ];

        let expected: [(&str, usize, usize, f64, bool); 7] = [
            ("deploy_success_verbose.json", 1168, 1, 99.9, false),
            ("deploy_success_concise.json", 86, 1, 98.8, false),
            ("deploy_failed.json", 162, 10, 93.8, false),
            ("retrieve_success.json", 99, 1, 99.0, false),
            ("apex_test_pass_with_coverage.json", 281, 3, 98.9, false),
            ("apex_test_failed.json", 289, 11, 96.2, false),
            ("apex_test_async.json", 11, 11, 0.0, true),
        ];

        for ((file, raw, rtk, pct, passthrough), exp) in cases.iter().zip(expected.iter()) {
            assert_eq!(*file, exp.0, "fixture name");
            assert_eq!(*raw, exp.1, "{file} raw tokens");
            assert_eq!(*rtk, exp.2, "{file} rtk tokens");
            assert!(
                (pct - exp.3).abs() < 0.15,
                "{file} savings: got {pct:.1} expected {}",
                exp.3
            );
            assert_eq!(*passthrough, exp.4, "{file} passthrough");
        }
    }
}
