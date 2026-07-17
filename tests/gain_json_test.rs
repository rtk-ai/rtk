//! `rtk gain --format json` is the scripting surface (#2764): it must stay valid
//! JSON on stdout, keep token counts as integers rather than the humanized "9.9M"
//! strings the text report shows, and carry enough context (scope, version) for a
//! consumer to interpret and version-guard the payload.

use std::process::Command;

/// Record one tracked command so the report has data, then read it back as JSON.
fn gain_json(extra: &[&str]) -> serde_json::Value {
    let db = tempfile::tempdir().unwrap();
    let db_path = db.path().join("t.db");

    let seed = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["grep", "-c", "fn", file!()])
        .env("RTK_DB_PATH", &db_path)
        .output()
        .expect("rtk grep");
    assert!(seed.status.success(), "seeding command failed");

    let mut args = vec!["gain", "--format", "json"];
    args.extend_from_slice(extra);
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(&args)
        .env("RTK_DB_PATH", &db_path)
        .output()
        .expect("rtk gain");

    assert!(
        out.stderr.is_empty(),
        "stdout must be the only channel: stderr had {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout was not valid JSON ({e}): {:?}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

#[test]
fn json_export_carries_scope_version_and_breakdown() {
    let v = gain_json(&[]);

    assert_eq!(v["scope"], "global");
    assert_eq!(
        v["rtk_version"],
        env!("CARGO_PKG_VERSION"),
        "consumers version-guard on this"
    );
    assert!(
        v["generated_at"].as_str().is_some_and(|s| s.ends_with('Z')),
        "generated_at should be RFC3339 UTC, got {:?}",
        v["generated_at"]
    );
    assert!(v["by_command"].is_array(), "per-command breakdown missing");
}

#[test]
fn json_export_keeps_token_counts_numeric() {
    let v = gain_json(&[]);

    // The text report humanizes these ("9.9M"); the JSON must not.
    for field in [
        "total_commands",
        "total_input",
        "total_output",
        "total_saved",
    ] {
        assert!(
            v["summary"][field].is_number(),
            "summary.{field} must be a number, got {:?}",
            v["summary"][field]
        );
    }
}

#[test]
fn json_export_reports_project_scope() {
    let v = gain_json(&["--project"]);

    // --project scopes the report, so the payload must say so rather than
    // claiming to be global data. Assert the field is a real string first: a
    // missing key is Null, which would sail past a bare `!= "global"`.
    let scope = v["scope"]
        .as_str()
        .unwrap_or_else(|| panic!("scope missing from payload: {v}"));
    assert_ne!(scope, "global", "--project run reported itself as global");
}
