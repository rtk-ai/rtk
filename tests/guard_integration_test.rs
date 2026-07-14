//! End-to-end proof of the never-worse guard (src/core/guard.rs).

use std::io::Write;
use std::process::{Command, Stdio};

use rusqlite::Connection;
use sha2::{Digest, Sha256};

fn rtk_stdin(args: &[&str], input: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn rtk");
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
fn guard_shows_raw_when_filter_would_bloat_tiny_input() {
    let input = "{\"a\":1,\"b\":2,\"c\":3,\"d\":4}";
    let out = rtk_stdin(&["json", "-"], input);

    assert_eq!(
        out.trim(),
        input,
        "guard should emit the raw minified JSON, not a larger pretty-printed form"
    );
    assert!(
        out.trim().len() <= input.len(),
        "never-worse violated: {} chars emitted for a {}-char raw input",
        out.trim().len(),
        input.len()
    );
}

#[test]
fn guard_does_not_block_real_compression() {
    let mut input = String::from("{");
    for i in 0..60 {
        input.push_str(&format!("\"key_{i}\":\"value_{i}\","));
    }
    input.push_str("\"last\":1}");

    let out = rtk_stdin(&["json", "-"], &input);
    assert!(
        out.len() < input.len(),
        "filter should compress large input (guard must not over-trigger): {} vs {}",
        out.len(),
        input.len()
    );
}

fn rtk_output_in_dir(dir: &std::path::Path, args: &[&str]) -> (String, String, Option<i32>) {
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn rtk");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code(),
    )
}

fn rtk_output_in_dir_with_db(
    dir: &std::path::Path,
    db_path: &std::path::Path,
    args: &[&str],
) -> (String, String, Option<i32>) {
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(args)
        .current_dir(dir)
        .env("RTK_DB_PATH", db_path)
        .output()
        .expect("spawn rtk with isolated tracking DB");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code(),
    )
}

fn rtk_in_dir(dir: &std::path::Path, args: &[&str]) -> (String, Option<i32>) {
    let (stdout, _, code) = rtk_output_in_dir(dir, args);
    (stdout, code)
}

fn rg_available() -> bool {
    Command::new("rg")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn init_git_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "t@t.t"][..],
        &["config", "user.name", "t"][..],
        &["commit", "-q", "--allow-empty", "-m", "init"][..],
    ] {
        let ok = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(ok, "git setup failed: {args:?}");
    }
    dir
}

fn git_in_dir(dir: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git command failed: {args:?}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn git_stdout_in_dir(dir: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git command failed: {args:?}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn git_commit_reports_verified_object_hash_and_tracks_actual_output() {
    let dir = init_git_repo();
    git_in_dir(dir.path(), &["switch", "-qc", "feature/not-a-sha"]);
    let db_path = dir.path().join("tracking.db");

    let (stdout, stderr, code) = rtk_output_in_dir_with_db(
        dir.path(),
        &db_path,
        &["git", "commit", "--allow-empty", "-m", "change"],
    );

    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert!(
        stderr.is_empty(),
        "successful commit wrote stderr: {stderr:?}"
    );
    let expected_hash = git_stdout_in_dir(dir.path(), &["rev-parse", "--short=7", "HEAD"]);
    assert_eq!(stdout.trim(), format!("ok {expected_hash}"));
    assert!(!stdout.contains("feature/not-a-sha"));
    git_in_dir(
        dir.path(),
        &["cat-file", "-e", &format!("{expected_hash}^{{commit}}")],
    );

    let conn = Connection::open(&db_path).expect("open isolated tracking DB");
    let (exit_code, output_sha256): (Option<i32>, Option<String>) = conn
        .query_row(
            "SELECT exit_code, output_sha256 FROM commands ORDER BY id DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("tracked commit evidence");
    let expected_output_sha256 = format!("{:x}", Sha256::digest(stdout.trim().as_bytes()));
    assert_eq!(exit_code, Some(0));
    assert_eq!(
        output_sha256.as_deref(),
        Some(expected_output_sha256.as_str())
    );
}

#[test]
fn git_commit_noop_preserves_failure_head_and_tracking_exit_code() {
    let dir = init_git_repo();
    let db_path = dir.path().join("tracking.db");
    let before = git_stdout_in_dir(dir.path(), &["rev-parse", "HEAD"]);

    let (stdout, stderr, code) =
        rtk_output_in_dir_with_db(dir.path(), &db_path, &["git", "commit", "-m", "noop"]);

    assert_ne!(code, Some(0));
    assert!(
        stdout.trim().is_empty(),
        "failure must not emit success stdout: {stdout:?}"
    );
    assert!(
        stderr.contains("nothing to commit"),
        "native failure detail was lost: {stderr:?}"
    );
    assert!(
        !stderr.contains("ok "),
        "failure was rendered as success: {stderr:?}"
    );
    assert_eq!(
        before,
        git_stdout_in_dir(dir.path(), &["rev-parse", "HEAD"])
    );

    let conn = Connection::open(&db_path).expect("open isolated tracking DB");
    let tracked_exit_code: Option<i32> = conn
        .query_row(
            "SELECT exit_code FROM commands ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("tracked no-op evidence");
    assert_eq!(tracked_exit_code, code);
}

#[test]
fn git_status_migrates_legacy_tracking_schema_before_recording_evidence() {
    let dir = init_git_repo();
    let db_path = dir.path().join("legacy-tracking.db");
    let conn = Connection::open(&db_path).expect("create legacy tracking DB");
    conn.execute_batch(
        "CREATE TABLE commands (
            id INTEGER PRIMARY KEY,
            timestamp TEXT NOT NULL,
            original_cmd TEXT NOT NULL,
            rtk_cmd TEXT NOT NULL,
            input_tokens INTEGER NOT NULL,
            output_tokens INTEGER NOT NULL,
            saved_tokens INTEGER NOT NULL,
            savings_pct REAL NOT NULL
        );",
    )
    .expect("create legacy commands schema");
    drop(conn);

    let (_, stderr, code) =
        rtk_output_in_dir_with_db(dir.path(), &db_path, &["git", "status", "--short"]);
    assert_eq!(code, Some(0), "legacy schema migration failed: {stderr}");

    let conn = Connection::open(&db_path).expect("reopen migrated tracking DB");
    let mut columns = conn
        .prepare("PRAGMA table_info(commands)")
        .expect("prepare table info");
    let column_names = columns
        .query_map([], |row| row.get::<_, String>(1))
        .expect("query table info")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("collect columns");
    for expected in ["exit_code", "input_sha256", "output_sha256"] {
        assert!(
            column_names.iter().any(|name| name == expected),
            "missing migrated column: {expected}"
        );
    }
    let (exit_code, input_sha256, output_sha256): (Option<i32>, Option<String>, Option<String>) =
        conn.query_row(
            "SELECT exit_code, input_sha256, output_sha256 FROM commands ORDER BY id DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("migrated evidence row");
    assert_eq!(exit_code, Some(0));
    assert_eq!(input_sha256.as_deref().map(str::len), Some(64));
    assert_eq!(output_sha256.as_deref().map(str::len), Some(64));
}

fn read_text_normalized(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).unwrap().replace("\r\n", "\n")
}

#[test]
fn grep_no_match_emits_empty_not_a_message() {
    if !rg_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.txt"), "hello world\n").expect("write");
    // Faithful grep needs -r to descend a directory; we no longer force recursion
    // by routing through rg (the engine-faithful contract).
    let (out, code) = rtk_in_dir(dir.path(), &["grep", "-r", "zzz_no_match_xyz", "."]);
    assert!(
        out.trim().is_empty(),
        "no-match grep must emit empty, not a '0 matches' line: {out:?}"
    );
    assert_eq!(code, Some(1), "grep no-match must preserve exit 1");
}

#[test]
fn find_no_results_emits_empty() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.txt"), "x").expect("write");
    let (out, _) = rtk_in_dir(dir.path(), &["find", ".", "-name", "zzz_no_match_xyz"]);
    assert!(
        out.trim().is_empty(),
        "no-result find must emit empty, not a '0 for' line: {out:?}"
    );
}

#[test]
fn git_stash_list_no_stashes_emits_empty() {
    let dir = init_git_repo();
    let (out, code) = rtk_in_dir(dir.path(), &["git", "stash", "list"]);
    assert!(
        out.trim().is_empty(),
        "no-stashes must emit empty, not 'No stashes': {out:?}"
    );
    assert_eq!(code, Some(0));
}

#[test]
fn git_stash_show_no_stash_emits_empty_and_propagates_failure() {
    // Regression: previously printed "Empty stash" and returned Ok(0), masking
    // the underlying `git stash show` failure.
    let dir = init_git_repo();
    let (out, code) = rtk_in_dir(dir.path(), &["git", "stash", "show"]);
    assert!(
        out.trim().is_empty(),
        "must emit empty, not 'Empty stash': {out:?}"
    );
    assert_ne!(
        code,
        Some(0),
        "a real git stash show failure must not be masked as exit 0"
    );
}

#[test]
fn git_checkout_branch_switch_emits_compact_ok() {
    let dir = init_git_repo();
    git_in_dir(dir.path(), &["checkout", "-q", "-b", "feature/test"]);

    let (out, code) = rtk_in_dir(dir.path(), &["git", "checkout", "main"]);

    assert_eq!(code, Some(0));
    assert_eq!(out.trim(), "ok main");
}

#[test]
fn git_checkout_new_branch_emits_compact_ok() {
    let dir = init_git_repo();

    let (out, code) = rtk_in_dir(dir.path(), &["git", "checkout", "-b", "feature/test"]);

    assert_eq!(code, Some(0));
    assert_eq!(out.trim(), "ok feature/test (new)");
}

#[test]
fn git_checkout_reset_branch_does_not_claim_new_branch() {
    let dir = init_git_repo();

    let (out, code) = rtk_in_dir(dir.path(), &["git", "checkout", "-B", "feature/test"]);

    assert_eq!(code, Some(0));
    assert_eq!(out.trim(), "ok feature/test");
}

#[test]
fn git_checkout_file_restore_emits_restored_count() {
    let dir = init_git_repo();
    std::fs::write(dir.path().join("a.txt"), "original\n").expect("write a");
    std::fs::write(dir.path().join("b.txt"), "original\n").expect("write b");
    git_in_dir(dir.path(), &["add", "a.txt", "b.txt"]);
    git_in_dir(dir.path(), &["commit", "-q", "-m", "add files"]);

    std::fs::write(dir.path().join("a.txt"), "changed\n").expect("write a");
    std::fs::write(dir.path().join("b.txt"), "changed\n").expect("write b");

    let (out, code) = rtk_in_dir(
        dir.path(),
        &["git", "checkout", "HEAD", "--", "a.txt", "b.txt"],
    );

    assert_eq!(code, Some(0));
    assert!(
        out.trim().is_empty() || out.trim() == "ok 2 files restored",
        "guarded output may stay empty when native git emits no success text: {out:?}"
    );
    assert_eq!(
        read_text_normalized(&dir.path().join("a.txt")),
        "original\n"
    );
    assert_eq!(
        read_text_normalized(&dir.path().join("b.txt")),
        "original\n"
    );
}

#[test]
fn git_checkout_dirty_tree_error_keeps_file_list() {
    let dir = init_git_repo();
    std::fs::write(dir.path().join("a.txt"), "main\n").expect("write a");
    git_in_dir(dir.path(), &["add", "a.txt"]);
    git_in_dir(dir.path(), &["commit", "-q", "-m", "add a"]);
    git_in_dir(dir.path(), &["checkout", "-q", "-b", "feature/test"]);
    std::fs::write(dir.path().join("a.txt"), "feature\n").expect("write feature");
    git_in_dir(dir.path(), &["commit", "-am", "feature change"]);
    git_in_dir(dir.path(), &["checkout", "-q", "main"]);
    std::fs::write(dir.path().join("a.txt"), "dirty\n").expect("write dirty");

    let (stdout, stderr, code) =
        rtk_output_in_dir(dir.path(), &["git", "checkout", "feature/test"]);
    let combined = format!("{stdout}{stderr}");

    assert_ne!(code, Some(0));
    assert!(
        combined.contains("error:"),
        "dirty checkout failure should keep error header: {combined:?}"
    );
    assert!(
        combined.contains("a.txt"),
        "dirty checkout failure should keep conflicting filename: {combined:?}"
    );
    assert!(
        combined.contains("Aborting"),
        "dirty checkout failure should keep abort line: {combined:?}"
    );
}
