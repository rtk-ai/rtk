//! Signed net savings must agree across gain's public output formats.
mod common;

use rusqlite::{Connection, params};
use serde_json::Value;

struct GainFixture {
    conn: Connection,
    dir: tempfile::TempDir,
}

impl GainFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temporary gain directory");
        let db = dir.path().join("history.db");
        let output = common::rtk_command()
            .env("RTK_DB_PATH", &db)
            .args(["gain", "--format", "json"])
            .output()
            .expect("initialize tracking database");
        assert!(output.status.success());
        Self {
            conn: Connection::open(db).expect("open database"),
            dir,
        }
    }

    fn insert(&self, command: &str, input: i64, output: i64, day: &str) {
        self.conn
            .execute(
                "INSERT INTO commands (timestamp, original_cmd, rtk_cmd, project_path,
             input_tokens, output_tokens, saved_tokens, savings_pct, exec_time_ms)
             VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?4 - ?5,
                     CASE WHEN ?4 > 0 THEN (?4 - ?5) * 100.0 / ?4 ELSE 0.0 END, 25)",
                params![
                    format!("{day}T12:00:00Z"),
                    command,
                    self.dir
                        .path()
                        .canonicalize()
                        .expect("canonical project")
                        .to_string_lossy(),
                    input,
                    output
                ],
            )
            .expect("insert command");
    }

    fn text(&self, args: &[&str]) -> String {
        let output = common::rtk_command()
            .env("RTK_DB_PATH", self.dir.path().join("history.db"))
            .current_dir(self.dir.path())
            .arg("gain")
            .args(args)
            .output()
            .expect("run gain");
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("UTF-8 output")
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut args = args.to_vec();
        args.extend(["--format", "json"]);
        serde_json::from_str(&self.text(&args)).expect("gain JSON")
    }
}

#[test]
fn mixed_savings_agree_in_summary_periods_exports_and_project_scope() {
    let fixture = GainFixture::new();
    fixture.insert("rtk mixed", 100, 50, "2026-10-01");
    fixture.insert("rtk mixed", 100, 200, "2026-10-01");
    for scope in [vec![], vec!["--project"]] {
        let mut args = scope.clone();
        args.push("--all");
        let json = fixture.json(&args);
        assert_eq!(json["summary"]["total_saved"], -50);
        assert_eq!(json["summary"]["avg_savings_pct"], -25.0);
        for period in ["daily", "weekly", "monthly"] {
            assert_eq!(json[period][0]["saved_tokens"], -50);
            assert_eq!(json[period][0]["savings_pct"], -25.0);
        }
        let table = fixture.text(&args);
        assert_eq!(
            table
                .lines()
                .filter(|l| l.starts_with("TOTAL") && l.contains("-50") && l.contains("-25.0%"))
                .count(),
            3,
            "{table}"
        );
        args.extend(["--format", "csv"]);
        let csv = fixture.text(&args);
        assert_eq!(csv.matches(",200,250,-50,-25.00,").count(), 3, "{csv}");
        let summary = fixture.text(&scope);
        assert!(summary.contains("-50 (-25.0%)"), "{summary}");
        assert!(
            summary
                .lines()
                .any(|l| l.contains("rtk mixed") && l.contains("-50") && l.contains("-25.0%")),
            "{summary}"
        );
    }
}

#[test]
fn history_and_graph_show_losses_without_double_signs() {
    let fixture = GainFixture::new();
    fixture.insert("rtk saving", 100, 50, "2026-10-01");
    fixture.insert("rtk err missing", 16, 26, "2026-10-02");
    let text = fixture.text(&["--history", "--graph"]);
    let history = text.split("Recent Commands").nth(1).expect("history");
    assert!(history.contains("-62% (-10)"), "{text}");
    assert!(history.contains("50% (50)"), "{text}");
    assert!(!history.contains("--62%"), "{text}");
    let graph = text
        .split("Daily Savings")
        .nth(1)
        .expect("graph")
        .split("Recent Commands")
        .next()
        .expect("graph only");
    let loss = graph
        .lines()
        .find(|l| l.starts_with("10-02"))
        .expect("loss row");
    let saving = graph
        .lines()
        .find(|l| l.starts_with("10-01"))
        .expect("saving row");
    let (loss_left, loss_right) = loss.split_once('│').expect("loss zero axis");
    assert!(
        loss_left.contains('█') && !loss_right.contains('█'),
        "{graph}"
    );
    assert!(loss_right.ends_with("-10"), "{graph}");
    let (saving_left, saving_right) = saving.split_once('│').expect("saving zero axis");
    assert!(
        !saving_left.contains('█') && saving_right.contains('█'),
        "{graph}"
    );
}

#[test]
fn zero_input_retains_signed_tokens_with_finite_zero_percentage() {
    let fixture = GainFixture::new();
    fixture.insert("rtk empty", 0, 0, "2026-10-01");
    fixture.insert("rtk expansion", 0, 20, "2026-10-01");
    let json = fixture.json(&["--all"]);
    assert_eq!(json["summary"]["total_saved"], -20);
    assert_eq!(json["summary"]["avg_savings_pct"], 0.0);
    for period in ["daily", "weekly", "monthly"] {
        assert_eq!(json[period][0]["saved_tokens"], -20);
        assert_eq!(json[period][0]["savings_pct"], 0.0);
    }
    let text = fixture.text(&["--graph", "--history"]);
    assert!(text.contains("0% (-20)"), "{text}");
    assert!(!text.contains("NaN") && !text.contains("inf"), "{text}");
}

#[test]
fn negative_quota_is_reported_as_net_loss() {
    let fixture = GainFixture::new();
    fixture.insert("rtk regression", 6_000_000, 9_000_000, "2026-10-01");
    let text = fixture.text(&["--quota", "--tier", "pro"]);
    assert!(text.contains("-3.0M (-50.0%)"), "{text}");
    assert!(
        text.lines()
            .any(|l| l.contains("Quota preserved") && l.contains("-50.0%")),
        "{text}"
    );
}

#[test]
fn regressions_remain_visible_beyond_top_ten_savers_in_loss_order() {
    let fixture = GainFixture::new();
    for n in 0..12 {
        fixture.insert(&format!("rtk saver {n}"), 100, 0, "2026-10-01");
    }
    fixture.insert("rtk small loss", 100, 125, "2026-10-01");
    fixture.insert("rtk large loss", 100, 300, "2026-10-01");
    let text = fixture.text(&[]);
    let regressions = text
        .split("Regressions")
        .nth(1)
        .expect("regressions section");
    let large = regressions
        .find("rtk large loss")
        .expect("largest loss visible");
    let small = regressions
        .find("rtk small loss")
        .expect("smaller loss visible");
    assert!(large < small, "{regressions}");
    assert!(
        regressions.contains("-200") && regressions.contains("-25"),
        "{regressions}"
    );
}

#[test]
fn temporal_totals_include_both_saving_and_regressing_days() {
    let fixture = GainFixture::new();
    fixture.insert("rtk mixed", 100, 50, "2026-10-01");
    fixture.insert("rtk mixed", 100, 200, "2026-10-02");
    let json = fixture.json(&["--all"]);
    assert_eq!(json["daily"][0]["saved_tokens"], 50);
    assert_eq!(json["daily"][1]["saved_tokens"], -100);
    assert_eq!(json["weekly"][0]["saved_tokens"], -50);
    assert_eq!(json["monthly"][0]["saved_tokens"], -50);
    let text = fixture.text(&["--all"]);
    assert_eq!(
        text.lines()
            .filter(|l| l.starts_with("TOTAL") && l.contains("-50") && l.contains("-25.0%"))
            .count(),
        3,
        "{text}"
    );
}

#[test]
fn existing_unsigned_era_database_opens_without_rewriting_history() {
    let dir = tempfile::tempdir().expect("legacy database directory");
    let db = dir.path().join("history.db");
    let conn = Connection::open(&db).expect("legacy database");
    conn.execute_batch("CREATE TABLE commands (
        id INTEGER PRIMARY KEY, timestamp TEXT NOT NULL, original_cmd TEXT NOT NULL,
        rtk_cmd TEXT NOT NULL, input_tokens INTEGER NOT NULL, output_tokens INTEGER NOT NULL,
        saved_tokens INTEGER NOT NULL, savings_pct REAL NOT NULL);
        INSERT INTO commands VALUES (1, '2026-10-01T12:00:00Z', 'cmd', 'rtk saving', 100, 50, 50, 50.0);
        INSERT INTO commands VALUES (2, '2026-10-01T12:00:00Z', 'cmd', 'rtk old clamped', 100, 150, 0, 0.0);")
        .expect("create legacy rows");
    let output = common::rtk_command()
        .env("RTK_DB_PATH", &db)
        .args(["gain", "--all", "--format", "json"])
        .output()
        .expect("read legacy database");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).expect("legacy JSON");
    assert_eq!(json["summary"]["total_saved"], 50);
    assert_eq!(json["daily"][0]["saved_tokens"], 50);
    let row: (i64, f64) = conn
        .query_row(
            "SELECT saved_tokens, savings_pct FROM commands WHERE id = 2",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("unchanged row");
    assert_eq!(row, (0, 0.0));
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .expect("schema version");
    assert_eq!(version, 1);
}

#[test]
fn project_regressions_exclude_outside_losses_and_preserve_tie_order() {
    let fixture = GainFixture::new();
    fixture.insert("rtk loss z", 100, 150, "2026-10-01");
    fixture.insert("rtk loss a", 100, 150, "2026-10-01");
    fixture.insert("rtk outside", 100, 2000, "2026-10-01");
    fixture
        .conn
        .execute(
            "UPDATE commands SET project_path = ?1 WHERE rtk_cmd = 'rtk outside'",
            params![
                fixture
                    .dir
                    .path()
                    .parent()
                    .expect("parent directory")
                    .join("outside")
                    .to_string_lossy()
            ],
        )
        .expect("outside project");
    let global = fixture.text(&[]);
    assert!(global.contains("rtk outside"));
    let scoped = fixture.text(&["--project"]);
    assert!(!scoped.contains("rtk outside"), "{scoped}");
    assert!(scoped.contains("-100 (-50.0%)"), "{scoped}");
    let regressions = scoped.split("Regressions").nth(1).expect("regressions");
    assert!(
        regressions.find("rtk loss a").expect("loss a")
            < regressions.find("rtk loss z").expect("loss z"),
        "{regressions}"
    );
}

#[test]
fn positive_only_and_empty_data_keep_existing_output_shape() {
    let fixture = GainFixture::new();
    assert!(fixture.text(&[]).contains("No tracking data yet"));
    assert_eq!(fixture.json(&[])["summary"]["total_saved"], 0);
    fixture.insert("rtk saver", 100, 25, "2026-10-01");
    let text = fixture.text(&["--graph", "--history"]);
    assert!(text.contains("75 (75.0%)"), "{text}");
    assert!(!text.contains("Regressions"), "{text}");
    assert!(
        text.lines()
            .any(|l| l.starts_with("10-01 │█") && l.ends_with("75")),
        "{text}"
    );
}
