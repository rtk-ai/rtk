#![cfg(unix)]
//! A grouped search must only advertise per-file recovery that survived the
//! run's storage limits, and a storage failure must leave the matches visible.

use std::path::Path;
use std::process::{Command, Output};

mod common;

fn fixture(root: &Path, max_entries: usize, mode: &str) {
    std::fs::create_dir(root.join("corpus")).expect("corpus");
    for i in 0..4 {
        let text: String = (1..=60)
            .map(|line| format!("needle file {i} line {line:02}\n"))
            .collect();
        std::fs::write(root.join(format!("corpus/file_{i}.txt")), text).expect("file");
    }
    for config in [".config/rtk", "Library/Application Support/rtk"] {
        let dir = root.join(config);
        std::fs::create_dir_all(&dir).expect("config dir");
        std::fs::write(
            dir.join("config.toml"),
            format!("[retriever]\nmode = \"{mode}\"\nmax_entries = {max_entries}\n"),
        )
        .expect("config");
    }
}

fn rtk(root: &Path) -> Command {
    let mut command = common::rtk_command();
    command
        .current_dir(root)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join(".config"))
        .env("RTK_RECALL_DB", root.join("recall.db"))
        .env("RTK_DB_PATH", root.join("history.db"))
        .env("RTK_TEE_DIR", root.join("tee"));
    command
}

fn search(root: &Path, max: &str) -> Output {
    rtk(root)
        .args(["grep", "--max", max, "-rn", "needle", "corpus"])
        .output()
        .expect("grep")
}

fn hints(output: &str) -> Vec<&str> {
    output
        .split("rtk recall ")
        .skip(1)
        .map(|tail| tail.split(']').next().expect("hash"))
        .collect()
}

fn assert_recall(root: &Path, hash: &str, file: usize, start: usize) {
    let recalled = rtk(root).args(["recall", hash]).output().expect("recall");
    assert!(recalled.status.success(), "hint must remain recoverable");
    let expected: String = (start..=60)
        .map(|line| format!("corpus/file_{file}.txt:{line}:needle file {file} line {line:02}\n"))
        .collect();
    assert_eq!(String::from_utf8_lossy(&recalled.stdout), expected);
}

#[test]
fn batched_search_preserves_per_file_delta_hints() {
    let dir = tempfile::tempdir().expect("tempdir");
    fixture(dir.path(), 200, "sqlite");
    let result = search(dir.path(), "100");
    assert!(result.status.success());
    let output = String::from_utf8_lossy(&result.stdout);
    let hashes = hints(&output);
    assert_eq!(hashes.len(), 4, "{output}");
    assert_eq!(output.matches("[+35 hidden:").count(), 4);
    for (i, hash) in hashes.iter().enumerate() {
        assert_recall(dir.path(), hash, i, 26);
    }
}

#[test]
fn batched_search_does_not_advertise_rows_evicted_by_its_own_cap() {
    let dir = tempfile::tempdir().expect("tempdir");
    fixture(dir.path(), 2, "sqlite");
    let result = search(dir.path(), "100");
    assert!(result.status.success());
    let output = String::from_utf8_lossy(&result.stdout);
    let hashes = hints(&output);
    assert_eq!(
        hashes.len(),
        2,
        "only the retained rows can have hints: {output}"
    );
    assert_recall(dir.path(), hashes[0], 2, 26);
    assert_recall(dir.path(), hashes[1], 3, 26);
    assert!(output.contains("+35 more in corpus/file_0.txt\n"));
    assert!(output.contains("+35 more in corpus/file_1.txt\n"));
}

#[test]
fn batched_search_keeps_skipped_file_recovery_separate() {
    let dir = tempfile::tempdir().expect("tempdir");
    fixture(dir.path(), 200, "sqlite");
    let result = search(dir.path(), "50");
    assert!(result.status.success());
    let output = String::from_utf8_lossy(&result.stdout);
    let hashes = hints(&output);
    assert_eq!(hashes.len(), 3, "{output}");
    assert_recall(dir.path(), hashes[0], 0, 26);
    assert_recall(dir.path(), hashes[1], 1, 26);
    assert!(output.contains("+2 more files [+120 hidden:"));
    let recalled = rtk(dir.path())
        .args(["recall", hashes[2]])
        .output()
        .expect("recall");
    assert!(recalled.status.success());
    let text = String::from_utf8_lossy(&recalled.stdout);
    assert_eq!(text.lines().count(), 120);
    assert!(text.starts_with("corpus/file_2.txt:1:"));
    assert!(text.ends_with("corpus/file_3.txt:60:needle file 3 line 60\n"));
}

#[test]
fn batched_search_fails_open_when_storage_is_unavailable() {
    let dir = tempfile::tempdir().expect("tempdir");
    fixture(dir.path(), 200, "sqlite");
    std::fs::create_dir(dir.path().join("recall.db")).expect("unwritable database");
    let result = search(dir.path(), "100");
    assert!(result.status.success());
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(hints(&output).is_empty());
    assert!(output.contains("needle file 0 line 01"));
    assert_eq!(output.matches("+35 more in").count(), 4);
}

#[test]
fn batched_search_respects_disabled_recovery() {
    let dir = tempfile::tempdir().expect("tempdir");
    fixture(dir.path(), 200, "disabled");
    let result = search(dir.path(), "100");
    assert!(result.status.success());
    assert!(hints(&String::from_utf8_lossy(&result.stdout)).is_empty());
    assert!(!dir.path().join("recall.db").exists());
}

#[test]
fn batched_budget_hints_and_actual_recall_costs_stay_consistent() {
    let dir = tempfile::tempdir().expect("directory");
    fixture(dir.path(), 200, "sqlite");
    for config in [".config/rtk", "Library/Application Support/rtk"] {
        let path = dir.path().join(config).join("config.toml");
        let mut text = std::fs::read_to_string(&path).expect("config");
        text.push_str("compression = false\nmax_total_bytes = 6000\n");
        std::fs::write(path, text).expect("bounded config");
    }
    let result = search(dir.path(), "100");
    assert!(result.status.success());
    let output = String::from_utf8(result.stdout).expect("output");
    let references = hints(&output);
    assert_eq!(references.len(), 2, "{output}");
    assert_eq!(
        output
            .matches(" (unfiltered; may contain sensitive data)")
            .count(),
        references.len()
    );
    assert!(references.iter().all(|r| r.starts_with("c_")));
    let recall = rusqlite::Connection::open(dir.path().join("recall.db")).expect("recall DB");
    let (entries, bytes): (i64, i64) = recall
        .query_row("SELECT entries, blob_bytes FROM recall_storage", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .expect("totals");
    assert_eq!(entries, 2);
    assert!(bytes <= 6000);
    assert_eq!(
        bytes,
        recall
            .query_row("SELECT SUM(length(blob)) FROM recall", [], |r| r
                .get::<_, i64>(0))
            .expect("actual bytes")
    );
    let history = rusqlite::Connection::open(dir.path().join("history.db")).expect("history DB");
    let (invocation, output_tokens): (String, i64) = history
        .query_row(
            "SELECT invocation_id, output_tokens FROM commands ORDER BY id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("command");
    assert_eq!(
        output_tokens as usize,
        output.len().div_ceil(4),
        "disclosure and reference bytes count toward initial output"
    );
    let mut retrieved_tokens = 0;
    for reference in &references {
        let read = rtk(dir.path())
            .args(["recall", reference, "--grep", "line 2[67]"])
            .output()
            .expect("filtered recall");
        assert!(read.status.success());
        assert_eq!(String::from_utf8_lossy(&read.stdout).lines().count(), 2);
        retrieved_tokens += read.stdout.len().div_ceil(4);
        let (source, capture, bytes): (String, String, i64) = history.query_row(
            "SELECT source_invocation_id, source_capture_id, emitted_bytes FROM recall_events ORDER BY rowid DESC LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        ).expect("recall event");
        assert_eq!(source, invocation);
        assert_eq!(capture, reference[2..]);
        assert_eq!(bytes as usize, read.stdout.len());
    }
    let exported = rtk(dir.path())
        .args(["gain", "--history", "--format", "json"])
        .output()
        .expect("accounting");
    assert!(exported.status.success());
    let json: serde_json::Value = serde_json::from_slice(&exported.stdout).expect("json");
    assert_eq!(
        json["output_accounting"]["retrieved_output_tokens"],
        retrieved_tokens
    );
    assert_eq!(json["output_accounting"]["unattributed_recall_events"], 0);
    drop(recall);
    let purged = rtk(dir.path())
        .args(["recall", "--purge"])
        .output()
        .expect("purge");
    assert!(
        purged.status.success(),
        "{}",
        String::from_utf8_lossy(&purged.stderr)
    );
    let recall = rusqlite::Connection::open(dir.path().join("recall.db")).expect("recall DB");
    assert_eq!(
        recall
            .query_row("SELECT COUNT(*) FROM recall_captures", [], |r| r
                .get::<_, i64>(0))
            .expect("captures"),
        0
    );
    assert_eq!(
        recall
            .query_row("SELECT SUM(elisions) FROM recall_stats", [], |r| r
                .get::<_, i64>(0))
            .expect("statistics"),
        4
    );
    drop(recall);
    assert!(search(dir.path(), "100").status.success());
    for reference in references {
        let expired = rtk(dir.path())
            .args(["recall", reference])
            .output()
            .expect("expired reference");
        assert!(!expired.status.success());
        assert!(expired.stdout.is_empty());
    }
}

#[test]
fn forget_erases_batched_payloads_and_captures() {
    let dir = tempfile::tempdir().expect("directory");
    fixture(dir.path(), 200, "sqlite");
    let result = search(dir.path(), "100");
    assert!(result.status.success());
    assert!(dir.path().join("recall.db").exists());
    let forgotten = rtk(dir.path())
        .args(["telemetry", "forget"])
        .output()
        .expect("forget");
    assert!(
        forgotten.status.success(),
        "{}",
        String::from_utf8_lossy(&forgotten.stderr)
    );
    for name in [
        "recall.db",
        "recall.db-wal",
        "recall.db-shm",
        "recall.db-journal",
    ] {
        assert!(!dir.path().join(name).exists(), "{name}");
    }
}
