//! Filters the progress lines of `docker compose up -d`, `down`, `stop`, `start`,
//! `restart` and `create`.
//!
//! Compose prints one `<Kind> <name>  <State>` line when an operation on a
//! container, network or volume begins (`Creating`) and another when it ends
//! (`Created`). Once the end line is there, the begin line says nothing more, so
//! it is dropped. A begin line with no end line stays: that resource is the one
//! that failed, and compose's own error text names it.
//!
//! An attached `up` streams container logs until interrupted, so it runs
//! unfiltered, as does anything this module does not know.

use crate::cmds::cloud::container::run_compose_passthrough;
use crate::core::arg_tokenizer::{
    Dialect, TokenKind, ValueSpec, before_dashdash, tokenize_grammar,
};
use crate::core::runner::{self, RunOptions};
use crate::core::shell::display_args;
use crate::core::utils::resolved_command;
use anyhow::Result;
use regex::Regex;
use std::collections::HashSet;
use std::ffi::OsString;
use std::sync::LazyLock;

/// Subcommands whose output is a bounded run of progress lines.
const PROGRESS_SUBCOMMANDS: &[&str] = &["up", "down", "stop", "start", "restart", "create"];

/// States compose prints when it starts an operation.
const IN_FLIGHT: &[&str] = &[
    "Creating",
    "Starting",
    "Stopping",
    "Removing",
    "Restarting",
    "Recreate",
    "Waiting",
];

/// States compose prints when an operation is done.
const COMPLETED: &[&str] = &[
    "Created",
    "Started",
    "Stopped",
    "Removed",
    "Recreated",
    "Healthy",
];

static PROGRESS_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*(Container|Network|Volume)\s+("?[^\s"]+"?)\s+([A-Z][a-z]+)\s*$"#).unwrap()
});

/// Flags of the progress subcommands that take a separate value, from
/// `docker compose <subcommand> --help` (v2.29).
fn takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    match (kind, name) {
        (TokenKind::Short, "t") => Some(ValueSpec::value()),
        (
            TokenKind::Long,
            "timeout" | "attach" | "no-attach" | "exit-code-from" | "pull" | "scale"
            | "wait-timeout" | "rmi",
        ) => Some(ValueSpec::value()),
        _ => None,
    }
}

/// True when `args` (the words after `docker compose`) are a progress
/// subcommand that ends on its own.
fn is_bounded_progress_command(args: &[String]) -> bool {
    let Some(subcommand) = args.first().map(String::as_str) else {
        return false;
    };
    if !PROGRESS_SUBCOMMANDS.contains(&subcommand) {
        return false;
    }
    if subcommand != "up" {
        return true;
    }

    let tokens = tokenize_grammar(&args[1..], &takes_value, Dialect::Posix);
    let has_flag = |short: Option<&str>, long: &str| {
        before_dashdash(&tokens).iter().any(|t| match t.kind {
            TokenKind::Short => short == Some(t.text),
            TokenKind::Long => t.text == long,
            _ => false,
        })
    };
    // `--wait` implies detached mode. `--watch` keeps `up` alive to sync files.
    let detached = has_flag(Some("d"), "detach") || has_flag(None, "wait");
    detached && !has_flag(Some("w"), "watch")
}

pub fn run(args: &[OsString], verbose: u8) -> Result<i32> {
    let words: Option<Vec<String>> = args
        .iter()
        .map(|arg| arg.to_str().map(str::to_owned))
        .collect();
    let Some(words) = words.filter(|w| is_bounded_progress_command(w)) else {
        return run_compose_passthrough(args, verbose);
    };

    let mut cmd = resolved_command("docker");
    cmd.arg("compose").args(&words);
    if verbose > 0 {
        eprintln!("Running: docker compose {}", words.join(" "));
    }

    let display = format!("compose {}", display_args(&words));
    let tee_label = format!("docker_compose_{}", words[0]);
    runner::run_filtered(
        cmd,
        "docker",
        &display,
        filter_progress,
        RunOptions::with_tee(&tee_label),
    )
}

/// One parsed progress line: which resource, and the state it reports.
struct Progress<'a> {
    resource: (&'a str, &'a str),
    state: &'a str,
}

fn parse_progress(line: &str) -> Option<Progress<'_>> {
    let caps = PROGRESS_LINE.captures(line)?;
    let kind = caps.get(1)?.as_str();
    let name = caps.get(2)?.as_str().trim_matches('"');
    let state = caps.get(3)?.as_str();
    Some(Progress {
        resource: (kind, name),
        state,
    })
}

/// Drop a begin line when the same resource later reports completion. Every
/// other line, including errors, is kept as compose printed it.
pub fn filter_progress(raw: &str) -> String {
    let mut completed: HashSet<(&str, &str)> = HashSet::new();
    let mut kept: Vec<&str> = Vec::new();

    // Walking backwards means "completes later" is a set lookup.
    for line in raw.lines().rev() {
        match parse_progress(line) {
            Some(p) if COMPLETED.contains(&p.state) => {
                completed.insert(p.resource);
                kept.push(line);
            }
            Some(p) if IN_FLIGHT.contains(&p.state) && completed.contains(&p.resource) => {}
            _ => kept.push(line),
        }
    }

    kept.reverse();
    kept.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const UP: &str = include_str!("../../../tests/fixtures/docker_compose_up_raw.txt");
    const DOWN: &str = include_str!("../../../tests/fixtures/docker_compose_down_raw.txt");
    const RESTART: &str = include_str!("../../../tests/fixtures/docker_compose_restart_raw.txt");
    const STOP: &str = include_str!("../../../tests/fixtures/docker_compose_stop_raw.txt");
    const HEALTHY: &str = include_str!("../../../tests/fixtures/docker_compose_up_healthy_raw.txt");
    const RECREATE: &str =
        include_str!("../../../tests/fixtures/docker_compose_up_recreate_raw.txt");
    const NOOP: &str = include_str!("../../../tests/fixtures/docker_compose_up_noop_raw.txt");
    const PORT_CONFLICT: &str =
        include_str!("../../../tests/fixtures/docker_compose_up_port_conflict_raw.txt");
    const MISSING_IMAGE: &str =
        include_str!("../../../tests/fixtures/docker_compose_up_missing_image_raw.txt");

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    fn savings_pct(raw: &str, filtered: &str) -> f64 {
        100.0 - (filtered.len() as f64 / raw.len() as f64 * 100.0)
    }

    #[test]
    fn test_up_keeps_one_line_per_resource() {
        let out = filter_progress(UP);
        assert_eq!(
            out.lines().collect::<Vec<_>>(),
            vec![
                " Network rtk1305demo_default  Created",
                " Volume \"rtk1305demo_dbdata\"  Created",
                " Container rtk1305demo-web-1  Created",
                " Container rtk1305demo-db-1  Created",
                " Container rtk1305demo-worker-1  Created",
                " Container rtk1305demo-web-1  Started",
                " Container rtk1305demo-db-1  Started",
                " Container rtk1305demo-worker-1  Started",
            ]
        );
        assert!(savings_pct(UP, &out) >= 40.0, "{out}");
    }

    #[test]
    fn test_down_and_restart_savings() {
        for (name, raw) in [("down", DOWN), ("restart", RESTART), ("stop", STOP)] {
            let out = filter_progress(raw);
            assert!(!out.contains("Stopping"), "{name}: {out}");
            assert!(!out.contains("Removing"), "{name}: {out}");
            assert!(!out.contains("Restarting"), "{name}: {out}");
            assert!(savings_pct(raw, &out) >= 20.0, "{name}: {out}");
        }
        let out = filter_progress(DOWN);
        assert!(
            out.contains("Container rtk1305demo-web-1  Removed"),
            "{out}"
        );
        assert!(
            out.contains("Network rtk1305demo_default  Removed"),
            "{out}"
        );
    }

    #[test]
    fn test_healthcheck_wait_collapses_to_healthy() {
        let out = filter_progress(HEALTHY);
        assert!(!out.contains("Waiting"), "{out}");
        assert!(out.contains("Container rtk1305re-db-1  Healthy"), "{out}");
        assert!(out.contains("Container rtk1305re-web-1  Started"), "{out}");
    }

    #[test]
    fn test_recreate_collapses_to_recreated() {
        let out = filter_progress(RECREATE);
        assert!(!out.contains("db-1  Recreate\n"), "{out}");
        assert!(out.contains("Container rtk1305re-db-1  Recreated"), "{out}");
        assert!(out.contains("Container rtk1305re-web-1  Stopped"), "{out}");
    }

    #[test]
    fn test_nothing_to_drop_is_unchanged() {
        assert_eq!(filter_progress(NOOP), NOOP.trim_end());
    }

    #[test]
    fn test_unfinished_resource_and_daemon_error_are_kept() {
        let out = filter_progress(PORT_CONFLICT);
        // web2 never reached Started: its begin line is the clue next to the error.
        assert!(
            out.contains("Container rtk1305fail2-web2-1  Starting"),
            "{out}"
        );
        assert!(
            out.contains("Bind for 0.0.0.0:8099 failed: port is already allocated"),
            "{out}"
        );
        assert!(!out.contains("web-1  Creating"), "{out}");
        assert!(savings_pct(PORT_CONFLICT, &out) >= 20.0, "{out}");
    }

    #[test]
    fn test_pull_failure_is_unchanged() {
        assert_eq!(filter_progress(MISSING_IMAGE), MISSING_IMAGE.trim_end());
    }

    #[test]
    fn test_lines_that_only_resemble_progress_are_kept() {
        let raw = "Container web  Creating\nContainer web  Error\nService web Creating\n";
        assert_eq!(filter_progress(raw), raw.trim_end());
    }

    #[test]
    fn test_bounded_commands() {
        for cmd in [
            &["up", "-d"][..],
            &["up", "--detach", "web"],
            &["up", "web", "-d"],
            &["up", "--wait"],
            &["up", "-d", "--build", "--scale", "web=2"],
            &["down"],
            &["down", "-v", "--remove-orphans"],
            &["down", "-t", "5"],
            &["stop"],
            &["start", "web"],
            &["restart"],
            &["create"],
        ] {
            assert!(is_bounded_progress_command(&args(cmd)), "{cmd:?}");
        }
    }

    #[test]
    fn test_streaming_and_unknown_commands_stay_unfiltered() {
        for cmd in [
            &["up"][..],
            &["up", "web"],
            &["up", "--abort-on-container-exit"],
            &["up", "-d", "--watch"],
            &["up", "-d", "-w"],
            // The value of --scale is not a flag.
            &["up", "--scale", "-d"],
            &["up", "--", "-d"],
            &["run", "--rm", "web"],
            &["exec", "web", "sh"],
            &["pull"],
            &["rm"],
            &["-f", "other.yaml", "up", "-d"],
            &[],
        ] {
            assert!(!is_bounded_progress_command(&args(cmd)), "{cmd:?}");
        }
    }
}
