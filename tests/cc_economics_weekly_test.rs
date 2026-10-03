//! Exercise the compiled CLI against a real, isolated tracker database and a
//! deterministic ccusage subprocess. The fake-only PATH prevents npx, network
//! access, and reads of the contributor's real Claude usage/configuration.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

use rusqlite::{Connection, params};
use serde_json::{Value, json};

mod common;

const INITIAL_ARGS: &str = "<weekly><--json><--since><20250101>\n";
const RETRY_ARGS: &str = "<weekly><--json><--since><20250101><--start-of-week><monday>\n";
const CSV_HEADER: &str = "period,spent,input_tokens,output_tokens,cache_create,cache_read,active_tokens,total_tokens,saved_tokens,weighted_savings,active_savings,blended_savings,rtk_commands\n";

struct Sandbox {
    root: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("create isolated economics test directory");
        for dir in ["bin", "home", "config", "data", "cwd"] {
            fs::create_dir(root.path().join(dir)).expect("create test subdirectory");
        }
        let sandbox = Self { root };
        // Let the compiled CLI create the production schema, rather than
        // duplicating it in this test or borrowing a real user's database.
        let init = sandbox
            .command()
            .args(["gain", "--format", "json"])
            .output()
            .expect("initialize tracker through rtk gain");
        assert_success(&init);
        assert!(sandbox.root.path().join("data/history.db").is_file());
        sandbox
    }

    fn command(&self) -> Command {
        let root = self.root.path();
        let mut cmd = common::rtk_command();
        cmd.current_dir(root.join("cwd"))
            .env("HOME", root.join("home"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("RTK_DB_PATH", root.join("data/history.db"))
            .env("RTK_TEE_DIR", root.join("data/tee"))
            .env("RTK_RECALL_DB", root.join("data/recall.db"))
            .env("CCUSAGE_TEST_ROOT", root)
            // Do not append the host PATH: even the missing-tool case must
            // never launch a real ccusage or npx installation.
            .env("PATH", root.join("bin"))
            .env("NO_COLOR", "1")
            .env("LC_ALL", "C");
        cmd
    }

    fn seed(&self, rows: &[(&str, u64)]) {
        let conn = Connection::open(self.root.path().join("data/history.db"))
            .expect("open isolated production tracker schema");
        for &(date, multiplier) in rows {
            conn.execute(
                "INSERT INTO commands
                 (timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens,
                  saved_tokens, savings_pct, exec_time_ms, project_path)
                 VALUES (?1, 'synthetic command', 'rtk synthetic', ?2, ?3, ?4,
                         80.0, 5, '')",
                params![
                    format!("{date}T12:00:00Z"),
                    1000 * multiplier,
                    200 * multiplier,
                    800 * multiplier
                ],
            )
            .expect("seed isolated tracker row");
        }
    }

    fn stub(&self, responses: &[(&str, i32)]) {
        // All commands in this script are /bin/sh builtins. Logging each argv
        // separately also catches extra probes, daily fetches and third retries.
        let mut script = String::from(
            "#!/bin/sh\n\
             set -eu\n\
             printf '<%s>' \"$@\" >> \"$CCUSAGE_TEST_ROOT/calls\"\n\
             printf '\\n' >> \"$CCUSAGE_TEST_ROOT/calls\"\n\
             count=0\n\
             if [ -f \"$CCUSAGE_TEST_ROOT/count\" ]; then\n\
                 read -r count < \"$CCUSAGE_TEST_ROOT/count\"\n\
             fi\n\
             count=$((count + 1))\n\
             printf '%s\\n' \"$count\" > \"$CCUSAGE_TEST_ROOT/count\"\n\
             case \"$count\" in\n",
        );
        for (index, &(body, status)) in responses.iter().enumerate() {
            let quoted = body.replace('\'', "'\\''");
            let redirect = if status == 0 { "" } else { " >&2" };
            script.push_str(&format!(
                "{}) printf '%s\\n' '{}'{}; exit {} ;;\n",
                index + 1,
                quoted,
                redirect,
                status
            ));
        }
        script.push_str("*) printf 'unexpected ccusage invocation\\n' >&2; exit 99 ;;\nesac\n");
        let path = self.root.path().join("bin/ccusage");
        fs::write(&path, script).expect("write fake ccusage");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("make fake ccusage executable");
    }

    fn run(&self, format: &str) -> Output {
        self.command()
            .args(["cc-economics", "--weekly", "--format", format])
            .output()
            .expect("run compiled cc-economics CLI")
    }

    fn assert_calls(&self, count: usize) {
        let path = self.root.path().join("calls");
        let actual = if path.exists() {
            fs::read_to_string(path).expect("read fake ccusage argv log")
        } else {
            String::new()
        };
        let expected = match count {
            0 => String::new(),
            1 => INITIAL_ARGS.to_string(),
            2 => format!("{INITIAL_ARGS}{RETRY_ARGS}"),
            _ => panic!("weekly fetch must never need more than two calls"),
        };
        assert_eq!(actual, expected, "ccusage argv and call count");
    }
}

fn fixture(key: &str, rows: &[(&str, u64)]) -> String {
    let weekly: Vec<_> = rows
        .iter()
        .map(|&(date, multiplier)| {
            json!({
                (key): date,
                "inputTokens": 100 * multiplier,
                "outputTokens": 20 * multiplier,
                "cacheCreationTokens": 40 * multiplier,
                "cacheReadTokens": 200 * multiplier,
                "totalTokens": 360 * multiplier,
                "totalCost": 2.7 * multiplier as f64
            })
        })
        .collect();
    json!({"weekly": weekly}).to_string()
}

fn assert_success(out: &Output) {
    assert!(
        out.status.success(),
        "CLI failed: status={}\nstdout={}\nstderr={}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn weekly_rows(out: &Output) -> Vec<Value> {
    assert_success(out);
    let report: Value = serde_json::from_slice(&out.stdout).expect("valid economics JSON");
    assert!(report["daily"].is_null());
    assert!(report["monthly"].is_null());
    assert!(report["totals"].is_null());
    report["weekly"].as_array().expect("weekly rows").clone()
}

fn assert_metric(row: &Value, key: &str, expected: f64) {
    let actual = row[key]
        .as_f64()
        .unwrap_or_else(|| panic!("missing numeric metric {key}: {row}"));
    assert!(
        (actual - expected).abs() < 1e-10,
        "{key}: expected {expected}, got {actual}; row={row}"
    );
}

fn assert_joined(row: &Value, label: &str, cc: u64, rtk: u64, commands: u64) {
    assert_eq!(row.as_object().expect("period object").len(), 17);
    assert_eq!(row["label"], label);
    for (key, expected) in [
        ("cc_input_tokens", 100 * cc),
        ("cc_output_tokens", 20 * cc),
        ("cc_cache_create_tokens", 40 * cc),
        ("cc_cache_read_tokens", 200 * cc),
        ("cc_active_tokens", 120 * cc),
        ("cc_total_tokens", 360 * cc),
        ("rtk_commands", commands),
        ("rtk_saved_tokens", 800 * rtk),
    ] {
        assert_eq!(row[key], expected, "{key}: {row}");
    }
    for (key, expected) in [
        ("cc_cost", 2.7 * cc as f64),
        ("rtk_savings_pct", 80.0),
        ("weighted_input_cpt", 0.01),
        ("savings_weighted", 8.0 * rtk as f64),
        ("blended_cpt", 0.0075),
        ("active_cpt", 0.0225),
        ("savings_blended", 6.0 * rtk as f64),
        ("savings_active", 18.0 * rtk as f64),
    ] {
        assert_metric(row, key, expected);
    }
}

fn assert_rtk_only(out: &Output) {
    assert_eq!(
        weekly_rows(out),
        vec![json!({
            "label": "2026-09-28",
            "cc_cost": null,
            "cc_total_tokens": null,
            "cc_active_tokens": null,
            "cc_input_tokens": null,
            "cc_output_tokens": null,
            "cc_cache_create_tokens": null,
            "cc_cache_read_tokens": null,
            "rtk_commands": 1,
            "rtk_saved_tokens": 800,
            "rtk_savings_pct": 80.0,
            "weighted_input_cpt": null,
            "savings_weighted": null,
            "blended_cpt": null,
            "active_cpt": null,
            "savings_blended": null,
            "savings_active": null
        })]
    );
}

fn assert_fetch_error(out: &Output) {
    assert!(!out.status.success(), "malformed weekly data must fail");
    assert!(out.stdout.is_empty(), "must not emit a partial JSON report");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ccusage"),
        "missing fetch context: {stderr}"
    );
}

fn monday_join(key: &str, date: &str) {
    let sandbox = Sandbox::new();
    sandbox.seed(&[("2026-09-28", 1)]);
    let monday = fixture(key, &[(date, 1)]);
    sandbox.stub(&[(&monday, 0)]);
    let rows = weekly_rows(&sandbox.run("json"));
    sandbox.assert_calls(1);
    assert_eq!(rows.len(), 1, "same Monday must produce one joined row");
    assert_joined(&rows[0], "2026-09-28", 1, 1, 1);
}

#[test]
fn legacy_week_key_joins_the_tracker_monday_without_retry() {
    monday_join("week", "2026-09-28");
}

#[test]
fn current_period_key_joins_the_tracker_monday_without_retry() {
    monday_join("period", "2026-09-28");
}

#[test]
fn monday_dates_are_canonicalized_without_rebucketing() {
    monday_join("period", "2026-9-28");
}

#[test]
fn csv_contains_exactly_one_complete_joined_row() {
    let sandbox = Sandbox::new();
    sandbox.seed(&[("2026-09-28", 1)]);
    let monday = fixture("period", &[("2026-09-28", 1)]);
    sandbox.stub(&[(&monday, 0)]);
    let out = sandbox.run("csv");
    assert_success(&out);
    sandbox.assert_calls(1);
    assert_eq!(
        String::from_utf8(out.stdout).expect("UTF-8 CSV"),
        format!(
            "{CSV_HEADER}2026-09-28,2.7000,100,20,40,200,120,360,800,8.0000,18.0000,6.0000,1\n"
        )
    );
}

#[test]
fn text_contains_one_monday_row_with_spending_and_savings() {
    let sandbox = Sandbox::new();
    sandbox.seed(&[("2026-09-28", 1)]);
    let monday = fixture("week", &[("2026-09-28", 1)]);
    sandbox.stub(&[(&monday, 0)]);
    let out = sandbox.run("text");
    assert_success(&out);
    sandbox.assert_calls(1);
    let stdout = String::from_utf8(out.stdout).expect("UTF-8 text");
    assert!(stdout.contains("Weekly Economics"));
    let rows: Vec<_> = stdout
        .lines()
        .filter(|line| line.starts_with("2026-"))
        .collect();
    assert_eq!(rows.len(), 1, "same Monday must not split: {stdout}");
    assert_eq!(
        rows[0].split_whitespace().collect::<Vec<_>>(),
        ["2026-09-28", "$2.70", "800", "$8.00", "1"]
    );
}

#[test]
fn sunday_buckets_are_replaced_by_genuinely_regrouped_monday_buckets() {
    let sandbox = Sandbox::new();
    // Daily contributions 1, 3, 2, 4 produce Sunday buckets 4, 6 but
    // Monday buckets 1, 5, 4. Relabeling or adding either response is wrong.
    sandbox.seed(&[
        ("2026-09-27", 1),
        ("2026-09-28", 3),
        ("2026-10-04", 2),
        ("2026-10-05", 4),
    ]);
    let sunday = fixture("period", &[("2026-09-27", 4), ("2026-10-04", 6)]);
    let monday = fixture(
        "period",
        &[("2026-09-21", 1), ("2026-09-28", 5), ("2026-10-05", 4)],
    );
    sandbox.stub(&[(&sunday, 0), (&monday, 0)]);
    let rows = weekly_rows(&sandbox.run("json"));
    sandbox.assert_calls(2);
    assert_eq!(rows.len(), 3, "the first response must be discarded");
    assert_joined(&rows[0], "2026-09-21", 1, 1, 1);
    assert_joined(&rows[1], "2026-09-28", 5, 5, 2);
    assert_joined(&rows[2], "2026-10-05", 4, 4, 1);
}

#[test]
fn mixed_week_starts_replace_even_initial_monday_rows() {
    let sandbox = Sandbox::new();
    sandbox.seed(&[("2026-09-28", 1)]);
    let mixed = fixture("week", &[("2026-09-28", 99), ("2026-10-04", 100)]);
    let monday = fixture("week", &[("2026-09-28", 1)]);
    sandbox.stub(&[(&mixed, 0), (&monday, 0)]);
    let rows = weekly_rows(&sandbox.run("json"));
    sandbox.assert_calls(2);
    assert_eq!(rows.len(), 1, "discard the entire mixed response");
    assert_joined(&rows[0], "2026-09-28", 1, 1, 1);
}

#[test]
fn any_valid_non_monday_start_requests_a_monday_retry() {
    let sandbox = Sandbox::new();
    sandbox.seed(&[("2026-09-28", 1)]);
    let wednesday = fixture("period", &[("2026-09-30", 7)]);
    let monday = fixture("period", &[("2026-9-28", 1)]);
    sandbox.stub(&[(&wednesday, 0), (&monday, 0)]);
    let rows = weekly_rows(&sandbox.run("json"));
    sandbox.assert_calls(2);
    assert_eq!(rows.len(), 1);
    assert_joined(&rows[0], "2026-09-28", 1, 1, 1);
}

#[test]
fn ignored_monday_flag_errors_after_exactly_two_calls() {
    let sandbox = Sandbox::new();
    sandbox.seed(&[("2026-09-28", 1)]);
    let sunday = fixture("period", &[("2026-09-27", 4)]);
    sandbox.stub(&[(&sunday, 0), (&sunday, 0)]);
    let out = sandbox.run("json");
    sandbox.assert_calls(2);
    assert_fetch_error(&out);
}

#[test]
fn initially_empty_weekly_data_does_not_retry() {
    let sandbox = Sandbox::new();
    sandbox.seed(&[("2026-09-28", 1)]);
    sandbox.stub(&[(r#"{"weekly":[]}"#, 0)]);
    let out = sandbox.run("json");
    sandbox.assert_calls(1);
    assert_rtk_only(&out);
}

#[test]
fn missing_ccusage_and_npx_preserve_rtk_only_reporting() {
    let sandbox = Sandbox::new();
    sandbox.seed(&[("2026-09-28", 1)]);
    let out = sandbox.run("json");
    sandbox.assert_calls(0);
    assert_rtk_only(&out);
    assert!(String::from_utf8_lossy(&out.stderr).contains("[warn] ccusage not found"));
}

#[test]
fn failed_initial_subprocess_preserves_rtk_only_reporting() {
    let sandbox = Sandbox::new();
    sandbox.seed(&[("2026-09-28", 1)]);
    sandbox.stub(&[("synthetic subprocess failure", 17)]);
    let out = sandbox.run("json");
    sandbox.assert_calls(1);
    assert_rtk_only(&out);
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("[warn] ccusage exited with 17: synthetic subprocess failure")
    );
}

#[test]
fn failed_retry_discards_initial_spending_and_preserves_rtk_only_reporting() {
    let sandbox = Sandbox::new();
    sandbox.seed(&[("2026-09-28", 1)]);
    let sunday = fixture("period", &[("2026-09-27", 99)]);
    sandbox.stub(&[(&sunday, 0), ("synthetic retry failure", 23)]);
    let out = sandbox.run("json");
    sandbox.assert_calls(2);
    assert_rtk_only(&out);
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("[warn] ccusage exited with 23: synthetic retry failure")
    );
}

#[test]
fn empty_retry_discards_initial_spending_without_a_third_call() {
    let sandbox = Sandbox::new();
    sandbox.seed(&[("2026-09-28", 1)]);
    let sunday = fixture("period", &[("2026-09-27", 99)]);
    sandbox.stub(&[(&sunday, 0), (r#"{"weekly":[]}"#, 0)]);
    let out = sandbox.run("json");
    sandbox.assert_calls(2);
    assert_rtk_only(&out);
}

#[test]
fn invalid_initial_date_after_a_sunday_errors_without_retry() {
    let sandbox = Sandbox::new();
    sandbox.seed(&[("2026-09-28", 1)]);
    let invalid = fixture("period", &[("2026-09-27", 4), ("2026-02-30", 6)]);
    sandbox.stub(&[(&invalid, 0)]);
    let out = sandbox.run("json");
    sandbox.assert_calls(1);
    assert_fetch_error(&out);
    assert!(String::from_utf8_lossy(&out.stderr).contains("2026-02-30"));
}

#[test]
fn invalid_retry_date_errors_without_a_third_call() {
    let sandbox = Sandbox::new();
    sandbox.seed(&[("2026-09-28", 1)]);
    let sunday = fixture("week", &[("2026-09-27", 4)]);
    let invalid = fixture("week", &[("2026-09-28", 1), ("not-a-date", 6)]);
    sandbox.stub(&[(&sunday, 0), (&invalid, 0)]);
    let out = sandbox.run("json");
    sandbox.assert_calls(2);
    assert_fetch_error(&out);
    assert!(String::from_utf8_lossy(&out.stderr).contains("not-a-date"));
}

#[test]
fn malformed_initial_json_errors_without_retry() {
    let sandbox = Sandbox::new();
    sandbox.seed(&[("2026-09-28", 1)]);
    sandbox.stub(&[(r#"{"weekly":[{"period":"2026-09-27"},"#, 0)]);
    let out = sandbox.run("json");
    sandbox.assert_calls(1);
    assert_fetch_error(&out);
}

#[test]
fn malformed_retry_json_errors_without_a_third_call() {
    let sandbox = Sandbox::new();
    sandbox.seed(&[("2026-09-28", 1)]);
    let sunday = fixture("period", &[("2026-09-27", 4)]);
    sandbox.stub(&[(&sunday, 0), ("invalid JSON", 0)]);
    let out = sandbox.run("json");
    sandbox.assert_calls(2);
    assert_fetch_error(&out);
}

fn duplicated_monday_fixture(key: &str) -> String {
    // Legacy groupByProject output can repeat a date without retaining any
    // project identity. totalTokens is deliberately independent of the four
    // category counters, so aggregation must preserve all five input fields.
    json!({"weekly": [
        {
            (key): "2026-09-28",
            "inputTokens": 101,
            "outputTokens": 23,
            "cacheCreationTokens": 47,
            "cacheReadTokens": 211,
            "totalTokens": 999,
            "totalCost": 11.0
        },
        {
            (key): "2026-09-28",
            "inputTokens": 307,
            "outputTokens": 59,
            "cacheCreationTokens": 73,
            "cacheReadTokens": 419,
            "totalTokens": 2001,
            "totalCost": 29.0
        }
    ]})
    .to_string()
}

fn assert_duplicate_mondays_are_summed(key: &str, retry: bool) {
    let sandbox = Sandbox::new();
    sandbox.seed(&[("2026-09-28", 1)]);
    let duplicates = duplicated_monday_fixture(key);
    let sunday = fixture(key, &[("2026-09-27", 99)]);
    if retry {
        sandbox.stub(&[(&sunday, 0), (&duplicates, 0)]);
    } else {
        sandbox.stub(&[(&duplicates, 0)]);
    }
    let rows = weekly_rows(&sandbox.run("json"));
    sandbox.assert_calls(if retry { 2 } else { 1 });
    assert_eq!(rows.len(), 1, "duplicate Mondays belong to one joined row");
    let row = &rows[0];
    assert_eq!(row.as_object().expect("period object").len(), 17);
    assert_eq!(row["label"], "2026-09-28");
    for (key, expected) in [
        ("cc_input_tokens", 408),
        ("cc_output_tokens", 82),
        ("cc_cache_create_tokens", 120),
        ("cc_cache_read_tokens", 630),
        ("cc_active_tokens", 490),
        ("cc_total_tokens", 3000),
        ("rtk_commands", 1),
        ("rtk_saved_tokens", 800),
    ] {
        assert_eq!(row[key], expected, "{key}: {row}");
    }
    for (key, expected) in [
        ("cc_cost", 40.0),
        ("rtk_savings_pct", 80.0),
        ("weighted_input_cpt", 40.0 / 1031.0),
        ("savings_weighted", 800.0 * 40.0 / 1031.0),
        ("blended_cpt", 40.0 / 3000.0),
        ("active_cpt", 40.0 / 490.0),
        ("savings_blended", 800.0 * 40.0 / 3000.0),
        ("savings_active", 800.0 * 40.0 / 490.0),
    ] {
        assert_metric(row, key, expected);
    }
}

#[test]
fn duplicate_legacy_week_rows_sum_every_metric() {
    assert_duplicate_mondays_are_summed("week", false);
}

#[test]
fn duplicate_current_period_rows_sum_every_metric() {
    assert_duplicate_mondays_are_summed("period", false);
}

#[test]
fn duplicate_monday_retry_rows_sum_without_initial_spending() {
    assert_duplicate_mondays_are_summed("period", true);
}
