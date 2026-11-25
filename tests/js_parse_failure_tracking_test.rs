//! A run whose output no parser tier understood is shown from both streams and
//! booked as a parse failure, never as savings; a run the parser reads is still
//! booked as savings. Each case replays what the real tool printed (captured
//! under `tests/fixtures/`) through a stand-in on PATH, then reads back the rows
//! rtk wrote.

#![cfg(unix)]

use rusqlite::Connection;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;

mod common;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

/// Writes an executable `name` in `dir` that prints `stdout` and `stderr`
/// (fixture file names, or `None` for an empty stream) and exits `code`.
fn fake_tool(dir: &Path, name: &str, stdout: Option<&str>, stderr: Option<&str>, code: i32) {
    let mut body = String::from("#!/bin/sh\n");
    if let Some(f) = stdout {
        body.push_str(&format!("cat '{FIXTURES}/{f}'\n"));
    }
    if let Some(f) = stderr {
        body.push_str(&format!("cat '{FIXTURES}/{f}' >&2\n"));
    }
    body.push_str(&format!("exit {code}\n"));
    let path = dir.join(name);
    fs::write(&path, body).expect("write fake tool");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod fake tool");
}

/// What one rtk run printed and booked.
struct Run {
    out: Output,
    /// `(rtk_cmd, input_tokens, output_tokens)` per `commands` row.
    commands: Vec<(String, i64, i64)>,
    /// `raw_command` per `parse_failures` row.
    failures: Vec<String>,
}

impl Run {
    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.out.stdout).into_owned()
    }
}

/// Runs rtk in a directory with no lockfile (so playwright resolves through
/// npx), the stand-ins in `bin` first on PATH, and a history of its own.
fn rtk_with(bin: &Path, args: &[&str]) -> Run {
    let work = common::tempdir();
    let db = work.path().join("history.db");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = common::rtk_command()
        .args(args)
        .current_dir(work.path())
        .env("PATH", path)
        .env("RTK_DB_PATH", &db)
        .output()
        .expect("run rtk");
    let conn = Connection::open(&db).expect("open history");
    let commands = conn
        .prepare("SELECT rtk_cmd, input_tokens, output_tokens FROM commands")
        .expect("prepare commands")
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("query commands")
        .collect::<Result<_, _>>()
        .expect("read commands");
    let failures = conn
        .prepare("SELECT raw_command FROM parse_failures")
        .expect("prepare parse_failures")
        .query_map([], |r| r.get(0))
        .expect("query parse_failures")
        .collect::<Result<_, _>>()
        .expect("read parse_failures");
    Run {
        out,
        commands,
        failures,
    }
}

/// The one row a parsed run books: `rtk <label>` with its saving, and no failure.
fn assert_booked_as_savings(run: &Run, label: &str) {
    assert_eq!(run.commands.len(), 1, "{:?}", run.commands);
    let (rtk_cmd, input, output) = &run.commands[0];
    assert_eq!(rtk_cmd, &format!("rtk {label}"));
    assert!(
        input > output,
        "a parsed run books its saving: {input} -> {output}"
    );
    assert!(run.failures.is_empty(), "{:?}", run.failures);
}

/// The one row a passthrough books: no tokens on either side.
fn assert_booked_as_failure(run: &Run, label: &str) {
    assert_eq!(
        run.commands,
        vec![(format!("rtk:passthrough {label}"), 0, 0)],
        "an unparsed run is not savings"
    );
    assert_eq!(run.failures, vec![label.to_string()]);
}

#[test]
fn pnpm_outdated_error_on_stderr_is_shown_not_reported_up_to_date() {
    let bin = common::tempdir();
    fake_tool(
        bin.path(),
        "pnpm",
        None,
        Some("pnpm_outdated_bad_manifest_stderr.txt"),
        1,
    );

    let run = rtk_with(bin.path(), &["pnpm", "outdated"]);
    let stdout = run.stdout();

    assert!(
        stdout.contains("ERR_PNPM_PACKAGE_MANIFEST_SERIALIZATION_ERROR"),
        "pnpm's error must reach the user, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("All packages up-to-date"),
        "a failed run must not read as up to date, got:\n{stdout}"
    );
    assert_booked_as_failure(&run, "pnpm outdated");
}

#[test]
fn pnpm_outdated_parsed_run_is_still_savings() {
    let bin = common::tempdir();
    fake_tool(bin.path(), "pnpm", Some("pnpm_outdated_raw.json"), None, 1);

    let run = rtk_with(bin.path(), &["pnpm", "outdated"]);

    assert!(run.stdout().contains("lodash"), "got:\n{}", run.stdout());
    assert_booked_as_savings(&run, "pnpm outdated");
}

#[test]
fn pnpm_list_parsed_run_is_still_savings() {
    let bin = common::tempdir();
    fake_tool(
        bin.path(),
        "pnpm",
        Some("pnpm_list_depth0_raw.json"),
        None,
        0,
    );

    let run = rtk_with(bin.path(), &["pnpm", "list"]);

    assert!(run.stdout().contains("lodash"), "got:\n{}", run.stdout());
    assert_booked_as_savings(&run, "pnpm list --depth=0");
}

/// pnpm 12 lists a project with no `name` field as a bare `path`, which the
/// list parser does not read. Should it learn to, this case needs another
/// stream the parser cannot read.
#[test]
fn pnpm_list_output_the_parser_cannot_read_is_a_parse_failure() {
    let bin = common::tempdir();
    fake_tool(
        bin.path(),
        "pnpm",
        Some("pnpm_list_no_dependencies_raw.json"),
        None,
        0,
    );

    let run = rtk_with(bin.path(), &["pnpm", "list"]);

    assert!(run.stdout().contains("\"path\""), "got:\n{}", run.stdout());
    assert_booked_as_failure(&run, "pnpm list --depth=0");
}

#[test]
fn playwright_startup_error_on_stderr_is_shown_and_not_counted() {
    let bin = common::tempdir();
    fake_tool(
        bin.path(),
        "npx",
        None,
        Some("playwright_test_missing_config_stderr.txt"),
        1,
    );

    let run = rtk_with(
        bin.path(),
        &["playwright", "test", "--config", "nope.config.js"],
    );
    let stdout = run.stdout();

    assert!(
        stdout.contains("nope.config.js does not exist"),
        "playwright's error must reach the user, got:\n{stdout}"
    );
    assert_eq!(run.out.status.code(), Some(1));
    assert_booked_as_failure(&run, "playwright test --config nope.config.js");
}

#[test]
fn vitest_startup_error_on_stderr_is_shown_and_not_counted() {
    let bin = common::tempdir();
    fake_tool(
        bin.path(),
        "vitest",
        None,
        Some("vitest_run_missing_config_stderr.txt"),
        1,
    );

    let run = rtk_with(
        bin.path(),
        &["vitest", "run", "--config", "does-not-exist.config.ts"],
    );
    let stdout = run.stdout();

    assert!(
        stdout.contains("Could not resolve \"/app/does-not-exist.config.ts\""),
        "vitest's error must reach the user, got:\n{stdout}"
    );
    assert_eq!(run.out.status.code(), Some(1));
    assert_booked_as_failure(&run, "vitest run");
}

#[test]
fn playwright_parsed_run_is_still_savings() {
    let bin = common::tempdir();
    fake_tool(
        bin.path(),
        "npx",
        Some("playwright_test_reporter_json_raw.json"),
        None,
        1,
    );

    let run = rtk_with(bin.path(), &["playwright", "test"]);

    assert!(run.stdout().contains("fails"), "got:\n{}", run.stdout());
    assert_eq!(run.out.status.code(), Some(1));
    assert_booked_as_savings(&run, "playwright test");
}

#[test]
fn vitest_parsed_run_is_still_savings() {
    let bin = common::tempdir();
    fake_tool(
        bin.path(),
        "vitest",
        Some("vitest_run_reporter_json_raw.json"),
        None,
        1,
    );

    let run = rtk_with(bin.path(), &["vitest", "run"]);

    assert!(run.stdout().contains("fails"), "got:\n{}", run.stdout());
    assert_eq!(run.out.status.code(), Some(1));
    assert_booked_as_savings(&run, "vitest run");
}
