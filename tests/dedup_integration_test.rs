//! End-to-end proof of session-level output dedup (src/core/dedup.rs).
//!
//! Drives the real binary over the stdin read path with `RTK_DEDUP=1` and an
//! isolated `RTK_DB_PATH`, exercising the full chain: session resolution ->
//! config/env gate -> ledger lookup/insert/bump -> stub emission.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// A body large enough to clear the 200-token min gate (~4 chars/token).
fn big_input() -> String {
    (0..80)
        .map(|i| format!("line number {i} with some representative content here\n"))
        .collect()
}

fn fresh_db(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("rtk_dedup_it_{name}.db"));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}

/// Run `rtk read -` with dedup forced on and an isolated DB. `session` is the
/// optional session id. Returns stdout.
fn rtk_read(db: &PathBuf, session: Option<&str>, input: &str) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rtk"));
    cmd.args(["read", "-"])
        .env("RTK_DEDUP", "1")
        .env("RTK_DB_PATH", db)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(s) = session {
        cmd.env("RTK_SESSION_ID", s);
    } else {
        cmd.env_remove("RTK_SESSION_ID");
    }
    let mut child = cmd.spawn().expect("spawn rtk");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait rtk");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn suppresses_identical_reread_within_session() {
    let db = fresh_db("within_session");
    let input = big_input();

    let first = rtk_read(&db, Some("sess-A"), &input);
    assert!(
        !first.contains("rtk-dedup:"),
        "first emission must be full, got stub: {first}"
    );
    assert!(
        first.contains("line number 79"),
        "first should show content"
    );

    let second = rtk_read(&db, Some("sess-A"), &input);
    assert!(
        second.contains("rtk-dedup:"),
        "identical re-read must be suppressed to a stub, got: {second}"
    );
    assert!(
        second.contains("force full: rtk proxy"),
        "stub must carry the recovery command"
    );
}

#[test]
fn no_session_is_never_suppressed() {
    let db = fresh_db("no_session");
    let input = big_input();

    // RTK_DEDUP=1 but no session id -> always full, even on repeat.
    let first = rtk_read(&db, None, &input);
    let second = rtk_read(&db, None, &input);
    assert!(!first.contains("rtk-dedup:"));
    assert!(
        !second.contains("rtk-dedup:"),
        "no session id must disable suppression entirely"
    );
}

/// Fire the PostCompact hook (`rtk hook compact`) for a session.
fn rtk_compact(db: &PathBuf, session: &str) {
    let payload = format!("{{\"session_id\":\"{session}\",\"hook_event_name\":\"PostCompact\"}}");
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["hook", "compact"])
        .env("RTK_DB_PATH", db)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn rtk");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.as_bytes())
        .expect("write stdin");
    assert!(child.wait().expect("wait rtk").success());
}

#[test]
fn postcompact_reset_reemits_full() {
    let db = fresh_db("compact_reset");
    let input = big_input();

    rtk_read(&db, Some("sess-C"), &input); // full (records)
    let suppressed = rtk_read(&db, Some("sess-C"), &input);
    assert!(
        suppressed.contains("rtk-dedup:"),
        "second read should suppress"
    );

    // Compaction clears the session ledger...
    rtk_compact(&db, "sess-C");

    // ...so the next read re-emits full (the earlier output may be gone from context).
    let after = rtk_read(&db, Some("sess-C"), &input);
    assert!(
        !after.contains("rtk-dedup:"),
        "read after PostCompact reset must re-emit full, got a stub: {after}"
    );
}

#[test]
fn sessions_are_isolated() {
    let db = fresh_db("isolated");
    let input = big_input();

    let a = rtk_read(&db, Some("sess-X"), &input);
    // Same content, different session: its context never saw the first
    // emission, so it must not be suppressed.
    let b = rtk_read(&db, Some("sess-Y"), &input);
    assert!(!a.contains("rtk-dedup:"));
    assert!(
        !b.contains("rtk-dedup:"),
        "a different session must see full output, not a stub"
    );
}
