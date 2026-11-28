//! Filters `launchctl list` by leaving out Apple's own `com.apple.*` system services, which
//! make up nearly all of the listing on a stock macOS (530 of 545 jobs on the fixture), and
//! keeping the header and every other job verbatim. Every other invocation passes through:
//! `list <label>` prints one job's dictionary, `print` a different report, and
//! `load`/`unload`/`bootstrap`/`bootout` print nothing on success.

use crate::core::guard::never_worse;
use crate::core::shell::{display_args, with_args};
use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};
use std::ffi::OsString;
use std::sync::LazyLock;

/// `PID<TAB>Status<TAB>Label`, where a job that is not running has `-` for its PID and a job
/// that never exited has `-` for its status. A status is negative when the job was killed by a
/// signal (`-9` is routine for Apple agents that launchd reclaims).
static JOB_LINE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^(?:-|\d+)\t(?:-|-?\d+)\t[^\t]+$").unwrap());

const HEADER: &str = "PID\tStatus\tLabel";

/// Launchd's own namespace for the services Apple ships. User-launched Apple apps register as
/// `application.com.apple.<App>.<ids>` and stay visible like any other app.
const SYSTEM_PREFIX: &str = "com.apple.";

/// Recovery slug for the jobs the filter leaves out.
const TEE_SLUG: &str = "launchctl";

/// Bytes the compaction must save before storing the output to point at is worth it, as in
/// `ast_grep_cmd`: the hint names a path that only exists once the store is written, so it
/// cannot be measured before the decision that creates it.
const HINT_RESERVE: usize = 256;

struct Filtered {
    text: String,
    hidden_jobs: usize,
}

fn unchanged(raw: &str) -> Filtered {
    Filtered {
        text: raw.to_string(),
        hidden_jobs: 0,
    }
}

fn is_system_job(line: &str) -> bool {
    line.rsplit('\t')
        .next()
        .is_some_and(|label| label.starts_with(SYSTEM_PREFIX))
}

/// Any line outside the `PID<TAB>Status<TAB>Label` shape leaves the whole output alone: a
/// listing launchctl has started to print differently is safer shown whole than half-parsed.
fn filter_launchctl_list(raw: &str) -> Filtered {
    let mut lines = raw.lines();
    if lines.next() != Some(HEADER) {
        return unchanged(raw);
    }
    let jobs: Vec<&str> = lines.collect();
    if !jobs.iter().all(|line| JOB_LINE_RE.is_match(line)) {
        return unchanged(raw);
    }

    let hidden_jobs = jobs.iter().filter(|line| is_system_job(line)).count();
    if hidden_jobs == 0 {
        return unchanged(raw);
    }

    let mut text = format!("{HEADER}\n");
    for line in jobs.iter().filter(|line| !is_system_job(line)) {
        text.push_str(line);
        text.push('\n');
    }
    text.push_str(&format!(
        "… {hidden_jobs} {SYSTEM_PREFIX}* system services not shown\n"
    ));
    Filtered { text, hidden_jobs }
}

/// `launchctl list` takes no options, and a label after it switches to a one-job dictionary
/// report, so the only invocation with the tabular shape is the bare one. That is an exact
/// match on the whole argument list rather than a flag scan, so nothing needs tokenizing.
fn filters_this_invocation(args: &[String]) -> bool {
    matches!(args, [only] if only == "list")
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if !filters_this_invocation(args) {
        let forwarded: Vec<OsString> = args.iter().map(OsString::from).collect();
        return crate::core::runner::run_passthrough("launchctl", &forwarded, verbose);
    }

    let timer = tracking::TimedExecution::start();
    let real_cmd = with_args("launchctl", &display_args(args));

    let mut cmd = resolved_command("launchctl");
    cmd.args(args);
    let result = exec_capture(&mut cmd).context("Failed to execute launchctl")?;

    let mut out = filter_launchctl_list(&result.stdout);
    if out.hidden_jobs > 0 {
        let margin = result.stdout.len().saturating_sub(out.text.len());
        match (margin > HINT_RESERVE)
            .then(|| crate::core::tee::force_tee_hint(&result.stdout, TEE_SLUG))
            .flatten()
        {
            Some(hint) => {
                out.text.push_str(&hint);
                out.text.push('\n');
            }
            None => out
                .text
                .push_str("  (rtk bypass launchctl list for the full list)\n"),
        }
    }

    let shown = never_worse(&result.stdout, &out.text);
    timer.track(&real_cmd, "rtk launchctl", &result.stdout, shown);
    print!("{}", shown);

    if !result.stderr.is_empty() {
        eprint!("{}", result.stderr);
    }

    Ok(result.exit_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../../tests/fixtures/launchctl_list_raw.txt");

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn keeps_header_and_non_system_jobs_and_counts_the_rest() {
        let raw = "PID\tStatus\tLabel\n\
                   -\t0\tcom.apple.SafariHistoryServiceAgent\n\
                   3002\t0\thomebrew.mxcl.postgresql@16\n\
                   -\t-9\tcom.apple.progressd\n\
                   -\t78\tcom.example.server\n\
                   1271\t0\tcom.apple.Finder\n";
        let out = filter_launchctl_list(raw);
        assert_eq!(
            out.text,
            "PID\tStatus\tLabel\n\
             3002\t0\thomebrew.mxcl.postgresql@16\n\
             -\t78\tcom.example.server\n\
             … 3 com.apple.* system services not shown\n"
        );
        assert_eq!(out.hidden_jobs, 3);
    }

    #[test]
    fn user_launched_apple_apps_stay_visible() {
        let raw = "PID\tStatus\tLabel\n\
                   28479\t0\tapplication.com.apple.Terminal.1152921500311915999.1152921500311916004\n\
                   -\t0\tcom.apple.cloudphotod\n";
        let out = filter_launchctl_list(raw);
        assert!(
            out.text.contains(
                "\tapplication.com.apple.Terminal.1152921500311915999.1152921500311916004\n"
            ),
            "got:\n{}",
            out.text
        );
        assert_eq!(out.hidden_jobs, 1);
    }

    #[test]
    fn real_listing_keeps_every_non_system_job_in_order() {
        let out = filter_launchctl_list(FIXTURE);
        let expected_kept: Vec<&str> = FIXTURE
            .lines()
            .skip(1)
            .filter(|l| {
                !l.split('\t')
                    .nth(2)
                    .unwrap_or("")
                    .starts_with(SYSTEM_PREFIX)
            })
            .collect();
        let kept: Vec<&str> = out
            .text
            .lines()
            .skip(1)
            .take_while(|l| !l.starts_with('…'))
            .collect();
        assert_eq!(kept, expected_kept);
        assert_eq!(kept.len(), 15);
        assert_eq!(out.hidden_jobs, 530);
        assert!(out.text.starts_with("PID\tStatus\tLabel\n"));
        assert!(!out.text.contains("\tcom.apple."));
    }

    #[test]
    fn real_listing_saves_at_least_twenty_percent() {
        let out = filter_launchctl_list(FIXTURE);
        let savings =
            100.0 - (count_tokens(&out.text) as f64 / count_tokens(FIXTURE) as f64 * 100.0);
        assert!(savings >= 20.0, "expected >=20% savings, got {savings:.1}%");
    }

    #[test]
    fn only_system_jobs_leaves_header_and_count() {
        let raw = "PID\tStatus\tLabel\n-\t0\tcom.apple.a\n-\t-9\tcom.apple.b\n";
        let out = filter_launchctl_list(raw);
        assert_eq!(
            out.text,
            "PID\tStatus\tLabel\n… 2 com.apple.* system services not shown\n"
        );
    }

    #[test]
    fn listing_without_system_jobs_is_unchanged() {
        let raw = "PID\tStatus\tLabel\n-\t0\tcom.example.server\n";
        let out = filter_launchctl_list(raw);
        assert_eq!(out.text, raw);
        assert_eq!(out.hidden_jobs, 0);
    }

    #[test]
    fn empty_output_is_unchanged() {
        let out = filter_launchctl_list("");
        assert_eq!(out.text, "");
        assert_eq!(out.hidden_jobs, 0);
    }

    #[test]
    fn output_without_the_header_is_unchanged() {
        let raw = "Could not find service \"com.example.missing\" in domain for port\n";
        let out = filter_launchctl_list(raw);
        assert_eq!(out.text, raw);
        assert_eq!(out.hidden_jobs, 0);
    }

    #[test]
    fn an_unparsed_line_leaves_the_whole_output_unchanged() {
        let raw = "PID\tStatus\tLabel\n-\t0\tcom.apple.a\nsomething launchctl added later\n";
        let out = filter_launchctl_list(raw);
        assert_eq!(out.text, raw);
        assert_eq!(out.hidden_jobs, 0);
    }

    #[test]
    fn only_the_bare_list_is_filtered() {
        assert!(filters_this_invocation(&args(&["list"])));
        for other in [
            &[][..],
            &["list", "com.example.server"][..],
            &["list", "-x"][..],
            &["print", "gui/501"][..],
            &["load", "/Library/LaunchAgents/x.plist"][..],
            &["unload", "/Library/LaunchAgents/x.plist"][..],
            &["help"][..],
        ] {
            assert!(!filters_this_invocation(&args(other)), "{other:?}");
        }
    }
}
