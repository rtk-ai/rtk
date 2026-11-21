//! What the PostToolUse hook books for a Bash call, read back from the savings
//! history: output an rtk command already booked is not booked a second time,
//! while output rtk never filtered keeps its row.

use rusqlite::Connection;
use serde_json::json;
use std::fs;
use std::io::Write;
use std::process::Stdio;

mod common;

const POSTHOOK: &str = "[pipeline]\nextends = \"rtk:posthook\"\n";

/// Run one Bash PostToolUse event through `rtk hook claude` under the
/// `pipeline` file, in a history of its own, and return the
/// `(input_tokens, output_tokens)` of each posthook row it booked.
fn posthook_rows(pipeline: &str, command: &str, stdout: &str) -> Vec<(i64, i64)> {
    let home = common::tempdir();
    let config = home.path().join("config");
    fs::create_dir_all(&config).expect("create the config directory");
    fs::write(config.join("pipeline.toml"), pipeline).expect("write pipeline.toml");
    let db = home.path().join("rtk.db");
    let event = json!({
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": command},
        "tool_response": {
            "stdout": stdout,
            "stderr": "",
            "interrupted": false,
            "isImage": false
        }
    });

    let mut cmd = common::rtk_command();
    cmd.args(["hook", "claude"])
        .env("RTK_DB_PATH", &db)
        .env("RTK_CONFIG_DIR", &config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn rtk hook claude");
    child
        .stdin
        .take()
        .expect("stdin pipe")
        .write_all(event.to_string().as_bytes())
        .expect("feed the event");
    let out = child.wait_with_output().expect("wait rtk hook claude");
    assert!(
        out.status.success(),
        "rtk hook claude failed for {command:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    if !db.exists() {
        return Vec::new();
    }
    let conn = Connection::open(&db).expect("open tracking db");
    let mut stmt = conn
        .prepare(
            "SELECT input_tokens, output_tokens FROM commands \
             WHERE rtk_cmd LIKE 'rtk posthook bash%'",
        )
        .expect("prepare the row query");
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query posthook rows")
        .collect::<Result<_, _>>()
        .expect("read posthook rows")
}

/// Grep-like output above the posthook's 512-byte floor that the generic
/// `term` chain leaves as it is: no ANSI, no progress, no repeated lines.
fn plain_output() -> String {
    (1..=40)
        .map(|i| format!("src/lib.rs:{i}: let value_{i} = compute(foo, {i});\n"))
        .collect()
}

/// Test-runner output whose ANSI colours the `term` chain strips.
fn coloured_output() -> String {
    (0..40)
        .map(|i| format!("\x1b[32mPASS\x1b[0m test_{i}\n"))
        .collect()
}

#[test]
fn output_of_a_command_that_ran_rtk_is_not_booked_again() {
    let booked: Vec<_> = ["rtk grep -n foo src", "cd sub && rtk grep -n foo src"]
        .into_iter()
        .map(|command| (command, posthook_rows(POSTHOOK, command, &plain_output())))
        .filter(|(_, rows)| !rows.is_empty())
        .collect();
    assert!(
        booked.is_empty(),
        "rtk grep booked this output already, posthook rows: {booked:?}"
    );
}

#[test]
fn output_rtk_never_filtered_keeps_its_coverage_row() {
    let rows = posthook_rows(POSTHOOK, "sed -n 1,80p notes.txt", &plain_output());
    assert_eq!(rows.len(), 1, "one coverage row expected: {rows:?}");
    let (input, output) = rows[0];
    assert_eq!(input, output, "nothing was removed, so nothing is saved");
}

#[test]
fn output_the_posthook_shortens_books_its_saving() {
    let rows = posthook_rows(POSTHOOK, "python3 run_tests.py", &coloured_output());
    assert_eq!(rows.len(), 1, "one row expected: {rows:?}");
    let (input, output) = rows[0];
    assert!(output < input, "stripped ANSI is a saving: {rows:?}");
}

#[test]
fn a_rule_that_shortens_rtk_output_again_books_that_saving() {
    let pipeline = format!(
        "{POSTHOOK}\n[[hook.after_tool.shell]]\nname = \"term-everything\"\nuse = [\"term\"]\n"
    );
    let rows = posthook_rows(&pipeline, "rtk grep -n PASS tests", &coloured_output());
    assert_eq!(rows.len(), 1, "one row expected: {rows:?}");
    let (input, output) = rows[0];
    assert!(output < input, "stripped ANSI is a saving: {rows:?}");
}
