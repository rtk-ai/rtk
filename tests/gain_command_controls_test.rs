//! By-command selection must happen before the old top-ten cap, for every format.
mod common;

use rusqlite::{Connection, params};
use serde_json::Value;
use std::path::Path;
use std::process::Output;

struct GainFixture {
    // Close SQLite before removing its directory, including on Windows.
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
        let conn = Connection::open(db).expect("open tracking database");
        Self { conn, dir }
    }

    fn insert(&self, command: &str, input: i64, output: i64, project: &Path) {
        self.conn
            .execute(
                "INSERT INTO commands (timestamp, original_cmd, rtk_cmd, project_path,
                 input_tokens, output_tokens, saved_tokens, savings_pct, exec_time_ms)
                 VALUES ('2026-10-01T12:00:00Z', ?1, ?1, ?2, ?3, ?4, ?3 - ?4, 0, 25)",
                params![command, project.to_string_lossy(), input, output],
            )
            .expect("insert tracked command");
    }

    fn run(&self, args: &[&str]) -> Output {
        common::rtk_command()
            .env("RTK_DB_PATH", self.dir.path().join("history.db"))
            .current_dir(self.dir.path())
            .arg("gain")
            .args(args)
            .output()
            .expect("run gain")
    }

    fn text(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "gain {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("UTF-8 gain output")
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut args = args.to_vec();
        args.extend(["--format", "json"]);
        serde_json::from_str(&self.text(&args)).expect("gain JSON")
    }
}

fn names(json: &Value) -> Vec<&str> {
    json["by_command"]
        .as_array()
        .expect("by_command array")
        .iter()
        .map(|row| row["command"].as_str().expect("command name"))
        .collect()
}

#[test]
fn gain_sorts_and_filters_all_groups_before_limiting() {
    let fixture = GainFixture::new();
    for i in 0..12 {
        fixture.insert(&format!("rtk large {i:02}"), 1000, 500, fixture.dir.path());
    }
    for _ in 0..20 {
        fixture.insert("rtk frequent", 2, 1, fixture.dir.path());
    }
    fixture.insert("rtk efficient", 10, 0, fixture.dir.path());

    let default = fixture.json(&[]);
    assert_eq!(names(&default).len(), 10);
    assert_eq!(names(&default)[0], "rtk large 00");
    assert_eq!(names(&default)[9], "rtk large 09");
    assert_eq!(default["summary"]["total_commands"], 33);

    assert_eq!(
        names(&fixture.json(&["--sort", "count", "--limit", "1"])),
        ["rtk frequent"]
    );
    assert_eq!(
        names(&fixture.json(&["--sort", "ratio", "--limit", "1"])),
        ["rtk efficient"]
    );
    assert_eq!(
        names(&fixture.json(&["--filter", "frequent", "--limit", "1"])),
        ["rtk frequent"]
    );
    assert_eq!(names(&fixture.json(&["--limit", "0"])).len(), 14);
    let filtered = fixture.json(&["--exclude", "large", "--limit", "1"]);
    assert_eq!(names(&filtered), ["rtk frequent"]);
    assert_eq!(filtered["summary"], default["summary"]);
}

#[test]
fn gain_literal_filters_compose_and_preserve_totals() {
    let fixture = GainFixture::new();
    for command in [
        "rtk gradle test",
        "rtk gradle build",
        "rtk Git",
        "rtk 100%_[]",
    ] {
        fixture.insert(command, 100, 10, fixture.dir.path());
    }
    let selected = fixture.json(&["--filter", "gradle", "--exclude", "test"]);
    assert_eq!(names(&selected), ["rtk gradle build"]);
    assert_eq!(selected["summary"]["total_commands"], 4);
    assert_eq!(names(&fixture.json(&["--filter", "%_[]"])), ["rtk 100%_[]"]);
    assert!(names(&fixture.json(&["--filter", "git"])).is_empty());
    assert!(names(&fixture.json(&["--filter", "' OR 1=1 --"])).is_empty());
    assert!(
        fixture
            .text(&["--filter", "absent"])
            .contains("No commands match")
    );
}

#[test]
fn gain_ratio_is_weighted_and_zero_input_and_negative_rows_are_preserved() {
    let fixture = GainFixture::new();
    fixture.insert("rtk weighted", 1000, 100, fixture.dir.path());
    fixture.insert("rtk weighted", 10, 10, fixture.dir.path());
    fixture.insert("rtk medium", 100, 40, fixture.dir.path());
    fixture.insert("rtk zero", 0, 0, fixture.dir.path());
    fixture.insert("rtk a_loss", 100, 120, fixture.dir.path());
    fixture.insert("rtk z_loss", 100, 110, fixture.dir.path());
    let descending = fixture.json(&["--sort", "ratio", "--limit", "0"]);
    assert_eq!(
        names(&descending),
        [
            "rtk weighted",
            "rtk medium",
            "rtk zero",
            "rtk z_loss",
            "rtk a_loss"
        ]
    );
    let pct = descending["by_command"][0]["savings_pct"]
        .as_f64()
        .expect("ratio");
    assert!((pct - 90000.0 / 1010.0).abs() < 0.0001);
    assert_eq!(descending["by_command"][2]["savings_pct"], 0.0);
    assert_eq!(descending["by_command"][4]["saved_tokens"], 0);
    assert_eq!(descending["by_command"][4]["savings_pct"], -20.0);
    assert_eq!(
        names(&fixture.json(&["--filter", "loss", "--limit", "0"])),
        ["rtk z_loss", "rtk a_loss"],
        "saved ordering uses signed sums before display clamps negatives"
    );
    for sort in ["saved", "ratio"] {
        assert_eq!(
            names(&fixture.json(&["--sort", sort, "--reverse", "--limit", "2"])),
            ["rtk a_loss", "rtk z_loss"]
        );
    }
}

#[test]
fn gain_command_selection_respects_project_scope() {
    let fixture = GainFixture::new();
    let project = fixture
        .dir
        .path()
        .canonicalize()
        .expect("canonical project");
    fixture.insert("rtk local", 100, 10, &project);
    fixture.insert("rtk child", 100, 10, &project.join("child"));
    fixture.insert("rtk sibling", 1000, 0, &project.with_extension("sibling"));
    assert_eq!(names(&fixture.json(&["--limit", "1"])), ["rtk sibling"]);
    let selected = fixture.json(&["--project", "--sort", "count", "--limit", "0"]);
    assert_eq!(names(&selected), ["rtk child", "rtk local"]);
    assert_eq!(selected["summary"]["total_commands"], 2);
}

#[test]
fn gain_text_json_and_csv_share_command_selection() {
    let fixture = GainFixture::new();
    for command in ["rtk alpha", "rtk beta", "rtk gamma"] {
        fixture.insert(command, 100, 10, fixture.dir.path());
    }
    let args = [
        "--sort",
        "count",
        "--reverse",
        "--exclude",
        "alpha",
        "--limit",
        "1",
    ];
    let json = fixture.json(&args);
    assert_eq!(names(&json), ["rtk beta"]);
    assert_eq!(json["by_command"][0]["count"], 1);
    assert_eq!(json["by_command"][0]["saved_tokens"], 90);
    assert_eq!(json["by_command"][0]["avg_time_ms"], 25);
    let text = fixture.text(&args);
    assert!(text.contains("rtk beta"));
    assert!(!text.contains("rtk gamma"));
    let mut csv_args = args.to_vec();
    csv_args.extend(["--format", "csv"]);
    let csv = fixture.text(&csv_args);
    assert!(csv.contains("command,count,saved_tokens,savings_pct,avg_time_ms"));
    assert!(csv.contains("rtk beta,1,90,90.00,25"));
    assert!(!csv.contains("rtk gamma"));
}

#[test]
fn gain_csv_escapes_command_names() {
    let fixture = GainFixture::new();
    fixture.insert("rtk say \"a,b\"\nnext", 100, 10, fixture.dir.path());
    let csv = fixture.text(&["--format", "csv"]);
    assert!(csv.contains("\"rtk say \"\"a,b\"\"\nnext\",1,90,90.00,25"));
}

#[test]
fn gain_empty_data_and_temporal_exports_keep_existing_behavior() {
    let fixture = GainFixture::new();
    assert!(names(&fixture.json(&[])).is_empty());
    assert!(fixture.text(&[]).contains("No tracking data yet."));
    fixture.insert("rtk git status", 100, 10, fixture.dir.path());
    for period in ["--daily", "--weekly", "--monthly", "--all"] {
        let json = fixture.json(&[period]);
        assert!(json.get("by_command").is_none());
        assert_eq!(json["summary"]["total_commands"], 1);
        assert_eq!(json["summary"]["total_saved"], 90);
        assert!(fixture.text(&[period, "--format", "csv"]).contains("Data"));
    }
}

#[test]
fn gain_rejects_invalid_or_ignored_command_controls() {
    let fixture = GainFixture::new();
    for args in [
        vec!["--sort", "unknown"],
        vec!["--limit", "-1"],
        vec!["--limit", "many"],
    ] {
        assert!(!fixture.run(&args).status.success(), "{args:?}");
    }
    for view in [
        "--daily",
        "--weekly",
        "--monthly",
        "--all",
        "--failures",
        "--recalls",
        "--reset",
    ] {
        for option in [
            vec!["--sort", "count"],
            vec!["--sort", "saved"],
            vec!["--limit", "0"],
            vec!["--limit", "10"],
            vec!["--filter", "git"],
            vec!["--exclude", "git"],
            vec!["--reverse"],
        ] {
            let mut args = option;
            args.push(view);
            assert!(!fixture.run(&args).status.success(), "{args:?}");
        }
    }
    assert!(
        fixture
            .run(&["--graph", "--history", "--sort", "count"])
            .status
            .success()
    );
}

#[test]
fn gain_unlimited_text_aligns_three_digit_ranks_and_large_limits_do_not_wrap() {
    let fixture = GainFixture::new();
    for i in 0..101 {
        fixture.insert(&format!("rtk cmd{i:03}"), 100, 10, fixture.dir.path());
    }
    let text = fixture.text(&["--limit", "0"]);
    let first = text
        .lines()
        .find(|line| line.contains("rtk cmd000"))
        .expect("first row");
    let last = text
        .lines()
        .find(|line| line.contains("rtk cmd100"))
        .expect("last row");
    assert_eq!(first.find("rtk"), last.find("rtk"));
    assert!(last.starts_with("101."));
    let large = usize::MAX.to_string();
    assert_eq!(names(&fixture.json(&["--limit", &large])).len(), 101);
}

#[test]
fn gain_mixed_and_zero_input_groups_keep_recorded_accounting() {
    let fixture = GainFixture::new();
    fixture.insert("rtk mixed", 100, 50, fixture.dir.path());
    fixture.insert("rtk mixed", 100, 200, fixture.dir.path());
    fixture.insert("rtk zero", 0, 10, fixture.dir.path());
    fixture.insert("rtk empty", 0, 0, fixture.dir.path());
    let json = fixture.json(&["--sort", "ratio", "--reverse"]);
    assert_eq!(names(&json), ["rtk mixed", "rtk empty", "rtk zero"]);
    assert_eq!(json["by_command"][0]["count"], 2);
    assert_eq!(json["by_command"][0]["saved_tokens"], 0);
    assert_eq!(json["by_command"][0]["savings_pct"], -25.0);
    assert_eq!(json["by_command"][2]["savings_pct"], 0.0);
    assert_eq!(json["summary"]["total_saved"], 50);
    assert_eq!(
        names(&fixture.json(&["--sort", "saved", "--reverse"])),
        ["rtk mixed", "rtk zero", "rtk empty"]
    );
}
